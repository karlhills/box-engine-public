//! Shared by the project upgrade tests: copies of vendored data projects
//! and the check that they are current and load without losing data.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use box_engine::Game;
use box_engine::box_core::migrate;
use box_engine::box_core::scene::{self, UnknownFields, unknown_types};
use box_engine::prelude::*;
use box_engine::upgrade;
use serde_json::Value as Json;

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/upgrade")
        .join(name)
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A fresh copy of fixture `name` in the test's temp folder.
pub fn copy_fixture(name: &str, tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("upgrade_{tag}_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&fixture(name), &dir);
    dir
}

pub fn files_with(root: &Path, suffix: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, suffix, out);
            } else if path.to_string_lossy().ends_with(suffix) {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join("assets"), suffix, &mut out);
    out.sort();
    out
}

pub fn read_json(path: &Path) -> Json {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Every value `old` states is in `new` (objects: each key, recursively;
/// arrays: same length, element by element; numbers: equal as `f32`). New
/// fields in `new` are allowed: they are the defaults of fields added since.
pub fn assert_preserved(old: &Json, new: &Json, at: &str) {
    match (old, new) {
        (Json::Object(o), Json::Object(n)) => {
            for (k, v) in o {
                let child = format!("{at}.{k}");
                let nv = n
                    .get(k)
                    .unwrap_or_else(|| panic!("{child}: missing after loading"));
                assert_preserved(v, nv, &child);
            }
        }
        (Json::Array(o), Json::Array(n)) => {
            assert_eq!(o.len(), n.len(), "{at}: length");
            for (i, (a, b)) in o.iter().zip(n).enumerate() {
                assert_preserved(a, b, &format!("{at}[{i}]"));
            }
        }
        (Json::Number(a), Json::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(a as f32 == b as f32, "{at}: {a} became {b}");
        }
        // An asset reference written as `"file#Label"` reads as the object
        // form.
        (Json::String(s), Json::Object(n)) if n.contains_key("path") => {
            let mut full = n["path"].as_str().unwrap_or_default().to_string();
            if let Some(label) = n.get("label").and_then(Json::as_str) {
                full = format!("{full}#{label}");
            }
            assert_eq!(s, &full, "{at}");
        }
        (o, n) => assert_eq!(o, n, "{at}"),
    }
}

pub fn engine_game() -> Game {
    Game::new()
}

/// What a vendored data project is expected to hold.
pub struct Expect {
    /// Scenes use engine types only (false for a native game's data).
    pub engine_types_only: bool,
    /// The number of material files, when it is pinned.
    pub materials: Option<usize>,
}

/// Checks that the vendored data project `name` is current and that every
/// value its files state survives loading.
pub fn check_data_project(name: &str, expect: Expect) {
    let game = engine_game();
    let registry = game.registry();
    {
        let root = copy_fixture(name, "current");
        let plan = upgrade::plan(&root, &registry).unwrap();
        assert!(
            plan.is_current(),
            "{name}: no shipped format changed since, yet:\n{}",
            plan.summary()
        );

        for path in files_with(&root, ".scene.json") {
            let what = format!("{name}/{}", path.display());
            let old = read_json(&path);
            let world = scene::load(&std::fs::read(&path).unwrap(), registry.clone())
                .unwrap_or_else(|e| panic!("{what}: {e}"));
            let unknown: Vec<String> = unknown_types(&world).into_iter().map(|t| t.name).collect();
            if expect.engine_types_only {
                assert!(unknown.is_empty(), "{what}: unknown types {unknown:?}");
            }
            assert_eq!(
                world.query::<&UnknownFields>().iter().count(),
                0,
                "{what}: unknown fields"
            );
            let saved: Json = serde_json::from_slice(&scene::save(&world).unwrap()).unwrap();
            assert_preserved(
                &old["resources"],
                &saved["resources"],
                &format!("{what} resources"),
            );
            let entities = old["entities"].as_array().unwrap();
            assert_eq!(
                entities.len(),
                saved["entities"].as_array().unwrap().len(),
                "{what}"
            );
            for e in entities {
                let uid = &e["uid"];
                let s = saved["entities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["uid"] == *uid)
                    .unwrap_or_else(|| panic!("{what}: entity {uid} lost"));
                assert_preserved(e, s, &format!("{what} {uid}"));
            }
        }

        // Single-value files: materials, animation graphs, settings.
        let mut values = Vec::new();
        for path in files_with(&root, ".material.json") {
            let ty = if read_json(&path).get("shader").is_some() {
                "ShaderMaterial"
            } else {
                "StandardMaterial"
            };
            values.push((path, ty.to_string()));
        }
        for path in files_with(&root, ".animgraph.json") {
            values.push((path, "AnimationGraph".to_string()));
        }
        for ty in registry.settings() {
            let path = root.join(format!("assets/settings/{}.json", ty.name()));
            if path.is_file() {
                values.push((path, ty.name().to_string()));
            }
        }
        for (path, ty) in &values {
            let old = read_json(path);
            let type_id = registry
                .type_registry()
                .iter()
                .find(|r| r.type_info().type_path_table().short_path() == ty)
                .map(|r| r.type_id())
                .unwrap_or_else(|| panic!("{ty} is not a registered type"));
            let migrate::ValueFile {
                value,
                unknown: extra,
                steps,
            } = migrate::read_value_file(&registry, ty, type_id, &old).unwrap();
            assert!(extra.is_empty() && steps.is_empty(), "{}", path.display());
            let typed = registry
                .value_from_json(type_id, &value)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let back = registry.value_to_json(typed.as_ref()).unwrap();
            assert_preserved(&old, &back, &path.display().to_string());
        }
        let materials = values
            .iter()
            .filter(|(_, ty)| ty.ends_with("Material"))
            .count();
        if let Some(expected) = expect.materials {
            assert_eq!(materials, expected, "{name}: material files checked");
        }

        // Input action maps are unversioned; they still parse.
        for path in files_with(&root, ".actions.json") {
            let text = std::fs::read_to_string(&path).unwrap();
            ActionMap::from_json(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
}
