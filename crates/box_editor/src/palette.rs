//! The command palette (Cmd+K, also F1 and Help ▸ Command Palette): a
//! centred search over every action of the registry (`actions.rs`) and the
//! actions that depend on what is open (Add ▸ kinds, Add Component, Open
//! Scene, assets, recent projects, undo history).
//!
//! The search is fuzzy: every word of the query must appear in the label,
//! the category or the keywords, as a substring or as letters in order
//! ("sva" finds Save Scene As). Labels count more than categories, word
//! starts more than the middle of a word, and recently used actions come
//! first (with an empty query, in the order they were used). Up and Down
//! move, Enter runs, Escape closes; each row shows the shortcut, or the
//! reason it is disabled.

use egui::{Align2, Key, Modifiers, RichText, Sense, vec2};

use crate::actions::{Action, context_actions};
use crate::shortcuts::Command;
use crate::state::EditorState;
use crate::theme::Tokens;

/// Rows drawn at most (the search narrows the rest).
const MAX_ROWS: usize = 200;

/// The open palette.
#[derive(Debug, Default)]
pub struct PaletteState {
    pub query: String,
    /// Index into the results.
    pub selected: usize,
    /// Everything searchable, built when the palette opened.
    entries: Vec<Action>,
    /// Indices of `entries` matching `ranked_for`, best first.
    ranked: Vec<usize>,
    ranked_for: Option<String>,
    /// Scroll the selected row into view on the next frame.
    scroll: bool,
}

/// One result row, as the palette shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteRow {
    pub id: String,
    pub category: String,
    pub label: String,
    pub shortcut: Option<Command>,
    /// `Err(reason)` when disabled.
    pub enabled: Result<(), String>,
    /// For toggles, whether it is on.
    pub checked: Option<bool>,
}

impl EditorState {
    /// Opens the command palette (empty query, recently used first).
    pub fn open_palette(&mut self) {
        let mut entries: Vec<Action> = self.actions.iter().cloned().collect();
        entries.extend(context_actions(self));
        self.palette = Some(PaletteState {
            entries,
            scroll: true,
            ..Default::default()
        });
        self.rank_palette();
    }

    pub fn close_palette(&mut self) {
        self.palette = None;
    }

    pub fn palette_open(&self) -> bool {
        self.palette.is_some()
    }

    /// Replaces the search text (what typing in the palette does).
    pub fn set_palette_query(&mut self, query: &str) {
        if let Some(p) = &mut self.palette {
            p.query = query.to_owned();
        }
        self.rank_palette();
    }

    /// Moves the selection by `delta` rows (clamped).
    pub fn palette_move(&mut self, delta: isize) {
        if let Some(p) = &mut self.palette {
            let last = p.ranked.len().saturating_sub(1);
            p.selected = p.selected.saturating_add_signed(delta).min(last);
            p.scroll = true;
        }
    }

    /// The results for the current query, best first.
    pub fn palette_results(&self) -> Vec<PaletteRow> {
        let Some(p) = &self.palette else {
            return Vec::new();
        };
        p.ranked.iter().map(|&i| row(self, &p.entries[i])).collect()
    }

    /// Runs the selected result if it is enabled and closes the palette;
    /// a disabled one leaves it open and returns the reason.
    pub fn palette_run_selected(&mut self) -> Result<(), String> {
        let Some(p) = &self.palette else {
            return Err("the palette is not open".into());
        };
        let action = p
            .ranked
            .get(p.selected)
            .map(|&i| p.entries[i].clone())
            .ok_or_else(|| "No matching action".to_string())?;
        action.availability(self)?;
        self.palette = None;
        self.perform(&action)
    }

    /// Re-ranks the entries when the query changed since the last ranking.
    fn rank_palette(&mut self) {
        let recent = self.actions.recent().to_vec();
        let Some(p) = &mut self.palette else {
            return;
        };
        if p.ranked_for.as_deref() == Some(p.query.as_str()) {
            return;
        }
        p.ranked = rank(&p.entries, &p.query, &recent);
        p.ranked_for = Some(p.query.clone());
        p.selected = 0;
        p.scroll = true;
    }
}

fn row(state: &EditorState, a: &Action) -> PaletteRow {
    PaletteRow {
        id: a.id().to_owned(),
        category: a.category().to_owned(),
        label: a.label_in(state),
        shortcut: a.shortcut_command(),
        enabled: a.availability(state),
        checked: a.checked(state),
    }
}

/// Indices of `entries` matching `query`, best first; ties keep the
/// registry order. `recent` (most recent first) lifts recently used ones.
pub fn rank(entries: &[Action], query: &str, recent: &[String]) -> Vec<usize> {
    let recency = |a: &Action| recent.iter().position(|r| r == a.id());
    if query.trim().is_empty() {
        let mut out: Vec<usize> = (0..entries.len()).collect();
        // Recently used first (most recent first), then the registry order.
        out.sort_by_key(|&i| recency(&entries[i]).unwrap_or(usize::MAX));
        return out;
    }
    let mut scored: Vec<(i32, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let s = fuzzy_score(query, a)?;
            let bonus = recency(a).map_or(0, |r| 60 - 2 * r.min(30) as i32);
            Some((s + bonus, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// The match score of `query` for `action` (higher is better), or `None`
/// when a word of the query matches neither the label, the category nor
/// the keywords.
pub fn fuzzy_score(query: &str, action: &Action) -> Option<i32> {
    let label = action.label().to_lowercase();
    let rest = format!("{} {}", action.category(), action.search_keywords()).to_lowercase();
    let mut total = 0;
    for word in query.split_whitespace() {
        let w = word.to_lowercase();
        let in_label = word_score(&w, &label).map(|s| s + 40);
        let in_rest = word_score(&w, &rest);
        total += in_label.max(in_rest)?;
    }
    // Shorter labels win ties ("Cube" before "Cube Map").
    Some(total - label.chars().count().min(40) as i32 / 4)
}

fn is_word_start(text: &str, byte: usize) -> bool {
    byte == 0
        || text[..byte]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
}

/// A substring scores far above letters in order.
fn word_score(word: &str, text: &str) -> Option<i32> {
    if let Some(p) = text.find(word) {
        let start = if p == 0 {
            60
        } else if is_word_start(text, p) {
            40
        } else {
            0
        };
        return Some(200 + 8 * word.chars().count() as i32 + start - p.min(30) as i32);
    }
    // Letters in order, preferring word starts and runs.
    let mut score = 0;
    let mut chars = text.char_indices().peekable();
    let mut last: Option<usize> = None;
    for wc in word.chars() {
        let mut found = None;
        for (i, c) in chars.by_ref() {
            if c == wc {
                found = Some(i);
                break;
            }
        }
        let i = found?;
        score += 10;
        if is_word_start(text, i) {
            score += 18;
        }
        if let Some(l) = last {
            let gap = text[l..i].chars().count().saturating_sub(1);
            if gap == 0 {
                score += 12;
            } else {
                score -= gap.min(10) as i32;
            }
        }
        last = Some(i);
    }
    Some(score)
}

/// Draws the open palette and runs what was picked.
pub(crate) fn palette_ui(ctx: &egui::Context, state: &mut EditorState, t: &Tokens) {
    if state.palette.is_none() {
        return;
    }
    // The palette owns these keys (before the text field sees them).
    let (up, down, enter, escape) = ctx.input_mut(|i| {
        (
            i.consume_key(Modifiers::NONE, Key::ArrowUp),
            i.consume_key(Modifiers::NONE, Key::ArrowDown),
            i.consume_key(Modifiers::NONE, Key::Enter),
            i.consume_key(Modifiers::NONE, Key::Escape),
        )
    });
    if escape {
        state.close_palette();
        return;
    }
    if up {
        state.palette_move(-1);
    }
    if down {
        state.palette_move(1);
    }
    if enter {
        // A disabled row stays selected; its reason is on screen.
        let _ = state.palette_run_selected();
        if state.palette.is_none() {
            return;
        }
    }
    let mut query = state
        .palette
        .as_ref()
        .map(|p| p.query.clone())
        .unwrap_or_default();
    let mut clicked = None;
    let width = 600.0_f32.min(ctx.content_rect().width() - 32.0);
    let area = egui::Modal::default_area(egui::Id::unique("box_editor_palette"))
        .anchor(Align2::CENTER_TOP, vec2(0.0, 72.0))
        .fade_in(false);
    let frame = egui::Frame::popup(&ctx.global_style())
        .fill(t.surface)
        .inner_margin(8);
    let response = egui::Modal::new(egui::Id::unique("box_editor_palette"))
        .area(area)
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(60))
        .show(ctx, |ui| {
            ui.set_width(width);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text("Search actions, scenes, assets and recent projects")
                    .desired_width(f32::INFINITY)
                    .margin(vec2(8.0, 6.0)),
            );
            if !edit.has_focus() {
                edit.request_focus();
            }
            ui.add_space(6.0);
            let (rows, selected, scroll) = match &state.palette {
                Some(p) => {
                    let rows: Vec<PaletteRow> = p
                        .ranked
                        .iter()
                        .take(MAX_ROWS)
                        .map(|&i| row(state, &p.entries[i]))
                        .collect();
                    (rows, p.selected, p.scroll)
                }
                None => return,
            };
            if rows.is_empty() {
                ui.label(RichText::new("No matching action").color(t.text_faint));
            }
            egui::ScrollArea::vertical()
                .max_height(400.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (i, r) in rows.iter().enumerate() {
                        let resp = row_ui(ui, t, r, i == selected);
                        if i == selected && scroll {
                            resp.scroll_to_me(None);
                        }
                        if resp.clicked() && r.enabled.is_ok() {
                            clicked = Some(i);
                        }
                    }
                });
            ui.add_space(6.0);
            let footer = match rows.get(selected) {
                Some(PaletteRow {
                    enabled: Err(reason),
                    ..
                }) => RichText::new(reason.as_str()).color(t.warn),
                Some(_) => {
                    let tip = state
                        .palette
                        .as_ref()
                        .and_then(|p| p.ranked.get(selected).map(|&i| &p.entries[i]))
                        .and_then(|a| a.tooltip_text())
                        .unwrap_or_default()
                        .to_owned();
                    RichText::new(tip).color(t.text_weak)
                }
                None => RichText::new(""),
            };
            ui.horizontal(|ui| {
                ui.label(footer.font(t.small()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("↑↓ move   Enter run   Esc close")
                            .font(t.small())
                            .color(t.text_faint),
                    );
                });
            });
        });
    if let Some(p) = &mut state.palette {
        p.scroll = false;
    }
    if response.backdrop_response.clicked() {
        state.close_palette();
        return;
    }
    if state.palette.as_ref().is_some_and(|p| p.query != query) {
        state.set_palette_query(&query);
    }
    if let Some(i) = clicked {
        if let Some(p) = &mut state.palette {
            p.selected = i;
        }
        let _ = state.palette_run_selected();
    }
}

/// One result: category and label on the left; the shortcut, On/Off or the
/// disabled reason on the right.
fn row_ui(ui: &mut egui::Ui, t: &Tokens, r: &PaletteRow, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let enabled = r.enabled.is_ok();
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::accesskit::Role::Button, enabled, selected, &r.label)
    });
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, t.radius, t.accent_bg);
    } else if resp.hovered() {
        p.rect_filled(rect, t.radius, t.hover);
    }
    let (strong, weak) = match (enabled, selected) {
        (false, _) => (t.text_faint, t.text_faint),
        (true, true) => (t.accent_text, t.text),
        (true, false) => (t.text_strong, t.text_weak),
    };
    let x = rect.left() + 8.0;
    let cat = p.text(
        egui::pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        format!("{}  ›", r.category),
        t.body(),
        weak,
    );
    p.text(
        egui::pos2(cat.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        &r.label,
        t.body(),
        strong,
    );
    let right = egui::pos2(rect.right() - 8.0, rect.center().y);
    match (&r.enabled, r.checked, r.shortcut) {
        (Err(reason), _, _) => {
            p.text(right, Align2::RIGHT_CENTER, reason, t.small(), t.text_faint);
        }
        (Ok(()), checked, shortcut) => {
            let mut x = right.x;
            if let Some(c) = shortcut {
                let keys = crate::shortcuts::text(ui.ctx(), c);
                let r = p.text(
                    egui::pos2(x, right.y),
                    Align2::RIGHT_CENTER,
                    keys,
                    t.mono(),
                    weak,
                );
                x = r.left() - 12.0;
            }
            if let Some(on) = checked {
                let (text, color) = if on { ("On", t.accent) } else { ("Off", weak) };
                let color = if selected { t.accent_text } else { color };
                p.text(
                    egui::pos2(x, right.y),
                    Align2::RIGHT_CENTER,
                    text,
                    t.small(),
                    color,
                );
            }
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(id: &str, cat: &str, label: &str) -> Action {
        Action::new(id, cat, label, |_| {})
    }

    #[test]
    fn substring_beats_letters_in_order_and_words_match_anywhere() {
        let entries = [
            a("1", "File", "Save Scene As…"),
            a("2", "File", "Save Scene"),
            a("3", "Add", "Sphere"),
            a("4", "View", "Scene Settings"),
        ];
        let ids = |q: &str| -> Vec<&str> {
            rank(&entries, q, &[])
                .into_iter()
                .map(|i| entries[i].id())
                .collect()
        };
        assert_eq!(ids("save")[..2], ["2", "1"], "shorter label first");
        assert_eq!(ids("sva"), ["1"], "letters in order");
        assert_eq!(ids("add sph"), ["3"], "category words count");
        assert_eq!(ids("scene file"), ["2", "1"]);
        assert!(ids("zzz").is_empty());
    }

    #[test]
    fn recently_used_come_first() {
        let entries = [
            a("x", "A", "Alpha"),
            a("y", "A", "Alpine"),
            a("z", "B", "Beta"),
        ];
        let recent = ["z".to_string(), "y".to_string()];
        let order: Vec<&str> = rank(&entries, "", &recent)
            .into_iter()
            .map(|i| entries[i].id())
            .collect();
        assert_eq!(order, ["z", "y", "x"]);
        let order: Vec<&str> = rank(&entries, "alp", &recent)
            .into_iter()
            .map(|i| entries[i].id())
            .collect();
        assert_eq!(order, ["y", "x"]);
    }
}
