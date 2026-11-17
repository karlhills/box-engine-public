//! The arena files: up to date with `authoring/`, loadable with the game's
//! types, and (GPU-gated) rendered by the player without validation errors.

use std::path::PathBuf;

use box_engine::RunOptions;
use box_engine::box_core::scene;
use box_engine::player::{self, PlayerOptions};
use box_engine::prelude::*;
use collector::{ArenaExit, Collectible, FollowCamera, PlayerController, authoring};

#[test]
fn scenes_match_authoring() {
    let game = authoring::game();
    for name in authoring::SCENES {
        let built = authoring::build_scene(&game, &authoring::commands_path(name)).unwrap();
        let saved = std::fs::read(authoring::scene_path(name)).unwrap();
        assert!(
            built == saved,
            "{name}.scene.json is out of date with authoring/{name}.commands.json; \
             run `cargo run -p collector --example collector_author`"
        );
    }
}

#[test]
fn each_arena_has_a_player_camera_sun_exit_and_gems() {
    let game = authoring::game();
    let mut layouts = Vec::new();
    for name in authoring::SCENES {
        let bytes = std::fs::read(authoring::scene_path(name)).unwrap();
        let world = scene::load(&bytes, game.registry()).unwrap();
        assert_eq!(world.count::<&PlayerController>(), 1, "{name}");
        assert_eq!(world.count::<(&Camera, &FollowCamera)>(), 1);
        assert_eq!(world.count::<&DirectionalLight>(), 1);
        let exits: Vec<String> = world
            .query::<&ArenaExit>()
            .iter()
            .map(|a| a.next_scene.clone())
            .collect();
        assert_eq!(exits.len(), 1, "{name}");
        assert!(authoring::SCENES.contains(&exits[0].as_str()));
        let gems = world.count::<(&Collectible, &Collider)>();
        assert!((5..=8).contains(&gems), "{name}: {gems} gems");
        for (_, c) in world.query::<(&Collectible, &Collider)>().iter() {
            assert!(c.is_trigger, "{name}: gem colliders are triggers");
        }
        let solids = world
            .query::<&Collider>()
            .iter()
            .filter(|c| !c.is_trigger)
            .count();
        assert!(solids >= 9, "{name}: floor, walls and obstacles collide");
        layouts.push((
            world.resource::<Environment>().sky_zenith,
            world.count::<&Primitive>(),
        ));
    }
    assert_ne!(layouts[0], layouts[1], "the arenas look different");
}

fn gpu_tests_skipped() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

/// `target/captures/collector/<file>`.
fn capture_path(file: &str) -> PathBuf {
    authoring::project_dir()
        .join("../../target/captures/collector")
        .join(file)
}

#[test]
fn the_player_renders_both_arenas_headlessly() {
    if gpu_tests_skipped() {
        return;
    }
    for name in authoring::SCENES {
        let out = capture_path(&format!("{name}.png"));
        let mut run = RunOptions::new("collector");
        run.capture = Some(out.clone());
        run.frames = Some(30);
        let options = PlayerOptions {
            project: Some(authoring::project_dir()),
            scene: Some(name.into()),
            run,
            watch: false,
            ..Default::default()
        };
        let summary = player::run_game(options, authoring::game()).unwrap();
        assert_eq!(summary.validation_errors, 0, "{name}");
        assert_eq!(summary.captures, vec![out.clone()]);
        assert_eq!(image::image_dimensions(&out).unwrap(), (1280, 720));
    }
}

/// Holding W from the start: the camera follows the player past the first
/// gem (collected) toward the far wall.
#[test]
fn the_player_renders_a_scripted_run() {
    if gpu_tests_skipped() {
        return;
    }
    let out = capture_path("arena-walk.png");
    let options = PlayerOptions {
        project: Some(authoring::project_dir()),
        scene: Some("arena".into()),
        ..Default::default()
    };
    let (mut app, _) = player::build_app(&authoring::game(), &options).unwrap();
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    let mut run = RunOptions::new("collector");
    run.capture = Some(out.clone());
    run.frames = Some(75);
    let summary = box_engine::run_with(app, run).unwrap();
    assert_eq!(summary.validation_errors, 0);
    assert!(out.is_file());
}

/// The editor with the player selected: the inspector shows the game's
/// components next to the engine's.
#[test]
fn the_editor_inspects_game_components() {
    if gpu_tests_skipped() {
        return;
    }
    let options = box_editor::EditorOptions {
        project: Some(authoring::project_dir()),
        ..Default::default()
    };
    let mut editor = box_editor::build_game_editor(
        &options,
        box_editor::log::LogBuffer::default(),
        authoring::game(),
    )
    .unwrap();
    let player = editor
        .state
        .world
        .query::<(&Uid, &PlayerController)>()
        .iter()
        .map(|(u, _)| *u)
        .next();
    editor.state.select(player);
    let image =
        box_editor::headless::capture(&mut editor, &Default::default()).expect("editor capture");
    let out = capture_path("editor-player.png");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    image.save(&out).unwrap();
}
