//! The code in the guides (`docs/guide`) and the reference overviews
//! (`docs/reference`) is tested code:
//!
//! - Every ```` ```lua ```` block is copied verbatim from a `.luau` file of
//!   the `examples/guide_samples` data project, which this test plays
//!   headless with the stock engine (no script may fail).
//! - Every ```` ```rust ```` block of a guide is copied verbatim from the
//!   `examples/guide_native` crate, which builds and has its own tests
//!   (`cargo test -p guide_native`).
//!
//! To change a sample, change the file in the example and paste the same
//! text into the guide (or regenerate the reference).

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_script::ScriptRuntime;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            files(&path, ext, out);
        } else if path.to_string_lossy().ends_with(ext) {
            out.push(path);
        }
    }
}

/// The fenced blocks of `lang` in a Markdown text, without their fences.
fn blocks(text: &str, lang: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        let t = line.trim_start();
        match &mut current {
            None => {
                if t.strip_prefix("```").is_some_and(|l| l.trim() == lang) {
                    current = Some(Vec::new());
                }
            }
            Some(lines) => {
                if t.starts_with("```") {
                    out.push(lines.join("\n"));
                    current = None;
                } else {
                    lines.push(line);
                }
            }
        }
    }
    out
}

/// Every `lang` block of the Markdown files is a substring of one of
/// `sources`; returns the ones that are not, with their file.
fn foreign_blocks(docs: &[PathBuf], lang: &str, sources: &[PathBuf]) -> Vec<String> {
    let sources: Vec<String> = sources
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap_or_default())
        .collect();
    let mut missing = Vec::new();
    for doc in docs {
        let text = std::fs::read_to_string(doc).unwrap_or_default();
        for block in blocks(&text, lang) {
            let block = block.trim_matches('\n');
            if !sources.iter().any(|s| s.contains(block)) {
                missing.push(format!("{}:\n{block}", doc.display()));
            }
        }
    }
    missing
}

fn guide_docs() -> Vec<PathBuf> {
    let mut docs = Vec::new();
    files(&root().join("docs/guide"), ".md", &mut docs);
    files(&root().join("docs/guide"), ".md", &mut docs);
    docs
}

#[test]
fn lua_blocks_come_from_the_guide_samples_project() {
    let mut docs = guide_docs();
    files(&root().join("docs/reference"), ".md", &mut docs);
    let mut sources = Vec::new();
    files(
        &root().join("examples/guide_samples/assets"),
        ".luau",
        &mut sources,
    );
    assert!(!sources.is_empty());
    let missing = foreign_blocks(&docs, "lua", &sources);
    assert!(
        missing.is_empty(),
        "Luau blocks not found verbatim in examples/guide_samples/assets/**/*.luau:\n\n{}",
        missing.join("\n\n")
    );
}

#[test]
fn rust_blocks_come_from_the_guide_native_crate() {
    let docs = guide_docs();
    let mut sources = Vec::new();
    files(&root().join("examples/guide_native"), ".rs", &mut sources);
    assert!(!sources.is_empty());
    let missing = foreign_blocks(&docs, "rust", &sources);
    assert!(
        missing.is_empty(),
        "Rust blocks not found verbatim in examples/guide_native/**/*.rs:\n\n{}",
        missing.join("\n\n")
    );
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut app.world, Duration::from_millis(20));
        }
        app.run_frames(1, 1.0 / 60.0);
    }
}

/// Plays one scene of the samples project headless for `n` frames.
fn play(scene: &str, n: usize) -> App {
    let project = root().join("examples/guide_samples");
    let options = PlayerOptions::from_args([
        project.display().to_string(),
        "--headless".into(),
        "--scene".into(),
        scene.into(),
    ])
    .unwrap();
    let (mut app, _project) = build_app(&Game::new(), &options).unwrap();
    frames(&mut app, n);
    app
}

/// No script instance failed and no rule or generator left a warning.
fn no_failed_scripts(app: &mut App, scene: &str) {
    if let Some(runtime) = app.world.get_resource::<ScriptRuntime>() {
        let stats = runtime.stats();
        assert_eq!(
            stats.disabled, 0,
            "{scene}: a sample script failed: {stats:?}"
        );
    }
    let warnings: Vec<String> = app
        .world
        .query::<(Entity, &box_engine::box_core::derived::GeneratedWarnings)>()
        .iter()
        .filter(|(_, w)| !w.0.is_empty())
        .map(|(e, w)| format!("{e:?}: {:?}", w.0))
        .collect();
    assert!(warnings.is_empty(), "{scene}: {warnings:#?}");
}

fn entity(app: &App, name: &str) -> Entity {
    app.world
        .find_by_name(name)
        .unwrap_or_else(|| panic!("no entity {name}"))
}

fn script_field<T: box_engine::box_script::mlua::FromLua>(app: &App, name: &str, field: &str) -> T {
    app.world
        .resource::<ScriptRuntime>()
        .instance(entity(app, name))
        .unwrap_or_else(|| panic!("{name} has no script instance"))
        .get(field)
        .unwrap()
}

/// The main sample scene does what the guides and the reference say.
#[test]
fn the_main_sample_scene_works() {
    let mut app = play("main", 60);
    no_failed_scripts(&mut app, "main");
    let stats = app.world.resource::<ScriptRuntime>().stats();
    assert_eq!(stats.instances, 20, "{stats:?}");
    let swarm = entity(&app, "Swarm");
    assert_eq!(
        app.world.children(swarm).len(),
        12,
        "the swarm spawned its orbs"
    );

    // Spinner, Lamp (entity methods), Level (spawn, resources), the rule.
    let totem = app
        .world
        .get::<Transform>(entity(&app, "Totem"))
        .unwrap()
        .rotation;
    assert!(totem.angle_between(Quat::IDENTITY) > 0.5, "the totem turns");
    let lamp = entity(&app, "Lamp");
    assert_eq!(app.world.get::<PointLight>(lamp).unwrap().intensity, 40.0);
    assert!(app.world.get::<Collider>(lamp).unwrap().is_trigger);
    assert_eq!(
        app.world.get::<Transform>(lamp).unwrap().scale,
        Vec3::splat(2.0)
    );
    let marker = entity(&app, "Marker");
    let shape = app.world.get::<Primitive>(marker).unwrap().shape.clone();
    assert!(
        matches!(shape, box_engine::box_core::primitive::Shape::Sphere { radius, .. } if radius == 0.3),
        "a partial variant patch keeps its variant: {shape:?}"
    );
    let row = entity(&app, "Fence Row");
    assert_eq!(
        app.world.children(row).len(),
        4,
        "the row rule made 4 posts"
    );
    // Lift's impulse threw the crate up.
    let crate_y = app
        .world
        .get::<Transform>(entity(&app, "Crate"))
        .unwrap()
        .translation
        .y;
    assert!(crate_y > 0.6, "the crate was lifted: {crate_y}");
    // The animated hero's state machine runs.
    let body = entity(&app, "Body");
    let state = app
        .world
        .get::<box_engine::box_core::animation::AnimationStateMachineState>(body)
        .map(|s| s.current().to_string());
    assert_eq!(state.as_deref(), Some("Idle"));

    // Pick sees the ball on screen once the game view has a size, and a
    // click casts the cursor ray.
    assert!(!script_field::<bool>(&app, "Picker", "ball_visible"));
    *app.world.resource_mut::<box_engine::WindowSize>() = box_engine::WindowSize {
        width: 1280,
        height: 720,
        scale_factor_milli: 1000,
    };
    app.world.resource_mut::<Input>().cursor_position = Some(Vec2::new(640.0, 360.0));
    app.world
        .resource_mut::<Input>()
        .press_button(MouseButton::Left);
    frames(&mut app, 2);
    app.world
        .resource_mut::<Input>()
        .release_button(MouseButton::Left);
    assert!(script_field::<bool>(&app, "Picker", "ball_visible"));

    // Walk forward (W) through both coins and the gem.
    let z0 = app
        .world
        .get::<Transform>(entity(&app, "Player"))
        .unwrap()
        .translation
        .z;
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    frames(&mut app, 90);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    frames(&mut app, 2);
    let z1 = app
        .world
        .get::<Transform>(entity(&app, "Player"))
        .unwrap()
        .translation
        .z;
    assert!(z1 < z0 - 5.0, "walked forward: {z0} -> {z1}");
    assert!(app.world.find_by_name("Coin 1").is_none());
    assert!(app.world.find_by_name("Coin 2").is_none());
    assert!(app.world.find_by_name("Gem").is_none());
    assert_eq!(script_field::<i64>(&app, "Score", "score"), 20);
    assert_eq!(script_field::<i64>(&app, "Game", "coins"), 2);
    no_failed_scripts(&mut app, "main");
}

#[test]
fn every_sample_scene_plays_without_script_errors() {
    let mut scenes = Vec::new();
    files(
        &root().join("examples/guide_samples/assets/scenes"),
        ".scene.json",
        &mut scenes,
    );
    assert!(!scenes.is_empty());
    for scene in scenes {
        let name = scene
            .file_name()
            .unwrap()
            .to_string_lossy()
            .trim_end_matches(".scene.json")
            .to_owned();
        let mut app = play(&name, 180);
        no_failed_scripts(&mut app, &name);
    }
}
