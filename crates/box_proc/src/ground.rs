//! The ground (world building W3): the height and normal of the terrain
//! under a point, for placing things on it.
//!
//! **[`Ground`]** is a world resource holding every [`Terrain`] with its
//! world transform. [`update_ground`] keeps it in step (change-driven: one
//! compare per terrain when nothing changed) and records which areas each
//! change touched ([`Ground::changed_since`]), so consumers that sampled
//! the ground re-run only where it moved. Heights are those of the drawn
//! and collided surface ([`Terrain::surface_height`]), so what stands on
//! the ground meets what the player walks on. Where terrains overlap the
//! highest wins.
//!
//! Consumers:
//!
//! - **[`FollowGround`]** on an entity keeps its origin on the ground plus
//!   an offset ([`follow_ground`]): landmarks, lamps, stairs. Change-driven;
//!   also runs in an editor's edit world.
//! - **Generators** declare `inputs.ground()` and read
//!   [`GenContext::ground`](crate::GenContext::ground): a `Sweep` with
//!   `follow_ground` lays its path on the terrain.
//! - **Edits**: the `snap_to_ground` command and the editor's Snap to
//!   Ground move an entity onto it once ([`sample_for_edit`]).
//! - **Rules and scripts** sample it through box_script's `ground` global.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use bevy_reflect::Reflect;
use box_core::reflect::{ReflectComponent, ReflectDefault, Unit};
use box_core::transform::{GlobalTransform, Transform, compute_global};
use box_core::world::{Entity, World};
use glam::{Affine3A, Vec2, Vec3};

use crate::hash::{StableHasher, reflect_hash};
use crate::terrain::Terrain;

/// An area on X and Z (world space): `(min, max)`.
pub type Area = (Vec2, Vec2);

/// How many changes [`Ground::changed_since`] remembers.
const KEPT_CHANGES: usize = 64;

/// One terrain as the ground sees it.
#[derive(Clone, Debug, PartialEq)]
struct GroundTerrain {
    entity: Entity,
    terrain: Terrain,
    transform: Affine3A,
    inverse: Affine3A,
}

impl GroundTerrain {
    fn new(entity: Entity, terrain: Terrain, transform: Affine3A) -> Self {
        Self {
            entity,
            terrain,
            transform,
            inverse: transform.inverse(),
        }
    }

    /// Same heights everywhere (the fields that shape the surface).
    fn same_shape(&self, o: &GroundTerrain) -> bool {
        self.transform == o.transform
            && self.terrain.base_height == o.terrain.base_height
            && self.terrain.cell_size == o.terrain.cell_size
            && self.terrain.size == o.terrain.size
            && self.terrain.stamps == o.terrain.stamps
    }

    /// Terrain-space area `a` as a world-space area.
    fn world_area(&self, a: Area) -> Area {
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for c in [a.0, Vec2::new(a.1.x, a.0.y), Vec2::new(a.0.x, a.1.y), a.1] {
            let w = self.transform.transform_point3(Vec3::new(c.x, 0.0, c.y));
            lo = lo.min(Vec2::new(w.x, w.z));
            hi = hi.max(Vec2::new(w.x, w.z));
        }
        (lo, hi)
    }

    fn area(&self) -> Area {
        self.world_area(self.terrain.extent())
    }

    /// The surface height at world (x, z), in world space.
    fn height(&self, x: f32, z: f32) -> Option<f32> {
        let local = self.inverse.transform_point3(Vec3::new(x, 0.0, z));
        let p = Vec2::new(local.x, local.z);
        let h = self.terrain.surface_height(p)?;
        Some(self.transform.transform_point3(Vec3::new(p.x, h, p.y)).y)
    }

    fn normal(&self, x: f32, z: f32) -> Vec3 {
        let local = self.inverse.transform_point3(Vec3::new(x, 0.0, z));
        let n = self.terrain.normal_at(Vec2::new(local.x, local.z));
        self.transform.transform_vector3(n).normalize_or(Vec3::Y)
    }
}

/// The ground at a point: height and surface normal (world space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundSample {
    pub height: f32,
    pub normal: Vec3,
}

/// The terrains of a world, cheap to clone (shared): what a rule or a
/// script samples while it runs.
#[derive(Clone, Debug, Default)]
pub struct GroundView {
    terrains: Arc<Vec<GroundTerrain>>,
}

impl GroundView {
    /// The ground height at world (x, z): the highest terrain there.
    /// `None` where no terrain is.
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        self.terrains
            .iter()
            .filter_map(|t| t.height(x, z))
            .reduce(f32::max)
    }

    /// Height and normal at world (x, z) (of the highest terrain there).
    pub fn sample(&self, x: f32, z: f32) -> Option<GroundSample> {
        let (t, height) = self
            .terrains
            .iter()
            .filter_map(|t| t.height(x, z).map(|h| (t, h)))
            .reduce(|a, b| if b.1 > a.1 { b } else { a })?;
        Some(GroundSample {
            height,
            normal: t.normal(x, z),
        })
    }

    /// The finest cell size of the terrains (metres; 1 without any): how
    /// often a path must sample the ground to follow it.
    pub fn cell_size(&self) -> f32 {
        self.terrains
            .iter()
            .map(|t| t.terrain.cell_size.max(0.01))
            .reduce(f32::min)
            .unwrap_or(1.0)
    }

    pub fn is_empty(&self) -> bool {
        self.terrains.is_empty()
    }
}

/// Resource: every [`Terrain`] of the world and the areas recent changes
/// touched. Kept current by [`update_ground`]. See the [module docs](self).
#[derive(Clone, Debug, Default)]
pub struct Ground {
    view: GroundView,
    version: u64,
    hash: u64,
    /// `(version, areas)` per change, oldest first.
    changes: VecDeque<(u64, Vec<Area>)>,
}

impl Ground {
    /// A ground of the given terrains (entity, terrain, world transform),
    /// for tests and tools; version 1.
    pub fn from_terrains(terrains: impl IntoIterator<Item = (Entity, Terrain, Affine3A)>) -> Self {
        let list: Vec<GroundTerrain> = terrains
            .into_iter()
            .map(|(e, t, tr)| GroundTerrain::new(e, t, tr))
            .collect();
        let mut g = Ground {
            hash: content_hash(&list),
            view: GroundView {
                terrains: Arc::new(list),
            },
            version: 1,
            changes: VecDeque::new(),
        };
        g.changes.push_back((1, Vec::new()));
        g
    }

    /// The terrains, shared ([`GroundView`]).
    pub fn view(&self) -> &GroundView {
        &self.view
    }

    /// See [`GroundView::height`].
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        self.view.height(x, z)
    }

    /// See [`GroundView::sample`].
    pub fn sample(&self, x: f32, z: f32) -> Option<GroundSample> {
        self.view.sample(x, z)
    }

    /// Bumped by every change of heights (0: never had a terrain).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// A hash of everything that shapes the heights (terrain transforms,
    /// sizes, stamps): equal hashes, equal ground.
    pub fn content_hash(&self) -> u64 {
        self.hash
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    /// The world areas whose heights changed after `version`; `None` when
    /// that is too long ago to know (treat everything as changed). Empty
    /// when nothing changed.
    pub fn changed_since(&self, version: u64) -> Option<Vec<Area>> {
        if version >= self.version {
            return Some(Vec::new());
        }
        let oldest = self.changes.front().map_or(u64::MAX, |c| c.0);
        if version + 1 < oldest {
            return None;
        }
        Some(
            self.changes
                .iter()
                .filter(|c| c.0 > version)
                .flat_map(|c| c.1.iter().copied())
                .collect(),
        )
    }
}

fn content_hash(terrains: &[GroundTerrain]) -> u64 {
    let mut h = StableHasher::new();
    for t in terrains {
        for v in t.transform.to_cols_array() {
            h.write_u32(v.to_bits());
        }
        h.write_u32(t.terrain.base_height.to_bits());
        h.write_u32(t.terrain.cell_size.to_bits());
        h.write_u32(t.terrain.size.x.to_bits());
        h.write_u32(t.terrain.size.y.to_bits());
        h.write_u64(reflect_hash(&t.terrain.stamps));
    }
    h.finish()
}

/// Brings the world's [`Ground`] up to date with its terrains (inserting
/// it when missing); true when heights changed. Reads world transforms, so
/// run it after propagation. One compare per terrain when nothing changed.
#[profiling::function]
pub fn update_ground(world: &mut World) -> bool {
    let ground = world.remove_resource::<Ground>().unwrap_or_default();
    let mut same = true;
    let mut n = 0;
    {
        let old = &ground.view.terrains;
        for (e, t, g) in world.query::<(Entity, &Terrain, &GlobalTransform)>().iter() {
            same &= old
                .get(n)
                .is_some_and(|o| o.entity == e && o.transform == g.0 && o.terrain == *t);
            n += 1;
            if !same {
                break;
            }
        }
    }
    if same && n == ground.view.terrains.len() {
        world.insert_resource(ground);
        return false;
    }
    let new: Vec<GroundTerrain> = world
        .query::<(Entity, &Terrain, &GlobalTransform)>()
        .iter()
        .map(|(e, t, g)| GroundTerrain::new(e, t.clone(), g.0))
        .collect();
    let old: HashMap<Entity, &GroundTerrain> =
        ground.view.terrains.iter().map(|t| (t.entity, t)).collect();
    let mut areas = Vec::new();
    for t in &new {
        match old.get(&t.entity) {
            None => areas.push(t.area()),
            Some(o) if o.same_shape(t) => {}
            Some(o) if o.transform != t.transform => {
                areas.push(o.area());
                areas.push(t.area());
            }
            Some(o) => areas.extend(
                t.terrain
                    .height_changes(&o.terrain)
                    .into_iter()
                    .map(|a| t.world_area(a)),
            ),
        }
    }
    for o in ground.view.terrains.iter() {
        if !new.iter().any(|t| t.entity == o.entity) {
            areas.push(o.area());
        }
    }
    let mut ground = ground;
    let changed = !areas.is_empty();
    if changed {
        ground.version += 1;
        ground.hash = content_hash(&new);
        ground.changes.push_back((ground.version, areas));
        while ground.changes.len() > KEPT_CHANGES {
            ground.changes.pop_front();
        }
    }
    ground.view.terrains = Arc::new(new);
    world.insert_resource(ground);
    changed
}

/// The ground for edits (`snap_to_ground`, the editor's Snap to Ground;
/// installed as the registry's `GroundSampler` by
/// [`register_proc_types`](crate::procedural::register_proc_types)). Brings
/// world transforms and [`Ground`] up to date first, so it works on a scene
/// just loaded by `box scene apply` and after an earlier command in the same
/// list moved or changed a terrain. Change-driven: cheap when nothing moved.
pub fn sample_for_edit(world: &mut World, x: f32, z: f32) -> Option<(f32, Vec3)> {
    box_core::transform::propagate_transforms(world);
    update_ground(world);
    let s = world.get_resource::<Ground>()?.sample(x, z)?;
    Some((s.height, s.normal))
}

/// Keeps an entity on the ground: its origin's height is the ground under
/// it plus `offset`, recomputed when the ground, the entity's position on
/// X and Z, or its parent moves. Move it freely on X and Z; its own height
/// is owned by the ground. Not for parts made by a rule (a rule samples
/// the ground itself).
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct FollowGround {
    /// Height of the origin above the ground (negative sinks it in).
    #[reflect(@Unit("m"))]
    pub offset: f32,
}

/// What [`follow_ground`] last did per entity.
#[derive(Default)]
struct FollowCache {
    entries: HashMap<Entity, Followed>,
}

#[derive(PartialEq)]
struct Followed {
    version: u64,
    translation: Vec3,
    offset: f32,
    parent: Affine3A,
}

/// Moves every [`FollowGround`] entity onto the ground where something
/// changed (see [`FollowGround`]), then propagates transforms if any
/// moved; returns how many moved. Run it after [`update_ground`].
#[profiling::function]
pub fn follow_ground(world: &mut World) -> usize {
    let Some(ground) = world.get_resource::<Ground>() else {
        return 0;
    };
    let version = ground.version();
    let view = ground.view().clone();
    let mut cache = world.remove_resource::<FollowCache>().unwrap_or_default();
    let mut writes: Vec<(Entity, Vec3)> = Vec::new();
    let mut seen: HashSet<Entity> = HashSet::new();
    for (e, follow, t) in world.query::<(Entity, &FollowGround, &Transform)>().iter() {
        seen.insert(e);
        let parent = world
            .parent(e)
            .map_or(Affine3A::IDENTITY, |p| compute_global(world, p));
        let key = Followed {
            version,
            translation: t.translation,
            offset: follow.offset,
            parent,
        };
        if cache.entries.get(&e) == Some(&key) {
            continue;
        }
        let w = parent.transform_point3(t.translation);
        let mut done = key;
        if let Some(h) = view.height(w.x, w.z) {
            let target = parent
                .inverse()
                .transform_point3(Vec3::new(w.x, h + follow.offset, w.z));
            if (target - t.translation).abs().max_element() > 1e-5 {
                writes.push((e, target));
                done.translation = target;
            }
        }
        cache.entries.insert(e, done);
    }
    if cache.entries.len() != seen.len() {
        cache.entries.retain(|e, _| seen.contains(e));
    }
    let moved = writes.len();
    for (e, target) in writes {
        if let Some(t) = world.component_mut::<Transform>(e) {
            t.translation = target;
        }
    }
    world.insert_resource(cache);
    if moved > 0 {
        box_core::transform::propagate_transforms(world);
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainStamp;

    fn hill(height: f32) -> Terrain {
        Terrain {
            size: Vec2::splat(32.0),
            stamps: vec![TerrainStamp::Hill {
                center: Vec2::ZERO,
                radius: 8.0,
                height,
            }],
            ..Terrain::default()
        }
    }

    #[test]
    fn heights_follow_the_terrain_transform_and_end_at_its_edge() {
        let mut w = World::new();
        let e = w.spawn((
            hill(3.0),
            Transform::from_xyz(10.0, 2.0, 0.0),
            GlobalTransform(Affine3A::from_translation(Vec3::new(10.0, 2.0, 0.0))),
        ));
        assert!(update_ground(&mut w));
        let g = w.resource::<Ground>();
        assert_eq!(g.version(), 1);
        let top = g.height(10.0, 0.0).unwrap();
        assert!((top - 5.0).abs() < 1e-4, "{top}");
        assert!((g.height(25.0, 0.0).unwrap() - 2.0).abs() < 1e-4);
        assert_eq!(g.height(27.0, 0.0), None, "beyond its 32 m");
        assert!(g.sample(10.0, 0.0).unwrap().normal.y > 0.99);
        // Unchanged: no new version.
        assert!(!update_ground(&mut w));
        // A non-height field changes nothing.
        w.get_mut::<Terrain>(e).unwrap().show_walkability = true;
        assert!(!update_ground(&mut w));
        assert_eq!(w.resource::<Ground>().version(), 1);
    }

    #[test]
    fn a_stamp_edit_reports_only_its_area() {
        let mut w = World::new();
        let mut t = hill(3.0);
        t.stamps.push(TerrainStamp::Hill {
            center: Vec2::new(-10.0, -10.0),
            radius: 2.0,
            height: 1.0,
        });
        let e = w.spawn((t, Transform::IDENTITY, GlobalTransform::default()));
        update_ground(&mut w);
        let v = w.resource::<Ground>().version();
        if let Some(mut t) = w.get_mut::<Terrain>(e)
            && let TerrainStamp::Hill { height, .. } = &mut t.stamps[1]
        {
            *height = 2.0;
        }
        assert!(update_ground(&mut w));
        let g = w.resource::<Ground>();
        let areas = g.changed_since(v).unwrap();
        assert_eq!(areas, vec![(Vec2::splat(-12.0), Vec2::splat(-8.0))]);
        assert_eq!(g.changed_since(g.version()), Some(Vec::new()));
        assert_eq!(
            g.changed_since(0).map(|a| a.len()),
            Some(2),
            "first add too"
        );
    }

    #[test]
    fn surface_heights_match_the_drawn_triangles() {
        use crate::generator::{GenContext, Generator};
        let t = Terrain {
            size: Vec2::new(40.0, 24.0),
            cell_size: 1.5,
            stamps: vec![
                TerrainStamp::Noise {
                    seed: 3,
                    scale: 7.0,
                    amplitude: 2.0,
                    octaves: 3,
                },
                TerrainStamp::Ridge {
                    points: vec![Vec2::new(-15.0, 0.0), Vec2::new(15.0, 5.0)],
                    width: 9.0,
                    height: 3.0,
                },
            ],
            ..Terrain::default()
        };
        let mut checked = 0;
        for (_, chunk) in t.chunks() {
            let mesh = chunk.generate(&GenContext::new(0)).mesh.unwrap();
            for tri in mesh.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[tri[k] as usize]);
                if a.y != a.y.max(-50.0) || [a, b, c].iter().any(|p| (p.y - a.y).abs() > 40.0) {
                    continue;
                }
                // Skirt triangles are vertical: skip them.
                let n = (b - a).cross(c - a);
                if n.y.abs() < 1e-6 {
                    continue;
                }
                let centre = (a + b + c) / 3.0;
                let h = t.surface_height(Vec2::new(centre.x, centre.z)).unwrap();
                assert!((h - centre.y).abs() < 1e-4, "{h} vs {}", centre.y);
                checked += 1;
            }
        }
        assert!(checked > 500, "{checked}");
    }

    #[test]
    fn followers_sit_on_the_ground_and_move_with_it() {
        let mut w = World::new();
        let terrain = w.spawn((hill(3.0), Transform::IDENTITY, GlobalTransform::default()));
        let lamp = w.spawn((
            Transform::from_xyz(0.0, 50.0, 0.0),
            GlobalTransform::default(),
            FollowGround { offset: 0.25 },
        ));
        update_ground(&mut w);
        assert_eq!(follow_ground(&mut w), 1);
        let y = w.get::<Transform>(lamp).unwrap().translation.y;
        assert!((y - 3.25).abs() < 1e-4, "{y}");
        assert_eq!(follow_ground(&mut w), 0, "nothing changed");
        if let Some(mut t) = w.get_mut::<Terrain>(terrain)
            && let TerrainStamp::Hill { height, .. } = &mut t.stamps[0]
        {
            *height = 3.5;
        }
        update_ground(&mut w);
        assert_eq!(follow_ground(&mut w), 1);
        let y = w.get::<Transform>(lamp).unwrap().translation.y;
        assert!((y - 3.75).abs() < 1e-4, "{y}");
    }
}
