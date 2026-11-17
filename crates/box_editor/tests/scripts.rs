//! Scripts in the editor (M7): the inspector shows a script's declared
//! properties (the edit world never runs scripts), Play runs them, editing
//! the file during Play takes effect without restarting Play, a script
//! error is reported in the Console with file and line (and as a toast)
//! without stopping Play, and Stop restores the edit scene.
//!
//! One test: it installs a log subscriber for the Console, and `tracing`
//! interest is cached per process.

use std::path::PathBuf;
use std::time::Duration;

use box_core::edit::EditStack;
use box_core::identity::Uid;
use box_core::prelude::*;
use box_core::world::Placement;
use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor};
use box_engine::box_script::{Script, ScriptAsset, ScriptRuntime, ScriptValue};
use egui::{Pos2, Rect, vec2};
use tracing_subscriber::layer::SubscriberExt;

const SPINNER: &str = r#"
local Spinner = {}

Spinner.properties = {
    speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s", doc = "Turn rate" },
    axis = { default = vector.create(0, 1, 0), doc = "Turn axis" },
    label = { default = "v1" },
}

function Spinner.on_start(self)
    self.turns = 0
end

function Spinner.on_update(self, dt)
    self.turns += 1
    self.entity:rotate(Quat.from_axis_angle(self.props.axis, math.rad(self.props.speed) * dt))
    self.entity:set_name(self.props.label)
end

return Spinner
"#;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ui_frame(ctx: &egui::Context, editor: &mut box_editor::Editor, time: f64) {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
        time: Some(time),
        predicted_dt: 1.0 / 60.0,
        focused: true,
        ..Default::default()
    };
    let _ = ctx.run_ui(raw, |ui| editor.ui(ui));
}

#[test]
fn scripts_run_in_play_reload_and_report_errors_in_the_console() {
    let log = LogBuffer::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(log.layer()));
    let dir = std::env::temp_dir().join(format!("box_editor_scripts_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _cleanup = TempDir(dir.clone());
    box_engine::Project::create(&dir, "scripts").unwrap();
    let script_path = dir.join("assets/scripts/spinner.luau");
    std::fs::create_dir_all(script_path.parent().unwrap()).unwrap();
    std::fs::write(&script_path, SPINNER).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, log.clone()).unwrap();

    // A spinner with one overridden property.
    let uid = Uid::new();
    let script = Script::new("scripts/spinner.luau").with("speed", ScriptValue::Number(180.0));
    let cube = Primitive::cuboid(Vec3::ONE);
    let mut edits = EditStack::new();
    edits
        .spawn(
            &mut editor.state.world,
            uid,
            "Spinner",
            Placement::last(None),
            &[("Script", &script), ("Primitive", &cube)],
        )
        .unwrap();
    editor.state.select(Some(uid));

    // The inspector asks for the script's declarations; the edit world never
    // gets a script VM.
    let ctx = egui::Context::default();
    editor.apply_theme(&ctx);
    for i in 0..4 {
        ui_frame(&ctx, &mut editor, f64::from(i) / 60.0);
        assert!(editor.state.wait_assets_idle(Duration::from_secs(20)));
    }
    let world = editor.state.edit_world();
    let lookup = world.resource::<box_core::assets::AssetLookup>();
    let handle = lookup
        .get::<ScriptAsset>(&script.source)
        .expect("the inspector loaded the script");
    let asset = world
        .resource::<box_core::assets::Assets<ScriptAsset>>()
        .get(handle)
        .unwrap();
    let names: Vec<&str> = asset.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["speed", "axis", "label"]);
    assert!(!world.contains_resource::<ScriptRuntime>());

    // The inspector with the Script section (GPU runs only).
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        let img = capture(
            &mut editor,
            &CaptureOptions {
                size: [1600, 1000],
                pixels_per_point: 1.0,
                frames: 4,
            },
        )
        .unwrap();
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/captures/m7/editor_script_inspector.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        img.save(&out).unwrap();
    }
    let saved = box_core::scene::save(&editor.state.world).unwrap();

    // Play runs the script.
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
    let named = |name: &'static str| {
        move |w: &World| {
            w.entity_by_uid(uid)
                .and_then(|e| w.get::<Name>(e).map(|n| n.as_str() == name))
                .unwrap_or(false)
        }
    };
    assert!(play_frames(&mut editor, &named("v1")), "the script ran");
    let turns = |w: &World| -> i64 {
        let e = w.entity_by_uid(uid).unwrap();
        w.resource::<ScriptRuntime>()
            .instance(e)
            .unwrap()
            .get::<i64>("turns")
            .unwrap()
    };
    let before = turns(&editor.state.world);

    // Editing the file during Play takes effect without restarting Play,
    // keeping the script's state.
    std::fs::write(
        &script_path,
        SPINNER.replace("default = \"v1\"", "default = \"v2\""),
    )
    .unwrap();
    let server = editor
        .state
        .edit_world()
        .resource::<box_assets::AssetServer>()
        .clone();
    server.rescan();
    assert!(
        play_frames(&mut editor, &named("v2")),
        "the edit reached Play"
    );
    assert!(editor.state.is_playing());
    assert!(
        turns(&editor.state.world) > before,
        "state kept across the reload"
    );

    // A runtime error: reported with file and line, Play keeps running.
    std::fs::write(
        &script_path,
        SPINNER.replace("self.turns += 1", "self.turns += nil"),
    )
    .unwrap();
    server.rescan();
    let disabled = |w: &World| {
        w.get_resource::<ScriptRuntime>()
            .is_some_and(|rt| rt.stats().disabled == 1)
    };
    assert!(
        play_frames(&mut editor, &disabled),
        "the error disabled the script"
    );
    for _ in 0..5 {
        assert!(editor.state.update_play(1.0 / 60.0).unwrap());
    }
    editor.state.poll_log();
    let records: Vec<(String, String, tracing::Level)> = log
        .lock()
        .records()
        .iter()
        .map(|r| (r.target.clone(), r.message.clone(), r.level))
        .collect();
    let error = records
        .iter()
        .find(|(t, m, l)| {
            t == "box_script" && *l == tracing::Level::ERROR && m.contains("arithmetic")
        })
        .unwrap_or_else(|| panic!("no script error in the console: {records:#?}"));
    assert!(
        error.1.starts_with("scripts/spinner.luau:15:"),
        "{}",
        error.1
    );
    assert!(error.1.contains("in on_update of \"v2\""), "{}", error.1);
    assert!(
        editor
            .state
            .toasts()
            .items()
            .iter()
            .any(|t| t.text.contains("scripts/spinner.luau:15")),
        "a toast"
    );

    // Stop: the edit scene is as it was.
    editor.state.stop_play();
    assert_eq!(box_core::scene::save(&editor.state.world).unwrap(), saved);
}
