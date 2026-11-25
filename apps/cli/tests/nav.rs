//! `box nav` (docs/NAVIGATION.md): paths on a project's start scene baked
//! from its colliders; `unreachable` behind a wall; `no_start` off the mesh.

use std::path::Path;
use std::process::Command;

use serde_json::Value as Json;

fn nav(dir: &Path, from: &str, to: &str) -> Json {
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(dir)
        .args(["nav", ".", "--from", from, "--to", to, "--json"])
        .output()
        .expect("run box");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("JSON")
}

#[test]
fn box_nav_reports_paths() {
    let dir = std::env::temp_dir().join(format!("box-nav-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let new = Command::new(env!("CARGO_BIN_EXE_box"))
        .args(["new", dir.to_str().unwrap(), "--json"])
        .output()
        .expect("box new");
    assert!(
        new.status.success(),
        "{}",
        String::from_utf8_lossy(&new.stderr)
    );
    // A closed wall across z = 0 on the starter ground.
    let cmds = dir.join("wall.json");
    std::fs::write(
        &cmds,
        r#"[{ "op": "spawn", "name": "Wall", "components": {
            "Transform": { "translation": [0.0, 1.5, 0.0] },
            "Primitive": { "shape": { "Cuboid": { "size": [400.0, 3.0, 0.4] } } },
            "Collider": { "shape": "Primitive" } } }]"#,
    )
    .unwrap();
    let apply = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(&dir)
        .args([
            "scene",
            "apply",
            "assets/scenes/main.scene.json",
            "wall.json",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stdout)
    );

    let open = nav(&dir, "-5,0,-5", "5,0,-8");
    assert_eq!(open["status"], "complete", "{open:#}");
    assert!(open["length"].as_f64().unwrap() > 5.0);
    assert!(open["polygons"].as_u64().unwrap() > 0);

    let walled = nav(&dir, "0,0,-5", "0,0,5");
    assert_eq!(walled["status"], "unreachable", "{walled:#}");
    let last = walled["points"].as_array().unwrap().last().unwrap().clone();
    assert!(last[2].as_f64().unwrap() < 0.0, "{last}");

    let off = nav(&dir, "0,-50,0", "1,0,1");
    assert_eq!(off["status"], "no_start", "{off:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn box_check_reports_rules_that_cannot_work() {
    let dir = std::env::temp_dir().join(format!("box-rules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let new = Command::new(env!("CARGO_BIN_EXE_box"))
        .args(["new", dir.to_str().unwrap(), "--json"])
        .output()
        .expect("box new");
    assert!(new.status.success());
    let cmds = dir.join("rules.json");
    std::fs::write(
        &cmds,
        r#"[
        { "op": "add_preset", "preset": "player", "at": [0.0, 0.0, 0.0] },
        { "op": "add_preset", "preset": "goal", "at": [0.0, 0.0, 8.0] },
        { "op": "set", "entity": "Goal", "path": "Interactable.requires", "value": [{ "GroupDone": { "group": "coins" } }] },
        { "op": "set_resource", "path": "GameRules", "value": { "win": [{ "AtLeast": { "var": "keys", "value": 3.0 } }] } },
        { "op": "add_preset", "preset": "door_to_scene", "at": [6.0, 0.0, 0.0] },
        { "op": "spawn", "name": "Empty Spawner", "components": { "Transform": {}, "Spawner": {} } },
        { "op": "add_preset", "preset": "enemy", "at": [-10.0, 0.0, -10.0] },
        { "op": "spawn", "name": "Cage N", "components": { "Transform": { "translation": [-10.0, 1.5, -12.0] }, "Primitive": { "shape": { "Cuboid": { "size": [5.0, 3.0, 0.4] } } }, "Collider": { "shape": "Primitive" } } },
        { "op": "spawn", "name": "Cage S", "components": { "Transform": { "translation": [-10.0, 1.5, -8.0] }, "Primitive": { "shape": { "Cuboid": { "size": [5.0, 3.0, 0.4] } } }, "Collider": { "shape": "Primitive" } } },
        { "op": "spawn", "name": "Cage W", "components": { "Transform": { "translation": [-12.0, 1.5, -10.0] }, "Primitive": { "shape": { "Cuboid": { "size": [0.4, 3.0, 5.0] } } }, "Collider": { "shape": "Primitive" } } },
        { "op": "spawn", "name": "Cage E", "components": { "Transform": { "translation": [-8.0, 1.5, -10.0] }, "Primitive": { "shape": { "Cuboid": { "size": [0.4, 3.0, 5.0] } } }, "Collider": { "shape": "Primitive" } } }
    ]"#,
    )
    .unwrap();
    let apply = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(&dir)
        .args([
            "scene",
            "apply",
            "assets/scenes/main.scene.json",
            "rules.json",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        apply.status.success(),
        "{}",
        String::from_utf8_lossy(&apply.stdout)
    );
    let out = Command::new(env!("CARGO_BIN_EXE_box"))
        .current_dir(&dir)
        .args(["check", "--json"])
        .output()
        .unwrap();
    let json: Json = serde_json::from_slice(&out.stdout).expect("JSON");
    let codes: Vec<(String, String)> = json["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["code"].as_str().unwrap_or("").to_string(),
                p["entity_name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    for (code, who) in [
        ("group_empty", "Goal"),
        ("var_never_set", ""),
        ("switch_scene_empty", "Door to Scene"),
        ("spawner_empty", "Empty Spawner"),
        ("nav_unreachable", "Enemy"),
    ] {
        assert!(
            codes.iter().any(|(c, w)| c == code && w == who),
            "{code} ({who}) in {codes:?}"
        );
    }
    // Warnings, not errors: the project still checks out.
    assert!(out.status.success(), "{json:#}");
    let _ = std::fs::remove_dir_all(&dir);
}
