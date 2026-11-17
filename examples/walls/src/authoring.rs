//! Scene authoring: each scene is built by applying the JSON edit commands
//! in `authoring/<scene>.commands.json` (the format `box scene apply`
//! takes) to an empty world with this game's registry, so `WallPath` and
//! `FencePath` validate like engine components.
//!
//! Regenerate the scene files with `cargo run -p walls --example walls_author`;
//! the `scenes_match_authoring` test fails when they are out of date.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use box_engine::Game;
use box_engine::box_core::edit::{EditStack, apply_commands, parse_commands};
use box_engine::box_core::scene;
use box_engine::prelude::*;

/// The scenes generated from `authoring/`, by name.
pub const SCENES: [&str; 2] = ["yard", "court"];

/// This example's project folder.
pub fn project_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `authoring/<name>.commands.json`.
pub fn commands_path(name: &str) -> PathBuf {
    project_dir()
        .join("authoring")
        .join(format!("{name}.commands.json"))
}

/// `assets/scenes/<name>.scene.json`.
pub fn scene_path(name: &str) -> PathBuf {
    project_dir()
        .join("assets/scenes")
        .join(format!("{name}.scene.json"))
}

/// The game the scenes are authored for.
pub fn game() -> Game {
    Game::new().with_plugin(crate::WallsPlugin)
}

/// Apply a commands file to an empty world (default `Environment`) and
/// return the saved scene bytes.
pub fn build_scene(game: &Game, commands_file: &Path) -> Result<Vec<u8>> {
    let text = std::fs::read_to_string(commands_file)
        .with_context(|| format!("could not read {}", commands_file.display()))?;
    let commands = parse_commands(&text)
        .with_context(|| format!("bad commands in {}", commands_file.display()))?;
    let mut world = game.new_world();
    world.insert_resource(Environment::default());
    let mut stack = EditStack::new();
    apply_commands(&mut world, &mut stack, &commands, "author")
        .with_context(|| format!("applying {}", commands_file.display()))?;
    let bytes = scene::save(&world).context("could not save the scene")?;
    scene::load(&bytes, game.registry()).context("the authored scene does not load")?;
    Ok(bytes)
}
