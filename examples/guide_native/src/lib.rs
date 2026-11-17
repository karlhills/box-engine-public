//! The Rust samples of the guides: `docs/guide/native-plugins.md` (this
//! file and the modules it lists) and `docs/guide/procedural.md`
//! ([`procedural`]). Every Rust block in those guides is copied verbatim
//! from this crate, so it compiles and its tests run.
//!
//! The crate is also a native box project (`box.project.json`, `assets/`):
//! `cargo run -p guide_native --bin guide-player` plays it and
//! `cargo run -p guide_native --features editor --bin guide-editor` opens it.

#[cfg(feature = "editor")]
pub mod editor;
pub mod gameplay;
pub mod procedural;
pub mod script_api;

use std::path::PathBuf;

use box_engine::prelude::*;

/// The game as the player, the editor and tests run it.
pub fn game() -> Game {
    Game::new()
        .with_plugin(gameplay::GameplayPlugin)
        .with_plugin(script_api::ScoreApiPlugin)
        .with_plugin(procedural::ProceduralSamplesPlugin)
}

/// This crate's project folder.
pub fn project_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
