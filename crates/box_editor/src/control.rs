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
use box_engine::control::{
    self, ControlError, ControlHost, ControlServer, HostKind, LogEntry, LogPage, Mode, Screenshot,
    Status, WorldQuery, code,
};
use box_render::Renderer;
use serde_json::{Value as Json, json};

use crate::editor::Editor;
use crate::play::{PlayError, PlayState};
use crate::state::EditorState;

/// Error code: Play could not start (autosave, snapshot or a panic while
/// building the game's plugins).
pub const PLAY_FAILED: &str = "play_failed";
/// Error code: game code panicked during `step_frames`; Play was stopped.
pub const GAME_PANIC: &str = "game_panic";

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
        let s = &mut self.editor.state;
        // Close an open drag or inspector edit so the agent's list is its
        // own undo step.
        s.edits.commit();
        s.inspector_edit = None;
        let playing = s.is_playing();
        let mut result =
            control::apply_to_world(&mut s.world, &mut s.edits, commands, dry_run, "Agent edit")?;
        if !dry_run {
            s.changed(true);
            tracing::info!(
                "agent applied {} command(s) to the {} world",
                commands.len(),
                s.world_label()
            );
        }
        result["world"] = json!(s.world_label());
        if playing {
            result["undoable"] = json!(false);
            result["discarded_on_stop"] = json!(true);
        }
        Ok(result)
    }

    fn screenshot(&mut self, size: Option<(u32, u32)>) -> Result<Screenshot, ControlError> {
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
        let camera = self.editor.state.viewport_camera();
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
