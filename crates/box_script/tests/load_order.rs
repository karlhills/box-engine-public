//! `on_load` runs after `on_start` has returned, waits included, and an
//! `on_start` that never returns while load data waits is reported (M8 gap
//! report item 11). The only test in its binary: it captures logs through
//! the process-wide subscriber.

mod common;

use box_engine::box_script::{load_script_state, save_script_state};
use box_engine::log_capture;
use box_engine::prelude::*;
use common::*;

const ORDERED: &str = r#"
local M = {}
function M.on_start(self)
    self.order = { "start" }
    self.score = 0
    wait(0.2)
    -- Set after a wait: on_load still overrides it.
    self.level = 1
    table.insert(self.order, "start done")
end
function M.on_save(self)
    return { score = 7, level = 3 }
end
function M.on_load(self, data)
    table.insert(self.order, "load")
    self.score = data.score
    self.level = data.level
end
return M
"#;

const ENDLESS: &str = r#"
local M = {}
function M.on_start(self)
    self.ticks = 0
    while true do
        wait(0.5)
        self.ticks += 1
    end
end
function M.on_load(self, data)
    self.loaded = true
end
return M
"#;

#[test]
fn on_load_runs_after_on_start_returns() {
    box_engine::init_logging();
    log_capture::enable();
    let p = Project::new(
        "load_order",
        &[("scripts/o.luau", ORDERED), ("scripts/e.luau", ENDLESS)],
    );
    let mut app = p.app();
    let e = spawn_script(&mut app, "Ordered", Script::new("scripts/o.luau"));
    let stuck = spawn_script(&mut app, "Endless", Script::new("scripts/e.luau"));
    assert!(run_until(&mut app, 5.0, |app| stats(app).instances == 2));
    let mut saved = save_script_state(&mut app.world);
    let stuck_uid = app.world.uid(stuck).unwrap().to_string();
    saved[&stuck_uid] = serde_json::json!({ "x": 1 });

    // Fresh instances, as a save load makes them: the data waits.
    app.world.remove_one::<Script>(e).unwrap();
    app.world.remove_one::<Script>(stuck).unwrap();
    app.run_frames(1, 1.0 / 60.0);
    app.world
        .insert_one(e, Script::new("scripts/o.luau"))
        .unwrap();
    app.world
        .insert_one(stuck, Script::new("scripts/e.luau"))
        .unwrap();
    assert_eq!(load_script_state(&mut app.world, &saved), 0);
    let seq = log_capture::next_seq();
    app.run_frames(30, 1.0 / 60.0);
    let order: Vec<String> = field::<box_engine::box_script::mlua::Table>(&app, e, "order")
        .unwrap()
        .sequence_values::<String>()
        .map(Result::unwrap)
        .collect();
    assert_eq!(order, ["start", "start done", "load"]);
    assert_eq!(field::<i64>(&app, e, "score"), Some(7));
    assert_eq!(field::<i64>(&app, e, "level"), Some(3));

    // The endless on_start keeps its on_load waiting, and says so once.
    app.run_frames(6 * 60, 1.0 / 60.0);
    assert_ne!(field::<bool>(&app, stuck, "loaded"), Some(true));
    assert!(field::<i64>(&app, stuck, "ticks").unwrap() > 8);
    let warnings: Vec<String> = log_capture::read(seq, 4096)
        .entries
        .into_iter()
        .filter(|l| l.level == "warn" && l.message.contains("waiting for on_start"))
        .map(|l| l.message)
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("scripts/e.luau") && warnings[0].contains("task.spawn"));
}
