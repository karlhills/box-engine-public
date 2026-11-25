//! Behaviours B2a (docs/BEHAVIOURS.md, docs/NAVIGATION.md): health, damage
//! and heal actions, spawners, and the Door to Scene, Key and Lock and Lever
//! presets, played headlessly with injected actions.
//!
//! - `health_hits_invulnerability_and_respawn`: a hit lowers health and runs
//!   `on_damage`; hits during invulnerability are ignored; zero respawns the
//!   player with full health; with one life, zero loses. HUD line and
//!   capture `target/captures/behaviours/health_hud.png`.
//! - `heal_and_despawn`: `Heal` stops at the maximum; `Despawn` removes a
//!   spawned copy and only hides a saved entity.
//! - `spawner_makes_copies_up_to_its_limits`: interval, `max_alive`,
//!   lifetime, `total`, hidden template left out of groups, `SetEnabled`,
//!   copies not saved; a spike ball copy hurts the player and goes. Capture
//!   `spawner.png`.
//! - `key_and_lock_and_lever`: the key opens the door once; the lever
//!   toggles its gate's visibility and collision.
//! - `door_to_scene_asks_for_the_scene`: the prompt shows and using it asks
//!   for the scene switch.

use std::path::PathBuf;

use box_engine::behaviours::{
    GamePhase, GameState, Health, HealthState, HudSettings, SpawnTemplate, Spawner, SpawnerState,
};
use box_engine::box_core::action::InjectedInput;
use box_engine::box_core::edit::{Command, EditStack, apply_commands};
use box_engine::box_core::visibility::{Transient, is_hidden};
use box_engine::interact::{
    GameVars, InteractAction, InteractFocus, InteractRequests, Interactable, Target, group_progress,
};
use box_engine::prelude::*;
use box_engine::{app_with_world, shared_engine_registry};
use box_physics::Collidable;
use box_render::RenderError;

const DT: f32 = 1.0 / 60.0;

fn captures() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/behaviours");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Play {
    app: App,
    renderer: Option<Renderer>,
}

impl Play {
    /// Ground, light, the player, rules, the HUD, plus `extra` commands.
    fn new(rules: serde_json::Value, extra: serde_json::Value) -> Self {
        let mut world = World::with_registry(shared_engine_registry());
        let mut cmds = vec![
            serde_json::json!({ "op": "spawn", "name": "Ground", "components": {
                "Transform": { "translation": [0.0, -0.5, 0.0] },
                "Primitive": { "shape": { "Cuboid": { "size": [40.0, 1.0, 40.0] } } },
                "StandardMaterial": { "base_color": [0.35, 0.5, 0.3, 1.0] },
                "Collider": { "shape": "Primitive" } } }),
            serde_json::json!({ "op": "spawn", "name": "Sun", "components": {
                "Transform": { "rotation": [-0.42, 0.27, 0.13, 0.86] },
                "DirectionalLight": { "intensity": 3.0 } } }),
            serde_json::json!({ "op": "add_preset", "preset": "player", "at": [0.0, 0.0, 0.0] }),
            serde_json::json!({ "op": "set_resource", "path": "GameRules", "value": rules }),
            serde_json::json!({ "op": "set_resource", "path": "HudSettings", "value": {} }),
        ];
        cmds.extend(extra.as_array().cloned().unwrap_or_default());
        let cmds: Vec<Command> = serde_json::from_value(serde_json::Value::Array(cmds)).unwrap();
        apply_commands(&mut world, &mut EditStack::default(), &cmds, "level").unwrap();
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

    /// Walks the player toward `target` (xz); the follow camera looks down
    /// -Z, so +Y on the stick is world -Z. Stops early when `stop` holds.
    fn walk_until(
        &mut self,
        target: Vec3,
        tolerance: f32,
        max_frames: usize,
        mut stop: impl FnMut(&mut Self) -> bool,
    ) {
        for _ in 0..max_frames {
            let at = self.pos("Player");
            let d = Vec2::new(target.x - at.x, target.z - at.z);
            if d.length() <= tolerance || stop(self) {
                self.steps(3);
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

    fn walk_to(&mut self, target: Vec3, tolerance: f32) {
        self.walk_until(target, tolerance, 600, |_| false);
    }

    fn press(&mut self, action: &str) {
        self.world()
            .resource_mut::<Actions>()
            .inject(action, InjectedInput::press(1));
        self.steps(2);
    }

    fn health(&mut self) -> f32 {
        let p = self.entity("Player");
        self.world().get::<Health>(p).unwrap().current
    }

    fn vars(&mut self, name: &str) -> f64 {
        self.world().resource::<GameVars>().get(name)
    }
}

/// A spike strip that hurts whoever walks onto it, and counts its hits.
fn spikes(at: [f32; 3]) -> serde_json::Value {
    serde_json::json!({ "op": "spawn", "name": "Spikes", "components": {
        "Transform": { "translation": at },
        "Primitive": { "shape": { "Cuboid": { "size": [1.5, 0.1, 1.5] } } },
        "Interactable": {
            "trigger": "Touch", "radius": 1.2, "offset": [0.0, 0.9, 0.0], "cooldown": 0.0,
            "actions": [{ "Damage": { "target": "Interactor", "amount": 1.0 } }] } } })
}

#[test]
fn health_hits_invulnerability_and_respawn() {
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([spikes([0.0, 0.0, -4.0])]),
    );
    let player = play.entity("Player");
    // The player preset has full health; the HUD shows it.
    assert_eq!(play.health(), 3.0);
    let hud = box_engine::behaviours::hud_lines(play.world(), &HudSettings::default(), None);
    assert!(hud.contains(&"Health 3 / 3".to_string()), "{hud:?}");
    // on_damage runs with every hit.
    play.world()
        .component_mut::<Health>(player)
        .unwrap()
        .on_damage = vec![InteractAction::AddVar {
        var: "hits".into(),
        amount: 1.0,
    }];

    // Walking onto the spikes: one hit.
    play.walk_until(Vec3::new(0.0, 0.0, -4.0), 0.3, 300, |p| p.health() < 3.0);
    play.steps(2);
    assert_eq!(play.health(), 2.0);
    assert_eq!(play.vars("hits"), 1.0);
    let hud = box_engine::behaviours::hud_lines(play.world(), &HudSettings::default(), None);
    assert!(hud.contains(&"Health 2 / 3".to_string()), "{hud:?}");
    play.capture("health_hud.png");

    // Invulnerable for 0.8 s after the hit.
    let src = play.entity("Spikes");
    assert!(!box_engine::behaviours::damage(
        play.world(),
        player,
        1.0,
        Some(src)
    ));
    assert_eq!(play.health(), 2.0);
    play.steps(50);
    assert!(box_engine::behaviours::damage(
        play.world(),
        player,
        1.0,
        Some(src)
    ));
    assert_eq!(play.health(), 1.0);
    play.steps(2);
    assert_eq!(play.vars("hits"), 2.0);

    // Zero: back to the start with full health (no lives set: free respawn).
    let start = box_engine::behaviours::respawn_point(play.world(), player).unwrap();
    play.steps(50);
    assert!(box_engine::behaviours::damage(
        play.world(),
        player,
        5.0,
        None
    ));
    play.steps(3);
    assert_eq!(play.health(), 3.0, "refilled after the respawn");
    assert_eq!(play.world().resource::<GameState>().respawns, 1);
    let at = play.pos("Player");
    assert!(
        Vec2::new(at.x - start.x, at.z - start.z).length() < 0.5,
        "{at} vs {start}"
    );
    assert_eq!(
        play.world().resource::<GameState>().phase,
        GamePhase::Playing
    );

    // A script (or the inspector) lowering health to zero also runs on_zero.
    play.steps(60);
    play.world()
        .component_mut::<Health>(player)
        .unwrap()
        .current = 0.0;
    play.steps(3);
    assert_eq!(play.world().resource::<GameState>().respawns, 2);
    assert_eq!(play.health(), 3.0);

    // With one life, zero loses.
    let mut play = Play::new(serde_json::json!({ "lives": 1 }), serde_json::json!([]));
    let player = play.entity("Player");
    box_engine::behaviours::damage(play.world(), player, 3.0, None);
    play.steps(3);
    assert_eq!(play.world().resource::<GameState>().phase, GamePhase::Lost);
}

#[test]
fn heal_and_despawn() {
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([
            { "op": "spawn", "name": "Crate", "components": {
                "Transform": { "translation": [3.0, 0.5, 0.0] },
                "Primitive": { "shape": { "Cuboid": { "size": [1.0, 1.0, 1.0] } } },
                "Collider": { "shape": "Primitive" } } }
        ]),
    );
    let player = play.entity("Player");
    box_engine::behaviours::damage(play.world(), player, 2.0, None);
    play.steps(2);
    assert_eq!(play.health(), 1.0);
    // Heal through the action queue, as an interactable would.
    let crate_ = play.entity("Crate");
    play.world().resource_mut::<InteractRequests>().run_actions(
        crate_,
        Some(player),
        vec![InteractAction::Heal {
            target: Target::Interactor,
            amount: 5.0,
        }],
    );
    play.steps(2);
    assert_eq!(play.health(), 3.0, "stops at the maximum");

    // Despawn on a saved entity hides it and switches its collision off.
    play.world().resource_mut::<InteractRequests>().run_actions(
        crate_,
        None,
        vec![InteractAction::Despawn {
            target: Target::This,
        }],
    );
    play.steps(2);
    assert!(play.world().contains(crate_));
    assert!(is_hidden(play.world(), crate_));
    assert_eq!(
        play.world().get::<Collidable>(crate_).map(|c| c.0),
        Some(false)
    );
    // On a transient one it despawns.
    let spawned = play
        .world()
        .spawn((Name::new("Spark"), Transform::default(), Transient));
    play.world().resource_mut::<InteractRequests>().run_actions(
        spawned,
        None,
        vec![InteractAction::Despawn {
            target: Target::This,
        }],
    );
    play.steps(2);
    assert!(!play.world().contains(spawned));
}

fn copies(play: &mut Play) -> Vec<Entity> {
    play.world()
        .query::<(Entity, &Name, &Transient)>()
        .iter()
        .filter(|(_, n, _)| n.0 == "Spike Ball")
        .map(|(e, _, _)| e)
        .collect()
}

#[test]
fn spawner_makes_copies_up_to_its_limits() {
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([
            { "op": "add_preset", "preset": "spawner", "at": [0.0, 0.0, -8.0] }
        ]),
    );
    let spawner = play.entity("Spawner");
    let template = play.world().children(spawner)[0];
    // The template is hidden and solid nowhere while playing.
    assert!(play.world().has::<SpawnTemplate>(template));
    assert!(is_hidden(play.world(), template));
    // Template groups do not count.
    play.world()
        .component_mut::<Interactable>(template)
        .unwrap()
        .group = "balls".into();
    assert_eq!(group_progress(play.world(), "balls"), (0, 0));

    // No copy before one interval (3 s; 0.5 s ran already).
    play.steps(140);
    assert!(copies(&mut play).is_empty());
    play.steps(20);
    let first = copies(&mut play);
    assert_eq!(first.len(), 1);
    let c = first[0];
    assert!(!is_hidden(play.world(), c));
    assert!(play.world().get::<Interactable>(c).is_some());
    // Within the radius (3 m) of the spawner.
    let at: Vec3 = play
        .world()
        .get::<GlobalTransform>(c)
        .unwrap()
        .0
        .translation
        .into();
    let base = play.pos("Spawner");
    assert!(
        Vec2::new(at.x - base.x, at.z - base.z).length() <= 3.01,
        "{at}"
    );
    assert_eq!(group_progress(play.world(), "balls"), (0, 0));

    // max_alive 3: no fourth copy while three live.
    play.steps(60 * 7);
    assert_eq!(copies(&mut play).len(), 3);
    assert_eq!(
        play.world().get::<SpawnerState>(spawner).unwrap().spawned,
        3
    );
    play.capture("spawner.png");

    // Lifetime 10 s: the first copy goes, and a new one replaces it.
    play.steps(60 * 3);
    assert!(!play.world().contains(c), "the first copy expired");
    // The free place fills after one more interval.
    play.steps(60 * 3 + 10);
    let n = play.world().get::<SpawnerState>(spawner).unwrap().spawned;
    assert!(n >= 4, "spawned {n}");

    // A copy hurts the player on touch and disappears.
    let ball = copies(&mut play)[0];
    let ball_at: Vec3 = play
        .world()
        .get::<GlobalTransform>(ball)
        .unwrap()
        .0
        .translation
        .into();
    let before = copies(&mut play);
    play.walk_until(ball_at, 0.2, 600, |p| p.health() < 3.0);
    play.steps(3);
    assert_eq!(play.health(), 2.0);
    // The ball that hit (spots are random per spawner, so maybe not the one
    // walked to) is gone.
    assert!(before.iter().any(|b| !play.world().contains(*b)));
    let _ = ball;

    // SetEnabled stops it; copies are not saved.
    play.world().resource_mut::<InteractRequests>().run_actions(
        spawner,
        None,
        vec![InteractAction::SetEnabled {
            target: Target::This,
            enabled: false,
        }],
    );
    play.steps(2);
    assert!(!play.world().get::<Spawner>(spawner).unwrap().enabled);
    let before = play.world().get::<SpawnerState>(spawner).unwrap().spawned;
    play.steps(60 * 4);
    assert_eq!(
        play.world().get::<SpawnerState>(spawner).unwrap().spawned,
        before
    );
    let save = box_engine::savegame::snapshot(play.world()).unwrap();
    box_engine::savegame::restore(play.world(), &save).unwrap();
    play.steps(2);
    assert!(copies(&mut play).is_empty(), "copies are not saved");

    // `total`: two copies in all.
    let spawner = play.entity("Spawner");
    {
        let s = &mut *play.world().component_mut::<Spawner>(spawner).unwrap();
        s.enabled = true;
        s.total = 2;
        s.interval = 0.5;
        s.lifetime = 0.6;
    }
    play.steps(60 * 5);
    let state = play.world().get::<SpawnerState>(spawner).unwrap().clone();
    assert_eq!(state.spawned, 2);
    assert_eq!(state.alive_count(), 0);
}

#[test]
fn key_and_lock_and_lever() {
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([
            { "op": "add_preset", "preset": "key_and_lock", "at": [0.0, 0.0, -8.0] },
            { "op": "add_preset", "preset": "lever", "at": [6.0, 0.0, 0.0] }
        ]),
    );
    // The locked door is blocked without a key.
    let door = play.entity("Locked Door");
    play.walk_to(Vec3::new(0.0, 0.0, -6.6), 0.3);
    assert_eq!(play.world().resource::<InteractFocus>().entity, Some(door));
    assert!(play.world().resource::<InteractFocus>().blocked);
    assert_eq!(play.world().resource::<InteractFocus>().text, "Needs a key");
    play.press("interact");
    assert!(!is_hidden(play.world(), door));

    // Picking the key up, then unlocking.
    let key_at = play.pos("Key");
    play.walk_to(key_at, 0.3);
    assert_eq!(play.vars("keys"), 1.0);
    play.walk_to(Vec3::new(0.0, 0.0, -6.6), 0.3);
    assert!(!play.world().resource::<InteractFocus>().blocked);
    play.press("interact");
    assert!(is_hidden(play.world(), door));
    assert_eq!(play.vars("keys"), 0.0);
    assert_eq!(
        play.world().get::<Collidable>(door).map(|c| c.0),
        Some(false)
    );

    // The lever opens its gate, and closes it again.
    let gate = play.entity("Gate");
    assert!(!is_hidden(play.world(), gate));
    play.walk_to(Vec3::new(6.0, 0.0, 1.2), 0.3);
    play.press("interact");
    assert!(is_hidden(play.world(), gate));
    assert_eq!(
        play.world().get::<Collidable>(gate).map(|c| c.0),
        Some(false)
    );
    let lever = play.entity("Lever");
    let down =
        box_engine::interact::resolve(play.world(), lever, &Target::Child("Handle Down".into()))
            .unwrap();
    assert!(!is_hidden(play.world(), down));
    play.steps(40);
    play.press("interact");
    assert!(!is_hidden(play.world(), gate));
    assert!(is_hidden(play.world(), down));
}

#[test]
fn door_to_scene_asks_for_the_scene() {
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([
            { "op": "add_preset", "preset": "door_to_scene", "at": [0.0, 0.0, -5.0] }
        ]),
    );
    let door = play.entity("Door to Scene");
    {
        let i = &mut *play.world().component_mut::<Interactable>(door).unwrap();
        for a in &mut i.actions {
            if let InteractAction::SwitchScene { scene } = a {
                *scene = "level2".into();
            }
        }
    }
    play.walk_to(Vec3::new(0.0, 0.0, -3.4), 0.3);
    assert_eq!(play.world().resource::<InteractFocus>().entity, Some(door));
    assert_eq!(play.world().resource::<InteractFocus>().text, "Enter");
    play.press("interact");
    assert_eq!(play.world().get::<Interactable>(door).unwrap().uses, 1);
    let _ = HealthState::default();
}

/// Navigation inline, so the enemy's paths are ready in the frame they are
/// needed (deterministic).
#[cfg(feature = "navigation")]
fn inline_nav(play: &mut Play) {
    play.world()
        .insert_resource(box_engine::nav_glue::Navigation::inline());
    play.steps(2);
}

#[cfg(feature = "navigation")]
#[test]
fn an_enemy_chases_around_a_wall_hits_searches_and_goes_home() {
    use box_engine::behaviours::{ChaserMode, ChaserState};
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([
            // A long wall south of the player, open at its west end.
            { "op": "spawn", "name": "Wall", "components": {
                "Transform": { "translation": [-1.0, 1.5, -3.0] },
                "Primitive": { "shape": { "Cuboid": { "size": [18.0, 3.0, 0.4] } } },
                "Collider": { "shape": "Primitive" } } },
            { "op": "add_preset", "preset": "enemy", "at": [-2.0, 0.0, -8.0] },
            { "op": "set", "entity": "Enemy", "path": "Chaser.sight", "value": 12.0 },
            { "op": "set", "entity": "Enemy", "path": "Chaser.give_up", "value": 16.0 }
        ]),
    );
    inline_nav(&mut play);
    let enemy = play.entity("Enemy");
    let player = play.entity("Player");
    let home = play.pos("Enemy");
    // The wall hides the player: it waits at home.
    play.steps(30);
    let mode = play.world().get::<ChaserState>(enemy).unwrap().mode;
    assert_eq!(mode, ChaserMode::Patrol);
    assert!(Vec2::new(play.pos("Enemy").x - home.x, play.pos("Enemy").z - home.z).length() < 0.2);

    // In view south-west of the wall: it chases and hits.
    box_engine::behaviours::teleport(play.world(), player, Vec3::new(-12.0, 0.92, -6.0));
    let mut hit = false;
    for _ in 0..60 * 8 {
        play.step();
        if play.health() < 3.0 {
            hit = true;
            break;
        }
    }
    assert!(hit, "not hit; enemy at {}", play.pos("Enemy"));
    assert_eq!(
        play.world().get::<ChaserState>(enemy).unwrap().mode,
        ChaserMode::Chase
    );
    play.capture("enemy_chase.png");

    // North of the wall, just round its end: it follows round the corner.
    play.steps(60);
    let before = play.health();
    box_engine::behaviours::teleport(play.world(), player, Vec3::new(-12.0, 0.92, 3.0));
    let mut hit = false;
    for _ in 0..60 * 10 {
        play.step();
        if play.health() < before {
            hit = true;
            break;
        }
    }
    assert!(
        hit,
        "not hit round the corner; enemy at {}",
        play.pos("Enemy")
    );
    assert!(
        play.pos("Enemy").z > -2.7,
        "round the wall: {}",
        play.pos("Enemy")
    );

    // Far away: it gives up, searches, and walks home.
    box_engine::behaviours::teleport(play.world(), player, Vec3::new(16.0, 0.92, 16.0));
    let mut modes = Vec::new();
    for _ in 0..60 * 30 {
        play.step();
        let m = play.world().get::<ChaserState>(enemy).unwrap().mode;
        if modes.last() != Some(&m) {
            modes.push(m);
        }
        if m == ChaserMode::Patrol {
            break;
        }
    }
    assert_eq!(
        modes,
        // The player is out of reach from the first step on.
        [ChaserMode::Search, ChaserMode::Return, ChaserMode::Patrol],
        "enemy at {}",
        play.pos("Enemy")
    );
    let at = play.pos("Enemy");
    assert!(
        Vec2::new(at.x - home.x, at.z - home.z).length() < 0.8,
        "home: {at}"
    );
}

#[cfg(feature = "navigation")]
#[test]
fn an_enemy_patrols_and_a_spawner_makes_enemies() {
    use box_engine::behaviours::{ChaserMode, ChaserState};
    let mut play = Play::new(
        serde_json::json!({}),
        serde_json::json!([
            { "op": "add_preset", "preset": "enemy", "at": [10.0, 0.0, 10.0] },
            { "op": "set", "entity": "Enemy", "path": "Chaser.patrol", "value": [[4.0, 0.0, 0.0], [0.0, 0.0, 0.0]] },
            { "op": "set", "entity": "Enemy", "path": "Chaser.wait", "value": 0.2 },
            { "op": "add_preset", "preset": "spawner", "at": [-12.0, 0.0, 12.0] }
        ]),
    );
    inline_nav(&mut play);
    let enemy = play.entity("Enemy");
    // Where it was placed (it may have started its beat already).
    let home = Vec3::new(10.0, 0.0, 10.0);
    let mut max_x: f32 = play.pos("Enemy").x;
    let mut back = false;
    for _ in 0..60 * 8 {
        play.step();
        let x = play.pos("Enemy").x;
        max_x = max_x.max(x);
        if max_x > home.x + 3.4 && (x - home.x).abs() < 0.6 {
            back = true;
        }
    }
    assert!(max_x > home.x + 3.4, "walked its beat: {max_x}");
    assert!(back, "and came back");
    assert_eq!(
        play.world().get::<ChaserState>(enemy).unwrap().mode,
        ChaserMode::Patrol
    );

    // An Enemy as a spawner's template: the hidden template stays put, the
    // copies chase.
    let spawner = play.entity("Spawner");
    let ball = play.world().children(spawner)[0];
    play.world().despawn(ball);
    let cmds: Vec<Command> = serde_json::from_value(serde_json::json!([
        { "op": "add_preset", "preset": "enemy", "name": "Spawned Enemy", "parent": "Spawner" }
    ]))
    .unwrap();
    apply_commands(play.world(), &mut EditStack::default(), &cmds, "template").unwrap();
    let template = play.entity("Spawned Enemy");
    let t0 = play.world().get::<Transform>(template).unwrap().translation;
    play.steps(60 * 4);
    let t1 = play.world().get::<Transform>(template).unwrap().translation;
    assert!(
        (t1 - t0).length() < 0.05,
        "the template stays: {t0} -> {t1}"
    );
    let copies: Vec<Entity> = play
        .world()
        .query::<(Entity, &Name, &box_engine::behaviours::Chaser, &Transient)>()
        .iter()
        .map(|(e, _, _, _)| e)
        .collect();
    assert!(!copies.is_empty(), "a spawned enemy");
}
