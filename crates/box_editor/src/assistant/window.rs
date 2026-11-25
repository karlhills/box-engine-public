//! The Assistant window: transcript, context line, message box, Send and
//! Stop, Undo Turn.

use egui::{Align2, Context, Key, RichText, ScrollArea, TextEdit, vec2};

use super::{TurnItem, TurnStatus};
use crate::state::EditorState;
use crate::theme::Tokens;

/// Requests offered while the transcript is empty (they fill the box).
const SUGGESTIONS: &[&str] = &[
    "Make a small game: a player, 8 coins to collect and a goal that wins once they are all collected",
    "Add a moving platform here that carries the player across",
    "Why can't the player jump onto the selected ledge?",
];

/// Where a floating window first appears: `offset` from a corner of the
/// screen. A start position, not an anchor, so the window can be dragged.
pub(crate) fn start_at(ctx: &Context, corner: Align2, offset: egui::Vec2) -> egui::Pos2 {
    let r = ctx.content_rect();
    let base = match corner {
        Align2::RIGHT_TOP => r.right_top(),
        Align2::RIGHT_BOTTOM => r.right_bottom(),
        Align2::LEFT_BOTTOM => r.left_bottom(),
        _ => r.left_top(),
    };
    base + offset
}

/// Draws the window when open and polls the running turn either way.
pub(crate) fn assistant_window(ctx: &Context, state: &mut EditorState, t: &Tokens) {
    if state.poll_assistant() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if !state.assistant.open {
        return;
    }
    let mut open = true;
    let mut send = false;
    let mut stop = false;
    let mut undo: Option<usize> = None;
    let status = state.assistant_backend_status();
    let busy = state.assistant_busy();
    let context = context_line(state);
    egui::Window::new("Assistant")
        .id(egui::Id::unique("box_editor_assistant"))
        .open(&mut open)
        .collapsible(false)
        .fade_in(false)
        .fade_out(false)
        .resizable(true)
        .default_size(vec2(440.0, 560.0))
        .default_pos(start_at(ctx, Align2::RIGHT_TOP, vec2(-340.0 - 440.0, 60.0)))
        .show(ctx, |ui| {
            match &status {
                Ok(name) => ui.label(
                    RichText::new(format!(
                        "{name} builds in this project with the box tools. Each request is one \
                         undo step."
                    ))
                    .color(t.text_weak)
                    .small(),
                ),
                Err(e) => ui.label(RichText::new(e).color(t.warn)),
            };
            ui.add_space(4.0);
            let input_h = 92.0;
            let transcript_h = (ui.available_height() - input_h - 40.0).max(120.0);
            ScrollArea::vertical()
                .id_salt("assistant_transcript")
                .max_height(transcript_h)
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if state.assistant.turns.is_empty() {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("Describe what to build or change. For example:")
                                .color(t.text_weak),
                        );
                        for s in SUGGESTIONS {
                            if ui
                                .add(egui::Button::new(RichText::new(*s).small()).wrap())
                                .clicked()
                            {
                                state.assistant.input = (*s).to_string();
                            }
                        }
                    }
                    for (i, turn) in state.assistant.turns.iter().enumerate() {
                        ui.add_space(10.0);
                        egui::Frame::new()
                            .fill(t.accent_bg)
                            .corner_radius(6.0)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(RichText::new(&turn.prompt).color(t.text_strong));
                            });
                        for item in &turn.items {
                            match item {
                                TurnItem::Text(text) => {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(plain_text(text)).color(t.text));
                                }
                                TurnItem::Step {
                                    label, ok, detail, ..
                                } => {
                                    let (mark, color) = match ok {
                                        None => ("…", t.text_weak),
                                        Some(true) => ("✓", t.accent),
                                        Some(false) => ("✗", t.error),
                                    };
                                    let r = ui.label(
                                        RichText::new(format!("{mark} {label}"))
                                            .color(color)
                                            .small(),
                                    );
                                    if !detail.is_empty() {
                                        r.on_hover_text(detail);
                                    }
                                }
                            }
                        }
                        let note = match &turn.status {
                            TurnStatus::Running => Some(("Working…".to_string(), t.text_weak)),
                            TurnStatus::Done => None,
                            TurnStatus::Stopped => {
                                Some(("Stopped. What it changed stays.".to_string(), t.warn))
                            }
                            TurnStatus::Failed(e) => Some((format!("Failed: {e}"), t.error)),
                            TurnStatus::Undone => Some(("Undone.".to_string(), t.text_weak)),
                        };
                        if let Some((text, color)) = note {
                            ui.label(RichText::new(text).color(color).small());
                        }
                        if !matches!(turn.status, TurnStatus::Running | TurnStatus::Undone)
                            && (turn.undo_steps > 0 || !turn.files.is_empty())
                        {
                            let can = state.assistant_can_undo(i);
                            let b = ui.add_enabled(
                                can.is_ok(),
                                egui::Button::new(RichText::new("Undo Turn").small()),
                            );
                            let b = match &can {
                                Ok(()) => b.on_hover_text(
                                    "Revert this request's scene edits and the files it wrote",
                                ),
                                Err(why) => b.on_disabled_hover_text(why),
                            };
                            if b.clicked() {
                                undo = Some(i);
                            }
                        }
                    }
                });
            ui.separator();
            ui.label(RichText::new(context).color(t.text_faint).small());
            let edit = TextEdit::multiline(&mut state.assistant.input)
                .frame(egui::Frame::NONE)
                .hint_text("Describe what to build or change…  (Cmd+Enter sends)")
                .desired_rows(3)
                .desired_width(f32::INFINITY);
            let r = egui::Frame::new()
                .fill(t.bg_app)
                .stroke(egui::Stroke::new(1.0, t.border))
                .corner_radius(6.0)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.add_sized(vec2(ui.available_width(), input_h - 42.0), edit)
                })
                .inner;
            if r.has_focus()
                && ui.input(|i| i.key_pressed(Key::Enter) && i.modifiers.command)
                && !busy
            {
                send = true;
            }
            ui.horizontal(|ui| {
                if busy {
                    if ui
                        .button("Stop")
                        .on_hover_text("Stop now; changes so far stay")
                        .clicked()
                    {
                        stop = true;
                    }
                    ui.spinner();
                } else {
                    let playing = state.is_playing();
                    let can_send =
                        status.is_ok() && !playing && !state.assistant.input.trim().is_empty();
                    let b = ui.add_enabled(can_send, egui::Button::new("Send"));
                    if playing {
                        b.on_disabled_hover_text(
                            "Stop Play first: changes made during Play are thrown away at Stop",
                        );
                        ui.label(
                            RichText::new("Stop Play to send")
                                .color(t.text_weak)
                                .small(),
                        );
                    } else if b.clicked() {
                        send = true;
                    }
                }
            });
        });
    if !open {
        state.assistant.open = false;
    }
    if stop {
        state.assistant_stop();
    }
    if let Some(i) = undo
        && let Err(e) = state.assistant_undo(i)
    {
        tracing::warn!("Undo Turn: {e}");
    }
    if send {
        let text = state.assistant.input.clone();
        match state.assistant_send(&text) {
            Ok(()) => state.assistant.input.clear(),
            Err(e) => tracing::warn!("assistant: {e}"),
        }
    }
}

/// Agent text without Markdown marks (the panel shows plain text):
/// heading hashes, bold and code marks go, list dashes become bullets.
pub(crate) fn plain_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let line = if trimmed.starts_with('#') {
                trimmed.trim_start_matches('#').trim_start()
            } else {
                line
            };
            let line = line.replace("**", "").replace("__", "").replace('`', "");
            match line.strip_prefix("- ") {
                Some(rest) => format!("• {rest}"),
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// "Scene main · Selected: Platform 2 · Edit".
fn context_line(state: &EditorState) -> String {
    let scene = state
        .scene
        .path
        .as_deref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .map_or("Unsaved scene".to_string(), |s| {
            format!("Scene {}", s.trim_end_matches(".scene"))
        });
    let names: Vec<String> = state
        .selection
        .iter()
        .take(3)
        .filter_map(|u| state.world.entity_by_uid(*u))
        .filter_map(|e| {
            state
                .world
                .get::<box_core::identity::Name>(e)
                .map(|n| n.0.clone())
        })
        .collect();
    let selected = match (names.len(), state.selection.len()) {
        (0, _) => "nothing selected".to_string(),
        (_, n) if n > 3 => format!("Selected: {} +{}", names.join(", "), n - 3),
        _ => format!("Selected: {}", names.join(", ")),
    };
    let mode = if state.is_playing() { "Play" } else { "Edit" };
    format!("{scene} · {selected} · {mode}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn markdown_becomes_plain_text() {
        assert_eq!(
            super::plain_text("## Done\n\n- **Player** at `main`\nok"),
            "Done\n\n• Player at main\nok"
        );
    }
}
