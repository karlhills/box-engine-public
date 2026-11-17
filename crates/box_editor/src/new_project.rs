//! File ▸ New Project (a template and a folder) and File ▸ Open Example
//! (copy an example project into a folder, then open the copy; the
//! original is never written). The templates and examples come from
//! `box_engine::templates`, the same lookup as `box new --template`.
//!
//! The flow is plain [`Editor`] methods ([`Editor::select_template`],
//! [`Editor::answer_new_project`], [`Editor::select_example`],
//! [`Editor::answer_open_example`]), so tests drive it headlessly; the
//! modals here only call them. Native projects (Rust crates) are listed
//! but made by `box new --native`: the modal shows that command with a
//! Copy button instead of Create.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use box_engine::templates::{self, DEFAULT_TEMPLATE, Example, PREVIEW_FILE, Template};
use egui::{
    Align2, Color32, CursorIcon, Key, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, pos2, vec2,
};

use crate::dialogs::{Dialog, Outcome, button, error_line, primary_button, title};
use crate::document::folder_name;
use crate::editor::Editor;
use crate::state::EditorState;
use crate::theme::Tokens;

/// The folder name a new project starts with.
const NEW_FOLDER: &str = "my-game";
/// Width of a template or example card (its preview is 16:9).
const CARD_W: f32 = 150.0;
/// Cards per row.
const CARD_COLUMNS: usize = 4;
const CARD_GAP: f32 = 10.0;

impl EditorState {
    /// The gallery's example projects, found on first use.
    pub(crate) fn example_list(&mut self) -> &[Example] {
        self.examples.get_or_insert_with(templates::examples)
    }
}

/// `parent/name`, or `parent/name-2`, `-3`, ... when that exists.
fn unused(parent: &Path, name: &str) -> PathBuf {
    let first = parent.join(name);
    if !first.exists() {
        return first;
    }
    (2..1000)
        .map(|i| parent.join(format!("{name}-{i}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Modal text to a folder: `~` is the home folder; blank is `None`.
fn resolve_folder(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix("~/").or(text.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return Some(home.join(rest));
    }
    Some(PathBuf::from(text))
}

/// The preview file of a template (whether it exists or not).
fn template_preview(t: &Template) -> Option<PathBuf> {
    t.dir.as_ref().map(|d| d.join(PREVIEW_FILE))
}

impl Editor {
    /// Where new projects and example copies go by default: next to the
    /// open project, else the home folder, else the working folder.
    fn default_parent(&self) -> PathBuf {
        if let Some(parent) = self.state.project.as_ref().and_then(|p| p.root().parent()) {
            return parent.to_path_buf();
        }
        home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// Shows the New Project modal with the default template chosen.
    pub(crate) fn begin_new_project(&mut self, discard: bool) {
        let list = templates::templates();
        let path = unused(&self.default_parent(), NEW_FOLDER);
        self.dialog = Some(Dialog::NewProject {
            templates: list,
            template: DEFAULT_TEMPLATE.into(),
            path: path.display().to_string(),
            error: None,
            discard,
        });
    }

    /// Shows the Open Example modal, with `name` (else the first example)
    /// chosen.
    pub(crate) fn begin_open_example(&mut self, name: Option<String>, discard: bool) {
        let list = self.state.example_list().to_vec();
        let chosen = name
            .filter(|n| list.iter().any(|e| &e.name == n))
            .or_else(|| list.first().map(|e| e.name.clone()));
        let Some(chosen) = chosen else {
            self.show_error(
                "No example projects found",
                "The examples are installed next to the editor (templates/examples) or come from a box-engine checkout; neither was found.".into(),
            );
            return;
        };
        let path = unused(&self.default_parent(), &chosen);
        self.dialog = Some(Dialog::OpenExample {
            examples: list,
            example: chosen,
            path: path.display().to_string(),
            error: None,
            discard,
        });
    }

    /// Chooses a template in the New Project modal.
    pub fn select_template(&mut self, name: &str) {
        if let Some(Dialog::NewProject {
            templates,
            template,
            error,
            ..
        }) = &mut self.dialog
            && templates.iter().any(|t| t.name == name)
        {
            *template = name.to_string();
            *error = None;
        }
    }

    /// Chooses an example in the Open Example modal. A folder that was
    /// named after the previous example follows the new one.
    pub fn select_example(&mut self, name: &str) {
        let parent = self.default_parent();
        if let Some(Dialog::OpenExample {
            examples,
            example,
            path,
            error,
            ..
        }) = &mut self.dialog
            && examples.iter().any(|e| e.name == name)
        {
            let folder = Path::new(path.trim());
            let named_after_old = folder
                .file_name()
                .map(|f| f.to_string_lossy())
                .is_some_and(|f| f == example.as_str() || f.starts_with(&format!("{example}-")));
            if named_after_old {
                let base = folder.parent().map_or(parent, Path::to_path_buf);
                *path = unused(&base, name).display().to_string();
            }
            *example = name.to_string();
            *error = None;
        }
    }

    /// Answers the New Project modal: `Some(folder)` creates the project
    /// there from the chosen template (the folder's name is the project's
    /// name) and opens it; `None` cancels. An error keeps the modal open.
    pub fn answer_new_project(&mut self, path: Option<&str>) {
        let Some(Dialog::NewProject {
            templates: list,
            template,
            discard,
            ..
        }) = self.dialog.clone()
        else {
            return;
        };
        let Some(text) = path else {
            self.dialog = None;
            return;
        };
        let Some(dir) = resolve_folder(text) else {
            self.set_dialog_error("Enter a folder for the new project".into());
            return;
        };
        let created = templates::find_template(&list, &template).and_then(|t| {
            templates::create_project(&dir, &folder_name(&dir), &list, t).map(|p| (p, t))
        });
        let (project, chosen) = match created {
            Ok(done) => done,
            Err(e) => return self.set_dialog_error(e.to_string()),
        };
        tracing::info!(
            "project created from the `{template}` template: {}",
            project.root().display()
        );
        let mut note = format!(
            "Created `{}` from the {} template",
            project.name(),
            chosen.title
        );
        if project.root().join("README.md").is_file() {
            note.push_str("; its README.md explains what is in it");
        }
        self.open_new(project.root(), discard, &note);
    }

    /// Answers the Open Example modal: `Some(folder)` copies the chosen
    /// example there (a new or empty folder) and opens the copy; `None`
    /// cancels. An error keeps the modal open.
    pub fn answer_open_example(&mut self, path: Option<&str>) {
        let Some(Dialog::OpenExample {
            examples,
            example,
            discard,
            ..
        }) = self.dialog.clone()
        else {
            return;
        };
        let Some(text) = path else {
            self.dialog = None;
            return;
        };
        let Some(dir) = resolve_folder(text) else {
            self.set_dialog_error("Enter a folder for the copy".into());
            return;
        };
        let copied = templates::find_example(&examples, &example)
            .and_then(|e| templates::copy_example(e, &dir).map(|p| (p, e)));
        let (project, source) = match copied {
            Ok(done) => done,
            Err(e) => return self.set_dialog_error(e.to_string()),
        };
        tracing::info!("example `{example}` copied to {}", project.root().display());
        let note = format!(
            "Opened a copy of the {} example in {}; the original is unchanged",
            source.title,
            project.root().display()
        );
        self.open_new(project.root(), discard, &note);
    }

    /// Opens a project just created or copied, closing the modal.
    fn open_new(&mut self, root: &Path, discard: bool, note: &str) {
        match self.run_project_action(root, discard) {
            Ok(true) => {
                self.dialog = None;
                self.state.notify(note);
                self.offer_recovery();
            }
            // The upgrade prompt replaced the modal.
            Ok(false) => {}
            Err(e) => self.set_dialog_error(format!("{e:#}")),
        }
    }

    /// Loads the preview images the open modal shows (each file once).
    pub(crate) fn load_previews(&mut self, ctx: &egui::Context) {
        let paths: Vec<PathBuf> = match &self.dialog {
            Some(Dialog::NewProject { templates, .. }) => {
                templates.iter().filter_map(template_preview).collect()
            }
            Some(Dialog::OpenExample { examples, .. }) => {
                examples.iter().filter_map(|e| e.preview.clone()).collect()
            }
            _ => return,
        };
        for path in paths {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.previews.entry(path) {
                let texture = load_png(ctx, slot.key());
                slot.insert(texture);
            }
        }
    }
}

fn load_png(ctx: &egui::Context, path: &Path) -> Option<TextureHandle> {
    let image = match image::open(path) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            // A template without a preview is fine; say why in the log only.
            tracing::debug!("no preview {}: {e}", path.display());
            return None;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(
        format!("preview:{}", path.display()),
        pixels,
        TextureOptions::LINEAR,
    ))
}

/// A clickable card: the preview image (or a blank panel) and a label.
fn card(
    ui: &mut Ui,
    t: &Tokens,
    texture: Option<&TextureHandle>,
    label: &str,
    selected: bool,
) -> egui::Response {
    let image_h = CARD_W * 9.0 / 16.0;
    let (rect, response) = ui.allocate_exact_size(vec2(CARD_W, image_h + 24.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), selected, label)
    });
    let image = Rect::from_min_size(rect.min, vec2(CARD_W, image_h));
    let painter = ui.painter();
    match texture {
        Some(tex) => painter.image(
            tex.id(),
            image,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        ),
        None => painter.rect_filled(image, 4.0, t.field),
    };
    let stroke = if selected {
        Stroke::new(2.0, t.accent)
    } else if response.hovered() {
        Stroke::new(1.0, t.text_weak)
    } else {
        Stroke::new(1.0, t.border)
    };
    painter.rect_stroke(image, 4.0, stroke, StrokeKind::Outside);
    let color = if selected { t.text_strong } else { t.text };
    painter.text(
        pos2(rect.center().x, image.max.y + 6.0),
        Align2::CENTER_TOP,
        label,
        if selected {
            t.strong_small()
        } else {
            t.small()
        },
        color,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// A row of cards; returns the name of the one clicked.
fn cards<'a>(
    ui: &mut Ui,
    t: &Tokens,
    items: impl Iterator<Item = (&'a str, &'a str, Option<&'a TextureHandle>)>,
    selected: &str,
) -> Option<String> {
    let mut clicked = None;
    let items: Vec<_> = items.collect();
    for row in items.chunks(CARD_COLUMNS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for (name, label, texture) in row {
                if card(ui, t, *texture, label, *name == selected).clicked() {
                    clicked = Some((*name).to_string());
                }
            }
        });
        ui.add_space(4.0);
    }
    clicked
}

/// The folder row: a text field and, with native dialogs, Browse….
fn folder_row(
    ui: &mut Ui,
    t: &Tokens,
    path: &mut String,
    title: &str,
    native: bool,
) -> egui::Response {
    ui.horizontal(|ui| {
        let browse_w = if native { 90.0 } else { 0.0 };
        let r = ui.add(
            egui::TextEdit::singleline(path)
                .font(t.mono())
                .desired_width(ui.available_width() - browse_w),
        );
        if native && button(ui, "Browse…").clicked() {
            let current = PathBuf::from(path.trim());
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(dir) = current.parent().filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(dir);
            }
            if let Some(name) = current.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
            // A save panel lets the user name a new folder.
            if let Some(chosen) = dialog.save_file() {
                *path = chosen.display().to_string();
            }
        }
        r
    })
    .inner
}

fn modal_width() -> f32 {
    CARD_W * CARD_COLUMNS as f32 + CARD_GAP * (CARD_COLUMNS - 1) as f32
}

/// The New Project modal. Enter creates, Escape cancels.
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_project_ui(
    ui: &mut Ui,
    t: &Tokens,
    previews: &HashMap<PathBuf, Option<TextureHandle>>,
    list: &[Template],
    selected: &str,
    path: &mut String,
    error: Option<&str>,
    native_dialogs: bool,
) -> Option<Outcome> {
    ui.set_width(modal_width());
    title(ui, t, "New Project");
    ui.label(RichText::new("Start from a template:").color(t.text_weak));
    ui.add_space(8.0);
    let preview = |tpl: &Template| {
        template_preview(tpl)
            .and_then(|p| previews.get(&p))
            .and_then(Option::as_ref)
    };
    let mut outcome = cards(
        ui,
        t,
        list.iter()
            .map(|tpl| (tpl.name.as_str(), tpl.title.as_str(), preview(tpl))),
        selected,
    )
    .map(Outcome::SelectTemplate);
    let chosen = list.iter().find(|tpl| tpl.name == selected);
    if let Some(tpl) = chosen {
        ui.label(RichText::new(&tpl.description).color(t.text));
    }
    ui.add_space(10.0);
    ui.label(
        RichText::new("Folder")
            .font(t.strong_small())
            .color(t.text_weak),
    );
    let field = folder_row(ui, t, path, "New Project", native_dialogs);
    ui.label(
        RichText::new("Created if missing; its name becomes the project name.")
            .font(t.small())
            .color(t.text_faint),
    );
    let native = chosen.is_some_and(|tpl| tpl.native);
    let command = box_engine::player::shell_line(&[
        "box".into(),
        "new".into(),
        path.trim().to_string(),
        "--native".into(),
    ]);
    if native {
        ui.add_space(8.0);
        ui.label(
            RichText::new(
                "A native project is a Rust crate built with cargo (it needs Rust and a box-engine checkout). Create it in a terminal:",
            )
            .color(t.text_weak),
        );
        ui.label(RichText::new(&command).font(t.mono()).color(t.text_strong));
    }
    error_line(ui, t, error);
    ui.add_space(10.0);
    let copied_id = ui.scope_id().with("native_command_copied");
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if native {
                if primary_button(ui, t, "Copy Command").clicked() {
                    ui.ctx().copy_text(command.clone());
                    ui.data_mut(|d| d.insert_temp(copied_id, command.clone()));
                }
                let copied = ui.data(|d| d.get_temp::<String>(copied_id));
                if copied.as_deref() == Some(command.as_str()) {
                    ui.label(RichText::new("Copied").color(t.text_weak));
                }
            } else if primary_button(ui, t, "Create").clicked()
                || (field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                outcome = Some(Outcome::NewProject(Some(path.clone())));
            }
            if button(ui, "Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                outcome = Some(Outcome::NewProject(None));
            }
        });
    });
    outcome
}

/// The Open Example modal. Enter copies and opens, Escape cancels.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_example_ui(
    ui: &mut Ui,
    t: &Tokens,
    previews: &HashMap<PathBuf, Option<TextureHandle>>,
    list: &[Example],
    selected: &str,
    path: &mut String,
    error: Option<&str>,
    native_dialogs: bool,
) -> Option<Outcome> {
    ui.set_width(modal_width());
    title(ui, t, "Open Example");
    ui.label(
        RichText::new("The example is copied into a folder of your own and the copy opens; the original is never changed.")
            .color(t.text_weak),
    );
    ui.add_space(8.0);
    let preview = |e: &Example| {
        e.preview
            .as_ref()
            .and_then(|p| previews.get(p))
            .and_then(Option::as_ref)
    };
    let mut outcome = cards(
        ui,
        t,
        list.iter()
            .map(|e| (e.name.as_str(), e.title.as_str(), preview(e))),
        selected,
    )
    .map(Outcome::SelectExample);
    if let Some(e) = list.iter().find(|e| e.name == selected) {
        ui.label(RichText::new(&e.description).color(t.text));
    }
    ui.add_space(10.0);
    ui.label(
        RichText::new("Copy to folder")
            .font(t.strong_small())
            .color(t.text_weak),
    );
    let field = folder_row(ui, t, path, "Copy Example To", native_dialogs);
    ui.label(
        RichText::new("A new or empty folder.")
            .font(t.small())
            .color(t.text_faint),
    );
    error_line(ui, t, error);
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, t, "Copy and Open").clicked()
                || (field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                outcome = Some(Outcome::OpenExample(Some(path.clone())));
            }
            if button(ui, "Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                outcome = Some(Outcome::OpenExample(None));
            }
        });
    });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unused_folders_count_up() {
        let base = std::env::temp_dir().join(format!("box_unused_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("game")).unwrap();
        std::fs::create_dir_all(base.join("game-2")).unwrap();
        assert_eq!(unused(&base, "other"), base.join("other"));
        assert_eq!(unused(&base, "game"), base.join("game-3"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn folders_expand_home() {
        assert_eq!(resolve_folder("  "), None);
        assert_eq!(resolve_folder("/a/b"), Some(PathBuf::from("/a/b")));
        if let Some(home) = home_dir() {
            assert_eq!(resolve_folder("~/games/x"), Some(home.join("games/x")));
        }
    }
}
