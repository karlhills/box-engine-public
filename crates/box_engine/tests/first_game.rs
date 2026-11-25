//! `examples/first_game` (docs/BEHAVIOURS.md, B3): a complete small game
//! made only of presets and `set` commands, no Luau.
//!
//! - `its_rules_hold_and_everything_is_reachable`: no rule warnings
//!   (`behaviours::check`), the enemy can reach the player, and every coin
//!   on the ground and the ledge can be walked to.
//! - `it_plays_through_to_the_win`: the goal waits for every coin; with all
//!   twelve collected (the player moved from coin to coin) it wins.
//!   Capture `target/captures/first_game/won.png`.

use std::path::PathBuf;

use box_engine::behaviours::{GamePhase, GameState};
use box_engine::interact::group_progress;
use box_engine::nav_glue::{nav_reachable, settle_navigation};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_render::RenderError;

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/first_game")
}

fn app() -> App {
    let options = PlayerOptions {
        project: Some(project()),
        watch: false,
        ..Default::default()
    };
    let (mut app, _) = build_app(&Game::new(), &options).unwrap();
    app.world.insert_resource(WindowSize {
        width: 960,
        height: 540,
        scale_factor_milli: 1000,
    });
    app.run_frames(3, 1.0 / 60.0);
    app
}

fn pos(app: &App, name: &str) -> Vec3 {
    let e = app
        .world
        .find_by_name(name)
        .unwrap_or_else(|| panic!("no {name}"));
    app.world
        .get::<GlobalTransform>(e)
        .unwrap()
        .0
        .translation
        .into()
}

#[test]
fn its_rules_hold_and_everything_is_reachable() {
    let mut app = app();
    let problems = box_engine::behaviours::check::rule_problems(&app.world);
    assert!(problems.is_empty(), "{problems:?}");
    settle_navigation(&mut app.world);
    let problems = box_engine::behaviours::check::reachability_problems(&app.world);
    assert!(problems.is_empty(), "{problems:?}");
    let start = pos(&app, "Player") - Vec3::Y * 0.92;
    for i in 1..=10 {
        let coin = pos(&app, &format!("Coin {i}"));
        // The coin floats 0.8 m above where you stand to get it.
        let feet = coin - Vec3::Y * 0.8;
        assert!(
            nav_reachable(&app.world, start, feet),
            "Coin {i} at {coin} cannot be walked to"
        );
    }
    assert!(nav_reachable(&app.world, start, pos(&app, "Goal")));
}

#[test]
fn it_plays_through_to_the_win() {
    let mut app = app();
    assert_eq!(group_progress(&app.world, "pickups"), (0, 12));
    let player = app.world.find_by_name("Player").unwrap();
    let go = |app: &mut App, to: Vec3| {
        box_engine::behaviours::teleport(&mut app.world, player, to);
        app.run_frames(6, 1.0 / 60.0);
    };
    // The goal does nothing before the coins are in.
    let goal = pos(&app, "Goal");
    go(&mut app, goal + Vec3::Y * 0.95);
    assert_eq!(app.world.resource::<GameState>().phase, GamePhase::Playing);
    for i in 1..=12 {
        let coin = pos(&app, &format!("Coin {i}"));
        go(&mut app, coin + Vec3::Y * 0.15);
    }
    assert_eq!(group_progress(&app.world, "pickups"), (12, 12));
    go(&mut app, Vec3::new(0.0, 0.95, 20.0));
    go(&mut app, goal + Vec3::Y * 0.95);
    assert_eq!(app.world.resource::<GameState>().phase, GamePhase::Won);
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_none() {
        match Renderer::new_headless(960, 540) {
            Ok(mut r) => {
                app.run_frames(30, 1.0 / 60.0);
                let img = r.capture(&mut app.world).unwrap();
                let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/captures/first_game");
                std::fs::create_dir_all(&dir).unwrap();
                img.save(dir.join("won.png")).unwrap();
                assert_eq!(r.stats().validation_errors, 0);
            }
            Err(RenderError::NoAdapter) => {}
            Err(e) => panic!("renderer: {e}"),
        }
    }
}
