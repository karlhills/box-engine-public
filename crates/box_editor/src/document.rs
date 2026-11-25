//! Projects and scene files: open, create, save, save as, and the recovery
//! copy written instead of silently losing unsaved changes: periodically
//! while editing (so a crash loses little), on quit and when leaving a scene.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use std::path::Component;
use std::sync::Arc;

use box_core::prelude::World;
use box_core::reflect::Registry;
use box_core::scene;
use box_engine::Project;
use box_engine::project::SCENE_EXTENSION;

use crate::scratch::prepare_world;
use crate::state::EditorState;

/// Writes `bytes` next to `path` and renames it into place, so a crash never
/// leaves a half-written scene.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    // The io error with what to do about it (permissions, a full disk).
    let fail = |what: &str, p: &Path, e: std::io::Error| {
        anyhow::anyhow!("{what} {}: {}", p.display(), box_engine::io_hint::text(&e))
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail("creating", dir, e))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| fail("writing", &tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| fail("replacing", path, e))
}

/// A new project's name: its folder's name.
pub(crate) fn folder_name(dir: &Path) -> String {
    std::path::absolute(dir)
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "New Project".into())
}

/// The scene name shown in the title: the file name without `.scene.json`.
pub fn scene_name(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.strip_suffix(SCENE_EXTENSION)
        .map(str::to_owned)
        .unwrap_or(file)
}

/// `path` with the `.scene.json` extension: kept when present, replacing a
/// bare `.json` (what a native save panel appends), appended otherwise.
pub fn normalize_scene_path(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return path.to_owned();
    };
    if name.ends_with(SCENE_EXTENSION) {
        return path.to_owned();
    }
    let stem = name.strip_suffix(".json").unwrap_or(&name);
    path.with_file_name(format!("{stem}{SCENE_EXTENSION}"))
}

/// True when `recovery` exists and was modified after `scene` (or `scene`
/// is missing): only then is it worth offering to restore.
pub fn recovery_is_newer(scene: &Path, recovery: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(r) = modified(recovery) else {
        return false;
    };
    modified(scene).is_none_or(|s| r > s)
}

/// Seconds between periodic recovery writes while there are unsaved changes.
pub const RECOVERY_INTERVAL_S: f64 = 30.0;

/// Where the recovery copy of `scene` lives: the scene's project-relative
/// path mirrored under `<project>/.box/recovery/project/`, so scenes with the
/// same file name in different folders never share one. A scene outside the
/// project (Save As elsewhere) mirrors its absolute path under
/// `.box/recovery/external/`. Files written directly into `.box/recovery/`
/// by the older name-keyed scheme are never looked at.
pub fn recovery_path_for(project: &Project, scene: &Path) -> PathBuf {
    let dir = project.local_dir().join("recovery");
    let abs = std::path::absolute(scene).unwrap_or_else(|_| scene.to_path_buf());
    let normal = |p: &Path| -> PathBuf {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(n) => Some(n),
                _ => None,
            })
            .collect()
    };
    match abs.strip_prefix(project.root()) {
        Ok(rel) if rel.components().all(|c| matches!(c, Component::Normal(_))) => {
            dir.join("project").join(rel)
        }
        _ => dir.join("external").join(normal(&abs)),
    }
}

/// Reads and prepares a scene file (with the game's registry) without
/// touching the editor.
fn read_scene(path: &Path, registry: Arc<Registry>) -> Result<World> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let world =
        scene::load(&bytes, registry).with_context(|| format!("loading {}", path.display()))?;
    Ok(prepared(world))
}

fn prepared(mut world: World) -> World {
    prepare_world(&mut world);
    box_core::transform::propagate_transforms(&mut world);
    box_engine::sync_scene_meshes(&mut world);
    world
}

/// The contents of a new scene (File ▸ New Scene, Assets ▸ New ▸ Scene):
/// the project starter scene ([`box_engine::project::starter_scene`]: sun,
/// ground, camera and the day environment) without its sample cube, with
/// fresh uids.
pub fn new_scene_bytes() -> Result<Vec<u8>> {
    let mut world = box_engine::project::starter_scene();
    let samples: Vec<_> = world
        .query::<(box_core::prelude::Entity, &box_core::identity::Name)>()
        .iter()
        .filter(|(_, n)| n.0 == "Cube")
        .map(|(e, _)| e)
        .collect();
    for e in samples {
        world.despawn(e);
    }
    Ok(scene::save(&world)?)
}

impl EditorState {
    fn set_scene(&mut self, world: World, path: &Path) {
        self.replace_world(world);
        self.scene.name = scene_name(path);
        self.scene.path = Some(path.to_owned());
        self.sync_control_scene();
        tracing::info!(
            "scene: {} ({} entities)",
            path.display(),
            self.world.entity_count()
        );
        self.announce_unknown_components();
    }

    /// Loads a scene file into the edit world (undo history is cleared). The
    /// current scene's unsaved changes are not kept; see [`Self::open_scene`].
    pub fn load_scene(&mut self, path: &Path) -> Result<()> {
        let world = read_scene(path, self.game.registry())?;
        self.set_scene(world, path);
        Ok(())
    }

    /// Switches to another scene file and frames it. Unsaved changes to the
    /// current scene go to its recovery file, or with `discard` are dropped
    /// (with any recovery file). Nothing changes if the file fails to load.
    pub fn open_scene(&mut self, path: &Path, discard: bool) -> Result<()> {
        let world = read_scene(path, self.game.registry())?;
        self.leave_scene(discard);
        self.set_scene(world, path);
        self.frame_on_open();
        Ok(())
    }

    /// Replaces the scene with a new, unsaved one ([`new_scene_bytes`])
    /// named after the first free `untitled` file name in the project's
    /// `assets/scenes/`; Save goes to Save As there. Unsaved changes to the
    /// current scene go to its recovery file, or with `discard` are dropped.
    pub fn new_scene(&mut self, discard: bool) -> Result<()> {
        let world = prepared(scene::load(&new_scene_bytes()?, self.game.registry())?);
        self.leave_scene(discard);
        self.replace_world(world);
        let taken = |name: &str| {
            self.project.as_ref().is_some_and(|p| {
                p.assets_dir()
                    .join("scenes")
                    .join(format!("{name}{SCENE_EXTENSION}"))
                    .exists()
            })
        };
        let name = std::iter::once("untitled".to_string())
            .chain((2..).map(|i| format!("untitled {i}")))
            .find(|n| !taken(n))
            .unwrap_or_else(|| "untitled".into());
        self.scene = crate::state::SceneDoc { name, path: None };
        self.sync_control_scene();
        self.frame_on_open();
        tracing::info!(
            "scene: new scene ({} entities, not saved yet)",
            self.world.entity_count()
        );
        Ok(())
    }

    /// The upgrade the project at `path` needs (`box_engine::upgrade`), or
    /// `None` when its files are current. An error for a project that
    /// cannot be opened, including one with files from a newer engine or
    /// game. Writes nothing.
    pub fn upgrade_plan(&self, path: &Path) -> Result<Option<box_engine::upgrade::UpgradePlan>> {
        let project = Project::open(path)?;
        let plan = box_engine::upgrade::plan(project.root(), &self.game.registry())?;
        anyhow::ensure!(plan.newer.is_empty(), "{}", plan.summary());
        Ok((plan.needed() || plan.blocked()).then_some(plan))
    }

    /// An error when the open project is read-only ([`Self::read_only`]).
    pub fn ensure_writable(&self) -> Result<()> {
        anyhow::ensure!(
            !self.read_only,
            "the project is open read-only because its files are from an older engine; \
             reopen it and choose Upgrade (or run `box upgrade`) to edit it"
        );
        Ok(())
    }

    /// Opens a project for the editor's start: like [`Self::open_project`],
    /// but when only the start scene fails to load, the project is opened
    /// with an empty, unsaved scene (the file is left alone) and the scene
    /// error is returned as `Ok(Some(..))` to show. `Err` when the project
    /// itself cannot be opened (nothing changes).
    pub fn open_project_at_start(&mut self, path: &Path) -> Result<Option<anyhow::Error>> {
        let project = Project::open(path)?;
        self.read_only = false;
        let start = project.start_scene_path();
        match read_scene(&start, self.game.registry()) {
            Ok(world) => {
                self.project = Some(project);
                self.set_scene(world, &start);
                self.frame_on_open();
                Ok(None)
            }
            Err(e) => {
                let mut world = World::with_registry(self.game.registry());
                world.insert_resource(box_render::Environment::day());
                self.project = Some(project);
                self.replace_world(prepared(world));
                self.scene = crate::state::SceneDoc {
                    name: "untitled".into(),
                    path: None,
                };
                self.sync_control_scene();
                Ok(Some(e))
            }
        }
    }

    /// Opens a project and its start scene. Unsaved changes to the current
    /// scene are written to its recovery file first.
    pub fn open_project(&mut self, path: &Path) -> Result<()> {
        self.switch_project(path, false)
    }

    /// Opens a project; `discard` drops the current scene's unsaved changes
    /// instead of writing them to the recovery file. Nothing changes if the
    /// project or its start scene fails to load.
    pub(crate) fn switch_project(&mut self, path: &Path, discard: bool) -> Result<()> {
        self.switch_project_mode(path, discard, false)
    }

    /// [`Self::switch_project`]; `read_only` opens it without writing any
    /// of its files ([`Self::read_only`]): the choice for a project that
    /// needs an upgrade the user did not want yet.
    pub(crate) fn switch_project_mode(
        &mut self,
        path: &Path,
        discard: bool,
        read_only: bool,
    ) -> Result<()> {
        let project = Project::open(path)?;
        let start = project.start_scene_path();
        let world = read_scene(&start, self.game.registry())?;
        self.read_only = read_only;
        // The recovery path depends on the outgoing project.
        self.leave_scene(discard);
        // Agent control was allowed for the outgoing project only.
        let same = self.project.as_ref().map(|p| p.root()) == Some(project.root());
        if self.agent_control().is_some() && !same {
            self.set_agent_control(false);
            tracing::info!(
                "agent control stopped with the project switch; turn it on again (File ▸ Allow Agent Control) for {}",
                project.name()
            );
        }
        self.project = Some(project);
        // Open scripts belong to the project that was open.
        self.scripts = Default::default();
        self.set_scene(world, &start);
        self.frame_on_open();
        Ok(())
    }

    /// Creates a project in `dir` (named after the folder) from the default
    /// template and opens it.
    pub fn create_project(&mut self, dir: &Path) -> Result<()> {
        let list = box_engine::templates::templates();
        let template =
            box_engine::templates::find_template(&list, box_engine::templates::DEFAULT_TEMPLATE)?;
        let project =
            box_engine::templates::create_project(dir, &folder_name(dir), &list, template)?;
        tracing::info!("project created: {}", project.root().display());
        self.switch_project(project.root(), false)
    }

    fn leave_scene(&mut self, discard: bool) {
        if discard {
            self.discard_changes();
        } else {
            self.write_recovery_if_dirty();
        }
    }

    /// Forgets the scene's unsaved changes (the world is left as it is; the
    /// caller replaces it or quits) and deletes its recovery file.
    pub fn discard_changes(&mut self) {
        self.stop_play();
        self.finish_edit();
        self.mark_saved();
        self.remove_recovery();
    }

    fn remove_recovery(&mut self) {
        if let Some(r) = self.recovery_path() {
            let _ = std::fs::remove_file(r);
        }
        self.recovery.written = false;
        self.recovery.generation = self.generation;
    }

    /// Saves the scene to its file. Returns the path written. A scene without
    /// a file (the scratch scene) needs [`Self::save_scene_as`].
    pub fn save_scene(&mut self) -> Result<PathBuf> {
        let path = self
            .scene
            .path
            .clone()
            .context("the scene has no file yet; use Save Scene As")?;
        self.write_scene(&path)?;
        tracing::info!("saved {}", path.display());
        self.notify_saved(&path);
        Ok(path)
    }

    /// Saves the scene to `path` (`.scene.json` is added when missing) and
    /// makes it the current scene file. Works with or without a project.
    pub fn save_scene_as(&mut self, path: &Path) -> Result<PathBuf> {
        let path = normalize_scene_path(path);
        let path = std::path::absolute(&path).unwrap_or(path);
        // Also drops the outgoing file's recovery copy: its changes are saved here.
        self.write_scene(&path)?;
        self.scene.name = scene_name(&path);
        self.scene.path = Some(path.clone());
        self.sync_control_scene();
        // A stale recovery file for the new path would offer old changes.
        self.remove_recovery();
        tracing::info!("saved as {}", path.display());
        self.notify_saved(&path);
        Ok(path)
    }

    /// "Saved main.scene.json" (a toast for the explicit action).
    fn notify_saved(&mut self, path: &Path) {
        let file = path.file_name().map_or_else(
            || path.display().to_string(),
            |f| f.to_string_lossy().into_owned(),
        );
        self.notify(&format!("Saved {file}"));
    }

    /// Atomic write of the world to `path`; marks the scene clean and drops
    /// the recovery file.
    fn write_scene(&mut self, path: &Path) -> Result<()> {
        // `world` is the play world while playing; it is never saved.
        anyhow::ensure!(!self.is_playing(), "Stop Play before saving the scene");
        self.ensure_writable()?;
        // End an active drag as one step so the file matches the undo history.
        self.finish_edit();
        self.canonicalize_asset_refs();
        let bytes = scene::save(&self.world)?;
        write_atomic(path, &bytes)?;
        self.mark_saved();
        self.remove_recovery();
        Ok(())
    }

    /// Before a save: writes ids and current paths into the scene's asset
    /// references (`box_engine::asset_glue::canonicalize_scene_refs`) as one
    /// undo step, so the file holds ids even for references typed as paths.
    /// A failure is logged and the scene is saved as it is.
    fn canonicalize_asset_refs(&mut self) {
        let Some(server) = self.asset_server.clone() else {
            return;
        };
        self.edits.begin("Repair asset references");
        let result = box_engine::asset_glue::canonicalize_scene_refs(
            &mut self.world,
            &mut self.edits,
            &server,
        );
        let committed = self.edits.commit();
        match result {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!("save: {n} asset reference(s) written with ids and current paths");
                if committed {
                    self.changed(false);
                }
            }
            Err(e) => tracing::warn!("save: asset references not repaired: {e}"),
        }
    }

    /// The recovery copy of the open scene (see [`recovery_path_for`]);
    /// `None` without a project or for a scene with no file.
    pub fn recovery_path(&self) -> Option<PathBuf> {
        let project = self.project.as_ref()?;
        Some(recovery_path_for(project, self.scene.path.as_ref()?))
    }

    /// The scene's recovery file, when it exists and is newer than the scene.
    pub fn recovery_candidate(&self) -> Option<PathBuf> {
        let scene = self.scene.path.as_ref()?;
        let recovery = self.recovery_path()?;
        recovery_is_newer(scene, &recovery).then_some(recovery)
    }

    /// Replaces the edit world with the recovery file's contents, as unsaved
    /// changes to the current scene (the recovery file stays until the scene
    /// is saved or the changes are discarded).
    pub fn restore_recovery(&mut self) -> Result<()> {
        let path = self
            .recovery_path()
            .context("no project, so no recovery file")?;
        let world = read_scene(&path, self.game.registry())?;
        self.replace_world(world);
        // Dirty: the restored changes are not in the scene file. The
        // recovery file already holds them.
        self.mark_unsaved();
        self.recovery.written = true;
        tracing::info!("restored unsaved changes from {}", path.display());
        Ok(())
    }

    /// With unsaved changes in a project scene, writes them to the recovery
    /// file (never over the scene itself). Returns the path written.
    pub fn write_recovery_if_dirty(&mut self) -> Option<PathBuf> {
        // Ends Play first: only the edit world is ever written.
        self.stop_play();
        self.finish_edit();
        if !self.is_dirty() || self.scene.path.is_none() {
            return None;
        }
        self.write_recovery()
    }

    /// True when the world changed since the recovery copy was last brought
    /// up to date: the per-frame check, one integer compare.
    pub fn recovery_pending(&self) -> bool {
        self.generation != self.recovery.generation
    }

    /// Keeps the recovery copy current while editing, so a crash loses at
    /// most [`RECOVERY_INTERVAL_S`] of work: call every frame with the UI
    /// time in seconds. Does nothing unless the world changed since the last
    /// write; then it writes at most once per interval, and never during a
    /// drag (its transaction is open). Back at the saved state the copy is
    /// deleted instead. Returns the path when a file was written.
    pub fn autosave_recovery(&mut self, now: f64) -> Option<PathBuf> {
        // During Play `world` is the play world, which is never written.
        if !self.recovery_pending() || self.edits.is_open() || self.is_playing() {
            return None;
        }
        if !self.is_dirty() {
            // Undone back to the saved state: a copy written meanwhile
            // would offer changes that no longer exist.
            if self.recovery.written {
                self.remove_recovery();
            }
            self.recovery.generation = self.generation;
            return None;
        }
        if self
            .recovery
            .written_at
            .is_some_and(|t| (0.0..RECOVERY_INTERVAL_S).contains(&(now - t)))
        {
            return None;
        }
        self.recovery.written_at = Some(now);
        self.write_recovery()
    }

    /// Writes the world to the recovery file. Whatever the outcome, the
    /// state counts as handled until the next change (no retry per frame).
    fn write_recovery(&mut self) -> Option<PathBuf> {
        self.recovery.generation = self.generation;
        let path = self.recovery_path()?;
        let result = scene::save(&self.world)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| write_atomic(&path, &bytes));
        match result {
            Ok(()) => {
                if !std::mem::replace(&mut self.recovery.written, true) {
                    tracing::info!("unsaved changes are kept in {}", path.display());
                }
                Some(path)
            }
            Err(e) => {
                tracing::error!("could not write the recovery file: {e:#}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_paths_get_the_scene_extension() {
        let n = |s: &str| normalize_scene_path(Path::new(s));
        assert_eq!(n("a/level.scene.json"), PathBuf::from("a/level.scene.json"));
        assert_eq!(n("a/level.json"), PathBuf::from("a/level.scene.json"));
        assert_eq!(n("a/level"), PathBuf::from("a/level.scene.json"));
        assert_eq!(n("level.v2"), PathBuf::from("level.v2.scene.json"));
    }
}
