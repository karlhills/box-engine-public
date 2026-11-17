//! A scripted playthrough of the M8 game, `examples/cinder_keep`, shared by
//! the player test (`box_engine/tests/m8_game.rs`) and the editor Play test
//! (`box_editor/tests/m8_cinder_keep.rs`). It plays with gamepad input only
//! (the left stick steered toward waypoints relative to the camera, South,
//! West and Start), as a person would: the dialogue, a checkpoint, pushing
//! crates, the rockfall, a death in the lava gap and the reload, the jump,
//! the second checkpoint, the full wave, the gate, the scene transition,
//! the pause menu with a restart from the checkpoint, and the throne.
//!
//! The host runs frames of 1/60 s and may capture images (`Host::capture`).

#![allow(dead_code)]

use box_engine::box_script::ScriptRuntime;
use box_engine::box_script::mlua::FromLua;
use box_engine::box_ui::Ui;
use box_engine::prelude::*;

pub const PAD: GamepadId = GamepadId(0);

/// What drives the game: a player app or an editor Play session.
pub trait Host {
    fn world(&mut self) -> &mut World;
    /// One frame of 1/60 s (assets updated first).
    fn step(&mut self);
    /// Capture the current frame as `name` (a PNG name); hosts without a
    /// renderer do nothing.
    fn capture(&mut self, _name: &str) {}
}

/// Numbers from a playthrough.
#[derive(Debug, Default)]
pub struct Report {
    pub frames: usize,
    pub kills: i64,
    pub score: i64,
    pub health: f64,
    pub deaths: usize,
    pub wave_frames: usize,
    pub embers_fired: i64,
    pub max_enemies: usize,
}

pub fn frames(h: &mut impl Host, n: usize) {
    for _ in 0..n {
        h.step();
    }
}

pub fn find(world: &World, name: &str) -> Option<Entity> {
    world.find_by_name(name)
}

pub fn player(world: &World) -> Entity {
    find(world, "Player").expect("a Player entity")
}

pub fn pos(world: &World) -> Vec3 {
    world.get::<Transform>(player(world)).unwrap().translation
}

/// A field of `entity`'s script instance (`self.<key>`), if it runs one.
pub fn field<T: FromLua>(world: &World, entity: Entity, key: &str) -> Option<T> {
    world
        .get_resource::<ScriptRuntime>()?
        .instance(entity)?
        .get::<Option<T>>(key)
        .ok()
        .flatten()
}

pub fn game(world: &World) -> Entity {
    let all: Vec<Entity> = world
        .query::<(Entity, &Name)>()
        .iter()
        .filter(|(_, n)| n.as_str() == "Game")
        .map(|(e, _)| e)
        .collect();
    assert_eq!(all.len(), 1, "exactly one game entity");
    all[0]
}

pub fn game_number(world: &World, key: &str) -> f64 {
    field::<f64>(world, game(world), key).unwrap_or_else(|| panic!("game.{key}"))
}

pub fn ui_text(world: &World, id: &str) -> String {
    let ui = world.resource::<Ui>();
    let n = ui
        .find(None, id)
        .unwrap_or_else(|| panic!("no UI node `{id}`"));
    ui.text(n).unwrap_or_default()
}

pub fn ui_shown(world: &World, id: &str) -> bool {
    let ui = world.resource::<Ui>();
    ui.find(None, id).is_some_and(|n| ui.is_shown(n))
}

pub fn focused(world: &World) -> Option<String> {
    let ui = world.resource::<Ui>();
    ui.focused().and_then(|n| ui.id_of(n).map(str::to_owned))
}

pub fn scene_file(world: &World) -> String {
    let scenes = world.resource::<Scenes>();
    let main = scenes.main().expect("a main scene");
    scenes
        .path(main)
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn script_errors(world: &World) -> (u64, usize) {
    let s = world.resource::<ScriptRuntime>().stats();
    (s.errors, s.disabled)
}

fn set_stick(h: &mut impl Host, v: Vec2) {
    let input = h.world().resource_mut::<Input>();
    input.set_gamepad_axis(PAD, GamepadAxis::LeftStickX, v.x);
    input.set_gamepad_axis(PAD, GamepadAxis::LeftStickY, v.y);
}

pub fn release_stick(h: &mut impl Host) {
    set_stick(h, Vec2::ZERO);
}

/// The camera's orbit yaw (camera.luau's `self.yaw`).
fn camera_yaw(world: &World) -> f32 {
    let camera = find(world, "Camera").expect("a camera");
    field::<f64>(world, camera, "yaw").unwrap_or(0.0) as f32
}

/// The left-stick value that moves along world direction `dir` (xz).
fn stick_for(world: &World, dir: Vec3) -> Vec2 {
    let yaw = camera_yaw(world);
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let d = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
    Vec2::new(d.dot(right), d.dot(forward))
}

pub fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

/// Steers toward `target` (xz) at full stick until within `tolerance`, or
/// panics after `max_frames`. The stick is released at the end.
pub fn walk_to(h: &mut impl Host, target: Vec3, tolerance: f32, max_frames: usize) {
    walk_until(h, target, tolerance, max_frames, |_| false);
}

/// [`walk_to`] that also stops (successfully) when `stop` holds.
pub fn walk_until(
    h: &mut impl Host,
    target: Vec3,
    tolerance: f32,
    max_frames: usize,
    stop: impl Fn(&World) -> bool,
) {
    for _ in 0..max_frames {
        let at = pos(h.world());
        if flat_distance(at, target) <= tolerance || stop(h.world()) {
            release_stick(h);
            return;
        }
        let stick = stick_for(h.world(), target - at);
        set_stick(h, stick);
        h.step();
    }
    release_stick(h);
    panic!(
        "did not reach {target} in {max_frames} frames (at {})",
        pos(h.world())
    );
}

pub fn press(h: &mut impl Host, b: GamepadButton) {
    h.world()
        .resource_mut::<Input>()
        .press_gamepad_button(PAD, b);
}

pub fn release(h: &mut impl Host, b: GamepadButton) {
    h.world()
        .resource_mut::<Input>()
        .release_gamepad_button(PAD, b);
}

pub fn tap(h: &mut impl Host, b: GamepadButton) {
    press(h, b);
    h.step();
    release(h, b);
    h.step();
}

/// Runs frames until `until` holds, at most `max` (then panics with `what`).
pub fn wait_for(h: &mut impl Host, what: &str, max: usize, until: impl Fn(&World) -> bool) {
    for _ in 0..max {
        if until(h.world()) {
            return;
        }
        h.step();
    }
    panic!("timed out waiting for {what}");
}

fn enemies(world: &World) -> Vec<(Vec3, bool)> {
    world
        .query::<(&Name, &Transform)>()
        .iter()
        .filter(|(n, _)| n.as_str() == "Cinderling" || n.as_str() == "Spitter")
        .map(|(n, t)| (t.translation, n.as_str() == "Spitter"))
        .collect()
}

fn wave_state(world: &World) -> String {
    let wave = find(world, "Wave").expect("the wave");
    field::<String>(world, wave, "state").unwrap_or_default()
}

/// The whole game, start to finish. Returns its numbers.
pub fn play_through(h: &mut impl Host) -> Report {
    let mut report = Report::default();

    // ---- the fields: start ----------------------------------------------
    wait_for(h, "the game's HUD", 600, |w| {
        w.get_resource::<Ui>()
            .is_some_and(|ui| ui.find(None, "health").is_some())
            && field::<f64>(w, game(w), "health").is_some()
    });
    frames(h, 30);
    let w = h.world();
    assert_eq!(scene_file(w), "outdoor.scene.json");
    assert_eq!(ui_text(w, "health"), "Health 100");
    assert_eq!(ui_text(w, "score"), "Score 0");
    assert!(ui_shown(w, "area"), "the area title");
    assert_eq!(ui_text(w, "area"), "The Ember Fields");
    // The imported level holds the hero up: standing on the meadow's mesh.
    let start = pos(w);
    assert!((start.y - 0.92).abs() < 0.02, "standing at {start}");
    h.capture("outdoor_start.png");

    // ---- the warden's dialogue -----------------------------------------------
    walk_until(h, Vec3::new(0.0, 0.0, 2.0), 0.3, 240, |w| {
        ui_shown(w, "dialogue")
    });
    wait_for(h, "the dialogue", 60, |w| ui_shown(w, "dialogue"));
    let w = h.world();
    assert_eq!(ui_text(w, "dialogue_speaker"), "Old Warden");
    assert!(
        ui_text(w, "dialogue_line").starts_with("So you came after all"),
        "{}",
        ui_text(w, "dialogue_line")
    );
    assert_eq!(focused(w).as_deref(), Some("dialogue_next"));
    // The dialogue captures gameplay input: the stick does not move the hero.
    let at = pos(w);
    set_stick(h, Vec2::new(0.0, 1.0));
    frames(h, 20);
    release_stick(h);
    assert!(
        flat_distance(pos(h.world()), at) < 0.05,
        "moved while talking: {at} -> {}",
        pos(h.world())
    );
    tap(h, GamepadButton::South);
    frames(h, 2);
    assert!(ui_text(h.world(), "dialogue_line").starts_with("Cinderlings swarm"));
    h.capture("dialogue.png");
    for _ in 0..3 {
        tap(h, GamepadButton::South);
        frames(h, 2);
    }
    assert!(!ui_shown(h.world(), "dialogue"), "the dialogue closed");
    frames(h, 5);
    // The last confirm did not reach gameplay as a jump.
    let w = h.world();
    assert_eq!(field::<f64>(w, player(w), "jumps"), Some(0.0));

    // ---- checkpoint 1 ---------------------------------------------------------
    let reached = |w: &World| {
        let c = find(w, "Checkpoint Meadow").unwrap();
        field::<bool>(w, c, "reached") == Some(true)
    };
    walk_until(h, Vec3::new(0.0, 0.0, -1.0), 0.3, 240, reached);
    // The game saves when it handles the signal (the next frame).
    h.step();
    let checkpoint_at = pos(h.world());
    wait_for(h, "the checkpoint toast", 30, |w| {
        ui_shown(w, "toast") && ui_text(w, "toast") == "Checkpoint reached"
    });
    frames(h, 3);
    h.capture("checkpoint.png");

    // ---- crates: push through them ---------------------------------------------
    let crate_at = |w: &World| {
        w.get::<Transform>(find(w, "Crate 2").unwrap())
            .unwrap()
            .translation
    };
    let crate_before = crate_at(h.world());
    walk_to(h, Vec3::new(-0.6, 0.0, -10.0), 0.3, 400);
    let crate_after = crate_at(h.world());
    assert!(
        (crate_after - crate_before).length() > 0.3,
        "the hero pushed a crate: {crate_before} -> {crate_after}"
    );

    // ---- the ramp and the rockfall ---------------------------------------------
    walk_to(h, Vec3::new(0.0, 0.0, -13.0), 0.4, 300);
    walk_to(h, Vec3::new(0.0, 0.0, -21.5), 0.3, 300);
    assert!(
        (pos(h.world()).y - 2.92).abs() < 0.05,
        "up the ramp: {}",
        pos(h.world())
    );
    let boulder_at = |w: &World| {
        w.get::<Transform>(find(w, "Boulder 1").unwrap())
            .unwrap()
            .translation
    };
    let boulder_before = boulder_at(h.world());
    walk_to(h, Vec3::new(0.0, 0.0, -24.5), 0.3, 120);
    frames(h, 60);
    let boulder_after = boulder_at(h.world());
    assert!(
        (boulder_after - boulder_before).length() > 1.0,
        "the boulders rolled: {boulder_before} -> {boulder_after}"
    );

    // ---- death in the lava gap, back to the checkpoint -----------------------------
    let game_before = game(h.world());
    walk_to(h, Vec3::new(0.0, 0.0, -29.5), 0.3, 120);
    wait_for(h, "the fall", 120, |w| pos(w).y < -3.0);
    report.deaths += 1;
    // Fade out, load the checkpoint: the game entity is replaced.
    wait_for(h, "the checkpoint to load", 240, |w| {
        w.find_by_name("Game").is_some() && game(w) != game_before
    });
    frames(h, 10);
    let w = h.world();
    let back = pos(w);
    assert!(
        flat_distance(back, checkpoint_at) < 1.0,
        "reloaded at the checkpoint: {back} (saved at {checkpoint_at})"
    );
    assert_eq!(ui_text(w, "health"), "Health 100");
    // Entity state: the crate is back where it was at the checkpoint.
    assert!(
        (crate_at(w) - crate_before).length() < 0.05,
        "crate restored: {}",
        crate_at(w)
    );
    // Script state: the warden remembers it already spoke.
    let warden = find(w, "Warden").unwrap();
    assert_eq!(field::<bool>(w, warden, "talked"), Some(true));
    assert!(!ui_shown(w, "dialogue"));
    frames(h, 60);

    // ---- again, and this time jump the gap -----------------------------------------
    walk_to(h, Vec3::new(1.2, 0.0, -10.0), 0.3, 400);
    walk_to(h, Vec3::new(0.0, 0.0, -13.0), 0.4, 300);
    walk_to(h, Vec3::new(0.0, 0.0, -23.0), 0.3, 300);
    // A run-up, then South at the edge.
    let mut jumped = false;
    for _ in 0..120 {
        let at = pos(h.world());
        let stick = stick_for(h.world(), Vec3::NEG_Z);
        set_stick(h, stick);
        if !jumped && at.z < -27.4 {
            press(h, GamepadButton::South);
            jumped = true;
        } else if jumped {
            release(h, GamepadButton::South);
        }
        h.step();
        if at.z < -32.0 {
            break;
        }
    }
    release_stick(h);
    frames(h, 10);
    let landed = pos(h.world());
    assert!(
        landed.z < -31.0 && (landed.y - 2.92).abs() < 0.05,
        "across the gap: {landed}"
    );

    // ---- checkpoint 2 ------------------------------------------------------
    walk_to(h, Vec3::new(0.0, 0.0, -33.5), 0.3, 120);
    wait_for(h, "the courtyard checkpoint", 30, |w| {
        let c = find(w, "Checkpoint Courtyard").unwrap();
        field::<bool>(w, c, "reached") == Some(true)
    });
    frames(h, 5);

    // ---- the wave ----------------------------------------------------------
    walk_to(h, Vec3::new(0.0, 0.0, -40.0), 0.3, 200);
    wait_for(h, "the wave", 30, |w| wave_state(w) == "fighting");
    frames(h, 2);
    report.max_enemies = enemies(h.world()).len();
    assert!(ui_shown(h.world(), "enemies"));
    let mut captured_fight = false;
    let mut since_swing = 100;
    for frame in 0..60 * 120 {
        if wave_state(h.world()) == "cleared" {
            break;
        }
        report.wave_frames += 1;
        let at = pos(h.world());
        let foes = enemies(h.world());
        let nearest = foes
            .iter()
            .map(|(p, s)| (flat_distance(*p, at), *p, *s))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        // Swing whenever something is near (or every half second).
        since_swing += 1;
        let near = nearest.is_some_and(|(d, _, _)| d < 2.6);
        if since_swing >= 26 && (near || since_swing >= 30) {
            press(h, GamepadButton::West);
            since_swing = 0;
        } else {
            release(h, GamepadButton::West);
        }
        // Walk to the nearest one unless a crowd is coming (spitters hold
        // back); otherwise hold the ground. After a death (the checkpoint
        // reloads with the wave waiting), walk back in.
        let crowd = foes
            .iter()
            .filter(|(p, _)| flat_distance(*p, at) < 5.0)
            .count();
        match nearest {
            Some((d, p, _)) if crowd < 6 && d > 2.0 => {
                let stick = stick_for(h.world(), p - at);
                set_stick(h, stick);
            }
            None if wave_state(h.world()) == "waiting" => {
                let stick = stick_for(h.world(), Vec3::new(0.0, 0.0, -40.0) - at);
                set_stick(h, stick);
            }
            _ => release_stick(h),
        }
        h.step();
        if frame % 60 == 0 {
            let w = h.world();
            eprintln!(
                "wave frame {frame}: health {:?}, enemies {}, crowd {crowd}, state {}",
                field::<f64>(w, game(w), "health"),
                enemies(w).len(),
                wave_state(w)
            );
        }
        if !captured_fight && frame > 150 && crowd >= 8 {
            h.capture("wave_fight.png");
            captured_fight = true;
        }
    }
    release_stick(h);
    release(h, GamepadButton::West);
    assert_eq!(wave_state(h.world()), "cleared", "the wave was defeated");
    let w = h.world();
    let wave = find(w, "Wave").unwrap();
    report.kills = field::<f64>(w, wave, "kills").unwrap_or(0.0) as i64;
    report.embers_fired = field::<f64>(w, wave, "fired").unwrap_or(0.0) as i64;
    assert!(game_number(w, "health") > 0.0, "survived the wave");
    assert!(enemies(w).is_empty());

    // ---- the gate swings open -----------------------------------------------
    wait_for(h, "the toast", 30, |w| {
        ui_text(w, "toast") == "The gate is open"
    });
    assert!(!ui_shown(h.world(), "enemies"));
    frames(h, 90);
    let gate = find(h.world(), "Gate").unwrap();
    let angle = h
        .world()
        .resource::<box_engine::box_physics::Dynamics>()
        .joint_position(gate)
        .unwrap_or(0.0);
    let w = h.world();
    assert!(
        angle > 1.0,
        "the gate is open: {angle} rad; {:?} {:?} {:?}",
        w.get::<Transform>(gate).map(|t| *t),
        w.get::<RigidBody>(gate).map(|r| r.kind),
        w.get::<Velocity>(gate).map(|v| *v),
    );

    // ---- into the keep -------------------------------------------------------------
    let score = game_number(h.world(), "score");
    let health = game_number(h.world(), "health");
    let game_entity = game(h.world());
    walk_to(h, Vec3::new(0.3, 0.0, -50.0), 0.4, 600);
    walk_to(h, Vec3::new(0.3, 0.0, -56.0), 0.3, 300);
    walk_to(h, Vec3::new(0.3, 0.0, -60.0), 0.3, 200);
    wait_for(h, "the keep", 240, |w| {
        scene_file(w) == "keep.scene.json" && find(w, "Throne Room").is_some()
    });
    wait_for(h, "the area title", 60, |w| {
        ui_shown(w, "area") && ui_text(w, "area") == "Cinder Keep"
    });
    frames(h, 40);
    let w = h.world();
    assert_eq!(game(w), game_entity, "the game persists");
    assert_eq!(game_number(w, "score"), score, "score kept");
    assert_eq!(game_number(w, "health"), health, "health kept");
    assert!(find(w, "Wave").is_none());
    h.capture("interior.png");

    // ---- pause menu: restart from the checkpoint (saved on arrival) ---------------
    let arrival = pos(h.world());
    walk_to(h, Vec3::new(0.0, 0.0, -9.0), 0.3, 300);
    tap(h, GamepadButton::Start);
    let w = h.world();
    assert!(ui_shown(w, "pause_menu"));
    assert!(w.resource::<Time>().paused);
    assert_eq!(focused(w).as_deref(), Some("resume"));
    assert_eq!(ui_text(w, "restart"), "Restart from checkpoint");
    h.capture("pause_menu.png");
    tap(h, GamepadButton::DPadDown);
    assert_eq!(focused(h.world()).as_deref(), Some("restart"));
    let game_before = game(h.world());
    tap(h, GamepadButton::South);
    wait_for(h, "the restart", 240, |w| {
        w.find_by_name("Game").is_some() && game(w) != game_before
    });
    frames(h, 10);
    let w = h.world();
    assert!(!w.resource::<Time>().paused);
    assert!(
        flat_distance(pos(w), arrival) < 0.5,
        "restarted at the keep's entrance: {}",
        pos(w)
    );
    assert_eq!(game_number(w, "score"), score);
    assert_eq!(scene_file(w), "keep.scene.json");
    frames(h, 50);

    // ---- the throne ----------------------------------------------------------
    walk_to(h, Vec3::new(0.0, 0.0, -14.0), 0.3, 300);
    walk_until(h, Vec3::new(0.0, 0.0, -24.0), 0.3, 300, |w| {
        ui_shown(w, "win")
    });
    wait_for(h, "victory", 60, |w| ui_shown(w, "win"));
    frames(h, 20);
    let w = h.world();
    assert!((pos(w).y - 2.92).abs() < 0.05, "up the stairs: {}", pos(w));
    assert_eq!(ui_text(w, "win_score"), format!("Final score {score}"));
    assert_eq!(focused(w).as_deref(), Some("again"));
    h.capture("win.png");
    report.score = score as i64;
    report.health = health;
    let (errors, disabled) = script_errors(h.world());
    assert_eq!((errors, disabled), (0, 0), "script errors");
    report
}
