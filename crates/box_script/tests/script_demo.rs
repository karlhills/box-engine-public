//! The `examples/script_demo` data project (no Rust) played headlessly
//! with the stock engine game: spinning, waits, input-driven movement, a
//! trigger collecting a gem, a script signal updating the score.

use std::path::PathBuf;
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_script::{ScriptRuntime, save_script_state};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;

fn demo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/script_demo")
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut app.world, Duration::from_millis(20));
        }
        app.run_frames(1, 1.0 / 60.0);
    }
}

fn entity(app: &App, name: &str) -> Entity {
    app.world
        .find_by_name(name)
        .unwrap_or_else(|| panic!("{name}"))
}

#[test]
fn the_script_demo_plays_without_rust() {
    let options =
        PlayerOptions::from_args([demo().display().to_string(), "--headless".into()]).unwrap();
    let (mut app, _project) = build_app(&Game::new(), &options).unwrap();
    frames(&mut app, 10);
    let stats = app.world.resource::<ScriptRuntime>().stats();
    assert_eq!(stats.instances, 8, "{stats:?}");
    assert_eq!(stats.disabled, 0);

    // The totem spins; the beacon blinks (a wait loop).
    let totem = entity(&app, "Totem");
    let beacon = entity(&app, "Beacon");
    let r0 = app.world.get::<Transform>(totem).unwrap().rotation;
    let mut intensities = Vec::new();
    for _ in 0..12 {
        frames(&mut app, 10);
        intensities.push(app.world.get::<PointLight>(beacon).unwrap().intensity);
    }
    let r1 = app.world.get::<Transform>(totem).unwrap().rotation;
    assert!(r0.angle_between(r1) > 0.1, "the totem turned");
    assert!(intensities.iter().any(|i| *i > 30.0) && intensities.iter().any(|i| *i < 10.0));

    // Walk forward (W) into Gem A.
    let player = entity(&app, "Player");
    let z0 = app.world.get::<Transform>(player).unwrap().translation.z;
    let gem = entity(&app, "Gem A");
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    frames(&mut app, 45);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    frames(&mut app, 2);
    let p = app.world.get::<Transform>(player).unwrap().translation;
    assert!(p.z < z0 - 2.0, "moved forward: {z0} -> {}", p.z);
    assert!((p.y - 0.5).abs() < 0.1, "stays on the ground: {}", p.y);
    assert!(!app.world.contains(gem), "collected");
    let rules = entity(&app, "Rules");
    let score: i64 = app
        .world
        .resource::<ScriptRuntime>()
        .instance(rules)
        .unwrap()
        .get("score")
        .unwrap();
    assert_eq!(score, 10);
    let saved = save_script_state(&mut app.world);
    let uid = app.world.uid(rules).unwrap().to_string();
    assert_eq!(saved[&uid]["score"], 10);
    assert_eq!(app.world.resource::<ScriptRuntime>().stats().disabled, 0);
}
