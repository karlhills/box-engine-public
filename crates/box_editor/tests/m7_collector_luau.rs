//! M7 acceptance in the editor: the Luau collector (`examples/collector_luau`,
//! a data project) plays in an editor Play session, the inspector shows a
//! running script's properties, and editing a script during Play takes
//! effect without restarting Play. (Script errors with file and line, and
//! the editor surviving them, are covered by the `scripts` test.)
//!
//! One test: it installs a log subscriber, and `tracing` interest is cached
//! per process.

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_core::identity::Uid;
use box_core::prelude::*;
use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};
use box_engine::box_script::ScriptRuntime;
use box_engine::box_ui::Ui;
use tracing_subscriber::layer::SubscriberExt;

/// The player and camera entities of `assets/scenes/arena.scene.json`.
const PLAYER: &str = "c0ffee00-0000-4000-8000-100100000010";
const CAMERA: &str = "c0ffee00-0000-4000-8000-100100000011";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Copies the project (without its local `.box` state) so the test can
/// edit scripts.
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

#[test]
fn the_luau_collector_plays_in_the_editor_and_reloads_scripts_during_play() {
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let dir =
        std::env::temp_dir().join(format!("box_editor_collector_luau_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = TempDir(dir.clone());
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/collector_luau"),
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
    let player = Uid::parse(PLAYER).unwrap();
    assert!(
        editor.state.world.entity_by_uid(player).is_some(),
        "the start scene (arena) is open"
    );
    let saved = box_core::scene::save(&editor.state.world).unwrap();

    // Play: the scripts start, the game builds its HUD.
    editor.state.start_play().unwrap();
    let play_frames = |editor: &mut box_editor::Editor, until: &dyn Fn(&World) -> bool| {
        for _ in 0..600 {
            editor.state.update_assets();
            editor.state.update_play(1.0 / 60.0).unwrap();
            if until(&editor.state.world) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    };
    let has_hud = |w: &World| {
        w.get_resource::<Ui>()
            .is_some_and(|ui| ui.find(None, "score").is_some())
    };
    assert!(play_frames(&mut editor, &has_hud), "the game started");
    let game = |w: &World| w.find_by_name("Game").expect("the game entity");
    let stats = editor.state.world.resource::<ScriptRuntime>().stats();
    assert_eq!((stats.errors, stats.disabled), (0, 0), "{stats:?}");
    assert!(stats.instances >= 10, "{stats:?}");

    // The inspector shows the running camera script's properties.
    editor.state.select(Uid::parse(CAMERA));
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        let img = capture(
            &mut editor,
            &CaptureOptions {
                size: [1600, 1000],
                pixels_per_point: 1.0,
                frames: 6,
            },
        )
        .unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/captures/m7/editor_collector_luau_play.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        img.save(&out).unwrap();
    }
    assert!(editor.state.is_playing());

    // Editing a script during Play: the new code runs at once, the game's
    // state (its score table, its UI) is kept.
    let path = dir.join("assets/scripts/game.luau");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace(
            "function Game.on_update(self, dt: number)\n",
            "function Game.on_update(self, dt: number)\n    self.edited = true\n",
        ),
    )
    .unwrap();
    editor
        .state
        .edit_world()
        .resource::<box_assets::AssetServer>()
        .clone()
        .rescan();
    let edited = |w: &World| {
        w.get_resource::<ScriptRuntime>()
            .and_then(|rt| rt.instance(game(w)))
            .is_some_and(|t| t.get::<bool>("edited").unwrap_or(false))
    };
    assert!(play_frames(&mut editor, &edited), "the edit reached Play");
    assert!(editor.state.is_playing());
    let rt = editor.state.world.resource::<ScriptRuntime>();
    assert!(rt.stats().reloads >= 1);
    assert_eq!(rt.stats().errors, 0);
    let score: i64 = rt
        .instance(game(&editor.state.world))
        .unwrap()
        .get("score")
        .unwrap();
    assert_eq!(score, 0, "state kept");

    editor.state.poll_log();
    let errors: Vec<String> = log
        .lock()
        .records()
        .iter()
        .filter(|r| r.level == tracing::Level::ERROR)
        .map(|r| r.message.clone())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");

    // Stop: the edit scene is as it was.
    editor.state.stop_play();
    assert_eq!(box_core::scene::save(&editor.state.world).unwrap(), saved);
}
