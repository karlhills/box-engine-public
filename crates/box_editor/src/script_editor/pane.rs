//! The Script pane's UI: file tabs, the confirm and changed-on-disk bars,
//! the find bar, and the text with a line-number margin.

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, RichText, ScrollArea, TextEdit, Ui, vec2};

use super::highlight::{Palette, layout_job};
use crate::state::EditorState;
use crate::theme::tokens;

fn palette(t: &crate::theme::Tokens) -> Palette {
    Palette {
        plain: t.text,
        keyword: Color32::from_rgb(198, 120, 221),
        constant: Color32::from_rgb(209, 154, 102),
        string: Color32::from_rgb(152, 195, 121),
        number: Color32::from_rgb(209, 154, 102),
        comment: t.text_faint,
        function: Color32::from_rgb(97, 175, 239),
        find: t.accent_bg,
    }
}

/// Byte ranges of `needle` in `text`, ignoring ASCII case.
fn matches(text: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let hay = text.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = hay[from..].find(&n) {
        let s = from + p;
        out.push((s, s + n.len()));
        from = s + n.len().max(1);
    }
    out
}

/// The char index where 1-based `line` starts.
fn line_start(text: &str, line: u32) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut seen = 1;
    for (ci, c) in text.chars().enumerate() {
        if c == '\n' {
            seen += 1;
            if seen == line {
                return ci + 1;
            }
        }
    }
    text.chars().count()
}

/// The pane (a dock tab beside the Viewport).
pub(crate) fn script_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    state.check_scripts_on_disk();
    if state.scripts.tabs.is_empty() {
        state.scripts.focused = false;
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No script open").color(t.text_strong));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "Double-click a .luau file in Assets, press Edit on a Script component, \
                     or double-click a script error in the Console.",
                )
                .color(t.text_weak),
            );
        });
        return;
    }
    let mut close: Option<usize> = None;
    let mut save: Option<usize> = None;
    let mut external = false;
    // Tabs and file actions.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        for i in 0..state.scripts.tabs.len() {
            let tab = &state.scripts.tabs[i];
            let label = if tab.dirty() {
                format!("{} •", tab.name())
            } else {
                tab.name().to_string()
            };
            let r = ui
                .selectable_label(i == state.scripts.active, label)
                .on_hover_text(format!("assets/{}", tab.path));
            if r.clicked() {
                state.scripts.active = i;
            }
            if ui.small_button("×").on_hover_text("Close").clicked() {
                close = Some(i);
            }
            ui.add_space(6.0);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            if ui
                .button("Open in Code Editor")
                .on_hover_text(
                    "Open this file in VS Code (or your default app) at the cursor's line",
                )
                .clicked()
            {
                external = true;
            }
            let dirty = state.scripts.tabs[state.scripts.active].dirty();
            if ui
                .add_enabled(dirty, egui::Button::new("Save"))
                .on_hover_text("Save the file (Cmd+S); running scripts reload")
                .clicked()
            {
                save = Some(state.scripts.active);
            }
        });
    });
    if let Some(i) = close {
        if state.scripts.tabs[i].dirty() {
            state.scripts.confirm_close = Some(i);
        } else {
            state.close_script(i);
            return;
        }
    }
    let active = state.scripts.active.min(state.scripts.tabs.len() - 1);
    state.scripts.active = active;

    // Bars: confirm close, changed on disk, errors of the last save.
    if let Some(i) = state
        .scripts
        .confirm_close
        .filter(|i| *i < state.scripts.tabs.len())
    {
        let name = state.scripts.tabs[i].name().to_string();
        let mut choice = None;
        bar(ui, &t, t.warn, |ui| {
            ui.label(RichText::new(format!("Save changes to {name}?")).color(t.text_strong));
            if ui.button("Save").clicked() {
                choice = Some(0);
            }
            if ui.button("Don't Save").clicked() {
                choice = Some(1);
            }
            if ui.button("Cancel").clicked() {
                choice = Some(2);
            }
        });
        match choice {
            Some(0) => match state.save_script(i) {
                Ok(()) => state.close_script(i),
                Err(e) => tracing::warn!("{e}"),
            },
            Some(1) => state.close_script(i),
            Some(2) => state.scripts.confirm_close = None,
            _ => {}
        }
        if state.scripts.tabs.is_empty() {
            return;
        }
    }
    let active = state.scripts.active.min(state.scripts.tabs.len() - 1);
    if state.scripts.tabs[active].changed_on_disk {
        let mut reload = None;
        bar(ui, &t, t.warn, |ui| {
            ui.label(
                RichText::new("This file changed on disk while you were editing it.")
                    .color(t.text_strong),
            );
            if ui
                .button("Reload")
                .on_hover_text("Drop your edits")
                .clicked()
            {
                reload = Some(true);
            }
            if ui
                .button("Keep Mine")
                .on_hover_text("Saving replaces the file")
                .clicked()
            {
                reload = Some(false);
            }
        });
        match reload {
            Some(true) => {
                if let Err(e) = state.reload_script(active) {
                    tracing::warn!("{e}");
                }
            }
            Some(false) => state.scripts.tabs[active].changed_on_disk = false,
            None => {}
        }
    }

    // Find bar.
    let path = state.scripts.tabs[active].path.clone();
    let text_id = egui::Id::unique("box_editor_script_text").with(&path);
    let cmd_f =
        state.scripts.focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::F));
    if cmd_f {
        state.scripts.find.get_or_insert_default();
        state.scripts.find_focus = true;
    }
    let mut find_marks = Vec::new();
    let mut jump_to: Option<usize> = None;
    if let Some(mut needle) = state.scripts.find.clone() {
        let found = matches(&state.scripts.tabs[active].text, &needle);
        let mut next = false;
        let mut close_find = false;
        bar(ui, &t, t.border, |ui| {
            ui.label("Find");
            let r = ui.add(TextEdit::singleline(&mut needle).desired_width(220.0));
            if state.scripts.find_focus {
                r.request_focus();
                state.scripts.find_focus = false;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                next = true;
                r.request_focus();
            }
            if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                close_find = true;
            }
            let count = if found.is_empty() {
                if needle.is_empty() {
                    String::new()
                } else {
                    "no matches".into()
                }
            } else {
                format!(
                    "{} of {}",
                    state.scripts.find_index % found.len() + 1,
                    found.len()
                )
            };
            ui.label(RichText::new(count).color(t.text_weak));
            if ui.button("Next").clicked() {
                next = true;
            }
            if ui.small_button("×").clicked() {
                close_find = true;
            }
        });
        if next && !found.is_empty() {
            state.scripts.find_index = (state.scripts.find_index + 1) % found.len();
            let (s, _) = found[state.scripts.find_index];
            jump_to = Some(state.scripts.tabs[active].text[..s].chars().count());
        }
        find_marks = found;
        state.scripts.find = if close_find { None } else { Some(needle) };
    }

    // Line problems from the log.
    let problems = state.problems_cached(&path);

    // The text with its margin.
    let font = FontId::monospace(13.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font));
    let pal = palette(&t);
    let goto = state.scripts.goto.take();
    if let Some(line) = goto {
        jump_to = Some(line_start(&state.scripts.tabs[active].text, line));
    }
    if let Some(ci) = jump_to {
        let mut st = TextEdit::load_state(ui.ctx(), text_id).unwrap_or_default();
        st.cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        st.store(ui.ctx(), text_id);
        ui.memory_mut(|m| m.request_focus(text_id));
    }
    let tab_text_lines = state.scripts.tabs[active].text.lines().count().max(1)
        + usize::from(state.scripts.tabs[active].text.ends_with('\n'));
    let mut focused = false;
    let mut cursor_line: Option<usize> = None;
    ScrollArea::both()
        .id_salt(("script_scroll", &path))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                // Margin: line numbers, problem lines in red.
                let mut job = LayoutJob::default();
                for n in 1..=tab_text_lines {
                    let bad = problems.iter().any(|(l, _)| *l as usize == n);
                    job.append(
                        &format!("{n:>4}\n"),
                        0.0,
                        TextFormat {
                            font_id: font.clone(),
                            color: if bad { t.error } else { t.text_faint },
                            ..Default::default()
                        },
                    );
                }
                let margin = ui.label(job);
                if let Some(p) = margin.hover_pos() {
                    let line = ((p.y - margin.rect.top()) / row_h).floor() as u32 + 1;
                    if let Some((_, msg)) = problems.iter().find(|(l, _)| *l == line) {
                        margin.on_hover_text(msg.clone());
                    }
                }
                let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _w: f32| {
                    let job = layout_job(buf.as_str(), font.clone(), &pal, &find_marks);
                    ui.fonts_mut(|f| f.layout_job(job))
                };
                let out = TextEdit::multiline(&mut state.scripts.tabs[active].text)
                    .id(text_id)
                    .font(font.clone())
                    .code_editor()
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .desired_rows(tab_text_lines)
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .show(ui);
                focused = out.response.has_focus();
                if let Some(r) = out.cursor_range {
                    let ci = r.primary.index;
                    let text = &state.scripts.tabs[active].text;
                    cursor_line =
                        Some(text.chars().take(ci.into()).filter(|c| *c == '\n').count() + 1);
                }
                if jump_to.is_some()
                    && let Some(line) = cursor_line
                {
                    let y = out.galley_pos.y + (line as f32 - 1.0) * row_h;
                    let rect = egui::Rect::from_min_size(
                        egui::pos2(out.galley_pos.x, y),
                        vec2(10.0, row_h),
                    );
                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                }
            });
        });
    state.scripts.focused = focused;
    if external {
        state.open_in_code_editor(&path, cursor_line.map(|l| l as u32));
    }
    if let Some(i) = save
        && let Err(e) = state.save_script(i)
    {
        tracing::warn!("{e}");
    }
}

fn bar(ui: &mut Ui, t: &crate::theme::Tokens, edge: Color32, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(t.surface_header)
        .stroke(egui::Stroke::new(1.0, edge))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(add);
        });
}

impl EditorState {
    /// [`EditorState::script_problems`], recomputed only when the log or the
    /// file changes.
    fn problems_cached(&mut self, path: &str) -> Vec<(u32, String)> {
        let seq = self.console.log.next_seq();
        if let Some((p, s, v)) = &self.scripts.problems
            && p == path
            && *s == seq
        {
            return v.clone();
        }
        let v = self.script_problems(path);
        self.scripts.problems = Some((path.to_string(), seq, v.clone()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_and_lines() {
        assert_eq!(
            matches("Speed speed SPEED", "speed"),
            vec![(0, 5), (6, 11), (12, 17)]
        );
        assert_eq!(line_start("a\nbc\nd", 1), 0);
        assert_eq!(line_start("a\nbc\nd", 3), 5);
        assert_eq!(line_start("a\nbc\nd", 9), 6);
    }
}
