//! The [`Physics`] resource: a query world mirrored from `Collider` and
//! `GlobalTransform`, and [`sync_physics`], the system that keeps it current
//! and reports trigger overlaps.

use std::collections::HashMap;

use box_core::event::Events;
use box_core::math::Ray;
use box_core::primitive::{Primitive, Shape};
use box_core::transform::GlobalTransform;
use box_core::world::{Entity, Without, World};
use glam::{Affine3A, Vec3};
use parry3d::bounding_volume::Aabb;
use parry3d::partitioning::{Bvh, BvhLeafUpdateStatus, BvhWorkspace};
use parry3d::query::{
    Ray as ParryRay, ShapeCastOptions, ShapeCastStatus, cast_shapes, intersection_test,
};
use parry3d::shape::SharedShape;

use box_core::mesh_asset::MeshAsset;

use crate::collider::{Collider, ColliderShape, MeshKey, MeshShapes, Resolved, mesh_key};
use crate::query::{Pose, QueryFilter, QueryShape, RayHit, ShapeHit};

/// Leaves are stored this much larger than their collider, so small motions
/// do not touch the tree.
const AABB_MARGIN: f32 = 0.05;
/// Longest distance a ray or shape cast may travel; larger limits are clamped.
const MAX_QUERY_DISTANCE: f32 = 1.0e6;

/// Whether a trigger started or stopped overlapping another collider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    Enter,
    Exit,
}

/// Sent through `Events<TriggerEvent>` when a trigger collider starts or
/// stops overlapping another collider. If either entity is despawned or loses
/// its collider while overlapping, an `Exit` is sent (the entity may no longer
/// exist). When two triggers overlap, each gets its own events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TriggerEvent {
    /// The entity with the trigger collider.
    pub trigger: Entity,
    /// The entity it overlaps.
    pub other: Entity,
    pub kind: TriggerKind,
}

/// What the last [`sync_physics`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Colliders compared with their cached copy.
    pub checked: usize,
    /// Colliders added to the query world.
    pub inserted: usize,
    /// Colliders whose pose or shape was updated.
    pub updated: usize,
    /// Shapes rebuilt (new colliders, or shape, primitive or scale changes).
    pub shapes_built: usize,
    /// Colliders removed (component removed, entity despawned, primitive gone).
    pub removed: usize,
}

/// Runtime marker linking an entity to its slot in [`Physics`]. Not
/// registered, so never saved, duplicated or recorded by undo.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColliderProxy {
    pub(crate) slot: u32,
}

/// One collider in the query world, and the inputs it was built from.
struct Slot {
    entity: Entity,
    collider: Collider,
    /// The primitive shape seen, for `ColliderShape::Primitive`.
    primitive: Option<Shape>,
    /// The mesh asset version seen, for `ColliderShape::Mesh`.
    mesh: Option<MeshKey>,
    global: Affine3A,
    /// World scale the shape was built with.
    scale: Vec3,
    shape: SharedShape,
    pose: parry3d::math::Pose,
    aabb: Aabb,
}

impl Slot {
    fn accepted_by(&self, filter: &QueryFilter) -> bool {
        filter.accepts(self.entity, self.collider.layers, self.collider.is_trigger)
    }
}

/// Resource: spatial queries over every entity with a `Collider` and a
/// `GlobalTransform`.
///
/// The query world is a parry BVH kept in step by [`sync_physics`] (added in
/// `PostUpdate` by [`PhysicsPlugin`](crate::PhysicsPlugin), after transform
/// propagation). Queries made during a frame see colliders as of the last
/// sync, which is the previous frame's `PostUpdate`.
#[derive(Default)]
pub struct Physics {
    bvh: Bvh,
    workspace: BvhWorkspace,
    slots: Vec<Option<Slot>>,
    free: Vec<u32>,
    live: usize,
    /// Overlapping (trigger, other) pairs, sorted.
    pairs: Vec<(Entity, Entity)>,
    /// Entity -> slot, for [`collider_pose`](Self::collider_pose).
    by_entity: HashMap<Entity, u32>,
    stats: SyncStats,
    /// Triangle meshes of `ColliderShape::Mesh` colliders.
    mesh_shapes: MeshShapes,
    /// Forces and impulses for the next dynamics step.
    #[cfg(feature = "dynamics")]
    pub(crate) commands: Vec<crate::dynamics::BodyCommand>,
}

impl Physics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of colliders in the query world.
    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// What the last sync did.
    pub fn last_sync(&self) -> SyncStats {
        self.stats
    }

    fn slot(&self, index: u32) -> Option<&Slot> {
        self.slots.get(index as usize)?.as_ref()
    }

    /// The slot for `index` if it belongs to `entity` (a proxy copied from
    /// another world points at someone else's slot).
    fn owned_slot(&self, index: u32, entity: Entity) -> Option<&Slot> {
        self.slot(index).filter(|s| s.entity == entity)
    }

    /// Where `entity`'s collider shape is in the query world (its centre,
    /// offset included, and rotation), as of the last sync or dynamics step.
    /// `None` if the entity has no collider there.
    pub fn collider_pose(&self, entity: Entity) -> Option<Pose> {
        let s = self.owned_slot(*self.by_entity.get(&entity)?, entity)?;
        Some(Pose::new(s.pose.translation, s.pose.rotation))
    }

    fn accepted(&self, leaf: u32, filter: &QueryFilter) -> Option<&Slot> {
        self.slot(leaf).filter(|s| s.accepted_by(filter))
    }

    // ---- queries -------------------------------------------------------

    /// The nearest solid collider hit by a ray from `origin` along `dir`
    /// (normalized here; any length) within `max_toi` metres: [`raycast`]
    /// with the default filter (every layer, triggers ignored).
    /// `physics.ray(eye, forward, 100.0)`. A zero or non-finite `dir` hits
    /// nothing.
    ///
    /// Colliders are as of the last sync (`PostUpdate`): a collider spawned
    /// or moved in this frame's `Update` is found from the next frame on.
    ///
    /// [`raycast`]: Self::raycast
    pub fn ray(&self, origin: Vec3, dir: Vec3, max_toi: f32) -> Option<RayHit> {
        let dir = dir.try_normalize()?;
        self.raycast(&Ray::new(origin, dir), max_toi, &QueryFilter::default())
    }

    /// The nearest non-filtered collider hit by `ray` within `max_toi`
    /// metres. A ray starting inside a collider hits it at `toi` 0.
    pub fn raycast(&self, ray: &Ray, max_toi: f32, filter: &QueryFilter) -> Option<RayHit> {
        let (pray, max_toi) = parry_ray(ray, max_toi)?;
        let (leaf, _) = self.bvh.cast_ray(&pray, max_toi, |leaf, best| {
            let s = self.accepted(leaf, filter)?;
            s.shape.cast_ray(&s.pose, &pray, best, true)
        })?;
        let s = self.slot(leaf)?;
        let hit = s
            .shape
            .cast_ray_and_get_normal(&s.pose, &pray, max_toi, true)?;
        Some(ray_hit(s.entity, ray, hit.time_of_impact, hit.normal))
    }

    /// Every non-filtered collider hit by `ray` within `max_toi` metres,
    /// nearest first (one hit per collider).
    pub fn raycast_all(&self, ray: &Ray, max_toi: f32, filter: &QueryFilter) -> Vec<RayHit> {
        let Some((pray, max_toi)) = parry_ray(ray, max_toi) else {
            return Vec::new();
        };
        let mut hits: Vec<RayHit> = self
            .bvh
            .leaves(|node| node.cast_ray(&pray, max_toi) <= max_toi)
            .filter_map(|leaf| {
                let s = self.accepted(leaf, filter)?;
                let hit = s
                    .shape
                    .cast_ray_and_get_normal(&s.pose, &pray, max_toi, true)?;
                Some(ray_hit(s.entity, ray, hit.time_of_impact, hit.normal))
            })
            .collect();
        hits.sort_by(|a, b| a.toi.total_cmp(&b.toi));
        hits
    }

    /// Move `shape` from `pose` along `direction` for up to `max_toi` metres
    /// and return the first non-filtered collider it touches. For kinematic
    /// movement: move by `toi`, then slide along `normal`. A shape that
    /// already overlaps a collider at `pose` hits it with `toi` 0 and
    /// `penetrating` set.
    pub fn shapecast(
        &self,
        shape: &QueryShape,
        pose: impl Into<Pose>,
        direction: Vec3,
        max_toi: f32,
        filter: &QueryFilter,
    ) -> Option<ShapeHit> {
        let local = shape.to_parry()?;
        let qshape = local.as_dyn();
        let ppose = pose.into().to_parry()?;
        let dir = direction.try_normalize()?;
        let max_toi = clamp_distance(max_toi)?;
        let start = qshape.compute_aabb(&ppose);
        let motion = dir * max_toi;
        let swept = Aabb::new(
            start.mins.min(start.mins + motion),
            start.maxs.max(start.maxs + motion),
        );
        let mut best: Option<(f32, u32, parry3d::query::ShapeCastHit)> = None;
        for leaf in self.bvh.intersect_aabb(&swept) {
            let Some(s) = self.accepted(leaf, filter) else {
                continue;
            };
            let limit = best.map_or(max_toi, |b| b.0);
            let options = ShapeCastOptions {
                max_time_of_impact: limit,
                target_distance: 0.0,
                stop_at_penetration: true,
                compute_impact_geometry_on_penetration: true,
            };
            match cast_shapes(&ppose, dir, qshape, &s.pose, Vec3::ZERO, &*s.shape, options) {
                Ok(Some(hit)) if hit.time_of_impact <= limit => {
                    if best.is_none_or(|b| hit.time_of_impact < b.0) {
                        best = Some((hit.time_of_impact, leaf, hit));
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::trace!("shapecast: {e:?}"),
            }
        }
        let (toi, leaf, hit) = best?;
        let s = self.slot(leaf)?;
        Some(ShapeHit {
            entity: s.entity,
            toi,
            point: s.pose * hit.witness2,
            normal: (s.pose.rotation * hit.normal2).normalize_or_zero(),
            penetrating: hit.status == ShapeCastStatus::PenetratingOrWithinTargetDist,
        })
    }

    /// Every non-filtered collider that `shape` at `pose` overlaps.
    pub fn overlap(
        &self,
        shape: &QueryShape,
        pose: impl Into<Pose>,
        filter: &QueryFilter,
    ) -> Vec<Entity> {
        let mut out = Vec::new();
        self.overlap_each(shape, pose, filter, |e| {
            out.push(e);
            true
        });
        out
    }

    /// [`overlap`](Self::overlap) without allocating: calls `f` for each
    /// overlapping collider's entity until it returns `false`.
    pub fn overlap_each(
        &self,
        shape: &QueryShape,
        pose: impl Into<Pose>,
        filter: &QueryFilter,
        mut f: impl FnMut(Entity) -> bool,
    ) {
        let Some(local) = shape.to_parry() else {
            return;
        };
        let Some(ppose) = pose.into().to_parry() else {
            return;
        };
        let qshape = local.as_dyn();
        let aabb = qshape.compute_aabb(&ppose);
        for leaf in self.bvh.intersect_aabb(&aabb) {
            let Some(s) = self.accepted(leaf, filter) else {
                continue;
            };
            let hit =
                intersection_test(&ppose, qshape, &s.pose, &*s.shape).is_ok_and(|i| i.intersecting);
            if hit && !f(s.entity) {
                return;
            }
        }
    }

    /// Every non-filtered collider containing `point` (solid shapes: a point
    /// inside a box counts).
    pub fn contains_point(&self, point: Vec3, filter: &QueryFilter) -> Vec<Entity> {
        if !point.is_finite() {
            return Vec::new();
        }
        let aabb = Aabb::new(point, point);
        self.bvh
            .intersect_aabb(&aabb)
            .filter_map(|leaf| {
                let s = self.accepted(leaf, filter)?;
                s.shape.contains_point(&s.pose, point).then_some(s.entity)
            })
            .collect()
    }

    // ---- sync ----------------------------------------------------------

    /// Bring the query world in step with `world` and return the trigger
    /// events this caused. See [`sync_physics`].
    #[profiling::function]
    fn sync(&mut self, world: &mut World) -> Vec<TriggerEvent> {
        self.stats = SyncStats::default();
        let work = self.collect_work(world);
        let gone = self.collect_removed(world);
        if work.is_empty() && gone.is_empty() {
            return Vec::new();
        }
        let mut needs_refit = false;
        for (entity, slot) in gone {
            if self.owned_slot(slot, entity).is_some() {
                self.free_slot(slot);
                needs_refit = true;
            }
            if world.contains(entity) {
                let _ = world.remove_one::<ColliderProxy>(entity);
            }
        }
        let mut new_proxies = Vec::new();
        for item in work {
            needs_refit |= self.apply(world, item, &mut new_proxies);
        }
        for (entity, slot) in new_proxies {
            let _ = world.insert_one(entity, ColliderProxy { slot });
        }
        if needs_refit {
            profiling::scope!("bvh refit");
            self.bvh.refit(&mut self.workspace);
            self.bvh.optimize_incremental(&mut self.workspace);
        }
        self.update_triggers()
    }

    /// Colliders that are new or differ from their cached copy.
    fn collect_work(&mut self, world: &World) -> Vec<Work> {
        let mut work = Vec::new();
        let mut query = world.query::<(
            Entity,
            &Collider,
            &GlobalTransform,
            Option<&Primitive>,
            Option<&MeshAsset>,
            Option<&ColliderProxy>,
        )>();
        for (entity, collider, global, primitive, mesh, proxy) in query.iter() {
            self.stats.checked += 1;
            let primitive = match collider.shape {
                ColliderShape::Primitive => primitive.map(|p| &p.shape),
                _ => None,
            };
            let mesh = match collider.shape {
                ColliderShape::Mesh | ColliderShape::ConvexHull => mesh_key(world, mesh),
                _ => None,
            };
            let slot = proxy.and_then(|p| Some((p.slot, self.owned_slot(p.slot, entity)?)));
            if let Some((_, s)) = slot
                && s.collider == *collider
                && same_affine(&s.global, &global.0)
                && s.primitive.as_ref() == primitive
                && s.mesh == mesh
            {
                continue;
            }
            work.push(Work {
                entity,
                slot: slot.map(|(i, _)| i),
                collider: collider.clone(),
                primitive: primitive.cloned(),
                mesh,
                global: global.0,
            });
        }
        work
    }

    /// Entities whose collider went away: the component or the transform was
    /// removed, or the entity was despawned.
    fn collect_removed(&self, world: &World) -> Vec<(Entity, u32)> {
        let mut gone: Vec<(Entity, u32)> = world
            .query::<Without<(Entity, &ColliderProxy), &Collider>>()
            .iter()
            .map(|(e, p)| (e, p.slot))
            .collect();
        gone.extend(
            world
                .query::<Without<(Entity, &ColliderProxy), &GlobalTransform>>()
                .iter()
                .map(|(e, p)| (e, p.slot)),
        );
        // Despawned entities took their proxy with them; scan the slots only
        // when there are more of them than proxies.
        let proxies = world.query::<&ColliderProxy>().iter().len();
        if self.live > proxies {
            gone.extend(
                self.slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| Some((i as u32, s.as_ref()?)))
                    .filter(|(_, s)| !world.contains(s.entity))
                    .map(|(i, s)| (s.entity, i)),
            );
        }
        gone
    }

    /// Insert or update one collider. Returns whether the tree changed.
    fn apply(
        &mut self,
        world: &mut World,
        item: Work,
        new_proxies: &mut Vec<(Entity, u32)>,
    ) -> bool {
        let built = self.build(world, &item);
        let Some((shape, shape_built, pose, scale)) = built else {
            // No usable shape (no primitive, or a degenerate transform).
            if let Some(slot) = item.slot {
                self.free_slot(slot);
                let _ = world.remove_one::<ColliderProxy>(item.entity);
                return true;
            }
            return false;
        };
        let aabb = shape.compute_aabb(&pose);
        if shape_built {
            self.stats.shapes_built += 1;
        }
        let slot = Slot {
            entity: item.entity,
            collider: item.collider,
            primitive: item.primitive,
            mesh: item.mesh,
            global: item.global,
            scale,
            shape,
            pose,
            aabb,
        };
        // A rebuilt shape may have shrunk: store its exact bounds.
        let margin = if shape_built { 0.0 } else { AABB_MARGIN };
        let index = match item.slot {
            Some(i) => {
                self.slots[i as usize] = Some(slot);
                self.stats.updated += 1;
                i
            }
            None => {
                let i = self.alloc_slot(slot);
                new_proxies.push((item.entity, i));
                self.stats.inserted += 1;
                i
            }
        };
        self.bvh.insert_or_update_partially(aabb, index, margin) != BvhLeafUpdateStatus::Unchanged
    }

    /// The shape (reused when its inputs did not change), whether it was
    /// built, the pose and the scale.
    fn build(
        &mut self,
        world: &World,
        item: &Work,
    ) -> Option<(SharedShape, bool, parry3d::math::Pose, Vec3)> {
        let (scale, rotation, _) = item.global.to_scale_rotation_translation();
        let rotation = rotation.normalize();
        let offset = if item.collider.shape.uses_mesh() {
            // The triangles are in the entity's own space.
            Vec3::ZERO
        } else if item.collider.offset.is_finite() {
            item.collider.offset
        } else {
            Vec3::ZERO
        };
        let translation = item.global.transform_point3(offset);
        if !scale.is_finite() || !rotation.is_finite() || !translation.is_finite() {
            return None;
        }
        let scale = scale.abs();
        let pose = parry3d::math::Pose::from_parts(translation, rotation);
        if let Some(old) = item.slot.and_then(|i| self.slot(i))
            && old.collider.shape == item.collider.shape
            && old.primitive == item.primitive
            && old.mesh == item.mesh
            && same_scale(old.scale, scale)
        {
            // Keep the scale the shape was built with, so slow drift from
            // rotation round-off cannot accumulate unnoticed.
            return Some((old.shape.clone(), false, pose, old.scale));
        }
        if item.collider.shape.uses_mesh() {
            let shape = self
                .mesh_shapes
                .get(world, item.mesh?, scale, &item.collider.shape)?;
            return Some((shape, true, pose, scale));
        }
        let resolved = Resolved::new(&item.collider.shape, item.primitive.as_ref())?;
        Some((resolved.build(scale), true, pose, scale))
    }

    /// Move `entity`'s collider to the world transform `global` at once (the
    /// dynamics step does this for bodies it moved, so queries made later in
    /// the same fixed step see them). Only the pose and bounds change; a
    /// changed scale is left to the next sync. Returns whether the tree
    /// needs a refit ([`refit`](Self::refit)).
    #[cfg(feature = "dynamics")]
    pub(crate) fn move_collider(&mut self, entity: Entity, global: Affine3A) -> bool {
        let Some(&index) = self.by_entity.get(&entity) else {
            return false;
        };
        let Some(slot) = self.slots.get_mut(index as usize).and_then(Option::as_mut) else {
            return false;
        };
        let (scale, rotation, _) = global.to_scale_rotation_translation();
        let offset = if slot.collider.offset.is_finite() {
            slot.collider.offset
        } else {
            Vec3::ZERO
        };
        let translation = global.transform_point3(offset);
        if !same_scale(slot.scale, scale.abs()) || !rotation.is_finite() || !translation.is_finite()
        {
            return false;
        }
        slot.global = global;
        slot.pose = parry3d::math::Pose::from_parts(translation, rotation.normalize());
        slot.aabb = slot.shape.compute_aabb(&slot.pose);
        self.bvh
            .insert_or_update_partially(slot.aabb, index, AABB_MARGIN)
            != BvhLeafUpdateStatus::Unchanged
    }

    /// Refit the tree after [`move_collider`](Self::move_collider) calls.
    #[cfg(feature = "dynamics")]
    pub(crate) fn refit(&mut self) {
        self.bvh.refit(&mut self.workspace);
        self.bvh.optimize_incremental(&mut self.workspace);
    }

    fn alloc_slot(&mut self, slot: Slot) -> u32 {
        self.live += 1;
        let entity = slot.entity;
        let index = match self.free.pop() {
            Some(i) => {
                self.slots[i as usize] = Some(slot);
                i
            }
            None => {
                self.slots.push(Some(slot));
                (self.slots.len() - 1) as u32
            }
        };
        self.by_entity.insert(entity, index);
        index
    }

    fn free_slot(&mut self, index: u32) {
        if let Some(s) = self.slots.get_mut(index as usize)
            && let Some(old) = s.take()
        {
            if self.by_entity.get(&old.entity) == Some(&index) {
                self.by_entity.remove(&old.entity);
            }
            self.bvh.remove(index);
            self.free.push(index);
            self.live -= 1;
            self.stats.removed += 1;
        }
    }

    /// Recompute trigger overlaps and diff them with the previous set. Each
    /// trigger is tested only against the colliders whose tree leaves
    /// overlap its bounds.
    #[profiling::function]
    fn update_triggers(&mut self) -> Vec<TriggerEvent> {
        let mut current = Vec::new();
        for s in self.slots.iter().flatten() {
            if !s.collider.is_trigger {
                continue;
            }
            for leaf in self.bvh.intersect_aabb(&s.aabb) {
                let Some(o) = self.slot(leaf) else { continue };
                if o.entity == s.entity
                    || s.collider.mask & o.collider.layers == 0
                    || o.collider.mask & s.collider.layers == 0
                {
                    continue;
                }
                if intersection_test(&s.pose, &*s.shape, &o.pose, &*o.shape)
                    .is_ok_and(|i| i.intersecting)
                {
                    current.push((s.entity, o.entity));
                }
            }
        }
        current.sort_unstable();
        current.dedup();
        let mut events = Vec::new();
        let (mut i, mut j) = (0, 0);
        let previous = &self.pairs;
        while i < previous.len() || j < current.len() {
            let event = |(trigger, other): (Entity, Entity), kind| TriggerEvent {
                trigger,
                other,
                kind,
            };
            match (previous.get(i), current.get(j)) {
                (Some(p), Some(c)) if p == c => {
                    i += 1;
                    j += 1;
                }
                (Some(p), Some(c)) if p < c => {
                    events.push(event(*p, TriggerKind::Exit));
                    i += 1;
                }
                (Some(p), None) => {
                    events.push(event(*p, TriggerKind::Exit));
                    i += 1;
                }
                (_, Some(c)) => {
                    events.push(event(*c, TriggerKind::Enter));
                    j += 1;
                }
                (None, None) => break,
            }
        }
        self.pairs = current;
        events
    }
}

struct Work {
    entity: Entity,
    /// The entity's current slot, if it has a valid one.
    slot: Option<u32>,
    collider: Collider,
    primitive: Option<Shape>,
    mesh: Option<MeshKey>,
    global: Affine3A,
}

/// Bitwise equality, so a NaN does not count as a change every frame.
fn same_affine(a: &Affine3A, b: &Affine3A) -> bool {
    a.to_cols_array().map(f32::to_bits) == b.to_cols_array().map(f32::to_bits)
}

/// Scales decomposed from rotated matrices differ in the last bits frame to
/// frame; only a relative change above 1e-5 rebuilds the shape.
fn same_scale(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() <= 1e-5 * a.max_element().max(b.max_element()).max(1e-6)
}

fn clamp_distance(max_toi: f32) -> Option<f32> {
    (max_toi >= 0.0).then(|| max_toi.min(MAX_QUERY_DISTANCE))
}

fn parry_ray(ray: &Ray, max_toi: f32) -> Option<(ParryRay, f32)> {
    let max_toi = clamp_distance(max_toi)?;
    (ray.origin.is_finite() && ray.direction.is_finite())
        .then(|| (ParryRay::new(ray.origin, ray.direction), max_toi))
}

fn ray_hit(entity: Entity, ray: &Ray, toi: f32, normal: Vec3) -> RayHit {
    RayHit {
        entity,
        toi,
        point: ray.at(toi),
        normal,
    }
}

/// Keep [`Physics`] in step with every entity's `Collider` and
/// `GlobalTransform`, then send [`TriggerEvent`]s for trigger overlaps that
/// started or ended. Inserts `Physics` if it is missing. Run it after
/// transform propagation ([`PhysicsPlugin`](crate::PhysicsPlugin) adds it to
/// `PostUpdate`); events go to `Events<TriggerEvent>` if that resource exists,
/// and systems read them in the next frame's `FixedUpdate` or `Update`.
///
/// Change-driven: each collider's component, `GlobalTransform` and (for
/// `ColliderShape::Primitive`) primitive shape are compared with the copy
/// cached at the last sync. Cost when nothing changed: one pass over collider
/// entities comparing 12 floats, the collider and an optional primitive, plus
/// an O(archetypes) count to detect despawns; no tree, shape or trigger work
/// and no allocation. A moved collider updates its pose and bounds (its tree
/// leaf only when it leaves a 5 cm margin); shapes are rebuilt only when the
/// shape, the primitive or the world scale changes. When anything changed the
/// tree is refitted once (O(colliders)) and triggers are re-tested against
/// the tree.
pub fn sync_physics(world: &mut World) {
    crate::character::warn_unmoved_characters(world);
    world.resource_or_default::<Physics>();
    let events = world.resource_scope::<Physics, _>(|world, physics| physics.sync(world));
    if !events.is_empty()
        && let Some(queue) = world.get_resource_mut::<Events<TriggerEvent>>()
    {
        queue.send_batch(events);
    }
}
