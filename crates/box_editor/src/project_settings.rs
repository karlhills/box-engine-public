//! Game ▸ Project Settings: the game's settings resources
//! (`Registry::register_settings`, files in `assets/settings/`, see
//! `box_engine::settings`) shown and edited in the inspector.
//!
//! Like material files, settings are file edits, not scene edits: there is
//! no undo, the scene does not become dirty, and an edit is written to
//! `assets/settings/<Type>.json` (atomically) when nothing is being dragged
//! or typed, or [`SETTINGS_WRITE_DELAY_S`] after the last change. The edit
//! applies at once to the edit world and, during Play, to the play world,
//! so tuning while playing is immediate and kept after Stop. Changes to the
//! file from outside (a text editor, an agent) hot-reload into both.
//!
//! When the editor opens a project it creates a file with the defaults for
//! each settings type that has none.

use box_core::reflect::PartialReflect;
use box_engine::settings::{self, Missing, ProjectSettings, pretty_json, settings_file};

use crate::state::EditorState;

/// Seconds after the last change before a settings edit is written even
/// while the pointer is still down (a long drag).
pub(crate) const SETTINGS_WRITE_DELAY_S: f64 = 0.3;

/// The settings resource the inspector shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsView {
    pub name: &'static str,
    /// UI time of the last unwritten change.
    pending_since: Option<f64>,
}

impl EditorState {
    /// The game's settings types, by name.
    pub fn settings_types(&self) -> Vec<&'static str> {
        self.game.registry().settings().map(|t| t.name()).collect()
    }

    /// Shows the settings resource `name` in the inspector (Game ▸ Project
    /// Settings). Needs a project. Returns whether it is shown.
    pub fn show_project_settings(&mut self, name: &str) -> bool {
        let Some(name) = self.settings_types().into_iter().find(|n| *n == name) else {
            tracing::warn!("`{name}` is not a settings type of this game");
            return false;
        };
        if self.project.is_none() {
            tracing::warn!("open a project to edit its settings");
            return false;
        }
        self.close_asset_inspector();
        self.settings_view = Some(SettingsView {
            name,
            pending_since: None,
        });
        true
    }

    /// The settings resource the inspector shows, if any.
    pub fn project_settings_shown(&self) -> Option<&'static str> {
        self.settings_view.as_ref().map(|v| v.name)
    }

    /// Back to the entity inspector, writing a pending settings edit.
    pub fn close_project_settings(&mut self) {
        self.flush_settings_edit(f64::INFINITY, true);
        self.settings_view = None;
    }

    /// Loads the project's settings into the edit world, creating missing
    /// files with the defaults (called when the asset server is attached).
    pub(crate) fn load_project_settings_resources(&mut self) {
        // A read-only project gets no new files.
        let missing = if self.read_only {
            Missing::UseDefault
        } else {
            Missing::CreateFile
        };
        settings::load_settings(self.edit_world_mut(), missing);
    }

    /// Hot reload of settings files into the edit world (the play world
    /// runs its own); once per frame after the asset update.
    pub(crate) fn reload_project_settings(&mut self) {
        settings::reload_settings(self.edit_world_mut());
    }

    /// Sets a field (`"CharacterSettings.skin"`) or the whole value of the
    /// shown settings resource in the world shown and, during Play, in the
    /// edit world too. Written to the file later
    /// ([`Self::flush_settings_edit`]). `now` is the UI time in seconds.
    pub fn edit_settings(&mut self, path: &str, value: &dyn PartialReflect, now: f64) -> bool {
        let Some(view) = &self.settings_view else {
            return false;
        };
        let name = view.name;
        if let Err(e) = self.ensure_writable() {
            tracing::warn!("{name}: {e:#}");
            return false;
        }
        let target = path.split('.').next().unwrap_or_default();
        if target != name {
            tracing::warn!("`{path}` is not a field of `{name}`");
            return false;
        }
        let set = |world: &mut box_core::world::World| {
            world
                .field_path(path)
                .and_then(|p| world.set_resource_field(&p, value))
        };
        if let Err(e) = set(&mut self.world) {
            tracing::warn!("set {path}: {e}");
            return false;
        }
        if self.is_playing()
            && let Err(e) = set(self.edit_world_mut())
        {
            tracing::warn!("set {path} in the edit world: {e}");
        }
        if let Some(v) = &mut self.settings_view {
            v.pending_since = Some(now);
        }
        true
    }

    /// Writes a pending settings edit to `assets/settings/<Type>.json` when
    /// `idle` (no drag or typing) or `SETTINGS_WRITE_DELAY_S` after the
    /// last change. The write's hot reload is not applied back over newer
    /// edits.
    pub fn flush_settings_edit(&mut self, now: f64, idle: bool) {
        let Some(view) = &mut self.settings_view else {
            return;
        };
        let Some(since) = view.pending_since else {
            return;
        };
        if !idle && now - since < SETTINGS_WRITE_DELAY_S {
            return;
        }
        view.pending_since = None;
        let name = view.name;
        let Some(project) = &self.project else {
            return;
        };
        let rel = settings_file(name);
        let path = project.assets_dir().join(&rel);
        let json = match settings::settings_file_json(self.edit_world(), name, &path) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("{rel}: {e}");
                return;
            }
        };
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(anyhow::Error::from)
            .and_then(|()| crate::document::write_atomic(&path, &pretty_json(&json)));
        if let Err(e) = written {
            tracing::error!("could not write {rel}: {e:#}");
            return;
        }
        if let Some(s) = self.edit_world_mut().get_resource_mut::<ProjectSettings>() {
            s.expect_echo(name, json.clone());
        }
        if self.is_playing()
            && let Some(s) = self.world.get_resource_mut::<ProjectSettings>()
        {
            s.expect_echo(name, json);
        }
    }
}
