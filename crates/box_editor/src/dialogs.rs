//! Document dialogs: the unsaved-changes, recovery and project upgrade
//! prompts, Save Scene As,
//! the New Project (templates) and Open Example (copies) dialogs
//! (`new_project.rs`), the Open Project path entry, the error modal (a failed Save, a
//! project or start scene that failed to open), the asset delete
//! confirmation, Help ▸ Keyboard Shortcuts and the "crashed last time"
//! notice.
//!
//! The flow is plain methods on [`Editor`] ([`Editor::begin`],
//! [`Editor::answer_unsaved`], ...), so it is testable headlessly; the egui
//! modals only call them. Native panels (`rfd`) are used only when the editor
//! was built [`with_native_dialogs`](Editor::with_native_dialogs); headless
//! runs and tests always get the in-editor path-entry modals instead.

use std::path::{Path, PathBuf};

use anyhow::Result;
use box_engine::project::SCENE_EXTENSION;
use egui::{Key, RichText, Ui, vec2};

use box_engine::templates::{Example, Template};

use crate::document::{normalize_scene_path, scene_name};
use crate::editor::Editor;
use crate::theme::Tokens;

/// An action that replaces or closes the open scene, so it first asks about
/// unsaved changes.
#[derive(Clone, Debug, PartialEq)]
pub enum DocAction {
    /// File ▸ New Scene: an unsaved scene with a sun, ground and camera.
    NewScene,
    /// File ▸ New Project: choose a template and a folder.
    NewProject,
    OpenProject,
    /// File ▸ Open Example, or an example of the Welcome card (preselected):
    /// copy an example project into a folder and open the copy.
    OpenExample(Option<String>),
    /// Open this scene file (of the current project).
    OpenScene(PathBuf),
    /// Open this project folder (File ▸ Open Recent, the Welcome card).
    OpenRecent(PathBuf),
    Quit,
}

impl DocAction {
    /// The "before ..." phrase of the unsaved-changes prompt.
    fn before(&self) -> String {
        match self {
            Self::NewScene => "before creating a new scene".into(),
            Self::NewProject => "before creating a project".into(),
            Self::OpenProject | Self::OpenRecent(_) | Self::OpenExample(_) => {
                "before opening another project".into()
            }
            Self::OpenScene(p) => format!("before opening “{}”", scene_name(p)),
            Self::Quit => "before quitting".into(),
        }
    }
}

/// The answer to the project upgrade prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeChoice {
    /// Back up and migrate the project's files, then open it.
    Upgrade,
    /// Open it without writing any of its files ([`EditorState::read_only`]).
    ///
    /// [`EditorState::read_only`]: crate::EditorState::read_only
    ReadOnly,
    /// Leave the current project open.
    Cancel,
}

/// The answer to the unsaved-changes prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsavedChoice {
    Save,
    DontSave,
    Cancel,
}

/// The modal shown over the editor; at most one at a time.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    /// Folder entry for Open Project: the fallback without native dialogs,
    /// or the error for a folder the native dialog returned.
    Project {
        path: String,
        error: Option<String>,
        /// The unsaved-changes prompt was answered Don't Save.
        discard: bool,
    },
    /// File entry for Save Scene As (without native dialogs, or after an
    /// error). `then` runs after a successful save.
    SaveAs {
        path: String,
        error: Option<String>,
        /// An existing file the user was warned about; saving to it again
        /// replaces it.
        replace: Option<PathBuf>,
        then: Option<DocAction>,
    },
    /// "Save changes to ... before ...?"
    Unsaved {
        action: DocAction,
        error: Option<String>,
    },
    /// The scene's recovery file is newer than the scene: restore or discard.
    Recovery { recovery: PathBuf },
    /// The project's files are from an older engine or older game types
    /// (`box_engine::upgrade`): Upgrade, Open Read-Only or Cancel.
    Upgrade {
        /// The project root.
        root: PathBuf,
        /// The project's name.
        name: String,
        /// What would change ([`UpgradePlan::summary`]).
        ///
        /// [`UpgradePlan::summary`]: box_engine::upgrade::UpgradePlan::summary
        summary: String,
        /// False when some file cannot be migrated: only Open Read-Only
        /// and Cancel are offered.
        can_upgrade: bool,
        /// The unsaved-changes prompt was answered Don't Save.
        discard: bool,
        error: Option<String>,
    },
    /// Something the user asked for failed (e.g. Save on a read-only file).
    Error { title: String, message: String },
    /// Delete in the asset browser: move this file (and its `.meta`) to the
    /// project's trash folder?
    DeleteAsset {
        id: box_core::asset_ref::AssetId,
        /// Relative to `assets/`.
        path: String,
        error: Option<String>,
    },
    /// File ▸ New Project: the templates, the chosen one and the folder.
    NewProject {
        templates: Vec<Template>,
        /// The chosen template's name.
        template: String,
        /// The new project's folder (created if missing; its name becomes
        /// the project name).
        path: String,
        error: Option<String>,
        /// The unsaved-changes prompt was answered Don't Save.
        discard: bool,
    },
    /// File ▸ Open Example: the examples, the chosen one and the folder its
    /// copy goes to.
    OpenExample {
        examples: Vec<Example>,
        /// The chosen example's name.
        example: String,
        /// A new or empty folder for the copy.
        path: String,
        error: Option<String>,
        discard: bool,
    },
    /// Help ▸ Keyboard Shortcuts.
    Shortcuts,
    /// The editor crashed last time: open the crash report or dismiss.
    CrashReport { report: PathBuf },
}

impl Dialog {
    fn set_error(&mut self, message: String) {
        match self {
            Self::Project { error, .. }
            | Self::SaveAs { error, .. }
            | Self::Unsaved { error, .. }
            | Self::Upgrade { error, .. }
            | Self::NewProject { error, .. }
            | Self::OpenExample { error, .. }
            | Self::DeleteAsset { error, .. } => *error = Some(message),
            Self::Recovery { .. }
            | Self::Error { .. }
            | Self::Shortcuts
            | Self::CrashReport { .. } => {
                tracing::error!("{message}")
            }
        }
    }
}

/// What the user did in a modal this frame.
pub(crate) enum Outcome {
    Unsaved(UnsavedChoice),
    Recovery { restore: bool },
    Upgrade(UpgradeChoice),
    SaveAs(Option<String>),
    Project(Option<String>),
    NewProject(Option<String>),
    SelectTemplate(String),
    OpenExample(Option<String>),
    SelectExample(String),
    DismissError,
    DeleteAsset(bool),
    CloseShortcuts,
    CrashReport { open: bool },
}

impl Editor {
    /// The open modal, if any.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Starts `action` (stopping Play first), asking about unsaved changes.
    /// Ignored while another dialog is open.
    pub fn begin(&mut self, action: DocAction) {
        if self.dialog.is_some() {
            return;
        }
        // Every document action works on the edit world: end Play first.
        self.state.stop_play();
        if self.state.is_dirty() {
            self.dialog = Some(Dialog::Unsaved {
                action,
                error: None,
            });
        } else {
            self.perform(action, false);
        }
    }

    /// Quit from the window's close button or the menu. True when the editor
    /// may close now; otherwise the unsaved-changes prompt is showing.
    pub fn request_quit(&mut self) -> bool {
        self.begin(DocAction::Quit);
        self.quit_confirmed
    }

    /// Answers the unsaved-changes prompt. Save on a scene without a file
    /// goes through Save Scene As, then runs the action.
    pub fn answer_unsaved(&mut self, choice: UnsavedChoice) {
        let Some(Dialog::Unsaved { action, .. }) = self.dialog.clone() else {
            return;
        };
        match choice {
            UnsavedChoice::Cancel => self.dialog = None,
            UnsavedChoice::DontSave => {
                self.dialog = None;
                self.perform(action, true);
            }
            UnsavedChoice::Save if self.state.scene.path.is_none() => {
                self.dialog = None;
                self.begin_save_as(Some(action));
            }
            UnsavedChoice::Save => match self.state.save_scene() {
                Ok(_) => {
                    self.dialog = None;
                    self.perform(action, false);
                }
                Err(e) => self.set_dialog_error(format!("Save failed: {e:#}")),
            },
        }
    }

    /// Save Scene: to its file, or Save Scene As for a scene without one.
    pub fn save(&mut self) {
        if self.state.scene.path.is_none() {
            self.begin_save_as(None);
        } else if let Err(e) = self.state.save_scene() {
            self.show_error("Could not save the scene", format!("{e:#}"));
        }
    }

    /// Logs a failure and shows it in a modal (replacing any open one).
    pub(crate) fn show_error(&mut self, title: &str, message: String) {
        tracing::error!("{title}: {message}");
        self.dialog = Some(Dialog::Error {
            title: title.into(),
            message,
        });
    }

    /// Asks before moving an asset file to the project's trash (Delete in
    /// the asset browser).
    pub fn begin_delete_asset(&mut self, id: box_core::asset_ref::AssetId) {
        if self.dialog.is_some() {
            return;
        }
        let Some(path) = self.state.asset_server().and_then(|s| s.path_of(id)) else {
            return;
        };
        self.dialog = Some(Dialog::DeleteAsset {
            id,
            path,
            error: None,
        });
    }

    /// Answers the asset delete confirmation: `true` moves the file to
    /// `.box/trash/`, `false` keeps it.
    pub fn answer_delete_asset(&mut self, confirm: bool) {
        let Some(Dialog::DeleteAsset { id, .. }) = self.dialog.clone() else {
            return;
        };
        if !confirm {
            self.dialog = None;
            return;
        }
        match self.state.trash_asset(id) {
            Ok(_) => self.dialog = None,
            Err(e) => self.set_dialog_error(format!("{e:#}")),
        }
    }

    /// Closes Help ▸ Keyboard Shortcuts.
    pub fn close_shortcuts(&mut self) {
        if matches!(self.dialog, Some(Dialog::Shortcuts)) {
            self.dialog = None;
        }
    }

    /// Shows "The editor crashed last time" when the last editor run ended
    /// in a crash whose report was not dismissed yet (see
    /// [`box_engine::crash::last_crash`]). Waits behind any open dialog
    /// (the recovery prompt comes first).
    pub fn offer_crash_report(&mut self) {
        self.crash_notice = box_engine::crash::last_crash(crate::start::CRASH_KIND);
        self.show_crash_notice();
    }

    /// The pending crash notice becomes the dialog once no other is open.
    pub(crate) fn show_crash_notice(&mut self) {
        if self.dialog.is_none()
            && let Some(report) = self.crash_notice.take()
        {
            self.dialog = Some(Dialog::CrashReport { report });
        }
    }

    /// Answers the crash notice: `open` shows the report in the system's
    /// text viewer (windowed editors only; headless runs just log its
    /// path). Either way the notice is not shown again; the file is kept.
    pub fn answer_crash_report(&mut self, open: bool) {
        let Some(Dialog::CrashReport { report }) = self.dialog.clone() else {
            return;
        };
        self.dialog = None;
        box_engine::crash::dismiss_last_crash(crate::start::CRASH_KIND);
        if open {
            if self.native_dialogs {
                crate::actions::open_in_os(&report);
            } else {
                tracing::info!("crash report: {}", report.display());
            }
        }
    }

    /// Closes the error modal.
    pub fn dismiss_error(&mut self) {
        if matches!(self.dialog, Some(Dialog::Error { .. })) {
            self.dialog = None;
        }
    }

    /// Save Scene As: a native save panel, or the path-entry modal. `then`
    /// runs after a successful save.
    pub fn begin_save_as(&mut self, then: Option<DocAction>) {
        let default = self.default_scene_path();
        if self.native_dialogs {
            // Blocks this frame until the panel closes, like any modal dialog.
            let Some(path) = native_save_scene(&default) else {
                return;
            };
            match self.state.save_scene_as(&path) {
                Ok(_) => {
                    if let Some(action) = then {
                        self.perform(action, false);
                    }
                }
                Err(e) => {
                    self.dialog = Some(Dialog::SaveAs {
                        path: self.display_path(&path),
                        error: Some(format!("{e:#}")),
                        replace: None,
                        then,
                    });
                }
            }
        } else {
            self.dialog = Some(Dialog::SaveAs {
                path: self.display_path(&default),
                error: None,
                replace: None,
                then,
            });
        }
    }

    /// Answers the Save Scene As modal: `Some(path)` saves there (relative
    /// paths are inside the project, or the working folder without one;
    /// `.scene.json` is added when missing), `None` cancels. Saving over
    /// another existing file needs a second confirmation.
    pub fn answer_save_as(&mut self, path: Option<&str>) {
        let Some(Dialog::SaveAs { replace, then, .. }) = self.dialog.clone() else {
            return;
        };
        let Some(text) = path else {
            self.dialog = None;
            return;
        };
        let Some(target) = self.resolve_input(text) else {
            self.set_dialog_error("Enter a file name for the scene.".into());
            return;
        };
        let target = normalize_scene_path(&target);
        let current = self.state.scene.path.as_deref() == Some(target.as_path());
        if target.exists() && !current && replace.as_ref() != Some(&target) {
            if let Some(Dialog::SaveAs { replace, error, .. }) = &mut self.dialog {
                *error = Some(format!(
                    "{} already exists. Save again to replace it.",
                    self.state
                        .project
                        .as_ref()
                        .map_or_else(|| target.display().to_string(), |p| p.relative(&target))
                ));
                *replace = Some(target);
            }
            return;
        }
        match self.state.save_scene_as(&target) {
            Ok(_) => {
                self.dialog = None;
                if let Some(action) = then {
                    self.perform(action, false);
                }
            }
            Err(e) => self.set_dialog_error(format!("{e:#}")),
        }
    }

    /// Answers the Open Project modal: `Some(folder)` opens it, `None`
    /// cancels. An error keeps the modal open to correct the path.
    pub fn answer_project(&mut self, path: Option<&str>) {
        let Some(Dialog::Project { discard, .. }) = self.dialog.clone() else {
            return;
        };
        let Some(text) = path else {
            self.dialog = None;
            return;
        };
        match self.run_project_action(Path::new(text.trim()), discard) {
            Ok(true) => {
                self.dialog = None;
                self.offer_recovery();
            }
            // The upgrade prompt replaced this one.
            Ok(false) => {}
            Err(e) => self.set_dialog_error(format!("{e:#}")),
        }
    }

    /// Shows the upgrade prompt for the project at `root` ([`Dialog::Upgrade`]),
    /// replacing any open modal.
    pub(crate) fn show_upgrade(&mut self, plan: &box_engine::upgrade::UpgradePlan, discard: bool) {
        self.dialog = Some(Dialog::Upgrade {
            root: plan.root().to_path_buf(),
            name: plan.name.clone(),
            summary: plan.summary(),
            can_upgrade: !plan.blocked(),
            discard,
            error: None,
        });
    }

    /// Answers the upgrade prompt. Upgrade backs up and migrates the files
    /// (planned again, so the result matches the files now), then opens the
    /// project; Open Read-Only opens it without writing any of its files;
    /// Cancel keeps the current project. A failure keeps the prompt open
    /// with the error.
    pub fn answer_upgrade(&mut self, choice: UpgradeChoice) {
        let Some(Dialog::Upgrade { root, discard, .. }) = self.dialog.clone() else {
            return;
        };
        if choice == UpgradeChoice::Cancel {
            self.dialog = None;
            return;
        }
        // The outgoing project's layout and session, as for any switch.
        if self.state.project.is_some() {
            let _ = self.save_layout();
            self.state.save_prefs();
        }
        let opened = match choice {
            UpgradeChoice::Cancel => return,
            UpgradeChoice::ReadOnly => self
                .state
                .switch_project_mode(&root, discard, true)
                .map(|()| {
                    self.state.notify(
                        "Opened read-only: the project's files are from an older engine and are not written",
                    );
                }),
            UpgradeChoice::Upgrade => self.upgrade_and_open(&root, discard),
        };
        match opened {
            Ok(()) => {
                self.dialog = None;
                self.load_project_settings();
                self.offer_recovery();
            }
            Err(e) => self.set_dialog_error(format!("{e:#}")),
        }
    }

    fn upgrade_and_open(&mut self, root: &Path, discard: bool) -> Result<()> {
        use box_engine::upgrade;
        let plan = upgrade::plan(root, &self.state.game.registry())?;
        let report = upgrade::apply(&plan)?;
        self.state.switch_project_mode(root, discard, false)?;
        if !report.written.is_empty() {
            let backup = self.state.project.as_ref().map_or_else(
                || report.backup.display().to_string(),
                |p| p.relative(&report.backup),
            );
            self.state.notify(&format!(
                "Upgraded {} file(s); the originals are in {backup}",
                report.written.len()
            ));
        }
        Ok(())
    }

    /// Answers the recovery prompt: restore the recovery file as unsaved
    /// changes, or delete it.
    pub fn answer_recovery(&mut self, restore: bool) {
        let Some(Dialog::Recovery { recovery }) = self.dialog.clone() else {
            return;
        };
        self.dialog = None;
        if restore {
            if let Err(e) = self.state.restore_recovery() {
                tracing::error!("restore failed: {e:#}");
            }
        } else {
            match std::fs::remove_file(&recovery) {
                Ok(()) => tracing::info!("discarded {}", recovery.display()),
                Err(e) => tracing::error!("could not delete {}: {e}", recovery.display()),
            }
        }
    }

    /// Opens a project (start scene and saved layout), keeping unsaved
    /// changes of the current scene in its recovery file, and offers the new
    /// scene's recovery file if there is a newer one.
    /// A project whose files need an upgrade shows the upgrade prompt
    /// instead ([`Self::answer_upgrade`]).
    pub fn open_project(&mut self, path: &Path) -> Result<()> {
        if self.run_project_action(path, false)? {
            self.offer_recovery();
        }
        Ok(())
    }

    /// Shows the recovery prompt when the scene has a newer recovery file.
    pub(crate) fn offer_recovery(&mut self) {
        if self.dialog.is_none()
            && let Some(recovery) = self.state.recovery_candidate()
        {
            self.dialog = Some(Dialog::Recovery { recovery });
        }
    }

    pub(crate) fn set_dialog_error(&mut self, message: String) {
        if let Some(d) = &mut self.dialog {
            d.set_error(message);
        }
    }

    /// Runs `action`; `discard` drops the scene's unsaved changes instead of
    /// keeping them in the recovery file.
    fn perform(&mut self, action: DocAction, discard: bool) {
        match action {
            DocAction::Quit => {
                if discard {
                    self.state.discard_changes();
                }
                self.quit_confirmed = true;
            }
            DocAction::NewScene => match self.state.new_scene(discard) {
                Ok(()) => {}
                Err(e) => self.show_error("Could not create a new scene", format!("{e:#}")),
            },
            DocAction::NewProject => self.begin_new_project(discard),
            DocAction::OpenExample(name) => self.begin_open_example(name, discard),
            DocAction::OpenProject => self.begin_open_project(discard),
            DocAction::OpenRecent(path) => match self.run_project_action(&path, discard) {
                Ok(true) => self.offer_recovery(),
                Ok(false) => {}
                Err(e) => {
                    if !path.join(box_engine::project::PROJECT_FILE).is_file() {
                        self.state.recent.remove(&path);
                    }
                    self.show_error("Could not open the project", format!("{e:#}"));
                }
            },
            DocAction::OpenScene(path) => match self.state.open_scene(&path, discard) {
                Ok(()) => self.offer_recovery(),
                Err(e) => tracing::error!("open scene failed: {e:#}"),
            },
        }
    }

    fn begin_open_project(&mut self, discard: bool) {
        let parent = self
            .state
            .project
            .as_ref()
            .and_then(|p| p.root().parent())
            .map(PathBuf::from);
        if !self.native_dialogs {
            self.dialog = Some(Dialog::Project {
                path: parent
                    .map(|p| format!("{}/", p.display()))
                    .unwrap_or_default(),
                error: None,
                discard,
            });
            return;
        }
        // Blocks this frame until the panel closes, like any modal dialog.
        let Some(path) = native_pick_project(parent) else {
            return;
        };
        match self.run_project_action(&path, discard) {
            Ok(true) => self.offer_recovery(),
            Ok(false) => {}
            Err(e) => {
                self.dialog = Some(Dialog::Project {
                    path: path.display().to_string(),
                    error: Some(format!("{e:#}")),
                    discard,
                });
            }
        }
    }

    /// Opens the project at `path`, saving the old project's layout first
    /// and loading the new one's. A project that needs an upgrade is not
    /// opened: the upgrade prompt is shown and the result is `Ok(false)`.
    /// One with files from a newer engine is an error.
    pub(crate) fn run_project_action(&mut self, path: &Path, discard: bool) -> Result<bool> {
        if let Some(plan) = self.state.upgrade_plan(path)? {
            self.show_upgrade(&plan, discard);
            return Ok(false);
        }
        let _ = self.save_layout();
        // The outgoing project's session (scene, camera, selection).
        self.state.save_prefs();
        self.state.switch_project(path, discard)?;
        self.load_project_settings();
        Ok(true)
    }

    /// Where Save As starts: next to the scene file, else the project's
    /// `assets/scenes`, else the working folder.
    fn default_scene_path(&self) -> PathBuf {
        let file = format!("{}{SCENE_EXTENSION}", self.state.scene.name);
        let dir = match (&self.state.scene.path, &self.state.project) {
            (Some(p), _) => p.parent().map(Path::to_path_buf),
            (None, Some(project)) => Some(project.assets_dir().join("scenes")),
            (None, None) => std::env::current_dir().ok(),
        };
        dir.map_or_else(|| PathBuf::from(&file), |d| d.join(&file))
    }

    /// A path as shown in a modal: project-relative inside the project.
    fn display_path(&self, path: &Path) -> String {
        match &self.state.project {
            Some(p) => p.relative(path),
            None => path.display().to_string(),
        }
    }

    /// Modal text to a path: relative paths are inside the project, or the
    /// working folder without one. `None` for blank input.
    fn resolve_input(&self, text: &str) -> Option<PathBuf> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let path = PathBuf::from(text);
        if path.is_absolute() {
            return Some(path);
        }
        let base = match &self.state.project {
            Some(p) => p.root().to_path_buf(),
            None => std::env::current_dir().unwrap_or_default(),
        };
        Some(base.join(path))
    }

    /// Draws the open modal and applies the user's answer.
    pub(crate) fn dialog_ui(&mut self, ctx: &egui::Context) {
        self.show_crash_notice();
        let t = self.tokens;
        let scene = self.state.scene.name.clone();
        let has_file = self.state.scene.path.is_some();
        let in_project = self.state.project.is_some();
        self.load_previews(ctx);
        let native_dialogs = self.native_dialogs;
        let previews = &self.previews;
        let project = self.state.project.as_ref();
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut outcome = None;
        let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(16);
        let modal = egui::Modal::new(egui::Id::unique("box_editor_dialog")).frame(frame);
        modal.show(ctx, |ui| {
            outcome = match dialog {
                Dialog::Unsaved { action, error } => {
                    unsaved_ui(ui, &t, &scene, has_file, action, error.as_deref())
                }
                Dialog::Recovery { recovery } => {
                    let file = match project {
                        Some(p) => p.relative(recovery),
                        None => recovery.display().to_string(),
                    };
                    recovery_ui(ui, &t, &scene, &file)
                }
                Dialog::Upgrade {
                    name,
                    summary,
                    can_upgrade,
                    error,
                    ..
                } => upgrade_ui(ui, &t, name, summary, *can_upgrade, error.as_deref()),
                Dialog::Error { title, message } => error_ui(ui, &t, title, message),
                Dialog::DeleteAsset { path, error, .. } => {
                    delete_asset_ui(ui, &t, path, error.as_deref())
                }
                Dialog::Shortcuts => shortcuts_ui(ui, &t),
                Dialog::CrashReport { report } => crash_report_ui(ui, &t, report),
                Dialog::SaveAs { path, error, .. } => {
                    let hint = if in_project {
                        "Scene file (.scene.json is added; relative paths are inside the project)."
                    } else {
                        "Scene file (.scene.json is added; relative paths start in the working folder)."
                    };
                    path_ui(ui, &t, "Save Scene As", hint, "Save", path, error.as_deref())
                        .map(Outcome::SaveAs)
                }
                Dialog::Project { path, error, .. } => path_ui(
                    ui,
                    &t,
                    "Open Project",
                    "Project folder (the one holding box.project.json).",
                    "Open",
                    path,
                    error.as_deref(),
                )
                .map(Outcome::Project),
                Dialog::NewProject {
                    templates,
                    template,
                    path,
                    error,
                    ..
                } => crate::new_project::new_project_ui(
                    ui,
                    &t,
                    previews,
                    templates,
                    template,
                    path,
                    error.as_deref(),
                    native_dialogs,
                ),
                Dialog::OpenExample {
                    examples,
                    example,
                    path,
                    error,
                    ..
                } => crate::new_project::open_example_ui(
                    ui,
                    &t,
                    previews,
                    examples,
                    example,
                    path,
                    error.as_deref(),
                    native_dialogs,
                ),
            };
        });
        match outcome {
            Some(Outcome::Unsaved(choice)) => self.answer_unsaved(choice),
            Some(Outcome::Recovery { restore }) => self.answer_recovery(restore),
            Some(Outcome::Upgrade(choice)) => self.answer_upgrade(choice),
            Some(Outcome::SaveAs(path)) => self.answer_save_as(path.as_deref()),
            Some(Outcome::Project(path)) => self.answer_project(path.as_deref()),
            Some(Outcome::NewProject(path)) => self.answer_new_project(path.as_deref()),
            Some(Outcome::SelectTemplate(name)) => self.select_template(&name),
            Some(Outcome::OpenExample(path)) => self.answer_open_example(path.as_deref()),
            Some(Outcome::SelectExample(name)) => self.select_example(&name),
            Some(Outcome::DismissError) => self.dismiss_error(),
            Some(Outcome::DeleteAsset(confirm)) => self.answer_delete_asset(confirm),
            Some(Outcome::CloseShortcuts) => self.close_shortcuts(),
            Some(Outcome::CrashReport { open }) => self.answer_crash_report(open),
            None => {}
        }
    }
}

pub(crate) fn title(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(t.strong()).color(t.text_strong));
    ui.add_space(6.0);
}

pub(crate) fn primary_button(ui: &mut Ui, t: &Tokens, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(t.accent_text))
            .fill(t.accent_bg)
            .min_size(vec2(84.0, 24.0)),
    )
}

pub(crate) fn button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(label).min_size(vec2(84.0, 24.0)))
}

pub(crate) fn error_line(ui: &mut Ui, t: &Tokens, error: Option<&str>) {
    if let Some(e) = error {
        ui.add_space(6.0);
        ui.label(RichText::new(e).color(t.error));
    }
}

/// Enter saves, Escape cancels.
fn unsaved_ui(
    ui: &mut Ui,
    t: &Tokens,
    scene: &str,
    has_file: bool,
    action: &DocAction,
    error: Option<&str>,
) -> Option<Outcome> {
    ui.set_width(420.0);
    title(
        ui,
        t,
        &format!("Save changes to “{scene}” {}?", action.before()),
    );
    let body = if has_file {
        "Your changes will be lost if you don't save them."
    } else {
        "This scene has no file yet; Save asks where to put it. Your changes will be lost if you don't save them."
    };
    ui.label(RichText::new(body).color(t.text_weak));
    error_line(ui, t, error);
    ui.add_space(14.0);
    let mut choice = None;
    ui.horizontal(|ui| {
        if button(ui, "Don't Save").clicked() {
            choice = Some(UnsavedChoice::DontSave);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, t, "Save").clicked() {
                choice = Some(UnsavedChoice::Save);
            }
            if button(ui, "Cancel").clicked() {
                choice = Some(UnsavedChoice::Cancel);
            }
        });
    });
    let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if enter {
        choice = Some(UnsavedChoice::Save);
    } else if escape {
        choice = Some(UnsavedChoice::Cancel);
    }
    choice.map(Outcome::Unsaved)
}

/// Enter restores. There is no cancel: the file is either used or deleted.
fn recovery_ui(ui: &mut Ui, t: &Tokens, scene: &str, file: &str) -> Option<Outcome> {
    ui.set_width(420.0);
    title(ui, t, &format!("Restore unsaved changes to “{scene}”?"));
    ui.label(
        RichText::new(
            "A recovery copy newer than the scene file was kept from an earlier session \
             that ended with unsaved changes. Restore opens it as unsaved changes; Discard \
             deletes it.",
        )
        .color(t.text_weak),
    );
    ui.add_space(4.0);
    ui.label(RichText::new(file).font(t.mono()).color(t.text_faint));
    ui.add_space(14.0);
    let mut restore = None;
    ui.horizontal(|ui| {
        if button(ui, "Discard").clicked() {
            restore = Some(false);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, t, "Restore").clicked() {
                restore = Some(true);
            }
        });
    });
    if ui.input(|i| i.key_pressed(Key::Enter)) {
        restore = Some(true);
    }
    restore.map(|restore| Outcome::Recovery { restore })
}

/// Enter opens the report, Escape dismisses it.
fn crash_report_ui(ui: &mut Ui, t: &Tokens, report: &Path) -> Option<Outcome> {
    ui.set_width(460.0);
    title(ui, t, "The editor crashed last time");
    ui.label(
        RichText::new(
            "A crash report was saved on this computer (it is never uploaded). It says what \
             went wrong and where; attach it when you report the problem. Unsaved scene \
             changes are offered for restore when you open the scene.",
        )
        .color(t.text_weak),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(report.display().to_string())
            .font(t.mono())
            .color(t.text_faint),
    );
    ui.add_space(14.0);
    let mut open = None;
    ui.horizontal(|ui| {
        if button(ui, "Dismiss").clicked() {
            open = Some(false);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, t, "Open Crash Report").clicked() {
                open = Some(true);
            }
        });
    });
    let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if enter {
        open = Some(true);
    } else if escape {
        open = Some(false);
    }
    open.map(|open| Outcome::CrashReport { open })
}

/// Enter upgrades (when possible), Escape cancels.
fn upgrade_ui(
    ui: &mut Ui,
    t: &Tokens,
    name: &str,
    summary: &str,
    can_upgrade: bool,
    error: Option<&str>,
) -> Option<Outcome> {
    ui.set_width(560.0);
    title(
        ui,
        t,
        &format!("Upgrade “{name}” to this version of box-engine?"),
    );
    let body = if can_upgrade {
        "This project's files were written by an older engine or older game types. Upgrade \
         copies every file it changes to .box/backups/ and then migrates it; Open Read-Only \
         shows the project without writing any of its files."
    } else {
        "This project's files were written by an older engine or older game types, and some \
         cannot be migrated (below). Open Read-Only shows the project without writing any of \
         its files."
    };
    ui.label(RichText::new(body).color(t.text_weak));
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            ui.label(RichText::new(summary).font(t.mono()).color(t.text_faint));
        });
    error_line(ui, t, error);
    ui.add_space(14.0);
    let mut choice = None;
    ui.horizontal(|ui| {
        if button(ui, "Cancel").clicked() {
            choice = Some(UpgradeChoice::Cancel);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if can_upgrade && primary_button(ui, t, "Upgrade").clicked() {
                choice = Some(UpgradeChoice::Upgrade);
            }
            if button(ui, "Open Read-Only").clicked() {
                choice = Some(UpgradeChoice::ReadOnly);
            }
        });
    });
    let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if enter && can_upgrade {
        choice = Some(UpgradeChoice::Upgrade);
    } else if escape {
        choice = Some(UpgradeChoice::Cancel);
    }
    choice.map(Outcome::Upgrade)
}

/// A failure the user has to see. Enter or Escape closes it.
fn error_ui(ui: &mut Ui, t: &Tokens, heading: &str, message: &str) -> Option<Outcome> {
    ui.set_width(420.0);
    title(ui, t, heading);
    ui.label(RichText::new(message).color(t.error_text));
    ui.add_space(14.0);
    let mut close = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        close = primary_button(ui, t, "OK").clicked();
    });
    close |= ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape));
    close.then_some(Outcome::DismissError)
}

/// Enter moves the file to the trash, Escape keeps it.
fn delete_asset_ui(ui: &mut Ui, t: &Tokens, path: &str, error: Option<&str>) -> Option<Outcome> {
    ui.set_width(420.0);
    title(
        ui,
        t,
        &format!("Move “{}” to the trash?", crate::assets::file_name(path)),
    );
    ui.label(
        RichText::new(
            "The file and its .meta go to the project's .box/trash/ folder; move them back \
             into assets/ to restore them. References to it in scenes stop resolving. \
             This is not an undo step.",
        )
        .color(t.text_weak),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(format!("assets/{path}"))
            .font(t.mono())
            .color(t.text_faint),
    );
    error_line(ui, t, error);
    ui.add_space(14.0);
    let mut answer = None;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if primary_button(ui, t, "Move to Trash").clicked() {
            answer = Some(true);
        }
        if button(ui, "Cancel").clicked() {
            answer = Some(false);
        }
    });
    let (enter, escape) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    if enter {
        answer = Some(true);
    } else if escape {
        answer = Some(false);
    }
    answer.map(Outcome::DeleteAsset)
}

/// Help ▸ Keyboard Shortcuts: the shortcut table (`shortcuts.rs`) by group,
/// then the camera navigation (`camera::NAVIGATION`). Escape, Enter or
/// Close closes it.
fn shortcuts_ui(ui: &mut Ui, t: &Tokens) -> Option<Outcome> {
    ui.set_width(560.0);
    title(ui, t, "Keyboard Shortcuts");
    let row = |ui: &mut Ui, keys: &str, label: &str| {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(130.0, 18.0), egui::Sense::hover());
            ui.painter().text(
                r.left_center(),
                egui::Align2::LEFT_CENTER,
                keys,
                t.mono(),
                t.text_strong,
            );
            ui.label(RichText::new(label).color(t.text));
        });
    };
    let heading = |ui: &mut Ui, text: &str| {
        ui.add_space(8.0);
        ui.label(
            RichText::new(text)
                .font(t.strong_small())
                .color(t.text_weak),
        );
        ui.add_space(2.0);
    };
    let mut group = "";
    for s in crate::shortcuts::SHORTCUTS {
        if s.group != group {
            group = s.group;
            heading(ui, group);
        }
        let keys = crate::shortcuts::text(ui.ctx(), s.command);
        row(ui, &keys, s.label);
    }
    heading(ui, "Viewport navigation");
    for (what, how) in crate::camera::NAVIGATION {
        row(ui, what, how);
    }
    ui.add_space(14.0);
    let mut close = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        close = primary_button(ui, t, "Close").clicked();
    });
    close |= ui.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape));
    close.then_some(Outcome::CloseShortcuts)
}

/// A path-entry modal. `Some(Some(text))` confirms, `Some(None)` cancels.
fn path_ui(
    ui: &mut Ui,
    t: &Tokens,
    heading: &str,
    hint: &str,
    ok: &str,
    path: &mut String,
    error: Option<&str>,
) -> Option<Option<String>> {
    ui.set_width(460.0);
    title(ui, t, heading);
    ui.label(RichText::new(hint).color(t.text_weak));
    ui.add_space(6.0);
    let r = ui.add(
        egui::TextEdit::singleline(path)
            .font(t.mono())
            .desired_width(f32::INFINITY),
    );
    if !r.has_focus() && !r.lost_focus() {
        r.request_focus();
    }
    error_line(ui, t, error);
    ui.add_space(10.0);
    let mut result = None;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, t, ok).clicked()
                || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
            {
                result = Some(Some(path.clone()));
            }
            if button(ui, "Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                result = Some(None);
            }
        });
    });
    result
}

/// Asks the platform for a project folder. `None` when the user cancels.
fn native_pick_project(start: Option<PathBuf>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(dir) = start {
        dialog = dialog.set_directory(dir);
    }
    dialog.set_title("Open Project").pick_folder()
}

/// Asks the platform where to save a scene. `None` when the user cancels.
fn native_save_scene(default: &Path) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Save Scene As")
        .add_filter("Scene", &["json"]);
    if let Some(dir) = default.parent().filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    if let Some(name) = default.file_name() {
        dialog = dialog.set_file_name(name.to_string_lossy());
    }
    dialog.save_file().map(|p| normalize_scene_path(&p))
}
