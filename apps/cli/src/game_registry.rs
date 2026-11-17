//! A native project's game types for the CLI and MCP: the engine registry
//! plus the schema-only types of `<project>/.box/registry.json`
//! (`box_engine::registry_file`), so `registry`, `check`, `scene show` and
//! `scene apply` know game components without the game's code.

use std::path::Path;
use std::sync::Arc;

use box_engine::Project;
use box_engine::box_core::reflect::Registry;
use box_engine::registry_file::{self, GameRegistry};
use box_engine::scene_glue::{engine_registry_for, shared_engine_registry};
use serde_json::{Value as Json, json};

/// The registry tools use for a project, and what is worth telling about it.
pub(crate) struct ProjectRegistry {
    pub registry: Arc<Registry>,
    /// The game registry file, when the project has one.
    pub game: Option<GameRegistry>,
    /// Names of the game's types added as schemas, sorted.
    pub game_types: Vec<String>,
    /// `registry_stale` / `registry_invalid` warnings.
    pub problems: Vec<Json>,
}

impl ProjectRegistry {
    /// The engine registry alone (no project, or no registry file).
    fn engine() -> Self {
        Self {
            registry: shared_engine_registry(),
            game: None,
            game_types: Vec::new(),
            problems: Vec::new(),
        }
    }

    /// `"game_registry"` for JSON results: file, engine version, hash,
    /// type count, the engine modules the game is built with and
    /// staleness; `null` without a file.
    pub fn summary(&self, project: &Project) -> Json {
        match &self.game {
            Some(g) => json!({
                "file": project.relative(&g.path),
                "engine_version": g.json["engine_version"],
                "hash": g.json["hash"],
                "types": self.game_types,
                "modules": g.modules,
                "stale": g.stale.is_some(),
            }),
            None => Json::Null,
        }
    }
}

/// The registry for the project containing `path` (a project directory, a
/// scene file or any path inside a project). Outside a project, or for a
/// project without `.box/registry.json`, the engine registry.
pub(crate) fn for_path(path: &Path) -> ProjectRegistry {
    match Project::open(path) {
        Ok(project) => for_project(&project),
        Err(_) => ProjectRegistry::engine(),
    }
}

/// [`for_path`] for an open project.
pub(crate) fn for_project(project: &Project) -> ProjectRegistry {
    let file = project.relative(&registry_file::registry_path(project));
    match registry_file::read_registry(project) {
        Ok(None) => {
            let mut r = ProjectRegistry::engine();
            // A native project without the file: game types are unknown.
            if box_engine::player::NativeCrate::find(project.root()).is_some() {
                r.problems.push(json!({
                    "severity": "warning",
                    "code": "registry_missing",
                    "file": file,
                    "message": "this native project has no game registry file, so its game component types are unknown to `box`",
                    "hint": format!("write it: {}", registry_file::refresh_command(project)),
                }));
            }
            r
        }
        Ok(Some(game)) => {
            // The engine types of the modules the game is built with (all,
            // for a file that does not say).
            let all: Vec<&str> = box_engine::modules::MODULES
                .iter()
                .map(|m| m.feature)
                .collect();
            let features: Vec<&str> = game
                .modules
                .as_ref()
                .map_or(all, |m| m.iter().map(String::as_str).collect());
            let mut registry = engine_registry_for(&features);
            let game_types = registry.add_schema_types(&game.json);
            let mut problems = Vec::new();
            if let Some(reason) = &game.stale {
                problems.push(json!({
                    "severity": "warning",
                    "code": "registry_stale",
                    "file": file,
                    "message": format!("the game registry may be out of date: {reason}; game types are checked against it as it is"),
                    "hint": format!("refresh it: {}", registry_file::refresh_command(project)),
                }));
            }
            ProjectRegistry {
                registry: Arc::new(registry),
                game: Some(game),
                game_types,
                problems,
            }
        }
        Err(message) => {
            let mut r = ProjectRegistry::engine();
            r.problems.push(json!({
                "severity": "warning",
                "code": "registry_invalid",
                "file": file,
                "message": format!("the game registry cannot be read ({message}); using the engine's types only"),
                "hint": format!("rewrite it: {}", registry_file::refresh_command(project)),
            }));
            r
        }
    }
}
