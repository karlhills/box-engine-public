//! Shared by the live-control test binaries (`control.rs`,
//! `control_socket.rs`, `playtest.rs`): a headless editor with Allow Agent
//! Control on, and editor frames run on the main thread while a
//! `ControlClient` talks to it from another.
//!
//! Screenshots and the GPU cache need a GPU; with `BOX_SKIP_GPU_TESTS` (or no
//! adapter) the editor runs its UI without rendering.

// Each binary uses a different part of this module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_editor, layout};
use box_render::RenderError;
use egui::{RawInput, Rect, pos2, vec2};

pub const SCENE: &str = "assets/scenes/main.scene.json";

/// An empty temporary folder, unique per test binary, name and process.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "box_editor_{}_{name}_{}",
        env!("CARGO_CRATE_NAME"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A new project in a temporary folder, open in an editor with Allow Agent
/// Control on; returns the editor and the project root.
pub fn project_editor(name: &str, log: LogBuffer) -> (Editor, PathBuf) {
    let dir = temp_dir(name);
    box_engine::Project::create(&dir, name).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        control: true,
        ..Default::default()
    };
    let mut editor = build_editor(&options, log).unwrap();
    editor.tree = layout::default_layout();
    editor.state.autosave_dir = dir.join("autosave");
    let root = editor.state.project.as_ref().unwrap().root().to_path_buf();
    (editor, root)
}

/// Runs editor frames on this thread: rendered with a GPU, UI only without.
pub enum Frames {
    Gpu(Box<Headless>),
    Ui { ctx: egui::Context, frame: u32 },
}

impl Frames {
    pub fn new(editor: &mut Editor) -> Self {
        if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
            let options = CaptureOptions {
                size: [1280, 720],
                pixels_per_point: 1.0,
                frames: 1,
            };
            match Headless::new(editor, &options) {
                Ok(h) => return Self::Gpu(Box::new(h)),
                Err(e)
                    if matches!(
                        e.downcast_ref::<RenderError>(),
                        Some(RenderError::NoAdapter)
                    ) => {}
                Err(e) => panic!("{e:#}"),
            }
        }
        eprintln!("no GPU (or BOX_SKIP_GPU_TESTS): running the editor UI without rendering");
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        Self::Ui { ctx, frame: 0 }
    }

    pub fn gpu(&self) -> bool {
        matches!(self, Self::Gpu(_))
    }

    pub fn frame(&mut self, editor: &mut Editor) {
        match self {
            Self::Gpu(h) => {
                h.frame(editor, Vec::new());
            }
            Self::Ui { ctx, frame } => {
                editor.poll_control(None);
                let raw = RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
                    time: Some(*frame as f64 / 60.0),
                    focused: true,
                    ..Default::default()
                };
                let _ = ctx.run_ui(raw, |ui| editor.ui(ui));
                *frame += 1;
            }
        }
    }

    /// Runs frames until `conversation` finishes; returns its result.
    pub fn drive<T>(&mut self, editor: &mut Editor, conversation: thread::JoinHandle<T>) -> T {
        let start = Instant::now();
        while !conversation.is_finished() {
            self.frame(editor);
            thread::sleep(Duration::from_millis(1));
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "conversation hung"
            );
        }
        conversation.join().unwrap()
    }
}
