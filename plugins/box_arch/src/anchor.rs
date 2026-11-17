//! Attachment anchors: a saved [`Anchor`] places a decoration (any entity:
//! a lantern prefab, a sign, a planter) on a surface of another entity's
//! generator, and keeps it there when that generator's parameters or
//! transform change.
//!
//! **How it resolves.** Generators are pure and cannot move other entities,
//! so anchors are resolved by a system ([`resolve_anchors`], added by
//! [`ArchPlugin`](crate::ArchPlugin) in `PostUpdate` after generation). For
//! every entity with an `Anchor` it reads the target's generator component
//! and world transform, asks the target type for the surface frame
//! ([`Anchorable::anchor_frame`], pure geometry in the target's local
//! space), and writes the decoration's `Transform` (translation and
//! rotation; its scale is kept) relative to its own parent. It is
//! change-driven: each anchor keeps a snapshot of its anchor value, the
//! target's parameters (via [`Anchorable::Snapshot`]) and both world
//! transforms, and recomputes only when one differs, so idle cost is a few
//! comparisons per anchor.
//!
//! **Surface frames.** X runs along the surface (U), Y up, Z out of the
//! surface; `offset` is (u, v, w) in that frame. A decoration modelled
//! with +Z sticking out of a wall works on any face.
//!
//! Games make their own generators anchorable with
//! [`AnchorTargets::add`] (implement [`Anchorable`]).

use box_core::pmath;
use std::any::Any;
use std::collections::HashMap;

use bevy_reflect::Reflect;
use box_core::identity::Uid;
use box_core::reflect::{ReflectComponent, ReflectDefault, Unit};
use box_core::transform::{Transform, compute_global};
use box_core::world::{Component, Entity, World};
use box_proc::spline::Spline;
use glam::{Affine3A, Mat3, Vec2, Vec3};

use crate::roof::{Roof, RoofShape};
use crate::wall::{Wall, WallPath, transformed_spline};

/// Which face of a wall.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum WallSide {
    /// The +w face: right of the direction of travel (the outside of a
    /// loop drawn counter-clockwise seen from above).
    #[default]
    Outer,
    Inner,
}

/// The surface of the target an anchor sits on, and what its offset means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum AnchorSurface {
    /// The target's local space: offset is (x, y, z), no rotation.
    #[default]
    Local,
    /// A wall face: u = arc length along the centreline, v = height above
    /// the foot, w = distance out from the face.
    WallFace { side: WallSide },
    /// A wall's top: u along the centreline, v above the top, w across
    /// from the centreline (towards the outer face).
    WallTop,
    /// An opening of a wall, by index: origin at the middle of its head on
    /// the face; u along, v up, w out.
    WallOpening { index: u32, side: WallSide },
    /// A roof's ridge (apex for cones and domes, the high edge for sheds):
    /// origin on top of the ridge's middle; u along the ridge, v up, w across.
    RoofRidge,
}

/// Places this entity on a surface of the entity `target` (a `Wall`,
/// `Roof`, `Platform`, `Stair` or a game's anchorable generator) and keeps
/// it there as the target changes. Writes this entity's `Transform`
/// (translation and rotation).
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct Anchor {
    pub target: Option<Uid>,
    pub surface: AnchorSurface,
    /// (u, v, w) in the surface's frame (see `surface`).
    #[reflect(@Unit("m"))]
    pub offset: Vec3,
}

impl Anchor {
    pub fn new(target: Uid, surface: AnchorSurface, offset: Vec3) -> Self {
        Self {
            target: Some(target),
            surface,
            offset,
        }
    }
}

/// A generator component decorations can be anchored to.
pub trait Anchorable: Component + Clone {
    /// What the frame depends on, compared every frame to notice changes
    /// (usually a clone of the component; a spline-driven wall adds its
    /// spline).
    type Snapshot: PartialEq + Send + Sync + 'static;

    fn snapshot(&self, world: &World, entity: Entity) -> Self::Snapshot;

    /// Whether the frame inputs still equal `old` (called every frame per
    /// anchor; override to compare without building a snapshot).
    fn unchanged(&self, world: &World, entity: Entity, old: &Self::Snapshot) -> bool {
        self.snapshot(world, entity) == *old
    }

    /// The frame (X along, Y up, Z out of the surface, origin at `offset`)
    /// in the target entity's local space, or why there is none.
    fn anchor_frame(
        &self,
        surface: &AnchorSurface,
        offset: Vec3,
        world: &World,
        entity: Entity,
    ) -> Result<Affine3A, String>;
}

type FrameFn = fn(&World, Entity, &AnchorSurface, Vec3) -> Option<Result<Affine3A, String>>;
type SnapFn = fn(&World, Entity) -> Option<Box<dyn Any + Send + Sync>>;
type SameFn = fn(&World, Entity, &(dyn Any + Send + Sync)) -> bool;

#[derive(Clone, Copy)]
struct TargetType {
    frame: FrameFn,
    snap: SnapFn,
    same: SameFn,
}

/// Resource: the generator types anchors can target (`Wall`, `Roof`,
/// `Platform`, `Stair` built in; games add theirs).
#[derive(Clone)]
pub struct AnchorTargets {
    types: Vec<TargetType>,
}

impl Default for AnchorTargets {
    fn default() -> Self {
        let mut t = Self { types: Vec::new() };
        t.add::<Wall>();
        t.add::<Roof>();
        t.add::<crate::Platform>();
        t.add::<crate::Stair>();
        t
    }
}

impl AnchorTargets {
    /// Makes `T` a valid anchor target.
    pub fn add<T: Anchorable>(&mut self) -> &mut Self {
        self.types.push(TargetType {
            frame: |w, e, s, o| {
                let c = w.get::<T>(e)?;
                Some(c.anchor_frame(s, o, w, e))
            },
            snap: |w, e| {
                let c = w.get::<T>(e)?;
                Some(Box::new(c.snapshot(w, e)))
            },
            same: |w, e, old| match (w.get::<T>(e), old.downcast_ref::<T::Snapshot>()) {
                (Some(c), Some(old)) => c.unchanged(w, e, old),
                _ => false,
            },
        });
        self
    }
}

struct Cached {
    anchor: Anchor,
    target: Option<Entity>,
    target_world: Affine3A,
    parent_world: Affine3A,
    snapshot: Option<Box<dyn Any + Send + Sync>>,
    problem: Option<String>,
}

/// Resource: per-anchor change detection state (never saved).
#[derive(Default)]
pub struct AnchorCache {
    entries: HashMap<Entity, Cached>,
}

/// The name of the anchor system (for ordering).
pub const ANCHORS_SYSTEM: &str = "box_arch::anchors";

/// Resolves every [`Anchor`] whose anchor, target parameters or transforms
/// changed, writing the decorations' `Transform`s, then propagates
/// transforms if any moved. Returns how many decorations were moved.
/// Called by the system [`ANCHORS_SYSTEM`]; tools with their own loop (an
/// editor's edit world) can call it directly.
#[profiling::function]
pub fn resolve_anchors(world: &mut World) -> usize {
    let targets = world.resource_or_default::<AnchorTargets>().clone();
    let mut cache = world.remove_resource::<AnchorCache>().unwrap_or_default();
    let anchored: Vec<(Entity, Anchor)> = world
        .query::<(Entity, &Anchor)>()
        .iter()
        .map(|(e, a)| (e, a.clone()))
        .collect();
    cache
        .entries
        .retain(|e, _| anchored.iter().any(|(a, _)| a == e));
    let mut writes: Vec<(Entity, Affine3A)> = Vec::new();
    for (e, anchor) in anchored {
        let target = anchor.target.and_then(|u| world.entity_by_uid(u));
        let target_world = target.map_or(Affine3A::IDENTITY, |t| compute_global(world, t));
        let parent_world = world
            .parent(e)
            .map_or(Affine3A::IDENTITY, |p| compute_global(world, p));
        if let Some(c) = cache.entries.get(&e) {
            let same_target_params = match (target, &c.snapshot) {
                (Some(t), Some(s)) => targets
                    .types
                    .iter()
                    .any(|ty| (ty.same)(world, t, s.as_ref())),
                (None, None) => true,
                _ => false,
            };
            if c.anchor == anchor
                && c.target == target
                && c.target_world == target_world
                && c.parent_world == parent_world
                && same_target_params
            {
                continue;
            }
        }
        let mut snapshot = None;
        let result: Result<Affine3A, String> = match (anchor.target, target) {
            (None, _) => Err("anchor has no target".into()),
            (Some(u), None) => Err(format!("anchor target {u} does not exist")),
            (Some(_), Some(t)) => {
                let mut found = None;
                for ty in &targets.types {
                    if let Some(r) = (ty.frame)(world, t, &anchor.surface, anchor.offset) {
                        snapshot = (ty.snap)(world, t);
                        found = Some(r);
                        break;
                    }
                }
                found.unwrap_or_else(|| {
                    Err(
                        "anchor target has no anchorable generator (Wall, Roof, Platform, Stair)"
                            .into(),
                    )
                })
            }
        };
        let problem = match result {
            Ok(frame) => {
                writes.push((e, parent_world.inverse() * target_world * frame));
                None
            }
            Err(p) => Some(p),
        };
        let old_problem = cache.entries.get(&e).and_then(|c| c.problem.clone());
        if let Some(p) = &problem
            && old_problem.as_ref() != Some(p)
        {
            let name = world
                .uid(e)
                .map_or_else(|| format!("{e:?}"), |u| u.to_string());
            tracing::warn!(target: "box_arch::anchor", "entity {name}: {p}");
        }
        cache.entries.insert(
            e,
            Cached {
                anchor,
                target,
                target_world,
                parent_world,
                snapshot,
                problem,
            },
        );
    }
    let moved = writes.len();
    for (e, local) in writes {
        let (_, rotation, translation) = local.to_scale_rotation_translation();
        if let Some(t) = world.component_mut::<Transform>(e) {
            if t.translation != translation || t.rotation != rotation {
                t.translation = translation;
                t.rotation = rotation;
            }
        } else {
            let _ = world.insert_one(
                e,
                Transform {
                    translation,
                    rotation,
                    ..Transform::IDENTITY
                },
            );
        }
    }
    if moved > 0 {
        box_core::transform::propagate_transforms(world);
    }
    world.insert_resource(cache);
    moved
}

/// A frame from axes and an origin.
fn frame(x: Vec3, y: Vec3, z: Vec3, origin: Vec3) -> Affine3A {
    Affine3A::from_mat3_translation(Mat3::from_cols(x, y, z), origin)
}

fn to3(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

/// A wall's spline in its own space (for spline paths).
fn wall_spline(wall: &Wall, world: &World, entity: Entity) -> Option<Spline> {
    let WallPath::Spline { .. } = wall.path else {
        return None;
    };
    let own = world.uid(entity);
    let uid = wall.spline_uid(own)?;
    let se = world.entity_by_uid(uid)?;
    let spline: Spline = (*world.get::<Spline>(se)?).clone();
    if se == entity {
        return Some(spline);
    }
    let to_local = compute_global(world, entity).inverse() * compute_global(world, se);
    Some(transformed_spline(&spline, &to_local))
}

impl Anchorable for Wall {
    type Snapshot = (Wall, Option<Spline>);

    fn snapshot(&self, world: &World, entity: Entity) -> Self::Snapshot {
        (self.clone(), wall_spline(self, world, entity))
    }

    fn unchanged(&self, world: &World, entity: Entity, old: &Self::Snapshot) -> bool {
        // No clone of the wall; the spline only for spline paths.
        *self == old.0
            && match self.path {
                WallPath::Points { .. } => true,
                WallPath::Spline { .. } => wall_spline(self, world, entity) == old.1,
            }
    }

    fn anchor_frame(
        &self,
        surface: &AnchorSurface,
        offset: Vec3,
        world: &World,
        entity: Entity,
    ) -> Result<Affine3A, String> {
        let spline = wall_spline(self, world, entity);
        let line = self.centreline(spline.as_ref())?;
        let h = self.thickness.max(0.01) * 0.5;
        let top_at = |p: Vec2| match &self.roof {
            Some(r) if !r.surfaces.is_empty() => r.at(p),
            _ => self.height,
        };
        let face = |u: f32, v: f32, w: f32, side: WallSide| {
            let (p, d, n) = line.at(u);
            let (s, x) = match side {
                WallSide::Outer => (1.0, to3(d, 0.0)),
                WallSide::Inner => (-1.0, -to3(d, 0.0)),
            };
            let z = to3(n * s, 0.0);
            let origin = to3(p + n * (s * h), v) + z * w;
            frame(x, Vec3::Y, z, origin)
        };
        match *surface {
            AnchorSurface::Local => Ok(Affine3A::from_translation(offset)),
            AnchorSurface::WallFace { side } => Ok(face(offset.x, offset.y, offset.z, side)),
            AnchorSurface::WallTop => {
                let (p, d, n) = line.at(offset.x);
                let q = p + n * offset.z;
                Ok(frame(
                    to3(d, 0.0),
                    Vec3::Y,
                    to3(n, 0.0),
                    to3(q, top_at(q) + offset.y),
                ))
            }
            AnchorSurface::WallOpening { index, side } => {
                let o = self
                    .openings
                    .get(index as usize)
                    .ok_or_else(|| format!("the wall has no opening {index}"))?;
                Ok(face(
                    o.offset + offset.x,
                    o.sill + o.height + offset.y,
                    offset.z,
                    side,
                ))
            }
            AnchorSurface::RoofRidge => Err("a wall has no ridge".into()),
        }
    }
}

impl Anchorable for Roof {
    type Snapshot = Roof;

    fn snapshot(&self, _world: &World, _entity: Entity) -> Roof {
        self.clone()
    }

    fn unchanged(&self, _world: &World, _entity: Entity, old: &Roof) -> bool {
        self == old
    }

    fn anchor_frame(
        &self,
        surface: &AnchorSurface,
        offset: Vec3,
        _world: &World,
        _entity: Entity,
    ) -> Result<Affine3A, String> {
        match surface {
            AnchorSurface::Local => Ok(Affine3A::from_translation(offset)),
            AnchorSurface::RoofRidge => {
                let tan = pmath::tan(self.pitch.clamp(0.05, 1.4));
                let ov = self.overhang.clamp(0.0, 5.0);
                let up = self.thickness.clamp(0.02, 2.0);
                let ye = -ov * tan;
                let (origin, along_x) = match &self.shape {
                    RoofShape::Gable { size, .. } | RoofShape::Hip { size, .. } => {
                        let ax = self.ridge_along_x().unwrap_or(true);
                        let hb = if ax { size.y } else { size.x } * 0.5;
                        (Vec3::new(0.0, ye + (hb + ov) * tan + up, 0.0), ax)
                    }
                    RoofShape::Shed {
                        size, high_abuts, ..
                    } => {
                        let ax = self.ridge_along_x().unwrap_or(true);
                        let hb = if ax { size.y } else { size.x } * 0.5;
                        let over = if *high_abuts { 0.0 } else { ov };
                        let b = hb + over;
                        let y = ye + (2.0 * hb + ov + over) * tan + up;
                        let o = if ax {
                            Vec3::new(0.0, y, b)
                        } else {
                            Vec3::new(b, y, 0.0)
                        };
                        (o, ax)
                    }
                    RoofShape::Cone { radius, .. } => {
                        let re = radius.max(0.1) + ov;
                        (Vec3::new(0.0, ye + re * tan + up, 0.0), true)
                    }
                    RoofShape::Dome { radius, rise, .. } => {
                        let r = radius.max(0.1);
                        let re = r + ov;
                        let rise = rise.max(0.05);
                        let ye = -rise * (1.0 - (r / re).powi(2)).sqrt();
                        (Vec3::new(0.0, ye + rise + up, 0.0), true)
                    }
                };
                let (x, z) = if along_x {
                    (Vec3::X, Vec3::Z)
                } else {
                    (Vec3::Z, -Vec3::X)
                };
                Ok(frame(
                    x,
                    Vec3::Y,
                    z,
                    origin + x * offset.x + Vec3::Y * offset.y + z * offset.z,
                ))
            }
            other => Err(format!("a roof has no {other:?} surface")),
        }
    }
}

impl Anchorable for crate::Platform {
    type Snapshot = crate::Platform;

    fn snapshot(&self, _world: &World, _entity: Entity) -> Self {
        self.clone()
    }

    fn unchanged(&self, _world: &World, _entity: Entity, old: &Self) -> bool {
        self == old
    }

    fn anchor_frame(
        &self,
        surface: &AnchorSurface,
        offset: Vec3,
        _world: &World,
        _entity: Entity,
    ) -> Result<Affine3A, String> {
        match surface {
            AnchorSurface::Local => Ok(Affine3A::from_translation(offset)),
            other => Err(format!(
                "a platform has no {other:?} surface; use Local (its top is y = 0)"
            )),
        }
    }
}

impl Anchorable for crate::Stair {
    type Snapshot = crate::Stair;

    fn snapshot(&self, _world: &World, _entity: Entity) -> Self {
        self.clone()
    }

    fn unchanged(&self, _world: &World, _entity: Entity, old: &Self) -> bool {
        self == old
    }

    fn anchor_frame(
        &self,
        surface: &AnchorSurface,
        offset: Vec3,
        _world: &World,
        _entity: Entity,
    ) -> Result<Affine3A, String> {
        match surface {
            AnchorSurface::Local => Ok(Affine3A::from_translation(offset)),
            other => Err(format!("a stair has no {other:?} surface; use Local")),
        }
    }
}
