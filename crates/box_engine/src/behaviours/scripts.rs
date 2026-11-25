//! The script versions of behaviours (Convert to Script, the
//! `eject_behaviour` command; docs/BEHAVIOURS.md, B3): the Luau files in
//! `behaviours/luau/` and how each component's fields become the script's
//! properties. `crates/box_engine/tests/behaviours_b3.rs` plays each
//! component and its script side by side and compares them.

use box_core::preset::BehaviourScript;
use box_core::reflect::Registry;
use serde_json::{Value as Json, json};

/// `Number`, `Boolean`, `String` and `Vector` property records.
fn number(name: &str, v: &Json) -> Json {
    json!({ "name": name, "value": { "Number": v.as_f64().unwrap_or(0.0) } })
}
fn boolean(name: &str, v: &Json) -> Json {
    json!({ "name": name, "value": { "Boolean": v.as_bool().unwrap_or(true) } })
}
fn string(name: &str, v: &str) -> Json {
    json!({ "name": name, "value": { "String": v } })
}
fn vector(name: &str, v: &Json) -> Json {
    let f = |i: usize| v.get(i).and_then(Json::as_f64).unwrap_or(0.0);
    json!({ "name": name, "value": { "Vector": [f(0), f(1), f(2)] } })
}

fn spinner(c: &Json) -> Result<Vec<Json>, String> {
    Ok(vec![
        boolean("enabled", &c["enabled"]),
        vector("axis", &c["axis"]),
        number("speed", &c["speed"]),
    ])
}

fn bobber(c: &Json) -> Result<Vec<Json>, String> {
    Ok(vec![
        boolean("enabled", &c["enabled"]),
        number("height", &c["height"]),
        number("period", &c["period"]),
    ])
}

/// The most points the script version of a `Mover` takes.
pub const MOVER_SCRIPT_POINTS: usize = 8;

fn mover(c: &Json) -> Result<Vec<Json>, String> {
    let points = c["points"].as_array().cloned().unwrap_or_default();
    if points.len() > MOVER_SCRIPT_POINTS {
        return Err(format!(
            "this Mover has {} points; its script version takes at most {MOVER_SCRIPT_POINTS} (remove some first)",
            points.len()
        ));
    }
    let mode = c["mode"].as_str().unwrap_or("PingPong");
    let mut out = vec![
        boolean("enabled", &c["enabled"]),
        number("speed", &c["speed"]),
        string("mode", mode),
        number("wait", &c["wait"]),
        json!({ "name": "point_count", "value": { "Integer": points.len() } }),
    ];
    for (i, p) in points.iter().enumerate() {
        out.push(vector(&format!("point_{}", i + 1), p));
    }
    Ok(out)
}

/// The behaviours with a script version.
pub const BEHAVIOUR_SCRIPTS: &[BehaviourScript] = &[
    BehaviourScript {
        component: "Spinner",
        path: "scripts/behaviours/spinner.luau",
        source: include_str!("luau/spinner.luau"),
        properties: spinner,
    },
    BehaviourScript {
        component: "Bobber",
        path: "scripts/behaviours/bobber.luau",
        source: include_str!("luau/bobber.luau"),
        properties: bobber,
    },
    BehaviourScript {
        component: "Mover",
        path: "scripts/behaviours/mover.luau",
        source: include_str!("luau/mover.luau"),
        properties: mover,
    },
];

/// Registers [`BEHAVIOUR_SCRIPTS`].
pub fn register_behaviour_scripts(registry: &mut Registry) {
    for s in BEHAVIOUR_SCRIPTS {
        registry.register_behaviour_script(*s);
    }
}

/// Writes each `(path under assets/, text)` the project lacks (the
/// `eject_behaviour` scripts in `Applied::files`); existing files, perhaps
/// edited, are kept. Returns the paths written.
pub fn write_missing_files(
    assets_dir: &std::path::Path,
    files: &[(String, String)],
) -> std::io::Result<Vec<String>> {
    let mut written = Vec::new();
    for (rel, text) in files {
        let path = assets_dir.join(rel);
        if path.exists() {
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::project::write_atomic(&path, text.as_bytes())?;
        tracing::info!(target: "box_engine", "wrote {rel} (Convert to Script)");
        written.push(rel.clone());
    }
    Ok(written)
}
