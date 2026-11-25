//! The generator runtime: stamp, throttled flush, cache keys and owned
//! meshes (docs/PROCEDURAL.md section 5, step 1).
//!
//! [`flush`] runs once per frame (`box_engine::sync_scene_meshes` calls it,
//! so apps, the editor and tools all get it):
//!
//! 1. **Stamp** (every call, constant work per object): each generator
//!    component is compared with a cached copy (`PartialEq`), as is its
//!    `Procedural::seed`; each watched input is compared with its cached
//!    copy (components) or handle and version (assets). A difference puts
//!    the entity in the dirty queue once. No hashing, no allocation while
//!    idle.
//! 2. **Flush** (throttled): when the queue is not empty and at least
//!    [`ProcSettings::throttle`] has passed since the last flush (or the
//!    last flush ran out of budget), dirty entities are taken in the order
//!    they were stamped. Each computes its **cache key** = hash(generator
//!    type, `VERSION`, canonical reflected parameters, seed, input keys);
//!    an unchanged key is skipped, a changed one regenerates synchronously.
//!    After [`ProcSettings::budget`] the rest wait for the next frame's
//!    flush; at least one entity is processed per flush, so nothing starves.
//!
//! The first edit after a pause therefore regenerates in the same frame;
//! a continuous drag regenerates at most once per throttle interval, and
//! because a stamped entity stays queued until a flush takes it, the last
//! value always lands (at most one throttle interval after the drag stops).
//! A generator that just appeared (spawned, loaded, added, pasted) is not
//! throttled: a flush runs in that frame (within the budget), so new
//! objects show at once, as primitives always did.
//!
//! **Output.** The mesh goes into an `Assets<MeshData>` entry the runtime
//! owns, updated in place (`Assets::set`: same handle, new version, so the
//! renderer re-uploads it). An output equal to the current mesh (an edit of
//! a parameter that does not shape it) is not written, so nothing is
//! uploaded. A mesh with UVs and no tangents gets tangents from UV set 0
//! (`MeshData::generate_tangents`) before it is stored, so normal maps work
//! on every generated mesh; the generator's own output, and its golden
//! hash, are unchanged. Warnings are stored on [`ProcState`] and logged once each
//! time they change. Removing the generator (or despawning the entity)
//! frees the mesh.
//!
//! **Built-in types.** `box_core`'s `Primitive` is a generator
//! ([`crate::builtin`]) and always runs, even in a world whose registry
//! lacks its generator type data (`World::new()`).

use std::any::TypeId;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use box_core::app::{App, Plugin};
use box_core::assets::{Assets, Handle};
use box_core::identity::Name;
use box_core::mesh::MeshData;
use box_core::mesh_asset::GeneratedMesh;
use box_core::primitive::Primitive;
use box_core::reflect::Registry;
use box_core::world::{Entity, World};

use crate::generator::{ColliderHint, GenContext, Generator, GeneratorRegistry, ReflectGenerator};
use crate::hash::{StableHasher, reflect_hash};
use crate::inputs::{InputId, Inputs, Probe, ResolvedInput};
use crate::procedural::{Procedural, register_proc_types};

/// Tuning of the regeneration pipeline. A world resource; [`ProcPlugin`]
/// inserts the defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcSettings {
    /// Minimum time between two flushes that take newly stamped objects
    /// (default 30 ms). The first change after a pause is regenerated at
    /// once; continuous changes (a slider drag) at most once per interval.
    pub throttle: Duration,
    /// Generation time per flush (default 6 ms). Objects left over wait
    /// for the next frame; one object is always processed.
    pub budget: Duration,
    /// Where "now" comes from for the throttle.
    pub clock: ProcClock,
}

impl Default for ProcSettings {
    fn default() -> Self {
        Self {
            throttle: Duration::from_millis(30),
            budget: Duration::from_millis(6),
            clock: ProcClock::Real,
        }
    }
}

/// The throttle's clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ProcClock {
    /// Wall-clock time (the editor and interactive apps).
    #[default]
    Real,
    /// A time set by the caller: deterministic runs, tests and benchmarks
    /// advance it themselves.
    Manual(Duration),
}

/// Runtime state of a generator entity. Never saved (not registered).
#[derive(Debug)]
pub struct ProcState {
    generator: &'static str,
    type_id: TypeId,
    queued: bool,
    key: Option<u64>,
    mesh: Option<Handle<MeshData>>,
    slot_count: usize,
    warnings: Vec<String>,
    collider: ColliderHint,
    last_generate: Option<Duration>,
    generations: u64,
    inputs: Vec<InputId>,
}

impl ProcState {
    fn new(generator: &'static str, type_id: TypeId) -> Self {
        Self {
            generator,
            type_id,
            queued: false,
            key: None,
            mesh: None,
            slot_count: 1,
            warnings: Vec::new(),
            collider: ColliderHint::default(),
            last_generate: None,
            generations: 0,
            inputs: Vec::new(),
        }
    }

    /// The generator component's short type name.
    pub fn generator(&self) -> &'static str {
        self.generator
    }

    /// Waiting in the dirty queue for a flush.
    pub fn is_dirty(&self) -> bool {
        self.queued
    }

    /// The cache key of the current output (`None` before the first flush).
    pub fn key(&self) -> Option<u64> {
        self.key
    }

    /// The generated mesh, owned by the runtime (`None`: nothing to draw).
    pub fn mesh(&self) -> Option<Handle<MeshData>> {
        self.mesh
    }

    /// Material slots the renderer needs: the generator's declared slots,
    /// or more if the mesh's groups use higher slots (at least 1).
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// The warnings of the last generation (missing inputs first).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The generator's collider hint (not consumed by physics in M5).
    pub fn collider_hint(&self) -> &ColliderHint {
        &self.collider
    }

    /// How long the last `generate` call took.
    pub fn last_generate(&self) -> Option<Duration> {
        self.last_generate
    }

    /// How many times the entity was regenerated.
    pub fn generations(&self) -> u64 {
        self.generations
    }

    /// The inputs declared at the last flush.
    pub fn inputs(&self) -> &[InputId] {
        &self.inputs
    }
}

/// The last-seen copy of a generator's parameters and seed, per type.
struct Cached<T> {
    params: T,
    seed: u64,
}

/// What one stamp pass found.
#[derive(Default)]
pub(crate) struct Stamps {
    dirty: Vec<Entity>,
    /// A generator appeared (spawned, loaded, added): not throttled.
    fresh: bool,
}

/// The stamp pass of generator `T` (see the module docs).
pub(crate) fn stamp<T: Generator>(world: &mut World, stamps: &mut Stamps) {
    let me = TypeId::of::<T>();
    let mut new = Vec::new();
    for (e, g, p, cached, state) in world.query_mut::<(
        Entity,
        &T,
        Option<&Procedural>,
        Option<&mut Cached<T>>,
        Option<&ProcState>,
    )>() {
        let seed = p.map_or(0, |p| p.seed);
        match cached {
            None => new.push((e, g.clone(), seed)),
            Some(c) => {
                if c.seed != seed || c.params != *g {
                    c.params = g.clone();
                    c.seed = seed;
                    if state.is_some_and(|s| s.type_id == me) {
                        stamps.dirty.push(e);
                    }
                }
            }
        }
    }
    for (e, params, seed) in new {
        let other = world.get::<ProcState>(e).map(|s| s.generator);
        let _ = world.insert_one(e, Cached { params, seed });
        match other {
            None => {
                let _ = world.insert(
                    e,
                    (
                        ProcState::new(T::short_type_path(), me),
                        GeneratedMesh::default(),
                    ),
                );
                stamps.dirty.push(e);
                stamps.fresh = true;
            }
            Some(other) => tracing::warn!(
                "{}: has two generators, `{other}` and `{}`; only `{other}` is used",
                describe(world, e),
                T::short_type_path()
            ),
        }
    }
}

/// Entities that lost their `T` since the last pass (their cached copy is
/// dropped).
pub(crate) fn lost<T: Generator>(world: &mut World) -> Vec<Entity> {
    let gone: Vec<Entity> = world
        .query::<Entity>()
        .with::<&Cached<T>>()
        .without::<&T>()
        .iter()
        .collect();
    for &e in &gone {
        let _ = world.remove_one::<Cached<T>>(e);
    }
    gone
}

pub(crate) fn forget<T: Generator>(world: &mut World, e: Entity) {
    let _ = world.remove_one::<Cached<T>>(e);
}

/// "Name (uid)" or the entity id, for log lines.
fn describe(world: &World, e: Entity) -> String {
    let name = world.get::<Name>(e).map(|n| n.0.clone());
    match (name, world.uid(e)) {
        (Some(n), _) => format!("`{n}`"),
        (None, Some(uid)) => format!("entity {uid}"),
        (None, None) => format!("entity {e:?}"),
    }
}

struct Watcher {
    probe: Box<dyn Probe>,
    dependents: Vec<Entity>,
}

/// Counters of the regeneration pipeline, for tests, benchmarks and tools.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlushStats {
    /// Flushes that took entities from the queue.
    pub flushes: u64,
    /// Regenerations (changed cache keys).
    pub generated: u64,
    /// Dirty entities whose cache key had not changed.
    pub skipped: u64,
    /// Entities left in the queue by the last flush (over budget).
    pub deferred: usize,
    /// Wall time of the last flush.
    pub last_flush: Duration,
}

/// The runtime's bookkeeping: generator types, the dirty queue, input
/// watchers and owned meshes. A world resource, created on first use.
pub struct ProcRuntime {
    epoch: Instant,
    /// Types in use: the registry's generators plus [`AppGeneratorExt::add_generator`] ones.
    types: Vec<ReflectGenerator>,
    explicit: Vec<ReflectGenerator>,
    forget: HashMap<TypeId, fn(&mut World, Entity)>,
    /// The registry `types` was built from (its address; 0: rebuild).
    registry: usize,
    queue: VecDeque<Entity>,
    watchers: HashMap<InputId, Watcher>,
    owned: HashMap<Entity, Handle<MeshData>>,
    last_flush: Option<Duration>,
    backlog: bool,
    stats: FlushStats,
}

impl Default for ProcRuntime {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            types: Vec::new(),
            // Primitives draw in every world, whatever its registry lists.
            explicit: vec![ReflectGenerator::of::<Primitive>()],
            forget: HashMap::new(),
            registry: 0,
            queue: VecDeque::new(),
            watchers: HashMap::new(),
            owned: HashMap::new(),
            last_flush: None,
            backlog: false,
            stats: FlushStats::default(),
        }
    }
}

impl ProcRuntime {
    pub fn stats(&self) -> FlushStats {
        self.stats
    }

    /// Entities waiting for a flush.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Input watchers (one per distinct watched input).
    pub fn watched_inputs(&self) -> usize {
        self.watchers.len()
    }

    /// Generator types the runtime regenerates, by short name.
    pub fn generator_names(&self) -> Vec<&'static str> {
        self.types.iter().map(|t| t.info().name).collect()
    }

    fn add_type(&mut self, t: ReflectGenerator) {
        if !self.explicit.iter().any(|x| x.type_id == t.type_id) {
            self.explicit.push(t);
            self.registry = 0;
        }
    }

    fn refresh_types(&mut self, registry: &Arc<Registry>) {
        let ptr = Arc::as_ptr(registry) as usize;
        if ptr == self.registry {
            return;
        }
        self.registry = ptr;
        let mut types: Vec<ReflectGenerator> = registry
            .generators()
            .into_iter()
            .map(|g| g.reflect().clone())
            .collect();
        for t in &self.explicit {
            if !types.iter().any(|x| x.type_id == t.type_id) {
                types.push(t.clone());
            }
        }
        for t in &types {
            self.forget.entry(t.type_id).or_insert(t.forget);
        }
        self.types = types;
    }

    fn run(&mut self, world: &mut World, settings: &ProcSettings, force: bool) {
        self.refresh_types(&world.registry().clone());
        if self.types.is_empty() && self.owned.is_empty() {
            return;
        }
        let mut stamps = Stamps::default();
        let fresh;
        {
            profiling::scope!("box_proc::stamp");
            for t in &self.types {
                (t.stamp)(world, &mut stamps);
            }
            let mut gone = Vec::new();
            for t in &self.types {
                gone.extend((t.lost)(world));
            }
            for e in gone {
                // Released when the generator its state belongs to is gone.
                let current = world.get::<ProcState>(e).map(|s| s.type_id);
                let alive = current.is_some_and(|ty| {
                    self.types
                        .iter()
                        .any(|t| t.type_id == ty && (t.has)(world, e))
                });
                if !alive {
                    self.release(world, e);
                }
            }
            self.collect_despawned(world);
            for w in self.watchers.values_mut() {
                if w.probe.changed(world) {
                    stamps.dirty.extend(&w.dependents);
                }
            }
            fresh = stamps.fresh;
            for e in stamps.dirty {
                if let Some(mut s) = world.get_mut::<ProcState>(e)
                    && !s.queued
                {
                    s.queued = true;
                    self.queue.push_back(e);
                }
            }
        }
        if self.queue.is_empty() {
            return;
        }
        let now = match settings.clock {
            ProcClock::Real => self.epoch.elapsed(),
            ProcClock::Manual(t) => t,
        };
        let throttled = self
            .last_flush
            .is_some_and(|last| now.saturating_sub(last) < settings.throttle);
        // New objects show in the frame they appear; the throttle only
        // coalesces changes.
        if !force && !self.backlog && !fresh && throttled {
            return;
        }
        profiling::scope!("box_proc::flush");
        self.last_flush = Some(now);
        let start = Instant::now();
        let mut done = 0usize;
        while let Some(e) = self.queue.pop_front() {
            if !force && done > 0 && start.elapsed() >= settings.budget {
                self.queue.push_front(e);
                break;
            }
            if self.process(world, e) {
                done += 1;
            }
        }
        self.backlog = !self.queue.is_empty();
        self.stats.flushes += 1;
        self.stats.deferred = self.queue.len();
        self.stats.last_flush = start.elapsed();
    }

    /// Owned meshes of despawned entities are freed.
    fn collect_despawned(&mut self, world: &mut World) {
        let live = world.query::<&ProcState>().iter().len();
        if self.owned.len() <= live {
            return;
        }
        let dead: Vec<Entity> = self
            .owned
            .keys()
            .copied()
            .filter(|e| !world.contains(*e))
            .collect();
        for e in dead {
            if let Some(h) = self.owned.remove(&e) {
                world.resource_or_default::<Assets<MeshData>>().remove(h);
            }
            for w in self.watchers.values_mut() {
                w.dependents.retain(|d| *d != e);
            }
        }
        self.watchers.retain(|_, w| !w.dependents.is_empty());
    }

    /// The entity no longer has a generator: drop its state and mesh.
    fn release(&mut self, world: &mut World, e: Entity) {
        let Ok(state) = world.remove_one::<ProcState>(e) else {
            return;
        };
        self.unwatch(e, &state.inputs);
        let _ = world.remove_one::<GeneratedMesh>(e);
        if let Some(h) = state.mesh {
            world.resource_or_default::<Assets<MeshData>>().remove(h);
        }
        self.owned.remove(&e);
        // A second generator left waiting gets its turn on the next pass.
        for forget in self.forget.values() {
            forget(world, e);
        }
    }

    fn unwatch(&mut self, e: Entity, inputs: &[InputId]) {
        for id in inputs {
            if let Some(w) = self.watchers.get_mut(id) {
                w.dependents.retain(|d| *d != e);
                if w.dependents.is_empty() {
                    self.watchers.remove(id);
                }
            }
        }
    }

    fn rewatch(&mut self, world: &World, e: Entity, old: &[InputId], inputs: &Inputs) {
        let stale: Vec<InputId> = old
            .iter()
            .filter(|id| !inputs.ids().any(|n| n == *id))
            .cloned()
            .collect();
        self.unwatch(e, &stale);
        for d in &inputs.decls {
            let w = self
                .watchers
                .entry(d.id.clone())
                .or_insert_with(|| Watcher {
                    probe: (d.probe)(world, &d.id),
                    dependents: Vec::new(),
                });
            if !w.dependents.contains(&e) {
                w.dependents.push(e);
            }
        }
    }

    /// Regenerates `e` if its cache key changed. False when there was
    /// nothing to process (despawned, or no generator any more).
    fn process(&mut self, world: &mut World, e: Entity) -> bool {
        let Some((type_id, old_key, old_inputs)) = world.get_mut::<ProcState>(e).map(|mut s| {
            s.queued = false;
            (s.type_id, s.key, s.inputs.clone())
        }) else {
            return false;
        };
        let Some(t) = self.types.iter().find(|t| t.type_id == type_id).cloned() else {
            return false;
        };
        let owner = world.uid(e);
        let mut inputs = Inputs::for_owner(owner);
        if !(t.inputs_on)(world, e, &mut inputs) {
            return false;
        }
        let seed = world.get::<Procedural>(e).map_or(0, |p| p.seed);
        let Some(params) = (t.params_hash)(world, e) else {
            return false;
        };
        let key = cache_key(world, &t, params, seed, &inputs);
        self.rewatch(world, e, &old_inputs, &inputs);
        let ids: Vec<InputId> = inputs.ids().cloned().collect();
        if old_key == Some(key) {
            if let Some(mut s) = world.get_mut::<ProcState>(e) {
                s.inputs = ids;
            }
            self.stats.skipped += 1;
            return true;
        }

        let (out, elapsed, mut warnings) = {
            let resolved: Vec<ResolvedInput<'_>> = inputs
                .decls
                .iter()
                .map(|d| ResolvedInput {
                    id: d.id.clone(),
                    value: (d.resolve)(world, &d.id),
                })
                .collect();
            let warnings: Vec<String> = resolved
                .iter()
                .filter(|r| r.value.is_none())
                .map(|r| format!("input {} is missing", r.id))
                .collect();
            let ctx = GenContext::with_inputs(seed, owner, &resolved);
            let start = Instant::now();
            let out = (t.generate_on)(world, e, &ctx);
            (out, start.elapsed(), warnings)
        };
        let Some(out) = out else {
            return false;
        };
        warnings.extend(out.warnings);
        let slots = t.info().material_slots.len().max(1);
        let mesh = out.mesh.and_then(|mut m| match m.validate() {
            Ok(()) => {
                // Normal maps need tangents: meshes whose generator left
                // them out get them from UV set 0 here (not part of the
                // generator's output or its golden hash).
                if m.tangents.is_empty() && !m.uvs.is_empty() {
                    m.generate_tangents();
                }
                Some(m)
            }
            Err(err) => {
                warnings.push(format!(
                    "generated an invalid mesh ({err}); drawing nothing"
                ));
                None
            }
        });
        let max_slot = mesh
            .as_ref()
            .and_then(|m| m.groups.iter().map(|g| g.material_slot as usize + 1).max())
            .unwrap_or(0);
        if max_slot > slots {
            warnings.push(format!(
                "a material group uses slot {} but `{}` declares {slots} material slot(s); the extra slots draw the entity's material",
                max_slot - 1,
                t.info().name
            ));
        }

        let old_mesh = world.get::<ProcState>(e).and_then(|s| s.mesh);
        let exists = old_mesh.is_some_and(|h| {
            world
                .resource_or_default::<Assets<MeshData>>()
                .get(h)
                .is_some()
        });
        if mesh.is_some() && old_mesh.is_some() && !exists {
            tracing::warn!(
                "{}: generated mesh asset was removed by something else; rebuilding it",
                describe(world, e)
            );
        }
        let meshes = world.resource_or_default::<Assets<MeshData>>();
        let handle = match (mesh, old_mesh) {
            (Some(m), Some(h)) if exists => {
                // An identical mesh (a parameter that does not shape it) is
                // not written, so the renderer does not upload it again.
                if meshes.get(h) != Some(&m) {
                    meshes.set(h, m);
                }
                Some(h)
            }
            (Some(m), _) => {
                let h = meshes.add(m);
                self.owned.insert(e, h);
                Some(h)
            }
            (None, Some(h)) => {
                meshes.remove(h);
                self.owned.remove(&e);
                None
            }
            (None, None) => None,
        };
        let changed_warnings = world
            .get::<ProcState>(e)
            .is_some_and(|s| s.warnings != warnings);
        if changed_warnings {
            let who = describe(world, e);
            for w in &warnings {
                tracing::warn!("{who}: {}: {w}", t.info().name);
            }
        }
        let generated = GeneratedMesh {
            mesh: handle,
            heights: match &out.collider {
                ColliderHint::Heights(grid) if grid.is_valid() => Some(grid.clone()),
                _ => None,
            },
        };
        if let Some(mut s) = world.get_mut::<ProcState>(e) {
            s.key = Some(key);
            s.mesh = handle;
            s.slot_count = slots.max(max_slot);
            s.warnings = warnings;
            s.collider = out.collider;
            s.last_generate = Some(elapsed);
            s.generations += 1;
            s.inputs = ids;
        }
        if world.get::<GeneratedMesh>(e).as_deref() != Some(&generated) {
            let _ = world.insert_one(e, generated);
        }
        self.stats.generated += 1;
        true
    }
}

/// hash(type, VERSION, parameters, seed, input ids and keys).
fn cache_key(world: &World, t: &ReflectGenerator, params: u64, seed: u64, inputs: &Inputs) -> u64 {
    let mut h = StableHasher::new();
    h.write_str(t.info().name);
    h.write_u32(t.info().version);
    h.write_u64(params);
    h.write_u64(seed);
    for d in &inputs.decls {
        match &d.id {
            InputId::Component { uid, type_name, .. } => {
                h.write_str("component");
                h.write(&uid.0.as_u128().to_le_bytes());
                h.write_str(type_name);
            }
            InputId::Asset {
                asset, type_name, ..
            } => {
                h.write_str("asset");
                h.write_u64(reflect_hash(asset));
                h.write_str(type_name);
            }
            InputId::WorldTransform { uid } => {
                h.write_str("world transform");
                h.write(&uid.0.as_u128().to_le_bytes());
            }
            InputId::Ground => h.write_str("ground"),
        }
        h.write_u64((d.key)(world, &d.id));
    }
    h.finish()
}

/// Stamp changed generators and, when the throttle allows, regenerate
/// within the budget (see the [module docs](self)). Uses the world's
/// [`ProcSettings`] (defaults when missing). Cheap when nothing changed.
pub fn flush(world: &mut World) {
    let settings = world
        .get_resource::<ProcSettings>()
        .cloned()
        .unwrap_or_default();
    run(world, &settings, false);
}

/// Regenerate everything that is dirty now, ignoring the throttle and the
/// budget: for tools and headless runs that must not draw stale geometry
/// (CLI renders, bake, tests).
pub fn settle(world: &mut World) {
    let settings = world
        .get_resource::<ProcSettings>()
        .cloned()
        .unwrap_or_default();
    run(world, &settings, true);
}

fn run(world: &mut World, settings: &ProcSettings, force: bool) {
    world.resource_or_default::<ProcRuntime>();
    world.resource_scope(|world, rt: &mut ProcRuntime| rt.run(world, settings, force));
}

/// True while objects wait for a flush. A host that does not draw frames
/// continuously (the editor does) must keep calling [`flush`] until it is
/// false, so the last edit of a drag lands; tools may call [`settle`].
pub fn pending(world: &World) -> bool {
    world
        .get_resource::<ProcRuntime>()
        .is_some_and(|rt| rt.queued() > 0)
}

/// Inserts [`ProcSettings`] and [`ProcRuntime`] and registers
/// `Procedural`. `box_engine::app_with_world` adds it. Generation itself
/// runs inside `box_engine::sync_scene_meshes` (which also builds the
/// renderers), so it reaches the editor and tools that call that function
/// without an `App`; this plugin adds no system.
pub struct ProcPlugin;

impl Plugin for ProcPlugin {
    fn register(&self, registry: &mut Registry) {
        register_proc_types(registry);
    }

    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<ProcSettings>();
        app.world.resource_or_default::<ProcRuntime>();
    }
}

/// `app.add_generator::<T>()`.
pub trait AppGeneratorExt {
    /// Regenerate generator `T` in this app even if the app's registry
    /// does not list it (tests, runtime-only generators). Games do not
    /// need this: `registry.register_generator::<T>()` in
    /// `Plugin::register` makes every app, the editor and the CLI pick it
    /// up. Warns when the registry lacks `T`, since it is then neither
    /// saved nor shown in the inspector.
    fn add_generator<T: Generator>(&mut self) -> &mut Self;
}

impl AppGeneratorExt for App {
    fn add_generator<T: Generator>(&mut self) -> &mut Self {
        if self
            .world
            .registry()
            .generator(T::short_type_path())
            .is_none()
        {
            tracing::warn!(
                "generator `{}` is not in this app's registry, so it is neither saved nor shown in the inspector; register it with registry.register_generator::<{}>() in Plugin::register",
                T::short_type_path(),
                T::short_type_path()
            );
        }
        self.world
            .resource_or_default::<ProcRuntime>()
            .add_type(ReflectGenerator::of::<T>());
        self
    }
}
