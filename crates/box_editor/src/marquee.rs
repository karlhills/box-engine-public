//! Marquee (rectangle) selection in the viewport.
//!
//! A left drag that starts in empty space (not on a gizmo handle, without
//! Alt) draws a rectangle; every saved entity it touches is selected, live
//! while dragging. Shift adds to the selection, Cmd (Ctrl elsewhere)
//! toggles; Escape restores the selection from before the drag.
//!
//! Hit rule: a mesh is hit when the rectangle intersects the screen outline
//! of its world-space bounding box (the box's eight corners, clipped to the
//! near plane, projected, as a convex polygon). The hit selects the mesh's
//! nearest saved ancestor-or-self, as clicking does. An entity with a
//! viewport icon (light, camera, empty; see `icons.rs`) is hit when the
//! rectangle meets the icon's disc, as clicking it does; with icons off, or
//! for another saved entity without a mesh of its own (a group of meshes),
//! when its origin is inside the rectangle. Flat meshes (a bounds axis thinner than 0.1 mm,
//! such as ground planes; the same test `scene_bounds` uses) are selected
//! only when their whole outline is inside the rectangle, so dragging over
//! the floor does not select the floor. Nothing is visibility-tested:
//! occluded and back-facing objects inside the rectangle are selected too,
//! as in most editors.
//!
//! Cost: while a marquee is dragged, one pass over the mesh entities (eight
//! corner projections each) and the saved entities per frame. Mesh bounds
//! are cached per asset version for the drag; nothing runs otherwise.

use std::collections::{HashMap, HashSet};

use box_core::identity::Uid;
use box_core::math::Aabb;
use box_core::mesh::MeshData;
use box_core::prelude::{Assets, Entity, GlobalTransform, World};
use box_render::MeshRenderer;
use glam::{Vec2, Vec3, Vec4};

use crate::gizmo::GizmoView;
use crate::icons::{ICON_RADIUS, disc_meets_rect, icon_entities};
use crate::state::{EditorState, SelectMode};

/// Clip-space `w` (depth) below which a point counts as behind the camera.
const NEAR_W: f32 = 1e-3;

/// A marquee drag in progress. Points are viewport-local (origin top-left).
#[derive(Clone, Debug)]
pub struct Marquee {
    pub start: Vec2,
    pub current: Vec2,
    pub mode: SelectMode,
    /// The selection when the drag started, restored by Escape and
    /// combined with the hits.
    base: Vec<Uid>,
    /// Local mesh bounds by (asset id, asset version).
    bounds: HashMap<(u64, u64), Aabb>,
}

impl Marquee {
    /// The rectangle as (min, max) in viewport points.
    pub fn rect(&self) -> (Vec2, Vec2) {
        (self.start.min(self.current), self.start.max(self.current))
    }
}

/// Saved entities touched by the rectangle `min..max` (viewport points)
/// seen through `view`, in scene (hierarchy) order. See the module docs
/// for the hit rule.
/// Icons are on ([`crate::icons::ICON_RADIUS`]).
pub fn marquee_hits(world: &World, view: &GizmoView, min: Vec2, max: Vec2) -> Vec<Uid> {
    hits_cached(world, view, min, max, ICON_RADIUS, &mut HashMap::new())
}

fn hits_cached(
    world: &World,
    view: &GizmoView,
    min: Vec2,
    max: Vec2,
    icon_radius: f32,
    cache: &mut HashMap<(u64, u64), Aabb>,
) -> Vec<Uid> {
    let vp = view.view_proj();
    let mut hit: HashSet<Entity> = HashSet::new();
    if let Some(meshes) = world.get_resource::<Assets<MeshData>>() {
        for (e, mr, gt) in world
            .query::<(Entity, &MeshRenderer, &GlobalTransform)>()
            .iter()
        {
            let Some(version) = meshes.version(mr.mesh) else {
                continue;
            };
            let local = *cache
                .entry((mr.mesh.id(), version))
                .or_insert_with(|| meshes.get(mr.mesh).map_or(Aabb::EMPTY, |m| m.bounds()));
            if local.is_empty() {
                continue;
            }
            let corners: [Vec4; 8] = std::array::from_fn(|k| {
                let p = Vec3::new(
                    if k & 1 == 0 { local.min.x } else { local.max.x },
                    if k & 2 == 0 { local.min.y } else { local.max.y },
                    if k & 4 == 0 { local.min.z } else { local.max.z },
                );
                vp * gt.0.transform_point3(p).extend(1.0)
            });
            let flat = local.half_extents().min_element() < 1e-4;
            let touched = if flat {
                box_inside_rect(view, &corners, min, max)
            } else {
                box_meets_rect(view, &corners, min, max)
            };
            if touched && let Some(owner) = saved_owner(world, e) {
                hit.insert(owner);
            }
        }
    }
    let icons: HashSet<Entity> = if icon_radius > 0.0 {
        icon_entities(world).into_iter().map(|(e, ..)| e).collect()
    } else {
        HashSet::new()
    };
    for (e, _, gt) in world.query::<(Entity, &Uid, &GlobalTransform)>().iter() {
        let icon = icons.contains(&e);
        if !icon && (world.has::<MeshRenderer>(e) || box_core::prefab::is_generated(world, e)) {
            continue;
        }
        let radius = if icon { icon_radius } else { 0.0 };
        let clip = vp * Vec3::from(gt.0.translation).extend(1.0);
        if clip.w > NEAR_W
            && let Some(s) = view.clip_to_points(clip)
            && disc_meets_rect(s, radius, min, max)
        {
            hit.insert(e);
        }
    }
    // Scene order, like the hierarchy.
    let mut out = Vec::with_capacity(hit.len());
    let mut stack: Vec<Entity> = world.roots().iter().rev().copied().collect();
    while let Some(e) = stack.pop() {
        if hit.contains(&e)
            && let Some(u) = world.uid(e)
        {
            out.push(u);
        }
        stack.extend(world.children(e).into_iter().rev());
    }
    out
}

/// The nearest ancestor-or-self with a `Uid` that is not instanced prefab
/// content (a prefab's parts select its instance root).
fn saved_owner(world: &World, mut e: Entity) -> Option<Entity> {
    loop {
        if world.uid(e).is_some() && !box_core::prefab::is_generated(world, e) {
            return Some(e);
        }
        e = world.parent(e)?;
    }
}

pub(crate) const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// True if the screen outline of a box (its corners in clip space) meets
/// the rectangle. Edges crossing the near plane are clipped, so a box the
/// camera is inside or next to still has a correct outline.
fn box_meets_rect(view: &GizmoView, corners: &[Vec4; 8], min: Vec2, max: Vec2) -> bool {
    let mut points: Vec<Vec2> = Vec::with_capacity(16);
    let mut push = |c: Vec4| {
        if let Some(p) = view.clip_to_points(c) {
            points.push(p);
        }
    };
    for c in corners {
        if c.w > NEAR_W {
            push(*c);
        }
    }
    for (a, b) in BOX_EDGES {
        let (ca, cb) = (corners[a], corners[b]);
        if (ca.w > NEAR_W) != (cb.w > NEAR_W) {
            let t = (NEAR_W * 2.0 - ca.w) / (cb.w - ca.w);
            push(ca + (cb - ca) * t.clamp(0.0, 1.0));
        }
    }
    let hull = convex_hull(points);
    convex_meets_rect(&hull, min, max)
}

/// True if the whole box (corners in clip space) is in front of the camera
/// and projects inside the rectangle.
fn box_inside_rect(view: &GizmoView, corners: &[Vec4; 8], min: Vec2, max: Vec2) -> bool {
    corners.iter().all(|c| {
        c.w > NEAR_W
            && view
                .clip_to_points(*c)
                .is_some_and(|p| p.cmpge(min).all() && p.cmple(max).all())
    })
}

/// Andrew's monotone chain; counter-clockwise in a y-up frame, without the
/// closing point. Fewer than three distinct points come back as they are.
pub fn convex_hull(mut pts: Vec<Vec2>) -> Vec<Vec2> {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull: Vec<Vec2> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 {
            Box::new(pts.iter())
        } else {
            Box::new(pts.iter().rev())
        };
        for &p in iter {
            while hull.len() >= start + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// Separating-axis test of a convex polygon (or a point or segment) against
/// an axis-aligned rectangle.
pub fn convex_meets_rect(poly: &[Vec2], min: Vec2, max: Vec2) -> bool {
    let Some(&first) = poly.first() else {
        return false;
    };
    let (pmin, pmax) = poly
        .iter()
        .fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    if pmax.x < min.x || pmin.x > max.x || pmax.y < min.y || pmin.y > max.y {
        return false;
    }
    let rect = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    let n = poly.len();
    for i in 0..n {
        let edge = poly[(i + 1) % n] - poly[i];
        if edge.length_squared() < 1e-12 {
            continue;
        }
        let axis = edge.perp();
        let span = |pts: &[Vec2]| {
            pts.iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                    let d = axis.dot(*p);
                    (lo.min(d), hi.max(d))
                })
        };
        let (a0, a1) = span(poly);
        let (b0, b1) = span(&rect);
        if a1 < b0 || b1 < a0 {
            return false;
        }
    }
    true
}

impl EditorState {
    /// Starts a marquee at `start` (viewport points), remembering the
    /// current selection for `mode` and for Escape.
    pub fn marquee_begin(&mut self, start: Vec2, mode: SelectMode) {
        self.viewport.marquee = Some(Marquee {
            start,
            current: start,
            mode,
            base: self.selection.clone(),
            bounds: HashMap::new(),
        });
        self.gizmo.hover = None;
    }

    /// Moves the marquee's free corner to `cursor` and updates the selection
    /// live: the drag-start selection combined with what the rectangle hits.
    pub fn marquee_update(&mut self, cursor: Vec2) {
        let Some(view) = self.gizmo_view() else {
            return;
        };
        let radius = if self.viewport.view.icons {
            ICON_RADIUS
        } else {
            0.0
        };
        let Some(m) = &mut self.viewport.marquee else {
            return;
        };
        // Off the viewport the rectangle stops at its edge.
        m.current = cursor.clamp(Vec2::ZERO, view.size);
        let (min, max) = m.rect();
        let mut hits = hits_cached(&self.world, &view, min, max, radius, &mut m.bounds);
        // Locked and hidden entities are not marquee-selected.
        hits.retain(|u| {
            self.world
                .entity_by_uid(*u)
                .is_none_or(|e| !self.blocked_in_viewport(e))
        });
        let Some(m) = &self.viewport.marquee else {
            return;
        };
        self.selection = m.mode.combine(&m.base, &hits);
    }

    /// Ends the marquee, keeping the selection, and reveals the primary
    /// selection in the hierarchy.
    pub fn marquee_end(&mut self) {
        if self.viewport.marquee.take().is_some()
            && let Some(p) = self.primary()
        {
            self.reveal(p);
        }
    }

    /// Cancels the marquee (Escape): the selection from before it is
    /// restored. Returns false when no marquee was active.
    pub fn marquee_cancel(&mut self) -> bool {
        match self.viewport.marquee.take() {
            Some(m) => {
                self.selection = m.base;
                self.prune_selection();
                true
            }
            None => false,
        }
    }

    /// A whole marquee at once, from corner `a` to corner `b` (viewport
    /// points): what a drag from `a` to `b` with `mode` selects.
    pub fn marquee_select(&mut self, a: Vec2, b: Vec2, mode: SelectMode) {
        self.marquee_begin(a, mode);
        self.marquee_update(b);
        self.marquee_end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_and_rect_overlap() {
        let square = convex_hull(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
            Vec2::new(5.0, 5.0),
        ]);
        assert_eq!(square.len(), 4);
        assert!(convex_meets_rect(
            &square,
            Vec2::new(8.0, 8.0),
            Vec2::new(20.0, 20.0)
        ));
        assert!(!convex_meets_rect(
            &square,
            Vec2::new(11.0, 0.0),
            Vec2::new(20.0, 20.0)
        ));
        // A diamond whose bounding box overlaps the rectangle but whose
        // outline does not: the separating axis catches it.
        let diamond = convex_hull(vec![
            Vec2::new(5.0, 0.0),
            Vec2::new(10.0, 5.0),
            Vec2::new(5.0, 10.0),
            Vec2::new(0.0, 5.0),
        ]);
        assert!(!convex_meets_rect(
            &diamond,
            Vec2::new(8.5, 8.5),
            Vec2::new(12.0, 12.0)
        ));
        assert!(convex_meets_rect(
            &diamond,
            Vec2::new(6.0, 6.0),
            Vec2::new(12.0, 12.0)
        ));
        // Points and segments.
        assert!(convex_meets_rect(
            &[Vec2::new(1.0, 1.0)],
            Vec2::ZERO,
            Vec2::splat(2.0)
        ));
        let seg = [Vec2::new(0.0, 10.0), Vec2::new(10.0, 0.0)];
        assert!(!convex_meets_rect(
            &seg,
            Vec2::new(7.0, 7.0),
            Vec2::new(9.0, 9.0)
        ));
        assert!(convex_meets_rect(
            &seg,
            Vec2::new(4.0, 4.0),
            Vec2::new(6.0, 6.0)
        ));
    }

    #[test]
    fn modes_combine_with_the_base_selection() {
        let u = |n: u128| Uid::from_u128(n);
        let base = [u(1), u(2)];
        let hits = [u(2), u(3)];
        assert_eq!(SelectMode::Replace.combine(&base, &hits), vec![u(2), u(3)]);
        assert_eq!(
            SelectMode::Add.combine(&base, &hits),
            vec![u(1), u(2), u(3)]
        );
        assert_eq!(SelectMode::Toggle.combine(&base, &hits), vec![u(1), u(3)]);
        assert_eq!(SelectMode::Replace.combine(&base, &[]), vec![]);
    }
}
