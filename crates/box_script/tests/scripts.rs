//! Script runtime tests: lifecycle, properties, hot reload, errors,
//! sandbox, waits, batch queries, events, triggers, saved state and
//! determinism. Each builds a temporary project with `.luau` files and runs
//! the engine's `Game` app headlessly.

mod common;

use box_engine::box_script::mlua::Table;
use box_engine::box_script::{ScriptRuntime, ScriptValue, load_script_state, save_script_state};
use box_engine::prelude::*;
use common::*;

const LIFECYCLE: &str = r#"
local M = {}
function M.on_start(self)
    self.log = { "start" }
end
function M.on_fixed_update(self, dt)
    table.insert(self.log, "fixed")
end
function M.on_update(self, dt)
    table.insert(self.log, "update")
end
function M.on_destroy(self)
    local probe = world.find("Probe")
    probe:set_name("destroyed:" .. #self.log)
end
return M
"#;

fn log_of(app: &App, e: Entity) -> Vec<String> {
    let t: Option<Table> = field(app, e, "log");
    t.map(|t| t.sequence_values::<String>().map(Result::unwrap).collect())
        .unwrap_or_default()
}

#[test]
fn lifecycle_runs_in_order_and_on_destroy_runs_on_despawn() {
    let p = Project::new("lifecycle", &[("scripts/life.luau", LIFECYCLE)]);
    let mut app = p.app();
    let probe = app.world.spawn((Name::new("Probe"),));
    let e = spawn_script(&mut app, "Life", Script::new("scripts/life.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    let log = log_of(&app, e);
    assert_eq!(log[0], "start");
    // Every frame at 60 Hz: one fixed step, then update.
    app.run_frames(3, 1.0 / 60.0);
    let log = log_of(&app, e);
    let tail: Vec<&str> = log[log.len() - 6..].iter().map(String::as_str).collect();
    assert_eq!(
        tail,
        ["fixed", "update", "fixed", "update", "fixed", "update"]
    );
    let n = log.len();
    app.world.despawn(e);
    app.run_frames(1, 1.0 / 60.0);
    assert_eq!(stats(&app).instances, 0);
    assert_eq!(
        app.world.get::<Name>(probe).unwrap().as_str(),
        format!("destroyed:{n}")
    );
}

#[test]
fn properties_have_defaults_overrides_and_survive_scene_save_and_load() {
    let src = r#"
local M = {}
M.properties = {
    speed = { type = "number", default = 2, min = 0, max = 10 },
    label = { default = "gem" },
    up = { default = vector.create(0, 1, 0) },
}
function M.on_start(self)
    self.seen = string.format("%g %s %g", self.props.speed, self.props.label, self.props.up.y)
end
return M
"#;
    let p = Project::new("props", &[("scripts/p.luau", src)]);
    let mut app = p.app();
    let script = Script::new("scripts/p.luau").with("speed", ScriptValue::Number(7.5));
    let e = spawn_script(&mut app, "P", script.clone());
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    assert_eq!(
        field::<String>(&app, e, "seen").as_deref(),
        Some("7.5 gem 1")
    );
    // Through a scene file and back.
    let bytes = box_engine::box_core::scene::save(&app.world).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("\"Script\""), "{text}");
    assert!(text.contains("\"speed\""), "{text}");
    let loaded = box_engine::box_core::scene::load(&bytes, app.world.registry().clone()).unwrap();
    let back = loaded.find_by_name("P").unwrap();
    assert_eq!(*loaded.get::<Script>(back).unwrap(), script);
    // Inspector edits during play reach `self.props` within 16 frames.
    let mut edited = script.clone();
    edited.set("label", ScriptValue::String("coin".into()));
    app.world.insert_one(e, edited).unwrap();
    app.run_frames(17, 1.0 / 60.0);
    let props: Table = field(&app, e, "props").unwrap();
    assert_eq!(props.get::<String>("label").unwrap(), "coin");
}

#[test]
fn hot_reload_during_play_keeps_state_and_playing() {
    let v1 = r#"
local M = {}
function M.on_start(self) self.count = 0 end
function M.on_update(self, dt)
    self.count += 1
    self.entity:set_name("v1")
end
return M
"#;
    let v2 = r#"
local M = {}
function M.on_start(self) self.count = 1000 end
function M.on_update(self, dt)
    self.count += 1
    self.entity:set_name("v2")
end
function M.on_reload(self)
    self.reloaded = true
end
return M
"#;
    let p = Project::new("reload", &[("scripts/r.luau", v1)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "R", Script::new("scripts/r.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    app.run_frames(10, 1.0 / 60.0);
    let before: i64 = field(&app, e, "count").unwrap();
    assert!(before >= 10);
    assert_eq!(app.world.get::<Name>(e).unwrap().as_str(), "v1");
    // Edit the file while playing.
    p.write("scripts/r.luau", v2);
    let id = server(&app).id_of("scripts/r.luau").unwrap();
    server(&app).reimport(id).unwrap();
    assert!(run_until(&mut app, 5.0, |app| app
        .world
        .get::<Name>(e)
        .is_some_and(|n| n.as_str() == "v2")));
    let after: i64 = field(&app, e, "count").unwrap();
    // State kept: the counter continued instead of restarting at 1000.
    assert!(after > before && after < 1000, "{before} -> {after}");
    assert_eq!(field::<bool>(&app, e, "reloaded"), Some(true));
    assert_eq!(stats(&app).reloads, 1);
}

#[test]
fn a_syntax_error_on_reload_keeps_the_previous_version() {
    let good = "local M = {} function M.on_update(self) self.entity:set_name('good') end return M";
    let p = Project::new("syntax", &[("scripts/s.luau", good)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "S", Script::new("scripts/s.luau"));
    assert!(run_until(&mut app, 5.0, |app| app
        .world
        .get::<Name>(e)
        .is_some_and(|n| n.as_str() == "good")));
    p.write(
        "scripts/s.luau",
        "local M = {\nfunction M.on_update(self)\n",
    );
    let id = server(&app).id_of("scripts/s.luau").unwrap();
    server(&app).reimport(id).unwrap();
    assert!(run_until(&mut app, 5.0, |app| stats(app).errors > 0));
    app.world.insert_one(e, Name::new("renamed")).unwrap();
    app.run_frames(2, 1.0 / 60.0);
    assert_eq!(app.world.get::<Name>(e).unwrap().as_str(), "good");
    assert_eq!(stats(&app).disabled, 0);
}

#[test]
fn a_runtime_error_names_file_and_line_and_disables_until_reload() {
    let bad = "local M = {}\nfunction M.on_update(self, dt)\n    local t = nil\n    self.entity:set_name(t.name)\nend\nreturn M\n";
    let p = Project::new("error", &[("scripts/bad.luau", bad)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Bad", Script::new("scripts/bad.luau"));
    let ok = spawn_script(&mut app, "Other", Script::new("scripts/bad.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).disabled == 2));
    let reason = app
        .world
        .resource::<ScriptRuntime>()
        .disabled_reason(e)
        .unwrap();
    assert!(reason.starts_with("scripts/bad.luau:4:"), "{reason}");
    assert!(reason.contains("attempt to index nil"), "{reason}");
    // The game keeps running.
    app.run_frames(5, 1.0 / 60.0);
    // Fixing the file enables both again.
    p.write(
        "scripts/bad.luau",
        "local M = {}\nfunction M.on_update(self, dt)\n    self.entity:set_name('fixed')\nend\nreturn M\n",
    );
    let id = server(&app).id_of("scripts/bad.luau").unwrap();
    server(&app).reimport(id).unwrap();
    assert!(run_until(&mut app, 5.0, |app| app
        .world
        .get::<Name>(ok)
        .is_some_and(|n| n.as_str() == "fixed")));
    assert_eq!(stats(&app).disabled, 0);
}

#[test]
fn engine_function_errors_point_at_the_calling_line() {
    let src = "local M = {}\nfunction M.on_start(self)\n\n    self.entity:get('Transfrom')\nend\nreturn M\n";
    let p = Project::new("api_error", &[("scripts/api.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "A", Script::new("scripts/api.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).disabled == 1));
    let reason = app
        .world
        .resource::<ScriptRuntime>()
        .disabled_reason(e)
        .unwrap();
    assert!(reason.starts_with("scripts/api.luau:4:"), "{reason}");
    assert!(reason.contains("did you mean `Transform`"), "{reason}");
}

#[test]
fn the_sandbox_has_no_files_processes_or_outside_requires() {
    let src = r#"
local M = {}
function M.on_start(self)
    self.io = io == nil
    self.execute = os.execute == nil
    self.getenv = os.getenv == nil
    self.loadstring = loadstring == nil
    self.dofile = dofile == nil
    local ok, err = pcall(require, "../../secret")
    self.err = tostring(err)
    self.outside = (not ok) and string.find(tostring(err), "inside the project") ~= nil
    local ok2 = pcall(function() string.upper = nil end)
    self.stdlib_locked = not ok2
    local util = require("./util")
    self.util = util.answer
end
return M
"#;
    let p = Project::new(
        "sandbox",
        &[
            ("scripts/s.luau", src),
            ("scripts/util.luau", "return { answer = 42 }"),
        ],
    );
    let mut app = p.app();
    let e = spawn_script(&mut app, "S", Script::new("scripts/s.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    let err: String = field(&app, e, "err").unwrap_or_default();
    for key in [
        "io",
        "execute",
        "getenv",
        "loadstring",
        "dofile",
        "outside",
        "stdlib_locked",
    ] {
        assert_eq!(field::<bool>(&app, e, key), Some(true), "{key} ({err})");
    }
    assert_eq!(field::<i64>(&app, e, "util"), Some(42));
}

#[test]
fn waits_resume_on_time_frames_and_conditions() {
    let src = r#"
local M = {}
function M.on_start(self)
    self.stage = 0
    local waited = wait(0.5)
    self.waited = waited
    self.stage = 1
    wait_frames(3)
    self.stage = 2
    local met = wait_until(function() return self.go end, 10)
    self.met = met
    self.stage = 3
    task.spawn(function()
        wait(0.1)
        self.child = true
    end)
end
function M.on_update(self) self.updates = (self.updates or 0) + 1 end
return M
"#;
    let p = Project::new("waits", &[("scripts/w.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "W", Script::new("scripts/w.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    assert_eq!(field::<i64>(&app, e, "stage"), Some(0));
    let stage = |app: &App| field::<i64>(app, e, "stage").unwrap();
    // wait(0.5) at 60 Hz: 30 frames.
    let mut frames = 0;
    while stage(&app) == 0 && frames < 100 {
        app.run_frames(1, 1.0 / 60.0);
        frames += 1;
    }
    assert!((30..=31).contains(&frames), "{frames} frames");
    let waited: f64 = field(&app, e, "waited").unwrap();
    assert!((0.5..0.52).contains(&waited), "{waited}");
    // wait_frames(3).
    app.run_frames(2, 1.0 / 60.0);
    assert_eq!(stage(&app), 1);
    app.run_frames(1, 1.0 / 60.0);
    assert_eq!(stage(&app), 2);
    app.run_frames(5, 1.0 / 60.0);
    assert_eq!(field::<i64>(&app, e, "stage"), Some(2), "waits for self.go");
    let this = app.world.resource::<ScriptRuntime>().instance(e).unwrap();
    this.set("go", true).unwrap();
    app.run_frames(1, 1.0 / 60.0);
    assert_eq!(field::<i64>(&app, e, "stage"), Some(3));
    assert_eq!(field::<bool>(&app, e, "met"), Some(true));
    app.run_frames(8, 1.0 / 60.0);
    assert_eq!(field::<bool>(&app, e, "child"), Some(true));
    // Updates ran all along.
    assert!(field::<i64>(&app, e, "updates").unwrap() > 40);
}

#[test]
fn an_endless_loop_is_interrupted_by_the_watchdog() {
    let src = "local M = {}\nfunction M.on_update(self)\n    local n = 0\n    while true do n += 1 end\nend\nreturn M\n";
    let p = Project::new("watchdog", &[("scripts/loop.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Loop", Script::new("scripts/loop.luau"));
    // Start it, then shorten the limit before its first update.
    assert!(run_until(&mut app, 5.0, |app| app
        .world
        .contains_resource::<ScriptRuntime>()));
    app.world
        .resource::<ScriptRuntime>()
        .set_watchdog(Some(std::time::Duration::from_millis(100)));
    let start = std::time::Instant::now();
    assert!(run_until(&mut app, 5.0, |app| stats(app).disabled == 1));
    assert!(start.elapsed().as_secs_f32() < 2.0);
    let reason = app
        .world
        .resource::<ScriptRuntime>()
        .disabled_reason(e)
        .unwrap();
    assert!(reason.contains("endless loop"), "{reason}");
    assert!(reason.starts_with("scripts/loop.luau:4"), "{reason}");
    app.run_frames(3, 1.0 / 60.0);
}

#[test]
fn wait_outside_a_task_is_an_error() {
    let src = "local M = {}\nfunction M.on_update(self)\n    wait(1)\nend\nreturn M\n";
    let p = Project::new("wait_err", &[("scripts/w.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "W", Script::new("scripts/w.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).disabled == 1));
    let reason = app
        .world
        .resource::<ScriptRuntime>()
        .disabled_reason(e)
        .unwrap();
    assert!(reason.contains("scripts/w.luau:3"), "{reason}");
    assert!(
        reason.contains("works only in on_start or in a task.spawn"),
        "{reason}"
    );
}

#[test]
fn batch_queries_and_spawned_entities_are_correct() {
    let src = r#"
local M = {}
function M.on_start(self)
    for i = 1, 50 do
        world.spawn({
            name = "Coin " .. i,
            Transform = { translation = vector.create(i, 0, 0) },
            Primitive = { shape = { Sphere = { radius = 0.25 } } },
        })
    end
end
function M.on_update(self, dt)
    local sum = 0
    for _, e in world.query("Transform", "Primitive") do
        sum += e:position().x
        e:translate(vector.create(0, 1, 0))
    end
    self.sum = sum
    self.count = world.count("Primitive")
    local names = 0
    world.each({ "Name", "Primitive" }, function(e, name, primitive)
        if string.sub(name, 1, 4) == "Coin" and primitive.shape.Sphere then
            names += 1
        end
    end)
    self.each = names
end
return M
"#;
    let p = Project::new("batch", &[("scripts/b.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "Manager", Script::new("scripts/b.luau"));
    assert!(run_until(&mut app, 5.0, |app| field::<i64>(
        app, e, "count"
    )
    .is_some()));
    assert_eq!(field::<i64>(&app, e, "count"), Some(50));
    assert_eq!(field::<i64>(&app, e, "sum"), Some((1..=50).sum::<i64>()));
    assert_eq!(field::<i64>(&app, e, "each"), Some(50));
    let ys: Vec<f32> = app
        .world
        .query::<(&Transform, &Primitive)>()
        .iter()
        .map(|(t, _)| t.translation.y)
        .collect();
    assert_eq!(ys.len(), 50);
    let frames = ys[0];
    assert!(frames >= 1.0 && ys.iter().all(|y| *y == frames), "{ys:?}");
}

#[test]
fn component_values_are_copies_and_set_patches() {
    let src = r#"
local M = {}
function M.on_start(self)
    local t = self.entity:get("Transform")
    t.translation = vector.create(9, 9, 9)
    self.unchanged = self.entity:position().x == 0
    self.entity:set("Transform", { translation = vector.create(1, 2, 3) })
    self.entity:set("Transform.scale", vector.create(2, 2, 2))
    self.y = self.entity:get("Transform.translation").y
    self.entity:add("StandardMaterial", { base_color = Color.rgb(1, 0, 0), roughness = 0.25 })
    self.has = self.entity:has("StandardMaterial")
    self.rough = self.entity:get("StandardMaterial").roughness
    self.env = world.resource("Environment") ~= nil
    world.set_resource("Environment", { exposure = 2 })
    self.missing = self.entity:get("PointLight") == nil
    self.rot = tostring(Quat.from_rotation_y(math.pi / 2) * vector.create(0, 0, -1))
end
return M
"#;
    let p = Project::new("copies", &[("scripts/c.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "C", Script::new("scripts/c.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    assert_eq!(stats(&app).disabled, 0);
    assert_eq!(field::<bool>(&app, e, "unchanged"), Some(true));
    assert_eq!(field::<f64>(&app, e, "y"), Some(2.0));
    let t = *app.world.get::<Transform>(e).unwrap();
    assert_eq!(t.translation, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(t.scale, Vec3::splat(2.0));
    assert_eq!(field::<bool>(&app, e, "has"), Some(true));
    assert_eq!(field::<f64>(&app, e, "rough"), Some(0.25));
    assert_eq!(
        app.world.get::<StandardMaterial>(e).unwrap().base_color,
        Color::linear(1.0, 0.0, 0.0)
    );
    assert_eq!(field::<bool>(&app, e, "env"), Some(true));
    assert_eq!(app.world.resource::<Environment>().exposure, 2.0);
    assert_eq!(field::<bool>(&app, e, "missing"), Some(true));
    let rot: String = field(&app, e, "rot").unwrap();
    let x: f64 = rot.split(',').next().unwrap().trim().parse().unwrap();
    assert!((x + 1.0).abs() < 1e-5, "{rot}");
}

#[test]
fn signals_and_registry_events_reach_handlers() {
    let sender = r#"
local M = {}
function M.on_update(self)
    if not self.sent then
        self.sent = true
        events.send("opened", { door = 3 })
    end
end
return M
"#;
    let listener = r#"
local M = {}
function M.on_start(self)
    self.got = 0
    events.on("opened", function(ev) self.got += ev.door end)
end
return M
"#;
    let p = Project::new(
        "signals",
        &[
            ("scripts/send.luau", sender),
            ("scripts/listen.luau", listener),
        ],
    );
    let mut app = p.app();
    let l = spawn_script(&mut app, "L", Script::new("scripts/listen.luau"));
    spawn_script(&mut app, "S", Script::new("scripts/send.luau"));
    assert!(run_until(&mut app, 5.0, |app| field::<i64>(app, l, "got") == Some(3)));
    app.run_frames(5, 1.0 / 60.0);
    assert_eq!(field::<i64>(&app, l, "got"), Some(3), "delivered once");
}

#[test]
fn registry_events_are_sent_to_rust_and_to_script_handlers() {
    let src = r#"
local M = {}
function M.on_start(self)
    self.heard = 0
    events.on("PlaySound", function(ev)
        self.heard += 1
        self.clip = ev.clip
        self.volume = ev.volume
    end)
end
function M.on_update(self)
    if not self.sent then
        self.sent = true
        events.send("PlaySound", { clip = "audio/none.wav", volume = 0.5, position = vector.create(1, 2, 3) })
    end
end
return M
"#;
    let p = Project::new("registry_events", &[("scripts/e.luau", src)]);
    let mut app = p.app();
    app.world
        .insert_resource(box_engine::box_audio::AudioOutput::Silent);
    let mut cursor = EventCursor::<PlaySound>::default();
    let e = spawn_script(&mut app, "E", Script::new("scripts/e.luau"));
    let mut seen = Vec::new();
    assert!(run_until(&mut app, 5.0, |app| {
        seen.extend(
            cursor
                .read(app.world.resource::<Events<PlaySound>>())
                .cloned(),
        );
        field::<i64>(app, e, "heard") == Some(1)
    }));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].clip, AssetRef::path("audio/none.wav"));
    assert_eq!(seen[0].volume, 0.5);
    assert_eq!(seen[0].position, Some(Vec3::new(1.0, 2.0, 3.0)));
    assert_eq!(
        field::<String>(&app, e, "clip").as_deref(),
        Some("audio/none.wav")
    );
    assert_eq!(field::<f64>(&app, e, "volume"), Some(0.5));
}

#[test]
fn trigger_callbacks_get_the_other_entity() {
    let gem = r#"
local M = {}
function M.on_trigger_enter(self, other)
    other:set_name(other:name() .. " got " .. self.entity:name())
    self.entity:despawn()
end
return M
"#;
    let mover = r#"
local M = {}
function M.on_update(self, dt)
    self.entity:translate(vector.create(0, 0, -5 * dt))
end
return M
"#;
    let p = Project::new(
        "trigger",
        &[("scripts/gem.luau", gem), ("scripts/mover.luau", mover)],
    );
    let mut app = p.app();
    let gem = app.world.spawn((
        Uid::new(),
        Name::new("Gem"),
        Transform::from_xyz(0.0, 0.0, -3.0),
        Collider::sphere(0.5).trigger(),
        Script::new("scripts/gem.luau"),
    ));
    let player = app.world.spawn((
        Uid::new(),
        Name::new("Player"),
        Transform::default(),
        Collider::sphere(0.5),
        Script::new("scripts/mover.luau"),
    ));
    assert!(run_until(&mut app, 8.0, |app| !app.world.contains(gem)));
    assert!(app.world.contains(player));
    assert_eq!(
        app.world.get::<Name>(player).unwrap().as_str(),
        "Player got Gem"
    );
    // The removal reaches the runtime at its next stage.
    app.run_frames(1, 1.0 / 60.0);
    assert_eq!(stats(&app).instances, 1);
}

#[test]
fn script_state_saves_and_loads_through_hooks() {
    let src = r#"
local M = {}
function M.on_start(self) self.score = 0 end
function M.on_update(self) self.score += 1 end
function M.on_save(self)
    return { score = self.score, at = self.entity:position(), friend = world.find("Friend") }
end
function M.on_load(self, data)
    self.score = data.score
    self.entity:set_position(data.at)
    self.friend = data.friend
end
return M
"#;
    let p = Project::new("state", &[("scripts/st.luau", src)]);
    let mut app = p.app();
    let friend = app.world.spawn((Uid::new(), Name::new("Friend")));
    let e = spawn_script(&mut app, "St", Script::new("scripts/st.luau"));
    app.world.component_mut::<Transform>(e).unwrap().translation = Vec3::new(4.0, 5.0, 6.0);
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    app.run_frames(9, 1.0 / 60.0);
    let saved = save_script_state(&mut app.world);
    let uid = app.world.uid(e).unwrap().to_string();
    let entry = &saved[&uid];
    let score = entry["score"].as_i64().unwrap();
    assert!(score >= 10, "{saved}");
    assert_eq!(entry["at"]["$vector"][0], 4.0);
    assert_eq!(
        entry["friend"]["$entity"],
        app.world.uid(friend).unwrap().to_string()
    );
    // Change the world, then load the saved state back.
    app.world.component_mut::<Transform>(e).unwrap().translation = Vec3::ZERO;
    app.run_frames(5, 1.0 / 60.0);
    assert_eq!(load_script_state(&mut app.world, &saved), 1);
    assert_eq!(field::<i64>(&app, e, "score"), Some(score));
    assert_eq!(
        app.world.get::<Transform>(e).unwrap().translation,
        Vec3::new(4.0, 5.0, 6.0)
    );
    let this = app.world.resource::<ScriptRuntime>().instance(e).unwrap();
    let f: box_engine::box_script::mlua::Value = this.get("friend").unwrap();
    assert_eq!(box_engine::box_script::convert::to_entity(&f), Some(friend));
}

fn fixed_run(hz: f32) -> (i64, f32) {
    let src = r#"
local M = {}
function M.on_start(self) self.steps = 0 self.v = 0 end
function M.on_fixed_update(self, dt)
    self.steps += 1
    self.v += 9.81 * dt
    self.entity:translate(vector.create(0, -self.v * dt, 0))
end
return M
"#;
    let p = Project::new("fixed", &[("scripts/f.luau", src)]);
    let mut app = p.app();
    let e = spawn_script(&mut app, "F", Script::new("scripts/f.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    let steps0: i64 = field(&app, e, "steps").unwrap();
    let y0 = app.world.get::<Transform>(e).unwrap().translation.y;
    let frames = hz as usize;
    app.run_frames(frames, 1.0 / hz);
    let steps = field::<i64>(&app, e, "steps").unwrap() - steps0;
    (
        steps,
        app.world.get::<Transform>(e).unwrap().translation.y - y0,
    )
}

#[test]
fn fixed_update_is_deterministic_at_any_frame_rate() {
    let (s60, y60) = fixed_run(60.0);
    for hz in [30.0, 144.0] {
        let (s, y) = fixed_run(hz);
        assert!((s - s60).abs() <= 1, "{hz} Hz: {s} vs {s60}");
        if s == s60 {
            assert_eq!(y, y60, "{hz} Hz");
        }
    }
    // Two identical runs agree exactly.
    assert_eq!(fixed_run(60.0), (s60, y60));
}

#[test]
fn random_numbers_repeat_between_runs() {
    let src =
        "local M = {} function M.on_start(self) self.r = math.random(1, 1000000) end return M";
    let run = || {
        let p = Project::new("random", &[("scripts/r.luau", src)]);
        let mut app = p.app();
        let e = spawn_script(&mut app, "R", Script::new("scripts/r.luau"));
        assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
        field::<i64>(&app, e, "r").unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn worlds_without_scripts_never_create_a_vm() {
    let p = Project::new("none", &[]);
    let mut app = p.app();
    app.world.spawn((Transform::default(),));
    app.run_frames(5, 1.0 / 60.0);
    assert!(!app.world.contains_resource::<ScriptRuntime>());
}

/// `ScriptPlugin` orders `on_fixed_update` before `move_characters`, and
/// brings the character system along, so it also orders cleanly in a bare
/// app (no unknown-name error putting the stage back in insertion order).
#[test]
fn script_plugin_runs_fixed_scripts_before_characters_in_any_app() {
    let mut app = App::new();
    app.add_plugin(&box_engine::box_script::ScriptPlugin);
    app.add_plugin(&CharacterPlugin);
    app.check_order().unwrap();
    let fixed = app.system_names(Stage::FixedUpdate);
    let at = |n: &str| fixed.iter().position(|s| s.ends_with(n)).unwrap();
    assert_eq!(fixed.len(), 2, "{fixed:?}");
    assert!(
        at("script_fixed_update") < at("move_characters"),
        "{fixed:?}"
    );
}

#[test]
fn game_definitions_include_engine_apis_and_types() {
    let defs = box_engine::Game::new().script_definitions();
    assert!(defs.contains("declare scenes: {"));
    assert!(defs.contains("type Script = {"));
    assert!(defs.contains("type Collider = {"));
    // Script rules: the intent, the output and every component by name.
    assert!(defs.contains("type ScriptIntent = {"));
    assert!(defs.contains("type RuleIntent = {"));
    assert!(defs.contains("type RuleOut = {"));
    assert!(defs.contains("    Sweep: SweepPatch?,"));
    assert_eq!(defs, box_engine::Game::new().script_definitions());
}

/// `e:look_at(target)` takes a point in the world, also for an entity under a
/// moved and turned parent: its world forward then points at the target.
#[test]
fn look_at_aims_at_a_world_point_under_a_turned_parent() {
    let src = r#"
local M = {}
function M.on_start(self)
    self.entity:look_at(vector.create(10, 0, 5))
end
return M
"#;
    let p = Project::new("look_at", &[("scripts/aim.luau", src)]);
    let mut app = p.app();
    let parent = app.world.spawn((
        Uid::new(),
        Name::new("Turret base"),
        Transform::from_translation(Vec3::new(4.0, 0.0, -3.0))
            .with_rotation(Quat::from_rotation_y(1.1)),
    ));
    let e = spawn_script(&mut app, "Barrel", Script::new("scripts/aim.luau"));
    app.world.set_parent(e, Some(parent)).unwrap();
    *app.world.get_mut::<Transform>(e).unwrap() =
        Transform::from_translation(Vec3::new(1.0, 0.5, 0.0));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 1));
    app.update();
    let global = box_engine::box_core::transform::compute_global(&app.world, e);
    let (_, rotation, position) = global.to_scale_rotation_translation();
    let forward = rotation * Vec3::NEG_Z;
    let wanted = (Vec3::new(10.0, 0.0, 5.0) - position).normalize();
    assert!(
        forward.dot(wanted) > 0.9999,
        "forward {forward:?}, wanted {wanted:?}"
    );
}
