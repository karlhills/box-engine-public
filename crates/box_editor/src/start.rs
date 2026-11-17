//! Starting the editor: the user config folder, the recent projects list
//! (File ▸ Open Recent), what a launch without a project folder opens, and
//! the Welcome card shown over the viewport when that is nothing.
//!
//! The user config folder is `$BOX_EDITOR_CONFIG_DIR` when set, else
//! `~/Library/Application Support/box-editor` on macOS, `%APPDATA%\box-editor`
//! on Windows and `$XDG_CONFIG_HOME/box-editor` (or `~/.config/box-editor`)
//! elsewhere. Only windowed runs use it ([`crate::run_game`]): tests and
//! `--capture` runs never read or write the user's recent list.

use std::path::{Path, PathBuf};

use egui::{Align2, RichText, vec2};
use serde::{Deserialize, Serialize};

use crate::state::EditorState;
use crate::theme::Tokens;

/// Overrides the user config folder (tests, sandboxes).
pub const CONFIG_DIR_ENV: &str = "BOX_EDITOR_CONFIG_DIR";

/// The app kind the editor's crash reports are filed under
/// (`box_engine::crash::last_crash`).
pub const CRASH_KIND: &str = "editor";

/// Recent projects kept.
pub const MAX_RECENT: usize = 10;

/// The recent projects file inside the config folder.
pub const RECENT_FILE: &str = "recent.json";

/// The user config folder (see the module docs); `None` without a home
/// folder.
pub fn user_config_dir() -> Option<PathBuf> {
    config_dir_from(|k| std::env::var_os(k))
}

/// [`user_config_dir`] with the environment read through `var`.
pub fn config_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let env = |k: &str| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(dir) = env(CONFIG_DIR_ENV) {
        return Some(dir);
    }
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    base.map(|b| b.join("box-editor"))
}

/// One recently opened project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    /// The project's root folder.
    pub path: PathBuf,
    /// Its name when it was last opened.
    pub name: String,
}

#[derive(Serialize, Deserialize)]
struct RecentFile {
    version: u32,
    projects: Vec<RecentProject>,
}

/// The recent projects, most recent first, at most [`MAX_RECENT`]. With a
/// file (see [`Self::load`]) every change is written to it; without one
/// the list lasts for the session.
#[derive(Clone, Debug, Default)]
pub struct RecentProjects {
    file: Option<PathBuf>,
    entries: Vec<RecentProject>,
}

impl RecentProjects {
    /// Reads `<config_dir>/recent.json` (a missing or broken file is an
    /// empty list; a broken one is reported and rewritten at the next
    /// change). `None` keeps the list in memory only.
    pub fn load(config_dir: Option<&Path>) -> Self {
        let Some(dir) = config_dir else {
            return Self::default();
        };
        let file = dir.join(RECENT_FILE);
        let entries = match std::fs::read_to_string(&file) {
            Ok(text) => match serde_json::from_str::<RecentFile>(&text) {
                Ok(f) => f.projects,
                Err(e) => {
                    tracing::warn!("{}: {e}; starting a new recent list", file.display());
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        let mut r = Self {
            file: Some(file),
            entries,
        };
        r.entries.truncate(MAX_RECENT);
        r
    }

    /// Most recent first.
    pub fn entries(&self) -> &[RecentProject] {
        &self.entries
    }

    /// The most recent project, if any.
    pub fn last(&self) -> Option<&RecentProject> {
        self.entries.first()
    }

    /// Puts `path` first (once), dropping the oldest beyond [`MAX_RECENT`].
    pub fn add(&mut self, path: &Path, name: &str) {
        let path = canonical(path);
        self.entries.retain(|e| canonical(&e.path) != path);
        self.entries.insert(
            0,
            RecentProject {
                path,
                name: name.to_owned(),
            },
        );
        self.entries.truncate(MAX_RECENT);
        self.save();
    }

    /// Forgets `path`.
    pub fn remove(&mut self, path: &Path) {
        let path = canonical(path);
        let before = self.entries.len();
        self.entries.retain(|e| canonical(&e.path) != path);
        if self.entries.len() != before {
            self.save();
        }
    }

    /// File ▸ Open Recent ▸ Clear Recent Projects.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }

    fn save(&self) {
        let Some(file) = &self.file else {
            return;
        };
        let body = RecentFile {
            version: 1,
            projects: self.entries.clone(),
        };
        let result = serde_json::to_vec_pretty(&body)
            .map_err(anyhow::Error::from)
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                crate::document::write_atomic(file, &bytes)
            });
        if let Err(e) = result {
            tracing::warn!("could not save the recent projects: {e:#}");
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// What a launch without a project folder opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// The working folder is inside this project.
    WorkingFolder(PathBuf),
    /// The most recent project (it still exists).
    LastProject(PathBuf),
    /// Nothing: the scratch scene with the Welcome card.
    Welcome,
}

/// The working folder's project, else (unless `no_restore`) the most
/// recent project that still exists, else the Welcome card.
pub fn choose_launch(cwd: Option<&Path>, recent: &RecentProjects, no_restore: bool) -> Launch {
    if let Some(root) = cwd.and_then(box_engine::Project::find_root) {
        return Launch::WorkingFolder(root);
    }
    if !no_restore
        && let Some(last) = recent.last()
        && last.path.join(box_engine::project::PROJECT_FILE).is_file()
    {
        return Launch::LastProject(last.path.clone());
    }
    Launch::Welcome
}

impl EditorState {
    /// Records the open project as the most recent one and hides the
    /// Welcome card. Called whenever a project opens.
    pub(crate) fn note_recent_project(&mut self) {
        if let Some(p) = &self.project {
            let (root, name) = (p.root().to_path_buf(), p.name().to_owned());
            self.recent.add(&root, &name);
            self.welcome = false;
        }
    }
}

/// What the Welcome card asked for.
enum WelcomeChoice {
    NewProject,
    OpenProject,
    Recent(PathBuf),
    Example(String),
    Dismiss,
}

/// The Welcome card, centred over the viewport: New Project, Open Project,
/// the recent projects, the example projects (each opens a copy; see
/// `new_project.rs`) and "Explore the sample scene" (closes it).
pub(crate) fn welcome_ui(ctx: &egui::Context, state: &mut EditorState, t: &Tokens) {
    if !state.welcome {
        return;
    }
    let Some(rect) = state.viewport.rect.filter(|r| r.width() > 0.0) else {
        return;
    };
    let mut choice = None;
    let width = 440.0_f32.min(rect.width() - 32.0).max(240.0);
    egui::Area::new(egui::Id::unique("box_editor_welcome"))
        .order(egui::Order::Middle)
        // Fully drawn on its first frame (captures, a quick glance).
        .fade_in(false)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(rect.center())
        .constrain_to(rect)
        .show(ctx, |ui| {
            egui::Frame::popup(&ctx.global_style())
                .fill(t.surface)
                .inner_margin(20)
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.label(
                        RichText::new("Welcome to box-engine")
                            .font(egui::FontId::new(
                                t.font_body + 6.0,
                                egui::FontFamily::Name(crate::theme::SEMIBOLD.into()),
                            ))
                            .color(t.text_strong),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "A project is a folder with your scenes and assets. \
                             Create one, open one, or look around the sample scene first.",
                        )
                        .color(t.text_weak),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let b = |ui: &mut egui::Ui, label: &str, primary: bool| {
                            let text = if primary {
                                RichText::new(label).color(t.accent_text)
                            } else {
                                RichText::new(label)
                            };
                            let mut button = egui::Button::new(text).min_size(vec2(130.0, 28.0));
                            if primary {
                                button = button.fill(t.accent_bg);
                            }
                            ui.add(button)
                        };
                        if b(ui, "New Project…", true).clicked() {
                            choice = Some(WelcomeChoice::NewProject);
                        }
                        if b(ui, "Open Project…", false).clicked() {
                            choice = Some(WelcomeChoice::OpenProject);
                        }
                    });
                    let examples = state.example_list();
                    if !examples.is_empty() {
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new("Examples")
                                .font(t.strong_small())
                                .color(t.text_weak),
                        );
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for e in examples {
                                let resp = ui
                                    .add(egui::Button::new(&e.title).min_size(vec2(0.0, 24.0)))
                                    .on_hover_text(format!(
                                        "{}\n\nOpens a copy in a folder you choose; the example itself is never changed.",
                                        e.description
                                    ));
                                if resp.clicked() {
                                    choice = Some(WelcomeChoice::Example(e.name.clone()));
                                }
                            }
                        });
                    }
                    ui.add_space(16.0);
                    // The recent list scrolls in a small viewport; the
                    // buttons above and below stay in view.
                    let recent_h = (rect.height() - 300.0).max(56.0);
                    egui::ScrollArea::vertical()
                        .max_height(recent_h)
                        .show(ui, |ui| {
                    ui.label(
                        RichText::new("Recent projects")
                            .font(t.strong_small())
                            .color(t.text_weak),
                    );
                    ui.add_space(4.0);
                    let current = state.project.as_ref().map(|p| p.root().to_path_buf());
                    if state.recent.entries().is_empty() {
                        ui.label(RichText::new("None yet").color(t.text_faint));
                    }
                    for r in state.recent.entries().iter().take(8) {
                        let exists = r.path.join(box_engine::project::PROJECT_FILE).is_file();
                        let open = current.as_deref() == Some(r.path.as_path());
                        let folder = r.path.display().to_string();
                        let text = if exists {
                            RichText::new(&r.name).color(t.text_strong)
                        } else {
                            RichText::new(format!("{} (missing)", r.name)).color(t.text_faint)
                        };
                        let resp = ui
                            .add_enabled(
                                exists && !open,
                                egui::Button::new(text)
                                    .right_text(
                                        RichText::new(short_path(&folder, 38)).color(t.text_faint),
                                    )
                                    .frame_when_inactive(false)
                                    .min_size(vec2(width, 24.0)),
                            )
                            .on_hover_text(&folder)
                            .on_disabled_hover_text(if open {
                                "Already open".to_string()
                            } else {
                                format!("{folder} was moved or deleted")
                            });
                        if resp.clicked() {
                            choice = Some(WelcomeChoice::Recent(r.path.clone()));
                        }
                    }
                        });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let label = if state.project.is_some() {
                            "Close"
                        } else {
                            "Explore the sample scene"
                        };
                        if ui
                            .add(egui::Button::new(label).min_size(vec2(0.0, 26.0)))
                            .clicked()
                        {
                            choice = Some(WelcomeChoice::Dismiss);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let keys =
                                crate::shortcuts::text(ctx, crate::shortcuts::Command::Palette);
                            ui.label(
                                RichText::new(format!("{keys}  search every command"))
                                    .font(t.small())
                                    .color(t.text_faint),
                            );
                        });
                    });
                });
        });
    match choice {
        Some(WelcomeChoice::NewProject) => state.requests.new_project = true,
        Some(WelcomeChoice::OpenProject) => state.requests.open_project = true,
        Some(WelcomeChoice::Recent(path)) => state.requests.open_recent = Some(path),
        Some(WelcomeChoice::Example(name)) => state.requests.open_example = Some(Some(name)),
        Some(WelcomeChoice::Dismiss) => state.welcome = false,
        None => {}
    }
}

/// `path` cut to its last `max` characters (with a leading ellipsis).
fn short_path(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_owned();
    }
    let tail: String = path.chars().skip(n - max + 1).collect();
    format!("…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_paths_keep_the_end() {
        assert_eq!(short_path("/a/b", 10), "/a/b");
        assert_eq!(short_path("/abcdefghij", 5), "…ghij");
    }
}
