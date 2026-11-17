//! The native-plugin guide's samples behave as the guide says: headless
//! apps from `game().new_app()` and the sample project played by the
//! player's `build_app`.

use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use guide_native::gameplay::{Mover, PickedUp, Pickup, Score, Tuning};
use guide_native::{game, project_dir};

const DT: f32 = 1.0 / 60.0;

fn pickup(app: &mut App, x: f32, points: u32) -> Entity {
    app.world.spawn((
        Transform::from_xyz(x, 0.0, 0.0),
        Pickup {
            points,
            bob_height: 0.1,
            note: String::new(),
        },
    ))
}

#[test]
fn the_registry_has_the_game_types_with_their_metadata() {
    let registry = game().registry();
    let pickup = registry.component("Pickup").expect("Pickup registered");
    let fields = pickup.fields();
    let points = fields.iter().find(|f| f.name == "points").unwrap();
    assert_eq!(points.step, Some(1.0));
    assert_eq!(points.doc, Some("Score for collecting it."));
    let bob = fields.iter().find(|f| f.name == "bob_height").unwrap();
    assert_eq!(bob.unit, Some("m"));
    assert!(fields.iter().find(|f| f.name == "note").unwrap().hidden);
    assert!(registry.component("Mover").is_some());
    assert!(registry.settings().any(|s| s.name() == "Tuning"));
    assert!(registry.event("PickedUp").is_some());
    assert!(
        registry.component("Score").is_none(),
        "runtime resource: not saved"
    );
    assert!(game().registration_errors().is_empty());
}

#[test]
fn systems_are_ordered_by_name() {
    let mut app = game().new_app();
    app.check_order().unwrap();
    let names = app.system_names(Stage::FixedUpdate);
    let at = |n: &str| names.iter().position(|s| s.ends_with(n)).unwrap();
    assert!(at("move_player") < at("collect_pickups"), "{names:?}");
}

#[test]
fn hooks_add_runtime_state_and_settings_default() {
    let mut app = game().new_app();
    let player = app.world.spawn((Transform::default(), Mover::default()));
    let gem = pickup(&mut app, 5.0, 1);
    app.run_frames(1, DT);
    assert!(app.world.has::<Interpolated>(player));
    assert!(app.world.has::<Tween>(gem));
    // No project: the settings resource has its default value.
    assert_eq!(app.world.resource::<Tuning>().pickup_radius, 1.0);
}

#[test]
fn the_move_action_moves_the_player() {
    let mut app = game().new_app();
    let player = app
        .world
        .spawn((Transform::default(), Mover { speed: 4.0 }));
    app.world.resource_mut::<Input>().press_key(KeyCode::D);
    app.run_frames(60, DT);
    app.world.resource_mut::<Input>().release_key(KeyCode::D);
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    app.run_frames(60, DT);
    let at = app.world.get::<Transform>(player).unwrap().translation;
    assert!((at.x - 4.0).abs() < 0.2, "{at}");
    assert!((at.z + 4.0).abs() < 0.2, "forward is -Z: {at}");
}

#[test]
fn pickups_near_the_player_are_collected_and_scored() {
    let mut app = game().new_app();
    app.world.spawn((Transform::default(), Mover::default()));
    let near = pickup(&mut app, 0.5, 10);
    let far = pickup(&mut app, 8.0, 20);
    app.run_frames(2, DT);
    assert!(!app.world.contains(near), "collected and despawned");
    assert!(app.world.contains(far));
    assert_eq!(app.world.resource::<Score>().points, 10);
    assert_eq!(app.world.resource::<Score>().levels, 0);
}

#[test]
fn a_registered_event_sent_from_rust_reaches_the_handler() {
    let mut app = game().new_app();
    pickup(&mut app, 50.0, 1); // one left: no level restart
    app.world.send(PickedUp { points: 7 });
    app.run_frames(1, DT);
    assert_eq!(app.world.resource::<Score>().points, 7);
}

#[test]
fn the_script_api_is_declared_for_luau() {
    let definitions = game().script_definitions();
    assert!(definitions.contains("declare score: {"), "{definitions}");
    assert!(definitions.contains("add: (points: number) -> ()"));
}

// ----- the sample project, as the player runs it -----

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut app.world, Duration::from_millis(20));
        }
        app.run_frames(1, DT);
    }
}

fn play() -> App {
    let options = PlayerOptions::from_args([
        project_dir().display().to_string(),
        "--headless".into(),
        "--no-watch".into(),
    ])
    .unwrap();
    let (app, _project) = build_app(&game(), &options).unwrap();
    app
}

fn move_to(app: &mut App, name: &str) {
    let to = app
        .world
        .find_by_name(name)
        .and_then(|e| app.world.get::<Transform>(e))
        .map(|t| t.translation)
        .unwrap();
    let player = app.world.find_by_name("Player").unwrap();
    app.world.get_mut::<Transform>(player).unwrap().translation = to;
}

#[test]
fn the_projects_action_file_adds_bindings() {
    let mut app = play();
    frames(&mut app, 2);
    let player = app.world.find_by_name("Player").unwrap();
    let x = |app: &App| app.world.get::<Transform>(player).unwrap().translation.x;
    let start = x(&app);
    // `L` is bound only in assets/settings/input.actions.json.
    app.world.resource_mut::<Input>().press_key(KeyCode::L);
    frames(&mut app, 30);
    assert!(x(&app) - start > 2.0, "{} -> {}", start, x(&app));
}

#[test]
fn the_project_plays_with_settings_script_api_and_level_restart() {
    let mut app = play();
    // The settings file overrides the default.
    assert_eq!(app.world.resource::<Tuning>().pickup_radius, 1.2);
    frames(&mut app, 5);
    // bonus.luau: `score.add(5)` through the Rust API, then a `PickedUp`
    // of 1 point sent by name and handled by the Rust `on_event`.
    assert_eq!(app.world.resource::<Score>().points, 6);
    assert_eq!(app.world.count::<&Pickup>(), 3);

    for name in ["Pickup 1", "Pickup 2", "Pickup 3"] {
        move_to(&mut app, name);
        frames(&mut app, 2);
    }
    frames(&mut app, 5);
    let score = app.world.resource::<Score>();
    assert_eq!(score.levels, 1, "cleared once");
    // 6 + 10 + 10 + 20, then the restarted level's bonus again.
    assert_eq!(score.points, 46 + 6);
    assert_eq!(app.world.count::<&Pickup>(), 3, "the level restarted");
}
