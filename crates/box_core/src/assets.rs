//! Typed asset stores with change tracking.
//!
//! Handles are plain IDs (copyable, not reference counted). The asset server
//! (`box_assets`) owns loaded assets' lifetimes; [`AssetLookup`] maps the
//! persistent [`AssetRef`]s that saved data holds to loaded handles, so
//! consumers such as the renderer resolve references without depending on
//! the asset server.

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::asset_ref::{AssetId, AssetRef};

/// Asset IDs are unique across every store in the process, so consumers that
/// cache by ID (the renderer) never confuse assets from different worlds.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A typed ID into an [`Assets`] store.
pub struct Handle<T> {
    id: u64,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub fn id(&self) -> u64 {
        self.id
    }

    fn from_raw(id: u64) -> Self {
        Self {
            id,
            _marker: PhantomData,
        }
    }
}

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T> Eq for Handle<T> {}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle<{}>({})", std::any::type_name::<T>(), self.id)
    }
}

#[derive(Clone)]
struct Entry<T> {
    value: T,
    version: u64,
}

/// A store of assets of one type. Every mutable access bumps the entry's
/// version, which consumers (such as the renderer) use to detect changes.
pub struct Assets<T> {
    entries: HashMap<u64, Entry<T>>,
    removed: Vec<u64>,
    /// Bumped by every add, set, mutable borrow and removal.
    generation: u64,
}

impl<T> Default for Assets<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            removed: Vec::new(),
            generation: 0,
        }
    }
}

impl<T> Assets<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, value: T) -> Handle<T> {
        let handle = self.reserve();
        self.generation += 1;
        self.entries.insert(handle.id, Entry { value, version: 1 });
        handle
    }

    /// A new handle with no value yet (an asset that is still loading).
    /// [`get`](Self::get) returns `None` until [`set`](Self::set) fills it.
    pub fn reserve(&mut self) -> Handle<T> {
        Handle::from_raw(next_id())
    }

    /// Stores `value` under `handle`, replacing any previous value and
    /// bumping the version (a finished load or a hot reload: handles stay
    /// valid and consumers see the change).
    pub fn set(&mut self, handle: Handle<T>, value: T) {
        self.generation += 1;
        match self.entries.get_mut(&handle.id) {
            Some(e) => {
                e.value = value;
                e.version += 1;
            }
            None => {
                self.entries.insert(handle.id, Entry { value, version: 1 });
            }
        }
    }

    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        self.entries.get(&handle.id).map(|e| &e.value)
    }

    /// Mutable access; marks the asset as changed.
    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        let e = self.entries.get_mut(&handle.id)?;
        self.generation += 1;
        e.version += 1;
        Some(&mut e.value)
    }

    pub fn version(&self, handle: Handle<T>) -> Option<u64> {
        self.entries.get(&handle.id).map(|e| e.version)
    }

    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        let e = self.entries.remove(&handle.id)?;
        self.generation += 1;
        self.removed.push(handle.id);
        Some(e.value)
    }

    /// IDs removed since the last call.
    pub fn drain_removed(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.removed)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate as `(id, version, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (u64, u64, &T)> {
        self.entries
            .iter()
            .map(|(id, e)| (*id, e.version, &e.value))
    }

    pub fn get_by_id(&self, id: u64) -> Option<&T> {
        self.entries.get(&id).map(|e| &e.value)
    }

    /// Changes whenever an entry is added, replaced ([`set`](Self::set)),
    /// mutably borrowed or removed, so a consumer can skip scanning an
    /// unchanged store with one comparison.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl<T: Clone> Assets<T> {
    /// Copies entry `id` of `from` (value and version) into this store, so a
    /// consumer caching by `(id, version)` (the renderer) treats it as the
    /// same asset. Returns false when `from` has no such entry. Used to give
    /// a Play world the edit world's loaded assets without re-importing or
    /// re-uploading them.
    pub fn copy_entry(&mut self, from: &Assets<T>, id: u64) -> bool {
        let Some(e) = from.entries.get(&id) else {
            return false;
        };
        self.generation += 1;
        self.entries.insert(id, e.clone());
        true
    }
}

/// Maps persistent asset references (id or path, plus sub-asset label) to
/// loaded handles. A world resource maintained by the asset server; readers
/// resolve [`AssetRef`]s with [`get`](Self::get). A reference to an asset of
/// another type resolves to `None`.
#[derive(Clone, Default)]
pub struct AssetLookup {
    by_id: HashMap<(AssetId, Option<String>), (TypeId, u64)>,
    by_path: HashMap<(String, Option<String>), (TypeId, u64)>,
    generation: u64,
}

impl AssetLookup {
    /// Records that the asset `id` (sub-asset `label`), found at `path`
    /// (relative to `assets/`, `/` separators), is loaded as `handle`.
    pub fn insert<T: 'static>(
        &mut self,
        id: AssetId,
        label: Option<&str>,
        path: Option<&str>,
        handle: Handle<T>,
    ) {
        self.generation += 1;
        let value = (TypeId::of::<T>(), handle.id);
        let label = label.map(str::to_string);
        if let Some(path) = path {
            self.by_path
                .insert((path.to_string(), label.clone()), value);
        }
        self.by_id.insert((id, label), value);
    }

    /// Forgets the paths recorded for `id` (a rename) and records `new_path`.
    pub fn rename(&mut self, id: AssetId, new_path: &str) {
        self.generation += 1;
        let moved: Vec<_> = self
            .by_id
            .iter()
            .filter(|((i, _), _)| *i == id)
            .map(|((_, l), v)| (l.clone(), *v))
            .collect();
        self.by_path
            .retain(|_, v| !moved.iter().any(|(_, m)| m == v));
        for (label, value) in moved {
            self.by_path.insert((new_path.to_string(), label), value);
        }
    }

    /// Forgets every entry for `id`.
    pub fn remove(&mut self, id: AssetId) {
        self.generation += 1;
        let gone: Vec<_> = self
            .by_id
            .iter()
            .filter(|((i, _), _)| *i == id)
            .map(|(_, v)| *v)
            .collect();
        self.by_id.retain(|(i, _), _| *i != id);
        self.by_path.retain(|_, v| !gone.contains(v));
    }

    /// The loaded handle for `r`: by id when it has one, else by path.
    pub fn get<T: 'static>(&self, r: &AssetRef) -> Option<Handle<T>> {
        let label = r.label.clone();
        let (ty, raw) = match (&r.id, &r.path) {
            (Some(id), _) => *self.by_id.get(&(*id, label))?,
            (None, Some(path)) => *self.by_path.get(&(path.clone(), label))?,
            (None, None) => return None,
        };
        (ty == TypeId::of::<T>()).then(|| Handle::from_raw(raw))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Changes whenever an entry is inserted, renamed or removed, so readers
    /// that cache resolved handles (the renderer's materials) re-resolve only
    /// then instead of every frame.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Ids of every handle of type `T` the lookup maps to.
    pub fn handle_ids<T: 'static>(&self) -> Vec<u64> {
        let ty = TypeId::of::<T>();
        self.by_id
            .values()
            .filter(|(t, _)| *t == ty)
            .map(|(_, id)| *id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_then_set_keeps_the_handle_and_bumps_the_version() {
        let mut a = Assets::<u32>::new();
        let h = a.reserve();
        assert!(a.get(h).is_none());
        a.set(h, 5);
        assert_eq!((a.get(h), a.version(h)), (Some(&5), Some(1)));
        a.set(h, 6);
        assert_eq!((a.get(h), a.version(h)), (Some(&6), Some(2)));
    }

    #[test]
    fn lookup_resolves_by_id_or_path_and_checks_the_type() {
        let mut meshes = Assets::<u32>::new();
        let roof = meshes.add(1);
        let id = AssetId::from_u128(1);
        let mut lookup = AssetLookup::default();
        assert_eq!(lookup.generation(), 0);
        lookup.insert(id, Some("Mesh/Roof"), Some("models/tower.glb"), roof);
        assert_eq!(lookup.generation(), 1);
        let by_id = AssetRef::id(id).with_label("Mesh/Roof");
        let by_path = AssetRef::path("models/tower.glb#Mesh/Roof");
        assert_eq!(lookup.get::<u32>(&by_id), Some(roof));
        assert_eq!(lookup.get::<u32>(&by_path), Some(roof));
        assert_eq!(lookup.get::<u8>(&by_id), None, "wrong type");
        assert_eq!(
            lookup.get::<u32>(&AssetRef::id(id)),
            None,
            "main asset not loaded"
        );
        assert_eq!(lookup.get::<u32>(&AssetRef::NONE), None);

        lookup.rename(id, "props/tower.glb");
        assert_eq!(lookup.generation(), 2);
        assert_eq!(lookup.get::<u32>(&by_path), None);
        assert_eq!(
            lookup.get::<u32>(&AssetRef::path("props/tower.glb#Mesh/Roof")),
            Some(roof)
        );
        assert_eq!(lookup.get::<u32>(&by_id), Some(roof));
        lookup.remove(id);
        assert_eq!(lookup.generation(), 3);
        assert!(lookup.is_empty());
        assert_eq!(lookup.get::<u32>(&by_id), None);
    }

    #[test]
    fn versions_track_mutation_and_removal() {
        let mut a = Assets::new();
        let h = a.add(1u32);
        assert_eq!(a.version(h), Some(1));
        *a.get_mut(h).unwrap() = 2;
        assert_eq!(a.version(h), Some(2));
        assert_eq!(a.get(h), Some(&2));
        assert_eq!(a.remove(h), Some(2));
        assert_eq!(a.drain_removed(), vec![h.id()]);
        assert!(a.get(h).is_none());
    }
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn ids_are_unique_across_stores() {
        let mut a = Assets::new();
        let mut b = Assets::new();
        assert_ne!(a.add(1u8).id(), b.add(1u8).id());
    }
}
