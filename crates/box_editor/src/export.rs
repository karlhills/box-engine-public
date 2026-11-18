//! File ▸ Export… (and Game ▸ Project Settings ▸ Export Settings…): the
//! project's export settings (app name, bundle id, version, icon, startup
//! splash, saved in `box.project.json`) and an export run with the same code as `box export`
//! ([`box_export::export`]).
//!
//! The export runs on a background thread; its progress shows in the window
//! (step text and a bar), success as a toast with the bundle's path, and
//! failure as an error toast and in the Console (log target
//! [`EXPORT_TARGET`]). The editor stays usable meanwhile. Unsaved scene
//! changes are not exported: the export reads the project folder.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use box_engine::packed::Profile;
use box_engine::project::ExportSettings;
use box_export::{ExportOptions, ExportReport, Progress, Target};
use egui::{Align2, Context, RichText};

use crate::state::EditorState;
use crate::theme::Tokens;

/// Log target of export messages in the Console.
pub const EXPORT_TARGET: &str = "box_editor::export";

enum JobMessage {
    Progress(Progress),
    Done(Box<Result<ExportReport, String>>),
}

struct Job {
    rx: Receiver<JobMessage>,
    progress: Progress,
}

/// The export window's state.
#[derive(Default)]
pub(crate) struct ExportPanel {
    pub open: bool,
    pub target: Option<Target>,
    pub profile: Profile,
    /// Output folder as typed.
    pub out: String,
    /// The settings fields as typed (empty: the default).
    pub fields: [String; 4],
    /// The startup splash check box is off (`"splash": false`); the default
    /// (false) is the splash on, like an absent setting.
    pub no_splash: bool,
    /// The project root the fields were read from.
    loaded_for: Option<PathBuf>,
    job: Option<Job>,
    /// The last finished export: its bundle, or the error.
    pub last: Option<Result<PathBuf, String>>,
}

impl std::fmt::Debug for ExportPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportPanel")
            .field("open", &self.open)
            .field("running", &self.job.is_some())
            .finish_non_exhaustive()
    }
}

fn opt(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl EditorState {
    /// Opens the export window (File ▸ Export…). Needs a project.
    pub fn open_export(&mut self) {
        if self.project.is_none() {
            tracing::warn!(target: EXPORT_TARGET, "open a project to export it");
            return;
        }
        self.export.open = true;
        self.load_export_fields();
    }

    pub fn export_open(&self) -> bool {
        self.export.open
    }

    pub fn close_export(&mut self) {
        self.export.open = false;
    }

    /// Fills the window's fields from the project (once per project).
    fn load_export_fields(&mut self) {
        let Some(project) = &self.project else {
            return;
        };
        if self.export.loaded_for.as_deref() == Some(project.root()) {
            return;
        }
        let s = &project.file().export;
        self.export.fields = [
            s.app_name.clone().unwrap_or_default(),
            s.bundle_id.clone().unwrap_or_default(),
            s.version.clone().unwrap_or_default(),
            s.icon.clone().unwrap_or_default(),
        ];
        self.export.no_splash = !s.splash_enabled();
        self.export.out = project.root().join("export").display().to_string();
        self.export.loaded_for = Some(project.root().to_path_buf());
    }

    /// The export settings the window's fields describe.
    pub fn export_settings_edited(&self) -> ExportSettings {
        let [name, id, version, icon] = &self.export.fields;
        ExportSettings {
            app_name: opt(name),
            bundle_id: opt(id),
            version: opt(version),
            icon: opt(icon),
            // Written only when turned off: absent means on.
            splash: self.export.no_splash.then_some(false),
        }
    }

    /// Sets the window's fields (as if typed).
    pub fn set_export_fields(&mut self, s: &ExportSettings) {
        self.export.fields = [
            s.app_name.clone().unwrap_or_default(),
            s.bundle_id.clone().unwrap_or_default(),
            s.version.clone().unwrap_or_default(),
            s.icon.clone().unwrap_or_default(),
        ];
        self.export.no_splash = !s.splash_enabled();
    }

    /// Problems with the edited settings, as `(field, message)`.
    pub fn export_settings_problems(&self) -> Vec<(&'static str, String)> {
        match &self.project {
            Some(p) => box_export::settings_problems(p.root(), &self.export_settings_edited()),
            None => Vec::new(),
        }
    }

    /// Writes the edited settings into `box.project.json` when they changed
    /// and are valid.
    pub fn save_export_settings(&mut self) -> Result<(), String> {
        let settings = self.export_settings_edited();
        let problems = self.export_settings_problems();
        if let Some((field, message)) = problems.first() {
            return Err(format!("{field}: {message}"));
        }
        let project = self.project.as_mut().ok_or("no project is open")?;
        if project.file().export == settings {
            return Ok(());
        }
        project
            .set_export_settings(settings)
            .map_err(|e| e.to_string())?;
        tracing::info!(target: EXPORT_TARGET, "export settings saved to box.project.json");
        Ok(())
    }

    /// The options an export from the window uses.
    pub fn export_options(&self) -> Option<ExportOptions> {
        let project = self.project.as_ref()?;
        let out = PathBuf::from(self.export.out.trim());
        let out = if out.as_os_str().is_empty() {
            project.root().join("export")
        } else if out.is_relative() {
            project.root().join(out)
        } else {
            out
        };
        Some(ExportOptions::new(
            project.root(),
            self.export.target.unwrap_or_else(Target::host),
            self.export.profile,
            out,
        ))
    }

    /// Saves the settings, then starts an export in the background with
    /// `options` (see the module docs). Errors when one is running, the
    /// settings are invalid or no project is open.
    pub fn start_export(&mut self, options: ExportOptions) -> Result<(), String> {
        if self.export.job.is_some() {
            return Err("an export is already running".into());
        }
        self.save_export_settings()?;
        if self.is_dirty() {
            tracing::warn!(
                target: EXPORT_TARGET,
                "the scene has unsaved changes; the export uses the saved files"
            );
        }
        let (tx, rx) = channel();
        let first = Progress::new("start", "Starting", 0.0);
        std::thread::Builder::new()
            .name("box-export".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let result = box_export::export(&options, &mut |p| {
                    let _ = progress_tx.send(JobMessage::Progress(p));
                })
                .map_err(|e| e.to_string());
                let _ = tx.send(JobMessage::Done(Box::new(result)));
            })
            .map_err(|e| format!("could not start the export: {e}"))?;
        self.export.job = Some(Job {
            rx,
            progress: first,
        });
        self.export.last = None;
        Ok(())
    }

    /// The running export's latest progress.
    pub fn export_progress(&self) -> Option<&Progress> {
        self.export.job.as_ref().map(|j| &j.progress)
    }

    /// Applies the running export's messages; a finished export becomes a
    /// toast. Once per frame; returns whether one is still running.
    pub fn poll_export(&mut self) -> bool {
        let Some(job) = &mut self.export.job else {
            return false;
        };
        let mut done = None;
        loop {
            match job.rx.try_recv() {
                Ok(JobMessage::Progress(p)) => job.progress = p,
                Ok(JobMessage::Done(result)) => {
                    done = Some(*result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done = Some(Err("the export stopped unexpectedly".into()));
                    break;
                }
            }
        }
        let Some(result) = done else {
            return true;
        };
        self.export.job = None;
        match result {
            Ok(report) => {
                tracing::info!(
                    target: EXPORT_TARGET,
                    "exported {} ({} ms)",
                    report.output.display(),
                    report.total_ms.round()
                );
                for w in &report.warnings {
                    tracing::warn!(target: EXPORT_TARGET, "{w}");
                }
                self.notify(&format!("Exported {}", report.output.display()));
                self.export.last = Some(Ok(report.output));
            }
            Err(e) => {
                tracing::error!(target: EXPORT_TARGET, "export failed: {e}");
                self.export.last = Some(Err(e));
            }
        }
        false
    }
}

/// Width of the window's text fields.
const FIELD_W: f32 = 300.0;

fn text_field(ui: &mut egui::Ui, value: &mut String, hint: &str, width: f32, t: &Tokens) {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            // Darker than the window (whose fill is the field colour).
            .background_color(t.surface_header)
            .margin(egui::vec2(6.0, 4.0))
            .desired_width(width),
    );
}

fn problem_row(ui: &mut egui::Ui, problem: Option<&str>, t: &Tokens) {
    if let Some(p) = problem {
        ui.label("");
        ui.add(egui::Label::new(RichText::new(p).color(t.error_text).small()).wrap());
        ui.end_row();
    }
}

fn field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    problem: Option<&str>,
    t: &Tokens,
) {
    ui.label(label);
    text_field(ui, value, hint, FIELD_W, t);
    ui.end_row();
    problem_row(ui, problem, t);
}

fn primary_button(ui: &mut egui::Ui, t: &Tokens, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(t.accent_text))
            .fill(t.accent_bg)
            .min_size(egui::vec2(84.0, 24.0)),
    )
}

/// The export window, while open. `native_dialogs` enables the Browse
/// buttons (platform file pickers).
pub(crate) fn export_window(
    ctx: &Context,
    state: &mut EditorState,
    t: &Tokens,
    native_dialogs: bool,
) {
    if !state.export.open {
        return;
    }
    if state.project.is_none() {
        state.export.open = false;
        return;
    }
    state.load_export_fields();
    let (project_name, root) = match &state.project {
        Some(p) => (p.name().to_string(), p.root().to_path_buf()),
        None => return,
    };
    let problems = state.export_settings_problems();
    let problem = |f: &str| problems.iter().find(|p| p.0 == f).map(|p| p.1.clone());
    let running = state.export.job.is_some();
    let mut open = true;
    let mut start = false;
    let mut save = false;
    egui::Window::new("Export Game")
        .id(egui::Id::unique("box_editor_export"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .fade_in(false)
        .fade_out(false)
        .anchor(Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .show(ctx, |ui| {
            ui.set_width(FIELD_W + 130.0);
            ui.label(RichText::new("Application (saved in box.project.json)").strong());
            ui.add_space(4.0);
            let name_hint = project_name.clone();
            let id_hint = box_export::default_bundle_id(
                opt(&state.export.fields[0])
                    .as_deref()
                    .unwrap_or(&project_name),
            );
            let p_name = problem("app_name");
            let p_id = problem("bundle_id");
            let p_version = problem("version");
            let p_icon = problem("icon");
            let mut browse_icon = false;
            egui::Grid::new("box_editor_export_settings")
                .num_columns(2)
                .min_col_width(110.0)
                .spacing(egui::vec2(10.0, 6.0))
                .show(ui, |ui| {
                    let [name, id, version, icon] = &mut state.export.fields;
                    field(ui, "Name", name, &name_hint, p_name.as_deref(), t);
                    field(ui, "Bundle id", id, &id_hint, p_id.as_deref(), t);
                    field(
                        ui,
                        "Version",
                        version,
                        box_export::DEFAULT_VERSION,
                        p_version.as_deref(),
                        t,
                    );
                    ui.label("Icon")
                        .on_hover_text("A square PNG in the project, 1024 px is best");
                    ui.horizontal(|ui| {
                        let w = if native_dialogs {
                            FIELD_W - 80.0
                        } else {
                            FIELD_W
                        };
                        text_field(ui, icon, "engine icon", w, t);
                        if native_dialogs && ui.button("Browse…").clicked() {
                            browse_icon = true;
                        }
                    });
                    ui.end_row();
                    problem_row(ui, p_icon.as_deref(), t);
                    ui.label("Splash screen");
                    let mut show = !state.export.no_splash;
                    crate::widgets::checkbox_labeled(
                        ui,
                        &mut show,
                        "Show \"made with box engine\" at startup",
                    )
                    .on_hover_text(
                        "A black screen with the box engine wordmark for about 2.5 s \
                         while the game loads",
                    );
                    state.export.no_splash = !show;
                    ui.end_row();
                });
            if browse_icon
                && let Some(path) = rfd::FileDialog::new()
                    .set_title("Game Icon")
                    .add_filter("PNG", &["png"])
                    .set_directory(&root)
                    .pick_file()
            {
                let rel = path
                    .strip_prefix(&root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.display().to_string());
                state.export.fields[3] = rel;
            }
            let changed = state
                .project
                .as_ref()
                .is_some_and(|p| p.file().export != state.export_settings_edited());
            ui.add_space(4.0);
            ui.add_enabled_ui(changed && problems.is_empty(), |ui| {
                if ui.button("Save Settings").clicked() {
                    save = true;
                }
            });
            ui.separator();
            ui.label(RichText::new("Export").strong());
            ui.add_space(4.0);
            let mut browse_out = false;
            egui::Grid::new("box_editor_export_options")
                .num_columns(2)
                .min_col_width(110.0)
                .spacing(egui::vec2(10.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Platform");
                    let current = state.export.target.unwrap_or_else(Target::host);
                    let mut index = Target::ALL.iter().position(|x| *x == current).unwrap_or(0);
                    let tips = [
                        "An .app, ad-hoc signed".to_string(),
                        "A folder with <Name>.exe and game.pak".to_string(),
                    ];
                    if crate::widgets::segmented(ui, &mut index, &["macOS", "Windows"], &tips, 26.0)
                    {
                        state.export.target = Some(Target::ALL[index]);
                    }
                    ui.end_row();
                    ui.label("Profile");
                    let profiles = [Profile::Release, Profile::Development];
                    let mut index = profiles
                        .iter()
                        .position(|p| *p == state.export.profile)
                        .unwrap_or(0);
                    let tips = [
                        "For players: no GPU validation, no console window".to_string(),
                        "For you: GPU validation, a console window on Windows".to_string(),
                    ];
                    if crate::widgets::segmented(
                        ui,
                        &mut index,
                        &["Release", "Development"],
                        &tips,
                        26.0,
                    ) {
                        state.export.profile = profiles[index];
                    }
                    ui.end_row();
                    ui.label("Output folder");
                    ui.horizontal(|ui| {
                        let w = if native_dialogs {
                            FIELD_W - 80.0
                        } else {
                            FIELD_W
                        };
                        text_field(ui, &mut state.export.out, "", w, t);
                        if native_dialogs && ui.button("Browse…").clicked() {
                            browse_out = true;
                        }
                    });
                    ui.end_row();
                });
            if browse_out
                && let Some(dir) = rfd::FileDialog::new()
                    .set_title("Export To")
                    .set_directory(&root)
                    .pick_folder()
            {
                state.export.out = dir.display().to_string();
            }
            if state.is_dirty() {
                ui.label(
                    RichText::new(
                        "The scene has unsaved changes: the export uses the saved files.",
                    )
                    .color(t.warn)
                    .small(),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!running && problems.is_empty(), |ui| {
                    if primary_button(ui, t, "Export").clicked() {
                        start = true;
                    }
                });
                if let Some(p) = state.export_progress() {
                    ui.add(
                        egui::ProgressBar::new(p.fraction.clamp(0.0, 1.0))
                            .desired_width(200.0)
                            .text(p.message.clone()),
                    );
                }
            });
            match &state.export.last {
                Some(Ok(path)) => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("Exported {}", path.display()))
                                .color(t.text_weak)
                                .small(),
                        )
                        .wrap(),
                    );
                }
                Some(Err(e)) => {
                    ui.add(egui::Label::new(RichText::new(e).color(t.error_text).small()).wrap());
                }
                None => {}
            }
        });
    if !open {
        state.export.open = false;
    }
    if save && let Err(e) = state.save_export_settings() {
        tracing::error!(target: EXPORT_TARGET, "{e}");
    }
    if start
        && let Some(options) = state.export_options()
        && let Err(e) = state.start_export(options)
    {
        tracing::error!(target: EXPORT_TARGET, "{e}");
    }
    if running {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}
