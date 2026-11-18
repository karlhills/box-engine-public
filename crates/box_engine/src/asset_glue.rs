//! Glue between saved asset references and the runtime (docs/ASSETS.md,
//! docs/SCENE_SYSTEM.md section 8).
//!
//! [`sync_assets`] runs the world's [`AssetServer`] (when it has one) and
//! keeps three things in step with scene data:
//!
//! - **Prefabs:** every [`PrefabInstance`] entity gets the entities of its
//!   scene asset instantiated beneath it, marked [`Instanced`] (runtime only:
//!   never saved, not editable). Uids are derived from the instance root's
//!   uid and the prefab-local uid ([`instanced_uid`]), so they are the same
//!   after a reload. The contents are rebuilt when `PrefabInstance.scene`
//!   changes and when the scene asset's version changes (hot reload). Nested
//!   prefabs work up to [`MAX_PREFAB_DEPTH`]; a prefab that contains itself
//!   is refused with an error.
//! - **Meshes:** every [`MeshAsset`] entity gets a [`MeshRenderer`] with the
//!   loaded mesh and one material per slot (`StandardMaterial` or
//!   `ShaderMaterial` assets); an empty material list uses the materials
//!   the mesh was imported with (from the model's `#Scene`). Nothing is drawn while the mesh or a material
//!   is loading; a reference that cannot be loaded draws the placeholder
//!   cube or the magenta placeholder material and logs one error. An entity
//!   that also has a `Primitive` is left to [`sync_scene_meshes`].
//! - **Skins and clips** ([`animation_glue`](crate::animation_glue)): every
//!   `SkinBinding` gets a runtime `SkinnedMesh`, and every clip or graph a
//!   player or state machine names is requested.
//! - **Material dependencies:** every texture referenced by a material in
//!   `Assets<StandardMaterial>` (inline `StandardMaterial` components, material
//!   files, glTF materials) and every `ShaderMaterial.shader` is requested
//!   from the server, so `AssetLookup` resolves it for the renderer.
//!
//! Everything is change-driven: per frame, one query over `PrefabInstance`
//! entities (comparing the reference and one asset version), one over
//! `MeshAsset` entities (comparing the component with a cached copy) and two
//! store generation compares. Rebuilds happen only for changed components,
//! for assets named by new [`AssetEvent`]s, and (while something is loading)
//! when an import finished.
//!
//! [`AssetGluePlugin`] (added by [`app_with_world`](crate::app_with_world))
//! runs it at the start of `Update`, after scene requests, so instanced
//! entities are propagated and drawn in the same frame;
//! [`sync_scene_meshes`] runs it too, so tools with their own loop (the
//! editor) get it from the call they already make. Without an
//! `AssetServer` (or [`AssetMirror`]) resource it resolves references only
//! through an existing `AssetLookup`, and does nothing in a world without one.
//!
//! A Play world gets the edit world's loaded assets with
//! [`copy_loaded_assets`] and follows the edit world's server with
//! [`AssetForwarder`]: nothing is re-imported or re-uploaded, and hot
//! reloads reach the Play world while it runs.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use box_assets::placeholder::{placeholder_material, placeholder_mesh};
use box_assets::{AssetEvent, AssetServer, LoadState, SceneAsset};
use box_core::animation::{AnimationClip, AnimationGraph, Skin};
use box_core::app::{App, Plugin, Stage};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::edit::{EditError, EditStack};
use box_core::event::{EventCursor, Events};
use box_core::identity::Uid;
use box_core::image::ImageData;
use box_core::mesh::MeshData;
use box_core::mesh_asset::MeshAsset;
use box_core::prefab::{Instanced, MAX_PREFAB_DEPTH, NotGenerated, PrefabInstance, instanced_uid};
use box_core::primitive::Primitive;
use box_core::reflect::{PartialReflect, ReflectMut, ReflectRef};
use box_core::scene;
use box_core::world::{Entity, Without, World};
use box_render::{MaterialHandle, MeshRenderer, ShaderMaterial, ShaderSource, StandardMaterial};
use serde_json::Value as Json;

#[cfg(doc)]
use crate::sync_scene_meshes;

/// Runs [`sync_assets`] in `Update` (after runtime scene requests) and adds
/// the [`AssetEvent`] queue. Added by [`app_with_world`](crate::app_with_world);
/// it does nothing until the world has an [`AssetServer`] resource (or an
/// `AssetLookup`).
pub struct AssetGluePlugin;

impl Plugin for AssetGluePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<AssetEvent>();
        app.add_system(Stage::Update, sync_assets);
    }
}

/// Resource in a Play world: the edit world's server, used only to request
/// loads. Its imports land in the edit world and reach the Play world
/// through [`AssetForwarder`]; [`sync_assets`] never runs it on this world.
/// (Defined in box_assets so crates below box_engine, such as box_audio,
/// load through it too.)
pub use box_assets::AssetMirror;
/// The server that loads for this world: its own, or the edit world's for a
/// Play world.
pub(crate) use box_assets::loader;

/// Resource: the glue's bookkeeping.
#[derive(Default)]
struct GlueState {
    cursor: EventCursor<AssetEvent>,
    /// `AssetServer::finished_imports` when pending entities were last checked.
    finished_imports: u64,
    lookup_generation: u64,
    placeholder_mesh: Option<Handle<MeshData>>,
    placeholder_material: Option<Handle<StandardMaterial>>,
    default_material: Option<Handle<StandardMaterial>>,
    /// Store generations of `Assets<StandardMaterial>` and
    /// `Assets<ShaderMaterial>` when their references were last scanned.
    material_generations: (u64, u64),
    /// Material asset id → version whose references were requested.
    material_versions: HashMap<u64, u64>,
    /// References already requested (textures, shaders).
    requested: HashSet<AssetRef>,
    /// Messages already logged, so a broken reference logs once.
    logged: HashSet<String>,
}

impl GlueState {
    fn error_once(&mut self, message: String) {
        if self.logged.insert(message.clone()) {
            tracing::error!("{message}");
        }
    }
}

/// What [`sync_mesh_assets`] last built for a `MeshAsset` entity. Runtime
/// only: not registered, so never saved, copied by duplicate or recorded.
#[derive(Debug)]
struct MeshAssetState {
    source: MeshAsset,
    /// Asset files the build used (mesh and materials).
    deps: Vec<AssetId>,
    /// Something was still loading: nothing is drawn yet.
    pending: bool,
    built: Option<MeshRenderer>,
}

/// What [`sync_prefabs`] last did for a `PrefabInstance` entity.
#[derive(Debug)]
struct PrefabState {
    source: AssetRef,
    handle: Option<Handle<SceneAsset>>,
    /// Version of the scene asset the contents were built from (`None`
    /// while it loads, or when it could not be instantiated).
    version: Option<u64>,
    /// Scene files from the outermost instance down to this one.
    chain: Vec<String>,
    /// Seed for instanced uids: the entity's uid, or a random one for a
    /// runtime entity without a uid.
    seed: Uid,
    /// `AssetLookup::generation` when a lookup-only resolve failed.
    lookup_generation: u64,
}

/// One frame of asset glue: run the server (if the world has one), then
/// prefabs, meshes and material dependencies (see the module docs). Cheap
/// when nothing changed.
pub fn sync_assets(world: &mut World) {
    profiling::scope!("sync_assets");
    let own = world.get_resource::<AssetServer>().cloned();
    if let Some(server) = &own {
        server.update(world);
    }
    let server = own.or_else(|| loader(world));
    if server.is_none() && !world.contains_resource::<AssetLookup>() {
        return;
    }
    let mut state = world.remove_resource::<GlueState>().unwrap_or_default();
    let changed: HashSet<AssetId> = match world.get_resource::<Events<AssetEvent>>() {
        Some(events) => state.cursor.read(events).map(|e| e.id).collect(),
        None => HashSet::new(),
    };
    let finished = server.as_ref().map_or(0, AssetServer::finished_imports);
    let lookup_generation = world
        .get_resource::<AssetLookup>()
        .map_or(0, AssetLookup::generation);
    let recheck = !changed.is_empty()
        || finished != state.finished_imports
        || lookup_generation != state.lookup_generation;
    state.finished_imports = finished;
    state.lookup_generation = lookup_generation;

    sync_prefabs(world, server.as_ref(), &mut state);
    sync_mesh_assets(world, server.as_ref(), &mut state, &changed, recheck);
    crate::animation_glue::sync_animation(world, server.as_ref());
    if let Some(server) = &server {
        request_material_dependencies(world, server, &mut state);
        request_particle_textures(world, server, &mut state);
    }
    world.insert_resource(state);
}

/// [`sync_assets`] until the world's server is idle and nothing the glue
/// requested is still loading, or `timeout` passes (logged), regenerating
/// every dirty procedural object ([`box_proc::settle`]) on each pass.
/// Headless runs and captures call it before the first frame so they never
/// draw a half-loaded scene. Returns whether everything settled.
pub fn settle_assets(world: &mut World, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if settle_step(world) {
            return true;
        }
        if start.elapsed() >= timeout {
            tracing::warn!(
                "assets still loading after {:.1} s; continuing without them",
                timeout.as_secs_f64()
            );
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// One non-blocking pass of [`settle_assets`]: applies finished loads,
/// requests what they need next and regenerates dirty procedural objects
/// in full, propagating world transforms before each pass so rules see
/// intents (children included) where they are. Returns whether the world's assets are settled: the server is
/// idle after a last pass that consumed its results (always true without
/// an [`AssetServer`]). The startup splash calls it once per frame while
/// the game waits ([`crate::splash`]).
pub fn settle_step(world: &mut World) -> bool {
    // `sync_scene_meshes` runs `sync_assets` and also turns inline material
    // components into material assets, whose textures the next pass requests.
    // Procedural objects are regenerated in full (no throttle or budget),
    // also after assets they read finished loading. World transforms first
    // (change-driven, cheap when nothing moved): rules read their intents'
    // world transforms, and the last pass may have spawned children.
    let pass = |world: &mut World| {
        box_core::transform::propagate_transforms(world);
        box_proc::settle(world);
        crate::sync_scene_meshes(world);
        crate::scatter_glue::settle(world);
    };
    let Some(server) = world.get_resource::<AssetServer>().cloned() else {
        pass(world);
        return true;
    };
    pass(world);
    // UI theme, its fonts, images and string tables.
    box_ui::request_ui_assets(world);
    if !server.is_idle() {
        return false;
    }
    // One more pass consumes the last results; it may request more (nested
    // prefabs, textures of new materials, a theme's fonts).
    pass(world);
    box_ui::request_ui_assets(world);
    server.is_idle()
}

// ------------------------------------------------------------- prefabs ----

/// The scene file a prefab reference names, as a string for cycle checks.
fn scene_key(server: Option<&AssetServer>, r: &AssetRef) -> String {
    let canonical = server.and_then(|s| s.canonical_ref(r));
    let r = canonical.as_ref().unwrap_or(r);
    match (&r.id, &r.path) {
        (Some(id), _) => id.to_string(),
        (None, Some(p)) => p.clone(),
        (None, None) => String::new(),
    }
}

fn sync_prefabs(world: &mut World, server: Option<&AssetServer>, state: &mut GlueState) {
    // Instances whose prefab state has no PrefabInstance any more: remove
    // their contents.
    let orphaned: Vec<Entity> = world
        .query::<Without<(Entity, &PrefabState), &PrefabInstance>>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in orphaned {
        despawn_contents(world, e);
        let _ = world.remove_one::<PrefabState>(e);
    }
    // Nested prefabs appear as new PrefabInstance entities while
    // instantiating; each pass handles one more level.
    for _ in 0..=MAX_PREFAB_DEPTH {
        let lookup_generation = state.lookup_generation;
        let todo: Vec<Entity> = {
            let scenes = world.get_resource::<Assets<SceneAsset>>();
            world
                .query::<(Entity, &PrefabInstance, Option<&PrefabState>)>()
                .iter()
                .filter(|(_, p, s)| match s {
                    None => true,
                    Some(s) => {
                        s.source != p.scene
                            || match s.handle {
                                Some(h) => scenes.and_then(|a| a.version(h)) != s.version,
                                None => s.lookup_generation != lookup_generation,
                            }
                    }
                })
                .map(|(e, _, _)| e)
                .collect()
        };
        if todo.is_empty() {
            return;
        }
        for e in todo {
            update_prefab(world, server, state, e);
        }
    }
}

/// Despawns the entities `instance` instantiated (its direct children
/// marked as its contents; their descendants go with them).
fn despawn_contents(world: &mut World, instance: Entity) {
    for child in world.children(instance) {
        if world
            .get::<Instanced>(child)
            .is_some_and(|i| i.instance == instance)
        {
            world.despawn(child);
        }
    }
}

fn update_prefab(
    world: &mut World,
    server: Option<&AssetServer>,
    state: &mut GlueState,
    e: Entity,
) {
    let Some(source) = world.get::<PrefabInstance>(e).map(|p| p.scene.clone()) else {
        return;
    };
    let old = world.remove_one::<PrefabState>(e).ok();
    let seed = world
        .uid(e)
        .or(old.as_ref().map(|s| s.seed))
        .unwrap_or_else(Uid::new_random);
    let mut next = PrefabState {
        source: source.clone(),
        handle: None,
        version: None,
        chain: Vec::new(),
        seed,
        lookup_generation: state.lookup_generation,
    };
    let source_changed = old.as_ref().is_none_or(|s| s.source != source);
    // Keep the handle when only the asset changed.
    let handle = match old.as_ref().and_then(|s| s.handle) {
        Some(h) if !source_changed => Some(h),
        _ if source.is_none() => None,
        _ => match server {
            Some(server) => match server.load::<SceneAsset>(&source) {
                Ok(h) => Some(h),
                Err(err) => {
                    state.error_once(format!("prefab {source}: {err}"));
                    None
                }
            },
            None => {
                let h = world
                    .get_resource::<AssetLookup>()
                    .and_then(|l| l.get::<SceneAsset>(&source));
                if h.is_none() {
                    state.error_once(format!(
                        "prefab {source}: not loaded (this world has no asset server)"
                    ));
                }
                h
            }
        },
    };
    next.handle = handle;
    let loaded = handle.and_then(|h| {
        let scenes = world.get_resource::<Assets<SceneAsset>>()?;
        Some((scenes.version(h)?, scenes.get(h)?.clone()))
    });
    let Some((version, scene)) = loaded else {
        // Loading (or unresolvable): contents of another scene go now; the
        // current contents stay until the reload lands.
        if source_changed {
            despawn_contents(world, e);
        }
        let _ = world.insert_one(e, next);
        return;
    };
    next.version = Some(version);

    let (depth, mut chain) = match world.get::<Instanced>(e).map(|i| *i) {
        Some(i) => (
            i.depth + 1,
            world
                .get::<PrefabState>(i.instance)
                .map(|s| s.chain.clone())
                .unwrap_or_default(),
        ),
        None => (1, Vec::new()),
    };
    let key = scene_key(server, &source);
    let refused = if depth > MAX_PREFAB_DEPTH {
        Some(format!(
            "prefab {source}: nested more than {MAX_PREFAB_DEPTH} levels deep; not instantiated"
        ))
    } else if chain.contains(&key) {
        Some(format!(
            "prefab {source}: contains itself (through {}); not instantiated",
            chain.join(" > ")
        ))
    } else {
        None
    };
    chain.push(key);
    next.chain = chain;
    despawn_contents(world, e);
    if let Some(message) = refused {
        state.error_once(message);
        let _ = world.insert_one(e, next);
        return;
    }
    if let Err(message) = instantiate_contents(world, e, seed, depth, &scene) {
        state.error_once(format!("prefab {source}: {message}"));
    }
    let _ = world.insert_one(e, next);
}

/// Spawns `scene`'s entities beneath `instance` with derived uids, marked
/// [`Instanced`]. Resources in the scene are ignored.
fn instantiate_contents(
    world: &mut World,
    instance: Entity,
    seed: Uid,
    depth: u32,
    scene: &SceneAsset,
) -> Result<(), String> {
    let mut records = scene.entities().to_vec();
    if records.is_empty() {
        return Ok(());
    }
    scene::remap_uids_with(&mut records, |local| instanced_uid(seed, local));
    let has_uid = world.uid(instance) == Some(seed);
    let mut roots = Vec::new();
    for (i, rec) in records.iter_mut().enumerate() {
        let Some(obj) = rec.as_object_mut() else {
            return Err(format!("entity record {i} is not an object"));
        };
        if obj.get("parent").is_none_or(Json::is_null) {
            roots.push(i);
            if has_uid {
                obj.insert("parent".into(), Json::String(seed.to_string()));
            }
        }
    }
    let spawned = scene::instantiate(world, &records, None).map_err(|e| e.to_string())?;
    let marker = Instanced { instance, depth };
    for &s in &spawned {
        let _ = world.insert_one(s, marker);
    }
    if !has_uid {
        for i in roots {
            let _ = world.set_parent(spawned[i], Some(instance));
        }
    }
    Ok(())
}

// -------------------------------------------------------------- meshes ----

pub(crate) enum Resolved<T> {
    Ready(T),
    Pending,
    Missing(String),
}

fn sync_mesh_assets(
    world: &mut World,
    server: Option<&AssetServer>,
    state: &mut GlueState,
    changed: &HashSet<AssetId>,
    recheck_pending: bool,
) {
    let mut todo = Vec::new();
    let mut handed_over = Vec::new();
    {
        let mut query = world.query::<(
            Entity,
            &MeshAsset,
            Option<&MeshAssetState>,
            Option<&MeshRenderer>,
            Option<&Primitive>,
        )>();
        for (e, mesh, s, renderer, primitive) in query.iter() {
            if primitive.is_some() {
                if s.is_some() {
                    handed_over.push(e);
                }
                continue;
            }
            let rebuild = match s {
                None => true,
                Some(s) => {
                    s.source != *mesh
                        || (s.pending && recheck_pending)
                        || s.deps.iter().any(|d| changed.contains(d))
                        || renderer != s.built.as_ref()
                }
            };
            if rebuild {
                todo.push((e, mesh.clone()));
            }
        }
    }
    for e in handed_over {
        let _ = world.remove_one::<MeshAssetState>(e);
    }
    // MeshAsset removed: remove the renderer the glue built.
    let removed: Vec<(Entity, Option<MeshRenderer>)> = world
        .query::<Without<(Entity, &MeshAssetState), &MeshAsset>>()
        .iter()
        .map(|(e, s)| (e, s.built.clone()))
        .collect();
    for (e, built) in removed {
        let _ = world.remove_one::<MeshAssetState>(e);
        if built.is_some() && world.get::<MeshRenderer>(e).map(|r| (*r).clone()) == built {
            let _ = world.remove_one::<MeshRenderer>(e);
        }
    }
    for (e, source) in todo {
        let (built, deps, pending) = build_renderer(world, server, state, &source);
        match &built {
            Some(r) => {
                let _ = world.insert_one(e, r.clone());
            }
            None => {
                let _ = world.remove_one::<MeshRenderer>(e);
            }
        }
        let _ = world.insert_one(
            e,
            MeshAssetState {
                source,
                deps,
                pending,
                built,
            },
        );
    }
}

fn file_id(server: Option<&AssetServer>, r: &AssetRef) -> Option<AssetId> {
    server
        .and_then(|s| s.canonical_ref(r))
        .and_then(|c| c.id)
        .or(r.id)
}

/// The renderer for `source`, the files it used, and whether something is
/// still loading (then nothing is drawn).
fn build_renderer(
    world: &mut World,
    server: Option<&AssetServer>,
    state: &mut GlueState,
    source: &MeshAsset,
) -> (Option<MeshRenderer>, Vec<AssetId>, bool) {
    let mut deps: Vec<AssetId> = Vec::new();
    if source.mesh.is_none() {
        return (None, deps, false);
    }
    deps.extend(file_id(server, &source.mesh));
    let mesh = resolve_mesh(world, server, &source.mesh);
    let mut pending = matches!(mesh, Resolved::Pending);
    let mesh_missing = matches!(mesh, Resolved::Missing(_));
    // No materials listed: the imported mesh's own, in slot order.
    let own;
    let refs: &[AssetRef] = if source.materials.is_empty() && !mesh_missing {
        match own_materials(world, &source.mesh) {
            Resolved::Ready(list) => {
                own = list;
                &own
            }
            Resolved::Pending => {
                pending = true;
                &[]
            }
            Resolved::Missing(_) => &[],
        }
    } else {
        &source.materials
    };
    let mut materials = Vec::with_capacity(refs.len());
    for r in refs {
        if r.is_none() {
            materials.push(MaterialHandle::Standard(default_material(world, state)));
            continue;
        }
        deps.extend(file_id(server, r));
        if mesh_missing {
            continue;
        }
        match resolve_material(world, server, r) {
            Resolved::Ready(h) => materials.push(h),
            Resolved::Pending => pending = true,
            Resolved::Missing(message) => {
                state.error_once(format!("material {r}: {message}; drawing the placeholder"));
                materials.push(MaterialHandle::Standard(placeholder_material_handle(
                    world, state,
                )));
            }
        }
    }
    deps.sort_unstable();
    deps.dedup();
    let mesh = match mesh {
        Resolved::Ready(h) => h,
        Resolved::Pending => return (None, deps, true),
        Resolved::Missing(message) => {
            state.error_once(format!(
                "mesh {}: {message}; drawing the placeholder",
                source.mesh
            ));
            let material = placeholder_material_handle(world, state);
            let renderer = MeshRenderer {
                mesh: placeholder_mesh_handle(world, state),
                materials: vec![MaterialHandle::Standard(material)],
                cast_shadows: source.cast_shadows,
            };
            return (Some(renderer), deps, false);
        }
    };
    if pending {
        return (None, deps, true);
    }
    let renderer = MeshRenderer {
        mesh,
        materials,
        cast_shadows: source.cast_shadows,
    };
    (Some(renderer), deps, false)
}

/// The materials an imported mesh (`model.glb#Mesh/<name>`) was imported
/// with, by slot: those of the first node of the model's `#Scene` that
/// draws it. `Missing` for other meshes and meshes no node draws (they use
/// the default material).
fn own_materials(world: &World, mesh: &AssetRef) -> Resolved<Vec<AssetRef>> {
    let Some(label) = mesh.label.as_deref().filter(|l| l.starts_with("Mesh/")) else {
        return Resolved::Missing("not an imported mesh".into());
    };
    let scene = AssetRef {
        label: Some("Scene".into()),
        ..mesh.clone()
    };
    match scene_meshes(world, &scene) {
        Resolved::Ready(list) => list
            .into_iter()
            .find(|(_, m)| m.mesh.label.as_deref() == Some(label))
            .map_or_else(
                || Resolved::Missing(format!("no node of the model draws {label}")),
                |(_, m)| Resolved::Ready(m.materials),
            ),
        Resolved::Pending => Resolved::Pending,
        Resolved::Missing(m) => Resolved::Missing(m),
    }
}

/// The renderer a `MeshAsset` entity with `source` would get (same
/// resolution, placeholders and logging), and whether something is still
/// loading (then `None`). For runtime-only renderers built from asset
/// references, such as scatter species.
pub(crate) fn mesh_asset_renderer(
    world: &mut World,
    source: &MeshAsset,
) -> (Option<MeshRenderer>, bool) {
    let server = loader(world);
    let mut state = world.remove_resource::<GlueState>().unwrap_or_default();
    let (renderer, _, pending) = build_renderer(world, server.as_ref(), &mut state, source);
    world.insert_resource(state);
    (renderer, pending)
}

/// The `MeshAsset`s of the scene asset `source` (a prefab or a model's
/// `#Scene`) with their transforms relative to the scene's roots, in file
/// order. Other components are ignored.
pub(crate) fn scene_meshes(
    world: &World,
    source: &AssetRef,
) -> Resolved<Vec<(glam::Affine3A, MeshAsset)>> {
    use box_core::reflect::FromReflect;
    use box_core::transform::Transform;
    let handle = match loader(world) {
        Some(server) => match server.load::<SceneAsset>(source) {
            Ok(h) => h,
            Err(e) => return Resolved::Missing(missing_text(&e)),
        },
        None => match world
            .get_resource::<AssetLookup>()
            .and_then(|l| l.get::<SceneAsset>(source))
        {
            Some(h) => h,
            None => return Resolved::Missing("not loaded (this world has no asset server)".into()),
        },
    };
    let Some(scene) = world
        .get_resource::<Assets<SceneAsset>>()
        .and_then(|a| a.get(handle))
    else {
        return Resolved::Pending;
    };
    let registry = world.registry();
    let typed = |json: Option<&Json>, ty: std::any::TypeId| {
        json.and_then(|j| registry.value_from_json(ty, j).ok())
    };
    let mut locals: HashMap<String, (Option<String>, glam::Affine3A)> = HashMap::new();
    let mut meshes: Vec<(String, MeshAsset)> = Vec::new();
    for (i, rec) in scene.entities().iter().enumerate() {
        let uid = rec
            .get("uid")
            .and_then(Json::as_str)
            .map_or_else(|| format!("#{i}"), str::to_owned);
        let parent = rec.get("parent").and_then(Json::as_str).map(str::to_owned);
        let components = rec.get("components");
        let local = typed(
            components.and_then(|c| c.get("Transform")),
            std::any::TypeId::of::<Transform>(),
        )
        .and_then(|v| Transform::from_reflect(v.as_ref()))
        .unwrap_or_default();
        locals.insert(uid.clone(), (parent, local.to_affine()));
        if let Some(mesh) = typed(
            components.and_then(|c| c.get("MeshAsset")),
            std::any::TypeId::of::<MeshAsset>(),
        )
        .and_then(|v| MeshAsset::from_reflect(v.as_ref()))
        {
            meshes.push((uid, mesh));
        }
    }
    let to_root = |uid: &str| {
        let mut m = glam::Affine3A::IDENTITY;
        let mut cur = Some(uid.to_owned());
        // Bounded: a malformed parent chain must not loop forever.
        for _ in 0..locals.len() + 1 {
            let Some((parent, local)) = cur.as_ref().and_then(|u| locals.get(u)) else {
                break;
            };
            m = *local * m;
            cur = parent.clone();
        }
        m
    };
    Resolved::Ready(
        meshes
            .into_iter()
            .map(|(uid, mesh)| (to_root(&uid), mesh))
            .collect(),
    )
}

/// Why a reference does not load, for "drawing the placeholder" messages
/// that already name the reference.
fn missing_text(e: &box_assets::AssetError) -> String {
    match e {
        box_assets::AssetError::NotFound(_) => {
            "no such file in assets/ (moved, renamed or deleted?); fix the reference in the inspector or put the file back".into()
        }
        other => other.to_string(),
    }
}

fn resolve_mesh(
    world: &World,
    server: Option<&AssetServer>,
    r: &AssetRef,
) -> Resolved<Handle<MeshData>> {
    let handle = match server {
        Some(server) => match server.load::<MeshData>(r) {
            // A failed mesh holds the placeholder cube; it is drawn with the
            // placeholder material, whatever the slots say.
            Ok(h) if server.is_placeholder(h) => {
                let message = match server.handle_state(h) {
                    Some(LoadState::Failed(m)) => m,
                    _ => "failed to load".into(),
                };
                return Resolved::Missing(message);
            }
            Ok(h) => h,
            Err(e) => return Resolved::Missing(missing_text(&e)),
        },
        None => match world
            .get_resource::<AssetLookup>()
            .and_then(|l| l.get::<MeshData>(r))
        {
            Some(h) => h,
            None => return Resolved::Missing("not loaded (this world has no asset server)".into()),
        },
    };
    let ready = world
        .get_resource::<Assets<MeshData>>()
        .is_some_and(|a| a.get(handle).is_some());
    if ready {
        Resolved::Ready(handle)
    } else {
        Resolved::Pending
    }
}

/// A material asset of either kind. The file decides the kind (a material
/// file with a `"shader"` key is a `ShaderMaterial`), so the whole file is
/// loaded and the result looked up by type.
pub(crate) fn resolve_material(
    world: &World,
    server: Option<&AssetServer>,
    r: &AssetRef,
) -> Resolved<MaterialHandle> {
    let canonical = server.and_then(|s| s.canonical_ref(r));
    let key = canonical.as_ref().unwrap_or(r);
    if let Some(lookup) = world.get_resource::<AssetLookup>() {
        if let Some(h) = lookup.get::<StandardMaterial>(key) {
            return Resolved::Ready(MaterialHandle::Standard(h));
        }
        if let Some(h) = lookup.get::<ShaderMaterial>(key) {
            return Resolved::Ready(MaterialHandle::Shader(h));
        }
    }
    let Some(server) = server else {
        return Resolved::Missing("not loaded (this world has no asset server)".into());
    };
    let id = match key.id {
        Some(id) => id,
        None => match server.load_file(r) {
            Ok(id) => id,
            Err(e) => return Resolved::Missing(missing_text(&e)),
        },
    };
    match server.file_state(id) {
        None => match server.load_file(key) {
            Ok(_) => Resolved::Pending,
            Err(e) => Resolved::Missing(missing_text(&e)),
        },
        Some(LoadState::Loading) => Resolved::Pending,
        Some(LoadState::Loaded) => Resolved::Missing("not a material".into()),
        Some(LoadState::Failed(message)) => Resolved::Missing(message),
    }
}

fn placeholder_mesh_handle(world: &mut World, state: &mut GlueState) -> Handle<MeshData> {
    let store = world.resource_or_default::<Assets<MeshData>>();
    match state.placeholder_mesh {
        Some(h) if store.get(h).is_some() => h,
        _ => *state.placeholder_mesh.insert(store.add(placeholder_mesh())),
    }
}

fn placeholder_material_handle(
    world: &mut World,
    state: &mut GlueState,
) -> Handle<StandardMaterial> {
    let store = world.resource_or_default::<Assets<StandardMaterial>>();
    match state.placeholder_material {
        Some(h) if store.get(h).is_some() => h,
        _ => *state
            .placeholder_material
            .insert(store.add(placeholder_material())),
    }
}

fn default_material(world: &mut World, state: &mut GlueState) -> Handle<StandardMaterial> {
    let store = world.resource_or_default::<Assets<StandardMaterial>>();
    match state.default_material {
        Some(h) if store.get(h).is_some() => h,
        _ => *state
            .default_material
            .insert(store.add(StandardMaterial::default())),
    }
}

// ------------------------------------------------- material dependencies ----

fn request_material_dependencies(world: &World, server: &AssetServer, state: &mut GlueState) {
    let standard = world.get_resource::<Assets<StandardMaterial>>();
    let shader = world.get_resource::<Assets<ShaderMaterial>>();
    let generations = (
        standard.map_or(0, Assets::generation),
        shader.map_or(0, Assets::generation),
    );
    if generations == state.material_generations {
        return;
    }
    state.material_generations = generations;
    let mut textures = Vec::new();
    let mut shaders = Vec::new();
    for (id, version, m) in standard.into_iter().flat_map(Assets::iter) {
        if state.material_versions.insert(id, version) != Some(version) {
            textures.extend(m.textures().into_iter().filter(|t| t.is_some()).cloned());
        }
    }
    for (id, version, m) in shader.into_iter().flat_map(Assets::iter) {
        if state.material_versions.insert(id, version) != Some(version) && m.shader.is_some() {
            shaders.push(m.shader.clone());
        }
    }
    let live = standard.map_or(0, Assets::len) + shader.map_or(0, Assets::len);
    if state.material_versions.len() > 2 * live + 64 {
        state.material_versions.retain(|id, _| {
            standard.is_some_and(|a| a.get_by_id(*id).is_some())
                || shader.is_some_and(|a| a.get_by_id(*id).is_some())
        });
    }
    for r in textures {
        if state.requested.insert(r.clone())
            && let Err(e) = server.load::<ImageData>(&r)
        {
            state.error_once(format!("texture {r}: {e}"));
        }
    }
    for r in shaders {
        if state.requested.insert(r.clone())
            && let Err(e) = server.load::<ShaderSource>(&r)
        {
            state.error_once(format!("material shader {r}: {e}"));
        }
    }
}

/// Loads the textures particle emitters name (once per reference).
fn request_particle_textures(world: &World, server: &AssetServer, state: &mut GlueState) {
    for e in world.query::<&box_render::ParticleEmitter>().iter() {
        let r = &e.texture;
        if r.is_some() && !state.requested.contains(r) {
            state.requested.insert(r.clone());
            if let Err(err) = server.load::<ImageData>(r) {
                state.error_once(format!("particle texture {r}: {err}"));
            }
        }
    }
}

// ---------------------------------------------------------- Play worlds ----

/// Gives `to` (a new Play world) every asset `from` (the edit world) loaded
/// through its server: the entries of `Assets<MeshData>`, `<ImageData>`,
/// `<StandardMaterial>`, `<ShaderMaterial>`, `<ShaderSource>`,
/// `<SceneAsset>`, `<JsonAsset>`, `<Skin>`, `<AnimationClip>`, `<AnimationGraph>`,
/// `<FontAsset>`, `<StringTable>`, `<DataTable>`, (audio feature) `<AudioClip>` and (script
/// feature) `<ScriptAsset>` that its `AssetLookup` maps (same handle ids and versions,
/// so the renderer's GPU copies are reused), a copy of the lookup, and an
/// [`AssetMirror`] of its server. Nothing is re-imported. Assets created by
/// systems (primitive meshes, inline materials) are not copied: the Play
/// world builds its own.
pub fn copy_loaded_assets(from: &World, to: &mut World) {
    profiling::scope!("copy_loaded_assets");
    let Some(lookup) = from.get_resource::<AssetLookup>() else {
        return;
    };
    fn copy<T: Clone + 'static>(lookup: &AssetLookup, from: &World, to: &mut World) {
        let Some(src) = from.get_resource::<Assets<T>>() else {
            return;
        };
        let dst = to.resource_or_default::<Assets<T>>();
        for id in lookup.handle_ids::<T>() {
            dst.copy_entry(src, id);
        }
    }
    copy::<MeshData>(lookup, from, to);
    copy::<ImageData>(lookup, from, to);
    copy::<StandardMaterial>(lookup, from, to);
    copy::<ShaderMaterial>(lookup, from, to);
    copy::<ShaderSource>(lookup, from, to);
    copy::<SceneAsset>(lookup, from, to);
    copy::<box_assets::JsonAsset>(lookup, from, to);
    copy::<Skin>(lookup, from, to);
    copy::<AnimationClip>(lookup, from, to);
    copy::<AnimationGraph>(lookup, from, to);
    copy::<box_assets::FontAsset>(lookup, from, to);
    copy::<box_assets::StringTable>(lookup, from, to);
    copy::<box_assets::DataTable>(lookup, from, to);
    #[cfg(feature = "audio")]
    copy::<box_audio::AudioClip>(lookup, from, to);
    #[cfg(feature = "script")]
    copy::<box_script::ScriptAsset>(lookup, from, to);
    to.insert_resource(lookup.clone());
    if let Some(server) = from.get_resource::<AssetServer>() {
        to.insert_resource(AssetMirror(server.clone()));
    }
}

/// Forwards the edit world's asset changes to a Play world: for every new
/// [`AssetEvent`] in the edit world (a hot reload, a finished load the Play
/// world requested through its [`AssetMirror`], a rename) the slot's value
/// and version are copied, the lookup is refreshed and the event is re-sent
/// in the Play world. Create it when Play starts; call
/// [`forward`](Self::forward) after each `AssetServer::update` of the edit
/// world.
pub struct AssetForwarder {
    cursor: EventCursor<AssetEvent>,
}

impl AssetForwarder {
    /// Starts after the events already in `edit` (the Play world's copy
    /// already reflects them).
    pub fn new(edit: &World) -> Self {
        let mut cursor = EventCursor::default();
        if let Some(events) = edit.get_resource::<Events<AssetEvent>>() {
            cursor.skip_all(events);
        }
        Self { cursor }
    }

    /// Copies what changed since the last call; returns the number of events
    /// forwarded.
    pub fn forward(&mut self, edit: &World, play: &mut World) -> usize {
        let events: Vec<AssetEvent> = match edit.get_resource::<Events<AssetEvent>>() {
            Some(e) => self.cursor.read(e).cloned().collect(),
            None => return 0,
        };
        if events.is_empty() {
            return 0;
        }
        fn copy<T: Clone + 'static>(id: u64, edit: &World, play: &mut World) {
            if let Some(src) = edit.get_resource::<Assets<T>>() {
                play.resource_or_default::<Assets<T>>().copy_entry(src, id);
            }
        }
        for e in &events {
            let t = e.type_id;
            use std::any::TypeId;
            if t == TypeId::of::<MeshData>() {
                copy::<MeshData>(e.handle, edit, play);
            } else if t == TypeId::of::<ImageData>() {
                copy::<ImageData>(e.handle, edit, play);
            } else if t == TypeId::of::<StandardMaterial>() {
                copy::<StandardMaterial>(e.handle, edit, play);
            } else if t == TypeId::of::<ShaderMaterial>() {
                copy::<ShaderMaterial>(e.handle, edit, play);
            } else if t == TypeId::of::<ShaderSource>() {
                copy::<ShaderSource>(e.handle, edit, play);
            } else if t == TypeId::of::<SceneAsset>() {
                copy::<SceneAsset>(e.handle, edit, play);
            } else if t == TypeId::of::<box_assets::JsonAsset>() {
                copy::<box_assets::JsonAsset>(e.handle, edit, play);
            } else if t == TypeId::of::<Skin>() {
                copy::<Skin>(e.handle, edit, play);
            } else if t == TypeId::of::<AnimationClip>() {
                copy::<AnimationClip>(e.handle, edit, play);
            } else if t == TypeId::of::<AnimationGraph>() {
                copy::<AnimationGraph>(e.handle, edit, play);
            } else if t == TypeId::of::<box_assets::FontAsset>() {
                copy::<box_assets::FontAsset>(e.handle, edit, play);
            } else if t == TypeId::of::<box_assets::StringTable>() {
                copy::<box_assets::StringTable>(e.handle, edit, play);
            } else if t == TypeId::of::<box_assets::DataTable>() {
                copy::<box_assets::DataTable>(e.handle, edit, play);
            }
            #[cfg(feature = "audio")]
            if t == TypeId::of::<box_audio::AudioClip>() {
                copy::<box_audio::AudioClip>(e.handle, edit, play);
            }
            #[cfg(feature = "script")]
            if t == TypeId::of::<box_script::ScriptAsset>() {
                copy::<box_script::ScriptAsset>(e.handle, edit, play);
            }
        }
        if let Some(lookup) = edit.get_resource::<AssetLookup>() {
            play.insert_resource(lookup.clone());
        }
        let n = events.len();
        play.resource_or_default::<Events<AssetEvent>>()
            .send_batch(events);
        n
    }
}

// ---------------------------------------------------------- save repair ----

/// Rewrites every asset reference in the world's saved components and
/// resources to its canonical form ([`AssetServer::canonical_ref`]: the id
/// filled in for path-only references, the path hint updated after a
/// rename). Runs through `edits` (one `set_field`/`set_resource_field` per
/// changed component or resource; wrap the call in a transaction for one
/// undo step) and returns how many it changed. Generated entities (prefab
/// contents, derived parts) are skipped; references that name no indexed file are left as they are.
///
/// Call it before writing a scene (the CLI does on `scene apply`; the
/// editor should on Save).
pub fn canonicalize_scene_refs(
    world: &mut World,
    edits: &mut EditStack,
    server: &AssetServer,
) -> Result<usize, EditError> {
    let registry = world.registry().clone();
    let mut changes = Vec::new();
    for (e, uid) in world
        .query::<NotGenerated<(Entity, &Uid)>>()
        .iter()
        .map(|(e, u)| (e, *u))
        .collect::<Vec<_>>()
    {
        for name in world.component_names(e) {
            if !registry.component(name).is_some_and(|c| c.is_saved()) || name == "Uid" {
                continue;
            }
            let path = world.field_path(name)?;
            let mut value = world.get_field(e, &path)?;
            if repair_refs(value.as_mut(), server) {
                changes.push((Some(uid), path, value));
            }
        }
    }
    for ty in registry.resources().filter(|r| r.is_saved()) {
        let Ok(path) = world.field_path(ty.name()) else {
            continue;
        };
        let Ok(mut value) = world.get_resource_field(&path) else {
            continue;
        };
        if repair_refs(value.as_mut(), server) {
            changes.push((None, path, value));
        }
    }
    let n = changes.len();
    for (uid, path, value) in changes {
        match uid {
            Some(uid) => edits.set_field(world, uid, &path, value.as_ref())?,
            None => edits.set_resource_field(world, &path, value.as_ref())?,
        }
    }
    Ok(n)
}

/// An asset reference held by a saved component or resource
/// ([`held_asset_refs`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HeldRef {
    /// The entity holding it; `None` for a resource.
    pub entity: Option<Uid>,
    /// The component or resource type holding it.
    pub owner: String,
    pub asset: AssetRef,
}

/// Every non-empty asset reference in the world's saved components (of
/// entities that are not generated: prefab contents, derived parts) and saved resources, in
/// no particular order. Read-only; pair it with
/// [`AssetServer::canonical_ref`] to find references that name no file.
pub fn held_asset_refs(world: &World) -> Vec<HeldRef> {
    let registry = world.registry().clone();
    let mut out = Vec::new();
    let mut found = Vec::new();
    for (e, uid) in world
        .query::<NotGenerated<(Entity, &Uid)>>()
        .iter()
        .map(|(e, u)| (e, *u))
        .collect::<Vec<_>>()
    {
        for name in world.component_names(e) {
            if !registry.component(name).is_some_and(|c| c.is_saved()) || name == "Uid" {
                continue;
            }
            let Ok(path) = world.field_path(name) else {
                continue;
            };
            let Ok(value) = world.get_field(e, &path) else {
                continue;
            };
            collect_refs(value.as_ref(), &mut found);
            out.extend(found.drain(..).map(|asset| HeldRef {
                entity: Some(uid),
                owner: name.to_string(),
                asset,
            }));
        }
    }
    for ty in registry.resources().filter(|r| r.is_saved()) {
        let Ok(path) = world.field_path(ty.name()) else {
            continue;
        };
        let Ok(value) = world.get_resource_field(&path) else {
            continue;
        };
        collect_refs(value.as_ref(), &mut found);
        out.extend(found.drain(..).map(|asset| HeldRef {
            entity: None,
            owner: ty.name().to_string(),
            asset,
        }));
    }
    out
}

/// Every non-empty `AssetRef` inside `value`.
fn collect_refs(value: &dyn PartialReflect, out: &mut Vec<AssetRef>) {
    if let Some(r) = value.try_downcast_ref::<AssetRef>() {
        if r.is_some() {
            out.push(r.clone());
        }
        return;
    }
    match value.reflect_ref() {
        ReflectRef::Struct(s) => (0..s.field_len())
            .filter_map(|i| s.field_at(i))
            .for_each(|f| collect_refs(f, out)),
        ReflectRef::TupleStruct(s) => (0..s.field_len())
            .filter_map(|i| s.field(i))
            .for_each(|f| collect_refs(f, out)),
        ReflectRef::Tuple(s) => (0..s.field_len())
            .filter_map(|i| s.field(i))
            .for_each(|f| collect_refs(f, out)),
        ReflectRef::List(s) => s.iter().for_each(|f| collect_refs(f, out)),
        ReflectRef::Array(s) => s.iter().for_each(|f| collect_refs(f, out)),
        ReflectRef::Enum(s) => (0..s.field_len())
            .filter_map(|i| s.field_at(i))
            .for_each(|f| collect_refs(f, out)),
        _ => {}
    }
}

/// Canonicalizes every `AssetRef` inside `value`; true if one changed.
fn repair_refs(value: &mut dyn PartialReflect, server: &AssetServer) -> bool {
    if let Some(r) = value.try_downcast_mut::<AssetRef>() {
        return match server.canonical_ref(r) {
            Some(c) if r.is_some() && c != *r => {
                *r = c;
                true
            }
            _ => false,
        };
    }
    let mut changed = false;
    match value.reflect_mut() {
        ReflectMut::Struct(s) => {
            for i in 0..s.field_len() {
                if let Some(f) = s.field_at_mut(i) {
                    changed |= repair_refs(f, server);
                }
            }
        }
        ReflectMut::TupleStruct(s) => {
            for i in 0..s.field_len() {
                if let Some(f) = s.field_mut(i) {
                    changed |= repair_refs(f, server);
                }
            }
        }
        ReflectMut::Tuple(s) => {
            for i in 0..s.field_len() {
                if let Some(f) = s.field_mut(i) {
                    changed |= repair_refs(f, server);
                }
            }
        }
        ReflectMut::List(s) => {
            for i in 0..s.len() {
                if let Some(f) = s.get_mut(i) {
                    changed |= repair_refs(f, server);
                }
            }
        }
        ReflectMut::Array(s) => {
            for i in 0..s.len() {
                if let Some(f) = s.get_mut(i) {
                    changed |= repair_refs(f, server);
                }
            }
        }
        ReflectMut::Enum(s) => {
            for i in 0..s.field_len() {
                if let Some(f) = s.field_at_mut(i) {
                    changed |= repair_refs(f, server);
                }
            }
        }
        _ => {}
    }
    changed
}
