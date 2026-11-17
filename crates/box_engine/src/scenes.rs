//! Runtime scenes: switch, load additively and unload scene files while a
//! game runs.
//!
//! Game systems queue requests on the [`Scenes`] resource:
//!
//! ```
//! use box_engine::prelude::*;
//!
//! /// Set by gameplay when the goal is reached.
//! #[derive(Default)]
//! struct LevelComplete(bool);
//!
//! fn next_level(world: &mut World) {
//!     if std::mem::take(&mut world.resource_mut::<LevelComplete>().0) {
//!         world.resource_mut::<Scenes>().switch("level2");
//!     }
//! }
//! # let mut app = box_engine::default_app();
//! # app.world.insert_resource(LevelComplete::default());
//! # app.add_system(Stage::Update, next_level);
//! # app.update();
//! ```
//!
//! **When requests apply.** [`apply_scene_requests`] runs as the first
//! `Update` system of every engine app ([`crate::app_with_world`] installs
//! [`ScenePlugin`] before anything else). Requests queued during a frame's
//! `Update` or `PostUpdate` are applied together at the start of the next
//! frame's `Update`, after that frame's `FixedUpdate` steps. So every system
//! sees either the old or the new scene, never a half-loaded world, and the
//! new entities get transforms and meshes in the same frame's `PostUpdate`
//! and are drawn that frame.
//!
//! **What a request does.**
//! - [`Scenes::switch`]: the file is read and fully validated first; on
//!   failure nothing changes. Then every scene-loaded entity (all instances,
//!   additive ones included) is despawned with its descendants, every
//!   resource saved in the new file replaces the current one (resources the
//!   file does not mention keep their values), and the file's entities are
//!   spawned as the new main instance.
//! - [`Scenes::load_additive`]: spawns the file's entities as another
//!   instance next to the current ones. The file's resources are ignored.
//! - [`Scenes::unload`]: despawns one instance's entities with their
//!   descendants.
//!
//! **Which entities belong to a scene.** Loaded entities carry a
//! [`SceneInstance`] marker (runtime only: not registered, never saved).
//! Entities spawned at runtime with plain `world.spawn` have no marker;
//! despawning a scene entity also despawns everything parented under it.
//! Insert `SceneInstance(id)` on a runtime entity to tie it to an instance.
//! Entities already in the world when the [`ScenePlugin`] is built (a scene
//! the player or the editor loaded before building the app) are adopted as
//! the main instance.
//!
//! **Runtime entities on a switch** follow [`Scenes::runtime_entities`]:
//! - [`RuntimeEntities::KeepAll`] (the current default, for compatibility):
//!   every runtime entity survives, so projectiles, effects and spawned
//!   enemies of the old level leak into the new one unless the game cleans
//!   them up.
//! - [`RuntimeEntities::DespawnUnlessPersistent`] (recommended; how Unity
//!   and Godot behave): every runtime root entity without a [`Persistent`]
//!   marker is despawned with its descendants after the old scene is
//!   unloaded. Mark the root of anything that should carry over (the player
//!   rig, a music player) with [`Persistent`]. Additive loads and unloads
//!   never touch runtime entities.
//!
//! The default will change to `DespawnUnlessPersistent` in a later release;
//! new games should opt in now with
//! `scenes.set_runtime_entities(RuntimeEntities::DespawnUnlessPersistent)`.
//!
//! **Uids.** A file's uids are kept unless one of them is already used by a
//! live entity (loading the same scene twice additively); then the whole
//! instance gets fresh uids, and uid references inside its components are
//! rewritten to match.
//!
//! **Paths** resolve against the project ([`Scenes::resolve`]): absolute
//! paths as given, otherwise `<project>/assets/<path>`, then
//! `<project>/<path>`, with or without the `.scene.json` extension, then a
//! scene name (`"level2"` finds `assets/**/level2.scene.json`). Without a
//! project root, relative paths resolve against the working directory.
//!
//! **Results** are reported as [`SceneEvent`]s (`Events<SceneEvent>`) and
//! through `tracing`. Errors never panic.

use std::fmt;
use std::path::{Path, PathBuf};

use box_core::event::Events;
use box_core::identity::Uid;
use box_core::prelude::*;
use box_core::scene::{self, UnknownResourceFields, UnknownResources};
use serde_json::Value as Json;

use crate::project::{Project, SCENE_EXTENSION};

/// Identifies one loaded scene instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SceneInstanceId(pub u64);

impl fmt::Display for SceneInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "scene#{}", self.0)
    }
}

/// Component: the scene instance an entity was loaded from. Runtime only
/// (not registered, never saved).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SceneInstance(pub SceneInstanceId);

/// Component: keep this runtime entity (and its descendants) when the scene
/// switches under [`RuntimeEntities::DespawnUnlessPersistent`]. Runtime
/// only. Has no effect on scene-loaded entities, which always belong to
/// their instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Persistent;

/// What a scene switch does with runtime entities (spawned by game code, not
/// loaded from a scene). See the [module docs](self).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RuntimeEntities {
    /// Every runtime entity survives a switch. The default for now, for
    /// compatibility; prefer [`DespawnUnlessPersistent`](Self::DespawnUnlessPersistent).
    #[default]
    KeepAll,
    /// Runtime root entities without [`Persistent`] are despawned (with
    /// their descendants) by a switch.
    DespawnUnlessPersistent,
}

/// What happened to a scene request.
#[derive(Clone, Debug, PartialEq)]
pub enum SceneEvent {
    /// A scene instance was spawned (or adopted when the app was built).
    Loaded {
        id: SceneInstanceId,
        /// The resolved file, when known.
        path: Option<PathBuf>,
        additive: bool,
        /// Entities spawned.
        entities: usize,
    },
    /// An instance was despawned, by [`Scenes::unload`] or a switch.
    Unloaded {
        id: SceneInstanceId,
        path: Option<PathBuf>,
    },
    /// A request failed; the world is unchanged by it.
    Failed {
        id: SceneInstanceId,
        /// The scene as requested.
        scene: String,
        message: String,
    },
}

/// A loaded instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneInstanceInfo {
    pub id: SceneInstanceId,
    pub path: Option<PathBuf>,
    pub additive: bool,
}

#[derive(Debug)]
enum Request {
    Switch(SceneInstanceId, PathBuf),
    Additive(SceneInstanceId, PathBuf),
    Unload(SceneInstanceId),
}

/// Resource: scene requests and the loaded instances. See the [module
/// docs](self).
#[derive(Debug, Default)]
pub struct Scenes {
    project_root: Option<PathBuf>,
    start_scene: Option<PathBuf>,
    pending: Vec<Request>,
    instances: Vec<SceneInstanceInfo>,
    main: Option<SceneInstanceId>,
    next_id: u64,
    runtime_entities: RuntimeEntities,
}

impl Scenes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve relative scene paths and names against this project.
    pub fn with_project_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.project_root = Some(root.into());
        self
    }

    /// The file the entities already in the world came from, recorded for
    /// the main instance when [`ScenePlugin`] adopts them.
    pub fn with_start_scene(mut self, path: impl Into<PathBuf>) -> Self {
        self.start_scene = Some(path.into());
        self
    }

    /// What a switch does with runtime entities (builder form of
    /// [`set_runtime_entities`](Self::set_runtime_entities)).
    pub fn with_runtime_entities(mut self, policy: RuntimeEntities) -> Self {
        self.runtime_entities = policy;
        self
    }

    /// What a switch does with runtime entities: keep them all (the default
    /// for now) or despawn those without [`Persistent`] (recommended).
    pub fn set_runtime_entities(&mut self, policy: RuntimeEntities) {
        self.runtime_entities = policy;
    }

    pub fn runtime_entities(&self) -> RuntimeEntities {
        self.runtime_entities
    }

    pub fn set_project_root(&mut self, root: Option<PathBuf>) {
        self.project_root = root;
    }

    pub fn project_root(&self) -> Option<&Path> {
        self.project_root.as_deref()
    }

    /// The file given to [`with_start_scene`](Self::with_start_scene).
    pub fn start_scene(&self) -> Option<&Path> {
        self.start_scene.as_deref()
    }

    /// Replace every scene-loaded entity with `scene` (a path or a name).
    /// Returns the id the new main instance will have.
    pub fn switch(&mut self, scene: impl AsRef<Path>) -> SceneInstanceId {
        let id = self.reserve();
        self.pending
            .push(Request::Switch(id, scene.as_ref().to_path_buf()));
        id
    }

    /// Spawn `scene` next to the loaded scenes. Returns the id it will have.
    pub fn load_additive(&mut self, scene: impl AsRef<Path>) -> SceneInstanceId {
        let id = self.reserve();
        self.pending
            .push(Request::Additive(id, scene.as_ref().to_path_buf()));
        id
    }

    /// Despawn an instance's entities (also one requested earlier in the
    /// same frame).
    pub fn unload(&mut self, id: SceneInstanceId) {
        self.pending.push(Request::Unload(id));
    }

    /// Requests not applied yet.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// The instance loaded by the last switch (or adopted at startup).
    pub fn main(&self) -> Option<SceneInstanceId> {
        self.main
    }

    /// Loaded instances, in load order.
    pub fn instances(&self) -> &[SceneInstanceInfo] {
        &self.instances
    }

    pub fn is_loaded(&self, id: SceneInstanceId) -> bool {
        self.instances.iter().any(|i| i.id == id)
    }

    /// The file an instance was loaded from.
    pub fn path(&self, id: SceneInstanceId) -> Option<&Path> {
        self.instances
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.path.as_deref())
    }

    /// The scene file `scene` refers to (see the [module docs](self)).
    pub fn resolve(&self, scene: impl AsRef<Path>) -> Result<PathBuf, String> {
        let scene = scene.as_ref();
        let mut candidates = Vec::new();
        let mut push = |p: PathBuf| {
            if !p.to_string_lossy().ends_with(SCENE_EXTENSION) {
                let mut with_ext = p.clone().into_os_string();
                with_ext.push(SCENE_EXTENSION);
                candidates.push(p);
                candidates.push(PathBuf::from(with_ext));
            } else {
                candidates.push(p);
            }
        };
        match &self.project_root {
            _ if scene.is_absolute() => push(scene.to_path_buf()),
            Some(root) => {
                push(root.join("assets").join(scene));
                push(root.join(scene));
            }
            None => push(scene.to_path_buf()),
        }
        if let Some(found) = candidates.iter().find(|p| box_assets::vfs::is_file(p)) {
            return Ok(found.clone());
        }
        if let Some(root) = self.project_root.as_ref().filter(|_| scene.is_relative())
            && let Some(found) = find_by_name(root, scene)?
        {
            return Ok(found);
        }
        Err(format!(
            "scene `{}` not found{}",
            scene.display(),
            match &self.project_root {
                Some(root) => format!(" in project {}", root.display()),
                None => String::new(),
            }
        ))
    }

    /// Replaces the loaded instance list (a save game was restored): pending
    /// requests are dropped and each `(path, additive)` gets a fresh id; the
    /// first non-additive one becomes the main instance. Entities are not
    /// touched: the caller tags them with [`SceneInstance`]. Returns the ids
    /// in order.
    pub fn restore_instances(&mut self, list: &[(Option<PathBuf>, bool)]) -> Vec<SceneInstanceId> {
        self.pending.clear();
        self.instances.clear();
        self.main = None;
        let mut ids = Vec::with_capacity(list.len());
        for (path, additive) in list {
            let id = self.reserve();
            self.instances.push(SceneInstanceInfo {
                id,
                path: path.clone(),
                additive: *additive,
            });
            if !additive && self.main.is_none() {
                self.main = Some(id);
            }
            ids.push(id);
        }
        ids
    }

    fn reserve(&mut self) -> SceneInstanceId {
        self.next_id += 1;
        SceneInstanceId(self.next_id)
    }
}

/// A scene file under `<root>/assets` whose name (`level2`) or path below
/// `assets/` without the extension (`scenes/level2`) is `name`.
fn find_by_name(root: &Path, name: &Path) -> Result<Option<PathBuf>, String> {
    let wanted = name.to_string_lossy().replace('\\', "/");
    let wanted = wanted.trim_end_matches(SCENE_EXTENSION);
    let Ok(project) = Project::open(root) else {
        return Ok(None);
    };
    let files = project.scene_files().map_err(|e| e.to_string())?;
    let assets = project.assets_dir();
    let matches: Vec<PathBuf> = files
        .into_iter()
        .filter(|f| {
            let rel = f
                .strip_prefix(&assets)
                .unwrap_or(f)
                .to_string_lossy()
                .replace('\\', "/");
            let rel = rel.trim_end_matches(SCENE_EXTENSION);
            rel == wanted || rel.rsplit('/').next() == Some(wanted)
        })
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.into_iter().next()),
        n => Err(format!(
            "scene name `{wanted}` is ambiguous ({n} files: {}); use a path",
            matches
                .iter()
                .map(|m| project.relative(m))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Installs runtime scenes: the [`Scenes`] resource (kept if present),
/// `Events<SceneEvent>` and [`apply_scene_requests`] in `Update`. Entities
/// already in the world are adopted as the main instance. Add it before
/// other `Update` systems so requests apply before gameplay;
/// [`crate::app_with_world`] does.
pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SceneEvent>();
        app.world.resource_or_default::<Scenes>();
        adopt_loaded_entities(&mut app.world);
        app.add_system(Stage::Update, apply_scene_requests);
    }
}

/// Mark saved entities without a [`SceneInstance`] as a new main instance.
fn adopt_loaded_entities(world: &mut World) {
    let unmarked: Vec<Entity> = world
        .query::<(Entity, &Uid)>()
        .without::<&SceneInstance>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    if unmarked.is_empty() {
        return;
    }
    let scenes = world.resource_mut::<Scenes>();
    let id = scenes.reserve();
    let path = scenes.start_scene.take();
    scenes.instances.push(SceneInstanceInfo {
        id,
        path: path.clone(),
        additive: false,
    });
    scenes.main = Some(id);
    for &e in &unmarked {
        let _ = world.insert_one(e, SceneInstance(id));
    }
    send(
        world,
        SceneEvent::Loaded {
            id,
            path,
            additive: false,
            entities: unmarked.len(),
        },
    );
}

/// Apply every queued [`Scenes`] request, in request order. Installed by
/// [`ScenePlugin`]; a tool with its own loop may call it directly.
pub fn apply_scene_requests(world: &mut World) {
    let Some(scenes) = world.get_resource_mut::<Scenes>() else {
        return;
    };
    if scenes.pending.is_empty() {
        return;
    }
    profiling::scope!("apply_scene_requests");
    let requests = std::mem::take(&mut scenes.pending);
    for request in requests {
        match request {
            Request::Switch(id, scene) => load(world, id, &scene, false),
            Request::Additive(id, scene) => load(world, id, &scene, true),
            Request::Unload(id) => {
                if !unload(world, id) {
                    fail(
                        world,
                        id,
                        &id.to_string(),
                        "no such scene instance is loaded",
                    );
                }
            }
        }
    }
}

fn send(world: &mut World, event: SceneEvent) {
    if let Some(events) = world.get_resource_mut::<Events<SceneEvent>>() {
        events.send(event);
    }
}

fn fail(world: &mut World, id: SceneInstanceId, scene: &str, message: &str) {
    tracing::error!("scene `{scene}`: {message}");
    send(
        world,
        SceneEvent::Failed {
            id,
            scene: scene.to_string(),
            message: message.to_string(),
        },
    );
}

/// Despawn an instance's entities. False if it is not loaded.
fn unload(world: &mut World, id: SceneInstanceId) -> bool {
    let scenes = world.resource_mut::<Scenes>();
    let Some(index) = scenes.instances.iter().position(|i| i.id == id) else {
        return false;
    };
    let info = scenes.instances.remove(index);
    if scenes.main == Some(id) {
        scenes.main = None;
    }
    let members: Vec<Entity> = world
        .query::<(Entity, &SceneInstance)>()
        .iter()
        .filter(|(_, m)| m.0 == id)
        .map(|(e, _)| e)
        .collect();
    for e in members {
        // Already gone when an ancestor was despawned first.
        world.despawn(e);
    }
    tracing::info!("unloaded {id} ({})", path_text(info.path.as_deref()));
    send(
        world,
        SceneEvent::Unloaded {
            id,
            path: info.path,
        },
    );
    true
}

/// Despawn runtime root entities (no [`SceneInstance`], no parent) that are
/// not [`Persistent`], with their descendants.
fn despawn_runtime_entities(world: &mut World) {
    let doomed: Vec<Entity> = world
        .query::<Entity>()
        .without::<&SceneInstance>()
        .without::<&Persistent>()
        .without::<&Parent>()
        .iter()
        .collect();
    if doomed.is_empty() {
        return;
    }
    for &e in &doomed {
        world.despawn(e);
    }
    tracing::info!(
        "scene switch despawned {} runtime entities (not Persistent)",
        doomed.len()
    );
}

fn path_text(path: Option<&Path>) -> String {
    path.map_or_else(|| "unsaved".to_string(), |p| p.display().to_string())
}

/// Log one warning per component or resource type that `world` (a scene
/// just loaded from `scene`) uses but its registry does not know, with a
/// did-you-mean and how to run the game's own player or editor
/// ([`scene::unknown_types`]). A type of an engine module this build leaves
/// out (`RigidBody` without the `dynamics` feature) names that feature
/// instead ([`crate::modules`]). Returns the unknown types. The player and
/// runtime scene requests call it; editors and tools loading scene files
/// should too.
pub fn warn_unknown_types(world: &World, scene: &str) -> Vec<scene::UnknownType> {
    let unknown = scene::unknown_types(world);
    for u in &unknown {
        match crate::modules::missing_module(&u.name) {
            Some(module) => tracing::warn!(
                "{}",
                crate::modules::missing_module_message(u, module, scene)
            ),
            None => tracing::warn!("{}", u.message(scene)),
        }
    }
    unknown
}

/// A scene file read and validated, not yet in the world.
struct Prepared {
    path: PathBuf,
    /// Holds the file's resources.
    world: World,
    records: Vec<Json>,
}

fn prepare(world: &World, scene: &Path) -> Result<Prepared, String> {
    let path = world.resource::<Scenes>().resolve(scene)?;
    let bytes = box_assets::vfs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let tree: Json =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    // A full load into a scratch world validates every record and resource
    // before the live world is touched.
    let loaded = scene::load_value(&tree, world.registry().clone())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let shown = world
        .resource::<Scenes>()
        .project_root()
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(&path);
    warn_unknown_types(&loaded, &format!("`{}`", shown.display()));
    let records = tree
        .get("entities")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(Prepared {
        path,
        world: loaded,
        records,
    })
}

fn load(world: &mut World, id: SceneInstanceId, scene: &Path, additive: bool) {
    let requested = scene.display().to_string();
    let mut prepared = match prepare(world, scene) {
        Ok(p) => p,
        Err(message) => return fail(world, id, &requested, &message),
    };
    if !additive {
        let loaded: Vec<SceneInstanceId> = world
            .resource::<Scenes>()
            .instances
            .iter()
            .map(|i| i.id)
            .collect();
        for old in loaded {
            unload(world, old);
        }
        if world.resource::<Scenes>().runtime_entities == RuntimeEntities::DespawnUnlessPersistent {
            despawn_runtime_entities(world);
        }
        replace_resources(world, &mut prepared.world);
    }
    let collides = prepared.records.iter().any(|r| {
        r.get("uid")
            .and_then(Json::as_str)
            .and_then(Uid::parse)
            .is_some_and(|uid| world.entity_by_uid(uid).is_some())
    });
    if collides {
        scene::remap_uids(&mut prepared.records);
    }
    let spawned = match scene::instantiate(world, &prepared.records, None) {
        Ok(s) => s,
        Err(e) => {
            let message = format!("{}: {e}", prepared.path.display());
            return fail(world, id, &requested, &message);
        }
    };
    for &e in &spawned {
        let _ = world.insert_one(e, SceneInstance(id));
    }
    let scenes = world.resource_mut::<Scenes>();
    scenes.instances.push(SceneInstanceInfo {
        id,
        path: Some(prepared.path.clone()),
        additive,
    });
    if !additive {
        scenes.main = Some(id);
    }
    tracing::info!(
        "loaded {id} from {} ({} entities{}{})",
        prepared.path.display(),
        spawned.len(),
        if additive { ", additive" } else { "" },
        if collides { ", fresh uids" } else { "" },
    );
    send(
        world,
        SceneEvent::Loaded {
            id,
            path: Some(prepared.path),
            additive,
            entities: spawned.len(),
        },
    );
}

/// Every resource saved in the new scene replaces the current one.
fn replace_resources(world: &mut World, from: &mut World) {
    for name in from.resource_names() {
        let result = from
            .get_resource_reflect(name)
            .and_then(|value| world.insert_resource_reflect(name, value.as_partial_reflect()));
        if let Err(e) = result {
            tracing::error!("scene resource `{name}` could not be applied: {e}");
        }
    }
    match from.remove_resource::<UnknownResources>() {
        Some(unknown) => world.insert_resource(unknown),
        None => {
            world.remove_resource::<UnknownResources>();
        }
    }
    match from.remove_resource::<UnknownResourceFields>() {
        Some(extra) => world.insert_resource(extra),
        None => {
            world.remove_resource::<UnknownResourceFields>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Game;
    use box_core::event::EventCursor;
    use box_render::Environment;

    struct Fixture {
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A project with `main` (the starter scene), `level2` (two entities, one
    /// a child, an evening environment) and a broken scene.
    fn fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("box_scenes_{name}_{}", Uid::new()));
        let project = Project::create(&root, "Scenes").unwrap();
        let mut w = World::with_registry(crate::shared_engine_registry());
        w.insert_resource(Environment::evening());
        let a = w
            .spawn_saved(Uid::new(), "A", (Transform::default(),))
            .unwrap();
        let b = w
            .spawn_saved(Uid::new(), "B", (Transform::from_xyz(1.0, 0.0, 0.0),))
            .unwrap();
        w.set_parent(b, Some(a)).unwrap();
        let dir = project.assets_dir().join("scenes/levels");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("level2.scene.json"), scene::save(&w).unwrap()).unwrap();
        std::fs::write(
            project.assets_dir().join("scenes/broken.scene.json"),
            "{ \"version\": 1, \"entities\": [ { \"uid\": 5 } ] }",
        )
        .unwrap();
        Fixture { root }
    }

    /// An app like the player builds: the start scene loaded first, then adopted.
    fn app(f: &Fixture) -> App {
        let project = Project::open(&f.root).unwrap();
        let game = Game::new();
        let bytes = std::fs::read(project.start_scene_path()).unwrap();
        let mut world = scene::load(&bytes, game.registry()).unwrap();
        world.insert_resource(
            Scenes::new()
                .with_project_root(project.root())
                .with_start_scene(project.start_scene_path()),
        );
        game.app(world)
    }

    fn saved_count(world: &World) -> usize {
        world.query::<&Uid>().iter().count()
    }

    fn named(world: &World, name: &str) -> Option<Entity> {
        world
            .query::<(Entity, &Name)>()
            .iter()
            .find(|(_, n)| n.as_str() == name)
            .map(|(e, _)| e)
    }

    fn events(app: &App, cursor: &mut EventCursor<SceneEvent>) -> Vec<SceneEvent> {
        cursor
            .read(app.world.resource::<Events<SceneEvent>>())
            .cloned()
            .collect()
    }

    #[test]
    fn startup_entities_are_adopted_as_the_main_instance() {
        let f = fixture("adopt");
        let mut app = app(&f);
        let mut cursor = EventCursor::default();
        app.update();
        let scenes = app.world.resource::<Scenes>();
        let main = scenes.main().expect("main instance");
        assert_eq!(scenes.instances().len(), 1);
        assert!(scenes.path(main).unwrap().ends_with("main.scene.json"));
        assert_eq!(
            events(&app, &mut cursor),
            vec![SceneEvent::Loaded {
                id: main,
                path: Some(Project::open(&f.root).unwrap().start_scene_path()),
                additive: false,
                entities: 4,
            }]
        );
        let marked = app.world.query::<&SceneInstance>().iter().count();
        assert_eq!(marked, 4);
    }

    #[test]
    fn switch_replaces_scene_entities_and_resources_next_update() {
        let f = fixture("switch");
        let mut app = app(&f);
        let mut cursor = EventCursor::default();
        app.update();
        events(&app, &mut cursor);
        let old_main = app.world.resource::<Scenes>().main().unwrap();
        let cube = named(&app.world, "Cube").unwrap();
        // A runtime entity parented under a scene entity goes with it; a free
        // runtime entity survives.
        let attached = app.world.spawn((Transform::default(),));
        app.world.set_parent(attached, Some(cube)).unwrap();
        let free = app.world.spawn((Transform::default(),));
        assert_eq!(
            app.world.resource::<Environment>(),
            &crate::project::starter_environment()
        );

        let id = app.world.resource_mut::<Scenes>().switch("level2");
        // Nothing happens until the scene system runs.
        assert!(app.world.resource::<Scenes>().has_pending());
        assert_eq!(saved_count(&app.world), 4);
        app.update();

        assert_eq!(saved_count(&app.world), 2);
        assert!(!app.world.contains(cube) && !app.world.contains(attached));
        assert!(app.world.contains(free));
        assert_eq!(app.world.resource::<Environment>(), &Environment::evening());
        let a = named(&app.world, "A").unwrap();
        let b = named(&app.world, "B").unwrap();
        assert_eq!(app.world.parent(b), Some(a));
        assert_eq!(app.world.roots(), &[a]);
        assert_eq!(app.world.get::<SceneInstance>(b).unwrap().0, id);
        // Propagation ran in the same frame.
        let g = app.world.get::<GlobalTransform>(b).unwrap();
        assert_eq!(g.translation(), Vec3::new(1.0, 0.0, 0.0));
        let scenes = app.world.resource::<Scenes>();
        assert_eq!(scenes.main(), Some(id));
        assert_eq!(scenes.instances().len(), 1);
        let ev = events(&app, &mut cursor);
        assert!(matches!(&ev[0], SceneEvent::Unloaded { id: u, .. } if *u == old_main));
        assert!(matches!(
            &ev[1],
            SceneEvent::Loaded { id: l, additive: false, entities: 2, path: Some(p) }
                if *l == id && p.ends_with("levels/level2.scene.json")
        ));
    }

    #[derive(Default)]
    struct Seen {
        added: Vec<String>,
        removed: usize,
    }

    #[test]
    fn on_add_and_on_remove_see_scene_loads_and_switches() {
        let f = fixture("hooks");
        let mut app = app(&f);
        app.world.insert_resource(Seen::default());
        app.on_add::<Name>(|w, e| {
            let name = w.get::<Name>(e).unwrap().as_str().to_string();
            w.resource_mut::<Seen>().added.push(name);
        });
        app.on_remove::<Name>(|w, _| w.resource_mut::<Seen>().removed += 1);
        app.update();
        // The start scene's four entities, once each.
        assert_eq!(app.world.resource::<Seen>().added.len(), 4);
        app.update();
        assert_eq!(app.world.resource::<Seen>().added.len(), 4);
        app.world.resource_mut::<Scenes>().switch("level2");
        app.update();
        let seen = app.world.resource::<Seen>();
        assert_eq!(seen.added[4..], ["A", "B"]);
        assert_eq!(seen.removed, 4);
    }

    #[test]
    fn despawn_unless_persistent_clears_runtime_entities_on_a_switch_only() {
        let f = fixture("persist");
        let mut app = app(&f);
        app.update();
        assert_eq!(
            app.world.resource::<Scenes>().runtime_entities(),
            RuntimeEntities::KeepAll,
            "the default keeps compatibility"
        );
        app.world
            .resource_mut::<Scenes>()
            .set_runtime_entities(RuntimeEntities::DespawnUnlessPersistent);
        let projectile = app.world.spawn((Transform::default(),));
        let spark = app.world.spawn((Transform::default(),));
        app.world.set_parent(spark, Some(projectile)).unwrap();
        let rig = app.world.spawn((Transform::default(), Persistent));
        let held = app.world.spawn((Transform::default(),));
        app.world.set_parent(held, Some(rig)).unwrap();

        // Additive loads and unloads leave runtime entities alone.
        let extra = app.world.resource_mut::<Scenes>().load_additive("level2");
        app.update();
        app.world.resource_mut::<Scenes>().unload(extra);
        app.update();
        assert!(app.world.contains(projectile));

        app.world.resource_mut::<Scenes>().switch("level2");
        app.update();
        assert!(!app.world.contains(projectile) && !app.world.contains(spark));
        assert!(app.world.contains(rig) && app.world.contains(held));
        assert_eq!(saved_count(&app.world), 2, "the new scene is loaded");
    }

    #[test]
    fn additive_loads_and_unloads_keep_other_instances() {
        let f = fixture("additive");
        let mut app = app(&f);
        let mut cursor = EventCursor::default();
        app.update();
        events(&app, &mut cursor);
        let main = app.world.resource::<Scenes>().main().unwrap();
        let (first, second) = {
            let scenes = app.world.resource_mut::<Scenes>();
            (
                scenes.load_additive("scenes/levels/level2.scene.json"),
                scenes.load_additive("scenes/levels/level2"),
            )
        };
        app.update();
        // The same file twice: the second copy got fresh uids.
        assert_eq!(saved_count(&app.world), 4 + 2 + 2);
        assert_eq!(
            app.world.resource::<Environment>(),
            &crate::project::starter_environment()
        );
        let scenes = app.world.resource::<Scenes>();
        assert_eq!(scenes.main(), Some(main));
        assert_eq!(scenes.instances().len(), 3);
        let ev = events(&app, &mut cursor);
        assert_eq!(ev.len(), 2);
        assert!(
            ev.iter()
                .all(|e| matches!(e, SceneEvent::Loaded { additive: true, .. }))
        );

        app.world.resource_mut::<Scenes>().unload(first);
        app.update();
        assert_eq!(saved_count(&app.world), 4 + 2);
        let remaining: Vec<SceneInstanceId> = app
            .world
            .query::<&SceneInstance>()
            .iter()
            .map(|m| m.0)
            .collect();
        assert!(remaining.iter().all(|&i| i == main || i == second));
        // The copy's hierarchy is intact.
        let b = named(&app.world, "B").unwrap();
        assert_eq!(
            app.world
                .get::<SceneInstance>(app.world.parent(b).unwrap())
                .unwrap()
                .0,
            second
        );

        // A switch removes additive instances too.
        app.world.resource_mut::<Scenes>().switch("main");
        app.update();
        assert_eq!(saved_count(&app.world), 4);
        assert_eq!(app.world.resource::<Scenes>().instances().len(), 1);
    }

    #[test]
    fn failures_are_events_and_leave_the_world_alone() {
        let f = fixture("fail");
        let mut app = app(&f);
        let mut cursor = EventCursor::default();
        app.update();
        events(&app, &mut cursor);
        let main = app.world.resource::<Scenes>().main();
        let (missing, broken) = {
            let scenes = app.world.resource_mut::<Scenes>();
            let missing = scenes.switch("no_such_scene");
            let broken = scenes.switch("scenes/broken");
            scenes.unload(SceneInstanceId(999));
            (missing, broken)
        };
        app.update();
        assert_eq!(saved_count(&app.world), 4);
        assert_eq!(app.world.resource::<Scenes>().main(), main);
        let ev = events(&app, &mut cursor);
        assert_eq!(ev.len(), 3, "{ev:?}");
        match &ev[0] {
            SceneEvent::Failed { id, scene, message } => {
                assert_eq!((*id, scene.as_str()), (missing, "no_such_scene"));
                assert!(message.contains("not found"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        match &ev[1] {
            SceneEvent::Failed { id, message, .. } => {
                assert_eq!(*id, broken);
                assert!(message.contains("broken.scene.json"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            ev[2],
            SceneEvent::Failed {
                id: SceneInstanceId(999),
                ..
            }
        ));
    }

    #[test]
    fn resolve_paths_and_names() {
        let f = fixture("resolve");
        let scenes = Scenes::new().with_project_root(&f.root);
        let level2 = f.root.join("assets/scenes/levels/level2.scene.json");
        for s in [
            "level2",
            "scenes/levels/level2",
            "scenes/levels/level2.scene.json",
            "assets/scenes/levels/level2.scene.json",
        ] {
            assert_eq!(scenes.resolve(s).unwrap(), level2, "{s}");
        }
        assert_eq!(scenes.resolve(&level2).unwrap(), level2);
        assert!(scenes.resolve("nope").unwrap_err().contains("not found"));
        // Two files with the same name are ambiguous.
        let other = f.root.join("assets/level2.scene.json");
        std::fs::copy(&level2, &other).unwrap();
        assert!(scenes.resolve("level2").is_ok_and(|p| p == other));
        std::fs::rename(&other, f.root.join("assets/scenes/x.scene.json")).unwrap();
        std::fs::create_dir_all(f.root.join("assets/more")).unwrap();
        std::fs::copy(&level2, f.root.join("assets/more/level2.scene.json")).unwrap();
        let err = scenes.resolve("level2").unwrap_err();
        assert!(err.contains("ambiguous"), "{err}");
    }

    #[test]
    fn an_empty_app_has_no_main_until_a_switch() {
        let f = fixture("empty");
        let mut app = crate::default_app();
        app.world
            .resource_mut::<Scenes>()
            .set_project_root(Some(f.root.clone()));
        app.update();
        assert_eq!(app.world.resource::<Scenes>().main(), None);
        app.world.resource_mut::<Scenes>().switch("level2");
        app.update();
        assert_eq!(saved_count(&app.world), 2);
        assert!(app.world.resource::<Scenes>().main().is_some());
    }
}
