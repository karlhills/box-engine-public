//! Transforms and hierarchy propagation.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::{Affine3A, Mat3, Quat, Vec3};

use crate::reflect::{Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use crate::world::{Entity, World};

/// Local transform relative to the parent (or the world, for roots).
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Transform {
    /// Position relative to the parent.
    #[reflect(@Unit("m"), @Step(0.01))]
    pub translation: Vec3,
    /// Orientation relative to the parent (a unit quaternion, x y z w).
    pub rotation: Quat,
    /// Scale along the local axes.
    #[reflect(@Step(0.01))]
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn from_xyz(x: f32, y: f32, z: f32) -> Self {
        Self::from_translation(Vec3::new(x, y, z))
    }

    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    pub fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        self.rotation = rotation;
        self
    }

    pub fn with_scale(mut self, scale: Vec3) -> Self {
        self.scale = scale;
        self
    }

    /// Rotate so that local −Z points along `direction`.
    pub fn looking_to(mut self, direction: Vec3, up: Vec3) -> Self {
        self.look_to(direction, up);
        self
    }

    /// Rotate so that local −Z points at `target`.
    pub fn looking_at(mut self, target: Vec3, up: Vec3) -> Self {
        self.look_to(target - self.translation, up);
        self
    }

    pub fn look_to(&mut self, direction: Vec3, up: Vec3) {
        let back = -direction.normalize_or(Vec3::NEG_Z);
        let up = up.normalize_or(Vec3::Y);
        let right = up
            .cross(back)
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        let up = back.cross(right);
        self.rotation = Quat::from_mat3(&Mat3::from_cols(right, up, back));
    }

    pub fn rotate_y(&mut self, angle: f32) {
        self.rotation = Quat::from_rotation_y(angle) * self.rotation;
    }

    /// Local −Z in parent space.
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    pub fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    pub fn to_affine(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// World-space transform, derived from [`Transform`] and the hierarchy by
/// transform propagation (start of `PostUpdate`). Never saved.
///
/// Read-only for game code: a value written into it is not propagated to
/// children and is overwritten only when the entity or an ancestor next
/// changes, so it silently diverges from `Transform`. Move entities by
/// writing [`Transform`]; between propagations read the current world
/// transform with [`compute_global`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlobalTransform(pub Affine3A);

impl Default for GlobalTransform {
    fn default() -> Self {
        Self(Affine3A::IDENTITY)
    }
}

impl GlobalTransform {
    /// The world-space affine transform.
    pub fn affine(&self) -> Affine3A {
        self.0
    }

    pub fn translation(&self) -> Vec3 {
        self.0.translation.into()
    }

    /// World-space −Z.
    pub fn forward(&self) -> Vec3 {
        self.0
            .transform_vector3(Vec3::NEG_Z)
            .normalize_or(Vec3::NEG_Z)
    }

    pub fn right(&self) -> Vec3 {
        self.0.transform_vector3(Vec3::X).normalize_or(Vec3::X)
    }

    pub fn up(&self) -> Vec3 {
        self.0.transform_vector3(Vec3::Y).normalize_or(Vec3::Y)
    }
}

/// The entity's parent. Maintained by [`World::set_parent`]; read it with
/// [`World::parent`]. Do not insert it yourself: `insert_one(child,
/// Parent(p))` leaves `p`'s [`Children`] without the child, so the child does
/// not follow its parent (propagation warns once when it sees this).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// The entity's children, in order. Maintained by [`World::set_parent`];
/// read it with [`World::children`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Children(pub Vec<Entity>);

/// What [`propagate_transforms`] last computed for an entity: the local
/// transform and parent it saw and the resulting world transform. Runtime
/// only: not registered, so never saved, copied or recorded by undo.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Propagated {
    local: Option<Transform>,
    parent: Option<Entity>,
    /// The simulated world transform (before any interpolation).
    pub(crate) global: Affine3A,
}

/// Bitwise equality, so a NaN component does not count as a change every frame.
pub(crate) fn same_local(a: Option<&Transform>, b: Option<&Transform>) -> bool {
    fn bits(t: &Transform) -> [u32; 10] {
        let (p, r, s) = (t.translation, t.rotation, t.scale);
        [p.x, p.y, p.z, r.x, r.y, r.z, r.w, s.x, s.y, s.z].map(f32::to_bits)
    }
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => bits(a) == bits(b),
        _ => false,
    }
}

/// What one [`propagate_transforms_with_stats`] call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PropagationStats {
    /// Entities compared against their cached local transform and parent.
    pub checked: usize,
    /// Entities whose `GlobalTransform` was recomputed.
    pub recomputed: usize,
}

/// Compute [`GlobalTransform`] for every entity with a [`Transform`], only
/// for subtrees that changed since the last call.
///
/// An entity without a `Transform` (a group or folder entity) counts as the
/// identity: it gets no `GlobalTransform`, but its children are still
/// propagated through it. Inserts `GlobalTransform` where it is missing.
///
/// Change detection compares each entity's `Transform` and `Parent` with the
/// copy cached at the last propagation, so writes through raw queries
/// (`&mut Transform`), reflection, undo, reparenting, spawning and adding or
/// removing `Transform` are all picked up without anyone marking entities
/// dirty. A changed entity's whole subtree is recomputed from its parent's
/// cached world transform. Cost when nothing changed: one linear pass over the
/// hierarchy entities comparing 10 floats and a parent id each; no matrix
/// work, no random access, no allocation.
///
/// `GlobalTransform` is derived data: writing it directly is overwritten only
/// when the entity or an ancestor next changes.
pub fn propagate_transforms(world: &mut World) {
    propagate_transforms_with_stats(world);
}

/// [`propagate_transforms`], returning how much work it did.
#[profiling::function]
pub fn propagate_transforms_with_stats(world: &mut World) -> PropagationStats {
    let mut stats = PropagationStats::default();
    let mut dirty: Vec<Entity> = Vec::new();
    for (e, local, parent, has_children, cache) in world
        .query::<(
            Entity,
            Option<&Transform>,
            Option<&Parent>,
            hecs::Satisfies<&Children>,
            Option<&Propagated>,
        )>()
        .iter()
    {
        let parent = parent.map(|p| p.0);
        match cache {
            Some(c) => {
                stats.checked += 1;
                if c.parent != parent || !same_local(c.local.as_ref(), local) {
                    dirty.push(e);
                }
            }
            // Entities outside the hierarchy without a transform are not tracked.
            None if local.is_some() || parent.is_some() || has_children => dirty.push(e),
            None => {}
        }
    }
    if dirty.is_empty() {
        return stats;
    }

    let dirty_set: HashSet<Entity> = dirty.iter().copied().collect();
    let limit = world.entity_count() as usize;
    let mut new_caches: Vec<(Entity, Propagated)> = Vec::new();
    let mut new_globals: Vec<(Entity, GlobalTransform)> = Vec::new();
    let mut stack: Vec<(Entity, Affine3A)> = Vec::new();
    for &root in &dirty {
        // Covered by the subtree walk of a dirty ancestor. Bounded so a cycle
        // made through raw hecs access cannot hang.
        let mut ancestor = world.parent(root);
        let mut covered = false;
        for _ in 0..limit {
            let Some(a) = ancestor else { break };
            if dirty_set.contains(&a) {
                covered = true;
                break;
            }
            ancestor = world.parent(a);
        }
        // A `Parent` inserted directly (not through `World::set_parent`) is
        // missing from the parent's `Children`, so no subtree walk reaches it.
        let linked = world.parent(root).is_none_or(|p| {
            world
                .get::<Children>(p)
                .is_some_and(|c| c.0.contains(&root))
        });
        if !linked {
            warn_unlinked_parent(root);
        } else if covered {
            continue;
        }
        // The parent is clean, so its cached world transform is current.
        let parent_global = match world.parent(root) {
            Some(p) if linked => world
                .get::<Propagated>(p)
                .map(|c| c.global)
                .unwrap_or_else(|| compute_global(world, p)),
            Some(p) => compute_global(world, p),
            None => Affine3A::IDENTITY,
        };
        stack.push((root, parent_global));
        while let Some((e, parent_global)) = stack.pop() {
            let local = world.get::<Transform>(e).map(|t| *t);
            let global = parent_global
                * local
                    .as_ref()
                    .map_or(Affine3A::IDENTITY, Transform::to_affine);
            let cache = Propagated {
                local,
                parent: world.parent(e),
                global,
            };
            stats.recomputed += 1;
            match world.get_mut::<Propagated>(e) {
                Some(mut c) => *c = cache,
                None => new_caches.push((e, cache)),
            }
            match world.get_mut::<GlobalTransform>(e) {
                Some(mut g) => g.0 = global,
                None if local.is_some() => new_globals.push((e, GlobalTransform(global))),
                None => {}
            }
            if let Some(children) = world.get::<Children>(e) {
                stack.extend(children.0.iter().map(|&c| (c, global)));
            }
        }
    }
    for (e, cache) in new_caches {
        let _ = world.insert_one(e, cache);
    }
    for (e, global) in new_globals {
        let _ = world.insert_one(e, global);
    }
    stats
}

/// Logs, once per process, that an entity has a `Parent` its parent's
/// `Children` does not list.
fn warn_unlinked_parent(entity: Entity) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "entity {entity:?} has a `Parent` component its parent's `Children` does not list \
             (inserted with insert/insert_one?), so it will not follow its parent; use \
             world.set_parent(child, Some(parent)) instead (warned once)"
        );
    }
}

/// An entity's world transform computed from the local transforms up its
/// parent chain (transform-less entities count as the identity). Unlike
/// [`GlobalTransform`] it is never stale, so edits use it between
/// propagations. O(depth).
pub fn compute_global(world: &World, entity: Entity) -> Affine3A {
    let mut global = Affine3A::IDENTITY;
    let mut cur = Some(entity);
    // Bounded so a cycle made through raw hecs access cannot hang.
    for _ in 0..=world.entity_count() {
        let Some(e) = cur else { break };
        if let Some(t) = world.get::<Transform>(e) {
            global = t.to_affine() * global;
        }
        cur = world.parent(e);
    }
    global
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    #[test]
    fn propagation_through_hierarchy() {
        let mut w = World::new();
        let root = w.spawn((Transform::from_xyz(1.0, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),));
        let child = w.spawn((Transform::from_xyz(0.0, 0.0, -2.0),));
        let grandchild =
            w.spawn((Transform::from_xyz(0.0, 1.0, 0.0).with_scale(Vec3::splat(2.0)),));
        w.set_parent(child, Some(root)).unwrap();
        w.set_parent(grandchild, Some(child)).unwrap();
        propagate_transforms(&mut w);

        // Root rotated 90° about Y: local -Z maps to world -X.
        let c = w.get::<GlobalTransform>(child).unwrap().translation();
        assert!(approx(c, Vec3::new(-1.0, 0.0, 0.0)), "{c}");
        let g = w.get::<GlobalTransform>(grandchild).unwrap().translation();
        assert!(approx(g, Vec3::new(-1.0, 1.0, 0.0)), "{g}");
    }

    #[test]
    fn reparenting_updates_globals() {
        let mut w = World::new();
        let a = w.spawn((Transform::from_xyz(10.0, 0.0, 0.0),));
        let b = w.spawn((Transform::from_xyz(0.0, 5.0, 0.0),));
        let c = w.spawn((Transform::from_xyz(1.0, 0.0, 0.0),));
        w.set_parent(c, Some(a)).unwrap();
        propagate_transforms(&mut w);
        assert!(approx(
            w.get::<GlobalTransform>(c).unwrap().translation(),
            Vec3::new(11.0, 0.0, 0.0)
        ));
        w.set_parent(c, Some(b)).unwrap();
        propagate_transforms(&mut w);
        assert!(approx(
            w.get::<GlobalTransform>(c).unwrap().translation(),
            Vec3::new(1.0, 5.0, 0.0)
        ));
        w.set_parent(c, None).unwrap();
        propagate_transforms(&mut w);
        assert!(approx(
            w.get::<GlobalTransform>(c).unwrap().translation(),
            Vec3::new(1.0, 0.0, 0.0)
        ));
    }

    #[test]
    fn transform_less_parent_counts_as_identity() {
        let mut w = World::new();
        let group = w.spawn((7u8,));
        let child = w.spawn((Transform::from_xyz(5.0, 0.0, 0.0),));
        let grandchild = w.spawn((Transform::from_xyz(0.0, 1.0, 0.0),));
        w.set_parent(child, Some(group)).unwrap();
        w.set_parent(grandchild, Some(child)).unwrap();
        // A transform-less entity in the middle of a chain is identity too.
        let root = w.spawn((Transform::from_xyz(0.0, 0.0, 10.0),));
        let mid = w.spawn((8u8,));
        let leaf = w.spawn((Transform::from_xyz(1.0, 0.0, 0.0),));
        w.set_parent(mid, Some(root)).unwrap();
        w.set_parent(leaf, Some(mid)).unwrap();
        propagate_transforms(&mut w);
        assert!(w.get::<GlobalTransform>(group).is_none());
        assert!(w.get::<GlobalTransform>(mid).is_none());
        let g = |e| w.get::<GlobalTransform>(e).unwrap().translation();
        assert!(approx(g(child), Vec3::new(5.0, 0.0, 0.0)));
        assert!(approx(g(grandchild), Vec3::new(5.0, 1.0, 0.0)));
        assert!(approx(g(leaf), Vec3::new(1.0, 0.0, 10.0)));
    }

    fn global(w: &World, e: Entity) -> Vec3 {
        w.get::<GlobalTransform>(e).unwrap().translation()
    }

    /// A root with `n` children, each with one grandchild: 1 + 2n entities.
    fn grid(w: &mut World, n: usize) -> (Entity, Vec<Entity>, Vec<Entity>) {
        let root = w.spawn((Transform::from_xyz(100.0, 0.0, 0.0),));
        let (mut kids, mut grandkids) = (Vec::new(), Vec::new());
        for i in 0..n {
            let k = w.spawn((Transform::from_xyz(i as f32, 0.0, 0.0),));
            let g = w.spawn((Transform::from_xyz(0.0, 1.0, 0.0),));
            w.set_parent(k, Some(root)).unwrap();
            w.set_parent(g, Some(k)).unwrap();
            kids.push(k);
            grandkids.push(g);
        }
        (root, kids, grandkids)
    }

    #[test]
    fn unchanged_world_recomputes_nothing() {
        let mut w = World::new();
        grid(&mut w, 50);
        w.spawn((5u32,)); // outside the hierarchy, no transform: not tracked
        let first = propagate_transforms_with_stats(&mut w);
        assert_eq!(first.recomputed, 101);
        for _ in 0..3 {
            let s = propagate_transforms_with_stats(&mut w);
            assert_eq!(
                s,
                PropagationStats {
                    checked: 101,
                    recomputed: 0
                }
            );
        }
        // Writing the same value is not a change.
        let e = w.query::<(Entity, &Transform)>().iter().next().unwrap().0;
        let same = *w.get::<Transform>(e).unwrap();
        *w.get_mut::<Transform>(e).unwrap() = same;
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 0);
    }

    #[test]
    fn raw_query_mutation_propagates_to_descendants_only() {
        let mut w = World::new();
        let (root, kids, grandkids) = grid(&mut w, 10);
        propagate_transforms(&mut w);
        // A game system writing through a raw query, marking nothing.
        for (e, t) in w.query::<(Entity, &mut Transform)>().iter() {
            if e == kids[3] {
                t.translation.z = 7.0;
            }
        }
        let s = propagate_transforms_with_stats(&mut w);
        assert_eq!(s.recomputed, 2, "the child and its grandchild");
        assert!(approx(global(&w, kids[3]), Vec3::new(103.0, 0.0, 7.0)));
        assert!(approx(global(&w, grandkids[3]), Vec3::new(103.0, 1.0, 7.0)));
        // Moving the root recomputes everything below it.
        w.get_mut::<Transform>(root).unwrap().translation.x = 0.0;
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 21);
        assert!(approx(global(&w, grandkids[9]), Vec3::new(9.0, 1.0, 0.0)));
        // A NaN is propagated once, not every frame.
        w.get_mut::<Transform>(kids[0]).unwrap().translation.x = f32::NAN;
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 2);
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 0);
    }

    #[test]
    fn reparent_place_despawn_and_transform_changes_update_globals() {
        let mut w = World::new();
        let a = w.spawn((Transform::from_xyz(10.0, 0.0, 0.0),));
        let b = w.spawn((Transform::from_xyz(0.0, 5.0, 0.0),));
        let c = w.spawn((Transform::from_xyz(1.0, 0.0, 0.0),));
        let d = w.spawn((Transform::from_xyz(0.0, 0.0, 1.0),));
        w.set_parent(c, Some(a)).unwrap();
        w.set_parent(d, Some(c)).unwrap();
        propagate_transforms(&mut w);
        assert!(approx(global(&w, d), Vec3::new(11.0, 0.0, 1.0)));

        w.set_parent(c, Some(b)).unwrap();
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 2);
        assert!(approx(global(&w, d), Vec3::new(1.0, 5.0, 1.0)));

        // place() between parents (and a reorder, which changes nothing).
        use crate::identity::Uid;
        w.insert_one(a, Uid::from_u128(1)).unwrap();
        w.place(c, crate::world::Placement::last(Some(Uid::from_u128(1))))
            .unwrap();
        propagate_transforms(&mut w);
        assert!(approx(global(&w, d), Vec3::new(11.0, 0.0, 1.0)));
        w.place(c, crate::world::Placement::last(Some(Uid::from_u128(1))))
            .unwrap();
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 0);

        // Removing the parent's Transform makes it the identity for its children.
        w.remove_one::<Transform>(a).unwrap();
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 3);
        assert!(approx(global(&w, d), Vec3::new(1.0, 0.0, 1.0)));
        w.insert_one(a, Transform::from_xyz(0.0, 0.0, -3.0))
            .unwrap();
        propagate_transforms(&mut w);
        assert!(approx(global(&w, d), Vec3::new(1.0, 0.0, -2.0)));

        // Despawning a subtree leaves the rest clean; a new child is computed.
        w.despawn(c);
        let s = propagate_transforms_with_stats(&mut w);
        assert_eq!(
            s,
            PropagationStats {
                checked: 2,
                recomputed: 0
            }
        );
        let e = w.spawn((Transform::from_xyz(0.0, 2.0, 0.0),));
        w.set_parent(e, Some(b)).unwrap();
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 1);
        assert!(approx(global(&w, e), Vec3::new(0.0, 7.0, 0.0)));
        // Unparenting.
        w.set_parent(e, None).unwrap();
        propagate_transforms(&mut w);
        assert!(approx(global(&w, e), Vec3::new(0.0, 2.0, 0.0)));
    }

    #[test]
    fn deep_hierarchy() {
        let mut w = World::new();
        let depth = 2000;
        let mut chain = vec![w.spawn((Transform::from_xyz(0.0, 1.0, 0.0),))];
        for i in 1..depth {
            // Every tenth link is a transform-less group.
            let e = if i % 10 == 0 {
                w.spawn((0u8,))
            } else {
                w.spawn((Transform::from_xyz(0.0, 1.0, 0.0),))
            };
            w.set_parent(e, Some(chain[i - 1])).unwrap();
            chain.push(e);
        }
        let with_transform = depth - (depth - 1) / 10;
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, depth);
        let leaf = *chain.last().unwrap();
        assert!(approx(
            global(&w, leaf),
            Vec3::new(0.0, with_transform as f32, 0.0)
        ));
        assert!(approx(
            compute_global(&w, leaf).translation.into(),
            global(&w, leaf)
        ));
        // A change near the bottom recomputes only the tail.
        w.get_mut::<Transform>(chain[depth - 3])
            .unwrap()
            .translation
            .x = 1.0;
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, 3);
        assert!(approx(
            global(&w, leaf),
            Vec3::new(1.0, with_transform as f32, 0.0)
        ));
        // Many dirty entities on one chain: each subtree is walked once.
        let mut moved = 0;
        for &e in chain.iter().step_by(7) {
            if let Some(mut t) = w.get_mut::<Transform>(e) {
                t.translation.z = 0.5;
                moved += 1;
            }
        }
        assert_eq!(propagate_transforms_with_stats(&mut w).recomputed, depth);
        let expected = Vec3::new(1.0, with_transform as f32, 0.5 * moved as f32);
        assert!((global(&w, leaf) - expected).length() < 1e-3);
    }

    #[test]
    fn look_to_points_forward() {
        let t = Transform::default().looking_to(Vec3::new(1.0, 0.0, 0.0), Vec3::Y);
        assert!(approx(t.forward(), Vec3::X));
        assert!(approx(t.up(), Vec3::Y));
        let t = Transform::from_xyz(0.0, 5.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y);
        assert!(approx(t.forward(), Vec3::new(0.0, -1.0, -1.0).normalize()));
    }
}
