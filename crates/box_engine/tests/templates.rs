//! The project templates (`templates/projects`) play as their READMEs say,
//! headlessly in the stock engine game (no game code), from a project
//! created the way `box new --template` creates it:
//!
//! - `third-person`: W walks the character (a `PlayerController`) toward
//!   the follow camera's forward, into the first pickup, which the built-in
//!   HUD counts; Space jumps. No scripts.
//! - `procedural`: the Luau rule builds the garden path's parts and the
//!   scatter places shrubs and rocks, with no rule or script warnings.
//!
//! The command-line side (check, run, capture, the gallery) is in
//! `apps/cli/tests/templates.rs`.

use std::path::PathBuf;
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_core::derived::Derived;
use box_engine::box_ui::Ui;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_engine::scatter_glue::ScatterRuntime;
use box_engine::templates;

const DT: f32 = 1.0 / 60.0;

/// A new project from `template` in a fresh folder.
fn new_project(template: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "box_template_test_{template}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let list = templates::templates();
    let t = templates::find_template(&list, template).unwrap();
    templates::create_project(&dir, "Test", &list, t).unwrap();
    dir
}

fn app(project: PathBuf) -> App {
    let options = PlayerOptions {
        project: Some(project),
        watch: false,
        ..Default::default()
    };
    build_app(&Game::new(), &options).unwrap().0
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

fn position(world: &World, name: &str) -> Vec3 {
    let e = world
        .find_by_name(name)
        .unwrap_or_else(|| panic!("no `{name}`"));
    world.get::<Transform>(e).unwrap().translation
}

/// The built-in HUD's lines, as shown.
fn hud(world: &World) -> Vec<String> {
    let ui = world.resource::<Ui>();
    let root = ui.find(None, "game_hud").expect("the built-in HUD");
    ui.children(root)
        .iter()
        .map(|n| ui.text(*n).unwrap_or_default())
        .collect()
}

#[test]
fn third_person_template_walks_collects_and_jumps() {
    let dir = new_project("third-person");
    let mut app = app(dir.clone());
    frames(&mut app, 10);
    assert_eq!(
        app.world
            .query::<&box_engine::box_script::Script>()
            .iter()
            .count(),
        0
    );
    assert_eq!(hud(&app.world), ["Score 0", "Pickups 0 / 5"]);
    let start = position(&app.world, "Player");

    // W walks forward (the camera looks down -Z) into Pickup 1, 7 m ahead.
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    frames(&mut app, 80);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    frames(&mut app, 3);
    let end = position(&app.world, "Player");
    assert!(start.z - end.z > 6.0, "walked from {start} to {end}");
    assert!((end.x - start.x).abs() < 0.05, "drifted sideways: {end}");
    let pickup = app.world.find_by_name("Pickup 1").unwrap();
    assert!(
        app.world.get::<Visible>(pickup).is_some_and(|v| !v.0),
        "picked up"
    );
    assert_eq!(hud(&app.world), ["Score 1", "Pickups 1 / 5"]);

    // Space jumps.
    app.world.resource_mut::<Input>().press_key(KeyCode::Space);
    frames(&mut app, 1);
    app.world
        .resource_mut::<Input>()
        .release_key(KeyCode::Space);
    frames(&mut app, 15);
    let up = position(&app.world, "Player");
    assert!(up.y > end.y + 0.8, "jumped from {end} to {up}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn procedural_template_builds_rule_parts_and_scatter() {
    let dir = new_project("procedural");
    let mut app = app(dir.clone());
    frames(&mut app, 5);
    box_engine::settle_assets(&mut app.world, Duration::from_secs(10));
    frames(&mut app, 2);
    let path = app.world.find_by_name("Garden Path").unwrap();
    let parts = app
        .world
        .query::<&Derived>()
        .iter()
        .filter(|d| d.owner == path)
        .count();
    // Gravel, 4 legs of stones and 5 lanterns.
    assert!(parts > 10, "{parts} parts");
    let scatter = app.world.resource::<ScatterRuntime>().stats();
    assert!(scatter.instances > 20, "{scatter:?}");
    let _ = std::fs::remove_dir_all(dir);
}
