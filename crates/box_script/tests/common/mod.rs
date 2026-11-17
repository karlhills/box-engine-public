//! Script test projects: a temporary `assets/` folder with `.luau` files,
//! an asset server and a game app around it.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use box_engine::Game;
use box_engine::box_assets::AssetServer;
use box_engine::box_script::{ScriptRuntime, ScriptStats};
use box_engine::prelude::*;

pub struct Project {
    pub root: PathBuf,
}

impl Project {
    /// A fresh project folder with `files` (paths relative to `assets/`).
    pub fn new(name: &str, files: &[(&str, &str)]) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "box_script_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let p = Self { root };
        for (path, text) in files {
            p.write(path, text);
        }
        p
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn write(&self, path: &str, text: &str) {
        let full = self.assets().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    /// An app of the engine's game around an empty world with this
    /// project's asset server.
    pub fn app(&self) -> App {
        let game = Game::new();
        let mut world = game.new_world();
        let server = box_engine::asset_server(self.assets()).unwrap();
        world.insert_resource(server);
        game.app(world)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn server(app: &App) -> AssetServer {
    app.world.resource::<AssetServer>().clone()
}

/// Runs frames of 1/60 s until every import finished and `done` holds
/// (at most `limit` seconds of wall time).
pub fn run_until(app: &mut App, limit: f32, mut done: impl FnMut(&mut App) -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f32() < limit {
        let s = server(app);
        s.update_until_idle(&mut app.world, Duration::from_millis(50));
        app.run_frames(1, 1.0 / 60.0);
        if done(app) {
            return true;
        }
    }
    false
}

/// Spawns a saved entity with a script.
pub fn spawn_script(app: &mut App, name: &str, script: Script) -> Entity {
    app.world
        .spawn((Uid::new(), Name::new(name), Transform::default(), script))
}

pub fn stats(app: &App) -> ScriptStats {
    app.world
        .get_resource::<ScriptRuntime>()
        .map(ScriptRuntime::stats)
        .unwrap_or_default()
}

/// A field of an entity's script instance (`self.<key>`).
pub fn field<T: box_engine::box_script::mlua::FromLua>(
    app: &App,
    e: Entity,
    key: &str,
) -> Option<T> {
    let rt = app.world.get_resource::<ScriptRuntime>()?;
    rt.instance(e)?.get::<T>(key).ok()
}
