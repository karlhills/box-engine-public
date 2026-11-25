//! Project settings resources and the project's input action map file, both
//! data files under `assets/settings/` that hot-reload.
//!
//! **Settings resources.** A game registers tunables as a resource with
//! [`Registry::register_settings`] (it needs `#[reflect(Resource, Default)]`).
//! Each lives in `assets/settings/<TypeName>.json`, the type's reflected JSON
//! form (fields missing from the file take their defaults). Settings are
//! never saved in scenes, so every scene and arena shares them.
//!
//! - [`load_settings`] reads them into a world: [`Game::app`](crate::Game::app)
//!   calls it before the game's plugins are built (player, Play, tests with
//!   an asset server), so plugins can read them; the editor calls it for its
//!   edit world with [`Missing::CreateFile`], which writes a file with the
//!   defaults for each settings type that has none. The player never writes.
//! - [`SettingsPlugin`] (in every [`app_with_world`](crate::app_with_world))
//!   applies changed files: the `json` importer turns them into
//!   [`JsonAsset`]s, and a hot reload of one replaces the resource. A file
//!   that does not parse or does not fit the type keeps the last good value
//!   (with a warning).
//!
//! **Action map.** `assets/settings/input.actions.json` holds an
//! [`ActionMap`] in its JSON form. After the plugins are built (they add
//! their code defaults, ideally with [`ActionMap::merge_missing`]), the file
//! is applied over the defaults: actions in the file replace the game's
//! bindings for those actions; actions it leaves out keep the defaults. Hot
//! reload applies the file again over the same defaults; deleting the file
//! goes back to them. The editor has no binding editor yet: edit the file.

use std::path::{Path, PathBuf};

use box_assets::{AssetEvent, AssetEventKind, AssetServer, JsonAsset};
use box_core::action::ActionMap;
use box_core::app::{App, Plugin, Stage};
use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::event::{EventCursor, Events};
use box_core::reflect::{PartialReflect, Registry, Value};
use box_core::world::World;
use serde_json::{Map, Value as Json};

use crate::asset_glue::AssetMirror;
use crate::project::write_atomic;

/// Folder of settings files, relative to `assets/`.
pub const SETTINGS_DIR: &str = "settings";

/// The input action map file, relative to `assets/`.
pub const ACTIONS_FILE: &str = "settings/input.actions.json";

/// The settings file of the settings type `name`, relative to `assets/`:
/// `settings/<name>.json`.
pub fn settings_file(name: &str) -> String {
    format!("{SETTINGS_DIR}/{name}.json")
}

/// What [`load_settings`] does for a settings type without a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// Use the type's default value (player, Play, tests).
    UseDefault,
    /// Use the default and write it to a new file (the editor).
    CreateFile,
}

/// One watched file.
#[derive(Debug, Default)]
struct Watched {
    /// Path relative to `assets/`.
    rel: String,
    handle: Option<Handle<JsonAsset>>,
    /// The JSON last applied, to skip reloads that change nothing.
    applied: Option<Json>,
    /// JSON this process wrote (the editor): their reloads are echoes.
    echoes: Vec<Json>,
}

impl Watched {
    fn new(rel: String) -> Self {
        Self {
            rel,
            ..Default::default()
        }
    }

    /// True (and forgotten) when `json` is a write of this process coming
    /// back, or what is already applied.
    fn is_echo(&mut self, json: &Json) -> bool {
        if let Some(i) = self.echoes.iter().position(|e| e == json) {
            self.echoes.drain(..=i);
            return true;
        }
        self.applied.as_ref() == Some(json)
    }
}

/// Resource: the settings files a world follows (see the module docs).
#[derive(Default)]
pub struct ProjectSettings {
    assets_dir: PathBuf,
    /// Settings type name and its file.
    settings: Vec<(&'static str, Watched)>,
    actions: Watched,
    /// The game's code defaults, kept to apply the file over them again.
    action_defaults: Option<ActionMap>,
    cursor: EventCursor<AssetEvent>,
    /// Asset index generation when files without a handle were last looked for.
    index_generation: Option<u64>,
}

impl ProjectSettings {
    /// The assets folder the files are read from.
    pub fn assets_dir(&self) -> &Path {
        &self.assets_dir
    }

    /// Marks `json` as written to the settings file of `name` by this
    /// process (the editor), so its hot reload is not applied over newer
    /// in-memory edits.
    pub fn expect_echo(&mut self, name: &str, json: Json) {
        if let Some((_, w)) = self.settings.iter_mut().find(|(n, _)| *n == name) {
            w.applied = Some(json.clone());
            w.echoes.push(json);
            if w.echoes.len() > 16 {
                w.echoes.remove(0);
            }
        }
    }
}

/// The server that loads for this world: its own, or the edit world's for a
/// Play world.
fn loader(world: &World) -> Option<AssetServer> {
    world
        .get_resource::<AssetServer>()
        .cloned()
        .or_else(|| world.get_resource::<AssetMirror>().map(|m| m.0.clone()))
}

/// The type's default value as JSON.
fn default_json(registry: &Registry, name: &str) -> Option<Json> {
    let ty = registry.resource(name)?;
    let default = ty.default_value()?;
    registry.value_to_json(default.as_ref()).ok()
}

/// `file` over the default (both objects): the file's fields replace the
/// defaults; fields the type does not have are dropped with a warning.
fn over_default(name: &str, default: &Json, file: &Json) -> Result<Json, String> {
    let (Json::Object(d), Json::Object(f)) = (default, file) else {
        return if file.is_object() || !default.is_object() {
            Ok(file.clone())
        } else {
            Err(format!(
                "expected an object with the fields of `{name}`, got {file}"
            ))
        };
    };
    let mut out: Map<String, Json> = d.clone();
    for (k, v) in f {
        if d.contains_key(k) {
            out.insert(k.clone(), v.clone());
        } else {
            tracing::warn!(
                "{}: `{name}` has no field `{k}` (ignored; kept in the file)",
                settings_file(name)
            );
        }
    }
    Ok(Json::Object(out))
}

/// The settings value for `name` from file JSON (upgraded through the
/// type's migration steps first, in memory).
fn settings_value(registry: &Registry, name: &str, file: &Json) -> Result<Value, String> {
    let ty = registry
        .resource(name)
        .ok_or_else(|| format!("`{name}` is not registered"))?;
    let default = default_json(registry, name).unwrap_or(Json::Null);
    let mut file = file.clone();
    box_core::migrate::upgrade_value(registry, name, &mut file).map_err(|e| e.to_string())?;
    box_core::migrate::strip_version(registry, name, &mut file);
    let merged = over_default(name, &default, &file)?;
    registry
        .value_from_json(ty.type_id(), &merged)
        .map_err(|e| e.to_string())
}

/// Applies settings file JSON to the world's resource. Errors keep the
/// current value.
fn apply_settings_json(world: &mut World, name: &str, file: &Json) -> Result<(), String> {
    let registry = world.registry().clone();
    let value = settings_value(&registry, name, file)?;
    world
        .insert_resource_reflect(name, value.as_ref())
        .map_err(|e| e.to_string())
}

/// Pretty JSON with a final newline, as settings files are written.
pub fn pretty_json(json: &Json) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(json).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// Loads every settings resource of the world's registry from
/// `<assets>/settings/` into the world (see the module docs), using the
/// world's asset server (or a Play world's mirror of the editor's) for the
/// assets folder and hot reload. Does nothing without one. Returns the
/// settings files it created ([`Missing::CreateFile`]). Without a server
/// (a code-only app, a test, the editor without a project) settings
/// resources the world lacks get their defaults, so they always exist.
pub fn load_settings(world: &mut World, missing: Missing) -> Vec<String> {
    let Some(server) = loader(world) else {
        let registry = world.registry().clone();
        for ty in registry.settings() {
            if world.get_resource_reflect(ty.name()).is_err() {
                insert_default(world, &registry, ty.name());
            }
        }
        return Vec::new();
    };
    load_settings_from(world, server.assets_dir(), Some(&server), missing)
}

/// [`load_settings`] from an explicit assets folder, with an optional
/// server for hot reload.
pub fn load_settings_from(
    world: &mut World,
    assets_dir: &Path,
    server: Option<&AssetServer>,
    missing: Missing,
) -> Vec<String> {
    let registry = world.registry().clone();
    let mut created = Vec::new();
    let mut state = ProjectSettings {
        assets_dir: assets_dir.to_path_buf(),
        actions: Watched::new(ACTIONS_FILE.to_string()),
        ..Default::default()
    };
    // Reloads queued before now are not news.
    if let Some(events) = world.get_resource::<Events<AssetEvent>>() {
        state.cursor.skip_all(events);
    }
    for ty in registry.settings() {
        let name = ty.name();
        let rel = settings_file(name);
        let path = assets_dir.join(&rel);
        let mut watched = Watched::new(rel.clone());
        match box_assets::vfs::read(&path) {
            Ok(bytes) => {
                let applied = serde_json::from_slice::<Json>(&bytes)
                    .map_err(|e| e.to_string())
                    .and_then(|json| {
                        apply_settings_json(world, name, &json)?;
                        Ok(json)
                    });
                match applied {
                    Ok(json) => watched.applied = Some(json),
                    Err(e) => {
                        tracing::warn!("{rel}: {e}; using the defaults of `{name}`");
                        insert_default(world, &registry, name);
                    }
                }
            }
            Err(_) => {
                insert_default(world, &registry, name);
                if missing == Missing::CreateFile
                    && !box_assets::vfs::is_read_only(&path)
                    && let Some(mut json) = default_json(&registry, name)
                {
                    box_core::migrate::stamp_version(&registry, name, &mut json);
                    let written = path
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|()| write_atomic(&path, &pretty_json(&json)));
                    match written {
                        Ok(()) => {
                            tracing::info!("created {rel} with the defaults of `{name}`");
                            watched.applied = Some(json);
                            created.push(rel.clone());
                        }
                        Err(e) => tracing::warn!("could not create {rel}: {e}"),
                    }
                }
            }
        }
        if let Some(server) = server {
            watched.handle = load_handle(server, &rel);
        }
        state.settings.push((name, watched));
    }
    // Settings files of modules this build leaves out are not silently
    // ignored.
    for (rel, module) in
        crate::modules::left_out_settings_files(assets_dir, &crate::modules::MODULES)
    {
        tracing::warn!(
            "{}",
            crate::modules::left_out_settings_message(&rel, module)
        );
    }
    if let Some(server) = server {
        state.actions.handle = load_handle(server, ACTIONS_FILE);
        state.index_generation = Some(server.index_generation());
    }
    world.insert_resource(state);
    created
}

fn insert_default(world: &mut World, registry: &Registry, name: &str) {
    if let Some(default) = registry.resource(name).and_then(|t| t.default_value()) {
        let _ = world.insert_resource_reflect(name, default.as_ref());
    }
}

/// A handle for a data file, when it exists.
fn load_handle(server: &AssetServer, rel: &str) -> Option<Handle<JsonAsset>> {
    if server.id_of(rel).is_none() && !server.file_exists(rel) {
        return None;
    }
    server.load::<JsonAsset>(&AssetRef::path(rel)).ok()
}

/// The action map from the file (when there is one) over the game's code
/// defaults: call after the game's plugins are built. Keeps the defaults to
/// apply the file over them again on hot reload. Does nothing for a world
/// without [`ProjectSettings`].
pub fn apply_action_file(world: &mut World) {
    let Some(mut state) = world.remove_resource::<ProjectSettings>() else {
        return;
    };
    let defaults = world
        .get_resource::<ActionMap>()
        .cloned()
        .unwrap_or_default();
    let path = state.assets_dir.join(ACTIONS_FILE);
    // No file: the game's bindings as they are.
    if let Ok(bytes) = box_assets::vfs::read(&path) {
        let applied = serde_json::from_slice::<Json>(&bytes)
            .map_err(|e| e.to_string())
            .and_then(|json| Ok((actions_over(&json, &defaults)?, json)));
        match applied {
            Ok((map, json)) => {
                world.insert_resource(map);
                state.actions.applied = Some(json);
            }
            Err(e) => tracing::warn!("{ACTIONS_FILE}: {e}; using the game's default bindings"),
        }
    }
    state.action_defaults = Some(defaults);
    world.insert_resource(state);
}

/// The file's action map with the defaults merged in.
fn actions_over(json: &Json, defaults: &ActionMap) -> Result<ActionMap, String> {
    let mut map: ActionMap = serde_json::from_value(json.clone()).map_err(|e| e.to_string())?;
    map.merge_missing(defaults);
    Ok(map)
}

/// Applies settings and action files that changed since the last call
/// (hot reload). Cheap when nothing changed. The system of
/// [`SettingsPlugin`]; the editor calls it for its edit world.
pub fn reload_settings(world: &mut World) {
    let Some(mut state) = world.remove_resource::<ProjectSettings>() else {
        return;
    };
    // Files created since the last look (a new settings file, the action
    // map) get handles when the asset index changes.
    if let Some(server) = loader(world)
        && state.index_generation != Some(server.index_generation())
    {
        state.index_generation = Some(server.index_generation());
        for (_, w) in &mut state.settings {
            if w.handle.is_none() {
                w.handle = load_handle(&server, &w.rel);
            }
        }
        if state.actions.handle.is_none() {
            state.actions.handle = load_handle(&server, &state.actions.rel);
        }
    }
    let events: Vec<AssetEvent> = match world.get_resource::<Events<AssetEvent>>() {
        Some(e) => state.cursor.read(e).cloned().collect(),
        None => Vec::new(),
    };
    for e in events {
        let matches = |w: &Watched| w.handle.is_some_and(|h| e.is(h));
        let kind = e.kind;
        if let Some((name, w)) = state.settings.iter_mut().find(|(_, w)| matches(w)) {
            if !matches!(kind, AssetEventKind::Loaded | AssetEventKind::Reloaded) {
                continue;
            }
            let Some(json) = asset_json(world, w) else {
                continue;
            };
            if w.is_echo(&json) {
                continue;
            }
            match apply_settings_json(world, name, &json) {
                Ok(()) => {
                    tracing::info!("{}: reloaded `{name}`", w.rel);
                    w.applied = Some(json);
                }
                Err(err) => tracing::warn!("{}: {err}; keeping the last good `{name}`", w.rel),
            }
        } else if matches(&state.actions) {
            let defaults = state.action_defaults.clone().unwrap_or_default();
            match kind {
                AssetEventKind::Loaded | AssetEventKind::Reloaded => {
                    let Some(json) = asset_json(world, &state.actions) else {
                        continue;
                    };
                    if state.actions.is_echo(&json) {
                        continue;
                    }
                    match actions_over(&json, &defaults) {
                        Ok(map) => {
                            tracing::info!("{ACTIONS_FILE}: reloaded the input actions");
                            world.insert_resource(map);
                            state.actions.applied = Some(json);
                        }
                        Err(err) => tracing::warn!(
                            "{ACTIONS_FILE}: {err}; keeping the last good input actions"
                        ),
                    }
                }
                AssetEventKind::Removed => {
                    tracing::info!("{ACTIONS_FILE} removed: back to the game's default bindings");
                    world.insert_resource(defaults);
                    state.actions.applied = None;
                    state.actions.handle = None;
                }
                _ => {}
            }
        }
    }
    world.insert_resource(state);
}

/// The loaded JSON of a watched file.
fn asset_json(world: &World, w: &Watched) -> Option<Json> {
    let handle = w.handle?;
    world
        .get_resource::<Assets<JsonAsset>>()?
        .get(handle)
        .map(|a| a.0.clone())
}

/// Replaces the settings resource `name` in `world` with `value` (the
/// editor's Project Settings) and returns its JSON form for the file.
pub fn set_settings_value(
    world: &mut World,
    name: &str,
    value: &dyn PartialReflect,
) -> Result<Json, String> {
    world
        .insert_resource_reflect(name, value)
        .map_err(|e| e.to_string())?;
    settings_json(world, name)
}

/// The settings resource `name` in `world`, as its file's JSON (with
/// `"$version"` when the type has migration steps).
pub fn settings_json(world: &World, name: &str) -> Result<Json, String> {
    let value = world
        .get_resource_reflect(name)
        .map_err(|e| e.to_string())?;
    let registry = world.registry();
    let mut json = registry
        .value_to_json(value.as_partial_reflect())
        .map_err(|e| e.to_string())?;
    box_core::migrate::stamp_version(registry, name, &mut json);
    Ok(json)
}

/// [`settings_json`] for writing over the file at `path`: fields of the
/// current file that the type does not have (written by a newer game, or
/// notes) are kept, after the known ones.
pub fn settings_file_json(world: &World, name: &str, path: &Path) -> Result<Json, String> {
    let mut json = settings_json(world, name)?;
    let registry = world.registry();
    if let (Some(ty), Ok(bytes)) = (registry.resource(name), std::fs::read(path))
        && let Ok(file) = serde_json::from_slice::<Json>(&bytes)
    {
        let keep = box_core::migrate::unknown_fields_of(registry, ty.type_id(), &file);
        box_core::migrate::append_fields(&mut json, &keep);
    }
    Ok(json)
}

/// Adds [`reload_settings`] to `Update` (after the asset server's update):
/// settings and action files hot-reload. Added by
/// [`app_with_world`](crate::app_with_world).
pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, reload_settings);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use box_core::identity::Uid;
    use std::time::Duration;

    /// Character tunables.
    #[derive(Reflect, Clone, Debug, PartialEq)]
    #[reflect(Resource, Default, Clone)]
    struct Tuning {
        #[reflect(@Range { min: 0.0, max: 50.0 })]
        speed: f32,
        jump: f32,
    }

    impl Default for Tuning {
        fn default() -> Self {
            Self {
                speed: 6.0,
                jump: 7.5,
            }
        }
    }

    struct TuningGame;
    impl Plugin for TuningGame {
        fn register(&self, registry: &mut Registry) {
            registry.register_settings::<Tuning>().unwrap();
        }
        fn build(&self, app: &mut App) {
            // Plugins see the settings while they build.
            let speed = app.world.resource::<Tuning>().speed;
            app.world.insert_resource(BuiltWith(speed));
            app.world.resource_or_default::<ActionMap>().merge_missing(
                &ActionMap::new()
                    .button("jump", [Binding::Key(KeyCode::Space)])
                    .button("fire", [Binding::Key(KeyCode::F)]),
            );
        }
    }

    struct BuiltWith(f32);

    fn project(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("box_settings_{tag}_{}", Uid::new()));
        crate::Project::create(&root, "Settings Test").unwrap();
        root
    }

    fn app_for(root: &Path, game: &crate::Game) -> App {
        let mut world = game.new_world();
        let server = AssetServer::new(root.join("assets")).unwrap();
        server.watch(true).unwrap();
        world.insert_resource(server);
        game.app(world)
    }

    /// Updates until `done` holds. Hot reload normally goes through the file
    /// watcher, but macOS FSEvents can lose events while other processes
    /// write many files (a parallel cargo build, an rsync): after a second
    /// without the change, rescan the folder as a watcher-less project would.
    /// What these tests check is the settings reload, not event delivery.
    fn until(app: &mut App, mut done: impl FnMut(&World) -> bool) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if start.elapsed() > Duration::from_secs(1) {
                app.world.resource::<AssetServer>().rescan();
            }
            app.update();
            if done(&app.world) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn settings_load_with_defaults_and_hot_reload() {
        let root = project("reload");
        let game = crate::Game::new().with_plugin(TuningGame);
        assert!(game.registry().resource("Tuning").unwrap().is_settings());
        assert!(!game.registry().resource("Tuning").unwrap().is_saved());

        // No file: defaults, and the player does not write one.
        let mut app = app_for(&root, &game);
        assert_eq!(*app.world.resource::<Tuning>(), Tuning::default());
        assert!(!root.join("assets/settings/Tuning.json").exists());
        assert!(
            !box_core::scene::save_string(&app.world)
                .unwrap()
                .contains("Tuning"),
            "settings are not saved in scenes"
        );

        // The editor's mode writes the defaults.
        let mut edit = game.new_world();
        let created =
            load_settings_from(&mut edit, &root.join("assets"), None, Missing::CreateFile);
        // (Engine settings types too: the audio volumes, physics settings.)
        let mut expected = Vec::new();
        if cfg!(feature = "audio") {
            expected.push("settings/AudioSettings.json");
        }
        if cfg!(feature = "dynamics") {
            expected.push("settings/PhysicsSettings.json");
        }
        expected.push("settings/Tuning.json");
        assert_eq!(created, expected);
        let text = std::fs::read_to_string(root.join("assets/settings/Tuning.json")).unwrap();
        assert!(text.contains("\"speed\": 6.0"), "{text}");

        // A file is read before the plugins build; missing fields default.
        std::fs::write(
            root.join("assets/settings/Tuning.json"),
            r#"{ "speed": 9.0 }"#,
        )
        .unwrap();
        let mut app2 = app_for(&root, &game);
        assert_eq!(app2.world.resource::<Tuning>().speed, 9.0);
        assert_eq!(app2.world.resource::<Tuning>().jump, 7.5);
        assert_eq!(app2.world.resource::<BuiltWith>().0, 9.0);
        // Its first Loaded event changes nothing (runtime changes stay).
        app2.world.resource_mut::<Tuning>().jump = 1.0;
        for _ in 0..5 {
            app2.update();
        }
        assert_eq!(app2.world.resource::<Tuning>().jump, 1.0);

        // Hot reload.
        std::fs::write(
            root.join("assets/settings/Tuning.json"),
            r#"{ "speed": 12.0, "jump": 3.0 }"#,
        )
        .unwrap();
        assert!(
            until(&mut app2, |w| w.resource::<Tuning>().speed == 12.0),
            "reloaded"
        );
        assert_eq!(app2.world.resource::<Tuning>().jump, 3.0);
        // A broken file keeps the last good value.
        std::fs::write(
            root.join("assets/settings/Tuning.json"),
            r#"{ "speed": "fast" }"#,
        )
        .unwrap();
        for _ in 0..20 {
            app2.update();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(app2.world.resource::<Tuning>().speed, 12.0);
        // The first app (no file at its start) picks up the file once it
        // exists.
        std::fs::write(
            root.join("assets/settings/Tuning.json"),
            r#"{ "speed": 2.0 }"#,
        )
        .unwrap();
        assert!(until(&mut app, |w| w.resource::<Tuning>().speed == 2.0));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_action_file_overrides_code_defaults_per_action_and_hot_reloads() {
        let root = project("actions");
        let game = crate::Game::new().with_plugin(TuningGame);
        let file = root.join("assets").join(ACTIONS_FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            ActionMap::new()
                .button("jump", [Binding::Key(KeyCode::J)])
                .to_json(),
        )
        .unwrap();
        let mut app = app_for(&root, &game);
        let jump = |w: &World| w.resource::<ActionMap>().get("jump").cloned();
        assert_eq!(
            jump(&app.world),
            Some(ActionKind::Button {
                bindings: vec![Binding::Key(KeyCode::J)]
            }),
            "the file wins"
        );
        assert!(
            app.world.resource::<ActionMap>().get("fire").is_some(),
            "code defaults fill in"
        );
        // Pressing J jumps.
        app.world.resource_mut::<Input>().press_key(KeyCode::J);
        app.update();
        assert!(app.world.resource::<Actions>().held("jump"));
        app.world.resource_mut::<Input>().release_key(KeyCode::J);

        // Hot reload: K now, and `fire` still from the code.
        std::fs::write(
            &file,
            ActionMap::new()
                .button("jump", [Binding::Key(KeyCode::K)])
                .to_json(),
        )
        .unwrap();
        let k = Some(ActionKind::Button {
            bindings: vec![Binding::Key(KeyCode::K)],
        });
        assert!(until(&mut app, |w| jump(w) == k), "reloaded");
        assert!(app.world.resource::<ActionMap>().get("fire").is_some());
        // Deleting the file goes back to the code's bindings.
        std::fs::remove_file(&file).unwrap();
        let space = Some(ActionKind::Button {
            bindings: vec![Binding::Key(KeyCode::Space)],
        });
        assert!(
            until(&mut app, |w| jump(w) == space),
            "back to the defaults"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
