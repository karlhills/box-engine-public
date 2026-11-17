//! The M9 API review's script API changes: new names (`is_` predicates,
//! `scenes.load_additive`, `save.include`, `save.off`,
//! `input.mouse_released`), deprecated aliases that keep working and warn
//! once with the calling line, `ui.on(kind, handler)`, `Color.hex` with
//! alpha, and particle reads that fail on non-emitters. The only test in
//! its binary: it captures logs through the process-wide subscriber.

use std::path::PathBuf;
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_script::ScriptRuntime;
use box_engine::log_capture;
use box_engine::prelude::*;

const SCRIPT: &str = r##"
local T = {}
function T.on_start(self)
    wait_frames(2)
    -- New names.
    self.paused = time.is_paused()
    self.locked = input.is_cursor_locked()
    self.released = input.mouse_released("Left")
    -- Deprecated aliases still work (and warn once each).
    self.old_paused = time.paused()
    self.old_paused_again = time.paused()
    self.old_locked = input.cursor_locked()
    -- save.include / keep, save.off.
    local coin = world.spawn({ name = "Coin" })
    self.included = save.include(coin)
    local id = save.on(function() end)
    self.off = save.off(id) and not save.off(id)
    -- ui.on without an id.
    ui.add({ kind = "button", id = "b" })
    self.sub = ui.on("click", function(_) end) > 0
    -- Color.hex with alpha.
    local c = Color.hex("#ff000080")
    self.alpha = c.a
    self.red = c.r
    -- Particle reads: a despawned burst is finished; a non-emitter fails.
    local burst = particles.burst(vector.create(0, 0, 0), 4)
    burst:despawn()
    self.burst_done = particles.is_finished(burst) and particles.count(burst) == 0
    local ok, err = pcall(particles.is_finished, self.entity)
    self.not_emitter = not ok and string.find(tostring(err), "no ParticleEmitter") ~= nil
    self.done = true
end
return T
"##;

#[test]
fn renamed_api_and_deprecated_aliases() {
    box_engine::init_logging();
    log_capture::enable();
    let root = std::env::temp_dir().join(format!("box_api_review_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let scripts = root.join("assets/scripts");
    std::fs::create_dir_all(&scripts).unwrap();
    std::fs::write(scripts.join("t.luau"), SCRIPT).unwrap();
    let game = Game::new();
    let mut world = game.new_world();
    world.insert_resource(box_engine::asset_server(root.join("assets")).unwrap());
    let mut app = game.app(world);
    let e = app.world.spawn((
        Uid::new(),
        Name::new("T"),
        Transform::default(),
        Script::new("scripts/t.luau"),
    ));
    let seq = log_capture::next_seq();
    let start = std::time::Instant::now();
    let field = |app: &App, key: &str| -> Option<box_engine::box_script::mlua::Value> {
        app.world
            .get_resource::<ScriptRuntime>()?
            .instance(e)?
            .get(key)
            .ok()
    };
    while start.elapsed() < Duration::from_secs(20) {
        let s = app.world.resource::<AssetServer>().clone();
        s.update_until_idle(&mut app.world, Duration::from_millis(50));
        app.run_frames(1, 1.0 / 60.0);
        if matches!(
            field(&app, "done"),
            Some(box_engine::box_script::mlua::Value::Boolean(true))
        ) {
            break;
        }
    }
    let rt = app.world.resource::<ScriptRuntime>();
    assert_eq!(rt.stats().errors, 0, "{:?}", rt.disabled_reason(e));
    let get = |key: &str| field(&app, key).unwrap_or_else(|| panic!("self.{key}"));
    let yes = |key: &str| assert_eq!(get(key).as_boolean(), Some(true), "self.{key}");
    let no = |key: &str| assert_eq!(get(key).as_boolean(), Some(false), "self.{key}");
    no("paused");
    no("locked");
    no("released");
    no("old_paused");
    no("old_locked");
    yes("included");
    yes("off");
    yes("sub");
    yes("burst_done");
    yes("not_emitter");
    // `include` adds Saveable (a runtime-only marker: not a script type).
    let coin = app.world.find_by_name("Coin").unwrap();
    assert!(app.world.has::<Saveable>(coin));
    let alpha = get("alpha").as_f64().unwrap();
    assert!((alpha - 128.0 / 255.0).abs() < 1e-3, "{alpha}");
    let red = match get("red") {
        box_engine::box_script::mlua::Value::Integer(i) => i as f64,
        other => other.as_f64().unwrap(),
    };
    assert!((red - 1.0).abs() < 1e-6, "{red}");

    let warnings: Vec<String> = log_capture::read(seq, 4096)
        .entries
        .into_iter()
        .filter(|l| l.level == "warn" && l.message.contains("is deprecated"))
        .map(|l| l.message)
        .collect();
    assert_eq!(warnings.len(), 2, "once per name: {warnings:?}");
    assert!(
        warnings[0].starts_with("scripts/t.luau:")
            && warnings[0].contains("`time.paused` is deprecated: use `time.is_paused`"),
        "{warnings:?}"
    );
    assert!(warnings[1].contains("`input.is_cursor_locked`"));
    let _ = std::fs::remove_dir_all(PathBuf::from(&root));
}
