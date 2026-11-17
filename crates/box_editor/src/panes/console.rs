//! Console pane: captured log records with severity chips, a text filter,
//! Clear and virtualised rows.
//!
//! Rows can be selected (click, Cmd-click toggles, Shift-click a range) and
//! copied: Cmd+C copies the selected rows, the context menu copies them or
//! every shown row. Hovering a row shows the whole record. Showing the
//! Console clears the unseen-warnings badge on its tab.

use egui::{Align, Align2, Layout, Margin, Sense, Stroke, Ui, UiBuilder, pos2, vec2};

use crate::log::{LogRecord, Severity};
use crate::state::{ConsoleState, EditorState};
use crate::theme::tokens;
use crate::widgets;

const ROW_H: f32 = 19.0;

fn rebuild_rows(state: &mut EditorState) {
    let c = &mut state.console;
    let inner = c.log.lock();
    if !c.dirty && c.built_generation == Some(inner.generation()) {
        return;
    }
    let filter = c.filter.to_lowercase();
    c.rows.clear();
    c.rows.extend(
        inner
            .records()
            .iter()
            .filter(|r| {
                c.show[Severity::of(r.level) as usize]
                    && (filter.is_empty()
                        || r.message.to_lowercase().contains(&filter)
                        || r.target.to_lowercase().contains(&filter))
            })
            .map(|r| r.seq),
    );
    c.built_generation = Some(inner.generation());
    c.dirty = false;
    c.visible_range = None;
}

/// Copies the records of the visible rows out of the log, so that painting
/// them does not hold the log lock: the tracing layer pushes into the same
/// (non-reentrant) mutex, and a log emitted on this thread while rows are
/// painted would otherwise deadlock. Only `range` is copied, and only when
/// it or the row list changed.
fn snapshot_visible(c: &mut ConsoleState, range: std::ops::Range<usize>) {
    if c.visible_range.as_ref() == Some(&range) {
        return;
    }
    let inner = c.log.lock();
    c.visible.clear();
    c.visible.extend(
        c.rows[range.clone()]
            .iter()
            .map(|seq| inner.get(*seq).cloned()),
    );
    drop(inner);
    c.visible_range = Some(range);
}

/// One record as copied text: time, level, target and the full message.
pub fn record_text(r: &LogRecord) -> String {
    format!(
        "{:>9.3} {:<5} {} {}",
        r.time,
        r.level.as_str().to_lowercase(),
        r.target,
        r.message
    )
}

impl ConsoleState {
    /// The records `seqs` (in log order) as text, one per line.
    pub fn copy_text(&self, seqs: impl IntoIterator<Item = u64>) -> String {
        let inner = self.log.lock();
        let lines: Vec<String> = seqs
            .into_iter()
            .filter_map(|s| inner.get(s).map(record_text))
            .collect();
        lines.join("\n")
    }
}

/// What a row asked for this frame.
enum RowOp {
    Click { seq: u64, toggle: bool, range: bool },
    CopySelected,
    CopyAll,
}

pub fn console_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    let width = ui.available_width();
    let counts = state.console.log.lock().counts();
    // Shown: nothing is unseen any more.
    state.console.unseen = [0; 2];
    state.toasts.clear_overflow();
    let pane = ui.max_rect();

    // Filter bar.
    let (bar, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    let mut bui = ui.new_child(
        UiBuilder::new()
            .max_rect(bar.shrink2(vec2(t.pad_x - 2.0, 5.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    bui.spacing_mut().item_spacing.x = 2.0;
    for (i, (label, dot)) in [
        ("Info", t.text_weak),
        ("Warnings", t.warn),
        ("Errors", t.error),
    ]
    .into_iter()
    .enumerate()
    {
        let on = state.console.show[i];
        if widgets::chip(&mut bui, label, counts[i], dot, on).clicked() {
            state.console.show[i] = !on;
            state.console.dirty = true;
        }
    }
    bui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui
            .add(egui::Button::new(egui::RichText::new("Clear").color(t.text)).frame(false))
            .clicked()
        {
            state.console.log.clear();
        }
        ui.add_space(6.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut state.console.filter)
                .hint_text("Filter")
                .font(t.body())
                .margin(Margin::symmetric(8, 3))
                .desired_width(180.0),
        );
        if r.changed() {
            state.console.dirty = true;
        }
    });
    let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.top(), Stroke::new(1.0, t.border));

    rebuild_rows(state);

    ui.spacing_mut().item_spacing.y = 0.0;
    let mono = t.mono();
    let console = &mut state.console;
    let mut area = egui::ScrollArea::vertical()
        .id_salt("console")
        .auto_shrink([false, false])
        .stick_to_bottom(true);
    if let Some(seq) = console.scroll_to.take()
        && let Ok(i) = console.rows.binary_search(&seq)
    {
        let rows_shown = (ui.available_height() / ROW_H).floor().max(1.0);
        let top = (i as f32 - (rows_shown / 2.0).floor()).max(0.0);
        area = area.vertical_scroll_offset(top * ROW_H);
    }
    let mut ops = Vec::new();
    area.show_rows(ui, ROW_H, console.rows.len(), |ui, range| {
        snapshot_visible(console, range);
        // The log lock is not held from here on.
        for rec in &console.visible {
            let (rect, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            let Some(rec) = rec else {
                continue;
            };
            let p = ui.painter();
            let selected = console.selected.contains(&rec.seq);
            if selected {
                p.rect_filled(rect, 0.0, t.accent_bg);
            } else if resp.hovered() {
                p.rect_filled(rect, 0.0, t.hover);
            }
            let cy = rect.center().y;
            let mut x = rect.left() + t.pad_x + 2.0;
            p.text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                format!("{:>9.3}", rec.time),
                mono.clone(),
                t.text_faint,
            );
            x += 72.0;
            let severity = Severity::of(rec.level);
            let level_col = match severity {
                Severity::Info => t.text_faint,
                Severity::Warn => t.warn,
                Severity::Error => t.error,
            };
            p.text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                rec.level.as_str().to_lowercase(),
                mono.clone(),
                level_col,
            );
            x += 46.0;
            let target_rect =
                egui::Rect::from_min_max(pos2(x, rect.top()), pos2(x + 164.0, rect.bottom()));
            p.with_clip_rect(target_rect.intersect(rect)).text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                &rec.target,
                mono.clone(),
                t.text_weak,
            );
            x += 172.0;
            let msg_col = if selected {
                t.accent_text
            } else if severity == Severity::Error {
                t.error_text
            } else {
                t.text
            };
            p.with_clip_rect(rect).text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                rec.message.lines().next().unwrap_or_default(),
                mono.clone(),
                msg_col,
            );
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                ops.push(RowOp::Click {
                    seq: rec.seq,
                    toggle: m.command,
                    range: m.shift,
                });
            }
            if resp.secondary_clicked() && !selected {
                ops.push(RowOp::Click {
                    seq: rec.seq,
                    toggle: false,
                    range: false,
                });
            }
            let resp = resp.on_hover_ui(|ui| {
                ui.set_max_width(560.0);
                ui.label(
                    egui::RichText::new(format!("{}  {}", rec.level, rec.target))
                        .font(mono.clone())
                        .color(t.text_weak),
                );
                ui.label(egui::RichText::new(&rec.message).font(mono.clone()));
            });
            resp.context_menu(|ui| {
                if ui.button("Copy").clicked() {
                    ops.push(RowOp::CopySelected);
                }
                if ui.button("Copy All").clicked() {
                    ops.push(RowOp::CopyAll);
                }
            });
        }
    });

    // Keyboard copy belongs to the Console after a click in it, until a
    // click elsewhere.
    let (pressed_outside, copy) = ui.input(|i| {
        (
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| !pane.contains(p)),
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Copy)
                    || matches!(e, egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. } if modifiers.command)
            }),
        )
    });
    if pressed_outside {
        console.focused = false;
    }
    if copy && console.focused && !ui.ctx().egui_wants_keyboard_input() {
        ops.push(RowOp::CopySelected);
    }
    for op in ops {
        match op {
            RowOp::Click { seq, toggle, range } => {
                console.focused = true;
                click(console, seq, toggle, range);
            }
            RowOp::CopySelected => {
                let text = console.copy_text(console.selected.iter().copied());
                if !text.is_empty() {
                    ui.ctx().copy_text(text);
                }
            }
            RowOp::CopyAll => {
                let text = console.copy_text(console.rows.iter().copied());
                ui.ctx().copy_text(text);
            }
        }
    }
}

/// A row click: select it alone, toggle it (Cmd), or select the shown rows
/// from the anchor to it (Shift).
fn click(c: &mut ConsoleState, seq: u64, toggle: bool, range: bool) {
    if range && let Some(anchor) = c.anchor {
        let (a, b) = (anchor.min(seq), anchor.max(seq));
        let rows = c.rows.iter().copied().filter(|s| (a..=b).contains(s));
        if !toggle {
            c.selected.clear();
        }
        c.selected.extend(rows);
        return;
    }
    if toggle {
        if !c.selected.remove(&seq) {
            c.selected.insert(seq);
        }
    } else {
        c.selected.clear();
        c.selected.insert(seq);
    }
    c.anchor = Some(seq);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::LogBuffer;
    use box_core::prelude::World;

    #[test]
    fn only_the_visible_rows_are_copied_out_of_the_log() {
        let log = LogBuffer::default();
        for i in 0..5_000 {
            log.push(tracing::Level::INFO, "test", format!("record {i}"));
        }
        let mut state = EditorState::new(World::new(), log.clone());
        let ctx = egui::Context::default();
        let frame = |state: &mut EditorState| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(600.0, 300.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show(ui, |ui| console_ui(ui, state));
            });
        };
        frame(&mut state);
        frame(&mut state);
        assert_eq!(state.console.rows.len(), 5_000);
        let visible = state.console.visible.len();
        assert!(
            (1..=32).contains(&visible),
            "{visible} rows copied for a 300 pt tall pane"
        );
        assert!(state.console.visible.iter().all(Option::is_some));
        // Stuck to the bottom: the newest record is among them.
        let last = state.console.visible.last().unwrap().as_ref().unwrap();
        assert_eq!(last.message, "record 4999");

        // Unchanged log and scroll position: nothing is copied again.
        let range = state.console.visible_range.clone();
        state.console.visible.clear();
        frame(&mut state);
        assert_eq!(state.console.visible_range, range);
        assert!(state.console.visible.is_empty(), "snapshot reused");

        // A new record refreshes it.
        log.push(tracing::Level::WARN, "test", "newest".into());
        frame(&mut state);
        frame(&mut state);
        let last = state.console.visible.last().unwrap().as_ref().unwrap();
        assert_eq!(last.message, "newest");
    }

    /// Click a row, Shift-click another, Cmd+C: both records are copied as
    /// text; the context menu's Copy All copies every shown row.
    #[test]
    fn rows_are_selected_by_clicks_and_copied() {
        let log = LogBuffer::default();
        log.set_manual_time(1.25);
        log.push(tracing::Level::INFO, "box_editor", "first".into());
        log.push(tracing::Level::WARN, "box_assets", "second\nmore".into());
        log.push(tracing::Level::ERROR, "box_editor", "third".into());
        let mut state = EditorState::new(World::new(), log.clone());
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut frame = |state: &mut EditorState, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    vec2(600.0, 300.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            };
            time += 1.0 / 60.0;
            ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default().show(ui, |ui| console_ui(ui, state));
            })
        };
        frame(&mut state, Vec::new());
        // Rows start below the 32 pt filter bar and its hairline, inside the
        // panel's 8 pt margin.
        let row_y = |i: usize| 8.0 + 33.0 + ROW_H * (i as f32 + 0.5);
        let click = |pos: egui::Pos2, modifiers: egui::Modifiers| {
            vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                },
            ]
        };
        frame(
            &mut state,
            click(pos2(200.0, row_y(0)), egui::Modifiers::NONE),
        );
        assert_eq!(
            state.console.selected().iter().copied().collect::<Vec<_>>(),
            vec![0]
        );
        frame(
            &mut state,
            click(pos2(200.0, row_y(1)), egui::Modifiers::SHIFT),
        );
        assert_eq!(
            state.console.selected().iter().copied().collect::<Vec<_>>(),
            vec![0, 1]
        );
        let out = frame(&mut state, vec![egui::Event::Copy]);
        let copied: Vec<String> = out
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            copied,
            vec![
                "    1.250 info  box_editor first\n    1.250 warn  box_assets second\nmore"
                    .to_string()
            ]
        );
        assert_eq!(
            state
                .console
                .copy_text(state.console.rows.iter().copied())
                .lines()
                .count(),
            4,
            "Copy All: every shown record, multi-line messages whole"
        );
        // A click outside the Console takes keyboard copy away from it.
        frame(&mut state, click(pos2(200.0, 290.0), egui::Modifiers::NONE));
        let _ = frame(&mut state, Vec::new());
    }
}
