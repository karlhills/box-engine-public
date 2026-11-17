//! M8 acceptance in the editor: the second game (`examples/cinder_keep`, a
//! data project) is completable in an editor Play session, start to
//! finish, through the same scripted playthrough as the player test
//! (`box_engine/tests/m8/playthrough.rs`): both scenes, the dialogue,
//! checkpoints, a death and reload, the wave, the pause menu, the win.
//! Stop then restores the edit scene.
//!
//! One test: it installs a log subscriber, and `tracing` interest is cached
//! per process.

#[path = "../../box_engine/tests/m8/playthrough.rs"]
mod playthrough;

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_core::prelude::*;
use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_editor, layout};
use playthrough::Host;
use tracing_subscriber::layer::SubscriberExt;

const DT: f32 = 1.0 / 60.0;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copies the project without its local `.box` state (saves go to the
/// copy's `.box/saves`).
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".box" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// An editor in Play mode as a playthrough host. One full-editor capture
/// is taken mid-fight.
struct EditorHost {
    editor: Editor,
    frames: usize,
    captured: bool,
}

impl Host for EditorHost {
    fn world(&mut self) -> &mut World {
        &mut self.editor.state.world
    }

    fn step(&mut self) {
        self.editor.state.update_assets();
        // Let the edit world's importers finish what the game asked for.
        let server = self
            .editor
            .state
            .edit_world()
            .resource::<box_assets::AssetServer>()
            .clone();
        if !server.is_idle() {
            std::thread::sleep(Duration::from_millis(2));
            self.editor.state.update_assets();
        }
        self.editor.state.update_play(DT).unwrap();
        self.frames += 1;
    }

    fn capture(&mut self, name: &str) {
        if self.captured
            || name != "wave_fight.png"
            || std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
        {
            return;
        }
        self.captured = true;
        let img = capture(
            &mut self.editor,
            &CaptureOptions {
                size: [1920, 1080],
                pixels_per_point: 1.0,
                frames: 3,
            },
        )
        .unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/captures/m8/editor_play_wave.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        img.save(&out).unwrap();
    }
}

#[test]
fn cinder_keep_is_completable_in_editor_play_mode() {
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let dir = std::env::temp_dir().join(format!("box_editor_cinder_keep_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = TempDir(dir.clone());
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cinder_keep"),
        &dir,
    );
    let options = EditorOptions {
        project: Some(dir.clone()),
        no_restore: true,
        ..Default::default()
    };
    let mut editor = build_editor(&options, log.clone()).unwrap();
    editor.state.autosave_dir = dir.join(".box/autosave");
    editor.tree = layout::default_layout();
    assert!(
        editor.state.world.find_by_name("Fields").is_some(),
        "the start scene (outdoor) is open"
    );
    let saved = box_core::scene::save(&editor.state.world).unwrap();

    editor.state.start_play().unwrap();
    let mut host = EditorHost {
        editor,
        frames: 0,
        captured: false,
    };
    let report = playthrough::play_through(&mut host);
    eprintln!("editor play: {report:?} in {} frames", host.frames);
    assert!(host.editor.state.is_playing());
    assert!(report.kills >= 120, "{report:?}");
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        assert!(host.captured, "the editor capture was taken");
    }

    host.editor.state.poll_log();
    let errors: Vec<String> = log
        .lock()
        .records()
        .iter()
        .filter(|r| r.level == tracing::Level::ERROR)
        .map(|r| r.message.clone())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");

    // Stop: the edit scene is as it was (Play never touched it).
    host.editor.state.stop_play();
    assert_eq!(
        box_core::scene::save(&host.editor.state.world).unwrap(),
        saved
    );
}
