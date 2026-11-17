//! Save games: snapshots of the running (play) world, generalised from the
//! construction slice's save system.
//!
//! **What is saved.** The same data a scene file holds, taken from the
//! running world: every entity with a [`Uid`] that is not generated (prefab
//! contents, derived parts) and not under a [`SaveExclude`] entity, with its
//! saved components (reflection, as in scenes), and every saved resource.
//! Entities loaded from scenes have a uid; give runtime-spawned entities one
//! by adding [`Saveable`] (or a `Uid`). Which scene instance each entity
//! belongs to is recorded, so runtime scene bookkeeping comes back too.
//! State that is not in saved components or resources (a script's private
//! variables, an inventory kept in a plain resource) is saved through
//! [`SaveHooks`]: named save and load callbacks, each owning one JSON value.
//! With the `script` feature every `Game::app` has the `scripts` hook: each
//! running script's `on_save(self)` result, handed back to its
//! `on_load(self, data)` after a load (`box_script::save_script_state` /
//! `load_script_state`), so a save restores entity and script state.
//!
//! **File.** `<dir>/<slot>.save.json` (`dir` from [`SaveConfig`], default
//! `<project>/.box/saves`):
//!
//! ```json
//! { "format": 1, "game": "collector", "version": 2, "saved_at": 1763200000,
//!   "scenes": [{ "path": "assets/scenes/arena.scene.json", "additive": false }],
//!   "resources": { ... }, "entities": [ { "uid": "…", "scene": 0, ... } ],
//!   "hooks": { "inventory": { ... } } }
//! ```
//!
//! Writes are atomic: a temporary file is renamed over the slot after the
//! previous file was copied to `<slot>.save.json.bak`; loading falls back to
//! the `.bak` when the slot cannot be read or parsed (a torn write), never
//! when a readable save is refused. A file from another game (a
//! different `game`), a newer `version` or a newer `format` is refused;
//! an older `version` goes through [`SaveConfig::migrate`] first.
//!
//! **Transient entities** ([`Transient`], `world.spawn{ transient = true }`
//! in scripts) are runtime-only: they and their descendants are never
//! written to a save, and loading a save despawns them, whatever
//! [`SaveExclude`] or [`Scenes::runtime_entities`] say. Use them for
//! spawned enemies, projectiles and effects: after a load the game state
//! is what the file holds, and the scripts that own such entities spawn
//! them again from their restored state.
//!
//! **Loading** is validated in a scratch world first, so a bad file changes
//! nothing. Then every entity that would be saved (and any live entity with
//! a uid the file contains) is despawned, every transient entity is
//! despawned, runtime entities without a uid
//! follow [`Scenes::runtime_entities`] as on a scene switch, the file's
//! entities are spawned (their `on_add` hooks run as for any spawn, after
//! the request system), saved resources are replaced, the scene instance
//! list is restored, and each hook's `load` runs with its value.
//!
//! **When.** [`SaveGames::save`] / [`SaveGames::load`] queue requests that
//! apply at the start of the next `Update` (after scene requests), so a
//! save never sees a half-updated frame; results arrive as [`SaveEvent`]s.
//! [`save_game`] / [`load_game`] act immediately (tools, tests).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use box_core::event::Events;
use box_core::identity::Uid;
use box_core::prefab::is_generated;
use box_core::prelude::*;
use box_core::scene;
use serde_json::{Map, Value as Json, json};

use crate::scenes::{Persistent, RuntimeEntities, SceneInstance, Scenes};
use box_core::visibility::Transient;

/// The save file format this engine writes and reads.
pub const SAVE_FORMAT: u64 = 1;

/// Component (runtime only): a runtime-spawned entity that save games keep.
/// It gets a fresh [`Uid`] when added (and at the latest at the next save)
/// if it has none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Saveable;

/// Component (runtime only): leave this entity and its descendants out of
/// save games; loading leaves them as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveExclude;

/// Upgrades a save of an older game version in place: `(from_version,
/// whole file JSON)`.
pub type Migrate = Arc<dyn Fn(u32, &mut Json) -> Result<(), String> + Send + Sync>;

/// Resource: where and how the game saves.
#[derive(Clone)]
pub struct SaveConfig {
    /// Folder of the slots; `None` is `<project>/.box/saves` (the
    /// [`Scenes`] project root; for an exported game
    /// `<`[`data_dir`](crate::packed::data_dir)`>/saves`), else `saves` in
    /// the working directory.
    pub dir: Option<PathBuf>,
    /// Written into saves; a save of another game is refused. Empty accepts
    /// any.
    pub game: String,
    /// The game's save version. Bump it when saved data changes meaning and
    /// handle older saves in `migrate`.
    pub version: u32,
    pub migrate: Option<Migrate>,
}

impl Default for SaveConfig {
    fn default() -> Self {
        Self {
            dir: None,
            game: String::new(),
            version: 1,
            migrate: None,
        }
    }
}

impl std::fmt::Debug for SaveConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SaveConfig")
            .field("dir", &self.dir)
            .field("game", &self.game)
            .field("version", &self.version)
            .field("migrate", &self.migrate.is_some())
            .finish()
    }
}

type SaveFn = Box<dyn Fn(&mut World) -> Json>;
type LoadFn = Box<dyn Fn(&mut World, &Json)>;

/// Resource: named save and load callbacks for state outside saved
/// components and resources (scripts register theirs here too). `save`
/// returns the hook's JSON value; `load` gets it back after the entities
/// and resources were restored. A save without a hook's value does not
/// call its `load`.
#[derive(Default)]
pub struct SaveHooks {
    hooks: Vec<(String, SaveFn, LoadFn)>,
}

impl SaveHooks {
    /// Adds (or replaces) the hook `name`.
    pub fn add(
        &mut self,
        name: &str,
        save: impl Fn(&World) -> Json + 'static,
        load: impl Fn(&mut World, &Json) + 'static,
    ) {
        self.add_mut(name, move |w: &mut World| save(w), load);
    }

    /// [`add`](Self::add) for a `save` that needs the world mutably (the
    /// script runtime's `on_save` callbacks run with it).
    pub fn add_mut(
        &mut self,
        name: &str,
        save: impl Fn(&mut World) -> Json + 'static,
        load: impl Fn(&mut World, &Json) + 'static,
    ) {
        self.hooks.retain(|(n, _, _)| n != name);
        self.hooks
            .push((name.to_string(), Box::new(save), Box::new(load)));
    }

    pub fn remove(&mut self, name: &str) {
        self.hooks.retain(|(n, _, _)| n != name);
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.hooks.iter().map(|(n, _, _)| n.as_str())
    }
}

/// What happened to a save request.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveEvent {
    Saved {
        slot: String,
        path: PathBuf,
    },
    Loaded {
        slot: String,
        path: PathBuf,
        entities: usize,
    },
    Failed {
        slot: String,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum Request {
    Save(String),
    Load(String),
}

/// A slot on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub slot: String,
    pub path: PathBuf,
    pub game: String,
    pub version: u32,
    /// Unix seconds.
    pub saved_at: u64,
    /// The main scene's path as saved.
    pub scene: Option<String>,
}

/// Resource: save and load requests (see the module docs).
#[derive(Debug, Default)]
pub struct SaveGames {
    pending: Vec<Request>,
}

impl SaveGames {
    /// Saves to `slot` at the start of the next `Update`.
    pub fn save(&mut self, slot: &str) {
        self.pending.push(Request::Save(slot.to_string()));
    }

    /// Loads `slot` at the start of the next `Update`.
    pub fn load(&mut self, slot: &str) {
        self.pending.push(Request::Load(slot.to_string()));
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Errors of saving and loading.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("slot names use letters, digits, `-` and `_` (got `{0}`)")]
    BadSlot(String),
    #[error("{path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("not a save file: {0}")]
    Format(String),
    #[error("save of another game (`{found}`, this is `{expected}`)")]
    OtherGame { found: String, expected: String },
    #[error("save version {found} is newer than this game's {supported}")]
    NewerVersion { found: u64, supported: u64 },
    #[error("migrating a version {from} save failed: {message}")]
    Migration { from: u32, message: String },
    #[error(transparent)]
    Scene(#[from] scene::SceneError),
}

/// Installs [`SaveGames`], [`SaveHooks`], [`SaveConfig`],
/// `Events<SaveEvent>` and the request system (start of `Update`).
pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<SaveGames>();
        app.world.resource_or_default::<SaveHooks>();
        app.world.resource_or_default::<SaveConfig>();
        app.add_event::<SaveEvent>();
        app.system(Stage::Update, apply_save_requests)
            .label("save_requests");
        // Runtime saveables get their uid at once, so they can be referred
        // to (a world label by uid) before the first save.
        app.on_add::<Saveable>(|w, e| {
            if !w.has::<Uid>(e) {
                let _ = w.insert_one(e, Uid::new());
            }
        });
    }
}

/// Applies queued [`SaveGames`] requests in order. Idle cost: one check.
pub fn apply_save_requests(world: &mut World) {
    let Some(games) = world.get_resource_mut::<SaveGames>() else {
        return;
    };
    if games.pending.is_empty() {
        return;
    }
    for request in std::mem::take(&mut games.pending) {
        let event = match request {
            Request::Save(slot) => match slot_path(world, &slot).and_then(|p| {
                save_game(world, &p)?;
                Ok(p)
            }) {
                Ok(path) => SaveEvent::Saved { slot, path },
                Err(e) => fail(slot, e),
            },
            Request::Load(slot) => {
                match slot_path(world, &slot).and_then(|p| load_game(world, &p).map(|n| (p, n))) {
                    Ok((path, entities)) => SaveEvent::Loaded {
                        slot,
                        path,
                        entities,
                    },
                    Err(e) => fail(slot, e),
                }
            }
        };
        if let Some(events) = world.get_resource_mut::<Events<SaveEvent>>() {
            events.send(event);
        }
    }
}

fn fail(slot: String, e: SaveError) -> SaveEvent {
    tracing::error!(target: "box_engine::save", "save slot `{slot}`: {e}");
    SaveEvent::Failed {
        slot,
        message: e.to_string(),
    }
}

/// The folder slots live in (see [`SaveConfig::dir`]).
pub fn save_dir(world: &World) -> PathBuf {
    if let Some(d) = world
        .get_resource::<SaveConfig>()
        .and_then(|c| c.dir.clone())
    {
        return d;
    }
    match world
        .get_resource::<Scenes>()
        .and_then(|s| s.project_root())
    {
        // An exported game cannot write into its pak.
        Some(root) if box_assets::vfs::is_read_only(root) => {
            crate::packed::data_dir().map_or_else(|| PathBuf::from("saves"), |d| d.join("saves"))
        }
        Some(root) => root.join(".box").join("saves"),
        None => PathBuf::from("saves"),
    }
}

/// The file of `slot`.
pub fn slot_path(world: &World, slot: &str) -> Result<PathBuf, SaveError> {
    let ok = !slot.is_empty()
        && slot.len() <= 64
        && slot
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(SaveError::BadSlot(slot.to_string()));
    }
    Ok(save_dir(world).join(format!("{slot}.save.json")))
}

/// The slots in the save folder, newest first. Files that do not read are
/// skipped (with a warning).
pub fn list_slots(world: &World) -> Vec<SlotInfo> {
    let dir = save_dir(world);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Some(slot) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".save.json"))
            .map(str::to_string)
        else {
            continue;
        };
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<Json>(&b).map_err(|e| e.to_string()))
        {
            Ok(j) => out.push(SlotInfo {
                slot,
                path,
                game: j["game"].as_str().unwrap_or("").to_string(),
                version: j["version"].as_u64().unwrap_or(0) as u32,
                saved_at: j["saved_at"].as_u64().unwrap_or(0),
                scene: j["scenes"][0]["path"].as_str().map(str::to_string),
            }),
            Err(err) => {
                tracing::warn!(target: "box_engine::save", "{}: {err}", path.display())
            }
        }
    }
    out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(a.slot.cmp(&b.slot)));
    out
}

/// Entities under (or being) a [`SaveExclude`] entity.
fn excluded(world: &World) -> std::collections::HashSet<Entity> {
    let mut out = std::collections::HashSet::new();
    let mut stack: Vec<Entity> = world
        .query::<(Entity, &SaveExclude)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    while let Some(e) = stack.pop() {
        if out.insert(e) {
            stack.extend(world.children(e));
        }
    }
    out
}

/// A path as written into a save: relative to the project when below it.
fn portable(world: &World, path: &Path) -> String {
    let root = world
        .get_resource::<Scenes>()
        .and_then(|s| s.project_root());
    root.and_then(|r| path.strip_prefix(r).ok())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The save file JSON of `world` (gives [`Saveable`] entities a uid first).
pub fn snapshot(world: &mut World) -> Result<Json, SaveError> {
    profiling::scope!("save snapshot");
    let missing: Vec<Entity> = world
        .query::<(Entity, &Saveable)>()
        .without::<&Uid>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in missing {
        let _ = world.insert_one(e, Uid::new());
    }
    let bytes = scene::save(world)?;
    let mut tree: Json = serde_json::from_slice(&bytes)
        .map_err(|e| SaveError::Format(format!("scene JSON: {e}")))?;
    let excluded: std::collections::HashSet<Uid> = excluded(world)
        .into_iter()
        .filter_map(|e| world.uid(e))
        .collect();
    let (instances, order): (Vec<Json>, Vec<crate::scenes::SceneInstanceId>) = world
        .get_resource::<Scenes>()
        .map(|s| {
            s.instances()
                .iter()
                .map(|i| {
                    (
                        json!({
                            "path": i.path.as_deref().map(|p| portable(world, p)),
                            "additive": i.additive,
                        }),
                        i.id,
                    )
                })
                .unzip()
        })
        .unwrap_or_default();
    let mut entities = Vec::new();
    if let Some(Json::Array(records)) = tree.get_mut("entities").map(Json::take) {
        for mut rec in records {
            let Some(uid) = rec.get("uid").and_then(Json::as_str).and_then(Uid::parse) else {
                continue;
            };
            if excluded.contains(&uid) {
                continue;
            }
            let instance = world
                .entity_by_uid(uid)
                .and_then(|e| world.get::<SceneInstance>(e).map(|s| s.0))
                .and_then(|id| order.iter().position(|o| *o == id));
            if let Some(obj) = rec.as_object_mut() {
                obj.insert("scene".into(), instance.map_or(Json::Null, |i| json!(i)));
            }
            entities.push(rec);
        }
    }
    let config = world
        .get_resource::<SaveConfig>()
        .cloned()
        .unwrap_or_default();
    let mut hooks = Map::new();
    if let Some(h) = world.remove_resource::<SaveHooks>() {
        for (name, save, _) in &h.hooks {
            hooks.insert(name.clone(), save(world));
        }
        world.insert_resource(h);
    }
    let saved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(json!({
        "format": SAVE_FORMAT,
        "game": config.game,
        "version": config.version,
        "saved_at": saved_at,
        "scenes": instances,
        "resources": tree.get("resources").cloned().unwrap_or(json!({})),
        "entities": entities,
        "hooks": hooks,
    }))
}

/// Writes `bytes` to `path` atomically, keeping the previous file as
/// `<path>.bak`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), SaveError> {
    let io = |p: &Path, e: std::io::Error| SaveError::Io {
        path: p.to_path_buf(),
        message: crate::io_hint::text(&e),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    }
    let tmp = with_suffix(path, ".tmp");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp).map_err(|e| io(&tmp, e))?;
        f.write_all(bytes).map_err(|e| io(&tmp, e))?;
        f.sync_all().map_err(|e| io(&tmp, e))?;
    }
    if path.exists() {
        let bak = with_suffix(path, ".bak");
        std::fs::copy(path, &bak).map_err(|e| io(&bak, e))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))?;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Saves `world` to `path` now.
pub fn save_game(world: &mut World, path: &Path) -> Result<(), SaveError> {
    let json = snapshot(world)?;
    let bytes = scene::to_pretty_json(&json)
        .map_err(|e| SaveError::Format(format!("could not write JSON: {e}")))?;
    write_atomic(path, &bytes)?;
    tracing::info!(target: "box_engine::save", "saved {}", path.display());
    Ok(())
}

/// Reads and checks a save file (migrating older versions).
pub fn read_save(world: &World, path: &Path) -> Result<Json, SaveError> {
    let read = |p: &Path| -> Result<Json, SaveError> {
        let bytes = std::fs::read(p).map_err(|e| SaveError::Io {
            path: p.to_path_buf(),
            message: e.to_string(),
        })?;
        let json: Json = serde_json::from_slice(&bytes)
            .map_err(|e| SaveError::Format(format!("{}: {e}", p.display())))?;
        check(world, json)
    };
    match read(path) {
        Ok(j) => Ok(j),
        // Only an unreadable or torn file falls back: a save refused for
        // its game or version must not silently load an older backup.
        Err(e @ (SaveError::Io { .. } | SaveError::Format(_))) => {
            let bak = with_suffix(path, ".bak");
            if bak.exists() {
                tracing::warn!(target: "box_engine::save", "{e}; trying the backup {}", bak.display());
                read(&bak)
            } else if let SaveError::Format(m) = e {
                Err(SaveError::Format(format!(
                    "{m} (the save file is damaged and there is no backup ({}); delete it to start over)",
                    bak.display()
                )))
            } else {
                Err(e)
            }
        }
        Err(e) => Err(e),
    }
}

fn check(world: &World, mut json: Json) -> Result<Json, SaveError> {
    let format = json.get("format").and_then(Json::as_u64).unwrap_or(0);
    if format == 0 {
        return Err(SaveError::Format("no \"format\"".into()));
    }
    if format > SAVE_FORMAT {
        return Err(SaveError::NewerVersion {
            found: format,
            supported: SAVE_FORMAT,
        });
    }
    let config = world
        .get_resource::<SaveConfig>()
        .cloned()
        .unwrap_or_default();
    let game = json.get("game").and_then(Json::as_str).unwrap_or("");
    if !config.game.is_empty() && game != config.game {
        return Err(SaveError::OtherGame {
            found: game.to_string(),
            expected: config.game.clone(),
        });
    }
    let version = json.get("version").and_then(Json::as_u64).unwrap_or(1);
    if version > u64::from(config.version) {
        return Err(SaveError::NewerVersion {
            found: version,
            supported: u64::from(config.version),
        });
    }
    if version < u64::from(config.version) {
        let from = version as u32;
        let Some(migrate) = &config.migrate else {
            return Err(SaveError::Migration {
                from,
                message: format!(
                    "the game is at version {} and has no SaveConfig::migrate",
                    config.version
                ),
            });
        };
        migrate(from, &mut json).map_err(|message| SaveError::Migration { from, message })?;
        json["version"] = json!(config.version);
        tracing::info!(target: "box_engine::save", "migrated a version {from} save to {}", config.version);
    }
    if !json.get("entities").is_some_and(Json::is_array) {
        return Err(SaveError::Format("no \"entities\" array".into()));
    }
    // Engine and game type steps (renames, `"$version"`), as for scenes.
    let mut report = box_core::migrate::TreeReport::default();
    box_core::migrate::upgrade_scene_types(world.registry(), &mut json, &mut report)
        .map_err(|e| SaveError::Format(e.to_string()))?;
    if !report.is_empty() {
        tracing::info!(target: "box_engine::save", "upgraded saved data: {}", report.lines().join("; "));
    }
    Ok(json)
}

/// Loads the save at `path` into `world` now. Returns the number of
/// entities spawned.
pub fn load_game(world: &mut World, path: &Path) -> Result<usize, SaveError> {
    let json = read_save(world, path)?;
    let n = restore(world, &json)?;
    tracing::info!(target: "box_engine::save", "loaded {} ({n} entities)", path.display());
    Ok(n)
}

/// Replaces `world`'s saved state with a checked save JSON (see the module
/// docs). Nothing changes when the entities do not instantiate.
pub fn restore(world: &mut World, save: &Json) -> Result<usize, SaveError> {
    profiling::scope!("save restore");
    let records: Vec<Json> = save["entities"].as_array().cloned().unwrap_or_default();
    // Validate first: a bad file must change nothing.
    let mut scratch = World::with_registry(world.registry().clone());
    let mut clean: Vec<Json> = records.clone();
    for r in &mut clean {
        if let Some(o) = r.as_object_mut() {
            o.remove("scene");
        }
    }
    scene::instantiate(&mut scratch, &clean, None)?;
    drop(scratch);

    // Despawn what the file replaces.
    let excluded = excluded(world);
    let file_uids: std::collections::HashSet<Uid> = clean
        .iter()
        .filter_map(|r| r.get("uid").and_then(Json::as_str).and_then(Uid::parse))
        .collect();
    let doomed: Vec<Entity> = world
        .query::<(Entity, &Uid)>()
        .iter()
        .filter(|(e, u)| {
            file_uids.contains(u) || (!excluded.contains(e) && !is_generated(world, *e))
        })
        .map(|(e, _)| e)
        .collect();
    for e in doomed {
        world.despawn(e);
    }
    // Saveables that did not get their uid yet would be saved too.
    let saveables: Vec<Entity> = world
        .query::<(Entity, &Saveable)>()
        .without::<&Uid>()
        .iter()
        .map(|(e, _)| e)
        .filter(|e| !excluded.contains(e))
        .collect();
    for e in saveables {
        world.despawn(e);
    }
    // Runtime-only entities (and their subtrees) never come back from a save.
    let transient: Vec<Entity> = world
        .query::<(Entity, &Transient)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in transient {
        world.despawn(e);
    }
    let policy = world
        .get_resource::<Scenes>()
        .map_or(RuntimeEntities::KeepAll, Scenes::runtime_entities);
    if policy == RuntimeEntities::DespawnUnlessPersistent {
        let runtime: Vec<Entity> = world
            .query::<Entity>()
            .without::<&SceneInstance>()
            .without::<&Persistent>()
            .without::<&Parent>()
            .without::<&Uid>()
            .without::<&SaveExclude>()
            .iter()
            .collect();
        for e in runtime {
            world.despawn(e);
        }
    }

    // Spawn the file's entities and restore scene bookkeeping.
    let spawned = scene::instantiate(world, &clean, None)?;
    let root = world
        .get_resource::<Scenes>()
        .and_then(|s| s.project_root().map(Path::to_path_buf));
    let list: Vec<(Option<PathBuf>, bool)> = save["scenes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    let path = s["path"].as_str().map(|p| {
                        let p = PathBuf::from(p);
                        match (&root, p.is_absolute()) {
                            (Some(r), false) => r.join(p),
                            _ => p,
                        }
                    });
                    (path, s["additive"].as_bool().unwrap_or(false))
                })
                .collect()
        })
        .unwrap_or_default();
    let ids = world
        .resource_or_default::<Scenes>()
        .restore_instances(&list);
    for (e, rec) in spawned.iter().zip(&records) {
        if let Some(id) = rec["scene"].as_u64().and_then(|i| ids.get(i as usize)) {
            let _ = world.insert_one(*e, SceneInstance(*id));
        }
    }

    // Resources.
    if let Some(resources) = save["resources"].as_object() {
        let registry = world.registry().clone();
        for (name, value) in resources {
            let Some(ty) = registry.resource(name) else {
                tracing::warn!(target: "box_engine::save", "saved resource `{name}` is not registered (skipped)");
                continue;
            };
            let mut value = value.clone();
            box_core::migrate::strip_version(&registry, name, &mut value);
            let extra =
                box_core::migrate::split_unknown_fields(&registry, ty.type_id(), &mut value);
            if !extra.is_empty() {
                tracing::warn!(target: "box_engine::save", "saved resource `{name}`: unknown fields {:?} kept", extra.keys().collect::<Vec<_>>());
                world
                    .resource_or_default::<scene::UnknownResourceFields>()
                    .0
                    .insert(name.clone(), extra);
            }
            match registry.value_from_json(ty.type_id(), &value) {
                Ok(v) => {
                    if let Err(e) = world.insert_resource_reflect(name, v.as_ref()) {
                        tracing::warn!(target: "box_engine::save", "resource `{name}`: {e}");
                    }
                }
                Err(e) => tracing::warn!(target: "box_engine::save", "resource `{name}`: {e}"),
            }
        }
    }

    // Hooks.
    if let Some(hooks) = world.remove_resource::<SaveHooks>() {
        for (name, _, load) in &hooks.hooks {
            match save["hooks"].get(name) {
                Some(v) => load(world, v),
                None => {
                    tracing::debug!(target: "box_engine::save", "save has no value for hook `{name}`")
                }
            }
        }
        world.insert_resource(hooks);
    }
    Ok(spawned.len())
}
