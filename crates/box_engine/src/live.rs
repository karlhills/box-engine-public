//! Live control of a runner-based app (the `--control` runner flag): the
//! player side of ARCHITECTURE.md section 12.
//!
//! [`LiveControl`] owns a [`ControlServer`] (kind `player`) and the state
//! the runner needs (paused, frame count, last frame's statistics). The
//! runner calls [`LiveControl::poll`] once per frame on the main thread,
//! before the frame's tick, and [`LiveControl::frame_done`] after rendering.
//! Requests are answered by [`PlayerHost`], a short-lived view of the app,
//! the renderer and that state.
//!
//! Semantics (see also `docs/ARCHITECTURE.md` section 12):
//! - `status`: mode `play` or `paused`, the main scene, frames simulated.
//! - `apply_commands`: edits the running world through the command layer;
//!   nothing is saved to the scene file and there is no undo.
//! - `pause` stops ticking (rendering and polling continue); `play` resumes;
//!   `step_frames(n)` ticks `n` fixed 1/60 s frames before answering.
//! - `stop` ends the player like [`AppExit`]: the run returns normally
//!   (exit code 0) and the discovery file is removed.
//! - `screenshot` renders the world into an offscreen target at the
//!   requested size or the window (headless: `--size`) size.
//! - `read_log` reads [`crate::log_capture`]; `frame_stats` reports the last
//!   rendered frame.
//!
//! A windowed player whose window is occluded does not tick (as without
//! `--control`), but keeps answering requests from a 60 Hz timer.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use box_core::app::{App, AppExit};
use box_core::edit::{Command, EditStack};
use box_core::input::Input;
use box_core::transform::propagate_transforms;
use box_core::world::World;
use box_render::{FrameStats, Renderer, ViewportTarget};
use serde_json::{Value as Json, json};

use crate::control::{
    ControlError, ControlHost, ControlServer, FIXED_DT, HostKind, LogPage, Mode, Screenshot,
    Status, WorldQuery, apply_to_world, code, query_world,
};
use crate::log_capture;
use crate::scenes::Scenes;

/// The control socket of a running app plus the state its requests change.
pub(crate) struct LiveControl {
    server: ControlServer,
    state: LiveState,
}

struct LiveState {
    project: PathBuf,
    /// The main scene relative to the project root, as last announced.
    scene: Option<String>,
    paused: bool,
    /// Frames simulated (ticked), including `step_frames`.
    frame: u64,
    last: Option<LastFrame>,
    /// Reused by screenshots of the same size.
    target: Option<ViewportTarget>,
}

/// Statistics of the last rendered frame (screenshots do not change them).
struct LastFrame {
    stats: FrameStats,
    frame_ms: f32,
    update_ms: f32,
    render_ms: f32,
}

/// The main scene (or the start scene before the first frame), relative to
/// the project root with `/` separators.
fn scene_of(world: &World, root: &Path) -> Option<String> {
    let scenes = world.get_resource::<Scenes>()?;
    let path = scenes
        .main()
        .and_then(|id| scenes.path(id))
        .or_else(|| scenes.start_scene())?;
    Some(match path.strip_prefix(root) {
        Ok(rel) => rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    })
}

impl LiveControl {
    /// Start the socket for the project of the world's [`Scenes`] resource
    /// and enable log capture. Fails when the world has no project root
    /// (the player sets one; a bare `App` does not).
    pub(crate) fn start(world: &World) -> Result<Self> {
        let project = world
            .get_resource::<Scenes>()
            .and_then(Scenes::project_root)
            .map(Path::to_path_buf)
            .context(
                "--control needs a project: run the game through the player (box-player, `box run`, or a native project's player), which sets the Scenes resource's project root",
            )?;
        log_capture::enable();
        let scene = scene_of(world, &project);
        let server = ControlServer::start(&project, HostKind::Player, scene.as_deref())
            .with_context(|| {
                format!(
                    "could not start the control socket for {}",
                    project.display()
                )
            })?;
        Ok(Self {
            server,
            state: LiveState {
                project,
                scene,
                paused: false,
                frame: 0,
                last: None,
                target: None,
            },
        })
    }

    /// Answer pending requests. Call once per frame before the tick.
    pub(crate) fn poll(&mut self, app: &mut App, renderer: Option<&mut Renderer>) {
        self.server.poll(&mut PlayerHost {
            app,
            renderer,
            state: &mut self.state,
        });
        let scene = scene_of(&app.world, &self.state.project);
        if scene != self.state.scene {
            if let Err(e) = self.server.set_scene(scene.as_deref()) {
                tracing::warn!(target: "box::control", "could not update the discovery file: {e}");
            }
            self.state.scene = scene;
        }
    }

    /// While paused the runner keeps rendering and polling but does not tick.
    pub(crate) fn paused(&self) -> bool {
        self.state.paused
    }

    /// Record a rendered frame (`ticked`: the app was ticked this frame).
    pub(crate) fn frame_done(
        &mut self,
        ticked: bool,
        frame_ms: f32,
        update_ms: f32,
        render_ms: f32,
        renderer: &Renderer,
    ) {
        if ticked {
            self.state.frame += 1;
        }
        // A skipped window frame (occluded, surface timeout) keeps the last
        // drawn frame's numbers.
        if !renderer.stats().presented {
            return;
        }
        self.state.last = Some(LastFrame {
            stats: renderer.stats().clone(),
            frame_ms,
            update_ms,
            render_ms,
        });
    }
}

/// [`ControlHost`] for a running app: borrows the app, the renderer and the
/// [`LiveControl`] state for one [`ControlServer::poll`].
struct PlayerHost<'a> {
    app: &'a mut App,
    renderer: Option<&'a mut Renderer>,
    state: &'a mut LiveState,
}

impl ControlHost for PlayerHost<'_> {
    fn status(&mut self) -> Status {
        Status {
            mode: if self.state.paused {
                Mode::Paused
            } else {
                Mode::Play
            },
            scene: scene_of(&self.app.world, &self.state.project),
            project: Some(self.state.project.display().to_string()),
            frame: self.state.frame,
            undo: None,
        }
    }

    fn world_query(&mut self, query: &WorldQuery) -> Result<Json, ControlError> {
        query_world(&self.app.world, query)
    }

    fn apply_commands(
        &mut self,
        commands: &[Command],
        dry_run: bool,
    ) -> Result<Json, ControlError> {
        // The player has no undo, so a fresh stack per call (one that kept
        // every transaction would only grow).
        let world = &mut self.app.world;
        let mut result =
            apply_to_world(world, &mut EditStack::new(), commands, dry_run, "control")?;
        if !dry_run {
            // Derive transforms and meshes now, so a paused player's
            // screenshots and queries see the edit before the next tick.
            propagate_transforms(world);
            crate::sync_scene_meshes(world);
            tracing::info!(
                target: "box::control",
                "applied {} command(s) to the running world",
                commands.len()
            );
        }
        result["undoable"] = json!(false);
        result["saved"] = json!(false);
        result["note"] = json!(
            "applied to the running player's world only: not saved to the scene file, not undoable"
        );
        Ok(result)
    }

    fn screenshot(&mut self, size: Option<(u32, u32)>) -> Result<Screenshot, ControlError> {
        let Some(renderer) = self.renderer.as_deref_mut() else {
            return Err(ControlError::new(
                code::UNSUPPORTED,
                "this player has no renderer yet; screenshots are unavailable",
            ));
        };
        let (width, height) = size.unwrap_or_else(|| renderer.size());
        let target = match &mut self.state.target {
            Some(t) => {
                renderer.resize_viewport_target(t, width, height);
                t
            }
            slot @ None => slot.insert(renderer.create_viewport_target(width, height)),
        };
        renderer.render_to(&mut self.app.world, target, None);
        let image = renderer
            .read_viewport_target(target)
            .map_err(|e| ControlError::new(code::RENDER, e.to_string()))?;
        Screenshot::from_rgba(&image)
    }

    fn play(&mut self) -> Result<Status, ControlError> {
        if self.state.paused {
            self.state.paused = false;
            tracing::info!(target: "box::control", frame = self.state.frame, "resumed");
        }
        Ok(self.status())
    }

    fn pause(&mut self) -> Result<Status, ControlError> {
        if !self.state.paused {
            self.state.paused = true;
            tracing::info!(target: "box::control", frame = self.state.frame, "paused");
        }
        Ok(self.status())
    }

    fn step_frames(&mut self, frames: u32) -> Result<Json, ControlError> {
        for _ in 0..frames {
            self.app.tick(FIXED_DT);
            if let Some(input) = self.app.world.get_resource_mut::<Input>() {
                input.end_frame();
            }
            self.state.frame += 1;
        }
        tracing::debug!(target: "box::control", frames, frame = self.state.frame, "stepped");
        Ok(json!({ "frames": frames, "frame": self.state.frame }))
    }

    fn stop(&mut self) -> Result<Status, ControlError> {
        tracing::info!(target: "box::control", "stop requested: the player exits");
        self.app.world.resource_or_default::<AppExit>().0 = true;
        Ok(self.status())
    }

    fn read_log(&mut self, since: u64, limit: usize) -> Result<LogPage, ControlError> {
        Ok(log_capture::read(since, limit))
    }

    fn frame_stats(&mut self) -> Result<Json, ControlError> {
        let mode = self.status().mode;
        let last = self.state.last.as_ref();
        let renderer = last.map(|l| {
            let s = &l.stats;
            json!({
                "objects": s.objects,
                "visible": s.visible,
                "draws": s.draws,
                "shadow_draws": s.shadow_draws,
                "shadow_casters": s.shadow_casters,
                "triangles": s.triangles,
                "upload_bytes": s.upload_bytes,
                "buffer_reallocations": s.buffer_reallocations,
                "validation_errors": s.validation_errors,
                "cpu_render_ms": s.cpu_render_ms,
            })
        });
        Ok(json!({
            "mode": mode,
            "frame": self.state.frame,
            "entities": self.app.world.entity_count(),
            "size": self.renderer.as_ref().map(|r| r.size()),
            "frame_ms": last.map(|l| l.frame_ms),
            "update_ms": last.map(|l| l.update_ms),
            "render_ms": last.map(|l| l.render_ms),
            "renderer": renderer,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ControlClient, discovery_path};
    use crate::player::{PlayerOptions, build_app};
    use crate::{Game, Project};
    use box_core::identity::Uid;
    use std::time::Duration;
    use tracing_subscriber::layer::SubscriberExt;

    fn player_app(name: &str) -> (App, PathBuf) {
        let root = std::env::temp_dir().join(format!("box_live_{name}_{}", Uid::new()));
        Project::create(&root, "Live").unwrap();
        let options = PlayerOptions {
            project: Some(root.clone()),
            ..Default::default()
        };
        let (app, _) = build_app(&Game::new(), &options).unwrap();
        (app, root)
    }

    #[test]
    fn needs_a_project_root() {
        let err = LiveControl::start(&World::new()).err().unwrap().to_string();
        assert!(err.contains("--control needs a project"), "{err}");
    }

    #[test]
    fn player_host_semantics_without_a_gpu() {
        let subscriber = tracing_subscriber::registry().with(log_capture::CaptureLayer);
        let _guard = tracing::subscriber::set_default(subscriber);
        let (mut app, root) = player_app("host");
        let mut live = LiveControl::start(&app.world).unwrap();
        assert!(discovery_path(&root).is_file());
        let log_start = log_capture::read(0, 0).next;

        let mut host = PlayerHost {
            app: &mut app,
            renderer: None,
            state: &mut live.state,
        };
        let status = host.status();
        assert_eq!(status.mode, Mode::Play);
        assert_eq!(
            status.scene.as_deref(),
            Some("assets/scenes/main.scene.json")
        );
        assert_eq!(status.frame, 0);

        let commands = box_core::edit::parse_commands(
            r#"[{"op": "spawn", "name": "Lamp", "components": {
                "Primitive": {"shape": {"Sphere": {"radius": 0.3, "sectors": 8, "stacks": 4}}},
                "Transform": {"translation": [0.0, 2.0, 0.0]}}}]"#,
        )
        .unwrap();
        let dry = host.apply_commands(&commands, true).unwrap();
        assert_eq!(dry["dry_run"], true);
        let found = host
            .world_query(&WorldQuery {
                name: Some("lamp".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(found["total"], 0, "a dry run leaves the world unchanged");
        let applied = host.apply_commands(&commands, false).unwrap();
        assert_eq!(applied["saved"], false);
        assert_eq!(applied["undoable"], false);
        let uid = Uid::parse(applied["created"][0].as_str().unwrap()).unwrap();
        let found = host
            .world_query(&WorldQuery {
                name: Some("lamp".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(found["total"], 1);
        // Derived right away (no tick needed): mesh and global transform.
        let e = host.app.world.entity_by_uid(uid).unwrap();
        assert!(host.app.world.get::<box_render::MeshRenderer>(e).is_some());
        let y = host
            .app
            .world
            .get::<box_core::transform::GlobalTransform>(e)
            .unwrap()
            .translation()
            .y;
        assert!((y - 2.0).abs() < 1e-5);

        assert_eq!(host.pause().unwrap().mode, Mode::Paused);
        let step = host.step_frames(3).unwrap();
        assert_eq!(step, json!({ "frames": 3, "frame": 3 }));
        assert_eq!(host.status().frame, 3);
        assert_eq!(host.play().unwrap().mode, Mode::Play);
        assert!(host.screenshot(None).is_err());
        let stats = host.frame_stats().unwrap();
        assert_eq!(stats["frame"], 3);
        assert!(stats["renderer"].is_null());

        let log = host.read_log(log_start, 100).unwrap();
        let messages: Vec<&str> = log.entries.iter().map(|e| e.message.as_str()).collect();
        assert!(
            messages
                .iter()
                .any(|m| m.starts_with("applied 1 command(s)")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.starts_with("paused")),
            "{messages:?}"
        );

        assert!(!host.app.exit_requested());
        host.stop().unwrap();
        assert!(app.exit_requested());

        drop(live);
        assert!(!discovery_path(&root).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn answers_over_the_socket() {
        let (mut app, root) = player_app("socket");
        let mut live = LiveControl::start(&app.world).unwrap();
        let project = root.clone();
        let client = std::thread::spawn(move || {
            let mut c = ControlClient::connect_project(&project, Duration::from_secs(20)).unwrap();
            assert_eq!(c.discovery().kind, HostKind::Player);
            let status = c.call("status", json!({})).unwrap();
            let shot = c.call("screenshot", json!({})).unwrap_err();
            (status, shot.code().to_string())
        });
        let start = std::time::Instant::now();
        while !client.is_finished() {
            live.poll(&mut app, None);
            std::thread::sleep(Duration::from_millis(1));
            assert!(start.elapsed() < Duration::from_secs(30), "client hung");
        }
        let (status, shot) = client.join().unwrap();
        assert_eq!(status["kind"], "player");
        assert_eq!(status["mode"], "play");
        assert_eq!(status["scene"], "assets/scenes/main.scene.json");
        assert_eq!(shot, "unsupported");
        drop(live);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
