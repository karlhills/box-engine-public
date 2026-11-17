//! A scene that uses `RigidBody` and `Script` plays in a build without the
//! `dynamics` and `script` modules: the data is kept, each type gets one
//! warning naming its feature, and the game runs. (Built with every module,
//! as under `cargo test --workspace`, the scene loads the real components.)
//!
//! The only test in this binary: it installs a log subscriber, and tracing
//! caches interest globally.

use std::io::Write;
use std::sync::{Arc, Mutex};

use box_engine::box_core::scene::UnknownComponents;
use box_engine::modules;
use box_engine::player::{self, PlayerOptions};
use box_engine::prelude::*;
use minimal_native::{game, main_scene_path, project_dir};
use serde_json::{Value as Json, json};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

const SPIN: &str = "local Spin = {}\nfunction Spin.on_update(self, dt)\n    self.entity:rotate_y(dt)\nend\nreturn Spin\n";

#[test]
fn a_scene_with_rigid_bodies_and_scripts_loads_with_warnings() {
    // A copy of the project whose scene gives the orb a body and a script.
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("minimal_native_modules");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("assets/scenes")).unwrap();
    std::fs::create_dir_all(root.join("assets/scripts")).unwrap();
    std::fs::copy(
        project_dir().join("box.project.json"),
        root.join("box.project.json"),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("assets/audio")).unwrap();
    std::fs::copy(
        project_dir().join("assets/audio/chime.wav"),
        root.join("assets/audio/chime.wav"),
    )
    .unwrap();
    std::fs::write(root.join("assets/scripts/spin.luau"), SPIN).unwrap();
    let mut tree: Json =
        serde_json::from_slice(&std::fs::read(main_scene_path()).unwrap()).unwrap();
    let orb = tree["entities"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|e| e["name"] == "Orb")
        .unwrap();
    let components = orb["components"].as_object_mut().unwrap();
    components.insert(
        "RigidBody".into(),
        json!({
            "kind": "Kinematic", "mass": 1.0, "linear_damping": 0.0, "angular_damping": 0.05,
            "gravity_scale": 1.0, "ccd": false, "can_sleep": true,
            "locks": {
                "translate_x": false, "translate_y": false, "translate_z": false,
                "rotate_x": false, "rotate_y": false, "rotate_z": false
            }
        }),
    );
    components.insert(
        "Script".into(),
        json!({ "source": "scripts/spin.luau", "properties": [] }),
    );
    std::fs::write(
        root.join("assets/scenes/main.scene.json"),
        serde_json::to_vec_pretty(&tree).unwrap(),
    )
    .unwrap();

    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    let options = PlayerOptions {
        project: Some(root.clone()),
        watch: false,
        ..Default::default()
    };
    let app = tracing::subscriber::with_default(subscriber, || {
        let (mut app, _) = player::build_app(&game(), &options).expect("the scene loads");
        app.run_frames(90, 1.0 / 60.0);
        app
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let world = &app.world;
    let orb = world.find_by_name("Orb").unwrap();
    // The game still runs: the orb orbits.
    let at = world.get::<Transform>(orb).unwrap().translation;
    assert!(
        at.distance(Vec3::new(-2.5, 0.6, 0.0)) > 1.0,
        "the orb moved ({at})"
    );

    let features = modules::enabled_features();
    if features.contains(&"dynamics") || features.contains(&"script") {
        eprintln!("box_engine has {features:?}; the components load as real ones");
        assert!(world.get::<UnknownComponents>(orb).is_none(), "{text}");
        return;
    }
    let kept = world.get::<UnknownComponents>(orb).expect("data kept");
    assert!(kept.0.contains_key("RigidBody") && kept.0.contains_key("Script"));
    drop(kept);
    for (name, feature) in [("RigidBody", "dynamics"), ("Script", "script")] {
        let line = text
            .lines()
            .find(|l| l.contains(&format!("component `{name}`")))
            .unwrap_or_else(|| panic!("no warning for {name}:\n{text}"));
        assert!(
            line.contains(&format!("box_engine's `{feature}` feature")),
            "{line}"
        );
        assert_eq!(
            text.matches(&format!("component `{name}`")).count(),
            1,
            "one warning per type:\n{text}"
        );
    }
    assert_eq!(
        text.lines().count(),
        2,
        "nothing else is reported (the script file is not imported):\n{text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
