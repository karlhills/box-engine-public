//! Play mode (D5): an isolated play world built only from serialized scene
//! data, driven through the narrow [`PlaySession`] interface.
//!
//! [`PlaySession`] knows nothing about the editor: it loads a scene snapshot
//! into a fresh world with the game's registry, builds the game's app around
//! it and steps it. Every call into game code (plugin `build`, systems,
//! dropping the app) runs under `catch_unwind`, so a panic becomes a
//! [`PlayError::Panic`] instead of taking the editor down.
//!
//! The editor side ([`EditorState::start_play`] and friends) writes the
//! pre-Play autosave, parks the edit world, its undo history and its dirty
//! state, and shows the play world in every pane until Stop. Edits made
//! during Play go to the play world and a throwaway undo stack; Stop drops
//! both and puts the parked edit world back untouched. Values marked "Keep
//! after Stop" in the inspector ([`EditorState::keep_after_stop`]) are read
//! from the play world on Stop and applied to the edit world afterwards, as
//! one undo step ("Apply Play changes").
//!
//! Assets: the play world gets the edit world's loaded assets (same handles
//! and versions, so nothing is re-imported or re-uploaded) and an
//! `AssetMirror` of its server for new loads. The server keeps serving the
//! parked edit world; every asset event there (hot reload, finished load,
//! rename) is copied into the play world before its next frame, so file
//! edits show while playing. Prefab contents are runtime entities: the
//! snapshot holds only the `PrefabInstance` roots and the play world
//! instantiates them again (same derived uids).
//!
//! Audio: only the play world runs the audio system (the edit world never
//! plays). Pause and step pause its buses (`Audio::set_paused`), resuming
//! where they were; Stop drops the play world and with it the output.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Once;

use box_core::app::{App, FixedTime};
use box_core::edit::EditStack;
use box_core::identity::Uid;
use box_core::input::{CursorGrab, CursorOptions, Input};
use box_core::prelude::World;
use box_core::scene::{self, SceneError};
use box_core::transform::GlobalTransform;
use box_engine::asset_glue::{AssetForwarder, copy_loaded_assets};
use box_engine::project::SCENE_EXTENSION;
use box_engine::scenes::Scenes;
use box_engine::{Game, WindowSize};
use box_render::{Camera, ViewCamera};

use crate::document::write_atomic;
use crate::state::{EditorState, History};

/// Where a game panic happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayPhase {
    /// Building the play app (plugin `build`) from the snapshot.
    Start,
    /// Running the given frame (1-based).
    Frame(u64),
    /// Dropping the play world and the game's systems.
    Stop,
}

/// A panic caught at the Play boundary.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.describe())]
pub struct GamePanic {
    pub phase: PlayPhase,
    /// The panic message.
    pub message: String,
    /// `file:line:column` of the panic, when the panic hook saw it.
    pub location: Option<String>,
    /// The system that panicked (`box_core::app::current_system`, e.g.
    /// `collector::move_players`), for a panic inside a frame's system.
    pub system: Option<String>,
    /// Names of the game's plugins, for context.
    pub plugins: Vec<String>,
}

impl GamePanic {
    fn describe(&self) -> String {
        let phase = match self.phase {
            PlayPhase::Start => "while building the game's plugins".to_owned(),
            PlayPhase::Frame(n) => format!("in frame {n}"),
            PlayPhase::Stop => "while dropping the play world".to_owned(),
        };
        let mut s = format!("game code panicked {phase}");
        if let Some(system) = &self.system {
            s.push_str(&format!(" in system `{system}`"));
        }
        if let Some(l) = &self.location {
            s.push_str(&format!(" at {l}"));
        }
        s.push_str(&format!(": {}", self.message));
        if !self.plugins.is_empty() {
            s.push_str(&format!(" (plugins: {})", self.plugins.join(", ")));
        }
        s
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlayError {
    #[error("could not snapshot the edit scene: {0}")]
    Snapshot(#[source] SceneError),
    #[error("could not load the scene snapshot: {0}")]
    Load(#[source] SceneError),
    #[error("could not write the pre-Play autosave, so Play did not start: {0}")]
    Autosave(String),
    #[error(transparent)]
    Panic(#[from] GamePanic),
}

// ----- panic capture -----

thread_local! {
    /// True while this thread runs game code under [`guarded`].
    static GUARDED: Cell<bool> = const { Cell::new(false) };
    /// Message, location and running system of the last panic caught under
    /// [`guarded`].
    static CAUGHT: RefCell<Option<Caught>> = const { RefCell::new(None) };
}

/// What the panic hook records: message, location and the system running
/// when the panic happened.
type Caught = (String, Option<String>, Option<&'static str>);

/// Wraps the process panic hook once: panics on a thread running guarded game
/// code are recorded (and reported once, by the editor) instead of printed;
/// every other panic goes to the previous hook unchanged.
fn install_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if GUARDED.with(Cell::get) {
                let message = box_core::panic_guard::payload_message(info.payload());
                let location = info.location().map(|l| l.to_string());
                let system = box_core::app::current_system();
                CAUGHT.with(|c| *c.borrow_mut() = Some((message, location, system)));
            } else {
                previous(info);
            }
        }));
    });
}

/// Runs game code, turning a panic into a [`GamePanic`].
fn guarded<T>(phase: PlayPhase, plugins: &[String], f: impl FnOnce() -> T) -> Result<T, GamePanic> {
    install_hook();
    let was = GUARDED.with(|g| g.replace(true));
    // Marked as caught, so the crash reporter leaves it to this hook.
    let result = box_core::panic_guard::catch(f);
    GUARDED.with(|g| g.set(was));
    result.map_err(|payload| {
        let (message, location, system) =
            CAUGHT.with(|c| c.borrow_mut().take()).unwrap_or_else(|| {
                (
                    box_core::panic_guard::payload_message(payload.as_ref()),
                    None,
                    None,
                )
            });
        // Only frames run systems; outside one the name is stale (it stays
        // set after a panic until the next system starts).
        let system = system
            .filter(|_| matches!(phase, PlayPhase::Frame(_)))
            .map(str::to_owned);
        GamePanic {
            phase,
            message,
            location,
            system,
            plugins: plugins.to_vec(),
        }
    })
}

// ----- the session -----

/// A running game built from a scene snapshot: the narrow interface between
/// the editor and the play world (D5). It is created only from serialized
/// scene data, so it could later run in another process with the same model.
pub struct PlaySession {
    app: App,
    paused: bool,
    step_pending: bool,
    frames: u64,
    plugins: Vec<String>,
}

impl PlaySession {
    /// Snapshots `world` (serializes it as a scene) and starts a session from
    /// the snapshot. `world` is only read.
    pub fn from_world(world: &World, game: &Game) -> Result<Self, PlayError> {
        Self::from_world_with_scenes(world, game, None)
    }

    /// [`from_world`](Self::from_world) with a [`Scenes`] resource (project
    /// root, start scene) inserted before the game's plugins are built, so
    /// the game's `Scenes::switch` and `load_additive` resolve against the
    /// project.
    pub fn from_world_with_scenes(
        world: &World,
        game: &Game,
        scenes: Option<Scenes>,
    ) -> Result<Self, PlayError> {
        let bytes = scene::save(world).map_err(PlayError::Snapshot)?;
        Self::start(&bytes, game, scenes, Some(world))
    }

    /// Loads a scene into a fresh world with the game's registry and builds
    /// the game's app around it (every plugin's `build`). Transforms are
    /// propagated and meshes built so the first frame can be drawn before the
    /// first update.
    pub fn from_scene(scene_bytes: &[u8], game: &Game) -> Result<Self, PlayError> {
        Self::from_scene_with_scenes(scene_bytes, game, None)
    }

    /// [`from_scene`](Self::from_scene) with a [`Scenes`] resource; see
    /// [`from_world_with_scenes`](Self::from_world_with_scenes).
    pub fn from_scene_with_scenes(
        scene_bytes: &[u8],
        game: &Game,
        scenes: Option<Scenes>,
    ) -> Result<Self, PlayError> {
        Self::start(scene_bytes, game, scenes, None)
    }

    /// Builds the session. With `assets` (the edit world), the play world
    /// gets its loaded assets and follows its asset server
    /// ([`box_engine::asset_glue::copy_loaded_assets`]): nothing is
    /// re-imported, and prefab contents are instantiated from the copied
    /// scene assets before the first frame.
    fn start(
        scene_bytes: &[u8],
        game: &Game,
        scenes: Option<Scenes>,
        assets: Option<&World>,
    ) -> Result<Self, PlayError> {
        let plugins: Vec<String> = game.plugins().iter().map(|p| p.name().to_owned()).collect();
        let app = guarded(PlayPhase::Start, &plugins, || {
            let mut world = scene::load(scene_bytes, game.registry())?;
            if let Some(scenes) = scenes {
                world.insert_resource(scenes);
            }
            if let Some(edit) = assets {
                copy_loaded_assets(edit, &mut world);
            }
            let mut app = game.app(world);
            // Prefab contents first, so they are propagated and meshed too.
            box_engine::sync_assets(&mut app.world);
            box_core::transform::propagate_transforms(&mut app.world);
            box_engine::sync_scene_meshes(&mut app.world);
            Ok(app)
        })?
        .map_err(PlayError::Load)?;
        Ok(Self {
            app,
            paused: false,
            step_pending: false,
            frames: 0,
            plugins,
        })
    }

    /// The play world.
    pub fn world(&self) -> &World {
        &self.app.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.app.world
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Paused sessions keep their world (for rendering and inspection) but
    /// do not step: time and fixed steps stand still.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.step_pending = false;
    }

    /// Pauses (if running) and runs exactly one frame on the next
    /// [`update`](Self::update), with a dt of one fixed step.
    pub fn step(&mut self) {
        self.paused = true;
        self.step_pending = true;
    }

    /// Frames run so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The game set `AppExit(true)`.
    pub fn exit_requested(&self) -> bool {
        self.app.exit_requested()
    }

    /// Runs one frame of `dt` seconds (fixed steps inside) unless paused; a
    /// pending [`step`](Self::step) runs one frame of one fixed step even
    /// while paused. Per-frame input is cleared afterwards. Returns whether a
    /// frame ran. After an error the session must be dropped.
    pub fn update(&mut self, dt: f32) -> Result<bool, PlayError> {
        let dt = if !self.paused {
            dt
        } else if std::mem::take(&mut self.step_pending) {
            self.app
                .world
                .get_resource::<FixedTime>()
                .map_or(1.0 / 60.0, |f| f.step)
        } else {
            return Ok(false);
        };
        let frame = self.frames + 1;
        let app = &mut self.app;
        guarded(PlayPhase::Frame(frame), &self.plugins, || {
            app.tick(dt);
            if let Some(input) = app.world.get_resource_mut::<Input>() {
                input.end_frame();
            }
        })?;
        self.frames = frame;
        Ok(true)
    }

    /// Drops the play world and the game's systems, catching a panic in a
    /// game type's `Drop`.
    pub fn shutdown(self) -> Result<(), PlayError> {
        let plugins = self.plugins.clone();
        guarded(PlayPhase::Stop, &plugins, move || drop(self))?;
        Ok(())
    }
}

// ----- the editor side -----

/// The editor's view of Play mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Editing,
    Playing,
    Paused,
}

/// What the editor sets aside while playing, restored unchanged on Stop.
struct Parked {
    world: World,
    edits: EditStack,
    generation: u64,
    history: History,
}

pub(crate) struct PlayMode {
    /// Between frames its world is a placeholder: the play world lives in
    /// `EditorState::world` so every pane shows it, and is lent back to the
    /// session for each update.
    session: PlaySession,
    parked: Parked,
    /// The file holding the pre-Play scene, when one was written.
    autosave: Option<PathBuf>,
    /// Keyboard and pointer input in the viewport go to the game.
    pub(crate) game_focus: bool,
    /// Look at the play world through the editor camera.
    pub(crate) free_camera: bool,
    /// Brings the parked edit world's asset changes (hot reloads, loads the
    /// game requested) into the play world.
    assets: AssetForwarder,
    /// Values to apply to the edit scene on Stop ("Keep after Stop").
    kept: Vec<Kept>,
    /// Save the scene at Stop (an agent applied edits with `save`).
    pub(crate) save_on_stop: bool,
}

/// A play-world value marked "Keep after Stop": the field (or whole
/// component) `path` of the entity `uid`, read when Play stops.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub uid: Uid,
    pub path: String,
}

impl PlayMode {
    /// Whether the parked edit world differs from the scene file. Play
    /// starts with no open transaction, so only the undo position counts.
    pub(crate) fn edit_dirty(&self) -> bool {
        !self.parked.history.is_saved(self.parked.edits.undo_len())
    }

    /// Undo steps of the parked edit world.
    pub(crate) fn edit_undo_len(&self) -> usize {
        self.parked.edits.undo_len()
    }

    /// Apply edit commands to the parked edit world (undoable there after
    /// Stop), for agent edits that must outlive the Play session.
    pub(crate) fn apply_to_edit_world(
        &mut self,
        commands: &[box_core::edit::Command],
        label: &str,
    ) -> Result<serde_json::Value, box_engine::control::ControlError> {
        self.parked.edits.commit();
        box_engine::control::apply_to_world(
            &mut self.parked.world,
            &mut self.parked.edits,
            commands,
            false,
            label,
        )
    }
}

/// `path` is `outer` or a field inside it (`Transform.translation` is
/// within `Transform`; `Transform` is within itself).
fn is_within(path: &str, outer: &str) -> bool {
    path == outer
        || path
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

/// Where scenes without a project keep their pre-Play autosave: the user's
/// application data folder (`~/Library/Application Support/box-editor/autosave`
/// on macOS, `%LOCALAPPDATA%\box-editor\autosave` on Windows,
/// `$XDG_DATA_HOME/box-editor/autosave` or `~/.local/share/...` elsewhere).
pub fn default_autosave_dir() -> PathBuf {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("LOCALAPPDATA")
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share")))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("box-editor")
        .join("autosave")
}

impl EditorState {
    pub fn play_state(&self) -> PlayState {
        match &self.play {
            None => PlayState::Editing,
            Some(m) if m.session.is_paused() => PlayState::Paused,
            Some(_) => PlayState::Playing,
        }
    }

    /// Playing or paused: `world` is the play world.
    pub fn is_playing(&self) -> bool {
        self.play.is_some()
    }

    /// The edit world: `world`, or during Play the parked one.
    pub fn edit_world(&self) -> &World {
        match &self.play {
            Some(m) => &m.parked.world,
            None => &self.world,
        }
    }

    /// The edit world, mutably (the parked one during Play). Only the asset
    /// server's update and other non-scene runtime data may change it while
    /// parked; scene edits go through the edit stack.
    pub(crate) fn edit_world_mut(&mut self) -> &mut World {
        match &mut self.play {
            Some(m) => &mut m.parked.world,
            None => &mut self.world,
        }
    }

    /// The running session (its own world is lent out between frames; read
    /// the play world through [`EditorState::world`]).
    pub fn play_session(&self) -> Option<&PlaySession> {
        self.play.as_ref().map(|m| &m.session)
    }

    /// The file holding the scene as it was when Play started, if one was
    /// written (only for unsaved changes; a clean scene's file is the copy).
    pub fn play_autosave(&self) -> Option<&Path> {
        self.play.as_ref().and_then(|m| m.autosave.as_deref())
    }

    /// Where Play writes the pre-Play scene: the project recovery file, or
    /// for a scene without a project `<autosave_dir>/<scene>.scene.json`.
    pub fn play_autosave_path(&self) -> PathBuf {
        self.recovery_path().unwrap_or_else(|| {
            self.autosave_dir
                .join(format!("{}{SCENE_EXTENSION}", self.scene.name))
        })
    }

    /// Writes the pre-Play autosave when the scene has unsaved changes (a
    /// clean scene is already safe: in its file, or the built-in scratch
    /// scene). Returns the path written.
    fn write_play_autosave(&mut self) -> Result<Option<PathBuf>, PlayError> {
        if !self.is_dirty() {
            return Ok(None);
        }
        let path = self.play_autosave_path();
        let bytes = scene::save(&self.world).map_err(PlayError::Snapshot)?;
        write_atomic(&path, &bytes).map_err(|e| PlayError::Autosave(format!("{e:#}")))?;
        Ok(Some(path))
    }

    /// Play: autosaves the scene, snapshots the edit world into a new play
    /// world with the game's plugins, and shows it. Nothing changes if any
    /// step fails (the error is logged and returned). Already playing: Ok.
    pub fn start_play(&mut self) -> Result<(), PlayError> {
        if self.play.is_some() {
            return Ok(());
        }
        // End an open drag as one step so the snapshot matches the undo
        // history; a viewport tool's operation (a drawing and its preview)
        // ends too.
        self.finish_edit();
        self.cancel_tool();
        // Play reads settings files: write pending settings edits first.
        self.flush_settings_edit(f64::INFINITY, true);
        let scenes = self.runtime_scenes();
        let result = self.write_play_autosave().and_then(|autosave| {
            PlaySession::from_world_with_scenes(&self.world, &self.game, Some(scenes))
                .map(|s| (s, autosave))
        });
        let (mut session, autosave) = match result {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Play failed: {e}");
                return Err(e);
            }
        };
        let play_world = std::mem::replace(session.world_mut(), World::new());
        let assets = AssetForwarder::new(&self.world);
        let parked = Parked {
            world: std::mem::replace(&mut self.world, play_world),
            edits: std::mem::replace(&mut self.edits, EditStack::new()),
            generation: self.generation,
            history: std::mem::take(&mut self.history),
        };
        self.reset_world_views();
        // Rules switched off in the rule list stay off in Play.
        self.apply_rule_switches();
        if let Some(p) = &autosave {
            tracing::info!("pre-Play autosave: {}", p.display());
        }
        tracing::info!("Play ({} entities)", self.world.entity_count());
        self.play = Some(PlayMode {
            session,
            parked,
            autosave,
            game_focus: true,
            free_camera: false,
            assets,
            kept: Vec::new(),
            save_on_stop: false,
        });
        Ok(())
    }

    /// "Keep after Stop" during Play: when Play stops, the current value of
    /// the field (or component) `path` of each entity in `uids` is applied
    /// to the edit scene, all of them as one undo step ("Apply Play
    /// changes"). Entities that are not in the edit scene (spawned by the
    /// game) or are prefab contents (runtime data) are refused with a
    /// warning. A path inside one already kept, or containing kept paths,
    /// replaces them. Returns how many entities were marked.
    pub fn keep_after_stop(&mut self, uids: &[Uid], path: &str) -> usize {
        if !self.is_playing() {
            return 0;
        }
        let mut accepted = Vec::new();
        for &uid in uids {
            let name = self
                .world
                .entity_by_uid(uid)
                .map(|e| self.display_name(e))
                .unwrap_or_else(|| uid.to_string());
            let edit = self.edit_world();
            match edit.entity_by_uid(uid) {
                None => {
                    tracing::warn!("{name} was created during Play, so its changes cannot be kept")
                }
                Some(e) if box_core::prefab::is_generated(edit, e) => tracing::warn!(
                    "{name} is generated (part of a prefab instance or derived by a rule), so its changes cannot be kept"
                ),
                Some(_) => accepted.push(uid),
            }
        }
        let Some(mode) = &mut self.play else {
            return 0;
        };
        for &uid in &accepted {
            let covered = mode
                .kept
                .iter()
                .any(|k| k.uid == uid && is_within(path, &k.path));
            if covered {
                continue;
            }
            mode.kept
                .retain(|k| !(k.uid == uid && is_within(&k.path, path)));
            mode.kept.push(Kept {
                uid,
                path: path.to_owned(),
            });
        }
        accepted.len()
    }

    /// Takes "Keep after Stop" back for `path` of `uids`.
    pub fn forget_kept(&mut self, uids: &[Uid], path: &str) {
        if let Some(mode) = &mut self.play {
            mode.kept
                .retain(|k| !(uids.contains(&k.uid) && k.path == path));
        }
    }

    /// Whether `path` (or a component containing it) of `uid` will be kept.
    pub fn is_kept(&self, uid: Uid, path: &str) -> bool {
        self.play.as_ref().is_some_and(|m| {
            m.kept
                .iter()
                .any(|k| k.uid == uid && is_within(path, &k.path))
        })
    }

    /// The values marked "Keep after Stop" (shown in the Play banner).
    pub fn kept_changes(&self) -> &[Kept] {
        self.play.as_ref().map_or(&[], |m| m.kept.as_slice())
    }

    /// Applies kept play values (read before the play world was dropped) to
    /// the edit world as one undo step.
    fn apply_kept(&mut self, values: Vec<(Kept, box_core::reflect::Value)>) {
        if values.is_empty() {
            return;
        }
        self.begin_edit("Apply Play changes");
        let mut applied = 0;
        for (k, v) in &values {
            if self.world.entity_by_uid(k.uid).is_none() {
                tracing::warn!(
                    "{}: not in the edit scene; its Play change is dropped",
                    k.uid
                );
                continue;
            }
            if self.set_field(k.uid, &k.path, v.as_ref()) {
                applied += 1;
            }
        }
        self.finish_edit();
        self.changed(true);
        tracing::info!("applied {applied} Play change(s) to the edit scene");
        self.notify(&format!(
            "Kept {applied} Play change{}",
            if applied == 1 { "" } else { "s" }
        ));
    }

    /// The play world's [`Scenes`]: scene names and relative paths resolve
    /// in the open project, and the snapshot counts as the scene file it was
    /// loaded from.
    fn runtime_scenes(&self) -> Scenes {
        let mut scenes = Scenes::new();
        if let Some(project) = &self.project {
            scenes = scenes.with_project_root(project.root());
        }
        if let Some(path) = &self.scene.path {
            scenes = scenes.with_start_scene(path.clone());
        }
        scenes
    }

    /// Stop: drops the play world and restores the edit world, undo history
    /// and dirty state exactly as they were before Play. Changes made during
    /// Play are discarded, except values marked "Keep after Stop"
    /// ([`Self::keep_after_stop`]), applied afterwards as one undo step.
    /// Does nothing when not playing.
    pub fn stop_play(&mut self) {
        let Some(mode) = self.play.take() else {
            return;
        };
        let frames = mode.session.frames();
        let autosave = mode.autosave.clone();
        let save = mode.save_on_stop;
        // Read the kept values while the play world is still here.
        let kept: Vec<(Kept, box_core::reflect::Value)> = mode
            .kept
            .iter()
            .filter_map(|k| {
                let value = self
                    .world
                    .field_path(&k.path)
                    .ok()
                    .zip(self.world.entity_by_uid(k.uid))
                    .and_then(|(p, e)| self.world.get_field(e, &p).ok());
                if value.is_none() {
                    tracing::warn!(
                        "{}: gone from the play world; its change is dropped",
                        k.path
                    );
                }
                value.map(|v| (k.clone(), v))
            })
            .collect();
        if let Err(e) = self.end_play(mode) {
            tracing::error!("{e}");
        }
        let unchanged = kept.is_empty();
        self.apply_kept(kept);
        // The project recovery file is the scene's regular unsaved-changes
        // copy and stays; a scene without a project needs no autosave once
        // its edit world is back.
        if let Some(p) = autosave.filter(|p| Some(p) != self.recovery_path().as_ref()) {
            let _ = std::fs::remove_file(p);
        }
        if unchanged && !save {
            tracing::info!("Stop after {frames} frames; the edit scene is unchanged");
        } else {
            tracing::info!("Stop after {frames} frames");
        }
        if save && let Err(e) = self.save_scene() {
            tracing::error!("saving the agent's edits at Stop failed: {e:#}");
        }
    }

    /// Restores the parked state and shuts the session down.
    fn end_play(&mut self, mode: PlayMode) -> Result<(), PlayError> {
        let PlayMode {
            mut session,
            parked,
            ..
        } = mode;
        let play_world = std::mem::replace(&mut self.world, parked.world);
        self.edits = parked.edits;
        self.generation = parked.generation;
        self.history = parked.history;
        self.open_dirty = false;
        self.reset_world_views();
        // The game's world goes back into its session so a panicking `Drop`
        // in game data is caught with the rest.
        *session.world_mut() = play_world;
        session.shutdown()
    }

    /// After a game panic: stop Play and report where the pre-Play scene is.
    fn abort_play(&mut self, error: &PlayError) {
        let Some(mode) = self.play.take() else {
            return;
        };
        let kept = match &mode.autosave {
            Some(p) => format!("; the pre-Play autosave is {}", p.display()),
            None => String::new(),
        };
        if let Err(e) = self.end_play(mode) {
            tracing::error!("{e}");
        }
        tracing::error!("Play stopped: {error}. The edit scene is unchanged{kept}");
    }

    /// Caches tied to the world shown (hierarchy rows, gizmo drag, open
    /// inspector edit) are reset when the edit and play worlds swap. The
    /// selection is by uid, so it carries over where the entity exists.
    fn reset_world_views(&mut self) {
        self.hierarchy.dirty = true;
        self.hierarchy.renaming = None;
        self.hierarchy.dragging = None;
        self.gizmo = Default::default();
        self.inspector_edit = None;
        let world = &self.world;
        self.selection.retain(|u| world.entity_by_uid(*u).is_some());
    }

    /// Play, or Stop while playing (toolbar and Cmd+P). A failed start is
    /// logged.
    pub fn toggle_play(&mut self) {
        if self.is_playing() {
            self.stop_play();
        } else {
            let _ = self.start_play();
        }
    }

    /// Pause or resume while playing (Cmd+Shift+P); resuming gives input
    /// back to the game.
    pub fn toggle_pause(&mut self) {
        match self.play_state() {
            PlayState::Editing => {}
            PlayState::Playing => self.set_play_paused(true),
            PlayState::Paused => {
                self.set_play_paused(false);
                self.set_game_focus(true);
            }
        }
    }

    /// Pause or resume. Pausing releases game input.
    pub fn set_play_paused(&mut self, paused: bool) {
        if let Some(m) = &mut self.play {
            m.session.set_paused(paused);
        }
        self.sync_play_audio();
        if paused {
            self.set_game_focus(false);
        }
    }

    /// The play world's audio follows the session's pause: a paused
    /// session's systems do not run, so the audio system cannot notice by
    /// itself. Sounds fade to a pause and resume where they were.
    fn sync_play_audio(&mut self) {
        let Some(m) = &self.play else {
            return;
        };
        let paused = m.session.is_paused();
        if let Some(audio) = self
            .world
            .get_resource_mut::<box_engine::box_audio::Audio>()
        {
            audio.set_paused(paused);
        }
    }

    /// Pauses and advances the play world by exactly one frame (one fixed
    /// step) on the next update.
    pub fn step_play(&mut self) {
        if let Some(m) = &mut self.play {
            m.session.step();
        }
        self.sync_play_audio();
        self.set_game_focus(false);
    }

    /// Advances the play world by exactly `frames` frames of one fixed step
    /// each, synchronously, keeping the pause state (the control socket's
    /// `step_frames`). Stops early when Play ends (the game exits, or panics:
    /// the error is returned). Returns the frames run.
    pub fn step_play_frames(&mut self, frames: u32) -> Result<u32, PlayError> {
        let paused = self.play_state() == PlayState::Paused;
        let mut ran = 0;
        for _ in 0..frames {
            let Some(m) = &mut self.play else {
                break;
            };
            m.session.step();
            if self.update_play(0.0)? {
                ran += 1;
            }
        }
        if let Some(m) = &mut self.play {
            m.session.set_paused(paused);
        }
        self.sync_play_audio();
        Ok(ran)
    }

    /// Runs one play frame of `dt` seconds (nothing while paused, one fixed
    /// step after [`step_play`](Self::step_play)). A panic in game code stops
    /// Play: the edit world comes back unchanged and the error is logged and
    /// returned. A game that sets `AppExit` stops Play too. Returns whether a
    /// frame ran.
    pub fn update_play(&mut self, dt: f32) -> Result<bool, PlayError> {
        let Some(mode) = &mut self.play else {
            return Ok(false);
        };
        if let Some(rect) = self.viewport.rect.filter(|r| r.width() > 0.0)
            && self.viewport.target_px[0] > 0
        {
            let ppp = self.viewport.request_px[0] as f32 / rect.width();
            *self.world.resource_or_default::<WindowSize>() = WindowSize {
                width: self.viewport.target_px[0],
                height: self.viewport.target_px[1],
                scale_factor_milli: (ppp * 1000.0).round() as u32,
            };
        }
        // The edit world's server keeps running on the parked world
        // (`update_assets`); its changes reach the play world here.
        mode.assets.forward(&mode.parked.world, &mut self.world);
        std::mem::swap(&mut self.world, mode.session.world_mut());
        let result = mode.session.update(dt);
        std::mem::swap(&mut self.world, mode.session.world_mut());
        match result {
            Ok(ran) => {
                if ran {
                    // Systems may spawn, despawn and rename: rebuild the
                    // hierarchy rows (one flatten per played frame).
                    self.hierarchy.dirty = true;
                    let world = &self.world;
                    self.selection.retain(|u| world.entity_by_uid(*u).is_some());
                }
                if mode.session.exit_requested() {
                    tracing::info!("the game asked to exit");
                    self.stop_play();
                }
                Ok(ran)
            }
            Err(e) => {
                self.abort_play(&e);
                Err(e)
            }
        }
    }

    pub fn game_focus(&self) -> bool {
        self.play.as_ref().is_some_and(|m| m.game_focus)
    }

    /// The game has input and the cursor is grabbed for it (the play world's
    /// `CursorOptions::active_grab`, written by the windowed runner): mouse
    /// motion then comes from raw device motion instead of the pointer.
    pub fn game_cursor_grabbed(&self) -> bool {
        self.game_focus()
            && self
                .world
                .get_resource::<CursorOptions>()
                .is_some_and(|c| c.active_grab != CursorGrab::None)
    }

    /// Gives keyboard and pointer input in the viewport to the game (only
    /// while running and not in free-camera view), or takes it back,
    /// releasing every key and button the game saw as held.
    pub fn set_game_focus(&mut self, focus: bool) {
        let Some(m) = &mut self.play else {
            return;
        };
        let focus = focus && !m.session.is_paused() && !m.free_camera;
        if m.game_focus
            && !focus
            && let Some(input) = self.world.get_resource_mut::<Input>()
        {
            input.release_all();
            input.modifiers = Default::default();
            input.cursor_position = None;
        }
        m.game_focus = focus;
    }

    pub fn free_camera(&self) -> bool {
        self.play.as_ref().is_some_and(|m| m.free_camera)
    }

    /// View the play world through the editor camera (with the editor
    /// overlays) instead of the game's camera. Releases game input.
    pub fn set_free_camera(&mut self, on: bool) {
        if let Some(m) = &mut self.play {
            m.free_camera = on;
        }
        if on {
            self.set_game_focus(false);
        }
    }

    /// The play world's first active camera, if the viewport shows it (Play,
    /// not in free-camera view).
    fn game_camera(&self) -> Option<(Camera, GlobalTransform)> {
        if !self.is_playing() || self.free_camera() {
            return None;
        }
        self.world
            .query::<(&Camera, &GlobalTransform)>()
            .iter()
            .find(|(c, _)| c.active)
            .map(|(c, g)| (*c, *g))
    }

    /// The viewport shows the game's own camera (not the editor camera).
    pub fn shows_game_camera(&self) -> bool {
        self.game_camera().is_some()
    }

    /// The camera the viewport renders with: `None` means the world's active
    /// camera (the game view during Play), otherwise the editor camera.
    pub fn viewport_camera(&self) -> Option<ViewCamera> {
        match self.game_camera() {
            Some(_) => None,
            None => Some(self.camera.view()),
        }
    }
}
