//! [`AssetServer`]: loading, background import, hot reload.

use std::any::{Any, TypeId};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use box_core::animation::{AnimationClip, AnimationGraph, Skin};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::event::Events;
use box_core::image::ImageData;
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::shader_material::{ShaderMaterial, ShaderSource};
use box_core::world::World;

use crate::error::{AssetError, ImportError};
use crate::importer::{ImportContext, ImportedAsset, Importer, ImporterSet};
use crate::importers::{
    AnimationGraphImporter, DataTable, DataTableImporter, FontAsset, FontImporter, GltfImporter,
    JsonAsset, JsonImporter, MaterialImporter, SceneImporter, ShaderImporter, StringTable,
    StringTableImporter, TextureImporter,
};
use crate::index::{self, Change, Index, normalize_rel};
use crate::meta::{self, META_SUFFIX};
use crate::placeholder::{
    placeholder_image, placeholder_material, placeholder_mesh, placeholder_shader,
};
use crate::scene_asset::SceneAsset;
use crate::watch::Watcher;

/// The load state of one asset or sub-asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadState {
    /// Requested; the import has not finished.
    Loading,
    /// Its value is in the `Assets<T>` store.
    Loaded,
    /// The import failed (or the file or sub-asset is gone). The slot holds
    /// the type's placeholder, or the last good value when a re-import
    /// failed; [`AssetServer::is_placeholder`] tells which.
    Failed(String),
}

/// What happened to an asset slot. Sent as [`Events<AssetEvent>`] from
/// [`AssetServer::update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetEventKind {
    /// First value stored in the slot.
    Loaded,
    /// The slot's value was replaced by a re-import (hot reload); handles
    /// stay valid.
    Reloaded,
    /// An import failed or a sub-asset disappeared from its file
    /// ([`AssetEvent::message`] says why).
    Failed,
    /// The source file was deleted; the slot now holds the placeholder.
    Removed,
    /// The source file was renamed or moved ([`AssetEvent::old_path`]).
    Moved,
}

/// An event about one asset slot.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetEvent {
    pub kind: AssetEventKind,
    pub id: AssetId,
    /// Sub-asset label; `None` for the file's main asset.
    pub label: Option<String>,
    /// Current source path relative to `assets/`.
    pub path: String,
    /// `std::any::type_name` of the asset type.
    pub type_name: &'static str,
    pub type_id: TypeId,
    /// [`Handle::id`] of the slot.
    pub handle: u64,
    /// The error, for `Failed` and `Removed`.
    pub message: Option<String>,
    /// The previous path, for `Moved`.
    pub old_path: Option<String>,
}

impl AssetEvent {
    /// A reference to the asset (id, label, current path).
    pub fn asset_ref(&self) -> AssetRef {
        AssetRef {
            id: Some(self.id),
            label: self.label.clone(),
            path: Some(self.path.clone()),
        }
    }

    /// True when the event is about `handle`.
    pub fn is<T: 'static>(&self, handle: Handle<T>) -> bool {
        self.type_id == TypeId::of::<T>() && self.handle == handle.id()
    }
}

/// One sub-asset of a file, as listed by [`AssetServer::sub_assets`].
#[derive(Clone, Debug, PartialEq)]
pub struct SubAsset {
    pub label: Option<String>,
    pub type_name: &'static str,
    pub state: LoadState,
}

// ------------------------------------------------------------ internals ----

type SlotKey = (AssetId, Option<String>);

/// Type-erased access to one asset type's store in a world.
trait ErasedOps: Send + Sync {
    fn new_handle(&self) -> (u64, Box<dyn Any + Send + Sync>);
    fn set(&self, world: &mut World, handle: &dyn Any, value: Box<dyn Any + Send>);
    fn set_placeholder(&self, world: &mut World, handle: &dyn Any) -> bool;
    fn register(
        &self,
        world: &mut World,
        handle: &dyn Any,
        id: AssetId,
        label: Option<&str>,
        path: &str,
    );
}

struct TypedOps<T> {
    placeholder: Option<fn() -> T>,
}

impl<T: Send + 'static> ErasedOps for TypedOps<T> {
    fn new_handle(&self) -> (u64, Box<dyn Any + Send + Sync>) {
        // Handle ids are unique across every store in the process, so a
        // handle reserved here is valid for the world's `Assets<T>`.
        let h = Assets::<T>::new().reserve();
        (h.id(), Box::new(h))
    }

    fn set(&self, world: &mut World, handle: &dyn Any, value: Box<dyn Any + Send>) {
        if let (Some(h), Ok(v)) = (handle.downcast_ref::<Handle<T>>(), value.downcast::<T>()) {
            world.resource_or_default::<Assets<T>>().set(*h, *v);
        }
    }

    fn set_placeholder(&self, world: &mut World, handle: &dyn Any) -> bool {
        match (self.placeholder, handle.downcast_ref::<Handle<T>>()) {
            (Some(make), Some(h)) => {
                world.resource_or_default::<Assets<T>>().set(*h, make());
                true
            }
            _ => false,
        }
    }

    fn register(
        &self,
        world: &mut World,
        handle: &dyn Any,
        id: AssetId,
        label: Option<&str>,
        path: &str,
    ) {
        if let Some(h) = handle.downcast_ref::<Handle<T>>() {
            world
                .resource_or_default::<AssetLookup>()
                .insert(id, label, Some(path), *h);
        }
    }
}

struct TypeEntry {
    name: &'static str,
    ops: Box<dyn ErasedOps>,
}

struct Slot {
    type_id: TypeId,
    raw: u64,
    handle: Box<dyn Any + Send + Sync>,
    state: LoadState,
    /// The store holds the type's placeholder.
    placeholder: bool,
    /// The store has held an imported value.
    loaded_once: bool,
}

/// Size and modification time of a file and its `.meta`, to skip re-imports
/// when an event changed nothing (a rename, a spurious notification).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint([Stamp; 2]);

/// Size and modification time of one file (zero and `None` when missing).
type Stamp = (u64, Option<SystemTime>);

fn stamp(path: &Path) -> Stamp {
    if crate::vfs::is_mounted(path) {
        // A pak never changes while mounted.
        return (crate::vfs::file_len(path).unwrap_or(0), None);
    }
    std::fs::metadata(path).map_or((0, None), |m| (m.len(), m.modified().ok()))
}

fn fingerprint(abs: &Path) -> Fingerprint {
    Fingerprint([stamp(abs), stamp(&meta::meta_path(abs))])
}

struct FileRecord {
    path: String,
    /// Labels of every slot of this file.
    labels: BTreeSet<Option<String>>,
    /// Labels the latest successful import produced.
    produced: BTreeSet<Option<String>>,
    /// Generation of the latest queued import; older results are dropped.
    generation: u64,
    importing: bool,
    /// The latest import succeeded.
    imported: bool,
    failed: Option<String>,
    /// Files the latest import read (paths relative to `assets/`), with
    /// their stamps after the import.
    dependencies: Vec<(String, Stamp)>,
    fingerprint: Option<Fingerprint>,
}

impl FileRecord {
    fn new(path: String) -> Self {
        Self {
            path,
            labels: BTreeSet::new(),
            produced: BTreeSet::new(),
            generation: 0,
            importing: false,
            imported: false,
            failed: None,
            dependencies: Vec::new(),
            fingerprint: None,
        }
    }

    fn requested(&self) -> bool {
        self.importing || self.imported || self.failed.is_some()
    }
}

enum Pending {
    /// Store the placeholder in a slot and send an event (late load of a
    /// missing sub-asset, a load of a failed file, a deleted file).
    Placeholder {
        key: SlotKey,
        kind: AssetEventKind,
        message: String,
    },
    /// The file of `id` moved to `path`.
    Moved {
        id: AssetId,
        from: String,
        path: String,
    },
}

struct Job {
    id: AssetId,
    path: String,
    generation: u64,
    importers: Arc<ImporterSet>,
}

struct JobResult {
    id: AssetId,
    generation: u64,
    outcome: Result<Vec<ImportedAsset>, ImportError>,
    dependencies: Vec<(String, Stamp)>,
    elapsed: Duration,
}

struct Shared {
    root: PathBuf,
    /// The root is inside a mounted pak ([`crate::vfs`]).
    read_only: bool,
    index: RwLock<Index>,
}

struct State {
    importers: Arc<ImporterSet>,
    types: HashMap<TypeId, TypeEntry>,
    slots: HashMap<SlotKey, Slot>,
    by_raw: HashMap<u64, SlotKey>,
    files: HashMap<AssetId, FileRecord>,
    pending: Vec<Pending>,
    in_flight: usize,
    next_generation: u64,
    jobs: Sender<Job>,
    /// Import durations of finished jobs (path, time), for measurements.
    timings: Vec<(String, Duration)>,
    /// Finished imports applied by `update` so far (current or superseded).
    finished: u64,
}

struct Inner {
    shared: Arc<Shared>,
    state: Mutex<State>,
    results: Mutex<Receiver<JobResult>>,
    watcher: Mutex<Option<Watcher>>,
    workers: usize,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// --------------------------------------------------------------- server ----

/// Loads assets from a project's `assets/` folder.
///
/// - **Identity:** at creation the folder is scanned; every file with an
///   importer gets a `.meta` sidecar with a persistent [`AssetId`] (created
///   when missing). Copied files (duplicate ids) get fresh ids.
/// - **Loading:** [`load`](Self::load) returns a [`Handle`] at once and
///   imports in the background on a small thread pool. The handle's value
///   appears in the world's `Assets<T>` when [`update`](Self::update)
///   applies the finished import; until then `Assets::get` returns `None`.
///   Loading the same reference twice returns the same handle. Loading any
///   sub-asset imports the whole file once and fills all its sub-assets.
/// - **Failure:** a failed import logs an error, sets
///   [`LoadState::Failed`] and stores the type's placeholder (a magenta and
///   black checker image, a 1 m cube mesh, a magenta material, an empty
///   scene). Bad files never panic.
/// - **Hot reload:** with [`watch`](Self::watch) on, changed files and
///   `.meta` settings are re-imported into the same slots (handles stay
///   valid); deleted files leave their slots with the placeholder.
///
/// The server is a cheap handle (`Clone` shares it) and is meant to be a
/// world resource, so systems can load:
///
/// ```no_run
/// # use box_assets::AssetServer;
/// # use box_core::prelude::*;
/// # fn system(world: &mut World) -> Result<(), box_assets::AssetError> {
/// let server = world.resource::<AssetServer>().clone();
/// let roof: Handle<MeshData> = server.load(&AssetRef::path("models/tower.glb#Mesh/Roof"))?;
/// # Ok(()) }
/// ```
///
/// [`update`](Self::update) must run once per frame on the world that owns
/// the `Assets<T>` stores ([`update_assets`](crate::update_assets) as a
/// system, or [`AssetPlugin`](crate::AssetPlugin)). One server serves one
/// world.
#[derive(Clone)]
pub struct AssetServer {
    inner: Arc<Inner>,
}

impl AssetServer {
    /// A server for `assets_dir` with the built-in importers (`texture`,
    /// `gltf`, `material`, `scene`, `shader`, `json`, `animgraph`) and asset
    /// types (`ImageData`, `MeshData`, `StandardMaterial`, `ShaderMaterial`,
    /// `ShaderSource`, `SceneAsset`, `JsonAsset`, `Skin`, `AnimationClip`,
    /// `AnimationGraph`). Scans the folder,
    /// creating missing `.meta` files. Watching is off.
    ///
    /// `assets_dir` may be inside a mounted pak ([`crate::vfs`], an exported
    /// game): then the server is read-only ([`is_read_only`](Self::is_read_only)),
    /// never writes `.meta` files and cannot watch.
    pub fn new(assets_dir: impl AsRef<Path>) -> Result<Self, AssetError> {
        let dir = assets_dir.as_ref();
        let io = |source| AssetError::Io {
            path: dir.to_path_buf(),
            source,
        };
        let read_only = crate::vfs::is_mounted(dir);
        let root = if read_only {
            if !crate::vfs::is_dir(dir) {
                return Err(io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no such folder in the pak",
                )));
            }
            std::path::absolute(dir).map_err(io)?
        } else {
            // Canonical, so watcher paths (/private/var on macOS) strip cleanly.
            std::fs::canonicalize(dir).map_err(io)?
        };
        if !crate::vfs::is_dir(&root) {
            return Err(io(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "not a directory",
            )));
        }
        let mut importers = ImporterSet::default();
        importers.add(Arc::new(TextureImporter));
        importers.add(Arc::new(GltfImporter));
        importers.add(Arc::new(MaterialImporter));
        importers.add(Arc::new(SceneImporter));
        importers.add(Arc::new(ShaderImporter));
        importers.add(Arc::new(JsonImporter));
        importers.add(Arc::new(AnimationGraphImporter));
        importers.add(Arc::new(FontImporter));
        importers.add(Arc::new(StringTableImporter));
        importers.add(Arc::new(DataTableImporter));
        let mut index = Index::new(root.clone());
        index.scan(&importers);
        let shared = Arc::new(Shared {
            root,
            read_only,
            index: RwLock::new(index),
        });

        let (job_tx, job_rx) = channel::<Job>();
        let (result_tx, result_rx) = channel::<JobResult>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let workers = std::thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1))
            .clamp(1, 4);
        for i in 0..workers {
            let (shared, jobs, results) = (shared.clone(), job_rx.clone(), result_tx.clone());
            std::thread::Builder::new()
                .name(format!("box-assets-{i}"))
                .spawn(move || worker(&shared, &jobs, &results))
                .map_err(io)?;
        }

        let server = Self {
            inner: Arc::new(Inner {
                shared,
                state: Mutex::new(State {
                    importers: Arc::new(importers),
                    types: HashMap::new(),
                    slots: HashMap::new(),
                    by_raw: HashMap::new(),
                    files: HashMap::new(),
                    pending: Vec::new(),
                    in_flight: 0,
                    next_generation: 0,
                    jobs: job_tx,
                    timings: Vec::new(),
                    finished: 0,
                }),
                results: Mutex::new(result_rx),
                watcher: Mutex::new(None),
                workers,
            }),
        };
        server.register_asset_type::<ImageData>(Some(placeholder_image));
        server.register_asset_type::<MeshData>(Some(placeholder_mesh));
        server.register_asset_type::<StandardMaterial>(Some(placeholder_material));
        server.register_asset_type::<SceneAsset>(Some(SceneAsset::empty));
        // No placeholder: a material that failed is drawn with the magenta
        // placeholder material by the asset glue.
        server.register_asset_type::<ShaderMaterial>(None);
        // An empty shader that the renderer draws with its magenta error
        // material, like a shader that never compiled.
        server.register_asset_type::<ShaderSource>(Some(placeholder_shader));
        // No placeholder: a data file that failed keeps its last good value.
        server.register_asset_type::<JsonAsset>(None);
        // No placeholders: a skinned mesh without its skin is drawn in bind
        // pose, a player without its clip holds the last pose.
        server.register_asset_type::<Skin>(None);
        server.register_asset_type::<AnimationClip>(None);
        server.register_asset_type::<AnimationGraph>(None);
        // No placeholders: the UI draws with its default font meanwhile; a
        // table that failed keeps its last good value.
        server.register_asset_type::<FontAsset>(None);
        server.register_asset_type::<StringTable>(None);
        server.register_asset_type::<DataTable>(None);
        Ok(server)
    }

    /// The canonical path of the assets folder (for a pak, its path inside
    /// the mounted pak; read it with [`crate::vfs`]).
    pub fn assets_dir(&self) -> &Path {
        &self.inner.shared.root
    }

    /// True when the assets come from a mounted pak: nothing can be written
    /// and hot reload is unavailable.
    pub fn is_read_only(&self) -> bool {
        self.inner.shared.read_only
    }

    /// The contents of a file below the assets folder (`path` relative to
    /// it), from the disk or the pak. For files the server does not import
    /// (Luau modules, raw data).
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, AssetError> {
        let rel = normalize_rel(path).ok_or_else(|| AssetError::OutsideAssets(path.into()))?;
        let abs = self.inner.shared.root.join(&rel);
        crate::vfs::read(&abs).map_err(|source| AssetError::Io { path: abs, source })
    }

    /// True when `path` (relative to the assets folder) is a file, on disk
    /// or in the pak.
    pub fn file_exists(&self, path: &str) -> bool {
        normalize_rel(path).is_some_and(|rel| crate::vfs::is_file(self.inner.shared.root.join(rel)))
    }

    fn check_writable(&self, path: &str) -> Result<(), AssetError> {
        if self.inner.shared.read_only {
            return Err(AssetError::ReadOnly(path.to_string()));
        }
        Ok(())
    }

    /// Number of import worker threads.
    pub fn worker_count(&self) -> usize {
        self.inner.workers
    }

    /// Registers an asset type so it can be loaded and imported. `placeholder`
    /// makes the value stored when an import fails; without one a failed
    /// asset has no value (or keeps its last good one). Registering again
    /// replaces the placeholder.
    pub fn register_asset_type<T: Send + 'static>(&self, placeholder: Option<fn() -> T>) {
        lock(&self.inner.state).types.insert(
            TypeId::of::<T>(),
            TypeEntry {
                name: std::any::type_name::<T>(),
                ops: Box::new(TypedOps { placeholder }),
            },
        );
    }

    /// Registers an importer (replacing one with the same name) and indexes
    /// the files it handles, creating their missing `.meta` files.
    pub fn register_importer(&self, importer: impl Importer) {
        let mut st = lock(&self.inner.state);
        let mut set = (*st.importers).clone();
        set.add(Arc::new(importer));
        st.importers = Arc::new(set);
        let changes = self.index_mut().scan(&st.importers);
        self.apply_changes(&mut st, changes);
    }

    fn index(&self) -> std::sync::RwLockReadGuard<'_, Index> {
        self.inner
            .shared
            .index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn index_mut(&self) -> std::sync::RwLockWriteGuard<'_, Index> {
        self.inner
            .shared
            .index
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    // ------------------------------------------------------------ loading

    /// Requests an asset and returns its handle at once (see the type docs).
    ///
    /// The reference is resolved by id when it has one, else by path (an
    /// unknown path is looked up on disk, creating its `.meta`). An id that
    /// is not in the index falls back to the path, with a warning. Errors
    /// are for references that cannot name a file; import problems show up
    /// later as [`LoadState::Failed`].
    pub fn load<T: Send + 'static>(&self, r: &AssetRef) -> Result<Handle<T>, AssetError> {
        let mut st = lock(&self.inner.state);
        let type_id = TypeId::of::<T>();
        if !st.types.contains_key(&type_id) {
            return Err(AssetError::UnregisteredType(std::any::type_name::<T>()));
        }
        let (id, path) = self.resolve(&mut st, r)?;
        let key = (id, r.label.clone());
        if let Some(slot) = st.slots.get(&key) {
            return typed_handle(&st, slot, r);
        }
        let handle = Assets::<T>::new().reserve();
        st.slots.insert(
            key.clone(),
            Slot {
                type_id,
                raw: handle.id(),
                handle: Box::new(handle),
                state: LoadState::Loading,
                placeholder: false,
                loaded_once: false,
            },
        );
        st.by_raw.insert(handle.id(), key.clone());
        let file = st
            .files
            .entry(id)
            .or_insert_with(|| FileRecord::new(path.clone()));
        file.labels.insert(r.label.clone());
        let late_failure = if file.importing {
            None
        } else if file.imported {
            Some(missing_label_message(
                &path,
                r.label.as_deref(),
                &file.produced,
            ))
        } else {
            file.failed.clone()
        };
        match late_failure {
            Some(message) => {
                if let Some(slot) = st.slots.get_mut(&key) {
                    slot.state = LoadState::Failed(message.clone());
                }
                st.pending.push(Pending::Placeholder {
                    key,
                    kind: AssetEventKind::Failed,
                    message,
                });
            }
            None => {
                if !st.files.get(&id).is_some_and(|f| f.importing) {
                    self.queue_import(&mut st, id);
                }
            }
        }
        Ok(handle)
    }

    /// Imports a whole file (by id or path; a label is ignored) without
    /// requesting a typed slot: every sub-asset gets a slot when the import
    /// finishes. Returns the file's id. Does nothing if it is already
    /// imported or importing.
    pub fn load_file(&self, r: &AssetRef) -> Result<AssetId, AssetError> {
        let mut st = lock(&self.inner.state);
        let (id, path) = self.resolve(&mut st, r)?;
        let file = st.files.entry(id).or_insert_with(|| FileRecord::new(path));
        if !file.importing && !file.imported {
            self.queue_import(&mut st, id);
        }
        Ok(id)
    }

    /// Re-imports a file now (as if it had changed on disk).
    pub fn reimport(&self, id: AssetId) -> Result<(), AssetError> {
        let mut st = lock(&self.inner.state);
        let path = self
            .index()
            .path_of(id)
            .map(str::to_string)
            .ok_or_else(|| AssetError::NotFound(id.to_string()))?;
        st.files.entry(id).or_insert_with(|| FileRecord::new(path));
        self.queue_import(&mut st, id);
        Ok(())
    }

    /// The handle of an already requested asset, without loading.
    pub fn get_handle<T: 'static>(&self, r: &AssetRef) -> Option<Handle<T>> {
        let st = lock(&self.inner.state);
        let key = self.lookup_key(&st, r)?;
        let slot = st.slots.get(&key)?;
        (slot.type_id == TypeId::of::<T>())
            .then(|| slot.handle.downcast_ref::<Handle<T>>().copied())
            .flatten()
    }

    /// The load state of a requested asset; `None` if it was never requested.
    pub fn state(&self, r: &AssetRef) -> Option<LoadState> {
        let st = lock(&self.inner.state);
        let key = self.lookup_key(&st, r)?;
        st.slots.get(&key).map(|s| s.state.clone())
    }

    /// The load state behind a handle returned by [`load`](Self::load).
    pub fn handle_state<T: 'static>(&self, handle: Handle<T>) -> Option<LoadState> {
        let st = lock(&self.inner.state);
        let key = st.by_raw.get(&handle.id())?;
        st.slots.get(key).map(|s| s.state.clone())
    }

    /// True when the handle's store value is the type's placeholder.
    pub fn is_placeholder<T: 'static>(&self, handle: Handle<T>) -> bool {
        let st = lock(&self.inner.state);
        st.by_raw
            .get(&handle.id())
            .and_then(|k| st.slots.get(k))
            .is_some_and(|s| s.placeholder)
    }

    /// The state of a file's latest import; `None` if it was never imported.
    /// Shows failures of files loaded with [`load_file`](Self::load_file),
    /// which have no slots when the import fails.
    pub fn file_state(&self, id: AssetId) -> Option<LoadState> {
        let st = lock(&self.inner.state);
        let f = st.files.get(&id)?;
        if f.importing {
            Some(LoadState::Loading)
        } else if f.imported {
            Some(LoadState::Loaded)
        } else {
            f.failed.clone().map(LoadState::Failed)
        }
    }

    /// The sub-assets of a file that have slots (all of them once the file
    /// has been imported), sorted by label.
    pub fn sub_assets(&self, id: AssetId) -> Vec<SubAsset> {
        let st = lock(&self.inner.state);
        let Some(file) = st.files.get(&id) else {
            return Vec::new();
        };
        file.labels
            .iter()
            .filter_map(|label| {
                let slot = st.slots.get(&(id, label.clone()))?;
                Some(SubAsset {
                    label: label.clone(),
                    type_name: st.types.get(&slot.type_id).map_or("?", |t| t.name),
                    state: slot.state.clone(),
                })
            })
            .collect()
    }

    /// Every indexed asset file as `(id, path)`, sorted by path.
    pub fn files(&self) -> Vec<(AssetId, String)> {
        self.index().entries()
    }

    /// The current path (relative to `assets/`) of an asset file.
    pub fn path_of(&self, id: AssetId) -> Option<String> {
        self.index().path_of(id).map(str::to_string)
    }

    /// The id of the asset file at `path` (relative to `assets/`).
    pub fn id_of(&self, path: &str) -> Option<AssetId> {
        self.index().id_of(&normalize_rel(path)?)
    }

    /// The reference in its canonical form: id, label and current path.
    /// Repairs path-only references (fills the id) and stale path hints.
    /// `None` when neither the id nor the path names an indexed file.
    pub fn canonical_ref(&self, r: &AssetRef) -> Option<AssetRef> {
        let index = self.index();
        let id =
            r.id.filter(|id| index.path_of(*id).is_some())
                .or_else(|| index.id_of(&normalize_rel(r.path.as_deref()?)?))?;
        Some(AssetRef {
            id: Some(id),
            label: r.label.clone(),
            path: index.path_of(id).map(str::to_string),
        })
    }

    /// Files whose `.meta` cannot be used (they are not in
    /// [`files`](Self::files)), as `(path, message)` sorted by path.
    pub fn broken_files(&self) -> Vec<(String, String)> {
        self.index().broken_entries()
    }

    /// A counter bumped whenever the indexed files, their paths or the
    /// broken files change (scans, watcher events, renames). Cheap to read
    /// every frame; rebuild listings only when it changes.
    pub fn index_generation(&self) -> u64 {
        self.index().generation()
    }

    /// The `.meta` sidecar of an asset file (its importer and settings).
    pub fn meta(&self, id: AssetId) -> Result<meta::Meta, AssetError> {
        let path = self
            .path_of(id)
            .ok_or_else(|| AssetError::NotFound(id.to_string()))?;
        let abs = self.inner.shared.root.join(&path);
        match meta::read_meta(&abs) {
            Ok(Some(m)) => Ok(m),
            Ok(None) => Err(AssetError::BadMeta {
                path,
                message: "missing".into(),
            }),
            Err(message) => Err(AssetError::BadMeta { path, message }),
        }
    }

    /// Replaces the import settings in an asset's `.meta` (atomically, the
    /// id and importer are kept) and re-imports the file when it has been
    /// requested, so its slots get the new result (state `Loading`, then
    /// `Loaded` with bumped versions). `settings` must be a JSON object;
    /// for the built-in `texture` and `gltf` importers it is checked against
    /// [`TextureSettings`](meta::TextureSettings) and
    /// [`ModelSettings`](meta::ModelSettings) first (a wrongly typed field is
    /// refused, unknown fields are kept with a warning).
    pub fn set_settings(&self, id: AssetId, settings: serde_json::Value) -> Result<(), AssetError> {
        let mut m = self.meta(id)?;
        let path = self.path_of(id).unwrap_or_default();
        self.check_writable(&path)?;
        let bad = |message: String| AssetError::BadMeta {
            path: path.clone(),
            message,
        };
        if !settings.is_object() {
            return Err(bad("settings must be a JSON object".into()));
        }
        let unknown = match m.importer.as_str() {
            "texture" => meta::parse_settings::<meta::TextureSettings>(&settings).map(|s| s.1),
            "gltf" => meta::parse_settings::<meta::ModelSettings>(&settings).map(|s| s.1),
            _ => Ok(Vec::new()),
        }
        .map_err(&bad)?;
        for key in unknown {
            tracing::warn!("{path}.meta: unknown setting `{key}` kept");
        }
        if m.importer == "gltf"
            && let Some(scale) = settings.get("scale").and_then(serde_json::Value::as_f64)
            && !(scale.is_finite() && scale > 0.0)
        {
            return Err(bad(format!("scale must be finite and > 0, got {scale}")));
        }
        m.settings = settings;
        let abs = self.inner.shared.root.join(&path);
        meta::write_meta(&abs, &m).map_err(|source| AssetError::Io {
            path: meta::meta_path(&abs),
            source,
        })?;
        tracing::info!("{path}.meta: import settings changed");
        self.reimport_if_requested(id);
        Ok(())
    }

    /// Replaces an asset file's content (atomically: a hidden temporary file
    /// renamed into place) and re-imports it when it has been requested.
    /// For editors of text assets such as `.material.json`.
    pub fn write(&self, id: AssetId, bytes: &[u8]) -> Result<(), AssetError> {
        let path = self
            .path_of(id)
            .ok_or_else(|| AssetError::NotFound(id.to_string()))?;
        self.check_writable(&path)?;
        let abs = self.inner.shared.root.join(&path);
        write_hidden_then_rename(&abs, bytes)?;
        self.reimport_if_requested(id);
        Ok(())
    }

    /// Creates a new asset file at `path` (relative to `assets/`; missing
    /// folders are created) with `bytes`, indexes it (creating its `.meta`)
    /// and returns its id. Refuses to overwrite an existing file.
    pub fn create(&self, path: &str, bytes: &[u8]) -> Result<AssetId, AssetError> {
        let rel = normalize_rel(path).ok_or_else(|| AssetError::OutsideAssets(path.into()))?;
        self.check_writable(&rel)?;
        let abs = self.inner.shared.root.join(&rel);
        if abs.exists() {
            return Err(AssetError::AlreadyExists(rel));
        }
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(|source| AssetError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        write_hidden_then_rename(&abs, bytes)?;
        let mut st = lock(&self.inner.state);
        if self.index().id_of(&rel).is_none() {
            let changes = self.index_mut().refresh(&rel, &st.importers);
            self.apply_changes(&mut st, changes);
        }
        if let Some(id) = self.index().id_of(&rel) {
            return Ok(id);
        }
        Err(match self.index().broken(&rel) {
            Some(message) => AssetError::BadMeta {
                path: rel,
                message: message.to_string(),
            },
            None => AssetError::NoImporter(rel),
        })
    }

    fn reimport_if_requested(&self, id: AssetId) {
        let mut st = lock(&self.inner.state);
        if st.files.get(&id).is_some_and(FileRecord::requested) {
            self.queue_import(&mut st, id);
        }
    }

    /// True when no import is running or queued and no result is waiting
    /// for [`update`](Self::update).
    pub fn is_idle(&self) -> bool {
        let st = lock(&self.inner.state);
        st.in_flight == 0 && st.pending.is_empty()
    }

    /// Calls [`update`](Self::update) until the server is idle or `timeout`
    /// passes; returns whether it became idle. For tools and headless runs
    /// (CLI render, tests) that must not draw before assets are in.
    pub fn update_until_idle(&self, world: &mut World, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            self.update(world);
            if self.is_idle() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// How many finished imports [`update`](Self::update) has applied so
    /// far (successful or not). Lets a consumer waiting on files loaded with
    /// [`load_file`](Self::load_file), whose failures send no event when the
    /// file has no slots, re-check only when something finished.
    pub fn finished_imports(&self) -> u64 {
        lock(&self.inner.state).finished
    }

    /// Import durations of the imports finished so far, `(path, time)`, in
    /// completion order. Drained by the call.
    pub fn take_import_timings(&self) -> Vec<(String, Duration)> {
        std::mem::take(&mut lock(&self.inner.state).timings)
    }

    // ---------------------------------------------------------- identity

    /// Re-scans the assets folder: new files get `.meta` files, files moved
    /// together with their `.meta` by another tool keep their id (their
    /// slots and `AssetLookup` paths follow), deleted files leave their
    /// slots with the placeholder, and loaded files whose content, settings
    /// or dependencies (glTF buffers, external images) changed are
    /// re-imported. Use it when watching is off.
    pub fn rescan(&self) {
        let mut st = lock(&self.inner.state);
        let changes = self.index_mut().scan(&st.importers);
        self.apply_changes(&mut st, changes);
    }

    /// Renames or moves an asset file together with its `.meta` (paths
    /// relative to `assets/`; missing folders are created). The id is kept,
    /// so references by id still resolve; slots stay valid, the index and
    /// (at the next [`update`](Self::update)) the world's `AssetLookup`
    /// follow, and `Moved` events are sent. If the `.meta` cannot be moved
    /// the file is moved back.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), AssetError> {
        let from = normalize_rel(from).ok_or_else(|| AssetError::OutsideAssets(from.into()))?;
        let to = normalize_rel(to).ok_or_else(|| AssetError::OutsideAssets(to.into()))?;
        self.check_writable(&from)?;
        let mut st = lock(&self.inner.state);
        let id = self
            .index()
            .id_of(&from)
            .ok_or_else(|| AssetError::NotFound(from.clone()))?;
        let root = &self.inner.shared.root;
        let (src, dst) = (root.join(&from), root.join(&to));
        let (src_meta, dst_meta) = (meta::meta_path(&src), meta::meta_path(&dst));
        if dst.exists() || dst_meta.exists() {
            return Err(AssetError::AlreadyExists(to));
        }
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| AssetError::Io { path, source }
        };
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::rename(&src, &dst).map_err(io(&src))?;
        if let Err(e) = std::fs::rename(&src_meta, &dst_meta) {
            let _ = std::fs::rename(&dst, &src);
            return Err(io(&src_meta)(e));
        }
        self.index_mut().moved(id, &to);
        self.apply_changes(&mut st, vec![Change::Moved { id, from }]);
        Ok(())
    }

    // ------------------------------------------------------------ watching

    /// Turns hot reload on or off. On: the assets folder is watched
    /// (debounced, see [`DEBOUNCE`](crate::watch::DEBOUNCE)) and changes are
    /// handled in [`update`](Self::update). Meant for the editor and
    /// development builds, off in exported games.
    pub fn watch(&self, enabled: bool) -> Result<(), AssetError> {
        let mut w = lock(&self.inner.watcher);
        if enabled {
            if self.inner.shared.read_only {
                return Err(AssetError::ReadOnly(
                    self.inner.shared.root.display().to_string(),
                ));
            }
            if w.is_none() {
                *w = Some(Watcher::start(&self.inner.shared.root)?);
            }
        } else {
            *w = None;
        }
        Ok(())
    }

    /// True while watching.
    pub fn is_watching(&self) -> bool {
        lock(&self.inner.watcher).is_some()
    }

    // ------------------------------------------------------------- update

    /// Applies file changes reported by the watcher, finished imports and
    /// queued slot changes to `world`: values go into `Assets<T>`
    /// (`Assets::set`, so handles stay valid and versions bump), every slot
    /// is registered in the world's [`AssetLookup`] (id, label, path), and
    /// [`Events<AssetEvent>`] are sent. Missing resources are created. Run it
    /// once per frame on the main thread.
    pub fn update(&self, world: &mut World) {
        profiling::scope!("AssetServer::update");
        let paths = lock(&self.inner.watcher)
            .as_ref()
            .map(Watcher::drain)
            .unwrap_or_default();
        let mut st = lock(&self.inner.state);
        if !paths.is_empty() {
            self.handle_paths(&mut st, paths);
        }
        let results: Vec<JobResult> = lock(&self.inner.results).try_iter().collect();
        let mut events = Vec::new();
        for result in results {
            st.in_flight = st.in_flight.saturating_sub(1);
            st.finished += 1;
            apply_result(&mut st, world, result, &mut events);
        }
        for pending in std::mem::take(&mut st.pending) {
            apply_pending(&mut st, world, pending, &mut events);
        }
        drop(st);
        if !events.is_empty() {
            world
                .resource_or_default::<Events<AssetEvent>>()
                .send_batch(events);
        }
    }

    // ------------------------------------------------------------ helpers

    fn resolve(&self, st: &mut State, r: &AssetRef) -> Result<(AssetId, String), AssetError> {
        if r.is_none() {
            return Err(AssetError::NotFound("none".into()));
        }
        if let Some(id) = r.id {
            if let Some(p) = self.index().path_of(id) {
                return Ok((id, p.to_string()));
            }
            // Known but its file is gone: the slot keeps the placeholder.
            if let Some(f) = st.files.get(&id) {
                return Ok((id, f.path.clone()));
            }
        }
        let Some(raw_path) = &r.path else {
            return Err(AssetError::NotFound(r.to_string()));
        };
        let path =
            normalize_rel(raw_path).ok_or_else(|| AssetError::OutsideAssets(raw_path.clone()))?;
        let mut found = self.index().id_of(&path);
        if found.is_none() && self.index().broken(&path).is_none() {
            // Not seen yet (watching off, or the event has not arrived).
            let changes = self.index_mut().refresh(&path, &st.importers);
            self.apply_changes(st, changes);
            found = self.index().id_of(&path);
        }
        if let Some(found) = found {
            if let Some(id) = r.id {
                tracing::warn!(
                    "asset {id} not found; repaired the reference by its path {path} (id {found})"
                );
            }
            return Ok((found, path));
        }
        if let Some(message) = self.index().broken(&path) {
            return Err(AssetError::BadMeta {
                path,
                message: message.to_string(),
            });
        }
        if crate::vfs::is_file(self.inner.shared.root.join(&path))
            && st.importers.for_path(&path).is_none()
        {
            return Err(AssetError::NoImporter(path));
        }
        Err(AssetError::NotFound(r.to_string()))
    }

    /// The slot key of a reference without touching the disk.
    fn lookup_key(&self, st: &State, r: &AssetRef) -> Option<SlotKey> {
        let index = self.index();
        let id =
            r.id.filter(|id| index.path_of(*id).is_some() || st.files.contains_key(id))
                .or_else(|| index.id_of(&normalize_rel(r.path.as_deref()?)?))?;
        Some((id, r.label.clone()))
    }

    fn queue_import(&self, st: &mut State, id: AssetId) {
        st.next_generation += 1;
        let generation = st.next_generation;
        let importers = st.importers.clone();
        let Some(file) = st.files.get_mut(&id) else {
            return;
        };
        file.generation = generation;
        file.importing = true;
        file.fingerprint = Some(fingerprint(&self.inner.shared.root.join(&file.path)));
        let job = Job {
            id,
            path: file.path.clone(),
            generation,
            importers,
        };
        if st.jobs.send(job).is_ok() {
            st.in_flight += 1;
        } else {
            tracing::error!("asset import workers are gone; {} not imported", file.path);
            file.importing = false;
        }
    }

    /// Reacts to index changes (see [`rescan`](Self::rescan)).
    fn apply_changes(&self, st: &mut State, changes: Vec<Change>) {
        for change in changes {
            match change {
                Change::Unchanged { id } => {
                    let root = &self.inner.shared.root;
                    let stale = st.files.get(&id).is_some_and(|f| {
                        f.requested()
                            && (f.fingerprint != Some(fingerprint(&root.join(&f.path)))
                                || f.dependencies
                                    .iter()
                                    .any(|(d, s)| stamp(&root.join(d)) != *s))
                    });
                    if stale {
                        self.queue_import(st, id);
                    }
                }
                Change::New { id } => {
                    // A file that came back (deleted, then restored).
                    let path = self.index().path_of(id).map(str::to_string);
                    if let (Some(f), Some(path)) = (st.files.get_mut(&id), path) {
                        f.path = path;
                        if f.requested() {
                            self.queue_import(st, id);
                        }
                    }
                }
                Change::Moved { id, from } => {
                    let Some(path) = self.index().path_of(id).map(str::to_string) else {
                        continue;
                    };
                    if let Some(f) = st.files.get_mut(&id) {
                        tracing::info!("{from}: moved to {path}");
                        f.path = path.clone();
                        st.pending.push(Pending::Moved { id, from, path });
                    }
                }
                Change::Removed { id, path } => {
                    let Some(f) = st.files.get_mut(&id) else {
                        continue;
                    };
                    tracing::warn!("{path}: deleted; its assets now hold placeholders");
                    let message = format!("{path} was deleted");
                    f.imported = false;
                    f.failed = Some(message.clone());
                    // A running import's result is stale now.
                    f.generation = u64::MAX;
                    f.importing = false;
                    let labels: Vec<_> = f.labels.iter().cloned().collect();
                    for label in labels {
                        let key = (id, label);
                        if let Some(slot) = st.slots.get_mut(&key) {
                            slot.state = LoadState::Failed(message.clone());
                        }
                        st.pending.push(Pending::Placeholder {
                            key,
                            kind: AssetEventKind::Removed,
                            message: message.clone(),
                        });
                    }
                }
            }
        }
    }

    /// File system events: refresh the index for each path (existing files
    /// first, so a move is seen before the removal of its old path), then
    /// re-import files that depend on a changed path.
    fn handle_paths(&self, st: &mut State, paths: Vec<PathBuf>) {
        let root = self.inner.shared.root.clone();
        let mut rels = BTreeSet::new();
        for p in paths {
            let Some(rel) = index::rel_of(&root, &p) else {
                continue;
            };
            if index::is_hidden(&rel) {
                continue;
            }
            let rel = rel.strip_suffix(META_SUFFIX).unwrap_or(&rel).to_string();
            let abs = root.join(&rel);
            if abs.is_dir() {
                let mut files = Vec::new();
                index::walk(&root, &abs, &mut files);
                rels.extend(files);
            } else {
                rels.extend(self.index().paths_under(&rel));
                rels.insert(rel);
            }
        }
        let (existing, gone): (Vec<_>, Vec<_>) =
            rels.iter().partition(|rel| root.join(rel).is_file());
        let mut changes = Vec::new();
        {
            let mut index = self.index_mut();
            for rel in existing.into_iter().chain(gone) {
                changes.extend(index.refresh(rel, &st.importers));
            }
        }
        self.apply_changes(st, changes);
        let dependents: Vec<AssetId> = st
            .files
            .iter()
            .filter(|(_, f)| f.dependencies.iter().any(|(d, _)| rels.contains(d)))
            .map(|(id, _)| *id)
            .collect();
        for id in dependents {
            tracing::info!("re-importing {id}: a file it depends on changed");
            self.queue_import(st, id);
        }
    }
}

/// Writes `abs` through a hidden temporary file next to it and a rename, so
/// readers and the watcher never see a half-written file.
fn write_hidden_then_rename(abs: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    let name = abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = abs.with_file_name(format!(".{name}.tmp"));
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| AssetError::Io { path, source }
    };
    std::fs::write(&tmp, bytes).map_err(io(&tmp))?;
    std::fs::rename(&tmp, abs).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(abs)(e)
    })
}

fn typed_handle<T: 'static>(
    st: &State,
    slot: &Slot,
    r: &AssetRef,
) -> Result<Handle<T>, AssetError> {
    if slot.type_id == TypeId::of::<T>()
        && let Some(h) = slot.handle.downcast_ref::<Handle<T>>()
    {
        return Ok(*h);
    }
    Err(AssetError::TypeMismatch {
        reference: r.to_string(),
        loaded: st.types.get(&slot.type_id).map_or("?", |t| t.name),
        requested: std::any::type_name::<T>(),
    })
}

fn missing_label_message(
    path: &str,
    label: Option<&str>,
    produced: &BTreeSet<Option<String>>,
) -> String {
    let available: Vec<&str> = produced
        .iter()
        .flatten()
        .map(String::as_str)
        .take(8)
        .collect();
    match label {
        None => format!(
            "{path} has no main asset; load a sub-asset such as {path}#{}",
            available
                .iter()
                .find(|l| **l == "Scene")
                .or(available.first())
                .unwrap_or(&"Scene")
        ),
        Some(l) => format!(
            "{path} has no sub-asset `{l}` (it has: {})",
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        ),
    }
}

fn event(
    st: &State,
    key: &SlotKey,
    kind: AssetEventKind,
    message: Option<String>,
    old_path: Option<String>,
) -> Option<AssetEvent> {
    let slot = st.slots.get(key)?;
    let path = st.files.get(&key.0)?.path.clone();
    Some(AssetEvent {
        kind,
        id: key.0,
        label: key.1.clone(),
        path,
        type_name: st.types.get(&slot.type_id).map_or("?", |t| t.name),
        type_id: slot.type_id,
        handle: slot.raw,
        message,
        old_path,
    })
}

/// Stores the placeholder in a slot (if its type has one) and registers it.
fn store_placeholder(st: &mut State, world: &mut World, key: &SlotKey) {
    let Some(path) = st.files.get(&key.0).map(|f| f.path.clone()) else {
        return;
    };
    let State { slots, types, .. } = st;
    let Some(slot) = slots.get_mut(key) else {
        return;
    };
    let Some(ty) = types.get(&slot.type_id) else {
        return;
    };
    if ty.ops.set_placeholder(world, slot.handle.as_ref()) {
        slot.placeholder = true;
        ty.ops
            .register(world, slot.handle.as_ref(), key.0, key.1.as_deref(), &path);
    }
}

fn apply_pending(
    st: &mut State,
    world: &mut World,
    pending: Pending,
    events: &mut Vec<AssetEvent>,
) {
    match pending {
        Pending::Placeholder { key, kind, message } => {
            store_placeholder(st, world, &key);
            events.extend(event(st, &key, kind, Some(message), None));
        }
        Pending::Moved { id, from, path } => {
            world.resource_or_default::<AssetLookup>().rename(id, &path);
            let labels: Vec<_> = st
                .files
                .get(&id)
                .map(|f| f.labels.iter().cloned().collect())
                .unwrap_or_default();
            for label in labels {
                events.extend(event(
                    st,
                    &(id, label),
                    AssetEventKind::Moved,
                    None,
                    Some(from.clone()),
                ));
            }
        }
    }
}

enum Applied {
    /// The type is not registered.
    Skipped,
    Stored(AssetEventKind),
    /// The slot was requested as another type.
    WrongType(String),
}

/// Stores one imported value in its slot (created if needed).
fn apply_output(
    st: &mut State,
    world: &mut World,
    path: &str,
    key: &SlotKey,
    out: ImportedAsset,
) -> Applied {
    let State {
        slots,
        types,
        by_raw,
        files,
        ..
    } = st;
    let Some(ty) = types.get(&out.type_id) else {
        tracing::warn!(
            "{path}: the importer produced {} (not a registered asset type); skipped",
            out.type_name
        );
        return Applied::Skipped;
    };
    let slot = slots.entry(key.clone()).or_insert_with(|| {
        let (raw, handle) = ty.ops.new_handle();
        by_raw.insert(raw, key.clone());
        Slot {
            type_id: out.type_id,
            raw,
            handle,
            state: LoadState::Loading,
            placeholder: false,
            loaded_once: false,
        }
    });
    if let Some(f) = files.get_mut(&key.0) {
        f.labels.insert(key.1.clone());
    }
    if slot.type_id != out.type_id {
        let requested = types.get(&slot.type_id).map_or("?", |t| t.name);
        let message = format!(
            "{path}#{}: is a {}, not a {requested}",
            key.1.as_deref().unwrap_or(""),
            out.type_name
        );
        slot.state = LoadState::Failed(message.clone());
        return Applied::WrongType(message);
    }
    ty.ops.set(world, slot.handle.as_ref(), out.value);
    ty.ops
        .register(world, slot.handle.as_ref(), key.0, key.1.as_deref(), path);
    let kind = if slot.loaded_once {
        AssetEventKind::Reloaded
    } else {
        AssetEventKind::Loaded
    };
    slot.loaded_once = true;
    slot.placeholder = false;
    slot.state = LoadState::Loaded;
    Applied::Stored(kind)
}

fn apply_result(
    st: &mut State,
    world: &mut World,
    result: JobResult,
    events: &mut Vec<AssetEvent>,
) {
    let id = result.id;
    let Some(file) = st.files.get_mut(&id) else {
        return;
    };
    if file.generation != result.generation {
        return; // superseded by a newer import (or the file was deleted)
    }
    file.importing = false;
    file.dependencies = result.dependencies;
    let path = file.path.clone();
    st.timings.push((path.clone(), result.elapsed));
    match result.outcome {
        Ok(outputs) => {
            tracing::debug!(
                "{path}: imported {} asset(s) in {:.1} ms",
                outputs.len(),
                result.elapsed.as_secs_f64() * 1000.0
            );
            if let Some(f) = st.files.get_mut(&id) {
                f.imported = true;
                f.failed = None;
            }
            let mut produced = BTreeSet::new();
            let mut reloaded = false;
            for out in outputs {
                let key = (id, out.label.clone());
                let applied = apply_output(st, world, &path, &key, out);
                match applied {
                    Applied::Skipped => {}
                    Applied::Stored(kind) => {
                        reloaded |= kind == AssetEventKind::Reloaded;
                        produced.insert(key.1.clone());
                        events.extend(event(st, &key, kind, None, None));
                    }
                    Applied::WrongType(message) => {
                        produced.insert(key.1.clone());
                        tracing::error!("{message}");
                        store_placeholder(st, world, &key);
                        events.extend(event(st, &key, AssetEventKind::Failed, Some(message), None));
                    }
                }
            }
            if reloaded {
                tracing::info!(
                    "reloaded {path} ({:.0} ms)",
                    result.elapsed.as_secs_f64() * 1000.0
                );
            }
            // Requested labels the file does not have (or no longer has).
            let labels = match st.files.get_mut(&id) {
                Some(f) => {
                    f.produced = produced.clone();
                    f.labels.clone()
                }
                None => return,
            };
            for label in labels.iter().filter(|l| !produced.contains(*l)) {
                let key = (id, label.clone());
                let message = missing_label_message(&path, label.as_deref(), &produced);
                if let Some(slot) = st.slots.get_mut(&key) {
                    if slot.state == LoadState::Failed(message.clone()) {
                        continue;
                    }
                    slot.state = LoadState::Failed(message.clone());
                }
                tracing::error!("{message}");
                store_placeholder(st, world, &key);
                events.extend(event(st, &key, AssetEventKind::Failed, Some(message), None));
            }
        }
        Err(e) => {
            let message = failure_message(&path, &e);
            tracing::error!("{path}: import failed: {message}");
            let labels = match st.files.get_mut(&id) {
                Some(f) => {
                    f.imported = false;
                    f.failed = Some(message.clone());
                    f.labels.clone()
                }
                None => return,
            };
            for label in labels {
                let key = (id, label);
                let keep_last_good = match st.slots.get_mut(&key) {
                    Some(slot) => {
                        slot.state = LoadState::Failed(message.clone());
                        slot.loaded_once && !slot.placeholder
                    }
                    None => continue,
                };
                if !keep_last_good {
                    store_placeholder(st, world, &key);
                }
                events.extend(event(
                    st,
                    &key,
                    AssetEventKind::Failed,
                    Some(message.clone()),
                    None,
                ));
            }
        }
    }
}

// --------------------------------------------------------------- workers ----

fn worker(shared: &Shared, jobs: &Mutex<Receiver<Job>>, results: &Sender<JobResult>) {
    loop {
        let job = match lock(jobs).recv() {
            Ok(job) => job,
            Err(_) => return, // the server is gone
        };
        let start = Instant::now();
        let (outcome, dependencies) = run_job(shared, &job);
        let dependencies = dependencies
            .into_iter()
            .map(|d| {
                let s = stamp(&shared.root.join(&d));
                (d, s)
            })
            .collect();
        let result = JobResult {
            id: job.id,
            generation: job.generation,
            outcome,
            dependencies,
            elapsed: start.elapsed(),
        };
        if results.send(result).is_err() {
            return;
        }
    }
}

/// Binary formats whose decode errors mean a damaged or incomplete file.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "glb", "gltf", "wav", "ogg", "mp3", "flac", "ttf", "otf",
];

/// The message of a failed import: a decode error of a binary format says
/// what to do about it (the decoder's own text rarely does).
pub(crate) fn failure_message(path: &str, e: &ImportError) -> String {
    let message = e.to_string();
    let ext = path.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase());
    match (e, ext) {
        (ImportError::Decode(_), Some(ext)) if BINARY_EXTENSIONS.contains(&ext.as_str()) => {
            let format = match ext.as_str() {
                "glb" | "gltf" => "glTF".to_string(),
                other => other.to_ascii_uppercase(),
            };
            format!(
                "{}; the file is damaged, incomplete or not really {format}: export or download it again",
                message.trim_end_matches([';', '.', ' '])
            )
        }
        _ => message,
    }
}

fn run_job(shared: &Shared, job: &Job) -> (Result<Vec<ImportedAsset>, ImportError>, Vec<String>) {
    profiling::scope!("import", &job.path);
    let abs = shared.root.join(&job.path);
    let meta = match meta::read_meta(&abs) {
        Ok(m) => m,
        Err(e) => {
            return (
                Err(ImportError::Settings(format!("{}.meta: {e}", job.path))),
                Vec::new(),
            );
        }
    };
    let importer = match &meta {
        Some(m) => job.importers.by_name(&m.importer).or_else(|| {
            tracing::warn!(
                "{}.meta: unknown importer `{}`; using the one for its extension",
                job.path,
                m.importer
            );
            job.importers.for_path(&job.path)
        }),
        None => job.importers.for_path(&job.path),
    };
    let Some(importer) = importer else {
        return (
            Err(ImportError::Unsupported(format!(
                "no importer for {}",
                job.path
            ))),
            Vec::new(),
        );
    };
    let settings = meta.map_or_else(|| importer.default_settings(), |m| m.settings);
    let bytes = match crate::vfs::read(&abs) {
        Ok(b) => b,
        Err(e) => {
            return (
                Err(ImportError::Io(format!("{}: {e}", job.path))),
                Vec::new(),
            );
        }
    };
    let mut ctx = ImportContext {
        id: job.id,
        path: &job.path,
        bytes,
        settings,
        root: &shared.root,
        index: &shared.index,
        outputs: Vec::new(),
        dependencies: Vec::new(),
    };
    let outcome = box_core::panic_guard::catch(|| importer.import(&mut ctx));
    let outcome = match outcome {
        Ok(Ok(())) => Ok(std::mem::take(&mut ctx.outputs)),
        Ok(Err(e)) => Err(e),
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(ImportError::Decode(format!(
                "importer `{}` panicked: {msg}",
                importer.name()
            )))
        }
    };
    (outcome, ctx.dependencies)
}
