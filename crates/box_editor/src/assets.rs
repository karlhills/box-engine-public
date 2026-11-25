//! Assets in the editor (M4b): the project's [`AssetServer`] on the edit
//! world, the asset browser's state, and the asset operations the browser,
//! the viewport, the hierarchy and the inspector call. No UI here, so every
//! operation is tested headlessly; `panes::assets` draws the browser.
//!
//! - **Server.** With a project open, one `AssetServer` for its `assets/`
//!   folder, watching for changes, is a resource of the edit world (with
//!   `AssetLookup` and `Events<AssetEvent>`, as `AssetPlugin` would add) and
//!   is updated once per editor frame. It is recreated whenever the edit
//!   world is replaced (project or scene switch, recovery restore), because
//!   one server serves one world: its values live in that world's `Assets`
//!   stores. During Play it keeps driving the parked edit world; the play
//!   world gets no server (the asset glue decides what Play worlds load).
//! - **Edits.** Entities created by drops, material assignments (a link,
//!   `MaterialRef`, for `Primitive` entities) and path repairs go through
//!   the edit stack, one undo step each. Asset files (renames, import
//!   settings) are file operations, not undo steps; material file edits are
//!   steps of the file's own history (`asset_undo.rs`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use box_assets::meta::{ModelSettings, TextureSettings};
use box_assets::{AssetEvent, AssetEventKind, AssetServer, LoadState, SceneAsset};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::event::{EventCursor, Events};
use box_core::identity::Uid;
use box_core::image::ImageData;
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::mesh_asset::{MaterialRef, MeshAsset};
use box_core::prefab::PrefabInstance;
use box_core::prelude::{Entity, Primitive, Transform};
use box_core::reflect::{PartialReflect, ReflectRef};
use box_core::shader_material::{MaterialHeader, ShaderSource, parse_material_header};
use box_core::world::Placement;
use glam::{Vec2, Vec3};

use crate::state::EditorState;

/// File name suffixes with more than one dot, checked before the last dot.
const LONG_SUFFIXES: [&str; 3] = [".material.json", ".scene.json", ".prefab.json"];

/// A file name split into stem and suffix: `tower.glb` → (`tower`, `.glb`),
/// `stone.material.json` → (`stone`, `.material.json`).
pub fn split_name(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    for s in LONG_SUFFIXES {
        if lower.ends_with(s) && name.len() > s.len() {
            return name.split_at(name.len() - s.len());
        }
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// The last segment of a `/` path.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The folder of a `/` path (`""` for the assets root).
pub fn parent_folder(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

/// What kind of file an asset is, by its suffix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Image,
    Model,
    Material,
    Scene,
    Shader,
    /// A sound file (`AudioClip`).
    Audio,
    Other,
}

impl FileKind {
    pub fn of(path: &str) -> Self {
        let (_, suffix) = split_name(file_name(path));
        match suffix.to_ascii_lowercase().as_str() {
            ".png" | ".jpg" | ".jpeg" => Self::Image,
            ".glb" | ".gltf" => Self::Model,
            ".material.json" => Self::Material,
            ".scene.json" => Self::Scene,
            ".wgsl" => Self::Shader,
            ".ogg" | ".wav" | ".mp3" | ".flac" => Self::Audio,
            _ => Self::Other,
        }
    }

    /// A short tag for icons.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Image => "IMG",
            Self::Model => "3D",
            Self::Material => "MAT",
            Self::Scene => "SCN",
            Self::Shader => "WGSL",
            Self::Audio => "SND",
            Self::Other => "FILE",
        }
    }
}

/// The type of asset an `AssetRef` points at, used to match references,
/// drags and drop targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefType {
    Image,
    Mesh,
    Material,
    Scene,
    /// A `.wgsl` material shader (`ShaderSource`).
    Shader,
    /// A sound file (`AudioClip`).
    Audio,
    /// Unknown: anything is accepted.
    Any,
}

impl RefType {
    /// From an asset server type name (`SubAsset::type_name`).
    pub fn of_type_name(name: &str) -> Self {
        if name.ends_with("ImageData") {
            Self::Image
        } else if name.ends_with("MeshData") {
            Self::Mesh
        } else if name.ends_with("StandardMaterial") || name.ends_with("ShaderMaterial") {
            Self::Material
        } else if name.ends_with("ShaderSource") {
            Self::Shader
        } else if name.ends_with("SceneAsset") {
            Self::Scene
        } else if name.ends_with("AudioClip") {
            Self::Audio
        } else {
            Self::Any
        }
    }

    /// The type an `AssetRef` field expects, from its path: `*texture*` →
    /// image, `*mesh*` → mesh, `*material*` → material, `*scene*` → scene,
    /// `*clip*`/`*sound*`/`*audio*` → sound.
    /// Field names are the only hint reflection gives; others accept any.
    pub fn of_field(path: &str) -> Self {
        let field = path
            .rsplit('.')
            .next()
            .unwrap_or(path)
            .split('[')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if field.contains("texture") || field.contains("image") {
            Self::Image
        } else if field.contains("shader") {
            Self::Shader
        } else if field.contains("mesh") {
            Self::Mesh
        } else if field.contains("material") {
            Self::Material
        } else if field.contains("scene") || field.contains("prefab") {
            Self::Scene
        } else if field.contains("clip") || field.contains("sound") || field.contains("audio") {
            Self::Audio
        } else {
            Self::Any
        }
    }

    pub fn accepts(self, other: RefType) -> bool {
        self == Self::Any || other == Self::Any || self == other
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "texture",
            Self::Mesh => "mesh",
            Self::Material => "material",
            Self::Scene => "scene",
            Self::Shader => "shader",
            Self::Audio => "sound",
            Self::Any => "asset",
        }
    }
}

/// A browser item: an asset file (`label: None`) or one of its sub-assets.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssetItem {
    pub id: AssetId,
    pub label: Option<String>,
}

impl AssetItem {
    pub fn file(id: AssetId) -> Self {
        Self { id, label: None }
    }

    pub fn sub(id: AssetId, label: &str) -> Self {
        Self {
            id,
            label: Some(label.to_string()),
        }
    }
}

/// An asset being dragged from the browser.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetDrag {
    pub item: AssetItem,
    pub ty: RefType,
    /// Shown next to the pointer.
    pub name: String,
}

/// The state of a reference, for the inspector's picker.
#[derive(Clone, Debug, PartialEq)]
pub enum RefStatus {
    /// References nothing.
    None,
    /// Names no file in the project (or no project is open).
    Missing,
    /// Not requested yet, or importing.
    Loading,
    Failed(String),
    Loaded,
}

/// The cached file listing, rebuilt when the server's index changes.
#[derive(Default)]
pub(crate) struct Listing {
    pub generation: Option<u64>,
    pub files: Vec<(AssetId, String)>,
    /// Files with an unusable `.meta`: path and message.
    pub broken: Vec<(String, String)>,
    /// Every folder (relative path), from file paths and the disk.
    pub folders: BTreeSet<String>,
}

/// Import settings being edited in the inspector.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingsEdit {
    Texture(TextureSettings),
    Model(ModelSettings),
    /// Another importer: shown read-only.
    Other(serde_json::Value),
}

/// The settings as stored and as edited (Apply writes `edit`).
#[derive(Clone, Debug)]
pub struct ImportSettings {
    pub importer: String,
    pub saved: SettingsEdit,
    pub edit: SettingsEdit,
    /// The last Apply's error.
    pub error: Option<String>,
}

impl ImportSettings {
    pub fn is_dirty(&self) -> bool {
        self.saved != self.edit
    }
}

/// A material shown in the inspector: a `.material.json` file (edited and
/// written back) or a glTF material sub-asset (read-only).
#[derive(Clone, Debug)]
pub struct MaterialEdit {
    pub value: StandardMaterial,
    pub read_only: bool,
    /// The file's bytes when it was opened, for Revert.
    pub(crate) original: Vec<u8>,
    /// Changed in the inspector since the last write; the UI time (seconds)
    /// of the last change.
    pub(crate) pending_since: Option<f64>,
    /// What the pending change is (the field's label), for its undo step.
    pub(crate) pending_label: Option<String>,
    /// Some edit was written since the file was opened.
    pub(crate) written: bool,
    /// The bytes last written, so the reload it causes is not taken for an
    /// outside change.
    pub(crate) last_written: Option<Vec<u8>>,
}

impl MaterialEdit {
    /// Revert has something to undo.
    pub fn can_revert(&self) -> bool {
        self.written || self.pending_since.is_some()
    }
}

/// Seconds after the last change before a material edit is written even
/// while the pointer is still down (a long drag).
pub const MATERIAL_WRITE_DELAY_S: f64 = 0.3;

/// The asset the inspector shows instead of the selection.
pub struct Inspected {
    pub item: AssetItem,
    /// The entity selection when the asset was picked; once it changes the
    /// inspector goes back to entities.
    pub(crate) selection: Vec<Uid>,
    pub settings: Option<ImportSettings>,
    pub material: Option<MaterialEdit>,
}

/// The inspector's asset picker popup.
#[derive(Clone, Debug, Default)]
pub struct Picker {
    /// Field path of the reference being picked.
    pub path: String,
    pub filter: String,
}

/// Asset browser state, part of [`EditorState`].
#[derive(Default)]
pub struct BrowserState {
    /// Folder shown, relative to `assets/` (`""` is the root).
    pub folder: String,
    /// Case-insensitive name filter; while set, matches from every folder
    /// are shown.
    pub filter: String,
    pub selected: Option<AssetItem>,
    /// Files whose sub-assets are shown.
    pub expanded: HashSet<AssetId>,
    /// Inline rename: the file and the new stem.
    pub renaming: Option<(AssetId, String)>,
    pub drag: Option<AssetDrag>,
    pub inspect: Option<Inspected>,
    pub picker: Option<Picker>,
    /// The browser had the last click (F2 renames assets, not entities).
    pub active: bool,
    pub(crate) listing: Listing,
    /// Rebuild the folder list (a folder was created) at the next frame.
    pub(crate) folders_dirty: bool,
    /// Files whose import the browser started.
    pub(crate) requested: HashSet<AssetId>,
    /// Asset files moved since the last path repair.
    pub(crate) moved: HashSet<AssetId>,
    pub(crate) events: EventCursor<AssetEvent>,
    pub(crate) thumbs: crate::panes::assets::Thumbnails,
    /// Parsed `@material` headers of shaders shown in the inspector.
    pub(crate) headers: HashMap<String, HeaderEntry>,
    /// Shaders the inspector showed this frame; the paint path asks the
    /// renderer for their compile errors ([`Self::shader_errors`]).
    pub(crate) shader_queries: Vec<Handle<ShaderSource>>,
    /// Compile errors of the queried shaders (by handle id), from the last
    /// rendered frame.
    pub(crate) shader_errors: HashMap<u64, String>,
    /// Undo histories of edited asset files (see `asset_undo.rs`).
    pub(crate) histories: crate::asset_undo::AssetHistories,
}

/// Where a shader's text came from, to know when to parse it again.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HeaderStamp {
    /// A loaded `ShaderSource` (handle id, version).
    Asset(u64, u64),
    /// The `.wgsl` file read directly (length, modification time).
    File(u64, Option<std::time::SystemTime>),
}

pub(crate) struct HeaderEntry {
    stamp: HeaderStamp,
    checked: std::time::Instant,
    info: ShaderInfo,
}

/// A material shader as the inspector sees it.
#[derive(Clone, Debug)]
pub struct ShaderInfo {
    /// The loaded shader, when the world has one (for compile errors).
    pub handle: Option<Handle<ShaderSource>>,
    /// The parsed header (`None`: the shader declares no parameters), or why
    /// it could not be read or parsed.
    pub header: Result<Option<MaterialHeader>, String>,
}

/// How often a `.wgsl` file read directly is checked for changes.
const HEADER_FILE_RECHECK: Duration = Duration::from_secs(1);

impl EditorState {
    /// The project's asset server (none without a project).
    pub fn asset_server(&self) -> Option<&AssetServer> {
        self.asset_server.as_ref()
    }

    /// Creates the asset server for the open project and installs it in the
    /// edit world (called whenever the edit world is replaced). Without a
    /// project there is none.
    pub(crate) fn attach_asset_server(&mut self) {
        self.asset_server = None;
        self.assets.listing = Listing::default();
        self.assets.requested.clear();
        self.assets.moved.clear();
        self.assets.events = EventCursor::default();
        self.assets.inspect = None;
        self.assets.picker = None;
        self.assets.drag = None;
        self.assets.thumbs.clear();
        self.assets.headers.clear();
        self.assets.histories = Default::default();
        let Some(project) = &self.project else {
            return;
        };
        let dir = project.assets_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::error!("assets: cannot create {}: {e}", dir.display());
            return;
        }
        let server = match box_engine::asset_server(&dir) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("assets: {e}");
                return;
            }
        };
        if let Err(e) = server.watch(true) {
            tracing::warn!("assets: hot reload is off: {e}");
        }
        let world = self.edit_world_mut();
        world.insert_resource(server.clone());
        world.resource_or_default::<AssetLookup>();
        world.resource_or_default::<Events<AssetEvent>>();
        if !self.assets.folder.is_empty() && !dir.join(&self.assets.folder).is_dir() {
            self.assets.folder.clear();
        }
        self.asset_server = Some(server);
        self.load_project_settings_resources();
    }

    /// Once per editor frame: applies finished imports and file changes to
    /// the edit world (the parked one during Play), reacts to moves and
    /// reloads, and repairs stale path hints (see [`Self::repair_asset_paths`]).
    pub fn update_assets(&mut self) {
        let Some(server) = self.asset_server.clone() else {
            return;
        };
        let world = self.edit_world_mut();
        if let Some(events) = world.get_resource_mut::<Events<AssetEvent>>() {
            events.update();
        }
        server.update(world);
        self.after_asset_update();
        self.reload_project_settings();
    }

    /// Updates until every import has finished (or `timeout`), for headless
    /// captures and tests. Returns whether the server became idle.
    pub fn wait_assets_idle(&mut self, timeout: Duration) -> bool {
        let Some(server) = self.asset_server.clone() else {
            return true;
        };
        let idle = server.update_until_idle(self.edit_world_mut(), timeout);
        self.after_asset_update();
        idle
    }

    fn after_asset_update(&mut self) {
        let mut cursor = std::mem::take(&mut self.assets.events);
        let events: Vec<AssetEvent> = match self.edit_world().get_resource::<Events<AssetEvent>>() {
            Some(e) => cursor.read(e).cloned().collect(),
            None => Vec::new(),
        };
        self.assets.events = cursor;
        // Loads, failures and removals change the hierarchy's error marks.
        self.asset_marks.events += events.len() as u64;
        for e in &events {
            match e.kind {
                AssetEventKind::Moved => {
                    self.assets.moved.insert(e.id);
                }
                AssetEventKind::Loaded | AssetEventKind::Reloaded => {
                    self.refresh_inspected_material(e);
                }
                _ => {}
            }
        }
        // Not while playing (the edit world and its undo stack are parked)
        // or during a drag (its transaction is open); retried next frame.
        if !self.assets.moved.is_empty() && !self.is_playing() && !self.edit_in_progress() {
            let moved = std::mem::take(&mut self.assets.moved);
            self.repair_asset_paths(&moved);
        }
    }

    // ----- listing -----

    /// Rebuilds the cached listing if the index changed. Cheap otherwise.
    pub(crate) fn refresh_listing(&mut self) {
        let Some(server) = &self.asset_server else {
            return;
        };
        let generation = server.index_generation();
        let l = &mut self.assets.listing;
        if l.generation == Some(generation) && !self.assets.folders_dirty {
            return;
        }
        l.generation = Some(generation);
        l.files = server.files();
        l.broken = server.broken_files();
        let mut folders = BTreeSet::new();
        for p in l
            .files
            .iter()
            .map(|f| f.1.as_str())
            .chain(l.broken.iter().map(|b| b.0.as_str()))
        {
            let mut dir = parent_folder(p);
            while !dir.is_empty() && folders.insert(dir.to_string()) {
                dir = parent_folder(dir);
            }
        }
        walk_folders(server.assets_dir(), "", &mut folders);
        l.folders = folders;
        self.assets.folders_dirty = false;
    }

    /// Every asset file as `(id, path)`, sorted by path (the cached listing).
    pub fn asset_files(&mut self) -> Vec<(AssetId, String)> {
        self.refresh_listing();
        self.assets.listing.files.clone()
    }

    /// Every folder under `assets/`, sorted.
    pub fn asset_folders(&mut self) -> Vec<String> {
        self.refresh_listing();
        self.assets.listing.folders.iter().cloned().collect()
    }

    /// Starts importing a file the browser shows (once), so it gets a state,
    /// sub-assets and a thumbnail.
    pub fn request_file(&mut self, id: AssetId) {
        if !self.assets.requested.insert(id) {
            return;
        }
        if let Some(server) = &self.asset_server
            && let Err(e) = server.load_file(&AssetRef::id(id))
        {
            tracing::warn!("assets: {e}");
        }
    }

    /// The reference for a browser item: id, label and current path.
    pub fn item_ref(&self, item: &AssetItem) -> AssetRef {
        let mut r = AssetRef::id(item.id);
        r.label = item.label.clone();
        r.path = self.asset_server.as_ref().and_then(|s| s.path_of(item.id));
        r
    }

    /// The display name of an item: the file name, or the sub-asset label.
    pub fn item_name(&self, item: &AssetItem) -> String {
        match &item.label {
            Some(l) => l.clone(),
            None => self
                .asset_server
                .as_ref()
                .and_then(|s| s.path_of(item.id))
                .map(|p| file_name(&p).to_string())
                .unwrap_or_else(|| item.id.to_string()),
        }
    }

    /// What type of asset an item is.
    pub fn item_type(&self, item: &AssetItem) -> RefType {
        let Some(server) = &self.asset_server else {
            return RefType::Any;
        };
        if let Some(label) = &item.label {
            if let Some(sub) = server
                .sub_assets(item.id)
                .into_iter()
                .find(|s| s.label.as_deref() == Some(label))
            {
                return RefType::of_type_name(sub.type_name);
            }
            return match label.split('/').next() {
                Some("Mesh") => RefType::Mesh,
                Some("Material") => RefType::Material,
                Some("Texture") => RefType::Image,
                Some("Scene") => RefType::Scene,
                _ => RefType::Any,
            };
        }
        match server.path_of(item.id).map(|p| FileKind::of(&p)) {
            Some(FileKind::Image) => RefType::Image,
            Some(FileKind::Material) => RefType::Material,
            Some(FileKind::Scene) => RefType::Scene,
            Some(FileKind::Shader) => RefType::Shader,
            Some(FileKind::Audio) => RefType::Audio,
            // A glTF's useful whole is its scene.
            Some(FileKind::Model) => RefType::Scene,
            _ => RefType::Any,
        }
    }

    /// The state of a reference: resolved by id (path as a fallback), and
    /// its import state when it has been requested. A reference the asset
    /// server does not index but the world's `AssetLookup` resolves (an
    /// asset added by other code, such as shader sources) counts as loaded.
    pub fn ref_status(&self, r: &AssetRef) -> RefStatus {
        if r.is_none() {
            return RefStatus::None;
        }
        let canonical = self.asset_server.as_ref().and_then(|s| s.canonical_ref(r));
        let (Some(server), Some(c)) = (&self.asset_server, canonical) else {
            return if self.lookup_resolves(r) {
                RefStatus::Loaded
            } else {
                RefStatus::Missing
            };
        };
        let Some(id) = c.id else {
            return RefStatus::Missing;
        };
        match server.state(&c).or_else(|| server.file_state(id)) {
            Some(LoadState::Loaded) => RefStatus::Loaded,
            Some(LoadState::Failed(m)) => RefStatus::Failed(m),
            Some(LoadState::Loading) | None => RefStatus::Loading,
        }
    }

    /// True when the shown or the edit world's `AssetLookup` resolves `r` as
    /// one of the engine's asset types.
    fn lookup_resolves(&self, r: &AssetRef) -> bool {
        [&self.world, self.edit_world()].into_iter().any(|w| {
            w.get_resource::<AssetLookup>().is_some_and(|l| {
                l.get::<ImageData>(r).is_some()
                    || l.get::<MeshData>(r).is_some()
                    || l.get::<StandardMaterial>(r).is_some()
                    || l.get::<SceneAsset>(r).is_some()
                    || l.get::<ShaderSource>(r).is_some()
            })
        })
    }

    /// Assets of type `ty` for the inspector's picker: matching files and
    /// sub-assets of imported files, as `(item, display name)`, sorted by
    /// path. glTF files are imported on the first call so their sub-assets
    /// can be listed.
    pub fn compatible_assets(&mut self, ty: RefType) -> Vec<(AssetItem, String)> {
        let files = self.asset_files();
        let mut out = Vec::new();
        for (id, path) in files {
            let kind = FileKind::of(&path);
            let main_ok = match kind {
                FileKind::Image => ty.accepts(RefType::Image),
                FileKind::Material => ty.accepts(RefType::Material),
                FileKind::Scene => ty.accepts(RefType::Scene),
                FileKind::Shader => ty.accepts(RefType::Shader),
                FileKind::Audio => ty.accepts(RefType::Audio),
                FileKind::Model | FileKind::Other => false,
            };
            if main_ok {
                out.push((AssetItem::file(id), path.clone()));
            }
            if kind == FileKind::Model {
                self.request_file(id);
                if let Some(server) = &self.asset_server {
                    for sub in server.sub_assets(id) {
                        let Some(label) = sub.label else { continue };
                        if ty.accepts(RefType::of_type_name(sub.type_name)) {
                            out.push((AssetItem::sub(id, &label), format!("{path}#{label}")));
                        }
                    }
                }
            }
        }
        out
    }

    // ----- browser operations -----

    /// Selects an item in the browser and shows it in the inspector.
    pub fn select_asset(&mut self, item: Option<AssetItem>) {
        self.close_asset_inspector();
        self.assets.selected = item.clone();
        let Some(item) = item else {
            return;
        };
        let mut inspected = Inspected {
            item: item.clone(),
            selection: self.selection.clone(),
            settings: None,
            material: None,
        };
        if let Some(server) = &self.asset_server {
            if item.label.is_none()
                && let Ok(meta) = server.meta(item.id)
            {
                let parsed = match meta.importer.as_str() {
                    "texture" => {
                        box_assets::meta::parse_settings::<TextureSettings>(&meta.settings)
                            .map(|s| SettingsEdit::Texture(s.0))
                    }
                    "gltf" => box_assets::meta::parse_settings::<ModelSettings>(&meta.settings)
                        .map(|s| SettingsEdit::Model(s.0)),
                    _ => Ok(SettingsEdit::Other(meta.settings.clone())),
                };
                let (edit, error) = match parsed {
                    Ok(s) => (s, None),
                    Err(e) => (SettingsEdit::Other(meta.settings.clone()), Some(e)),
                };
                if meta.importer != "material" && meta.importer != "scene" {
                    inspected.settings = Some(ImportSettings {
                        importer: meta.importer.clone(),
                        saved: edit.clone(),
                        edit,
                        error,
                    });
                }
            }
            if self.item_type(&item) == RefType::Material {
                inspected.material = self.open_material(&item);
            }
        }
        self.close_project_settings();
        self.assets.inspect = Some(inspected);
    }

    fn open_material(&mut self, item: &AssetItem) -> Option<MaterialEdit> {
        let server = self.asset_server.clone()?;
        let r = self.item_ref(item);
        // A glTF sub-asset, or a project opened read-only.
        let read_only = item.label.is_some() || self.read_only;
        let original = match (&r.path, read_only) {
            (Some(p), false) => std::fs::read(server.assets_dir().join(p)).ok()?,
            _ => Vec::new(),
        };
        let handle = match server.load::<StandardMaterial>(&r) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!("assets: {e}");
                return None;
            }
        };
        let value = self
            .edit_world()
            .get_resource::<Assets<StandardMaterial>>()
            .and_then(|a| a.get(handle))
            .cloned()
            // Not imported yet: the reload event fills it in.
            .unwrap_or_default();
        Some(MaterialEdit {
            value,
            read_only,
            original,
            pending_since: None,
            pending_label: None,
            written: false,
            last_written: None,
        })
    }

    /// A material shown in the inspector was (re)imported: take the new
    /// values unless it has edits that are not written yet, or the reload is
    /// the echo of our own write.
    fn refresh_inspected_material(&mut self, e: &AssetEvent) {
        let Some(server) = self.asset_server.clone() else {
            return;
        };
        let Some(insp) = &self.assets.inspect else {
            return;
        };
        if insp.item.id != e.id || insp.item.label != e.label {
            return;
        }
        let Some(m) = &insp.material else {
            return;
        };
        if m.pending_since.is_some() {
            return;
        }
        let echo = m.last_written.is_some() && {
            let path = server.path_of(e.id).map(|p| server.assets_dir().join(p));
            path.and_then(|p| std::fs::read(p).ok()) == m.last_written
        };
        let Some(handle) = server.get_handle::<StandardMaterial>(&self.item_ref(&insp.item)) else {
            return;
        };
        let value = self
            .edit_world()
            .get_resource::<Assets<StandardMaterial>>()
            .and_then(|a| a.get(handle))
            .cloned();
        if let (Some(value), false) = (value, echo)
            && let Some(m) = self
                .assets
                .inspect
                .as_mut()
                .and_then(|i| i.material.as_mut())
        {
            m.value = value;
        }
    }

    /// The inspected asset, if the inspector should show it: dropped once the
    /// entity selection changed since it was picked.
    pub(crate) fn inspected_asset(&mut self) -> Option<&mut Inspected> {
        if self
            .assets
            .inspect
            .as_ref()
            .is_some_and(|i| i.selection != self.selection)
        {
            self.close_asset_inspector();
        }
        self.assets.inspect.as_mut()
    }

    /// Stops showing an asset in the inspector, writing pending material
    /// edits first.
    pub fn close_asset_inspector(&mut self) {
        self.flush_material_edit(f64::INFINITY, true);
        self.assets.inspect = None;
        self.assets.picker = None;
        // The entity inspector comes back: Project Settings and a part
        // inspected read-only (`generated.rs`) too. Every selection change
        // comes here.
        self.close_project_settings();
        self.inspected_part = None;
    }

    /// Opens an item (double-click): a `.scene.json` opens as the scene
    /// (through the unsaved-changes prompt), a `.luau` file in the Script
    /// pane, a glTF shows or hides its sub-assets.
    pub fn open_asset(&mut self, item: &AssetItem) {
        let Some(server) = &self.asset_server else {
            return;
        };
        let Some(path) = server.path_of(item.id) else {
            return;
        };
        if item.label.is_none() && path.ends_with(".luau") {
            if let Err(e) = self.open_script(&path, None) {
                tracing::warn!("{e}");
            }
            return;
        }
        match (FileKind::of(&path), &item.label) {
            (FileKind::Scene, None) => {
                self.requests.open_scene = Some(server.assets_dir().join(&path));
            }
            // Shows or hides the sub-assets.
            (FileKind::Model, None) if !self.assets.expanded.remove(&item.id) => {
                self.assets.expanded.insert(item.id);
                self.request_file(item.id);
            }
            _ => {}
        }
    }

    /// Renames or moves an asset file (with its `.meta`) to `to` (relative
    /// to `assets/`). References by id keep resolving; stale path hints in
    /// the open scene are repaired at the next update. A renamed open scene
    /// keeps being the open scene.
    pub fn rename_asset(&mut self, id: AssetId, to: &str) -> Result<()> {
        self.ensure_writable()?;
        let server = self.asset_server.clone().context("no project is open")?;
        let from = server
            .path_of(id)
            .with_context(|| format!("asset {id} not found"))?;
        let to = box_assets::normalize_rel(to)
            .with_context(|| format!("`{to}` is not a path inside assets/"))?;
        if to == from {
            return Ok(());
        }
        let (_, from_suffix) = split_name(file_name(&from));
        let (_, to_suffix) = split_name(file_name(&to));
        if !from_suffix.eq_ignore_ascii_case(to_suffix) {
            bail!("{to}: renaming cannot change the file type ({from_suffix})");
        }
        let old_abs = server.assets_dir().join(&from);
        let old_recovery = self.recovery_path();
        server.rename(&from, &to)?;
        tracing::info!("renamed {from} to {to}");
        let new_abs = server.assets_dir().join(&to);
        if self
            .scene
            .path
            .as_deref()
            .is_some_and(|p| same_file(p, &old_abs))
        {
            self.scene.path = Some(new_abs);
            self.scene.name = crate::document::scene_name(Path::new(&to));
            if let (Some(old), Some(new)) = (old_recovery, self.recovery_path())
                && old.is_file()
            {
                if let Some(dir) = new.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::rename(old, new);
            }
            self.sync_control_scene();
        }
        self.update_assets();
        Ok(())
    }

    /// Renames a file keeping its folder and suffix: `stem` is the new name
    /// without the suffix.
    pub fn rename_asset_stem(&mut self, id: AssetId, stem: &str) -> Result<()> {
        let stem = stem.trim();
        if stem.is_empty() || stem.contains(['/', '\\']) || stem.starts_with('.') {
            bail!("`{stem}` is not a valid file name");
        }
        let from = self
            .asset_server
            .as_ref()
            .and_then(|s| s.path_of(id))
            .context("asset not found")?;
        let (_, suffix) = split_name(file_name(&from));
        let to = join(parent_folder(&from), &format!("{stem}{suffix}"));
        self.rename_asset(id, &to)
    }

    /// Moves a file into `folder` (relative to `assets/`, `""` the root).
    pub fn move_asset_to_folder(&mut self, id: AssetId, folder: &str) -> Result<()> {
        self.ensure_writable()?;
        let from = self
            .asset_server
            .as_ref()
            .and_then(|s| s.path_of(id))
            .context("asset not found")?;
        self.rename_asset(id, &join(folder, file_name(&from)))
    }

    /// Creates `New Folder` (or `New Folder 2`, ...) in `parent` and returns
    /// its path.
    pub fn new_asset_folder(&mut self, parent: &str) -> Result<String> {
        self.ensure_writable()?;
        let server = self.asset_server.clone().context("no project is open")?;
        let rel = unique_path(server.assets_dir(), parent, "New Folder", "");
        std::fs::create_dir_all(server.assets_dir().join(&rel))
            .with_context(|| format!("creating {rel}"))?;
        self.assets.folders_dirty = true;
        tracing::info!("created folder {rel}");
        Ok(rel)
    }

    /// Creates `New Material.material.json` (default values) in `folder`,
    /// selects it and returns its id.
    pub fn new_material_asset(&mut self, folder: &str) -> Result<AssetId> {
        self.ensure_writable()?;
        let server = self.asset_server.clone().context("no project is open")?;
        let rel = unique_path(
            server.assets_dir(),
            folder,
            "New Material",
            ".material.json",
        );
        let json = self
            .world
            .registry()
            .value_to_json(&StandardMaterial::default())
            .map_err(|e| anyhow!("{e}"))?;
        let id = server.create(&rel, &pretty(&json))?;
        tracing::info!("created {rel}");
        self.select_asset(Some(AssetItem::file(id)));
        Ok(id)
    }

    /// Creates `New Scene.scene.json` (a sun, ground and camera; see
    /// [`crate::document::new_scene_bytes`]) in `folder`, selects it and
    /// returns its id.
    pub fn new_scene_asset(&mut self, folder: &str) -> Result<AssetId> {
        self.ensure_writable()?;
        let server = self.asset_server.clone().context("no project is open")?;
        let rel = unique_path(
            server.assets_dir(),
            folder,
            "New Scene",
            box_engine::project::SCENE_EXTENSION,
        );
        let id = server.create(&rel, &crate::document::new_scene_bytes()?)?;
        tracing::info!("created {rel}");
        self.select_asset(Some(AssetItem::file(id)));
        Ok(id)
    }

    /// Delete in the browser: asks to move the selected file to the trash
    /// (the editor shows a confirmation; see [`Self::trash_asset`]). Sub-assets
    /// and nothing selected do nothing.
    pub fn request_delete_selected_asset(&mut self) {
        if let Some(AssetItem { id, label: None }) = self.assets.selected.clone() {
            self.requests.delete_asset = Some(id);
        }
    }

    /// Moves an asset file and its `.meta` out of `assets/` into
    /// `<project>/.box/trash/<time>/<path>` (no new dependencies give a
    /// portable OS trash), so nothing is ever deleted outright: moving them
    /// back restores the asset with its id. The open scene's own file is
    /// refused. Returns the new location of the file.
    pub fn trash_asset(&mut self, id: AssetId) -> Result<PathBuf> {
        self.ensure_writable()?;
        let server = self.asset_server.clone().context("no project is open")?;
        let local = self
            .project
            .as_ref()
            .context("no project is open")?
            .local_dir();
        let rel = server
            .path_of(id)
            .with_context(|| format!("asset {id} not found"))?;
        let src = server.assets_dir().join(&rel);
        if self
            .scene
            .path
            .as_deref()
            .is_some_and(|p| same_file(p, &src))
        {
            bail!(
                "assets/{rel} is the open scene; open another scene before moving it to the trash"
            );
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let dst = local.join("trash").join(stamp.to_string()).join(&rel);
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::rename(&src, &dst)
            .with_context(|| format!("moving assets/{rel} to {}", dst.display()))?;
        let meta = box_assets::meta::meta_path(&src);
        if meta.exists() {
            let to = box_assets::meta::meta_path(&dst);
            if let Err(e) = std::fs::rename(&meta, &to) {
                tracing::warn!("could not move {}: {e}", meta.display());
            }
        }
        if self.assets.selected.as_ref().is_some_and(|i| i.id == id) {
            self.select_asset(None);
        }
        server.rescan();
        self.assets.folders_dirty = true;
        self.update_assets();
        tracing::info!("moved assets/{rel} to {}", dst.display());
        Ok(dst)
    }

    /// Copies files dropped from the operating system into the browser's
    /// folder (the originals are never moved or changed; `.meta` files are
    /// skipped, so copies get new ids; folders are copied recursively; a
    /// taken name gets a number). The server picks the copies up at once.
    /// Returns the new paths relative to `assets/`.
    pub fn import_files(&mut self, paths: &[PathBuf]) -> Result<Vec<String>> {
        self.ensure_writable()?;
        let server = self
            .asset_server
            .clone()
            .context("open a project to add files to its assets")?;
        let mut added = Vec::new();
        let folder = self.assets.folder.clone();
        for src in paths {
            match copy_into(server.assets_dir(), &folder, src, &mut added) {
                Ok(()) => {}
                Err(e) => tracing::error!("could not add {}: {e:#}", src.display()),
            }
        }
        server.rescan();
        self.assets.folders_dirty = true;
        for p in &added {
            tracing::info!("added {p}");
        }
        Ok(added)
    }

    // ----- drops -----

    /// Where a drop at `ndc` (−1..1, +Y up) in the viewport lands: on the
    /// surface under the pointer, else on the ground plane (y = 0), else at
    /// the camera's orbit target.
    pub fn viewport_drop_point(&self, ndc: Vec2, aspect: f32) -> Vec3 {
        let ray = crate::picking::camera_ray(&self.camera, ndc, aspect);
        if let Some(hit) = crate::picking::pick(&self.world, &ray) {
            return ray.origin + ray.direction * hit.distance;
        }
        if ray.direction.y < -1e-4 {
            let t = -ray.origin.y / ray.direction.y;
            if t > 0.0 && t < 1e4 {
                return ray.origin + ray.direction * t;
            }
        }
        self.camera.orbit.target
    }

    /// Drops a browser item into the scene at `at`: a glTF (its `#Scene`) or
    /// a `.scene.json` becomes an entity with [`PrefabInstance`], a mesh
    /// sub-asset an entity with [`MeshAsset`] (with the glTF's materials for
    /// it when its scene is loaded). When its meshes are loaded it stands on
    /// `at` by its lowest point ([`Self::land_on_surface`]). One undo step;
    /// the new entity is selected. Returns its uid, or `None` for items that cannot be placed.
    pub fn drop_asset_at(&mut self, item: &AssetItem, at: Vec3) -> Option<Uid> {
        let server = self.asset_server.clone()?;
        let path = server.path_of(item.id)?;
        let kind = FileKind::of(&path);
        let mut r = self.item_ref(item);
        let (name, component, value): (String, &str, Box<dyn PartialReflect>) =
            match (kind, item.label.as_deref()) {
                (FileKind::Scene, None) | (FileKind::Model, None | Some("Scene")) => {
                    if kind == FileKind::Model {
                        r.label = Some("Scene".into());
                    }
                    let stem = split_name(file_name(&path)).0.to_string();
                    let p = PrefabInstance { scene: r };
                    (stem, "PrefabInstance", Box::new(p))
                }
                (FileKind::Model, Some(label)) if label.starts_with("Mesh/") => {
                    let materials = self.gltf_mesh_materials(item.id, label);
                    let m = MeshAsset {
                        mesh: r,
                        materials,
                        cast_shadows: true,
                    };
                    let name = label.trim_start_matches("Mesh/").to_string();
                    (name, "MeshAsset", Box::new(m))
                }
                _ => {
                    tracing::warn!(
                        "{}: only models, mesh sub-assets and scenes can be placed in the scene",
                        self.item_name(item)
                    );
                    return None;
                }
            };
        let uid = Uid::new();
        let name = self.unique_name(&name);
        self.begin_edit(format!("Create {name}"));
        let transform = Transform::from_translation(at);
        let components: [(&str, &dyn PartialReflect); 2] =
            [("Transform", &transform), (component, value.as_ref())];
        let r = self.edits.spawn(
            &mut self.world,
            uid,
            &name,
            Placement::last(None),
            &components,
        );
        if self.report(&format!("place {name}"), r).is_none() {
            self.finish_edit();
            return None;
        }
        self.land_on_surface(uid, at.y);
        self.finish_edit();
        self.changed(true);
        self.select(Some(uid));
        Some(uid)
    }

    /// Lifts or lowers a just-placed entity so the lowest point of its
    /// meshes is at `surface` (a model whose origin is at its centre would
    /// otherwise sink halfway into the ground), as part of the open edit.
    /// Only when its meshes are already loaded: instancing a loaded prefab
    /// and building its renderers here is change-driven and cheap; an
    /// unloaded one keeps its origin on the surface.
    fn land_on_surface(&mut self, uid: Uid, surface: f32) {
        // Prefab contents are instanced by the asset sync and placed by the
        // propagation after it, so run both twice.
        for _ in 0..2 {
            box_engine::sync_scene_meshes(&mut self.world);
            box_core::transform::propagate_transforms(&mut self.world);
        }
        let Some(e) = self.world.entity_by_uid(uid) else {
            return;
        };
        let Some(bounds) = crate::state::entity_bounds(&self.world, e) else {
            return;
        };
        let lift = surface - bounds.min.y;
        if lift.abs() < 1e-4 || !lift.is_finite() {
            return;
        }
        let Some(mut t) = self.world.get::<Transform>(e).map(|t| *t) else {
            return;
        };
        t.translation.y += lift;
        let r = self
            .world
            .field_path("Transform")
            .map_err(box_core::edit::EditError::from)
            .and_then(|path| self.edits.set_field(&mut self.world, uid, &path, &t));
        self.report("land on the surface", r);
    }

    /// The materials the glTF's scene gives the mesh `label`, if its scene
    /// is loaded; empty otherwise.
    fn gltf_mesh_materials(&self, id: AssetId, label: &str) -> Vec<AssetRef> {
        let Some(server) = &self.asset_server else {
            return Vec::new();
        };
        let scene_ref = AssetRef::id(id).with_label("Scene");
        let Some(h) = server.get_handle::<SceneAsset>(&scene_ref) else {
            return Vec::new();
        };
        let Some(scene) = self
            .edit_world()
            .get_resource::<Assets<SceneAsset>>()
            .and_then(|a| a.get(h))
        else {
            return Vec::new();
        };
        scene
            .entities()
            .iter()
            .filter_map(|e| e.pointer("/components/MeshAsset"))
            .find(|m| m.pointer("/mesh/label").and_then(|l| l.as_str()) == Some(label))
            .and_then(|m| m.get("materials"))
            .and_then(|v| serde_json::from_value::<Vec<AssetRef>>(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Gives `uid` a material asset, as one undo step. An entity with
    /// [`MeshAsset`] gets it in material slot 0. A `Primitive` entity gets a
    /// link to it ([`MaterialRef`], added or changed), so later edits to the
    /// file show; its inline material is kept (the link takes precedence;
    /// Unlink copies the asset's values into it). Returns whether something
    /// changed.
    pub fn assign_material(&mut self, uid: Uid, item: &AssetItem) -> bool {
        let Some(e) = self.world.entity_by_uid(uid) else {
            return false;
        };
        if self.item_type(item) != RefType::Material {
            tracing::warn!("{} is not a material", self.item_name(item));
            return false;
        }
        let r = self.item_ref(item);
        self.finish_edit();
        if let Some(m) = self.world.get::<MeshAsset>(e).map(|m| m.materials.clone()) {
            return if m.is_empty() {
                self.set_field(uid, "MeshAsset.materials", &vec![r])
            } else {
                self.set_field(uid, "MeshAsset.materials[0]", &r)
            };
        }
        if !self.world.has::<Primitive>(e) {
            tracing::warn!(
                "{}: drop materials on a mesh (MeshAsset or Primitive)",
                self.display_name(e)
            );
            return false;
        }
        let name = self.item_name(item);
        self.link_material(&[uid], &r, &name)
    }

    /// Links every `Primitive` entity of `uids` to the material `r` (adds or
    /// changes its [`MaterialRef`]), as one undo step named after `name`.
    pub fn link_material(&mut self, uids: &[Uid], r: &AssetRef, name: &str) -> bool {
        let subject = self.subject_of(uids).unwrap_or_default();
        self.begin_edit(format!("Link {name} to {subject}"));
        let mut ok = true;
        for &uid in uids {
            let Some(e) = self.world.entity_by_uid(uid) else {
                continue;
            };
            if self.world.has::<MaterialRef>(e) {
                ok &= self.set_field(uid, "MaterialRef.material", r);
            } else {
                let link = MaterialRef::new(r.clone());
                let res =
                    self.edits
                        .add_component(&mut self.world, uid, "MaterialRef", Some(&link));
                ok &= self.report("add MaterialRef", res).is_some();
            }
        }
        self.finish_edit();
        self.changed(true);
        ok
    }

    /// Unlink (the inspector's Material Ref section): every entity of `uids`
    /// with a [`MaterialRef`] gets the linked asset's values as its inline
    /// material (a `StandardMaterial`, or a `ShaderMaterial` for a shader
    /// material file; the other kind is removed) and loses the link, as one
    /// undo step. Refused (with a warning) while the asset is not loaded.
    pub fn unlink_material(&mut self, uids: &[Uid]) -> bool {
        enum Inline {
            Standard(Box<StandardMaterial>),
            Shader(box_core::shader_material::ShaderMaterial),
        }
        let mut plan: Vec<(Uid, Option<Inline>)> = Vec::new();
        for &uid in uids {
            let Some(e) = self.world.entity_by_uid(uid) else {
                continue;
            };
            let Some(r) = self.world.get::<MaterialRef>(e).map(|m| m.material.clone()) else {
                continue;
            };
            let inline = if r.is_none() {
                None
            } else if let Some(m) = self.loaded_material(&r) {
                Some(Inline::Standard(Box::new(m)))
            } else if let Some(m) = self.loaded_shader_material(&r) {
                Some(Inline::Shader(m))
            } else {
                tracing::warn!("{r}: not loaded yet; try Unlink again");
                return false;
            };
            plan.push((uid, inline));
        }
        if plan.is_empty() {
            return false;
        }
        let ids: Vec<Uid> = plan.iter().map(|(u, _)| *u).collect();
        let subject = self.subject_of(&ids).unwrap_or_default();
        self.begin_edit(format!("Unlink material of {subject}"));
        for (uid, inline) in plan {
            let has = |s: &Self, name: &str| {
                s.world
                    .entity_by_uid(uid)
                    .is_some_and(|e| s.world.component_names(e).contains(&name))
            };
            let set_or_add = |s: &mut Self, name: &str, v: &dyn PartialReflect| {
                if has(s, name) {
                    s.set_field(uid, name, v);
                } else {
                    let r = s.edits.add_component(&mut s.world, uid, name, Some(v));
                    s.report(&format!("add {name}"), r);
                }
            };
            match inline {
                Some(Inline::Standard(m)) => {
                    set_or_add(self, "StandardMaterial", &*m);
                    if has(self, "ShaderMaterial") {
                        let r = self
                            .edits
                            .remove_component(&mut self.world, uid, "ShaderMaterial");
                        self.report("remove ShaderMaterial", r);
                    }
                }
                Some(Inline::Shader(m)) => set_or_add(self, "ShaderMaterial", &m),
                None => {}
            }
            let r = self
                .edits
                .remove_component(&mut self.world, uid, "MaterialRef");
            self.report("remove MaterialRef", r);
        }
        self.finish_edit();
        self.changed(true);
        true
    }

    /// Edit (the inspector's Material Ref section): shows the linked
    /// material asset in the inspector.
    pub fn edit_linked_material(&mut self, r: &AssetRef) {
        let Some(c) = self.asset_server().and_then(|s| s.canonical_ref(r)) else {
            tracing::warn!("{r}: not found in the project's assets");
            return;
        };
        let Some(id) = c.id else {
            return;
        };
        let item = AssetItem { id, label: c.label };
        self.request_file(id);
        self.select_asset(Some(item));
    }

    /// The loaded value of a shader material reference.
    fn loaded_shader_material(
        &self,
        r: &AssetRef,
    ) -> Option<box_core::shader_material::ShaderMaterial> {
        let server = self.asset_server.as_ref()?;
        let id = server.load_file(r).ok()?;
        let r = AssetRef {
            id: Some(id),
            ..r.clone()
        };
        let world = self.edit_world();
        let h = world
            .get_resource::<AssetLookup>()?
            .get::<box_core::shader_material::ShaderMaterial>(&r)?;
        world
            .get_resource::<Assets<box_core::shader_material::ShaderMaterial>>()?
            .get(h)
            .cloned()
    }

    /// The loaded value of a material reference (loading it if needed).
    fn loaded_material(&self, r: &AssetRef) -> Option<StandardMaterial> {
        // A `.material.json` holds a StandardMaterial or (with a "shader"
        // key) a ShaderMaterial: import the file and read whichever type it
        // produced instead of requesting a typed slot.
        let server = self.asset_server.as_ref()?;
        let id = server.load_file(r).ok()?;
        let r = AssetRef {
            id: Some(id),
            ..r.clone()
        };
        let world = self.edit_world();
        let h = world
            .get_resource::<AssetLookup>()?
            .get::<StandardMaterial>(&r)?;
        (server.handle_state(h) == Some(LoadState::Loaded))
            .then(|| {
                world
                    .get_resource::<Assets<StandardMaterial>>()?
                    .get(h)
                    .cloned()
            })
            .flatten()
    }

    /// Repairs stale path hints after asset files moved: every reference in
    /// the edit world (components of saved entities and resources) to one of
    /// `moved` whose path differs from the file's current path is rewritten
    /// to its canonical form (`AssetServer::canonical_ref`), as one undo step
    /// ("Update asset paths"); the scene becomes dirty. Ids are not changed.
    /// Returns how many references were updated.
    pub fn repair_asset_paths(&mut self, moved: &HashSet<AssetId>) -> usize {
        let Some(server) = self.asset_server.clone() else {
            return 0;
        };
        if self.is_playing() || moved.is_empty() {
            return 0;
        }
        let mut sets: Vec<(Option<Uid>, String, AssetRef)> = Vec::new();
        let world = &self.world;
        let mut visit = |uid: Option<Uid>, root: &str, value: &dyn PartialReflect| {
            let mut refs = Vec::new();
            collect_refs(value, root.to_string(), &mut refs);
            for (path, r) in refs {
                if !r.id.is_some_and(|id| moved.contains(&id)) {
                    continue;
                }
                if let Some(c) = server.canonical_ref(&r)
                    && c != r
                {
                    sets.push((uid, path, c));
                }
            }
        };
        // Generated entities (prefab contents, derived parts) are runtime
        // data (not editable, never saved); they follow their source.
        type Saved<'a> = box_core::prefab::NotGenerated<(Entity, &'a Uid)>;
        for (e, uid) in world.query::<Saved>().iter() {
            for name in world.component_names(e) {
                if let Ok(c) = world.get_reflect(e, name) {
                    visit(Some(*uid), name, c.as_partial_reflect());
                }
            }
        }
        for name in world.resource_names() {
            if let Ok(r) = world.get_resource_reflect(name) {
                visit(None, name, r.as_partial_reflect());
            }
        }
        if sets.is_empty() {
            return 0;
        }
        let n = sets.len();
        self.begin_edit("Update asset paths");
        for (uid, path, r) in sets {
            match uid {
                Some(uid) => {
                    self.set_field(uid, &path, &r);
                }
                None => {
                    self.set_resource_field(&path, &r);
                }
            }
        }
        self.finish_edit();
        self.changed(false);
        tracing::info!("updated {n} asset path hint(s) in the scene");
        n
    }

    // ----- shader materials -----

    /// The shader of a `ShaderMaterial` and its parsed `@material` header:
    /// from the `ShaderSource` the world has loaded for `r` (through
    /// `AssetLookup`; the world shown first, then the edit world), else from
    /// the `.wgsl` file in the project read directly. Parsed again only when
    /// the source changes (a loaded source's version, or the file's size and
    /// time, checked at most once a second). `None` for an empty reference.
    pub fn shader_info(&mut self, r: &AssetRef) -> Option<ShaderInfo> {
        if r.is_none() {
            return None;
        }
        let canonical = self.asset_server.as_ref().and_then(|s| s.canonical_ref(r));
        let key = format!("{:?}|{:?}|{:?}", r.id, r.label, r.path);
        let refs: Vec<&AssetRef> = std::iter::once(r).chain(canonical.as_ref()).collect();
        // A loaded ShaderSource.
        let mut loaded = None;
        for world in [&self.world, self.edit_world()] {
            let (Some(lookup), Some(sources)) = (
                world.get_resource::<AssetLookup>(),
                world.get_resource::<Assets<ShaderSource>>(),
            ) else {
                continue;
            };
            let Some(h) = refs.iter().find_map(|r| lookup.get::<ShaderSource>(r)) else {
                continue;
            };
            let (Some(src), Some(version)) = (sources.get(h), sources.version(h)) else {
                continue;
            };
            let stamp = HeaderStamp::Asset(h.id(), version);
            if let Some(e) = self.assets.headers.get(&key)
                && e.stamp == stamp
            {
                return Some(e.info.clone());
            }
            let info = ShaderInfo {
                handle: Some(h),
                header: parse_material_header(&src.text).map_err(|e| e.to_string()),
            };
            loaded = Some((stamp, info));
            break;
        }
        if let Some((stamp, info)) = loaded {
            self.cache_header(key, stamp, &info);
            return Some(info);
        }
        // The file itself.
        let path = canonical
            .as_ref()
            .and_then(|c| c.path.clone())
            .or_else(|| r.path.clone())
            .and_then(|p| box_assets::normalize_rel(&p));
        let (Some(server), Some(path)) = (&self.asset_server, path) else {
            return Some(ShaderInfo {
                handle: None,
                header: Err(format!("shader {r} not found")),
            });
        };
        let abs = server.assets_dir().join(&path);
        if let Some(e) = self.assets.headers.get(&key)
            && matches!(e.stamp, HeaderStamp::File(..))
            && e.checked.elapsed() < HEADER_FILE_RECHECK
        {
            return Some(e.info.clone());
        }
        let meta = std::fs::metadata(&abs).ok();
        let stamp = HeaderStamp::File(
            meta.as_ref().map_or(0, |m| m.len()),
            meta.as_ref().and_then(|m| m.modified().ok()),
        );
        if let Some(e) = self.assets.headers.get_mut(&key)
            && e.stamp == stamp
        {
            e.checked = std::time::Instant::now();
            return Some(e.info.clone());
        }
        let header = match std::fs::read_to_string(&abs) {
            Ok(text) => parse_material_header(&text).map_err(|e| e.to_string()),
            Err(e) => Err(format!("{path}: {e}")),
        };
        let info = ShaderInfo {
            handle: None,
            header,
        };
        self.cache_header(key, stamp, &info);
        Some(info)
    }

    fn cache_header(&mut self, key: String, stamp: HeaderStamp, info: &ShaderInfo) {
        self.assets.headers.insert(
            key,
            HeaderEntry {
                stamp,
                checked: std::time::Instant::now(),
                info: info.clone(),
            },
        );
    }

    /// The renderer's compile error for a shader the inspector showed, as of
    /// the last rendered frame (asks for it again on the next one).
    pub fn shader_error(&mut self, shader: Handle<ShaderSource>) -> Option<String> {
        if !self.assets.shader_queries.contains(&shader) {
            self.assets.shader_queries.push(shader);
        }
        self.assets.shader_errors.get(&shader.id()).cloned()
    }

    // ----- import settings and materials -----

    /// Writes the inspected file's edited import settings to its `.meta`,
    /// which re-imports it.
    pub fn apply_import_settings(&mut self) -> Result<()> {
        self.ensure_writable()?;
        let server = self.asset_server.clone().context("no project is open")?;
        let Some(insp) = self.assets.inspect.as_mut() else {
            bail!("no asset is inspected");
        };
        let Some(s) = insp.settings.as_mut() else {
            bail!("the asset has no import settings");
        };
        let json = match &s.edit {
            SettingsEdit::Texture(t) => serde_json::to_value(t)?,
            SettingsEdit::Model(m) => serde_json::to_value(m)?,
            SettingsEdit::Other(_) => bail!("these settings are read-only"),
        };
        match server.set_settings(insp.item.id, json) {
            Ok(()) => {
                s.saved = s.edit.clone();
                s.error = None;
                Ok(())
            }
            Err(e) => {
                s.error = Some(e.to_string());
                Err(e.into())
            }
        }
    }

    /// Drops unapplied import settings edits.
    pub fn revert_import_settings(&mut self) {
        if let Some(s) = self
            .assets
            .inspect
            .as_mut()
            .and_then(|i| i.settings.as_mut())
        {
            s.edit = s.saved.clone();
            s.error = None;
        }
    }

    /// Changes the inspected material (not written yet: see
    /// [`Self::flush_material_edit`]). `now` is the UI time in seconds.
    pub fn edit_material(&mut self, now: f64, f: impl FnOnce(&mut StandardMaterial)) {
        self.edit_material_labelled(now, None, f);
    }

    /// [`Self::edit_material`] naming the change (the field's label, such as
    /// `Roughness`) for the file's undo step.
    pub fn edit_material_labelled(
        &mut self,
        now: f64,
        label: Option<String>,
        f: impl FnOnce(&mut StandardMaterial),
    ) {
        if let Some(m) = self
            .assets
            .inspect
            .as_mut()
            .and_then(|i| i.material.as_mut())
            .filter(|m| !m.read_only)
        {
            let before = m.value.clone();
            f(&mut m.value);
            if m.value != before {
                // Several fields in one write: a general label.
                m.pending_label = match (m.pending_since, &m.pending_label, label) {
                    (None, _, l) => l,
                    (Some(_), Some(old), Some(l)) if *old == l => Some(l),
                    _ => Some("Edit".into()),
                };
                m.pending_since = Some(now);
            }
        }
    }

    /// Writes pending material edits to the `.material.json` (atomically;
    /// the asset reloads) when `idle` (no drag or typing) or
    /// [`MATERIAL_WRITE_DELAY_S`] after the last change. Each write is one
    /// step of the file's undo history (`asset_undo.rs`), not a scene undo
    /// step.
    pub fn flush_material_edit(&mut self, now: f64, idle: bool) {
        let Some(server) = self.asset_server.clone() else {
            return;
        };
        let registry = self.world.registry().clone();
        let Some(insp) = self.assets.inspect.as_mut() else {
            return;
        };
        let id = insp.item.id;
        let Some(m) = insp.material.as_mut() else {
            return;
        };
        let Some(since) = m.pending_since else {
            return;
        };
        if !idle && now - since < MATERIAL_WRITE_DELAY_S {
            return;
        }
        m.pending_since = None;
        let label = m.pending_label.take().unwrap_or_else(|| "Edit".into());
        let bytes = match material_file_bytes(&registry, &m.value, &m.original) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::error!("material: {e}");
                return;
            }
        };
        // What the file held, for the undo step.
        let before = server
            .path_of(id)
            .and_then(|p| std::fs::read(server.assets_dir().join(p)).ok())
            .or_else(|| m.last_written.clone())
            .unwrap_or_else(|| m.original.clone());
        match server.write(id, &bytes) {
            Ok(()) => {
                m.written = true;
                m.last_written = Some(bytes.clone());
                if before != bytes {
                    self.assets.histories.record(
                        id,
                        crate::asset_undo::FileStep {
                            label,
                            before,
                            after: bytes,
                        },
                    );
                }
            }
            Err(e) => tracing::error!("material: could not write: {e}"),
        }
    }

    /// Puts the inspected material file back as it was when it was opened.
    pub fn revert_material(&mut self) {
        let Some(server) = self.asset_server.clone() else {
            return;
        };
        let Some(insp) = self.assets.inspect.as_mut() else {
            return;
        };
        let id = insp.item.id;
        let Some(m) = insp.material.as_mut().filter(|m| !m.read_only) else {
            return;
        };
        m.pending_since = None;
        m.pending_label = None;
        if !m.written {
            // Nothing reached the file: just reload the shown values.
        } else {
            let before = server
                .path_of(id)
                .and_then(|p| std::fs::read(server.assets_dir().join(p)).ok());
            if let Err(e) = server.write(id, &m.original) {
                tracing::error!("material: could not revert: {e}");
                return;
            }
            // Revert is a step too, so it can be undone.
            if let Some(before) = before.filter(|b| *b != m.original) {
                self.assets.histories.record(
                    id,
                    crate::asset_undo::FileStep {
                        label: "Revert".into(),
                        before,
                        after: m.original.clone(),
                    },
                );
            }
        }
        m.written = false;
        m.last_written = None;
        let registry = self.world.registry().clone();
        if let Some(v) = material_from_file(&registry, &m.original) {
            m.value = v;
        }
    }
}

/// Every `AssetRef` inside a reflected value, with its field path below
/// `path` (`StandardMaterial.base_color_texture`, `MeshAsset.materials[1]`).
pub fn collect_refs(value: &dyn PartialReflect, path: String, out: &mut Vec<(String, AssetRef)>) {
    if let Some(r) = value.try_downcast_ref::<AssetRef>() {
        out.push((path, r.clone()));
        return;
    }
    match value.reflect_ref() {
        ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                if let (Some(n), Some(f)) = (s.name_at(i), s.field_at(i)) {
                    collect_refs(f, format!("{path}.{n}"), out);
                }
            }
        }
        ReflectRef::TupleStruct(s) => {
            for i in 0..s.field_len() {
                if let Some(f) = s.field(i) {
                    collect_refs(f, format!("{path}.{i}"), out);
                }
            }
        }
        ReflectRef::Tuple(t) => {
            for i in 0..t.field_len() {
                if let Some(f) = t.field(i) {
                    collect_refs(f, format!("{path}.{i}"), out);
                }
            }
        }
        ReflectRef::List(l) => {
            for i in 0..l.len() {
                if let Some(f) = l.get(i) {
                    collect_refs(f, format!("{path}[{i}]"), out);
                }
            }
        }
        ReflectRef::Array(a) => {
            for i in 0..a.len() {
                if let Some(f) = a.get(i) {
                    collect_refs(f, format!("{path}[{i}]"), out);
                }
            }
        }
        ReflectRef::Enum(e) => {
            for i in 0..e.field_len() {
                if let Some(f) = e.field_at(i) {
                    let n = e.name_at(i).map_or_else(|| i.to_string(), str::to_string);
                    collect_refs(f, format!("{path}.{n}"), out);
                }
            }
        }
        _ => {}
    }
}

/// A `.material.json` file's material: upgraded through `StandardMaterial`'s
/// migration steps, ignoring fields the type does not have.
pub(crate) fn material_from_file(
    registry: &box_core::reflect::Registry,
    bytes: &[u8],
) -> Option<StandardMaterial> {
    let json = serde_json::from_slice::<serde_json::Value>(bytes).ok()?;
    let ty = std::any::TypeId::of::<StandardMaterial>();
    let value = box_core::migrate::read_value_file(registry, "StandardMaterial", ty, &json)
        .ok()?
        .value;
    let v = registry.value_from_json(ty, &value).ok()?;
    <StandardMaterial as box_core::reflect::bevy_reflect::FromReflect>::from_reflect(v.as_ref())
}

/// The bytes of a `.material.json` holding `value`: with `"$version"` when
/// `StandardMaterial` is versioned, and the fields of `original` (the file
/// as it was) that the type does not have kept after the known ones.
fn material_file_bytes(
    registry: &box_core::reflect::Registry,
    value: &StandardMaterial,
    original: &[u8],
) -> Result<Vec<u8>, box_core::reflect::ReflectError> {
    let json = registry.value_to_json(value)?;
    let keep = serde_json::from_slice::<serde_json::Value>(original)
        .map(|file| {
            box_core::migrate::unknown_fields_of(
                registry,
                std::any::TypeId::of::<StandardMaterial>(),
                &file,
            )
        })
        .unwrap_or_default();
    Ok(pretty(&box_core::migrate::value_file_json(
        registry,
        "StandardMaterial",
        json,
        &keep,
    )))
}

/// Pretty JSON with a trailing newline.
fn pretty(json: &serde_json::Value) -> Vec<u8> {
    let mut b = serde_json::to_vec_pretty(json).unwrap_or_default();
    b.push(b'\n');
    b
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || canon(a) == canon(b) || {
        // `b` may no longer exist (it was just moved): compare its folder.
        match (a.parent(), b.parent(), a.file_name(), b.file_name()) {
            (Some(pa), Some(pb), Some(na), Some(nb)) => na == nb && canon(pa) == canon(pb),
            _ => false,
        }
    }
}

/// `folder/stem suffix`, or `folder/stem 2 suffix`, ... whichever is free.
fn unique_path(root: &Path, folder: &str, stem: &str, suffix: &str) -> String {
    let free = |rel: &str| {
        let abs = root.join(rel);
        !abs.exists() && !box_assets::meta::meta_path(&abs).exists()
    };
    let first = join(folder, &format!("{stem}{suffix}"));
    if free(&first) {
        return first;
    }
    (2..)
        .map(|i| join(folder, &format!("{stem} {i}{suffix}")))
        .find(|p| free(p))
        .unwrap_or(first)
}

/// Copies `src` (a file or a folder) into `root/folder`, appending the new
/// paths to `added`.
fn copy_into(root: &Path, folder: &str, src: &Path, added: &mut Vec<String>) -> Result<()> {
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .context("no file name")?;
    if name.starts_with('.') || name.ends_with(box_assets::meta::META_SUFFIX) {
        return Ok(());
    }
    let src_canon =
        std::fs::canonicalize(src).with_context(|| format!("reading {}", src.display()))?;
    if src_canon.starts_with(root) && src.is_dir() {
        bail!("{} is already inside the assets folder", src.display());
    }
    let (stem, suffix) = split_name(&name);
    let rel = unique_path(root, folder, stem, suffix);
    let dst = root.join(&rel);
    if src.is_dir() {
        std::fs::create_dir_all(&dst).with_context(|| format!("creating {}", dst.display()))?;
        let mut entries: Vec<_> = std::fs::read_dir(src)?
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for e in entries {
            copy_into(root, &rel, &e, added)?;
        }
    } else {
        if let Some(dir) = dst.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(src, &dst)
            .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
        added.push(rel);
    }
    Ok(())
}

/// Folders below `root/rel` (hidden ones skipped), added to `out`.
fn walk_folders(root: &Path, rel: &str, out: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let child = join(rel, &name);
        walk_folders(root, &child, out);
        out.insert(child);
    }
}

/// Folders directly inside `folder`, from a sorted folder set.
pub(crate) fn subfolders<'a>(folders: &'a BTreeSet<String>, folder: &str) -> Vec<&'a str> {
    folders
        .iter()
        .filter(|f| parent_folder(f) == folder && !f.is_empty())
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_files_keep_unknown_fields_through_an_edit() {
        let registry = box_core::reflect::Registry::with_core_types();
        let original = br#"{
  "base_color": [0.5, 0.5, 0.5, 1.0],
  "roughness": 0.3,
  "subsurface": { "radius": 0.2 },
  "note": "kept"
}"#;
        let mut m = material_from_file(&registry, original).expect("reads despite unknown fields");
        assert_eq!(m.roughness, 0.3);
        m.roughness = 0.9;
        let bytes = material_file_bytes(&registry, &m, original).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["roughness"], 0.9_f32 as f64);
        assert_eq!(json["subsurface"], serde_json::json!({ "radius": 0.2 }));
        assert_eq!(json["note"], "kept");
        // After the known fields, in file order.
        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(&keys[keys.len() - 2..], ["subsurface", "note"]);
        assert_eq!(material_from_file(&registry, &bytes).unwrap(), m);
    }

    #[test]
    fn names_split_at_known_suffixes() {
        assert_eq!(split_name("tower.glb"), ("tower", ".glb"));
        assert_eq!(
            split_name("stone.material.json"),
            ("stone", ".material.json")
        );
        assert_eq!(split_name("main.scene.json"), ("main", ".scene.json"));
        assert_eq!(split_name("a.b.png"), ("a.b", ".png"));
        assert_eq!(split_name("README"), ("README", ""));
        assert_eq!(FileKind::of("models/Tower.GLB"), FileKind::Model);
        assert_eq!(FileKind::of("x/stone.material.json"), FileKind::Material);
        assert_eq!(parent_folder("a/b/c.png"), "a/b");
        assert_eq!(parent_folder("c.png"), "");
    }

    #[test]
    fn field_paths_hint_the_reference_type() {
        assert_eq!(
            RefType::of_field("StandardMaterial.base_color_texture"),
            RefType::Image
        );
        assert_eq!(RefType::of_field("MeshAsset.mesh"), RefType::Mesh);
        assert_eq!(
            RefType::of_field("MeshAsset.materials[2]"),
            RefType::Material
        );
        assert_eq!(RefType::of_field("PrefabInstance.scene"), RefType::Scene);
        assert_eq!(RefType::of_field("ShaderMaterial.shader"), RefType::Shader);
        assert_eq!(RefType::of_field("Thing.target"), RefType::Any);
        assert!(RefType::Any.accepts(RefType::Mesh));
        assert!(!RefType::Image.accepts(RefType::Mesh));
    }

    #[test]
    fn refs_are_collected_with_their_paths() {
        let m = MeshAsset {
            mesh: AssetRef::path("a.glb#Mesh/A"),
            materials: vec![AssetRef::NONE, AssetRef::path("m.material.json")],
            cast_shadows: true,
        };
        let mut out = Vec::new();
        collect_refs(&m, "MeshAsset".into(), &mut out);
        let paths: Vec<&str> = out.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                "MeshAsset.mesh",
                "MeshAsset.materials[0]",
                "MeshAsset.materials[1]"
            ]
        );
    }
}
