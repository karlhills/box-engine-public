//! Minimal native runs without the `dynamics` and `script` engine modules.
//!
//! `cargo test -p minimal_native` builds box_engine with this crate's
//! features only (`physics`, `audio`). Under `cargo test --workspace` cargo
//! unifies features across the workspace members, so box_engine there has
//! every module; the absence check then says so and checks nothing (the
//! `box_cli` test `lean_native_project_builds_and_runs` builds a generated
//! project without the modules in any case).

use box_engine::RunOptions;
use box_engine::box_core::scene;
use box_engine::modules;
use box_engine::player::{self, PlayerOptions};
use box_engine::prelude::*;
use minimal_native::{Beacon, Orbit, game, main_scene, main_scene_path, project_dir};

/// True when box_engine was built for this crate alone.
fn standalone() -> bool {
    let features = modules::enabled_features();
    !features.contains(&"dynamics") && !features.contains(&"script")
}

#[test]
fn the_scene_file_is_current() {
    let built = main_scene(&game()).unwrap();
    let saved = std::fs::read(main_scene_path()).unwrap();
    assert!(
        built == saved,
        "assets/scenes/main.scene.json is out of date; run `cargo run -p minimal_native --example minimal_native_author`"
    );
}

#[test]
fn built_without_dynamics_and_script() {
    if !standalone() {
        eprintln!(
            "box_engine has {:?} (features unified with other workspace members); \
             run `cargo test -p minimal_native` to check the opt-out",
            modules::enabled_features()
        );
        return;
    }
    assert_eq!(modules::enabled_features(), ["physics", "audio"]);
    let registry = game().registry();
    for name in [
        "RigidBody",
        "Velocity",
        "Joint",
        "PhysicsSettings",
        "Script",
        "ScriptIntent",
    ] {
        assert!(
            registry.component(name).is_none() && registry.resource(name).is_none(),
            "`{name}` is registered"
        );
        assert!(modules::missing_module(name).is_some(), "{name}");
    }
    for name in [
        "Collider",
        "KinematicCharacter",
        "AudioSource",
        "Orbit",
        "Beacon",
    ] {
        assert!(registry.component(name).is_some(), "`{name}` is missing");
    }
    assert!(registry.resource("AudioSettings").is_some());
}

/// Headless, no GPU: the orb starts opposite the beacon, enters its
/// trigger after about a second (lit) and leaves it again (dark).
#[test]
fn the_orb_lights_the_beacon() {
    let options = PlayerOptions {
        project: Some(project_dir()),
        watch: false,
        ..Default::default()
    };
    let (mut app, _) = player::build_app(&game(), &options).unwrap();
    let world = scene::load(
        &std::fs::read(main_scene_path()).unwrap(),
        game().registry(),
    )
    .unwrap();
    assert_eq!(world.count::<&Orbit>(), 1);
    assert_eq!(world.count::<(&Beacon, &Collider)>(), 1);

    let glow = |app: &App| {
        let beacon = app.world.find_by_name("Beacon").unwrap();
        app.world.get::<StandardMaterial>(beacon).unwrap().emissive
    };
    let dt = 1.0 / 60.0;
    app.run_frames(30, dt);
    assert_eq!(glow(&app), Color::BLACK, "dark before the orb arrives");
    app.run_frames(30, dt);
    assert_eq!(
        glow(&app),
        Beacon::default().glow,
        "lit while the orb is inside"
    );
    app.run_frames(30, dt);
    assert_eq!(glow(&app), Color::BLACK, "dark after it left");
}

/// The player renders the start scene headlessly, the beacon lit, with no
/// wgpu validation errors. Capture: `target/captures/minimal_native/main.png`.
#[test]
fn the_player_captures_the_scene() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let out = project_dir().join("../../target/captures/minimal_native/main.png");
    let mut run = RunOptions::new("minimal_native");
    run.capture = Some(out.clone());
    run.frames = Some(60);
    let options = PlayerOptions {
        project: Some(project_dir()),
        run,
        watch: false,
        ..Default::default()
    };
    let summary = player::run_game(options, game()).unwrap();
    assert_eq!(summary.frames, 60);
    assert_eq!(summary.validation_errors, 0);
    assert_eq!(summary.captures, vec![out.clone()]);
    assert_eq!(image::image_dimensions(&out).unwrap(), (1280, 720));
}
