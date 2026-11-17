//! The M7 script bindings of the engine's modules, each driven from a
//! small Luau script in a temporary project: `character` and `physics`
//! (move_and_slide, forces, body reads, collision callbacks), `animation`,
//! `ui` and `strings`, `data`, `particles`, `camera`, `save` and `scenes`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_script::ScriptRuntime;
use box_engine::box_script::mlua::{FromLua, Table};
use box_engine::box_ui::{Strings, Ui};
use box_engine::prelude::*;
use box_engine::savegame::SaveExclude;
use box_engine::scenes::Persistent;

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "box_bindings_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let p = Self { root };
        for (path, text) in files {
            p.write(path, text);
        }
        p
    }

    fn write(&self, path: &str, text: &str) {
        let full = self.root.join("assets").join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    /// The stock engine game around a world with this project's assets.
    fn app(&self) -> App {
        let game = Game::new();
        let mut world = game.new_world();
        world.insert_resource(box_engine::asset_server(self.root.join("assets")).unwrap());
        let mut app = game.app(world);
        app.world.resource_mut::<SaveConfig>().dir = Some(self.root.join("saves"));
        app
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Runs frames until `done` holds (at most `limit` seconds of wall time).
fn run_until(app: &mut App, limit: f32, mut done: impl FnMut(&App) -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f32() < limit {
        let s = app.world.resource::<AssetServer>().clone();
        s.update_until_idle(&mut app.world, Duration::from_millis(50));
        app.run_frames(1, 1.0 / 60.0);
        if done(app) {
            return true;
        }
    }
    false
}

fn instance(app: &App, e: Entity) -> Option<Table> {
    app.world.get_resource::<ScriptRuntime>()?.instance(e)
}

fn field<T: FromLua>(app: &App, e: Entity, key: &str) -> T {
    instance(app, e)
        .unwrap_or_else(|| panic!("no script instance"))
        .get::<T>(key)
        .unwrap_or_else(|err| panic!("self.{key}: {err}"))
}

/// Runs until the script on `e` sets `self.done`, then checks it ran
/// without errors.
fn finish(app: &mut App, e: Entity) {
    let ok = run_until(app, 30.0, |app| {
        instance(app, e).is_some_and(|t| t.get::<bool>("done").unwrap_or(false))
    });
    let rt = app.world.get_resource::<ScriptRuntime>();
    let reason = rt.and_then(|rt| rt.disabled_reason(e));
    assert!(ok, "the script did not finish: {reason:?}");
    assert_eq!(rt.unwrap().stats().errors, 0, "{reason:?}");
}

fn spawn_script(
    app: &mut App,
    name: &str,
    path: &str,
    extra: impl box_engine::box_core::world::DynamicBundle,
) -> Entity {
    let e = app.world.spawn(extra);
    app.world
        .insert(e, (Uid::new(), Name::new(name), Script::new(path)))
        .unwrap();
    if !app.world.has::<Transform>(e) {
        app.world.insert_one(e, Transform::default()).unwrap();
    }
    e
}

fn floor(app: &mut App) {
    app.world.spawn((
        Name::new("Floor"),
        Transform::from_xyz(0.0, -0.5, 0.0),
        Collider::cuboid(Vec3::new(40.0, 1.0, 40.0)),
    ));
}

const CHARACTER: &str = r#"
local T = {}
function T.on_start(self)
    wait_frames(10)
    self.grounded = character.is_grounded(self.entity)
    self.ground = character.ground_entity(self.entity)
    self.normal_y = character.ground_normal(self.entity).y
    local x0 = self.entity:position().x
    character.move(self.entity, vector.create(3, 0, 0))
    wait(0.5)
    self.moved = self.entity:position().x - x0
    character.move(self.entity, vector.create(0, 0, 0))
    self.jumped = character.jump(self.entity)
    wait_frames(3)
    self.vy = character.velocity(self.entity).y
    self.airborne = not character.is_grounded(self.entity)
    character.teleport(self.entity, vector.create(5, 3, 5))
    self.teleported = self.entity:position()
    -- A capsule dropped onto the floor by a query.
    local slide = physics.move_and_slide(vector.create(-5, 3, 0), vector.create(0, -5, 0), { exclude = self.entity })
    self.slide_grounded = slide.grounded
    self.slide_y = slide.position.y
    -- Errors name the problem.
    local ok, err = pcall(character.jump, world.find("Floor"))
    self.no_character = not ok and string.find(tostring(err), "no KinematicCharacter") ~= nil
    self.done = true
end
return T
"#;

#[test]
fn character_and_move_and_slide() {
    let p = Project::new("character", &[("scripts/c.luau", CHARACTER)]);
    let mut app = p.app();
    floor(&mut app);
    let e = spawn_script(
        &mut app,
        "Hero",
        "scripts/c.luau",
        (
            Transform::from_xyz(0.0, 0.92, 0.0),
            KinematicCharacter::default(),
        ),
    );
    finish(&mut app, e);
    assert!(field::<bool>(&app, e, "grounded"));
    assert!(
        field::<Option<box_engine::box_script::mlua::LightUserData>>(&app, e, "ground").is_some()
    );
    assert!((field::<f64>(&app, e, "normal_y") - 1.0).abs() < 1e-3);
    let moved: f64 = field(&app, e, "moved");
    assert!((moved - 1.5).abs() < 0.15, "moved {moved}");
    assert!(field::<bool>(&app, e, "jumped"));
    assert!(field::<f64>(&app, e, "vy") > 1.0);
    assert!(field::<bool>(&app, e, "airborne"));
    let slide_y: f64 = field(&app, e, "slide_y");
    assert!(field::<bool>(&app, e, "slide_grounded"));
    assert!((slide_y - 0.92).abs() < 0.02, "{slide_y}");
    assert!(field::<bool>(&app, e, "no_character"));
}

#[cfg(feature = "dynamics")]
const IMPULSE: &str = r#"
local T = {}
function T.on_start(self)
    local box = world.find("Box")
    wait_frames(60)
    self.sleeping = physics.is_sleeping(box)
    physics.apply_impulse(box, vector.create(0, 0, -20))
    wait_frames(2)
    local linear, angular = physics.velocity(box)
    self.vz = linear.z
    self.has_angular = angular ~= nil
    physics.apply_torque_impulse(box, vector.create(0, 5, 0))
    physics.add_force(box, vector.create(0, 100, 0))
    physics.add_torque(box, vector.create(0, 1, 0))
    physics.apply_impulse_at_point(box, vector.create(1, 0, 0), box:position() + vector.create(0, 0.5, 0))
    wait_frames(2)
    local _, spin = physics.velocity(box)
    self.spin = spin.y
    self.joint = physics.joint_position(box)
    self.done = true
end
return T
"#;

#[cfg(feature = "dynamics")]
const BOX: &str = r#"
local Box = {}
function Box.on_start(self)
    self.hits = 0
end
function Box.on_collision_enter(self, other: Entity, impulse: number)
    self.hits += 1
    self.other = other:name()
    self.impulse = impulse
end
return Box
"#;

#[cfg(feature = "dynamics")]
#[test]
fn forces_body_reads_and_collision_callbacks() {
    let p = Project::new(
        "dynamics",
        &[("scripts/kick.luau", IMPULSE), ("scripts/box.luau", BOX)],
    );
    let mut app = p.app();
    floor(&mut app);
    let b = spawn_script(
        &mut app,
        "Box",
        "scripts/box.luau",
        (
            Transform::from_xyz(0.0, 2.0, 0.0),
            Collider::cuboid(Vec3::ONE),
            RigidBody::dynamic(1.0),
        ),
    );
    let kicker = spawn_script(&mut app, "Kicker", "scripts/kick.luau", ());
    finish(&mut app, kicker);
    assert!(field::<f64>(&app, kicker, "vz") < -5.0);
    assert!(field::<bool>(&app, kicker, "has_angular"));
    assert!(field::<f64>(&app, kicker, "spin") > 0.5);
    assert!(field::<Option<f64>>(&app, kicker, "joint").is_none());
    // It fell onto the floor: one collision, with an impulse.
    assert!(field::<i64>(&app, b, "hits") >= 1);
    assert_eq!(field::<String>(&app, b, "other"), "Floor");
    assert!(field::<f64>(&app, b, "impulse") > 0.0);
}

const ANIMATE: &str = r#"
local T = {}
function T.on_start(self)
    local hero = world.find("Hero")
    wait_until(function() return animation.state(hero) == "Idle" end, 10)
    self.idle = animation.state(hero)
    animation.set_float(hero, "speed", 2)
    animation.set_bool(hero, "flag", true)
    self.speed = animation.float(hero, "speed")
    self.flag = animation.bool(hero, "flag")
    wait_frames(3)
    self.walk = animation.state(hero)
    self.layers = #animation.layers(hero)
    self.weight = animation.layers(hero)[2].weight
    self.transitions = animation.transitions(hero)
    animation.trigger(hero, "jump")
    wait_frames(2)
    self.jump = animation.state(hero)
    wait(1.5)
    self.finished = animation.is_finished(hero)
    self.time = animation.time(hero)
    self.normalized = animation.normalized_time(hero)
    local hand = animation.joint(hero, "LowerArm.R")
    self.hand = hand ~= nil and hand:name()
    self.missing = animation.joint(hero, "Tail")
    -- A plain player: crossfade by hand.
    local dummy = world.find("Dummy")
    animation.play(dummy, "models/character.glb#Animation/Run", 0.4)
    wait_frames(5)
    local l = animation.layers(dummy)
    self.dummy_clip = l[#l].clip
    self.done = true
end
return T
"#;

const GRAPH: &str = r#"{
  "states": [
    { "name": "Idle", "clip": "models/character.glb#Animation/Idle", "speed": 1.0, "looping": true },
    { "name": "Walk", "clip": "models/character.glb#Animation/Walk", "speed": 1.0, "looping": true },
    { "name": "Jump", "clip": "models/character.glb#Animation/Jump", "speed": 1.0, "looping": false }
  ],
  "transitions": [
    { "from": "", "to": "Jump", "duration": 0.1, "exit_time": 0.0, "conditions": [{ "parameter": "jump", "test": "Triggered" }] },
    { "from": "Idle", "to": "Walk", "duration": 0.5, "exit_time": 0.0, "conditions": [{ "parameter": "speed", "test": { "Greater": 0.5 } }] }
  ],
  "start": "Idle"
}"#;

#[test]
fn animation_state_machines_players_and_joints() {
    let p = Project::new(
        "animation",
        &[
            ("scripts/a.luau", ANIMATE),
            ("anim/hero.animgraph.json", GRAPH),
        ],
    );
    let glb = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/animation_assets/models/character.glb");
    std::fs::create_dir_all(p.root.join("assets/models")).unwrap();
    std::fs::copy(glb, p.root.join("assets/models/character.glb")).unwrap();
    let mut app = p.app();
    let prefab = || PrefabInstance {
        scene: AssetRef::path("models/character.glb#Scene"),
    };
    app.world.spawn((
        Uid::new(),
        Name::new("Hero"),
        Transform::default(),
        prefab(),
        AnimationStateMachine::from_asset(AssetRef::path("anim/hero.animgraph.json")),
    ));
    app.world.spawn((
        Uid::new(),
        Name::new("Dummy"),
        Transform::from_xyz(2.0, 0.0, 0.0),
        prefab(),
        AnimationPlayer::new(AssetRef::path("models/character.glb#Animation/Idle")),
    ));
    let e = spawn_script(&mut app, "Driver", "scripts/a.luau", ());
    finish(&mut app, e);
    assert_eq!(field::<String>(&app, e, "idle"), "Idle");
    assert_eq!(field::<f64>(&app, e, "speed"), 2.0);
    assert!(field::<bool>(&app, e, "flag"));
    assert_eq!(field::<String>(&app, e, "walk"), "Walk");
    assert_eq!(field::<i64>(&app, e, "layers"), 2, "a crossfade");
    let w: f64 = field(&app, e, "weight");
    assert!(w > 0.0 && w < 1.0, "{w}");
    assert_eq!(field::<i64>(&app, e, "transitions"), 1);
    assert_eq!(field::<String>(&app, e, "jump"), "Jump");
    assert!(
        field::<bool>(&app, e, "finished"),
        "the one-shot jump ended"
    );
    assert!((field::<f64>(&app, e, "normalized") - 1.0).abs() < 1e-3);
    assert!((field::<f64>(&app, e, "time") - 1.0).abs() < 1e-3);
    assert_eq!(field::<String>(&app, e, "hand"), "LowerArm.R");
    assert!(field::<Option<bool>>(&app, e, "missing").is_none());
    assert!(field::<String>(&app, e, "dummy_clip").ends_with("#Animation/Run"));
}

const MENU: &str = r#"
local T = {}
function T.on_start(self)
    self.clicks = 0
    local menu = ui.add({ kind = "panel", id = "menu", modal = true, anchor = { 0.5, 0.5 },
        children = {
            { kind = "text", id = "title", key = "title", args = { n = 3 } },
            { kind = "button", id = "ok", text = "OK" },
            { kind = "slider", id = "volume", text = "Volume", value = 0.5 },
        } })
    self.menu = menu
    self.found = ui.find("ok") ~= nil and ui.find("ok", menu) == ui.find("ok")
    self.kind = ui.kind("volume")
    self.id = ui.id(ui.find("title"))
    self.children = #ui.children(menu)
    self.parent_is_menu = ui.parent("ok") == menu
    ui.set_value("volume", 0.25)
    self.value = ui.value("volume")
    ui.set_style("ok", { min_width = 300 })
    local ok, err = pcall(ui.set_text, "nope", "x")
    self.unknown = not ok and string.find(tostring(err), "no UI node with id `nope`") ~= nil
    ui.on("click", "ok", function(ev)
        self.clicks += 1
        self.clicked_node = ev.node == ui.find("ok")
        self.clicked_id = ev.id
    end)
    ui.on("change", nil, function(ev) self.changed = ev.value end)
    self.focus_ok = ui.focus("ok")
    self.focused = ui.id(ui.focused())
    wait_frames(2)
    self.captures = ui.captures_input()
    self.title = ui.text("title")
    self.hello = strings.format("greeting", { name = "Ada" })
    self.has = strings.has("greeting")
    strings.set_language("de")
    self.language = strings.language()
    self.german = strings.get("greeting")
    wait_frames(2)
    self.title_de = ui.text("title")
    self.done = true
end
function T.on_destroy(self)
    ui.remove(self.menu)
end
return T
"#;

#[test]
fn ui_documents_setters_getters_events_and_strings() {
    let p = Project::new("ui", &[("scripts/menu.luau", MENU)]);
    let mut app = p.app();
    {
        let s = app.world.resource_mut::<Strings>();
        s.insert("en", "title", "Pick {n}");
        s.insert("de", "title", "Wähle {n}");
        s.insert("en", "greeting", "Hello {name}");
        s.insert("de", "greeting", "Hallo {name}");
    }
    let e = spawn_script(&mut app, "Menu", "scripts/menu.luau", ());
    finish(&mut app, e);
    assert!(field::<bool>(&app, e, "found"));
    assert_eq!(field::<String>(&app, e, "kind"), "slider");
    assert_eq!(field::<String>(&app, e, "id"), "title");
    assert_eq!(field::<i64>(&app, e, "children"), 3);
    assert!(field::<bool>(&app, e, "parent_is_menu"));
    assert_eq!(field::<f64>(&app, e, "value"), 0.25);
    assert!(field::<bool>(&app, e, "unknown"));
    assert!(field::<bool>(&app, e, "focus_ok"));
    assert_eq!(field::<String>(&app, e, "focused"), "ok");
    assert!(field::<bool>(&app, e, "captures"));
    assert_eq!(field::<String>(&app, e, "title"), "Pick 3");
    assert_eq!(field::<String>(&app, e, "hello"), "Hello Ada");
    assert!(field::<bool>(&app, e, "has"));
    assert_eq!(field::<String>(&app, e, "language"), "de");
    assert_eq!(field::<String>(&app, e, "german"), "Hallo {name}");
    assert_eq!(field::<String>(&app, e, "title_de"), "Wähle 3");
    let ui = app.world.resource::<Ui>();
    let ok = ui.find(None, "ok").unwrap();
    assert_eq!(ui.text(ok).as_deref(), Some("OK"));

    // Enter confirms the focused button: the script's handler runs.
    app.world.resource_mut::<Input>().press_key(KeyCode::Enter);
    run_until(&mut app, 5.0, |_| true);
    app.world
        .resource_mut::<Input>()
        .release_key(KeyCode::Enter);
    run_until(&mut app, 5.0, |_| true);
    assert_eq!(field::<i64>(&app, e, "clicks"), 1);
    assert!(field::<bool>(&app, e, "clicked_node"));
    assert_eq!(field::<String>(&app, e, "clicked_id"), "ok");
    // Right on the focused slider: a change event with the value.
    let volume = app.world.resource::<Ui>().find(None, "volume").unwrap();
    app.world.resource_mut::<Ui>().focus(volume);
    app.world.resource_mut::<Input>().press_key(KeyCode::Right);
    run_until(&mut app, 5.0, |_| true);
    app.world
        .resource_mut::<Input>()
        .release_key(KeyCode::Right);
    run_until(&mut app, 5.0, |_| true);
    let changed: f64 = field(&app, e, "changed");
    assert!(changed > 0.25, "{changed}");

    // The handlers and documents go with the instance.
    app.world.despawn(e);
    run_until(&mut app, 5.0, |_| true);
    assert!(app.world.resource::<Ui>().find(None, "menu").is_none());
}

const DATA: &str = r#"
local T = {}
function T.on_start(self)
    wait_until(function() return data.table("data/waves.data.json") ~= nil end, 10)
    local t = data.table("data/waves.data.json")
    self.first = t.waves[1].count
    self.same = rawequal(t, data.table("data/waves.data.json"))
    self.version = data.version("data/waves.data.json")
    self.readonly = not pcall(function() t.waves[1].count = 99 end)
    self.missing = data.table("data/none.data.json") == nil
    self.ready = true
    wait_until(function() return data.version("data/waves.data.json") ~= self.version end, 20)
    self.reloaded = data.table("data/waves.data.json").waves[1].count
    self.done = true
end
return T
"#;

#[test]
fn data_tables_are_cached_read_only_and_reload() {
    let p = Project::new(
        "data",
        &[
            ("scripts/d.luau", DATA),
            (
                "data/waves.data.json",
                r#"{ "waves": [{ "count": 3 }, { "count": 5 }] }"#,
            ),
        ],
    );
    let mut app = p.app();
    let e = spawn_script(&mut app, "Data", "scripts/d.luau", ());
    assert!(run_until(&mut app, 20.0, |app| {
        instance(app, e).is_some_and(|t| t.get::<bool>("ready").unwrap_or(false))
    }));
    assert_eq!(field::<i64>(&app, e, "first"), 3);
    assert!(field::<bool>(&app, e, "same"), "cached per version");
    assert!(field::<bool>(&app, e, "readonly"));
    assert!(field::<bool>(&app, e, "missing"));
    p.write("data/waves.data.json", r#"{ "waves": [{ "count": 7 }] }"#);
    app.world.resource::<AssetServer>().rescan();
    finish(&mut app, e);
    assert_eq!(field::<i64>(&app, e, "reloaded"), 7);
}

const PARTICLES: &str = r#"
local T = {}
function T.on_start(self)
    local fountain = world.find("Fountain")
    wait_frames(20)
    self.running = particles.count(fountain)
    particles.stop(fountain)
    self.stopped = not fountain:get("ParticleEmitter").playing
    particles.start(fountain)
    particles.restart(fountain)
    wait_frames(1)
    self.restarted = particles.count(fountain)
    local burst = particles.burst(vector.create(0, 1, 0), 12, { lifetime = vector.create(0.2, 0.2) })
    wait_frames(2)
    self.burst = particles.count(burst)
    self.burst_name = burst:name()
    wait(0.5)
    self.burst_gone = not burst:is_alive()
    self.done = true
end
return T
"#;

#[test]
fn particles_burst_start_stop_restart() {
    let p = Project::new("particles", &[("scripts/p.luau", PARTICLES)]);
    let mut app = p.app();
    app.world.spawn((
        Name::new("Fountain"),
        Transform::default(),
        ParticleEmitter {
            rate: 60.0,
            ..Default::default()
        },
    ));
    let e = spawn_script(&mut app, "Fx", "scripts/p.luau", ());
    finish(&mut app, e);
    assert!(field::<i64>(&app, e, "running") > 5);
    assert!(field::<bool>(&app, e, "stopped"));
    assert!(
        field::<i64>(&app, e, "restarted") <= 2,
        "cleared by restart"
    );
    assert_eq!(field::<i64>(&app, e, "burst"), 12);
    assert_eq!(field::<String>(&app, e, "burst_name"), "Particle burst");
    assert!(
        field::<bool>(&app, e, "burst_gone"),
        "one-shot bursts despawn"
    );
}

const CAMERA: &str = r#"
local T = {}
function T.on_start(self)
    self.size = camera.screen_size()
    local ahead = camera.world_to_screen(vector.create(0, 0, -10))
    self.centre = ahead
    self.behind = camera.world_to_screen(vector.create(0, 0, 10)) == nil
    local p = vector.create(3, -1, -6)
    local s = camera.world_to_screen(p)
    local origin, direction = camera.screen_ray(s)
    self.miss = vector.magnitude(origin + direction * vector.magnitude(p - origin) - p)
    local o, d = camera.cursor_ray()
    self.no_cursor = o == nil and d == nil
    wait_frames(1)
    local co, cd = camera.cursor_ray()
    self.cursor_dir = cd
    self.done = true
end
return T
"#;

#[test]
fn camera_projects_points_and_casts_screen_and_cursor_rays() {
    let p = Project::new("camera", &[("scripts/c.luau", CAMERA)]);
    let mut app = p.app();
    *app.world.resource_mut::<box_engine::WindowSize>() = box_engine::WindowSize {
        width: 800,
        height: 400,
        scale_factor_milli: 1000,
    };
    app.world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        Camera::default(),
    ));
    let e = spawn_script(&mut app, "Cam", "scripts/c.luau", ());
    // The cursor enters the window after the script's first look.
    run_until(&mut app, 30.0, |app| instance(app, e).is_some());
    app.world.resource_mut::<Input>().cursor_position = Some(Vec2::new(400.0, 200.0));
    finish(&mut app, e);
    let v = |key: &str| {
        let v = field::<box_engine::box_script::mlua::Vector>(&app, e, key);
        Vec3::new(v.x(), v.y(), v.z())
    };
    assert_eq!(v("size"), Vec3::new(800.0, 400.0, 0.0));
    assert!((v("centre") - Vec3::new(400.0, 200.0, 10.0)).length() < 1e-3);
    assert!(field::<bool>(&app, e, "behind"));
    assert!(field::<f64>(&app, e, "miss") < 1e-4);
    assert!(field::<bool>(&app, e, "no_cursor"));
    assert!((v("cursor_dir") - Vec3::NEG_Z).length() < 1e-5);
}

const VISIBILITY: &str = r#"
local T = {}
function T.on_start(self)
    local pool = world.spawn({ name = "Pool", transient = true })
    local ember = world.spawn({
        name = "Ember",
        parent = pool,
        ParticleEmitter = { rate = 120 },
    })
    self.transient = pool:has("Transient") and not ember:has("Transient")
    wait_frames(10)
    self.burning = particles.count(ember)
    self.visible_before = ember:is_visible()
    pool:set_visible(false)
    self.visible_hidden = ember:is_visible()
    self.own_flag = ember:has("Visible")
    self.pool_flag = pool:get("Visible")
    wait_frames(2)
    self.hidden_count = particles.count(ember)
    pool:set_visible(true)
    wait_frames(10)
    self.shown_count = particles.count(ember)
    self.visible_after = ember:is_visible()
    local ok, err = pcall(world.spawn, { transient = 1 })
    self.bad_flag = not ok and string.find(tostring(err), "transient must be a boolean") ~= nil
    ember:despawn()
    local ok2, err2 = pcall(ember.set_visible, ember, false)
    self.dead = not ok2 and string.find(tostring(err2), "no longer exists") ~= nil
    self.done = true
end
return T
"#;

/// `e:set_visible` / `e:is_visible` (`Visible`, inherited; hidden emitters
/// drop their particles) and `world.spawn{ transient = true }` (M8 gap
/// report items 9 and 10).
#[test]
fn visibility_and_transient_spawns() {
    let p = Project::new("visibility", &[("scripts/v.luau", VISIBILITY)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Pooler", "scripts/v.luau", ());
    finish(&mut app, e);
    assert!(field::<bool>(&app, e, "transient"));
    assert!(field::<i64>(&app, e, "burning") > 3);
    assert!(field::<bool>(&app, e, "visible_before"));
    assert!(!field::<bool>(&app, e, "visible_hidden"), "inherited");
    assert!(!field::<bool>(&app, e, "own_flag"));
    assert!(!field::<bool>(&app, e, "pool_flag"));
    assert_eq!(field::<i64>(&app, e, "hidden_count"), 0);
    assert!(field::<i64>(&app, e, "shown_count") > 3);
    assert!(field::<bool>(&app, e, "visible_after"));
    assert!(field::<bool>(&app, e, "bad_flag"));
    assert!(field::<bool>(&app, e, "dead"));
}

const SAVES: &str = r#"
local T = {}
T.properties = { label = { default = "a" } }
function T.on_start(self)
    self.results = {}
    self.counter = 0
    save.on(function(r) table.insert(self.results, r.kind .. ":" .. r.slot) end)
    wait_frames(1)
    -- Set after a wait, and still overridden: a loaded instance gets
    -- on_load once on_start has returned.
    self.phase = "started"
    task.spawn(function() self:run() end)
end
function T.run(self)
    wait_frames(1)
    if self.loaded then
        self.done = true
        return
    end
    local kept = world.spawn({ name = "Coin" })
    self.kept = save.include(kept)
    local skipped = world.spawn({ name = "Ghost" })
    save.exclude(skipped)
    scenes.persist(self.entity)
    local ok, err = pcall(save.save, "bad slot!")
    self.bad_slot = not ok and string.find(tostring(err), "slot names") ~= nil
    local ok2, err2 = pcall(scenes.set_runtime_entities, "sometimes")
    self.bad_mode = not ok2 and string.find(tostring(err2), "keep_all") ~= nil
    self.counter = 41
    save.save("one")
    wait_frames(2)
    self.slots = #save.slots()
    self.first_slot = save.slots()[1].slot
    self.counter = 99
    save.load("one")
    save.load("missing")
end
function T.on_save(self)
    return { counter = self.counter }
end
function T.on_load(self, saved)
    self.counter = saved.counter
    self.loaded = true
    self.loaded_after_start = self.phase == "started"
end
return T
"#;

#[test]
fn save_games_from_scripts_keep_runtime_entities_and_script_state() {
    let p = Project::new("save", &[("scripts/s.luau", SAVES)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Saver", "scripts/s.luau", ());
    let uid = app.world.uid(e).unwrap();
    let saved = run_until(&mut app, 20.0, |app| {
        app.world
            .entity_by_uid(uid)
            .and_then(|e| instance(app, e))
            .is_some_and(|t| t.get::<bool>("loaded").unwrap_or(false))
    });
    assert!(saved, "loaded back");
    run_until(&mut app, 5.0, |_| true);
    let e = app.world.entity_by_uid(uid).unwrap();
    assert_eq!(
        field::<i64>(&app, e, "counter"),
        41,
        "script state restored"
    );
    assert!(
        field::<bool>(&app, e, "loaded_after_start"),
        "on_load ran after on_start returned"
    );
    let results: Vec<String> = field(&app, e, "results");
    assert_eq!(results, ["loaded:one", "failed:missing"]);
    // The kept runtime entity has a uid and came back; the excluded one
    // was left alone.
    let coin = app
        .world
        .find_by_name("Coin")
        .expect("kept entity restored");
    assert!(app.world.has::<Uid>(coin));
    let ghost = app
        .world
        .find_by_name("Ghost")
        .expect("excluded entity left");
    assert!(app.world.has::<SaveExclude>(ghost));
    let file = p.root.join("saves/one.save.json");
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(json["hooks"]["scripts"][uid.to_string()]["counter"], 41);
    assert!(
        json["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.to_string().contains("\"Coin\""))
    );
    assert!(
        !json["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.to_string().contains("\"Ghost\""))
    );
}

#[test]
fn script_bindings_report_their_preconditions() {
    let check = r#"
local T = {}
function T.on_start(self)
    local plain = world.spawn({ name = "Plain" })
    local function msg(f, ...)
        local ok, err = pcall(f, ...)
        return if ok then "ok" else tostring(err)
    end
    self.anim = msg(animation.set_float, plain, "speed", 1)
    self.player = msg(animation.play, plain, "x.glb#Animation/A")
    self.particles = msg(particles.stop, plain)
    self.ui_kind = msg(ui.on, "hover", nil, function() end)
    self.persist = scenes.persist(plain)
    self.done = true
end
return T
"#;
    let p = Project::new("errors", &[("scripts/e.luau", check)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Check", "scripts/e.luau", ());
    finish(&mut app, e);
    assert!(field::<String>(&app, e, "anim").contains("no AnimationStateMachine"));
    assert!(field::<String>(&app, e, "player").contains("no AnimationPlayer"));
    assert!(field::<String>(&app, e, "particles").contains("no ParticleEmitter"));
    assert!(field::<String>(&app, e, "ui_kind").contains("unknown event kind `hover`"));
    assert!(field::<bool>(&app, e, "persist"));
    let plain = app.world.find_by_name("Plain").unwrap();
    assert!(app.world.has::<Persistent>(plain));
}

// ---- Script input latency ---------------------------------------------------

/// W walks at 6 m/s, Space jumps; records the z it sees before the move.
const WALKER: &str = r#"
local T = {}
function T.on_start(self)
    self.steps = 0
end
function T.on_fixed_update(self, dt: number)
    self.steps += 1
    self.seen_z = self.entity:position().z
    character.move(self.entity, vector.create(0, 0, if input.key_down("W") then -6 else 0))
    if input.key_pressed("Space") then
        character.jump(self.entity)
    end
end
return T
"#;

/// The walker as a native FixedUpdate system ordered before
/// `move_characters`, the way native games read input.
struct NativeWalker;

#[derive(Default)]
struct NativeSeenZ(f32);

fn native_walk(world: &mut World) {
    let Some(e) = world.find_by_name("Native") else {
        return;
    };
    let (walk, jump) = {
        let input = world.resource::<Input>();
        (
            input.key_down(KeyCode::W),
            input.key_pressed(KeyCode::Space),
        )
    };
    let z = world.get::<Transform>(e).unwrap().translation.z;
    world.resource_or_default::<NativeSeenZ>().0 = z;
    let c = world.component_mut::<KinematicCharacter>(e).unwrap();
    c.velocity.x = 0.0;
    c.velocity.z = if walk { -6.0 } else { 0.0 };
    if jump {
        c.jump();
    }
}

impl Plugin for NativeWalker {
    fn build(&self, app: &mut App) {
        app.system(Stage::FixedUpdate, native_walk)
            .before("move_characters");
    }
}

/// Script input moves the character in the same fixed step as an equivalent
/// native system ordered before `move_characters`: both characters stay in
/// step frame by frame (within float noise: they stand 6 m apart on the
/// floor), and `on_fixed_update` sees the position the native system sees
/// (the previous step's move). Before scripts ran ahead of the move, the
/// scripted character was one step (0.1 m) behind.
#[test]
fn script_input_moves_the_character_in_the_same_fixed_step_as_native_input() {
    let p = Project::new("latency", &[("scripts/walker.luau", WALKER)]);
    let game = Game::new().with_plugin(NativeWalker);
    let mut world = game.new_world();
    world.insert_resource(box_engine::asset_server(p.root.join("assets")).unwrap());
    let mut app = game.app(world);
    floor(&mut app);
    let scripted = spawn_script(
        &mut app,
        "Scripted",
        "scripts/walker.luau",
        (
            Transform::from_xyz(-3.0, 0.92, 0.0),
            KinematicCharacter::default(),
        ),
    );
    let native = app.world.spawn((
        Name::new("Native"),
        Transform::from_xyz(3.0, 0.92, 0.0),
        KinematicCharacter::default(),
    ));
    assert!(run_until(&mut app, 30.0, |app| {
        instance(app, scripted).is_some_and(|t| t.get::<u32>("steps").unwrap_or(0) >= 10)
    }));
    let pos = |app: &App, e: Entity| app.world.get::<Transform>(e).unwrap().translation;
    let (s0, n0) = (pos(&app, scripted), pos(&app, native));
    assert!((s0.y - n0.y).abs() < 1e-6 && (s0.z - n0.z).abs() < 1e-6);

    let step = |app: &mut App| app.run_frames(1, 1.0 / 60.0);
    let in_step = |app: &App, what: &str| {
        let (s, n) = (pos(app, scripted) - s0, pos(app, native) - n0);
        assert!(
            (s - n).length() < 1e-4,
            "{what}: scripted moved {s}, native {n}"
        );
        let seen = field::<f32>(app, scripted, "seen_z") - s0.z;
        let native_seen = app.world.resource::<NativeSeenZ>().0 - n0.z;
        assert!(
            (seen - native_seen).abs() < 1e-4,
            "{what}: on_fixed_update saw z {seen}, native {native_seen}"
        );
        s
    };

    // The frame W goes down, both move one step (0.1 m at 6 m/s).
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    step(&mut app);
    let first = in_step(&app, "first step");
    assert!((first.z + 0.1).abs() < 1e-4, "first step moved {first}");
    for i in 0..59 {
        step(&mut app);
        in_step(&app, &format!("walking, frame {i}"));
    }
    // The frame W goes up, both stop at once.
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    step(&mut app);
    let stopped = in_step(&app, "release");
    step(&mut app);
    assert_eq!(in_step(&app, "standing"), stopped);
    assert!((stopped.z + 6.0).abs() < 1e-3, "walked {stopped}");

    // The frame Space goes down, both leave the ground.
    app.world.resource_mut::<Input>().press_key(KeyCode::Space);
    step(&mut app);
    app.world
        .resource_mut::<Input>()
        .release_key(KeyCode::Space);
    assert!(in_step(&app, "jump").y > 0.05);
    for i in 0..60 {
        step(&mut app);
        in_step(&app, &format!("airborne, frame {i}"));
    }
    assert_eq!(app.world.resource::<ScriptRuntime>().stats().errors, 0);

    // The order that gives this: the rigid-body step, native input, scripts,
    // then the character move.
    app.check_order().unwrap();
    let fixed = app.system_names(Stage::FixedUpdate);
    let at = |n: &str| {
        fixed
            .iter()
            .position(|s| s.ends_with(n))
            .unwrap_or_else(|| panic!("no {n} in {fixed:?}"))
    };
    // Without the `dynamics` feature there is no rigid-body step.
    #[cfg(feature = "dynamics")]
    assert!(at("step_dynamics") < at("native_walk"), "{fixed:?}");
    assert!(at("native_walk") < at("script_fixed_update"), "{fixed:?}");
    assert!(
        at("script_fixed_update") < at("move_characters"),
        "{fixed:?}"
    );
}
