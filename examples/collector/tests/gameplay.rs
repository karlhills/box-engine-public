//! The sample game run headlessly (no GPU) for a fixed number of frames with
//! scripted keyboard and gamepad input: movement, walls, landing, ramps,
//! collecting and the switch between the two arenas, in the player and in an
//! editor Play session.

use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use collector::{Collectible, FollowCamera, PlayerController, Score, authoring};

const DT: f32 = 1.0 / 60.0;
/// Capsule centre height when standing on a floor whose top is at y = 0
/// (half the height plus the character controller's skin).
const STAND: f32 = 0.9 + 0.02;

fn app(scene: &str) -> App {
    let options = PlayerOptions {
        project: Some(authoring::project_dir()),
        scene: Some(scene.into()),
        ..Default::default()
    };
    build_app(&authoring::game(), &options).unwrap().0
}

fn player(world: &World) -> Entity {
    world.first_with::<PlayerController>().expect("a player")
}

fn pos(world: &World) -> Vec3 {
    world.get::<Transform>(player(world)).unwrap().translation
}

fn state(world: &World) -> KinematicCharacter {
    KinematicCharacter::clone(&world.get::<KinematicCharacter>(player(world)).unwrap())
}

/// Put the player at `at`, at rest.
fn teleport(world: &mut World, at: Vec3) {
    let p = player(world);
    world.component_mut::<Transform>(p).unwrap().translation = at;
    if let Some(s) = world.component_mut::<KinematicCharacter>(p) {
        s.velocity = Vec3::ZERO;
    }
}

fn gem_positions(world: &World) -> Vec<Vec3> {
    world
        .query::<(&Transform, &Collectible)>()
        .iter()
        .map(|(t, _)| t.translation)
        .collect()
}

fn scene_file(world: &World) -> String {
    let scenes = world.resource::<Scenes>();
    let main = scenes.main().expect("a main scene");
    scenes
        .path(main)
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn close(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

#[test]
fn keyboard_walks_across_floor_seams_and_collects_the_gem_on_its_path() {
    let mut app = app("arena");
    app.run_frames(10, DT);
    let start = pos(&app.world);
    assert!(state(&app.world).grounded, "standing at {start}");
    assert!(close(start.y, STAND, 0.01), "{start}");
    assert!(app.world.find_by_name("Gem 1").is_some());

    // The camera looks down -Z: W walks forward along the x = 0 seam
    // between floor tiles and across the z = 0 seam, through Gem 1 at the
    // centre.
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    app.run_frames(60, DT);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    app.run_frames(2, DT);
    let end = pos(&app.world);
    let walked = start.z - end.z;
    assert!(
        close(walked, 6.0, 0.25),
        "walked {walked} m in 1 s at 6 m/s ({start} -> {end})"
    );
    assert!(close(end.x, start.x, 0.01), "drifted sideways: {end}");
    assert!(close(end.y, STAND, 0.01), "left the floor: {end}");
    assert!(state(&app.world).grounded);

    assert!(app.world.find_by_name("Gem 1").is_none(), "Gem 1 collected");
    let score = *app.world.resource::<Score>();
    assert_eq!((score.points, score.collected), (10, 1));
    assert_eq!(app.world.count::<&Collectible>(), 5);
}

/// The wind loop plays from the start; taking a gem plays the pickup sound
/// where the gem was. Mixed in memory (no audio device).
#[test]
fn the_arena_plays_its_wind_loop_and_a_pickup_sound() {
    use box_engine::box_audio::{AudioOutput, OutputKind};
    let mut app = app("arena");
    app.world
        .insert_resource(AudioOutput::Capture { sample_rate: 8000 });
    app.run_frames(10, DT);
    // The gems asked for the pickup sound on frame 1; let it finish loading.
    box_engine::settle_assets(&mut app.world, std::time::Duration::from_secs(10));
    // A clip that landed during the settle starts its voice in the next
    // frame's `update_audio` (under load the wind loop can be one of them).
    app.run_frames(1, DT);
    let audio = app.world.resource::<Audio>();
    assert_eq!(audio.output(), OutputKind::Capture);
    let voices: Vec<_> = audio.voices().cloned().collect();
    assert_eq!(voices.len(), 1, "{voices:?}");
    assert_eq!(
        (voices[0].bus, voices[0].looping),
        (AudioBus::Ambience, true)
    );
    assert!(voices[0].source.is_some());

    let gem = app.world.find_by_name("Gem 1").unwrap();
    let at = app.world.get::<GlobalTransform>(gem).unwrap().translation();
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    for _ in 0..90 {
        app.run_frames(1, DT);
        if app.world.find_by_name("Gem 1").is_none() {
            break;
        }
    }
    assert!(app.world.find_by_name("Gem 1").is_none(), "collected");
    app.run_frames(1, DT);
    let audio = app.world.resource::<Audio>();
    assert_eq!(audio.pending(), 0, "preloaded: no wait for the import");
    let pickup = audio
        .voices()
        .find(|v| v.source.is_none())
        .expect("the pickup one-shot");
    assert_eq!(pickup.clip, AssetRef::path(collector::PICKUP_SOUND));
    assert_eq!(pickup.bus, AudioBus::Effects);
    let p = pickup.position.unwrap();
    assert!((p - at).length() < 0.5, "played at the gem: {p} vs {at}");
    assert!(audio.levels().unwrap().peak > 0.01, "something was mixed");
}

/// The player moves at the 60 Hz fixed rate; on a 144 Hz display it is
/// drawn (its `GlobalTransform`) between steps, so it moves every frame
/// by the same amount instead of stopping on frames without a step.
#[test]
fn at_144_hz_the_player_is_drawn_moving_every_frame() {
    let mut app = app("arena");
    app.run_frames(10, DT);
    let drawn = |w: &World| w.get::<GlobalTransform>(player(w)).unwrap().translation().z;
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    app.run_frames(30, 1.0 / 144.0); // up to speed
    let (mut sim_steps, mut moves) = (Vec::new(), Vec::new());
    for _ in 0..60 {
        let (z, sim) = (drawn(&app.world), pos(&app.world).z);
        app.run_frames(1, 1.0 / 144.0);
        moves.push(z - drawn(&app.world));
        sim_steps.push(sim - pos(&app.world).z);
    }
    let per_frame = 6.0 / 144.0;
    assert!(
        sim_steps.iter().filter(|d| **d == 0.0).count() > 20,
        "the simulation itself stands still on most frames"
    );
    for (i, d) in moves.iter().enumerate() {
        assert!(
            close(*d, per_frame, per_frame * 0.05),
            "frame {i}: drawn moved {d}"
        );
    }
}

#[test]
fn gamepad_moves_relative_to_the_camera_and_jumps() {
    let pad = GamepadId(0);
    let mut app = app("arena");
    app.run_frames(10, DT);
    let start = pos(&app.world);

    // Left stick right: +X (the camera looks down -Z).
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(pad, GamepadAxis::LeftStickX, 1.0);
    app.run_frames(30, DT);
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.0);
    app.run_frames(2, DT);
    let moved = pos(&app.world) - start;
    assert!(close(moved.x, 3.0, 0.2), "{moved}");
    assert!(close(moved.z, 0.0, 0.01), "{moved}");

    // South jumps: up about v^2 / 2g = 1.4 m, then back onto the floor.
    app.world
        .resource_mut::<Input>()
        .press_gamepad_button(pad, GamepadButton::South);
    let mut top: f32 = 0.0;
    for _ in 0..90 {
        app.run_frames(1, DT);
        top = top.max(pos(&app.world).y);
    }
    app.world
        .resource_mut::<Input>()
        .release_gamepad_button(pad, GamepadButton::South);
    app.run_frames(2, DT);
    assert!(close(top - STAND, 1.4, 0.1), "jump apex {top}");
    let end = pos(&app.world);
    assert!(close(end.y, STAND, 0.01), "landed at {end}");
    assert!(state(&app.world).grounded);
}

#[test]
fn walls_stop_the_player_even_at_absurd_speed() {
    let mut app = app("arena");
    app.run_frames(5, DT);
    // The east wall's inner face is at x = 12.
    let limit = 12.0 - 0.4;
    for speed in [6.0, 300.0] {
        let p = player(&app.world);
        app.world
            .component_mut::<PlayerController>(p)
            .unwrap()
            .speed = speed;
        teleport(&mut app.world, Vec3::new(9.0, STAND, -1.0));
        app.run_frames(30, DT); // let the camera settle
        app.world.resource_mut::<Input>().press_key(KeyCode::D);
        app.run_frames(60, DT);
        app.world.resource_mut::<Input>().release_key(KeyCode::D);
        app.run_frames(1, DT);
        let end = pos(&app.world);
        assert!(end.x <= limit, "speed {speed}: through the wall at {end}");
        assert!(end.x > limit - 0.1, "speed {speed}: stopped short at {end}");
        assert!(close(end.y, STAND, 0.01), "speed {speed}: {end}");
    }
}

/// Walk forward (W) for half a second and return the horizontal direction.
fn walk_direction(app: &mut App) -> Vec3 {
    let start = pos(&app.world);
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    app.run_frames(30, DT);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    app.run_frames(1, DT);
    let d = pos(&app.world) - start;
    Vec3::new(d.x, 0.0, d.z).normalize()
}

#[test]
fn the_right_stick_and_the_locked_mouse_turn_the_view_and_movement_follows() {
    let mut app = app("arena");
    app.run_frames(5, DT);
    teleport(&mut app.world, Vec3::new(-2.0, STAND, 1.0));
    app.run_frames(2, DT);
    let camera = app.world.first_with::<FollowCamera>().unwrap();
    let forward = |w: &World| {
        let f = w.get::<Transform>(camera).unwrap().forward();
        Vec3::new(f.x, 0.0, f.z).normalize()
    };
    assert!(forward(&app.world).dot(Vec3::NEG_Z) > 0.99);

    // Right stick right for 0.5 s at 2.5 rad/s: the view turns 1.25 rad to
    // the right, and W walks the new way.
    let pad = GamepadId(0);
    let input = |app: &mut App, v: f32| {
        app.world
            .resource_mut::<Input>()
            .set_gamepad_axis(pad, GamepadAxis::RightStickX, v)
    };
    input(&mut app, 1.0);
    app.run_frames(30, DT);
    input(&mut app, 0.0);
    app.run_frames(30, DT); // the camera catches up
    let expected = Quat::from_rotation_y(-1.25) * Vec3::NEG_Z;
    assert!(forward(&app.world).dot(expected) > 0.99);
    assert!(walk_direction(&mut app).dot(expected) > 0.999);

    // Mouse motion turns the view only while the cursor is locked.
    app.world
        .resource_mut::<Input>()
        .add_mouse_motion(Vec2::new(-200.0, 0.0));
    app.run_frames(30, DT);
    assert!(forward(&app.world).dot(expected) > 0.99, "free cursor");
    app.world
        .resource_mut::<Input>()
        .press_button(MouseButton::Left);
    app.run_frames(1, DT);
    app.world
        .resource_mut::<Input>()
        .release_button(MouseButton::Left);
    assert_eq!(
        app.world.resource::<CursorOptions>().grab,
        CursorGrab::Locked
    );
    // 312.5 units at 0.004 rad each turn the view back by 1.25 rad.
    app.world
        .resource_mut::<Input>()
        .add_mouse_motion(Vec2::new(-312.5, 0.0));
    app.run_frames(30, DT);
    assert!(forward(&app.world).dot(Vec3::NEG_Z) > 0.99);
    assert!(walk_direction(&mut app).dot(Vec3::NEG_Z) > 0.999);
    app.world.resource_mut::<Input>().press_key(KeyCode::Escape);
    app.run_frames(1, DT);
    assert_eq!(app.world.resource::<CursorOptions>().grab, CursorGrab::None);
}

#[test]
fn a_falling_player_lands_on_the_floor() {
    let mut app = app("arena");
    app.run_frames(5, DT);
    teleport(&mut app.world, Vec3::new(2.0, 6.0, 2.0));
    app.run_frames(1, DT);
    assert!(!state(&app.world).grounded);
    app.run_frames(90, DT);
    let end = pos(&app.world);
    assert!(close(end.y, STAND, 0.01), "{end}");
    assert!(close(end.x, 2.0, 1e-4) && close(end.z, 2.0, 1e-4), "{end}");
    let s = state(&app.world);
    assert!(s.grounded && s.velocity == Vec3::ZERO, "{s:?}");
}

#[test]
fn the_player_walks_up_the_ramp_onto_the_ledge() {
    let mut app = app("arena2");
    app.run_frames(5, DT);
    // arena2's camera looks down +X, so W walks +X: up the 15 degree ramp
    // (x 3.5..9.47) onto the ledge (top 1.6 m, x 9.47..13.47).
    teleport(&mut app.world, Vec3::new(1.5, STAND, -4.0));
    app.run_frames(30, DT);
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    app.run_frames(100, DT);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    app.run_frames(5, DT);
    let end = pos(&app.world);
    assert!(end.x > 9.8 && end.x < 13.0, "{end}");
    assert!(close(end.y, 1.6 + STAND, 0.02), "{end}");
    assert!(close(end.z, -4.0, 0.05), "{end}");
    assert!(state(&app.world).grounded);
}

/// Collect every gem of the current arena by standing on each in turn.
fn collect_all(app: &mut App) {
    for at in gem_positions(&app.world) {
        teleport(&mut app.world, at);
        app.run_frames(3, DT);
    }
    app.run_frames(3, DT);
}

#[test]
fn collecting_every_gem_switches_arenas_and_keeps_the_score() {
    let mut app = app("arena");
    app.run_frames(3, DT);
    assert_eq!(scene_file(&app.world), "arena.scene.json");
    assert_eq!(gem_positions(&app.world).len(), 6);

    collect_all(&mut app);
    let score = *app.world.resource::<Score>();
    assert_eq!(
        score,
        Score {
            points: 75,
            collected: 6,
            arenas_completed: 1
        }
    );
    assert_eq!(scene_file(&app.world), "arena2.scene.json");
    assert_eq!(gem_positions(&app.world).len(), 7);
    let start = pos(&app.world);
    assert!(
        close(start.x, -9.0, 0.01) && close(start.z, 5.0, 0.01),
        "{start}"
    );
    assert!(app.world.find_by_name("Monolith").is_some());

    collect_all(&mut app);
    let score = *app.world.resource::<Score>();
    assert_eq!(
        score,
        Score {
            points: 160,
            collected: 13,
            arenas_completed: 2
        }
    );
    assert_eq!(scene_file(&app.world), "arena.scene.json", "loops back");
    assert_eq!(gem_positions(&app.world).len(), 6);
}

#[test]
fn the_game_plays_and_switches_arenas_in_an_editor_play_session() {
    let options = box_editor::EditorOptions {
        project: Some(authoring::project_dir()),
        play: true,
        ..Default::default()
    };
    let mut editor = box_editor::build_game_editor(
        &options,
        box_editor::log::LogBuffer::default(),
        authoring::game(),
    )
    .unwrap();
    let state = &mut editor.state;
    assert!(state.is_playing());
    let edit_gems = 6;
    for _ in 0..3 {
        state.update_play(DT).unwrap();
    }
    for at in gem_positions(&state.world) {
        teleport(&mut state.world, at);
        for _ in 0..3 {
            state.update_play(DT).unwrap();
        }
    }
    for _ in 0..3 {
        state.update_play(DT).unwrap();
    }
    assert_eq!(state.world.resource::<Score>().arenas_completed, 1);
    assert_eq!(scene_file(&state.world), "arena2.scene.json");
    assert_eq!(gem_positions(&state.world).len(), 7);

    state.stop_play();
    assert!(!state.is_playing());
    assert_eq!(gem_positions(&state.world).len(), edit_gems);
    assert!(state.world.get_resource::<Score>().is_none());
}
