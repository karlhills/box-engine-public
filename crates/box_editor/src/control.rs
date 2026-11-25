//! The editor as a live control host (ARCHITECTURE.md section 12): with
//! Allow Agent Control on (File menu, or `box-editor --control`), a
//! [`ControlServer`] for the open project answers `box mcp` and other local
//! tools. Requests are answered on the main thread when the runner calls
//! [`Editor::poll_control`] once per frame.
//!
//! - `world_query` reads the world shown: the play world during Play (the
//!   result says which in `"world"`).
//! - `apply_commands` goes through the same `EditStack` as the panes. In Edit
//!   mode an applied list is one undo step (Cmd+Z undoes it) and marks the
//!   scene dirty. During Play it edits the play world on Play's throwaway
//!   stack, exactly like edits made in the inspector during Play: not
//!   undoable, discarded on Stop (`"world": "play"`, `"undoable": false`,
//!   `"discarded_on_stop": true`).
//! - `screenshot` renders the viewport's camera (the game camera during Play)
//!   into an offscreen target at the viewport's size or the requested size,
//!   with the viewport's overlays but no editor chrome.
//! - `play` starts or resumes Play without giving the game keyboard and mouse
//!   focus (the user clicks the viewport for that), so an agent never grabs
//!   the user's cursor. `step_frames` runs fixed 1/60 s frames synchronously
//!   and keeps the pause state.

use box_core::edit::Command;
use box_core::world::World;
use box_engine::control::{
    self, ControlError, ControlHost, ControlServer, HostKind, LogEntry, LogPage, Mode, Screenshot,
    Status, WorldQuery, code,
};
use box_render::{Renderer, ViewCamera};
use serde_json::{Value as Json, json};

use crate::editor::Editor;
use crate::play::{PlayError, PlayState};
use crate::state::EditorState;

/// Error code: Play could not start (autosave, snapshot or a panic while
/// building the game's plugins).
pub const PLAY_FAILED: &str = "play_failed";
/// Error code: game code panicked during `step_frames`; Play was stopped.
pub const GAME_PANIC: &str = "game_panic";

/// The undo label of an agent's command list; an assistant turn merges
/// these into its own step ([`box_core::edit::EditStack::begin_group`]).
pub const AGENT_EDIT: &str = "Agent edit";

/// Size of a screenshot when the viewport has not been laid out yet.
const FALLBACK_SCREENSHOT: (u32, u32) = (1280, 720);

impl EditorState {
    /// The control socket, while Allow Agent Control is on.
    pub fn agent_control(&self) -> Option<&ControlServer> {
        self.control.as_ref()
    }

    /// Turns Allow Agent Control on or off. On starts a [`ControlServer`]
    /// for the open project and writes `<project>/.box/control.json`; it
    /// needs a project (the discovery file lives in it), so without one it
    /// logs why and stays off. Returns whether control is on.
    pub fn set_agent_control(&mut self, on: bool) -> bool {
        if !on {
            if self.control.take().is_some() {
                tracing::info!("agent control off");
            }
            return false;
        }
        if self.control.is_some() {
            return true;
        }
        let Some(project) = &self.project else {
            tracing::warn!(
                "agent control needs a project (its discovery file is <project>/.box/control.json); open or create a project first"
            );
            return false;
        };
        let root = project.root().to_path_buf();
        match ControlServer::start(&root, HostKind::Editor, self.control_scene().as_deref()) {
            Ok(server) => {
                tracing::info!(
                    "agent control on: `box mcp` in {} can query and edit this editor (127.0.0.1:{})",
                    root.display(),
                    server.port()
                );
                self.control = Some(server);
                true
            }
            Err(e) => {
                tracing::error!("could not start agent control: {e}");
                false
            }
        }
    }

    /// The open scene as `box mcp` reports it: relative to the project root,
    /// else the file path, else the scene's name.
    pub(crate) fn control_scene(&self) -> Option<String> {
        Some(match (&self.scene.path, &self.project) {
            (Some(path), Some(project)) => project.relative(path),
            (Some(path), None) => path.display().to_string(),
            (None, _) => self.scene.name.clone(),
        })
    }

    /// Writes the open scene to the discovery file (after a scene switch or
    /// Save As).
    pub(crate) fn sync_control_scene(&mut self) {
        let scene = self.control_scene();
        if let Some(server) = &mut self.control
            && let Err(e) = server.set_scene(scene.as_deref())
        {
            tracing::warn!("could not update the agent control file: {e}");
        }
    }

    fn mode(&self) -> Mode {
        match self.play_state() {
            PlayState::Editing => Mode::Edit,
            PlayState::Playing => Mode::Play,
            PlayState::Paused => Mode::Paused,
        }
    }

    fn world_label(&self) -> &'static str {
        if self.is_playing() { "play" } else { "edit" }
    }

    /// Applies an agent's commands (the control socket's `apply_commands`,
    /// the assistant panel): one undo step labelled [`AGENT_EDIT`] in Edit
    /// mode (merged into the assistant turn's step), the play world's
    /// throwaway stack during Play.
    pub fn apply_agent_commands(
        &mut self,
        commands: &[Command],
        dry_run: bool,
    ) -> Result<Json, ControlError> {
        // Close an open drag or inspector edit so the agent's list is its
        // own undo step.
        self.edits.commit();
        self.inspector_edit = None;
        let playing = self.is_playing();
        let mut result = control::apply_to_world(
            &mut self.world,
            &mut self.edits,
            commands,
            dry_run,
            AGENT_EDIT,
        )?;
        if !dry_run {
            self.changed(true);
            tracing::info!(
                "agent applied {} command(s) to the {} world",
                commands.len(),
                self.world_label()
            );
        }
        result["world"] = json!(self.world_label());
        if playing {
            result["undoable"] = json!(false);
            result["discarded_on_stop"] = json!(true);
        }
        Ok(result)
    }

    /// The `editor_context` answer: open scene, selection (uid, name),
    /// viewport camera (eye, a point it looks at, vertical field of view in
    /// degrees), Play state, the assistant turn and the next undo step.
    pub fn editor_context(&self) -> Json {
        let selection: Vec<Json> = self
            .selection
            .iter()
            .map(|u| {
                let name = self.world.entity_by_uid(*u).and_then(|e| {
                    self.world
                        .get::<box_core::identity::Name>(e)
                        .map(|n| n.0.clone())
                });
                json!({ "uid": u.to_string(), "name": name })
            })
            .collect();
        let view = self.camera.view();
        let eye: glam::Vec3 = view.transform.0.translation.into();
        let forward = view
            .transform
            .0
            .transform_vector3(glam::Vec3::NEG_Z)
            .normalize_or_zero();
        let look_at = eye + forward * self.camera.orbit.distance.max(1.0);
        json!({
            "scene": self.control_scene(),
            "project": self.project.as_ref().map(|p| p.root().display().to_string()),
            "mode": self.mode(),
            "selection": selection,
            "camera": {
                "eye": eye.to_array(),
                "look_at": look_at.to_array(),
                "fov": view.camera.fov_y.to_degrees(),
            },
            "turn": self.edits.group_label(),
            "undo": self.edits.undo_label(),
        })
    }

    /// Undoes what the open assistant turn changed so far (one step), and
    /// keeps the turn open for what comes next. False when there was
    /// nothing to undo or no turn is open.
    pub fn undo_agent_turn_so_far(&mut self) -> bool {
        let Some(label) = self.edits.group_label().map(str::to_string) else {
            return false;
        };
        self.edits.end_group();
        let undone = self.edits.undo_label() == Some(label.as_str()) && {
            self.undo_scene();
            true
        };
        self.edits.begin_group(label, AGENT_EDIT);
        undone
    }

    fn control_status(&self) -> Status {
        Status {
            mode: self.mode(),
            scene: self.control_scene(),
            project: self
                .project
                .as_ref()
                .map(|p| p.root().display().to_string()),
            frame: self.play_session().map_or(0, |s| s.frames()),
            undo: Some(self.edit_undo_len()),
        }
    }
}

impl Editor {
    /// Answers pending control requests (does nothing while Allow Agent
    /// Control is off). Call once per frame on the main thread, before the
    /// UI; `renderer` enables `screenshot` and GPU statistics.
    pub fn poll_control(&mut self, renderer: Option<&mut Renderer>) {
        let Some(mut server) = self.state.control.take() else {
            return;
        };
        server.poll(&mut EditorHost {
            editor: self,
            renderer,
        });
        if self.state.control.is_none() {
            self.state.control = Some(server);
        }
    }
}

/// [`ControlHost`] over the editor for one poll.
struct EditorHost<'a> {
    editor: &'a mut Editor,
    renderer: Option<&'a mut Renderer>,
}

fn play_error(e: &PlayError) -> ControlError {
    let code = match e {
        PlayError::Panic(_) => GAME_PANIC,
        _ => PLAY_FAILED,
    };
    ControlError::new(code, e.to_string())
}

impl ControlHost for EditorHost<'_> {
    fn status(&mut self) -> Status {
        self.editor.state.control_status()
    }

    fn world_query(&mut self, query: &WorldQuery) -> Result<Json, ControlError> {
        let state = &self.editor.state;
        let mut result = control::query_world(&state.world, query)?;
        result["world"] = json!(state.world_label());
        Ok(result)
    }

    fn apply_commands(
        &mut self,
        commands: &[Command],
        dry_run: bool,
    ) -> Result<Json, ControlError> {
        self.editor.state.apply_agent_commands(commands, dry_run)
    }

    fn editor_context(&mut self) -> Result<Json, ControlError> {
        Ok(self.editor.state.editor_context())
    }

    fn undo_turn(&mut self) -> Result<Json, ControlError> {
        let undone = self.editor.state.undo_agent_turn_so_far();
        Ok(json!({ "undone": undone }))
    }

    fn apply_and_save(&mut self, commands: &[Command]) -> Result<Json, ControlError> {
        let mut result = self.apply_commands(commands, false)?;
        let s = &mut self.editor.state;
        if let Some(mode) = s.play.as_mut() {
            // The play world has it now; the parked edit world gets it too
            // and is saved when Play stops.
            mode.apply_to_edit_world(commands, "Agent edit")?;
            mode.save_on_stop = true;
            result["saved"] = json!(false);
            result["saved_on_stop"] = json!(true);
            result["discarded_on_stop"] = json!(false);
            return Ok(result);
        }
        let path = s
            .save_scene()
            .map_err(|e| ControlError::new(code::COMMAND_FAILED, format!("{e:#}")))?;
        result["saved"] = json!(true);
        result["file"] = json!(path.display().to_string());
        Ok(result)
    }

    fn screenshot(
        &mut self,
        size: Option<(u32, u32)>,
        view: Option<&ViewCamera>,
    ) -> Result<Screenshot, ControlError> {
        let Some(renderer) = self.renderer.as_deref_mut() else {
            return Err(ControlError::new(
                code::UNSUPPORTED,
                "this editor has no renderer; screenshots are unavailable",
            ));
        };
        let viewport = self.editor.state.viewport.target_px;
        let (w, h) = size.unwrap_or(if viewport[0] > 0 && viewport[1] > 0 {
            (viewport[0], viewport[1])
        } else {
            FALLBACK_SCREENSHOT
        });
        let target = renderer.create_viewport_target(w, h);
        self.editor.prepare_render();
        let camera = match view {
            Some(view) => Some(*view),
            None => self.editor.state.viewport_camera(),
        };
        renderer.render_to(&mut self.editor.state.world, &target, camera.as_ref());
        let image = renderer
            .read_viewport_target(&target)
            .map_err(|e| ControlError::new(code::RENDER, e.to_string()))?;
        Screenshot::from_rgba(&image)
    }

    fn play(&mut self) -> Result<Status, ControlError> {
        let s = &mut self.editor.state;
        match s.play_state() {
            PlayState::Editing => {
                s.start_play().map_err(|e| play_error(&e))?;
                // Keyboard, mouse and cursor stay with the user until they
                // click into the viewport.
                s.set_game_focus(false);
                tracing::info!("agent pressed Play");
            }
            PlayState::Paused => {
                s.set_play_paused(false);
                tracing::info!("agent resumed Play");
            }
            PlayState::Playing => {}
        }
        Ok(s.control_status())
    }

    fn restart_play(&mut self, scene: Option<&str>, seed: Option<u32>) -> Result<(), ControlError> {
        let s = &mut self.editor.state;
        if let Some(scene) = scene {
            let open = s.control_status().scene;
            let matches = open.as_deref().is_some_and(|o| {
                o == scene
                    || o.rsplit('/')
                        .next()
                        .and_then(|f| f.strip_suffix(".scene.json"))
                        == Some(scene)
            });
            if !matches {
                return Err(ControlError::invalid_params(format!(
                    "the editor has {} open, not `{scene}`: open that scene first",
                    open.as_deref().unwrap_or("an unsaved scene")
                )));
            }
        }
        if s.is_playing() {
            s.stop_play();
        }
        s.start_play().map_err(|e| play_error(&e))?;
        s.set_game_focus(false);
        if let Some(seed) = seed {
            s.world
                .insert_resource(box_engine::box_script::ScriptSeed(seed));
        }
        tracing::info!("agent restarted Play");
        Ok(())
    }

    fn stop(&mut self) -> Result<Status, ControlError> {
        let s = &mut self.editor.state;
        if s.is_playing() {
            s.stop_play();
            tracing::info!("agent pressed Stop");
        }
        Ok(s.control_status())
    }

    fn pause(&mut self) -> Result<Status, ControlError> {
        let s = &mut self.editor.state;
        if !s.is_playing() {
            return Err(ControlError::not_playing("pause"));
        }
        s.set_play_paused(true);
        Ok(s.control_status())
    }

    fn step_frames(&mut self, frames: u32) -> Result<Json, ControlError> {
        let s = &mut self.editor.state;
        if !s.is_playing() {
            return Err(ControlError::not_playing("step_frames"));
        }
        let ran = s.step_play_frames(frames).map_err(|e| play_error(&e))?;
        Ok(json!({
            "frames": ran,
            "frame": s.play_session().map_or(0, |p| p.frames()),
            "mode": s.mode(),
        }))
    }

    fn read_log(&mut self, since: u64, limit: usize) -> Result<LogPage, ControlError> {
        let (records, next) = self.editor.state.console.log.page(since, limit);
        let entries = records
            .into_iter()
            .map(|r| LogEntry {
                seq: r.seq,
                time: r.time,
                level: r.level.as_str().to_ascii_lowercase(),
                target: r.target,
                message: r.message,
            })
            .collect();
        Ok(LogPage { entries, next })
    }

    fn play_world(&mut self, method: &str) -> Result<&mut World, ControlError> {
        // During Play the state's world is the play world.
        if !self.editor.state.is_playing() {
            return Err(ControlError::not_playing(method));
        }
        Ok(&mut self.editor.state.world)
    }

    fn frame_stats(&mut self) -> Result<Json, ControlError> {
        let s = &self.editor.state;
        let fps = s.status.fps;
        let renderer = self.renderer.as_deref().map(|r| {
            let st = r.stats();
            json!({
                "objects": st.objects,
                "visible": st.visible,
                "draws": st.draws,
                "shadow_draws": st.shadow_draws,
                "triangles": st.triangles,
                "upload_bytes": st.upload_bytes,
                "validation_errors": st.validation_errors,
                "cpu_render_ms": st.cpu_render_ms,
                "cached_meshes": r.cached_meshes(),
            })
        });
        Ok(json!({
            "mode": s.mode(),
            "world": s.world_label(),
            "frame": s.play_session().map_or(0, |p| p.frames()),
            "entities": s.world.entity_count(),
            "fps": fps,
            "frame_ms": if fps > 0.0 { 1000.0 / fps } else { 0.0 },
            "renderer": renderer,
        }))
    }
}
