//! Behaviours (docs/BEHAVIOURS.md): a small game built only from presets and
//! rules, played headlessly with injected actions.
//!
//! - `presets_place_as_one_undo_step`: every built-in preset places with
//!   `add_preset`, unique names, linked uids; one undo removes it all.
//! - `preset_game_plays_through`: the player walks to pickups (score and
//!   group progress on the HUD), rides a moving platform, is sent back by a
//!   hazard to the start and later to a checkpoint, pauses and resumes, wins
//!   once every pickup is collected, and Restart puts everything back.
//!   Captures in `target/captures/behaviours/` (`hud.png`, `pause.png`,
//!   `win.png`).
//! - `lives_and_time_limit_lose`: falling with one life left loses; a time
//!   limit loses.
//! - `motion_survives_save_and_load`: a mover's origin and progress come
//!   back from a save game.

use std::path::PathBuf;

use box_engine::behaviours::{
    GamePhase, GameRules, GameState, HudSettings, MotionState, PlayerController,
};
use box_engine::box_core::action::InjectedInput;
use box_engine::box_core::edit::{Command, EditStack, apply_commands};
use box_engine::interact::{Condition, GameVars, group_progress};
use box_engine::prelude::*;
use box_engine::{app_with_world, shared_engine_registry};
use box_render::RenderError;
use box_ui::Ui;

const DT: f32 = 1.0 / 60.0;

fn captures() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/behaviours");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn commands(json: serde_json::Value) -> Vec<Command> {
    serde_json::from_value(json).unwrap()
}

/// Ground, the player and the presets of a tiny level, plus rules.
fn level(world: &mut World, rules: serde_json::Value) {
    let mut stack = EditStack::default();
    let cmds = commands(serde_json::json!([
        { "op": "spawn", "name": "Ground", "components": {
            "Transform": { "translation": [0.0, -0.5, 0.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [40.0, 1.0, 40.0] } } },
            "StandardMaterial": { "base_color": [0.35, 0.5, 0.3, 1.0] },
            "Collider": { "shape": "Primitive" } } },
        { "op": "spawn", "name": "Sun", "components": {
            "Transform": { "rotation": [-0.42, 0.27, 0.13, 0.86] },
            "DirectionalLight": { "intensity": 3.0 } } },
        // The starter scene's own camera, which the player's follow camera
        // takes over from in Play.
        { "op": "spawn", "name": "Camera", "components": {
            "Transform": { "translation": [0.0, 10.0, 10.0] },
            "Camera": { "active": true } } },
        { "op": "add_preset", "preset": "player", "at": [0.0, 0.0, 0.0] },
        { "op": "add_preset", "preset": "pickup", "at": [0.0, 0.0, -4.0] },
        { "op": "add_preset", "preset": "pickup", "at": [3.0, 0.0, -4.0] },
        { "op": "add_preset", "preset": "hazard", "at": [-4.0, 0.0, 0.0] },
        { "op": "add_preset", "preset": "checkpoint", "at": [3.0, 0.0, 2.0] },
        { "op": "add_preset", "preset": "moving_platform", "at": [8.0, 0.0, -10.0] },
        { "op": "add_preset", "preset": "goal", "at": [0.0, 0.0, 8.0] },
        { "op": "set_resource", "path": "GameRules", "value": rules },
        { "op": "set_resource", "path": "HudSettings", "value": {} }
    ]));
    apply_commands(world, &mut stack, &cmds, "level").unwrap();
}

struct Play {
    app: App,
    renderer: Option<Renderer>,
}

impl Play {
    fn new(rules: serde_json::Value) -> Self {
        let mut world = World::with_registry(shared_engine_registry());
        level(&mut world, rules);
        let mut app = app_with_world(world);
        app.world.insert_resource(WindowSize {
            width: 960,
            height: 540,
            scale_factor_milli: 1000,
        });
        let renderer = if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
            None
        } else {
            match Renderer::new_headless(960, 540) {
                Ok(r) => Some(r),
                Err(RenderError::NoAdapter) => None,
                Err(e) => panic!("renderer: {e}"),
            }
        };
        let mut play = Self { app, renderer };
        play.steps(30);
        play
    }

    fn world(&mut self) -> &mut World {
        &mut self.app.world
    }

    fn step(&mut self) {
        self.app.run_frames(1, DT);
        self.app.world.resource_mut::<Input>().end_frame();
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn capture(&mut self, name: &str) {
        let Some(r) = &mut self.renderer else { return };
        let img = r.capture(&mut self.app.world).expect("capture");
        img.save(captures().join(name)).unwrap();
        assert_eq!(
            r.stats().validation_errors,
            0,
            "{name}: GPU validation errors"
        );
    }

    fn entity(&mut self, name: &str) -> Entity {
        self.world()
            .find_by_name(name)
            .unwrap_or_else(|| panic!("no entity named {name}"))
    }

    fn pos(&mut self, name: &str) -> Vec3 {
        let e = self.entity(name);
        self.world()
            .get::<GlobalTransform>(e)
            .unwrap()
            .0
            .translation
            .into()
    }

    /// Walks the player toward `target` (xz). The follow camera starts
    /// looking down -Z, so +Y on the stick is world -Z.
    fn walk_to(&mut self, target: Vec3, tolerance: f32, max_frames: usize) {
        for _ in 0..max_frames {
            let at = self.pos("Player");
            let d = Vec2::new(target.x - at.x, target.z - at.z);
            // Won or lost on the way: the player stops.
            if d.length() <= tolerance || self.phase() != GamePhase::Playing {
                self.steps(5);
                return;
            }
            let wish = Vec2::new(d.x, -d.y).normalize() * if d.length() < 1.0 { 0.5 } else { 1.0 };
            self.world()
                .resource_mut::<Actions>()
                .inject("move", InjectedInput::axis(wish, 1));
            self.step();
        }
        panic!("did not reach {target} (at {})", self.pos("Player"));
    }

    /// Walks toward `target` until the hazard's respawn (or a lost game)
    /// happens; panics if it does not within `max_frames`.
    fn walk_into_hazard(&mut self, target: Vec3, max_frames: usize) {
        let respawns = self.world().resource::<GameState>().respawns;
        for _ in 0..max_frames {
            let s = self.world().resource::<GameState>();
            if s.respawns > respawns || s.phase != GamePhase::Playing {
                self.steps(3);
                return;
            }
            let at = self.pos("Player");
            let d = Vec2::new(target.x - at.x, target.z - at.z);
            let wish = Vec2::new(d.x, -d.y).normalize_or_zero();
            self.world()
                .resource_mut::<Actions>()
                .inject("move", InjectedInput::axis(wish, 1));
            self.step();
        }
        panic!("the hazard at {target} did not send the player back");
    }

    fn press(&mut self, action: &str) {
        self.world()
            .resource_mut::<Actions>()
            .inject(action, InjectedInput::press(1));
        self.steps(2);
    }

    fn phase(&mut self) -> GamePhase {
        self.world().resource::<GameState>().phase
    }

    fn score(&mut self) -> f64 {
        self.world().resource::<GameVars>().get("score")
    }

    /// Focuses the button and confirms it, as a gamepad would.
    fn click(&mut self, id: &str) {
        let ui = self.world().resource_mut::<Ui>();
        let node = ui
            .find(None, id)
            .unwrap_or_else(|| panic!("no UI node {id}"));
        ui.focus(node);
        self.world()
            .resource_mut::<Actions>()
            .inject(box_ui::input::UI_CONFIRM, InjectedInput::press(1));
        self.steps(3);
    }

    fn has_ui(&mut self, id: &str) -> bool {
        self.world()
            .get_resource::<Ui>()
            .is_some_and(|ui| ui.find(None, id).is_some_and(|n| ui.is_shown(n)))
    }
}

#[test]
fn presets_place_as_one_undo_step() {
    let mut world = World::with_registry(shared_engine_registry());
    let names: Vec<String> = world.registry().presets().map(|p| p.name.clone()).collect();
    for expected in [
        "player",
        "pickup",
        "hazard",
        "moving_platform",
        "checkpoint",
        "goal",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing preset {expected}: {names:?}"
        );
    }
    let mut stack = EditStack::default();
    for name in &names {
        let cmds = commands(
            serde_json::json!([{ "op": "add_preset", "preset": name, "at": [1.0, 0.0, 2.0] }]),
        );
        apply_commands(&mut world, &mut stack, &cmds, name)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // A second pickup gets a unique name.
    let cmds = commands(serde_json::json!([{ "op": "add_preset", "preset": "pickup" }]));
    apply_commands(&mut world, &mut stack, &cmds, "again").unwrap();
    assert!(world.find_by_name("Pickup 2").is_some());
    // The player's camera link points at the preset's camera.
    let player = world.find_by_name("Player").unwrap();
    let cam_uid = world
        .get::<PlayerController>(player)
        .unwrap()
        .camera
        .unwrap();
    let cam = world.entity_by_uid(cam_uid).unwrap();
    assert_eq!(world.get::<Name>(cam).unwrap().0, "Player Camera");
    // Placed at `at`: the pickup's own height is kept.
    let pickup = world.find_by_name("Pickup").unwrap();
    let t = world.get::<Transform>(pickup).unwrap().translation;
    assert!((t - Vec3::new(1.0, 0.8, 2.0)).length() < 1e-5, "{t}");
    // Children come along under their parent.
    let hazard = world.find_by_name("Hazard").unwrap();
    assert_eq!(world.children(hazard).len(), 4);
    // One undo removes a whole preset.
    let before = world.query::<&Uid>().iter().count();
    stack.undo(&mut world).unwrap();
    assert_eq!(world.query::<&Uid>().iter().count(), before - 1);
    assert!(world.find_by_name("Pickup 2").is_none());
    // An unknown preset names the known ones.
    let cmds = commands(serde_json::json!([{ "op": "add_preset", "preset": "dragon" }]));
    let err = apply_commands(&mut world, &mut stack, &cmds, "bad")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no preset `dragon`") && err.contains("pickup"),
        "{err}"
    );
}

#[test]
fn preset_game_plays_through() {
    let mut play = Play::new(serde_json::json!({
        "win": [{ "GroupDone": { "group": "pickups" } }]
    }));
    assert_eq!(play.phase(), GamePhase::Playing);
    // The follow camera is the one the game is seen through.
    let active: Vec<String> = play
        .world()
        .query::<(&Camera, &Name)>()
        .iter()
        .filter(|(c, _)| c.active)
        .map(|(_, n)| n.0.clone())
        .collect();
    assert_eq!(active, ["Player Camera"]);
    let start = play.pos("Player");
    assert!(
        start.y > 0.5 && start.y < 1.3,
        "player stands on the ground: {start}"
    );

    // Collect one pickup: score, group progress, the pickup is gone.
    play.walk_to(Vec3::new(0.0, 0.0, -4.0), 0.3, 300);
    assert_eq!(play.score(), 1.0);
    let w = play.world();
    assert_eq!(group_progress(w, "pickups"), (1, 2));
    let pickup = play.entity("Pickup");
    assert!(play.world().get::<Visible>(pickup).is_some_and(|v| !v.0));
    play.steps(10);
    play.capture("hud.png");
    let hud = box_engine::behaviours::hud_lines(play.world(), &HudSettings::default(), None);
    assert!(hud.contains(&"Score 1".to_string()), "{hud:?}");
    assert!(hud.contains(&"Pickups 1 / 2".to_string()), "{hud:?}");

    // The spinner turns, the bobber bobs.
    let beacon = play.entity("Beacon");
    let r0 = play.world().get::<Transform>(beacon).unwrap().rotation;
    play.steps(10);
    let r1 = play.world().get::<Transform>(beacon).unwrap().rotation;
    assert!(r0.angle_between(r1) > 0.1, "spinner turned");

    // A hazard sends the player back to where it started.
    play.walk_into_hazard(Vec3::new(-4.0, 0.0, 0.0), 400);
    let back = play.pos("Player");
    assert!(
        Vec2::new(back.x - start.x, back.z - start.z).length() < 1.0,
        "respawned at the start: {back} (start {start})"
    );
    assert_eq!(play.world().resource::<GameState>().respawns, 1);

    // A checkpoint moves the respawn point.
    play.walk_to(Vec3::new(3.0, 0.0, 2.0), 0.5, 400);
    let cp = play
        .world()
        .resource::<GameState>()
        .checkpoint
        .expect("checkpoint set");
    assert!(
        (cp.x - 3.0).abs() < 0.01 && (cp.z - 2.0).abs() < 0.01,
        "{cp}"
    );
    play.walk_into_hazard(Vec3::new(-4.0, 0.0, 0.0), 600);
    let back = play.pos("Player");
    assert!(
        Vec2::new(back.x - 3.0, back.z - 2.0).length() < 1.0,
        "respawned at the checkpoint: {back}"
    );

    // The moving platform moves.
    let p0 = play.pos("Moving Platform");
    play.steps(60);
    let p1 = play.pos("Moving Platform");
    assert!((p1 - p0).length() > 0.5, "platform moved: {p0} -> {p1}");

    // Pause and resume.
    play.press("pause");
    assert_eq!(play.phase(), GamePhase::Paused);
    assert!(play.world().resource::<Time>().paused);
    assert!(play.has_ui("game_menu"));
    play.capture("pause.png");
    play.click("game_resume");
    assert_eq!(play.phase(), GamePhase::Playing);
    assert!(!play.world().resource::<Time>().paused);
    assert!(!play.has_ui("game_menu"));

    // The last pickup wins the game.
    play.walk_to(Vec3::new(3.0, 0.0, -4.0), 0.3, 600);
    play.steps(2);
    assert_eq!(play.phase(), GamePhase::Won);
    assert!(play.has_ui("game_menu"));
    play.capture("win.png");

    // Restart: back to the start, score 0, pickups back.
    play.click("game_restart");
    play.steps(5);
    assert_eq!(play.phase(), GamePhase::Playing);
    assert_eq!(play.score(), 0.0);
    assert_eq!(group_progress(play.world(), "pickups"), (0, 2));
    let pickup = play.entity("Pickup");
    assert!(play.world().get::<Visible>(pickup).is_none_or(|v| v.0));
    let at = play.pos("Player");
    assert!(
        Vec2::new(at.x - start.x, at.z - start.z).length() < 0.5,
        "restarted at the start: {at}"
    );
    assert!(play.world().resource::<GameState>().checkpoint.is_none());
}

#[test]
fn lives_and_time_limit_lose() {
    // One life: the first hazard loses.
    let mut play = Play::new(serde_json::json!({ "lives": 1 }));
    assert_eq!(play.world().resource::<GameState>().lives_left, 1);
    play.walk_into_hazard(Vec3::new(-4.0, 0.0, 0.0), 400);
    assert_eq!(play.phase(), GamePhase::Lost);
    assert!(play.has_ui("game_menu"));

    // A two-second limit (Play::new already ran half a second).
    let mut play = Play::new(serde_json::json!({ "time_limit": 2.0 }));
    play.steps(60);
    assert_eq!(play.phase(), GamePhase::Playing);
    let hud = box_engine::behaviours::hud_lines(
        play.world(),
        &HudSettings::default(),
        Some(&GameRules {
            time_limit: 2.0,
            ..Default::default()
        }),
    );
    assert!(hud.contains(&"Time 0:01".to_string()), "{hud:?}");
    play.steps(40);
    assert_eq!(play.phase(), GamePhase::Lost);

    // A lose condition on a var.
    let mut play = Play::new(serde_json::json!({
        "lose": [{ "AtLeast": { "var": "score", "value": 1.0 } }]
    }));
    play.walk_to(Vec3::new(0.0, 0.0, -4.0), 0.3, 300);
    assert_eq!(play.phase(), GamePhase::Lost);
    // Conditions see the world.
    let w = play.world();
    assert!(
        Condition::AtLeast {
            var: "score".into(),
            value: 1.0
        }
        .holds_in(w)
    );
    let _ = GameRules::default();
}

#[test]
fn motion_survives_save_and_load() {
    let mut play = Play::new(serde_json::json!({}));
    play.steps(90);
    let e = play.entity("Moving Platform");
    let before = *play.world().get::<MotionState>(e).unwrap();
    let pos = play.pos("Moving Platform");
    let save = box_engine::savegame::snapshot(play.world()).unwrap();
    play.steps(60);
    box_engine::savegame::restore(play.world(), &save).unwrap();
    let e = play.entity("Moving Platform");
    let after = *play.world().get::<MotionState>(e).unwrap();
    assert_eq!(after.origin, before.origin);
    assert!((after.travelled - before.travelled).abs() < 1e-4);
    play.steps(1);
    let again = play.pos("Moving Platform");
    assert!((again - pos).length() < 0.2, "{pos} -> {again}");
}

#[test]
fn a_moving_platform_carries_the_player() {
    let mut play = Play::new(serde_json::json!({}));
    // Stand the player on the platform (top at y 0.75 + the capsule's half
    // height).
    let top = play.pos("Moving Platform") + Vec3::new(0.0, 0.25 + 0.95, 0.0);
    let player = play.entity("Player");
    box_engine::behaviours::teleport(play.world(), player, top);
    play.steps(20);
    let p0 = play.pos("Player");
    let m0 = play.pos("Moving Platform");
    play.steps(60);
    let p1 = play.pos("Player");
    let m1 = play.pos("Moving Platform");
    let moved = m1 - m0;
    assert!(moved.length() > 1.0, "platform moved {moved}");
    let carried = p1 - p0;
    assert!(
        (carried.x - moved.x).abs() < 0.3 && (carried.z - moved.z).abs() < 0.3,
        "player moved {carried}, platform {moved}"
    );
    assert!(p1.y > 0.9, "still on top: {p1}");
}
