//! A native project's game registry for tools: `<project>/.box/registry.json`.
//!
//! The `box` CLI and `box mcp` do not link a native game's code, so they
//! cannot know its component types. The game's own editor and player write
//! the registry they run with ([`Registry::to_json`], the `box registry
//! --json` format, plus `format`, `engine_version` and a content `hash`) when
//! they start, and `--write-registry` writes it and exits. It also lists
//! the optional engine modules the game was built with (`modules`, the
//! `box_engine` features of [`crate::modules`]), so tools validate a lean
//! game's project against the types it really has. The CLI loads it
//! with [`read_registry`] and adds the game's types as schema-only types
//! ([`Registry::add_schema_types`]), so `box registry`, `box check`, `box
//! scene show` and `box scene apply` work with game components.
//!
//! The file is rewritten only when its content changes; otherwise only its
//! modification time is refreshed, so [`GameRegistry::stale`] can compare it
//! with the game's sources.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use box_core::reflect::Registry;
use serde_json::{Value as Json, json};

use crate::Game;
use crate::player::{NativeCrate, shell_line};
use crate::project::{ENGINE_VERSION, Project, write_atomic};

/// The file name inside the project's `.box/` folder.
pub const REGISTRY_FILE: &str = "registry.json";

/// Version of the file's own layout (the type entries follow `to_json`).
/// 2 added `modules`; a format-1 file reads as stale.
pub const REGISTRY_FORMAT: u64 = 2;

/// `<project>/.box/registry.json`.
pub fn registry_path(project: &Project) -> PathBuf {
    project.local_dir().join(REGISTRY_FILE)
}

/// FNV-1a, 64 bits: a stable content hash (std's hashers may change
/// between Rust releases).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The hash of a registry dump's types (`components` and `resources`, with
/// their stored-data versions), modules and type `renames`.
fn types_hash(dump: &Json) -> String {
    let mut types = json!({
        "components": dump["components"],
        "resources": dump["resources"],
        "modules": dump["modules"],
    });
    if let Some(renames) = dump.get("renames") {
        types["renames"] = renames.clone();
    }
    format!("{:016x}", fnv1a(types.to_string().as_bytes()))
}

/// The file's contents for `registry`: its dump plus `format`,
/// `engine_version`, the `modules` this build includes and `hash`.
pub fn registry_json(registry: &Registry) -> Json {
    let mut dump = registry.to_json();
    if let Some(o) = dump.as_object_mut() {
        o.insert("modules".into(), json!(crate::modules::enabled_features()));
    }
    let hash = types_hash(&dump);
    if let Some(o) = dump.as_object_mut() {
        o.insert("format".into(), json!(REGISTRY_FORMAT));
        o.insert("engine_version".into(), json!(ENGINE_VERSION));
        o.insert("hash".into(), json!(hash));
    }
    dump
}

/// What [`write_registry`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    /// The file was new or its content changed.
    Written,
    /// Same content: only the modification time was refreshed.
    Unchanged,
}

/// Writes `<project>/.box/registry.json` for `registry` (atomically), or,
/// when the file already holds the same types for this engine version,
/// only refreshes its modification time.
pub fn write_registry(project: &Project, registry: &Registry) -> std::io::Result<Written> {
    let path = registry_path(project);
    let new = registry_json(registry);
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(old) = serde_json::from_str::<Json>(&text)
        && old["hash"] == new["hash"]
        && old["engine_version"] == new["engine_version"]
        && old["format"] == new["format"]
    {
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(SystemTime::now())?;
        return Ok(Written::Unchanged);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut bytes = serde_json::to_vec_pretty(&new).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(&path, &bytes)?;
    Ok(Written::Written)
}

/// Whether a game's binary should keep its project's registry file current:
/// a native project (a `Cargo.toml` at its root) run by a binary with game
/// plugins. The stock editor and player (no plugins) never write it, so
/// opening a native project with them cannot replace the game's types with
/// the engine's alone.
pub fn should_write(game: &Game, project: &Project) -> bool {
    !game.plugins().is_empty() && NativeCrate::find(project.root()).is_some()
}

/// [`write_registry`] at the start of a game's editor or player when
/// [`should_write`] says so, and (with the `script` feature) the script
/// type definitions `.box/types/box.d.luau`; failures are logged, not
/// fatal.
pub fn write_at_startup(game: &Game, project: &Project) {
    // An exported game (read from a pak) has no project folder to write to.
    if project.is_packed() || !should_write(game, project) {
        return;
    }
    match write_registry(project, &game.registry()) {
        Ok(Written::Written) => tracing::info!(
            "game registry written to {} (for `box` and `box mcp`)",
            project.relative(&registry_path(project))
        ),
        Ok(Written::Unchanged) => {}
        Err(e) => tracing::warn!(
            "could not write the game registry {}: {e}",
            registry_path(project).display()
        ),
    }
    // The script type definitions with the game's own types and script APIs
    // (`box script types` can only use the registry file's schemas).
    #[cfg(feature = "script")]
    if let Err(e) = box_script::types::write_definitions(project.root(), &game.script_definitions())
    {
        tracing::warn!("could not write the script type definitions: {e}");
    }
}

/// A project's game registry file, as read by tools.
#[derive(Clone, Debug)]
pub struct GameRegistry {
    /// The optional engine modules the game was built with (`modules`);
    /// `None` for a file that does not list them (format 1).
    pub modules: Option<Vec<String>>,
    pub path: PathBuf,
    /// The whole file (`components`, `resources`, `engine_version`, `hash`).
    pub json: Json,
    /// Why the file may not match the game's code: written by another
    /// engine version, or older than the game's sources. `None` when
    /// current.
    pub stale: Option<String>,
}

/// The command that writes a project's registry file: its player with
/// `--write-registry` (or opening its editor).
pub fn refresh_command(project: &Project) -> String {
    match NativeCrate::find(project.root()) {
        Some(krate) => match krate.player_bin().or(krate.editor_bin()) {
            Some(bin) => format!(
                "{} (or open the project in the game's editor)",
                krate.shell_line_in_dir(&krate.cargo_run(bin, &["--write-registry".into()]))
            ),
            None => "run the game's editor or player with --write-registry".into(),
        },
        None => format!(
            "{} --write-registry",
            shell_line(&["<the game's player>".to_string()])
        ),
    }
}

/// The newest modification time of the game's sources: `Cargo.toml` and
/// every `.rs` file under `src/`.
fn newest_source(root: &Path) -> Option<SystemTime> {
    fn walk(dir: &Path, newest: &mut Option<SystemTime>, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() && depth < 8 {
                walk(&path, newest, depth + 1);
            } else if path.extension().is_some_and(|x| x == "rs")
                && let Ok(t) = e.metadata().and_then(|m| m.modified())
            {
                *newest = Some(newest.map_or(t, |n| n.max(t)));
            }
        }
    }
    let mut newest = std::fs::metadata(root.join("Cargo.toml"))
        .and_then(|m| m.modified())
        .ok();
    walk(&root.join("src"), &mut newest, 0);
    newest
}

/// Reads `<project>/.box/registry.json`. `Ok(None)` when there is none;
/// `Err` with a message when it cannot be read or parsed.
pub fn read_registry(project: &Project) -> Result<Option<GameRegistry>, String> {
    let path = registry_path(project);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let json: Json = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if !json["components"].is_array() || !json["resources"].is_array() {
        return Err(format!(
            "{}: not a registry dump (no components/resources lists)",
            path.display()
        ));
    }
    let written = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let version = json["engine_version"].as_str().unwrap_or("?");
    let stale = if json["format"].as_u64() != Some(REGISTRY_FORMAT) {
        Some(format!(
            "it has format {} (this box reads {REGISTRY_FORMAT})",
            json["format"]
        ))
    } else if version != ENGINE_VERSION {
        Some(format!(
            "it was written by engine {version} (this box is {ENGINE_VERSION})"
        ))
    } else {
        match (written, newest_source(project.root())) {
            (Some(w), Some(src)) if src > w => {
                Some("the game's sources changed after it was written".into())
            }
            _ => None,
        }
    };
    let modules = json["modules"].as_array().map(|a| {
        a.iter()
            .filter_map(|m| m.as_str().map(str::to_string))
            .collect()
    });
    Ok(Some(GameRegistry {
        modules,
        path,
        json,
        stale,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::identity::Uid;

    #[test]
    fn written_once_then_only_touched_and_staleness_follows_the_sources() {
        let root = std::env::temp_dir().join(format!("box_registry_file_{}", Uid::new()));
        let project = Project::create(&root, "Registry Test").unwrap();
        assert!(read_registry(&project).unwrap().is_none());
        let registry = crate::engine_registry();
        assert_eq!(
            write_registry(&project, &registry).unwrap(),
            Written::Written
        );
        let first = std::fs::read(registry_path(&project)).unwrap();
        // Sources newer than the file: stale.
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let past = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(registry_path(&project))
            .unwrap()
            .set_modified(past)
            .unwrap();
        let read = read_registry(&project).unwrap().unwrap();
        assert!(
            read.stale.as_deref().unwrap().contains("sources changed"),
            "{:?}",
            read.stale
        );
        assert!(refresh_command(&project).contains("--write-registry"));
        // Same types: the content stays, the time is refreshed.
        assert_eq!(
            write_registry(&project, &registry).unwrap(),
            Written::Unchanged
        );
        assert_eq!(std::fs::read(registry_path(&project)).unwrap(), first);
        let read = read_registry(&project).unwrap().unwrap();
        assert_eq!(read.stale, None);
        assert_eq!(read.json["engine_version"], ENGINE_VERSION);
        assert_eq!(read.json["hash"].as_str().unwrap().len(), 16);
        let modules = read.modules.clone().unwrap();
        assert_eq!(modules, crate::modules::enabled_features());
        // Another engine version: stale.
        let mut other = read.json.clone();
        other["engine_version"] = json!("0.0.1");
        std::fs::write(registry_path(&project), other.to_string()).unwrap();
        let read = read_registry(&project).unwrap().unwrap();
        assert!(read.stale.unwrap().contains("engine 0.0.1"));
        // Rewritten for this version.
        assert_eq!(
            write_registry(&project, &registry).unwrap(),
            Written::Written
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_a_native_games_binary_writes_it() {
        let root = std::env::temp_dir().join(format!("box_registry_native_{}", Uid::new()));
        let project = Project::create(&root, "Native Test").unwrap();
        struct P;
        impl box_core::app::Plugin for P {
            fn build(&self, _: &mut box_core::app::App) {}
        }
        assert!(!should_write(&Game::new().with_plugin(P), &project));
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert!(should_write(&Game::new().with_plugin(P), &project));
        assert!(!should_write(&Game::new(), &project), "the stock binaries");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
