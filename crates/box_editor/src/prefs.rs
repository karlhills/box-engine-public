//! Per-project editor preferences in `<project>/.box/editor.json`: the snap
//! steps and toggle, whether the viewport's navigation hint was dismissed,
//! and the last session (open scene, editor camera, selection). Loaded with
//! the project's layout, written when one of them changes (never per frame),
//! before switching projects and on quit. Without a project they last for
//! the session only.
//!
//! The session is restored when a project opens in a windowed run (unless
//! `--no-restore`) or with `--restore`: the scene first, then the camera and
//! the selection if that scene is the one they were saved with (selected
//! entities that no longer exist are skipped). The active dock tabs are part
//! of the layout (`layout.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use box_core::identity::Uid;
use glam::Vec3;

use crate::state::{EditorState, Snap};

/// The file's contents. Missing fields take their defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorPrefs {
    pub snap: SnapPrefs,
    pub nav_hint_dismissed: bool,
    pub session: SessionPrefs,
}

impl Default for EditorPrefs {
    fn default() -> Self {
        Self {
            snap: SnapPrefs::from(Snap::default()),
            nav_hint_dismissed: false,
            session: SessionPrefs::default(),
        }
    }
}

/// Where the last session left off.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionPrefs {
    /// The open scene, relative to the project root (`None` for a scene
    /// without a file or outside the project).
    pub scene: Option<String>,
    pub camera: Option<CameraPrefs>,
    /// Selected entities, the primary selection last.
    pub selection: Vec<Uid>,
}

/// The editor camera's orbit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraPrefs {
    pub target: [f32; 3],
    /// Metres from the target.
    pub distance: f32,
    /// Radians.
    pub yaw: f32,
    /// Radians.
    pub pitch: f32,
}

impl CameraPrefs {
    fn is_valid(&self) -> bool {
        self.target.iter().all(|v| v.is_finite())
            && self.distance.is_finite()
            && self.distance > 0.0
            && self.yaw.is_finite()
            && self.pitch.is_finite()
    }
}

/// [`Snap`] as stored.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapPrefs {
    pub enabled: bool,
    /// Metres.
    pub translate: f32,
    /// Degrees.
    pub rotate: f32,
    pub scale: f32,
}

impl Default for SnapPrefs {
    fn default() -> Self {
        Self::from(Snap::default())
    }
}

impl From<Snap> for SnapPrefs {
    fn from(s: Snap) -> Self {
        Self {
            enabled: s.enabled,
            translate: s.translate,
            rotate: s.rotate,
            scale: s.scale,
        }
    }
}

impl SnapPrefs {
    /// As a [`Snap`]; a step that is not a positive number keeps the default.
    pub fn to_snap(self) -> Snap {
        let d = Snap::default();
        let step = |v: f32, default: f32| if v.is_finite() && v > 0.0 { v } else { default };
        Snap {
            enabled: self.enabled,
            translate: step(self.translate, d.translate),
            rotate: step(self.rotate, d.rotate),
            scale: step(self.scale, d.scale),
        }
    }
}

/// `<project>/.box/editor.json`.
pub fn prefs_path(project: &box_engine::Project) -> PathBuf {
    project.local_dir().join("editor.json")
}

/// Reads the preferences; a missing file is the defaults. A broken file is
/// reported and ignored (it is rewritten at the next change).
pub fn load(path: &Path) -> EditorPrefs {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(
                "{}: {e}; using the default editor preferences",
                path.display()
            );
            EditorPrefs::default()
        }),
        Err(_) => EditorPrefs::default(),
    }
}

impl EditorState {
    /// The preferences as they are now.
    pub fn prefs(&self) -> EditorPrefs {
        EditorPrefs {
            snap: self.snap.into(),
            nav_hint_dismissed: !self.nav_hint,
            session: self.session(),
        }
    }

    /// The session as it is now (see [`SessionPrefs`]).
    pub fn session(&self) -> SessionPrefs {
        let scene = match (&self.project, &self.scene.path) {
            (Some(project), Some(path)) => {
                let rel = project.relative(path);
                project.try_resolve(&rel).is_ok().then_some(rel)
            }
            _ => None,
        };
        let o = &self.camera.orbit;
        SessionPrefs {
            scene,
            camera: Some(CameraPrefs {
                target: o.target.to_array(),
                distance: o.distance,
                yaw: o.yaw,
                pitch: o.pitch,
            }),
            selection: self.selection.clone(),
        }
    }

    /// Loads the open project's preferences (the defaults without one) and
    /// returns its saved session.
    pub(crate) fn load_prefs(&mut self) -> SessionPrefs {
        let prefs = match &self.project {
            Some(p) => load(&prefs_path(p)),
            None => EditorPrefs::default(),
        };
        self.snap = prefs.snap.to_snap();
        self.nav_hint = !prefs.nav_hint_dismissed;
        prefs.session
    }

    /// Restores a saved session in the open project: opens its scene (when
    /// it still exists and is not open), then, if that scene is open, the
    /// camera and the selection (entities that still exist). Never replaces
    /// unsaved changes or a start scene that failed to load. Returns true
    /// when the camera and selection were restored.
    pub fn apply_session(&mut self, session: &SessionPrefs) -> bool {
        let Some(project) = &self.project else {
            return false;
        };
        let Some(rel) = &session.scene else {
            return false;
        };
        if self.is_dirty() || self.scene.path.is_none() {
            return false;
        }
        let path = match project.try_resolve(rel) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("session: {e}");
                return false;
            }
        };
        if self.scene.path.as_deref() != Some(path.as_path()) {
            if !path.is_file() {
                tracing::info!("session: {rel} no longer exists; the start scene is open");
                return false;
            }
            if let Err(e) = self.open_scene(&path, false) {
                tracing::warn!("session: could not reopen {rel}: {e:#}");
                return false;
            }
        }
        if let Some(c) = session.camera.filter(CameraPrefs::is_valid) {
            let o = &mut self.camera.orbit;
            // `clamp` panics on swapped limits; these never do.
            let within = |v: f32, a: f32, b: f32| v.max(a.min(b)).min(a.max(b));
            o.target = Vec3::from_array(c.target);
            o.distance = within(c.distance, o.min_distance, o.max_distance);
            o.yaw = c.yaw;
            o.pitch = within(c.pitch, o.min_pitch, o.max_pitch);
        }
        let world = &self.world;
        let selection: Vec<Uid> = session
            .selection
            .iter()
            .copied()
            .filter(|u| world.entity_by_uid(*u).is_some())
            .collect();
        self.close_asset_inspector();
        self.selection = selection;
        if let Some(p) = self.primary() {
            self.reveal(p);
        }
        tracing::info!("session restored: {rel}");
        true
    }

    /// Writes the preferences to the open project (nothing without one).
    /// Call after a change, not per frame.
    pub fn save_prefs(&self) {
        let Some(project) = &self.project else {
            return;
        };
        let path = prefs_path(project);
        let result = serde_json::to_vec_pretty(&self.prefs())
            .map_err(anyhow::Error::from)
            .and_then(|mut bytes| {
                bytes.push(b'\n');
                crate::document::write_atomic(&path, &bytes)
            });
        if let Err(e) = result {
            tracing::warn!("could not save editor preferences: {e:#}");
        }
    }

    /// Hides the navigation hint for good (per project).
    pub fn dismiss_nav_hint(&mut self) {
        if self.nav_hint {
            self.nav_hint = false;
            self.save_prefs();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_steps_fall_back_to_the_defaults() {
        let p = SnapPrefs {
            enabled: true,
            translate: -1.0,
            rotate: f32::NAN,
            scale: 0.25,
        };
        let s = p.to_snap();
        assert_eq!(
            (s.enabled, s.translate, s.rotate, s.scale),
            (true, 0.5, 15.0, 0.25)
        );
        let partial: EditorPrefs = serde_json::from_str(r#"{"snap":{"rotate":5}}"#).unwrap();
        assert_eq!(partial.snap.rotate, 5.0);
        assert_eq!(partial.snap.translate, 0.5);
        assert!(!partial.nav_hint_dismissed);
    }
}
