//! The Rust collector (this crate) and the Luau collector
//! (`examples/collector_luau`, no game code) fed the same input frame by
//! frame: the player moves the same way in both, within float noise. The
//! Rust game reads input in a FixedUpdate system ordered before
//! `move_characters`; the Luau game in `on_fixed_update`, which runs there
//! too.

use std::path::PathBuf;
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use collector::authoring;

const DT: f32 = 1.0 / 60.0;
const PAD: GamepadId = GamepadId(0);

fn rust_app() -> App {
    let options = PlayerOptions {
        project: Some(authoring::project_dir()),
        scene: Some("arena".into()),
        watch: false,
        ..Default::default()
    };
    build_app(&authoring::game(), &options).unwrap().0
}

fn luau_app() -> App {
    let options = PlayerOptions {
        project: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../collector_luau")),
        scene: Some("arena".into()),
        watch: false,
        ..Default::default()
    };
    build_app(&Game::new(), &options).unwrap().0
}

/// One 1/60 s frame, imports finished first (the Luau scripts load
/// through the asset server).
fn frame(app: &mut App) {
    if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
        s.update_until_idle(&mut app.world, Duration::from_millis(50));
    }
    app.run_frames(1, DT);
}

fn pos(world: &World) -> Vec3 {
    let player = world.find_by_name("Player").expect("a player");
    world.get::<Transform>(player).unwrap().translation
}

/// What a frame does to the input before it runs.
type Script = Vec<(usize, fn(&mut Input))>;

/// Walk, stop, strafe with the stick, jump, walk diagonally while jumping,
/// stop. Frame numbers count from the end of the 10-frame settle.
fn inputs() -> Script {
    vec![
        (0, |i| i.press_key(KeyCode::W)),
        (60, |i| i.release_key(KeyCode::W)),
        (70, |i| {
            i.set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 1.0)
        }),
        (100, |i| {
            i.set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 0.0)
        }),
        (110, |i| i.press_gamepad_button(PAD, GamepadButton::South)),
        (111, |i| i.release_gamepad_button(PAD, GamepadButton::South)),
        (180, |i| i.press_key(KeyCode::A)),
        (181, |i| i.press_key(KeyCode::S)),
        (190, |i| i.press_key(KeyCode::Space)),
        (191, |i| i.release_key(KeyCode::Space)),
        (230, |i| i.release_key(KeyCode::A)),
        (231, |i| i.release_key(KeyCode::S)),
    ]
}

#[test]
fn the_rust_and_luau_collectors_move_the_player_alike() {
    let (mut rust, mut luau) = (rust_app(), luau_app());
    for _ in 0..10 {
        frame(&mut rust);
        frame(&mut luau);
    }
    let (r0, l0) = (pos(&rust.world), pos(&luau.world));
    assert!((r0 - l0).length() < 1e-5, "start: rust {r0}, luau {l0}");

    let script = inputs();
    let mut worst = (0.0f32, 0usize);
    let (mut path, mut last) = (0.0, r0);
    for n in 0..260 {
        for (_, apply) in script.iter().filter(|(at, _)| *at == n) {
            apply(rust.world.resource_mut::<Input>());
            apply(luau.world.resource_mut::<Input>());
        }
        frame(&mut rust);
        frame(&mut luau);
        let (r, l) = (pos(&rust.world), pos(&luau.world));
        path += (r - last).length();
        last = r;
        let d = (r - l).length();
        if d > worst.0 {
            worst = (d, n);
        }
    }
    let (r, l) = (pos(&rust.world), pos(&luau.world));
    assert!(path > 15.0, "the player walked {path} m");
    assert!(
        worst.0 < 1e-5,
        "worst difference {} m at frame {} (end: rust {r}, luau {l})",
        worst.0,
        worst.1
    );
    let errors = luau
        .world
        .resource::<box_engine::box_script::ScriptRuntime>()
        .stats()
        .errors;
    assert_eq!(errors, 0);
}
