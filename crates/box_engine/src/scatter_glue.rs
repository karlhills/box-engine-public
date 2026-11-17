//! Scatter instances: the runtime side of `box_proc::scatter`.
//!
//! Every entity with a [`Scatter`] gets its instances as runtime entities
//! (never saved), regenerated per chunk when the chunk's inputs change.
//!
//! **Representation.** One derived part per filled chunk (`<owner
//! uid>/chunk_<x>_<z>`, rule [`SCATTER_RULE`], made with
//! `box_proc::derive::reconcile_with`, so they are children of the owner,
//! skipped by scene save, and plan and species warnings show on the owner),
//! and under each chunk one plain child entity per instance (per mesh of a
//! prefab species): `Transform`, `MeshRenderer` and a [`Derived`] marker,
//! with no `Uid` or `Name`. Instances of one species share the mesh and the
//! materials (one material per tint variant), so the renderer can draw them
//! instanced. A full `reconcile` part per instance was measured too slow at
//! these counts (see `docs/measurements/m6-scatter.md`): reconcile walks
//! every child of the owner and hashes keys on each run, and a part costs a
//! uid, a name and two strings. Chunks bound every update to the instances
//! of the touched chunks, and the owner's child list stays short.
//!
//! **Pipeline** (inside [`sync_scene_meshes`](crate::sync_scene_meshes),
//! after generators and their renderers, so a generated ground is current):
//!
//! 1. *Stamp*, every frame: each `Scatter` is compared with a cached copy,
//!    and the ground (the `Scatter::surface` entity's mesh handle, the
//!    mesh's version and its transform relative to the owner) with a cached
//!    signature. Species are resolved again only when the species list, the
//!    asset lookup or finished imports change. A change queues the owner.
//! 2. *Flush*, throttled like generators ([`box_proc::ProcSettings`]: 30 ms
//!    between flushes while changes keep coming, a new scatter at once):
//!    the plan is rebuilt, every chunk's key is recomputed (cheap hashes),
//!    and only chunks whose key changed are placed again, within the
//!    regeneration budget (6 ms; the rest wait for the next frame). Placing
//!    a chunk updates its instance entities by instance id: kept instances
//!    are written only if they moved, new ones spawned, vanished ones
//!    despawned.
//!
//! Headless runs and captures call [`settle`] (as
//! [`settle_assets`](crate::settle_assets) does), which ignores the throttle
//! and the budget.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use box_core::assets::Assets;
use box_core::derived::{Derived, PartKey};
use box_core::identity::Uid;
use box_core::mesh::MeshData;
use box_core::mesh_asset::MeshAsset;
use box_core::transform::{GlobalTransform, Transform};
use box_core::world::{Entity, World};
use box_proc::derive::{Part, reconcile_with};
use box_proc::scatter::{
    ChunkCoord, Scatter, ScatterInstance, ScatterPlan, ScatterSpecies, ScatterSurface,
    SpeciesSource, TINT_VARIANTS, tint_factor,
};
use box_proc::{ProcClock, ProcSettings};
use box_render::{MaterialHandle, MeshRenderer, StandardMaterial};
use glam::Affine3A;

use crate::asset_glue::{Resolved, mesh_asset_renderer, scene_meshes};

/// The rule name of scatter chunk parts (`Derived::rule`).
pub const SCATTER_RULE: &str = "scatter";

/// Runtime component of a scatter chunk part: which chunk it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScatterChunk {
    pub x: i32,
    pub z: i32,
}

/// What flushes did, for tests, benchmarks and tools.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScatterStats {
    /// Flushes that had work.
    pub flushes: u64,
    /// Chunks placed (their key changed), in total.
    pub chunks_placed: u64,
    /// Instance entities spawned, written (moved or redrawn) and despawned,
    /// in total.
    pub spawned: u64,
    pub updated: u64,
    pub despawned: u64,
    /// The last flush alone.
    pub last: FlushReport,
    /// Live instance entities.
    pub instances: usize,
    /// Chunks waiting for the next flush.
    pub pending_chunks: usize,
}

/// One flush (see [`ScatterStats::last`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlushReport {
    pub chunks_placed: usize,
    pub spawned: usize,
    pub updated: usize,
    pub despawned: usize,
    pub time: Duration,
}

/// The scatter runtime's bookkeeping. A world resource, created on first
/// use.
pub struct ScatterRuntime {
    epoch: Instant,
    owners: HashMap<Entity, OwnerState>,
    last_flush: Option<Duration>,
    backlog: bool,
    stats: ScatterStats,
}

impl Default for ScatterRuntime {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            owners: HashMap::new(),
            last_flush: None,
            backlog: false,
            stats: ScatterStats::default(),
        }
    }
}

impl ScatterRuntime {
    pub fn stats(&self) -> ScatterStats {
        self.stats
    }

    /// The current ground of the scatter on `owner`, in the owner's space
    /// (what the Paint Scatter tool aims at).
    pub fn surface(&self, owner: Entity) -> Option<&ScatterSurface> {
        self.owners.get(&owner).map(|o| &o.surface)
    }

    /// Instance entities of the scatter on `owner`.
    pub fn instance_count(&self, owner: Entity) -> usize {
        self.owners
            .get(&owner)
            .map_or(0, |o| o.chunks.values().map(|c| c.instances.len()).sum())
    }

    /// Chunk keys of the scatter on `owner`, sorted by chunk.
    pub fn chunk_keys(&self, owner: Entity) -> Vec<(ChunkCoord, u64)> {
        let mut v: Vec<(ChunkCoord, u64)> = self
            .owners
            .get(&owner)
            .map(|o| o.chunks.iter().map(|(c, s)| (*c, s.key)).collect())
            .unwrap_or_default();
        v.sort_unstable_by_key(|(c, _)| (c.1, c.0));
        v
    }
}

/// What a species draws: per piece (one for a mesh, one per `MeshAsset`
/// of a prefab) its transform in the instance's space and its renderer per
/// tint variant.
#[derive(Clone, Debug, Default, PartialEq)]
struct SpeciesDraw {
    /// What the variants were built from: the untinted pieces and the
    /// jitter (bits).
    bases: Vec<(Affine3A, MeshRenderer)>,
    jitter: u32,
    pieces: Vec<(Affine3A, Vec<MeshRenderer>)>,
}

/// What the ground was built from.
#[derive(Clone, Debug, PartialEq)]
enum GroundSig {
    Flat,
    /// The entity is missing or has no mesh renderer.
    Missing(Uid),
    /// The mesh is loading.
    Pending(Uid, u64),
    Mesh {
        uid: Uid,
        mesh: u64,
        version: u64,
        relative: [u32; 12],
        chunk_size: u32,
    },
}

struct ChunkState {
    key: u64,
    entity: Option<Entity>,
    /// (instance id, piece, entity), sorted.
    instances: Vec<(u64, u32, Entity)>,
}

struct OwnerState {
    scatter: Scatter,
    /// The species list and resolve stamp the draws were built for.
    species: Option<(Vec<ScatterSpecies>, (u64, u64))>,
    draws: Vec<Option<SpeciesDraw>>,
    species_warnings: Vec<String>,
    /// Tint variant materials owned by this scatter.
    owned: Vec<MaterialHandle>,
    ground: Option<GroundSig>,
    surface: ScatterSurface,
    plan: Option<ScatterPlan>,
    chunks: HashMap<ChunkCoord, ChunkState>,
    /// Chunks waiting to be placed.
    dirty: BTreeSet<(i32, i32)>,
    /// The scatter, the ground or the species changed: plan again.
    replan: bool,
    /// Warnings last given to reconcile.
    warnings: Vec<String>,
    warned_no_uid: bool,
}

impl OwnerState {
    fn new(scatter: Scatter) -> Self {
        Self {
            scatter,
            species: None,
            draws: Vec::new(),
            species_warnings: Vec::new(),
            owned: Vec::new(),
            ground: None,
            surface: ScatterSurface::Flat,
            plan: None,
            chunks: HashMap::new(),
            dirty: BTreeSet::new(),
            replan: true,
            warnings: Vec::new(),
            warned_no_uid: false,
        }
    }

    fn has_work(&self) -> bool {
        self.replan || !self.dirty.is_empty()
    }
}

/// One frame of scatter: stamp every `Scatter` and, when the throttle
/// allows, place changed chunks within the budget (see the module docs).
/// [`sync_scene_meshes`](crate::sync_scene_meshes) calls it.
pub fn sync_scatter(world: &mut World) {
    run(world, false);
}

/// Places every changed chunk now, ignoring the throttle and the budget:
/// for headless runs, captures and tests.
pub fn settle(world: &mut World) {
    run(world, true);
}

/// True while chunks wait for a flush.
pub fn pending(world: &World) -> bool {
    world
        .get_resource::<ScatterRuntime>()
        .is_some_and(|rt| rt.owners.values().any(OwnerState::has_work))
}

fn run(world: &mut World, force: bool) {
    let has_scatter = world.query::<&Scatter>().iter().next().is_some();
    if !has_scatter && !world.contains_resource::<ScatterRuntime>() {
        return;
    }
    let settings = world
        .get_resource::<ProcSettings>()
        .cloned()
        .unwrap_or_default();
    world.resource_or_default::<ScatterRuntime>();
    world.resource_scope(|world, rt: &mut ScatterRuntime| rt.run(world, &settings, force));
}

impl ScatterRuntime {
    fn run(&mut self, world: &mut World, settings: &ProcSettings, force: bool) {
        let fresh = {
            profiling::scope!("box_engine::scatter::stamp");
            self.stamp(world)
        };
        let busy = self.owners.values().any(OwnerState::has_work);
        self.stats.pending_chunks = self.owners.values().map(|o| o.dirty.len()).sum();
        if !busy {
            return;
        }
        let now = match settings.clock {
            ProcClock::Real => self.epoch.elapsed(),
            ProcClock::Manual(t) => t,
        };
        let throttled = self
            .last_flush
            .is_some_and(|last| now.saturating_sub(last) < settings.throttle);
        if !force && !fresh && !self.backlog && throttled {
            return;
        }
        profiling::scope!("box_engine::scatter::flush");
        self.last_flush = Some(now);
        let start = Instant::now();
        let budget = (!force).then_some(settings.budget);
        let mut report = FlushReport::default();
        let mut order: Vec<Entity> = self
            .owners
            .iter()
            .filter(|(_, o)| o.has_work())
            .map(|(e, _)| *e)
            .collect();
        order.sort_unstable();
        let mut placed_any = false;
        for e in order {
            let Some(mut owner) = self.owners.remove(&e) else {
                continue;
            };
            flush_owner(
                world,
                e,
                &mut owner,
                start,
                budget,
                &mut placed_any,
                &mut report,
            );
            self.owners.insert(e, owner);
        }
        report.time = start.elapsed();
        self.backlog = self.owners.values().any(OwnerState::has_work);
        if report.spawned + report.updated + report.despawned > 0 {
            // New and moved instances get their world transforms now.
            box_core::transform::propagate_transforms(world);
        }
        self.stats.flushes += 1;
        self.stats.chunks_placed += report.chunks_placed as u64;
        self.stats.spawned += report.spawned as u64;
        self.stats.updated += report.updated as u64;
        self.stats.despawned += report.despawned as u64;
        self.stats.last = report;
        self.stats.instances = self
            .owners
            .values()
            .map(|o| o.chunks.values().map(|c| c.instances.len()).sum::<usize>())
            .sum();
        self.stats.pending_chunks = self.owners.values().map(|o| o.dirty.len()).sum();
    }

    /// Compares every scatter and its ground with the cached copies; drops
    /// the instances of scatters that are gone. True if a scatter appeared.
    fn stamp(&mut self, world: &mut World) -> bool {
        let stamp = crate::scene_glue::link_stamp(world);
        let mut fresh = false;
        let mut seen: HashSet<Entity> = HashSet::with_capacity(self.owners.len());
        let mut grounds: Vec<(Entity, Option<Uid>, f32)> = Vec::new();
        for (e, scatter) in world.query::<(Entity, &Scatter)>().iter() {
            seen.insert(e);
            match self.owners.get_mut(&e) {
                None => {
                    self.owners.insert(e, OwnerState::new(scatter.clone()));
                    fresh = true;
                }
                Some(o) => {
                    if o.scatter != *scatter {
                        o.scatter = scatter.clone();
                        o.replan = true;
                    }
                    if o.species.as_ref().is_some_and(|(_, s)| *s != stamp) {
                        o.replan = true;
                    }
                }
            }
            grounds.push((e, scatter.surface, scatter.mask.chunk_size()));
        }
        for (e, surface, chunk_size) in grounds {
            let sig = ground_sig(world, e, surface, chunk_size);
            if let Some(o) = self.owners.get_mut(&e)
                && o.ground.as_ref() != Some(&sig)
            {
                o.replan = true;
            }
        }
        let gone: Vec<Entity> = self
            .owners
            .keys()
            .filter(|e| !seen.contains(e))
            .copied()
            .collect();
        for e in gone {
            if let Some(o) = self.owners.remove(&e) {
                if world.contains(e) {
                    // The Scatter was removed: its chunks go with their
                    // instances; the warnings are cleared.
                    let _ = reconcile_with(world, e, SCATTER_RULE, Vec::new(), Vec::new());
                }
                for m in o.owned {
                    crate::scene_glue::remove_material(world, m);
                }
            }
        }
        fresh
    }
}

/// The ground signature of the scatter on `owner` now.
fn ground_sig(world: &World, owner: Entity, surface: Option<Uid>, chunk_size: f32) -> GroundSig {
    let Some(uid) = surface else {
        return GroundSig::Flat;
    };
    let Some(target) = world.entity_by_uid(uid) else {
        return GroundSig::Missing(uid);
    };
    let Some(mesh) = world.get::<MeshRenderer>(target).map(|r| r.mesh) else {
        return GroundSig::Missing(uid);
    };
    let Some(version) = world
        .get_resource::<Assets<MeshData>>()
        .and_then(|a| a.get(mesh).is_some().then(|| a.version(mesh)).flatten())
    else {
        return GroundSig::Pending(uid, mesh.id());
    };
    let global = |e: Entity| {
        world
            .get::<GlobalTransform>(e)
            .map_or(Affine3A::IDENTITY, |g| g.0)
    };
    let relative = global(owner).inverse() * global(target);
    GroundSig::Mesh {
        uid,
        mesh: mesh.id(),
        version,
        relative: relative.to_cols_array().map(f32::to_bits),
        chunk_size: chunk_size.to_bits(),
    }
}

fn flush_owner(
    world: &mut World,
    e: Entity,
    o: &mut OwnerState,
    start: Instant,
    budget: Option<Duration>,
    placed_any: &mut bool,
    report: &mut FlushReport,
) {
    if o.replan {
        o.replan = false;
        replan(world, e, o, report);
    }
    let Some(plan) = o.plan.as_ref() else {
        o.dirty.clear();
        return;
    };
    while let Some(&c) = o.dirty.iter().next() {
        if *placed_any && budget.is_some_and(|b| start.elapsed() >= b) {
            return;
        }
        o.dirty.remove(&c);
        let Some(state) = o.chunks.get_mut(&c) else {
            continue;
        };
        let Some(chunk_entity) = state.entity.filter(|x| world.contains(*x)) else {
            continue;
        };
        let instances = plan.place(c, &o.surface);
        let key = world
            .get::<Derived>(chunk_entity)
            .map(|d| d.key.child("instance"));
        let Some(key) = key else { continue };
        let (s, u, d) = write_instances(
            world,
            chunk_entity,
            &key,
            &mut state.instances,
            &instances,
            &o.draws,
        );
        report.chunks_placed += 1;
        report.spawned += s;
        report.updated += u;
        report.despawned += d;
        *placed_any = true;
    }
}

/// Re-resolves species and the ground if needed, rebuilds the plan, marks
/// chunks whose key changed and reconciles the chunk parts.
fn replan(world: &mut World, e: Entity, o: &mut OwnerState, report: &mut FlushReport) {
    let Some(uid) = world.uid(e) else {
        if !o.warned_no_uid {
            o.warned_no_uid = true;
            tracing::warn!(
                target: "box_engine::scatter",
                "a Scatter on entity {e:?} has no Uid: scatter owners must be saved entities; nothing is placed"
            );
        }
        return;
    };
    // Species, when their list or the asset state changed.
    let stamp = crate::scene_glue::link_stamp(world);
    let species_changed = o
        .species
        .as_ref()
        .is_none_or(|(list, s)| *list != o.scatter.species || *s != stamp);
    let mut redraw = false;
    if species_changed {
        let (draws, warnings) = resolve_species(world, &o.scatter.species, &o.draws, &mut o.owned);
        redraw = draws != o.draws;
        o.draws = draws;
        o.species_warnings = warnings;
        o.species = Some((o.scatter.species.clone(), stamp));
    }

    // The ground.
    let chunk_size = o.scatter.mask.chunk_size();
    let sig = ground_sig(world, e, o.scatter.surface, chunk_size);
    let mut ground_warning = None;
    if o.ground.as_ref() != Some(&sig) {
        match &sig {
            GroundSig::Flat => o.surface = ScatterSurface::Flat,
            GroundSig::Missing(u) => {
                ground_warning = Some(format!(
                    "the ground entity {u} does not exist or has no mesh; nothing is placed"
                ));
                o.surface = ScatterSurface::Mesh(empty_surface(chunk_size));
            }
            // Keep the current instances until the mesh is loaded.
            GroundSig::Pending(..) => {}
            GroundSig::Mesh { uid, .. } => {
                let target = world.entity_by_uid(*uid);
                let built = target.and_then(|t| {
                    let mesh = world.get::<MeshRenderer>(t)?.mesh;
                    let meshes = world.get_resource::<Assets<MeshData>>()?;
                    let global = |x: Entity| {
                        world
                            .get::<GlobalTransform>(x)
                            .map_or(Affine3A::IDENTITY, |g| g.0)
                    };
                    let relative = global(e).inverse() * global(t);
                    Some(ScatterSurface::from_mesh(
                        meshes.get(mesh)?,
                        relative,
                        chunk_size,
                    ))
                });
                if let Some(s) = built {
                    o.surface = s;
                }
            }
        }
        o.ground = Some(sig.clone());
    } else if let GroundSig::Missing(u) = &sig {
        ground_warning = Some(format!(
            "the ground entity {u} does not exist or has no mesh; nothing is placed"
        ));
    }
    // The ground mesh is loading: keep what is placed until it lands (its
    // version then changes, and the stamp plans again).
    if matches!(sig, GroundSig::Pending(..)) {
        return;
    }

    // The plan and the chunks whose key changed.
    let plan = ScatterPlan::new(&o.scatter);
    let mut parts = Vec::with_capacity(plan.chunk_count());
    let mut keys: HashMap<ChunkCoord, u64> = HashMap::with_capacity(plan.chunk_count());
    for c in plan.chunks() {
        if let Some(k) = plan.chunk_key(c, &o.surface) {
            keys.insert(c, k);
        }
        parts.push(
            Part::new(PartKey::new(uid, format!("chunk_{}_{}", c.0, c.1)))
                .with(ScatterChunk { x: c.0, z: c.1 }),
        );
    }
    let mut warnings: Vec<String> = plan.warnings().to_vec();
    warnings.extend(o.species_warnings.iter().cloned());
    warnings.extend(ground_warning);
    let set_changed =
        keys.len() != o.chunks.len() || keys.keys().any(|c| !o.chunks.contains_key(c));
    if set_changed || warnings != o.warnings {
        if let Ok(r) = reconcile_with(world, e, SCATTER_RULE, parts, warnings.clone()) {
            let _ = r;
        }
        o.warnings = warnings;
    }
    // Chunks that are gone took their instances with them (reconcile
    // despawns subtrees).
    o.chunks.retain(|c, s| {
        let keep = keys.contains_key(c);
        if !keep {
            report.despawned += s.instances.len();
        }
        keep
    });
    o.dirty.retain(|c| keys.contains_key(c));
    for (c, key) in keys {
        let entity = world.entity_by_uid(PartKey::new(uid, format!("chunk_{}_{}", c.0, c.1)).uid());
        match o.chunks.get_mut(&c) {
            Some(s) => {
                if s.entity != entity {
                    // The chunk part was rebuilt (an undo restored the
                    // owner): its instances are gone.
                    s.instances.clear();
                    s.entity = entity;
                    o.dirty.insert(c);
                }
                if s.key != key || redraw {
                    s.key = key;
                    o.dirty.insert(c);
                }
            }
            None => {
                o.chunks.insert(
                    c,
                    ChunkState {
                        key,
                        entity,
                        instances: Vec::new(),
                    },
                );
                o.dirty.insert(c);
            }
        }
    }
    o.plan = Some(plan);
}

fn empty_surface(chunk_size: f32) -> box_proc::scatter::MeshSurface {
    match ScatterSurface::from_mesh(&MeshData::default(), Affine3A::IDENTITY, chunk_size) {
        ScatterSurface::Mesh(m) => m,
        ScatterSurface::Flat => unreachable!("from_mesh always builds a mesh surface"),
    }
}

/// Resolves every species to its pieces and tint variants. Variants of a
/// species whose base renderers did not change are kept (their materials
/// too); others are rebuilt and the old variant materials freed.
fn resolve_species(
    world: &mut World,
    species: &[ScatterSpecies],
    old: &[Option<SpeciesDraw>],
    owned: &mut Vec<MaterialHandle>,
) -> (Vec<Option<SpeciesDraw>>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut draws = Vec::with_capacity(species.len());
    let mut keep: Vec<MaterialHandle> = Vec::new();
    for (i, sp) in species.iter().enumerate() {
        let bases: Option<Vec<(Affine3A, MeshRenderer)>> = match &sp.source {
            SpeciesSource::Mesh { mesh, materials } => {
                if mesh.is_none() {
                    warnings.push(format!("species {i}: no mesh chosen"));
                    None
                } else {
                    let source = MeshAsset {
                        mesh: mesh.clone(),
                        materials: if materials.is_empty() {
                            vec![box_core::asset_ref::AssetRef::NONE]
                        } else {
                            materials.clone()
                        },
                        cast_shadows: sp.cast_shadows,
                    };
                    match mesh_asset_renderer(world, &source) {
                        (Some(r), _) => Some(vec![(Affine3A::IDENTITY, r)]),
                        (None, _) => None,
                    }
                }
            }
            SpeciesSource::Prefab { scene } => {
                if scene.is_none() {
                    warnings.push(format!("species {i}: no prefab chosen"));
                    None
                } else {
                    match scene_meshes(world, scene) {
                        Resolved::Ready(list) => {
                            if list.is_empty() {
                                warnings.push(format!(
                                    "species {i}: prefab {scene} has no MeshAsset entities to scatter"
                                ));
                            }
                            let mut pieces = Vec::new();
                            let mut pending = false;
                            for (local, mut mesh) in list {
                                mesh.cast_shadows &= sp.cast_shadows;
                                match mesh_asset_renderer(world, &mesh) {
                                    (Some(r), _) => pieces.push((local, r)),
                                    (None, p) => pending |= p,
                                }
                            }
                            (!pending).then_some(pieces)
                        }
                        Resolved::Pending => None,
                        Resolved::Missing(m) => {
                            warnings.push(format!("species {i}: prefab {scene}: {m}"));
                            None
                        }
                    }
                }
            }
        };
        let Some(bases) = bases else {
            draws.push(None);
            continue;
        };
        let jitter = if sp.tint_jitter.is_finite() {
            sp.tint_jitter.clamp(0.0, 0.5)
        } else {
            0.0
        };
        // The same bases and jitter: keep the variants (and their materials).
        let reuse = old
            .get(i)
            .and_then(Option::as_ref)
            .filter(|d| d.bases == bases && d.jitter == jitter.to_bits());
        if let Some(d) = reuse {
            if jitter > 0.0 {
                for (_, variants) in &d.pieces {
                    for v in variants {
                        keep.extend(v.materials.iter().copied());
                    }
                }
            }
            draws.push(Some(d.clone()));
            continue;
        }
        let mut shader = false;
        let pieces = bases
            .iter()
            .map(|(local, base)| {
                let variants = if jitter > 0.0 {
                    (0..TINT_VARIANTS)
                        .map(|v| {
                            let (r, s) = tinted(world, base, tint_factor(jitter, v), &mut keep);
                            shader |= s;
                            r
                        })
                        .collect()
                } else {
                    vec![base.clone()]
                };
                (*local, variants)
            })
            .collect();
        if shader {
            warnings.push(format!(
                "species {i}: tint jitter applies to standard materials only"
            ));
        }
        draws.push(Some(SpeciesDraw {
            bases,
            jitter: jitter.to_bits(),
            pieces,
        }));
    }
    // Free variant materials no species uses any more.
    for m in owned.drain(..) {
        if !keep.contains(&m) {
            crate::scene_glue::remove_material(world, m);
        }
    }
    *owned = keep;
    (draws, warnings)
}

/// `base` with every standard material copied and its colour scaled by
/// `factor` (the copies are pushed to `owned`). True if a shader material
/// was left as it was.
fn tinted(
    world: &mut World,
    base: &MeshRenderer,
    factor: f32,
    owned: &mut Vec<MaterialHandle>,
) -> (MeshRenderer, bool) {
    let mut shader = false;
    let mut out = base.clone();
    for m in &mut out.materials {
        match *m {
            MaterialHandle::Standard(h) => {
                let Some(mut mat) = world
                    .get_resource::<Assets<StandardMaterial>>()
                    .and_then(|a| a.get(h).cloned())
                else {
                    continue;
                };
                let c = &mut mat.base_color;
                c.r *= factor;
                c.g *= factor;
                c.b *= factor;
                let handle: MaterialHandle = world
                    .resource_or_default::<Assets<StandardMaterial>>()
                    .add(mat)
                    .into();
                owned.push(handle);
                *m = handle;
            }
            MaterialHandle::Shader(_) => shader = true,
        }
    }
    (out, shader)
}

/// Makes the instance entities under `chunk` match `instances`, by
/// (instance id, piece): kept ones are written only when they differ.
/// Returns (spawned, updated, despawned).
fn write_instances(
    world: &mut World,
    chunk: Entity,
    key: &PartKey,
    current: &mut Vec<(u64, u32, Entity)>,
    instances: &[ScatterInstance],
    draws: &[Option<SpeciesDraw>],
) -> (usize, usize, usize) {
    let mut wanted: Vec<(u64, u32, Transform, &MeshRenderer)> = Vec::with_capacity(instances.len());
    for inst in instances {
        let Some(Some(draw)) = draws.get(inst.species as usize) else {
            continue;
        };
        for (p, (local, variants)) in draw.pieces.iter().enumerate() {
            let renderer = &variants[(inst.tint as usize).min(variants.len() - 1)];
            let transform = if *local == Affine3A::IDENTITY {
                inst.transform
            } else {
                let m = inst.transform.to_affine() * *local;
                let (scale, rotation, translation) = m.to_scale_rotation_translation();
                Transform {
                    translation,
                    rotation,
                    scale,
                }
            };
            wanted.push((inst.id, p as u32, transform, renderer));
        }
    }
    // `instances` is sorted by id and pieces are pushed in order.
    let (mut spawned, mut updated, mut despawned) = (0, 0, 0);
    let mut next: Vec<(u64, u32, Entity)> = Vec::with_capacity(wanted.len());
    let mut old = std::mem::take(current).into_iter().peekable();
    for (id, piece, transform, renderer) in wanted {
        // Despawn old entries before this one.
        while let Some(&(oid, op, oe)) = old.peek() {
            if (oid, op) < (id, piece) {
                world.despawn(oe);
                despawned += 1;
                old.next();
            } else {
                break;
            }
        }
        match old.peek().copied() {
            Some((oid, op, oe)) if (oid, op) == (id, piece) && world.contains(oe) => {
                old.next();
                let mut wrote = false;
                if world.get::<Transform>(oe).is_none_or(|t| *t != transform) {
                    let _ = world.insert_one(oe, transform);
                    wrote = true;
                }
                if world
                    .get::<MeshRenderer>(oe)
                    .is_none_or(|r| *r != *renderer)
                {
                    let _ = world.insert_one(oe, renderer.clone());
                    wrote = true;
                }
                if wrote {
                    updated += 1;
                }
                next.push((id, piece, oe));
            }
            _ => {
                let e = world.spawn((
                    transform,
                    renderer.clone(),
                    Derived {
                        owner: chunk,
                        key: key.clone(),
                        rule: SCATTER_RULE.to_owned(),
                    },
                ));
                let _ = world.set_parent(e, Some(chunk));
                spawned += 1;
                next.push((id, piece, e));
            }
        }
    }
    for (_, _, oe) in old {
        world.despawn(oe);
        despawned += 1;
    }
    *current = next;
    (spawned, updated, despawned)
}
