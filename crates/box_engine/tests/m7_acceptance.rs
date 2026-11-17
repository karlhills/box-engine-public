//! M7 acceptance (docs/ROADMAP.md, "M7 — Scripting and gameplay
//! foundations"), one test per criterion, played headlessly. Most run the
//! Luau collector, `examples/collector_luau`: the M3 sample game as a data
//! project (no Rust) with the procedural skinned humanoid, a HUD, a pause
//! menu and save games.
//!
//! | Criterion | Test |
//! |---|---|
//! | The M3 sample game is rewritten in Luau without touching Rust, with editor completion from generated definitions | `luau_collector_*` here; definitions: `luau_collector_definitions_are_current` |
//! | A script error is reported with file and line and does not crash the editor; editing a script during Play takes effect without restarting Play | `box_editor` tests `scripts` (`scripts_run_in_play_reload_and_report_errors_in_the_console`) and `m7_collector_luau` |
//! | A skinned character walks, runs and jumps under a character controller with blended animation | `skinned_character_walks_runs_and_jumps_with_blended_animation` |
//! | An interior scene is lit by spot and point lights with one shadowed local light and no sky ambient leak | `interior_is_lit_by_local_lights_without_sky_leak` (and the `golden` test's `interior` reference) |
//! | A menu can be navigated by gamepad, and its text comes from a string table | `pause_menu_navigates_by_gamepad_with_string_table_text` |
//! | A saved game restores entity and script state | `saved_game_restores_entities_and_script_state` |
//!
//! Performance (test strategy): 10,000 script callbacks per frame and a
//! batch query over 5,000 entities are measured by
//! `cargo test --release -p box_script --test bench -- --nocapture`
//! (docs/measurements/m7-script.md).
//!
//! Captures go to `target/captures/m7/` unless `BOX_SKIP_GPU_TESTS` is set.

#[path = "../examples/common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_core::animation::{AnimationPlayerState, AnimationStateMachineState};
use box_engine::box_script::ScriptRuntime;
use box_engine::box_ui::{Strings, Ui};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_render::{RenderError, golden};

const DT: f32 = 1.0 / 60.0;
/// Capsule centre height on a floor whose top is at y = 0.
const STAND: f32 = 0.9 + 0.02;
const PAD: GamepadId = GamepadId(0);

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/collector_luau")
}

fn captures() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m7");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fresh save folder per app, so tests do not share slots.
fn save_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "box_m7_saves_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

/// The Luau collector's `scene` in the stock engine game (no game code).
fn app(scene: &str) -> App {
    let options = PlayerOptions {
        project: Some(project()),
        scene: Some(scene.into()),
        watch: false,
        ..Default::default()
    };
    let (mut app, _) = build_app(&Game::new(), &options).unwrap();
    let dir = save_dir();
    let _ = std::fs::remove_dir_all(&dir);
    app.world.resource_mut::<SaveConfig>().dir = Some(dir);
    app
}

/// Runs `n` frames of 1/60 s, letting imports finish before each.
fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut app.world, Duration::from_millis(50));
        }
        app.run_frames(1, DT);
    }
}

fn find(world: &World, name: &str) -> Entity {
    world
        .find_by_name(name)
        .unwrap_or_else(|| panic!("no entity `{name}`"))
}

fn player(world: &World) -> Entity {
    find(world, "Player")
}

fn pos(world: &World) -> Vec3 {
    world.get::<Transform>(player(world)).unwrap().translation
}

fn character(world: &World) -> KinematicCharacter {
    KinematicCharacter::clone(&world.get::<KinematicCharacter>(player(world)).unwrap())
}

fn teleport(world: &mut World, at: Vec3) {
    let p = player(world);
    world.component_mut::<Transform>(p).unwrap().translation = at;
    if let Some(c) = world.component_mut::<KinematicCharacter>(p) {
        c.velocity = Vec3::ZERO;
    }
}

/// A field of `entity`'s script instance (`self.<key>`).
fn field<T: box_engine::box_script::mlua::FromLua>(world: &World, entity: Entity, key: &str) -> T {
    world
        .resource::<ScriptRuntime>()
        .instance(entity)
        .unwrap_or_else(|| panic!("entity {entity:?} runs no script"))
        .get::<T>(key)
        .unwrap_or_else(|e| panic!("self.{key}: {e}"))
}

fn game(world: &World) -> Entity {
    let all: Vec<Entity> = world
        .query::<(Entity, &Name)>()
        .iter()
        .filter(|(_, n)| n.as_str() == "Game")
        .map(|(e, _)| e)
        .collect();
    assert_eq!(all.len(), 1, "exactly one game entity");
    all[0]
}

fn score(world: &World) -> (i64, i64, i64) {
    let g = game(world);
    (
        field(world, g, "score"),
        field(world, g, "collected"),
        field(world, g, "arenas"),
    )
}

fn gem_positions(world: &World) -> Vec<Vec3> {
    let mut gems: Vec<(String, Vec3)> = world
        .query::<(&Name, &Transform, &Script)>()
        .iter()
        .filter(|(_, _, s)| s.source.path.as_deref() == Some("scripts/gem.luau"))
        .map(|(n, t, _)| (n.as_str().to_string(), t.translation))
        .collect();
    gems.sort_by(|a, b| a.0.cmp(&b.0));
    gems.into_iter().map(|(_, p)| p).collect()
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

/// Stand on each gem of the arena in turn.
fn collect_all(app: &mut App) {
    for at in gem_positions(&app.world) {
        teleport(&mut app.world, at);
        frames(app, 3);
    }
    frames(app, 3);
}

/// Text of the UI node with `id`, as laid out.
fn ui_text(world: &World, id: &str) -> String {
    let ui = world.resource::<Ui>();
    let n = ui
        .find(None, id)
        .unwrap_or_else(|| panic!("no UI node `{id}`"));
    ui.text(n).unwrap_or_default()
}

fn focused_id(world: &World) -> Option<String> {
    let ui = world.resource::<Ui>();
    ui.focused().and_then(|n| ui.id_of(n).map(str::to_owned))
}

fn tap(app: &mut App, button: GamepadButton) {
    app.world
        .resource_mut::<Input>()
        .press_gamepad_button(PAD, button);
    frames(app, 1);
    app.world
        .resource_mut::<Input>()
        .release_gamepad_button(PAD, button);
    frames(app, 1);
}

fn menu_open(world: &World) -> bool {
    let ui = world.resource::<Ui>();
    ui.find(None, "pause_menu")
        .is_some_and(|m| ui.is_visible(m))
}

fn renderer(w: u32, h: u32) -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return None;
    }
    match Renderer::new_headless(w, h) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping captures");
            None
        }
        Err(e) => panic!("renderer: {e}"),
    }
}

/// Captures the app's frame (UI included) to `target/captures/m7/<name>`.
fn capture(r: &mut Renderer, app: &mut App, name: &str) {
    let img = r.capture(&mut app.world).expect("capture");
    let path = captures().join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save(&path).unwrap();
    assert_eq!(
        r.stats().validation_errors,
        0,
        "{name}: GPU validation errors"
    );
}

fn sized(app: &mut App, w: u32, h: u32) {
    app.world.insert_resource(WindowSize {
        width: w,
        height: h,
        scale_factor_milli: 1000,
    });
}

// ---- 1. The M3 sample game in Luau ------------------------------------------

#[test]
fn luau_collector_walks_collects_and_scores() {
    let mut app = app("arena");
    frames(&mut app, 10);
    let stats = app.world.resource::<ScriptRuntime>().stats();
    assert_eq!(stats.disabled, 0, "{stats:?}");
    assert_eq!(stats.errors, 0, "{stats:?}");
    let start = pos(&app.world);
    assert!(character(&app.world).grounded, "standing at {start}");
    assert!(close(start.y, STAND, 0.01), "{start}");
    assert_eq!(ui_text(&app.world, "score"), "Score 0");
    assert_eq!(ui_text(&app.world, "gems"), "Gems left 6");

    // W walks forward (the camera looks down -Z) through Gem 1 at the centre.
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    frames(&mut app, 60);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    frames(&mut app, 2);
    let end = pos(&app.world);
    let walked = start.z - end.z;
    // 60 fixed steps of 0.1 m: `on_fixed_update` runs before the character
    // moves, so input applies in the step it is given (collector's
    // `luau_parity` test checks the Rust and Luau games frame by frame).
    assert!(
        close(walked, 6.0, 1e-3),
        "walked {walked} m in 1 s at 6 m/s ({start} -> {end})"
    );
    assert!(close(end.x, start.x, 0.01), "drifted sideways: {end}");
    assert!(close(end.y, STAND, 0.01), "left the floor: {end}");

    assert!(app.world.find_by_name("Gem 1").is_none(), "Gem 1 collected");
    assert_eq!(score(&app.world), (10, 1, 0));
    assert_eq!(gem_positions(&app.world).len(), 5);
    assert_eq!(ui_text(&app.world, "score"), "Score 10");
    assert_eq!(ui_text(&app.world, "gems"), "Gems left 5");
    // The pickup sparkles (particles.burst) and sound (PlaySound).
    assert!(app.world.find_by_name("Particle burst").is_some());
}

#[test]
fn luau_collector_switches_arenas_and_keeps_the_score() {
    let mut app = app("arena");
    frames(&mut app, 5);
    assert_eq!(scene_file(&app.world), "arena.scene.json");
    assert_eq!(gem_positions(&app.world).len(), 6);
    let game_entity = game(&app.world);

    collect_all(&mut app);
    assert_eq!(score(&app.world), (75, 6, 1));
    assert_eq!(scene_file(&app.world), "arena2.scene.json");
    assert_eq!(gem_positions(&app.world).len(), 7);
    assert_eq!(game(&app.world), game_entity, "the game persists");
    let start = pos(&app.world);
    assert!(
        close(start.x, -9.0, 0.01) && close(start.z, 5.0, 0.01),
        "{start}"
    );
    assert!(app.world.find_by_name("Monolith").is_some());
    assert_eq!(ui_text(&app.world, "score"), "Score 75");

    collect_all(&mut app);
    assert_eq!(score(&app.world), (160, 13, 2));
    assert_eq!(scene_file(&app.world), "arena.scene.json", "loops back");
    assert_eq!(gem_positions(&app.world).len(), 6);
    assert_eq!(app.world.resource::<ScriptRuntime>().stats().errors, 0);
}

#[test]
fn luau_collector_gamepad_moves_relative_to_the_camera_and_jumps() {
    let mut app = app("arena");
    frames(&mut app, 10);
    let start = pos(&app.world);

    // Left stick right: +X (the camera looks down -Z).
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 1.0);
    frames(&mut app, 30);
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 0.0);
    frames(&mut app, 3);
    let moved = pos(&app.world) - start;
    assert!(close(moved.x, 3.0, 1e-3), "{moved}");
    assert!(close(moved.z, 0.0, 0.01), "{moved}");

    // South jumps: up about v^2 / 2g = 1.4 m, then back onto the floor.
    app.world
        .resource_mut::<Input>()
        .press_gamepad_button(PAD, GamepadButton::South);
    let mut top: f32 = 0.0;
    for _ in 0..90 {
        frames(&mut app, 1);
        top = top.max(pos(&app.world).y);
    }
    app.world
        .resource_mut::<Input>()
        .release_gamepad_button(PAD, GamepadButton::South);
    frames(&mut app, 2);
    assert!(close(top - STAND, 1.4, 0.1), "jump apex {top}");
    let end = pos(&app.world);
    assert!(close(end.y, STAND, 0.01), "landed at {end}");
    assert!(character(&app.world).grounded);
    assert_eq!(field::<i64>(&app.world, player(&app.world), "jumps"), 1);
}

#[test]
fn luau_collector_walls_stop_the_player_and_falling_out_resets_it() {
    let mut app = app("arena");
    frames(&mut app, 5);
    let start = pos(&app.world);
    // The east wall's inner face is at x = 12.
    teleport(&mut app.world, Vec3::new(9.0, STAND, -1.0));
    frames(&mut app, 30);
    app.world.resource_mut::<Input>().press_key(KeyCode::D);
    frames(&mut app, 60);
    app.world.resource_mut::<Input>().release_key(KeyCode::D);
    frames(&mut app, 2);
    let end = pos(&app.world);
    assert!(end.x <= 12.0 - 0.4 && end.x > 12.0 - 0.5, "{end}");

    // Outside the walls, below the floor: falls past the limit (-30 m in
    // assets/data/tuning.data.json) and is put back at its start.
    teleport(&mut app.world, Vec3::new(30.0, -20.0, 0.0));
    let mut lowest = f32::MAX;
    let mut back = false;
    for _ in 0..120 {
        frames(&mut app, 1);
        let p = pos(&app.world);
        lowest = lowest.min(p.y);
        if (p - start).length() < 0.05 {
            back = true;
            break;
        }
    }
    assert!(
        back,
        "reset to {start}; lowest {lowest}, now {}",
        pos(&app.world)
    );
    assert!(lowest < -29.0, "fell to {lowest}");
    frames(&mut app, 5);
    assert!(character(&app.world).grounded);
}

/// The generated luau-lsp definitions in the project match what
/// `box script types` writes for the engine (regenerate with
/// `cargo run -p box_cli -- script types --project examples/collector_luau`),
/// and declare the APIs the scripts use.
#[test]
fn luau_collector_definitions_are_current() {
    let path = project().join(box_engine::box_script::types::DEFINITIONS_PATH);
    let committed = std::fs::read_to_string(&path).expect("committed definitions");
    let fresh = Game::new().script_definitions();
    // The committed file is what the stock tools (every engine module)
    // write; a build without `dynamics` declares fewer `physics` functions.
    assert!(
        committed == fresh || !cfg!(feature = "dynamics"),
        "{} is out of date: run `cargo run -p box_cli -- script types --project examples/collector_luau`",
        path.display()
    );
    for api in [
        "declare character:",
        "declare animation:",
        "declare ui:",
        "declare strings:",
        "declare save:",
        "declare data:",
        "declare particles:",
        "declare scenes:",
        "function interpolated_position(self): vector",
    ] {
        assert!(fresh.contains(api), "no `{api}`");
    }
    let vscode = std::fs::read_to_string(project().join(".vscode/settings.json")).unwrap();
    assert!(vscode.contains(".box/types/box.d.luau"));
}

// ---- 3. Skinned character -----------------------------------------------------

#[test]
fn skinned_character_walks_runs_and_jumps_with_blended_animation() {
    let (w, h) = (640, 360);
    let mut app = app("arena");
    sized(&mut app, w, h);
    let mut r = renderer(w, h);
    frames(&mut app, 10);
    let body = find(&app.world, "Body");
    let state = |w: &World| {
        w.get::<AnimationStateMachineState>(body)
            .map(|s| s.current().to_string())
            .unwrap_or_default()
    };
    let layers = |w: &World| {
        w.get::<AnimationPlayerState>(body)
            .map(|s| s.layers())
            .unwrap_or_default()
    };
    assert_eq!(state(&app.world), "Idle");
    // The skeleton came from the glTF and is animated: a joint moves.
    let hips = app
        .world
        .query::<(Entity, &Name)>()
        .iter()
        .find(|(_, n)| n.as_str() == "Hips")
        .map(|(e, _)| e)
        .expect("the character's joints");
    let foot = app
        .world
        .query::<(Entity, &Name)>()
        .iter()
        .find(|(_, n)| n.as_str() == "Foot.L")
        .map(|(e, _)| e)
        .unwrap();
    let mut sequence = 0;
    let mut shoot = |app: &mut App, r: &mut Option<Renderer>| {
        if let Some(r) = r {
            capture(r, app, &format!("character/f_{sequence:03}.png"));
        }
        sequence += 1;
    };

    // A light push forward on the stick: walk, blending in from idle.
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 0.45);
    let mut blended = None;
    for _ in 0..40 {
        frames(&mut app, 1);
        let l = layers(&app.world);
        if l.len() == 2 && blended.is_none() {
            blended = Some(l);
        }
    }
    let blend = blended.expect("a crossfade between two clips");
    let sum: f32 = blend.iter().map(|l| l.weight).sum();
    assert!(close(sum, 1.0, 1e-4), "{blend:?}");
    assert!(
        blend.iter().all(|l| l.weight > 0.0 && l.weight < 1.0),
        "{blend:?}"
    );
    assert!(
        blend[0].clip.to_string().ends_with("#Animation/Idle"),
        "{blend:?}"
    );
    assert!(
        blend[1].clip.to_string().ends_with("#Animation/Walk"),
        "{blend:?}"
    );
    assert_eq!(state(&app.world), "Walk");
    let walking = character(&app.world).velocity;
    let speed = Vec3::new(walking.x, 0.0, walking.z).length();
    assert!(speed > 0.5 && speed < 3.0, "walking at {speed} m/s");
    shoot(&mut app, &mut r);
    let foot_at = |w: &World| w.get::<GlobalTransform>(foot).unwrap().translation();
    let f0 = foot_at(&app.world);
    frames(&mut app, 15);
    assert!((foot_at(&app.world) - f0).length() > 0.05, "the legs move");

    // Full stick: run.
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 1.0);
    frames(&mut app, 30);
    assert_eq!(state(&app.world), "Run");
    let v = character(&app.world).velocity;
    assert!(close(Vec3::new(v.x, 0.0, v.z).length(), 6.0, 0.01), "{v}");
    shoot(&mut app, &mut r);
    // Running toward -Z: the body faces -Z (its forward).
    let facing = *app.world.get::<GlobalTransform>(body).unwrap();
    let forward = facing.0.transform_vector3(Vec3::NEG_Z).normalize();
    assert!(forward.dot(Vec3::NEG_Z) > 0.95, "faces {forward}");

    // Jump while running: the Jump state, airborne, then back to running.
    app.world
        .resource_mut::<Input>()
        .press_gamepad_button(PAD, GamepadButton::South);
    frames(&mut app, 4);
    app.world
        .resource_mut::<Input>()
        .release_gamepad_button(PAD, GamepadButton::South);
    assert_eq!(state(&app.world), "Jump");
    assert!(!character(&app.world).grounded);
    let mut states = Vec::new();
    for i in 0..60 {
        frames(&mut app, 1);
        if i % 6 == 0 {
            shoot(&mut app, &mut r);
        }
        let s = state(&app.world);
        if states.last() != Some(&s) {
            states.push(s);
        }
    }
    assert!(character(&app.world).grounded, "landed");
    assert_eq!(states.first().map(String::as_str), Some("Jump"));
    assert_eq!(states.last().map(String::as_str), Some("Run"), "{states:?}");
    assert!(app.world.get::<Transform>(hips).is_some());

    // Let go: back to idle.
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 0.0);
    frames(&mut app, 30);
    assert_eq!(state(&app.world), "Idle");
    shoot(&mut app, &mut r);
}

// ---- 4. Interior lighting -------------------------------------------------------

#[test]
fn interior_is_lit_by_local_lights_without_sky_leak() {
    use common::interior::{InteriorOptions, build_interior};
    let build = |ambient_override: bool| {
        let mut app = box_engine::default_app();
        build_interior(
            &mut app.world,
            InteriorOptions {
                ambient_override,
                ..Default::default()
            },
        );
        for _ in 0..3 {
            app.tick(DT);
        }
        app
    };
    let mut app = build(true);
    let w = &app.world;
    assert!(w.count::<&SpotLight>() >= 1, "spot lights");
    assert!(w.count::<&PointLight>() >= 1, "point lights");
    let shadowed = w.query::<&SpotLight>().iter().filter(|s| s.shadows).count()
        + w.query::<&PointLight>()
            .iter()
            .filter(|p| p.shadows)
            .count();
    assert_eq!(shadowed, 1, "one shadowed local light");
    let volume = w
        .query::<&AmbientVolume>()
        .iter()
        .next()
        .map(|v| v.sky_contribution);
    assert_eq!(volume, Some(0.0), "the room keeps the sky ambient out");

    let Some(mut r) = renderer(640, 360) else {
        return;
    };
    let inside = r.capture(&mut app.world).expect("capture");
    inside.save(captures().join("interior.png")).unwrap();
    assert_eq!(r.stats().validation_errors, 0);
    golden::check(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden"),
        "interior",
        &inside,
    )
    .unwrap();
    // Without the ambient volume the sky ambient lights the room: brighter.
    let mut leaky = build(false);
    let leak = r.capture(&mut leaky.world).expect("capture");
    leak.save(captures().join("interior_sky_leak.png")).unwrap();
    let mean = |img: &image::RgbaImage| {
        img.pixels()
            .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
            .sum::<u64>() as f64
            / img.pixels().len() as f64
    };
    assert!(
        mean(&leak) > mean(&inside) * 1.05,
        "the override darkens the room: {} vs {}",
        mean(&inside),
        mean(&leak)
    );
}

// ---- 5. Menu by gamepad, text from string tables ------------------------------

#[test]
fn pause_menu_navigates_by_gamepad_with_string_table_text() {
    let (w, h) = (1280, 720);
    let mut app = app("arena");
    sized(&mut app, w, h);
    let mut r = renderer(w, h);
    frames(&mut app, 10);
    assert!(!menu_open(&app.world));

    // Start opens the menu: modal, the game paused, focus on Resume.
    tap(&mut app, GamepadButton::Start);
    assert!(menu_open(&app.world));
    assert!(app.world.resource::<Time>().paused);
    assert!(app.world.resource::<Ui>().captures_input());
    assert_eq!(focused_id(&app.world).as_deref(), Some("resume"));
    assert_eq!(ui_text(&app.world, "resume"), "Resume");
    assert_eq!(ui_text(&app.world, "language"), "Language: English");
    if let Some(r) = &mut r {
        capture(r, &mut app, "collector_luau_menu_en.png");
    }

    // Gameplay input is captured while the menu is open.
    let at = pos(&app.world);
    // D-pad down three times: Save, Load, Language.
    for expected in ["save", "load", "language"] {
        tap(&mut app, GamepadButton::DPadDown);
        assert_eq!(focused_id(&app.world).as_deref(), Some(expected));
    }
    assert_eq!(pos(&app.world), at, "the d-pad did not move the player");
    // South on Language: German, from de.strings.json.
    tap(&mut app, GamepadButton::South);
    frames(&mut app, 2);
    assert_eq!(app.world.resource::<Strings>().language(), "de");
    assert_eq!(ui_text(&app.world, "resume"), "Weiterspielen");
    assert_eq!(ui_text(&app.world, "language"), "Sprache: Deutsch");
    assert_eq!(ui_text(&app.world, "score"), "Punkte 0");
    assert_eq!(ui_text(&app.world, "gems"), "Edelsteine übrig 6");
    if let Some(r) = &mut r {
        capture(r, &mut app, "collector_luau_menu_de.png");
    }
    // Wraps around: down from Language is Resume.
    tap(&mut app, GamepadButton::DPadDown);
    assert_eq!(focused_id(&app.world).as_deref(), Some("resume"));

    // East (back) closes it and the game runs again.
    tap(&mut app, GamepadButton::East);
    assert!(!menu_open(&app.world));
    assert!(!app.world.resource::<Time>().paused);
    // Start again opens it, South on Resume closes it.
    tap(&mut app, GamepadButton::Start);
    assert!(menu_open(&app.world));
    assert_eq!(focused_id(&app.world).as_deref(), Some("resume"));
    tap(&mut app, GamepadButton::South);
    assert!(!menu_open(&app.world));
    // The keyboard too: Escape opens and closes it.
    app.world.resource_mut::<Input>().press_key(KeyCode::Escape);
    frames(&mut app, 1);
    app.world
        .resource_mut::<Input>()
        .release_key(KeyCode::Escape);
    frames(&mut app, 1);
    assert!(menu_open(&app.world));
    app.world.resource_mut::<Input>().press_key(KeyCode::Escape);
    frames(&mut app, 1);
    app.world
        .resource_mut::<Input>()
        .release_key(KeyCode::Escape);
    frames(&mut app, 1);
    assert!(!menu_open(&app.world));
    assert_eq!(app.world.resource::<ScriptRuntime>().stats().errors, 0);
}

// ---- 6. Save games: entity and script state -------------------------------------

#[test]
fn saved_game_restores_entities_and_script_state() {
    let (w, h) = (1280, 720);
    let mut app = app("arena");
    sized(&mut app, w, h);
    let mut r = renderer(w, h);
    frames(&mut app, 10);
    let gem_at = |w: &World, name: &str| w.get::<Transform>(find(w, name)).unwrap().translation;

    // Collect Gem 1, jump once.
    let gem1 = gem_at(&app.world, "Gem 1");
    teleport(&mut app.world, gem1);
    frames(&mut app, 5);
    assert!(app.world.find_by_name("Gem 1").is_none());
    frames(&mut app, 30);
    tap(&mut app, GamepadButton::South);
    frames(&mut app, 60);
    assert_eq!(score(&app.world), (10, 1, 0));
    assert_eq!(field::<i64>(&app.world, player(&app.world), "jumps"), 1);
    let saved_at = pos(&app.world);

    // Menu: Save (one down from Resume).
    tap(&mut app, GamepadButton::Start);
    tap(&mut app, GamepadButton::DPadDown);
    assert_eq!(focused_id(&app.world).as_deref(), Some("save"));
    tap(&mut app, GamepadButton::South);
    frames(&mut app, 2);
    assert!(!menu_open(&app.world));
    let file = app
        .world
        .resource::<SaveConfig>()
        .dir
        .clone()
        .unwrap()
        .join("quick.save.json");
    assert!(file.exists(), "{} written", file.display());
    assert_eq!(ui_text(&app.world, "toast"), "Game saved");

    // Play on: collect Gem 2, jump again, walk away.
    let gem2 = gem_at(&app.world, "Gem 2");
    teleport(&mut app.world, gem2);
    frames(&mut app, 5);
    assert!(app.world.find_by_name("Gem 2").is_none());
    frames(&mut app, 30);
    tap(&mut app, GamepadButton::South);
    frames(&mut app, 60);
    teleport(&mut app.world, Vec3::new(-8.0, STAND, -8.0));
    frames(&mut app, 5);
    assert_eq!(score(&app.world), (20, 2, 0));
    assert_eq!(field::<i64>(&app.world, player(&app.world), "jumps"), 2);

    // Menu: Load (two down).
    tap(&mut app, GamepadButton::Start);
    tap(&mut app, GamepadButton::DPadDown);
    tap(&mut app, GamepadButton::DPadDown);
    assert_eq!(focused_id(&app.world).as_deref(), Some("load"));
    tap(&mut app, GamepadButton::South);
    frames(&mut app, 5);
    assert!(!menu_open(&app.world));
    assert!(!app.world.resource::<Time>().paused);

    // Entities: Gem 2 is back, Gem 1 stays collected, the player is where
    // it was saved.
    assert!(app.world.find_by_name("Gem 2").is_some(), "Gem 2 restored");
    assert!(
        app.world.find_by_name("Gem 1").is_none(),
        "Gem 1 still collected"
    );
    assert_eq!(gem_positions(&app.world).len(), 5);
    let p = pos(&app.world);
    assert!((p - saved_at).length() < 0.05, "{p} vs {saved_at}");
    // Script state: the game's score and the player's jump count.
    assert_eq!(score(&app.world), (10, 1, 0));
    assert_eq!(field::<i64>(&app.world, player(&app.world), "jumps"), 1);
    assert_eq!(ui_text(&app.world, "score"), "Score 10");
    assert_eq!(ui_text(&app.world, "gems"), "Gems left 5");
    assert_eq!(ui_text(&app.world, "toast"), "Game loaded");
    // One HUD, one menu: the replaced game removed its UI.
    let ui = app.world.resource::<Ui>();
    let huds = ui
        .roots()
        .iter()
        .filter(|n| ui.id_of(**n) == Some("hud"))
        .count();
    assert_eq!(huds, 1);
    if let Some(r) = &mut r {
        frames(&mut app, 10);
        capture(r, &mut app, "collector_luau_after_load.png");
    }
    // The game goes on: the restored gem can be collected again.
    let gem2 = gem_at(&app.world, "Gem 2");
    teleport(&mut app.world, gem2);
    frames(&mut app, 5);
    assert_eq!(score(&app.world), (20, 2, 0));
    let stats = app.world.resource::<ScriptRuntime>().stats();
    assert_eq!((stats.errors, stats.disabled), (0, 0), "{stats:?}");
}

/// Gameplay captures: running with the HUD, mid-jump.
#[test]
fn luau_collector_captures() {
    let (w, h) = (1280, 720);
    let mut app = app("arena");
    sized(&mut app, w, h);
    let Some(mut r) = renderer(w, h) else { return };
    frames(&mut app, 10);
    // Run diagonally toward the crates, collect a gem on the way.
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 1.0);
    frames(&mut app, 50);
    capture(&mut r, &mut app, "collector_luau_run.png");
    app.world
        .resource_mut::<Input>()
        .press_gamepad_button(PAD, GamepadButton::South);
    frames(&mut app, 18);
    app.world
        .resource_mut::<Input>()
        .release_gamepad_button(PAD, GamepadButton::South);
    capture(&mut r, &mut app, "collector_luau_jump.png");
}
