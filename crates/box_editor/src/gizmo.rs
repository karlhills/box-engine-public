//! Transform gizmos: translate (axis arrows and plane handles), rotate (axis
//! rings) and scale (axis handles and a uniform centre handle).
//!
//! The math works in viewport points (origin top-left, +Y down) and plain
//! glam types, so it is tested without a window. Handles are drawn as
//! display-referred overlay lines (always on top, colours unaffected by the
//! scene's exposure). A drag is one `EditStack` transaction: one undo step.
//! Undo, redo, save and delete end an active drag first; Escape cancels it.
//!
//! With several entities selected the gizmo sits at the pivot ([`Pivot`]:
//! the primary's origin, or the centre of the moved entities) with the
//! primary's axes, and a drag moves the top-most selected entities together
//! (a selected child of a selected parent moves once, with the parent).
//! Rotate turns them about the pivot; scale multiplies each one's local
//! scale and stretches its offset from the pivot along the gizmo axis.

use box_core::color::Color;
use box_core::identity::Uid;
use box_core::math::Ray;
use box_core::prelude::{Entity, GlobalTransform, Transform, World};
use box_core::reflect::PartialReflect;
use box_render::{DebugDraw, LineLayer};
use glam::{Affine3A, Mat4, Quat, Vec2, Vec3};

use crate::camera::EditorCamera;
use crate::picking::camera_ray;
use crate::state::{EditorState, Pivot, Snap, Space, TransformTool};
use crate::theme::Tokens;

/// Axis length on screen, in points.
const GIZMO_POINTS: f32 = 96.0;
/// Pick tolerance around handle lines, in points.
const PICK_POINTS: f32 = 7.0;
/// Plane handles span this fraction of the axis length.
const PLANE_MIN: f32 = 0.22;
const PLANE_MAX: f32 = 0.42;
const RING_SEGMENTS: usize = 64;

/// A grabbable part of the gizmo. Axis indices are 0 = X, 1 = Y, 2 = Z; a plane
/// is named by its normal axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Axis(usize),
    Plane(usize),
    Ring(usize),
    ScaleAxis(usize),
    ScaleUniform,
}

/// The camera and viewport the gizmo is seen through.
#[derive(Clone, Copy, Debug)]
pub struct GizmoView {
    pub camera: EditorCamera,
    /// Viewport size in points.
    pub size: Vec2,
}

impl GizmoView {
    fn aspect(&self) -> f32 {
        self.size.x / self.size.y.max(1.0)
    }

    /// The ray through a viewport point.
    pub fn ray(&self, cursor: Vec2) -> Ray {
        let ndc = Vec2::new(
            cursor.x / self.size.x * 2.0 - 1.0,
            1.0 - cursor.y / self.size.y * 2.0,
        );
        camera_ray(&self.camera, ndc, self.aspect())
    }

    /// World to clip space. Clip `w` is the depth in front of the camera.
    pub fn view_proj(&self) -> Mat4 {
        let view = self.camera.orbit.transform().to_affine().inverse();
        let proj = glam::camera::rh::proj::directx::perspective(
            self.camera.camera.fov_y,
            self.aspect(),
            0.01,
            1000.0,
        );
        proj * Mat4::from(view)
    }

    /// A clip-space point (from [`Self::view_proj`]) in viewport points, or
    /// `None` behind the camera.
    pub fn clip_to_points(&self, clip: glam::Vec4) -> Option<Vec2> {
        if clip.w <= 1e-5 {
            return None;
        }
        let ndc = clip.truncate().truncate() / clip.w;
        Some(Vec2::new(
            (ndc.x + 1.0) * 0.5 * self.size.x,
            (1.0 - ndc.y) * 0.5 * self.size.y,
        ))
    }

    /// A world point in viewport points, or `None` behind the camera.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        self.clip_to_points(self.view_proj() * p.extend(1.0))
    }

    /// World units per viewport point at `p`'s depth.
    pub fn world_per_point(&self, p: Vec3) -> f32 {
        let t = self.camera.orbit.transform();
        let depth = (p - t.translation).dot(t.forward()).max(0.01);
        2.0 * depth * (self.camera.camera.fov_y * 0.5).tan() / self.size.y.max(1.0)
    }
}

/// Where the gizmo sits: origin, unit axes (world or the entity's) and the
/// world length of an axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoFrame {
    pub origin: Vec3,
    pub axes: [Vec3; 3],
    pub size: f32,
}

impl GizmoFrame {
    /// The frame for `global` seen through `view`. Scale handles always use
    /// the entity's local axes.
    pub fn new(global: &Affine3A, space: Space, tool: TransformTool, view: &GizmoView) -> Self {
        let (_, rotation, origin) = global.to_scale_rotation_translation();
        let local = space == Space::Local || tool == TransformTool::Scale;
        let axes = if local {
            let r = rotation.normalize();
            [r * Vec3::X, r * Vec3::Y, r * Vec3::Z]
        } else {
            [Vec3::X, Vec3::Y, Vec3::Z]
        };
        Self {
            origin,
            axes,
            size: view.world_per_point(origin) * GIZMO_POINTS,
        }
    }

    fn plane_corners(&self, normal: usize) -> [Vec3; 4] {
        let (a, b) = (self.axes[(normal + 1) % 3], self.axes[(normal + 2) % 3]);
        let (lo, hi) = (PLANE_MIN * self.size, PLANE_MAX * self.size);
        [
            self.origin + a * lo + b * lo,
            self.origin + a * hi + b * lo,
            self.origin + a * hi + b * hi,
            self.origin + a * lo + b * hi,
        ]
    }

    fn ring_point(&self, axis: usize, t: f32) -> Vec3 {
        let (a, b) = (self.axes[(axis + 1) % 3], self.axes[(axis + 2) % 3]);
        self.origin + (a * t.cos() + b * t.sin()) * self.size
    }
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

fn inside_convex(p: Vec2, quad: &[Vec2; 4]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let c = (b - a).perp_dot(p - a);
        if c.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// The handle under `cursor` (viewport points), if any.
pub fn hit_test(
    tool: TransformTool,
    frame: &GizmoFrame,
    view: &GizmoView,
    cursor: Vec2,
) -> Option<Handle> {
    let o = view.project(frame.origin)?;
    let axis_end = |i: usize| view.project(frame.origin + frame.axes[i] * frame.size);
    let nearest_axis = |make: fn(usize) -> Handle| {
        (0..3)
            .filter_map(|i| Some((segment_distance(cursor, o, axis_end(i)?), make(i))))
            .filter(|(d, _)| *d < PICK_POINTS)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, h)| h)
    };
    match tool {
        TransformTool::Translate => nearest_axis(Handle::Axis).or_else(|| {
            (0..3).find_map(|n| {
                let c = frame.plane_corners(n).map(|p| view.project(p));
                let quad = [c[0]?, c[1]?, c[2]?, c[3]?];
                inside_convex(cursor, &quad).then_some(Handle::Plane(n))
            })
        }),
        TransformTool::Scale => {
            if cursor.distance(o) < PICK_POINTS + 3.0 {
                return Some(Handle::ScaleUniform);
            }
            nearest_axis(Handle::ScaleAxis)
        }
        TransformTool::Rotate => (0..3)
            .filter_map(|i| {
                let pts: Vec<Vec2> =
                    (0..=RING_SEGMENTS)
                        .filter_map(|k| {
                            view.project(frame.ring_point(
                                i,
                                k as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU,
                            ))
                        })
                        .collect();
                let d = pts
                    .windows(2)
                    .map(|w| segment_distance(cursor, w[0], w[1]))
                    .fold(f32::INFINITY, f32::min);
                (d < PICK_POINTS).then_some((d, Handle::Ring(i)))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, h)| h),
    }
}

/// Parameter along the line `origin + t * dir` closest to `ray`, or `None`
/// when they are (nearly) parallel.
fn line_param(origin: Vec3, dir: Vec3, ray: &Ray) -> Option<f32> {
    let a = dir.dot(ray.direction);
    let denom = 1.0 - a * a;
    if denom < 1e-4 {
        return None;
    }
    let w = origin - ray.origin;
    Some((a * ray.direction.dot(w) - dir.dot(w)) / denom)
}

fn plane_hit(origin: Vec3, normal: Vec3, ray: &Ray) -> Option<Vec3> {
    let d = ray.direction.dot(normal);
    if d.abs() < 1e-4 {
        return None;
    }
    let t = (origin - ray.origin).dot(normal) / d;
    (t > 0.0).then(|| ray.at(t))
}

fn snap_to(v: f32, step: f32) -> f32 {
    if step > 0.0 {
        (v / step).round() * step
    } else {
        v
    }
}

/// One entity a gizmo drag transforms, as it was at the press.
#[derive(Clone, Debug)]
pub struct DragTarget {
    pub uid: Uid,
    pub start_local: Transform,
    pub start_global: Affine3A,
    /// The parent's world transform (identity for roots).
    pub parent_global: Affine3A,
}

impl DragTarget {
    fn capture(world: &World, entity: Entity, uid: Uid) -> Option<Self> {
        Some(Self {
            uid,
            start_local: *world.get::<Transform>(entity)?,
            start_global: world.get::<GlobalTransform>(entity).map(|g| g.0)?,
            parent_global: world
                .parent(entity)
                .and_then(|p| world.get::<GlobalTransform>(p).map(|g| g.0))
                .unwrap_or(Affine3A::IDENTITY),
        })
    }
}

/// The world-space change a drag describes so far.
#[derive(Clone, Copy, Debug)]
enum Delta {
    /// Nothing measurable (the cursor ray is parallel to the axis).
    None,
    Translate(Vec3),
    /// Turn about the frame's axis through its origin.
    Rotate(Vec3, f32),
    /// Multiply local scale axis `i` by `k`; offsets from the origin
    /// stretch along the frame's axis `i`.
    ScaleAxis(usize, f32),
    ScaleUniform(f32),
}

/// An active gizmo drag: everything captured at the press. Every target
/// gets the same world-space change about the frame's origin (the pivot);
/// the frame's own entity, sitting at the pivot, only turns or scales.
#[derive(Clone, Debug)]
pub struct GizmoDrag {
    pub handle: Handle,
    /// The entities moved: the top-most selected ones, so a selected child
    /// of a selected parent moves once, with its parent.
    pub targets: Vec<DragTarget>,
    pub frame: GizmoFrame,
    pub start_cursor: Vec2,
    /// The cursor of the latest update (the readout shows its change).
    pub cursor: Vec2,
    /// Axis parameter, plane hit or ring vector at the press.
    start_param: f32,
    start_point: Vec3,
}

impl GizmoDrag {
    /// Starts a drag of `handle` on `entities` (entity, uid) from `cursor`.
    /// Entities without a `Transform` are left out; `None` if none is left.
    pub fn begin(
        world: &World,
        entities: &[(Entity, Uid)],
        handle: Handle,
        frame: GizmoFrame,
        view: &GizmoView,
        cursor: Vec2,
    ) -> Option<Self> {
        let targets: Vec<DragTarget> = entities
            .iter()
            .filter_map(|(e, u)| DragTarget::capture(world, *e, *u))
            .collect();
        if targets.is_empty() {
            return None;
        }
        let ray = view.ray(cursor);
        let (start_param, start_point) = match handle {
            Handle::Axis(i) | Handle::ScaleAxis(i) => {
                (line_param(frame.origin, frame.axes[i], &ray)?, Vec3::ZERO)
            }
            Handle::Plane(n) | Handle::Ring(n) => (
                0.0,
                plane_hit(frame.origin, frame.axes[n], &ray).unwrap_or(frame.origin),
            ),
            Handle::ScaleUniform => (0.0, Vec3::ZERO),
        };
        Some(Self {
            handle,
            targets,
            frame,
            start_cursor: cursor,
            cursor,
            start_param,
            start_point,
        })
    }

    /// The new local transform of every target for the cursor at `cursor`.
    pub fn transforms_at(
        &self,
        view: &GizmoView,
        cursor: Vec2,
        snap: &Snap,
    ) -> Vec<(Uid, Transform)> {
        let delta = self.delta(view, cursor, snap);
        self.targets
            .iter()
            .map(|t| (t.uid, self.apply(delta, t)))
            .collect()
    }

    fn delta(&self, view: &GizmoView, cursor: Vec2, snap: &Snap) -> Delta {
        let ray = view.ray(cursor);
        let f = &self.frame;
        let snapping = snap.enabled;
        match self.handle {
            Handle::Axis(i) => match line_param(f.origin, f.axes[i], &ray) {
                Some(p) => {
                    let mut d = p - self.start_param;
                    if snapping {
                        d = snap_to(d, snap.translate);
                    }
                    Delta::Translate(f.axes[i] * d)
                }
                None => Delta::None,
            },
            Handle::Plane(n) => match plane_hit(f.origin, f.axes[n], &ray) {
                Some(hit) => {
                    let delta = hit - self.start_point;
                    let (a, b) = (f.axes[(n + 1) % 3], f.axes[(n + 2) % 3]);
                    let (mut da, mut db) = (delta.dot(a), delta.dot(b));
                    if snapping {
                        da = snap_to(da, snap.translate);
                        db = snap_to(db, snap.translate);
                    }
                    Delta::Translate(a * da + b * db)
                }
                None => Delta::None,
            },
            Handle::Ring(n) => {
                let axis = f.axes[n];
                let angle = match plane_hit(f.origin, axis, &ray) {
                    Some(hit) => {
                        let v0 = (self.start_point - f.origin).reject_from(axis);
                        let v1 = (hit - f.origin).reject_from(axis);
                        axis.dot(v0.cross(v1)).atan2(v0.dot(v1))
                    }
                    // Edge-on ring: turn with horizontal mouse motion.
                    None => (cursor.x - self.start_cursor.x) * 0.01,
                };
                let angle = if snapping {
                    snap_to(angle.to_degrees(), snap.rotate).to_radians()
                } else {
                    angle
                };
                Delta::Rotate(axis, angle)
            }
            Handle::ScaleAxis(i) => match line_param(f.origin, f.axes[i], &ray) {
                Some(p) if self.start_param.abs() > 1e-4 => {
                    let mut k = p / self.start_param;
                    if snapping {
                        k = 1.0 + snap_to(k - 1.0, snap.scale);
                    }
                    Delta::ScaleAxis(i, k.max(0.01))
                }
                _ => Delta::None,
            },
            Handle::ScaleUniform => {
                let d = (cursor.x - self.start_cursor.x) - (cursor.y - self.start_cursor.y);
                let mut k = (d / 150.0).exp();
                if snapping {
                    k = 1.0 + snap_to(k - 1.0, snap.scale);
                }
                Delta::ScaleUniform(k.max(0.01))
            }
        }
    }

    /// `target`'s local transform after `delta`. A target away from the
    /// pivot also moves: rotation turns its position about the pivot, scale
    /// stretches its offset from the pivot. One at the pivot keeps its
    /// translation exactly.
    fn apply(&self, delta: Delta, target: &DragTarget) -> Transform {
        let mut t = target.start_local;
        let start_pos: Vec3 = target.start_global.translation.into();
        let pivot = self.frame.origin;
        let offset = start_pos - pivot;
        let to_local = |world_pos: Vec3| target.parent_global.inverse().transform_point3(world_pos);
        match delta {
            Delta::None => {}
            Delta::Translate(d) => t.translation = to_local(start_pos + d),
            Delta::Rotate(axis, angle) => {
                let turn = Quat::from_axis_angle(axis, angle);
                let (_, start_rot, _) = target.start_global.to_scale_rotation_translation();
                let (_, parent_rot, _) = target.parent_global.to_scale_rotation_translation();
                t.rotation = (parent_rot.inverse() * (turn * start_rot)).normalize();
                if offset != Vec3::ZERO {
                    t.translation = to_local(pivot + turn * offset);
                }
            }
            Delta::ScaleAxis(i, k) => {
                t.scale[i] = target.start_local.scale[i] * k;
                if offset != Vec3::ZERO {
                    let axis = self.frame.axes[i];
                    t.translation =
                        to_local(pivot + offset + axis * (offset.dot(axis) * (k - 1.0)));
                }
            }
            Delta::ScaleUniform(k) => {
                t.scale = target.start_local.scale * k;
                if offset != Vec3::ZERO {
                    t.translation = to_local(pivot + offset * k);
                }
            }
        }
        t
    }
}

/// Hover and drag state for the viewport.
#[derive(Clone, Debug, Default)]
pub struct GizmoState {
    pub hover: Option<Handle>,
    pub drag: Option<GizmoDrag>,
    /// The drag was ended or cancelled from the keyboard (undo, save, delete,
    /// Escape) with the button still held: the viewport ignores the pointer
    /// until it is released, so the release does not pick or start anything.
    pub(crate) await_release: bool,
    /// Cmd (Ctrl elsewhere) is held during the drag: snapping is inverted
    /// ([`Snap::effective`]). Set by the viewport every frame.
    pub invert_snap: bool,
}

impl EditorState {
    /// The view the gizmo is hit-tested and dragged in (needs the viewport
    /// size, known after its first frame).
    pub fn gizmo_view(&self) -> Option<GizmoView> {
        let rect = self.viewport.rect?;
        Some(GizmoView {
            camera: self.camera,
            size: Vec2::new(rect.width(), rect.height()),
        })
    }

    /// The gizmo frame: the primary selection's axes (or the world's), at
    /// the pivot: the primary's origin, or with [`Pivot::Centre`] and several
    /// entities selected the centre of the ones a drag would move.
    pub fn gizmo_frame(&self, view: &GizmoView) -> Option<GizmoFrame> {
        let e = self.primary_entity()?;
        // A locked entity (or one under a locked parent) gets no gizmo.
        if !self.world.has::<Transform>(e) || self.blocked_in_viewport(e) {
            return None;
        }
        let global = self.world.get::<GlobalTransform>(e)?.0;
        let mut frame = GizmoFrame::new(&global, self.space, self.tool, view);
        if self.pivot == Pivot::Centre
            && self.selection.len() > 1
            && let Some(c) = self.selection_centre()
        {
            frame.origin = c;
            frame.size = view.world_per_point(c) * GIZMO_POINTS;
        }
        Some(frame)
    }

    /// The entities a gizmo drag moves: the top-most selected entities
    /// with a `Transform`, in selection order.
    pub fn gizmo_targets(&self) -> Vec<(Entity, Uid)> {
        self.topmost_selection()
            .into_iter()
            .filter_map(|u| self.world.entity_by_uid(u).map(|e| (e, u)))
            .filter(|(e, _)| {
                self.world.has::<Transform>(*e)
                    && self.world.has::<GlobalTransform>(*e)
                    && !self.blocked_in_viewport(*e)
            })
            .collect()
    }

    /// Mean world position of [`Self::gizmo_targets`].
    pub fn selection_centre(&self) -> Option<Vec3> {
        let targets = self.gizmo_targets();
        let sum = targets
            .iter()
            .filter_map(|(e, _)| self.world.get::<GlobalTransform>(*e))
            .map(|g| Vec3::from(g.0.translation))
            .sum::<Vec3>();
        (!targets.is_empty()).then(|| sum / targets.len() as f32)
    }

    /// Updates the hovered handle for a cursor in viewport points.
    pub fn gizmo_hover(&mut self, cursor: Option<Vec2>) {
        if self.gizmo.drag.is_some() {
            return;
        }
        self.gizmo.hover = match (cursor, self.gizmo_view()) {
            (Some(c), Some(view)) => self
                .gizmo_frame(&view)
                .and_then(|f| hit_test(self.tool, &f, &view, c)),
            _ => None,
        };
    }

    /// Starts dragging the handle under `cursor`. Returns false (and does
    /// nothing) when no handle is there.
    pub fn gizmo_begin(&mut self, cursor: Vec2) -> bool {
        let Some(view) = self.gizmo_view() else {
            return false;
        };
        let Some(frame) = self.gizmo_frame(&view) else {
            return false;
        };
        let Some(handle) = hit_test(self.tool, &frame, &view, cursor) else {
            return false;
        };
        let targets = self.gizmo_targets();
        let Some(drag) = GizmoDrag::begin(&self.world, &targets, handle, frame, &view, cursor)
        else {
            return false;
        };
        let verb = match handle {
            Handle::Axis(_) | Handle::Plane(_) => "Move",
            Handle::Ring(_) => "Rotate",
            Handle::ScaleAxis(_) | Handle::ScaleUniform => "Scale",
        };
        let what = match targets.as_slice() {
            [(e, _)] => self.display_name(*e),
            many => format!("{} entities", many.len()),
        };
        self.begin_edit(format!("{verb} {what}"));
        self.gizmo.await_release = false;
        self.gizmo.hover = Some(handle);
        self.gizmo.drag = Some(drag);
        true
    }

    /// Moves the dragged handle to `cursor`; merges into the open transaction.
    pub fn gizmo_update(&mut self, cursor: Vec2) {
        let (Some(drag), Some(view)) = (self.gizmo.drag.clone(), self.gizmo_view()) else {
            return;
        };
        // An entity or the drag's transaction is gone (the stack was
        // changed behind the drag): end it rather than record loose steps.
        let current = |world: &World, uid: Uid| {
            world
                .entity_by_uid(uid)
                .and_then(|e| world.get::<Transform>(e).map(|c| *c))
        };
        if !self.edits.is_open()
            || drag
                .targets
                .iter()
                .any(|t| current(&self.world, t.uid).is_none())
        {
            self.finish_edit();
            return;
        }
        // One edit per entity: the stack merges each frame's sets.
        let moved: Vec<(Uid, Transform)> = drag
            .transforms_at(&view, cursor, &self.snap.effective(self.gizmo.invert_snap))
            .into_iter()
            .filter(|(uid, t)| current(&self.world, *uid) != Some(*t))
            .collect();
        if moved.is_empty() {
            return;
        }
        if let Some(d) = &mut self.gizmo.drag {
            d.cursor = cursor;
        }
        let Ok(path) = self.world.field_path("Transform") else {
            return;
        };
        let sets = moved.iter().map(|(u, t)| (*u, t as &dyn PartialReflect));
        self.set_fields(&path, sets);
    }

    /// Ends the drag: one undo step if anything moved.
    pub fn gizmo_end(&mut self) {
        if self.gizmo.drag.is_some() {
            self.finish_edit();
            // The button is up: nothing to wait for.
            self.gizmo.await_release = false;
        }
    }

    /// The change of the gizmo drag in progress as shown next to the
    /// cursor: `X  Δ 1.50 m`, `Y  45.0°`, `×1.25`, with the snapping state.
    pub fn gizmo_readout(&self) -> Option<String> {
        let drag = self.gizmo.drag.as_ref()?;
        let view = self.gizmo_view()?;
        let snap = self.snap.effective(self.gizmo.invert_snap);
        let axis = |i: usize| ["X", "Y", "Z"][i % 3];
        let plane = |n: usize| ["YZ", "XZ", "XY"][n % 3];
        let delta = drag.delta(&view, drag.cursor, &snap);
        let f = &drag.frame;
        let text = match (drag.handle, delta) {
            (Handle::Axis(i), Delta::Translate(d)) => {
                format!("{}  Δ {:.2} m", axis(i), d.dot(f.axes[i]))
            }
            (Handle::Plane(n), Delta::Translate(d)) => format!(
                "{}  Δ {:.2}, {:.2} m",
                plane(n),
                d.dot(f.axes[(n + 1) % 3]),
                d.dot(f.axes[(n + 2) % 3])
            ),
            (Handle::Ring(n), Delta::Rotate(_, a)) => {
                format!("{}  {:.1}°", axis(n), a.to_degrees())
            }
            (Handle::ScaleAxis(i), Delta::ScaleAxis(_, k)) => format!("{}  ×{k:.2}", axis(i)),
            (Handle::ScaleUniform, Delta::ScaleUniform(k)) => format!("×{k:.2}"),
            _ => "—".into(),
        };
        let step = match drag.handle {
            Handle::Axis(_) | Handle::Plane(_) => format!("{} m", snap.translate),
            Handle::Ring(_) => format!("{}°", snap.rotate),
            Handle::ScaleAxis(_) | Handle::ScaleUniform => format!("{}", snap.scale),
        };
        Some(if snap.enabled {
            format!("{text}   ·   Snap {step}")
        } else {
            format!("{text}   ·   Snap off")
        })
    }

    /// Queues the gizmo for the primary selection as overlay lines (none
    /// while a viewport tool takes the input; see `tools.rs`).
    pub(crate) fn draw_gizmo(&self, dd: &mut DebugDraw, tokens: &Tokens) {
        if self.tool_wants_input() {
            return;
        }
        let Some(view) = self.gizmo_view() else {
            return;
        };
        let Some(frame) = self.gizmo_frame(&view) else {
            return;
        };
        let active = self
            .gizmo
            .drag
            .as_ref()
            .map(|d| d.handle)
            .or(self.gizmo.hover);
        draw(dd, self.tool, &frame, &view, active, tokens);
    }
}

fn srgb(c: egui::Color32) -> Color {
    Color::srgb(
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
    )
}

const HIGHLIGHT: Color = Color::linear(1.0, 0.72, 0.12);

/// Draws handle lines about two points wide, always on top.
fn draw(
    dd: &mut DebugDraw,
    tool: TransformTool,
    f: &GizmoFrame,
    view: &GizmoView,
    active: Option<Handle>,
    t: &Tokens,
) {
    let cam = view.camera.orbit.transform();
    let wpp = view.world_per_point(f.origin);
    let offsets = [Vec3::ZERO, cam.right() * wpp * 0.5, cam.up() * wpp * 0.5];
    let mut line = |a: Vec3, b: Vec3, c: Color| {
        for o in offsets {
            dd.line_on(LineLayer::DisplayOverlay, a + o, b + o, c);
        }
    };
    let axis_colors = [srgb(t.axis_x), srgb(t.axis_y), srgb(t.axis_z)];
    let pick = |h: Handle, base: Color| if active == Some(h) { HIGHLIGHT } else { base };
    let s = f.size;
    match tool {
        TransformTool::Translate => {
            for (i, &axis_color) in axis_colors.iter().enumerate() {
                let c = pick(Handle::Axis(i), axis_color);
                let (dir, tip) = (f.axes[i], f.origin + f.axes[i] * s);
                line(f.origin + dir * 0.1 * s, tip, c);
                let (p, q) = (f.axes[(i + 1) % 3], f.axes[(i + 2) % 3]);
                let base = tip - dir * 0.2 * s;
                let ring = [p, q, -p, -q].map(|v| base + v * 0.06 * s);
                for k in 0..4 {
                    line(tip, ring[k], c);
                    line(ring[k], ring[(k + 1) % 4], c);
                }
                let c = pick(Handle::Plane(i), axis_color);
                let q = f.plane_corners(i);
                for k in 0..4 {
                    line(q[k], q[(k + 1) % 4], c);
                }
            }
        }
        TransformTool::Rotate => {
            for (i, &axis_color) in axis_colors.iter().enumerate() {
                let c = pick(Handle::Ring(i), axis_color);
                for k in 0..RING_SEGMENTS {
                    let a = k as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    let b = (k + 1) as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                    line(f.ring_point(i, a), f.ring_point(i, b), c);
                }
            }
        }
        TransformTool::Scale => {
            let cube =
                |line: &mut dyn FnMut(Vec3, Vec3, Color), centre: Vec3, half: f32, c: Color| {
                    let corner = |k: u32| {
                        centre
                            + f.axes[0] * if k & 1 == 0 { -half } else { half }
                            + f.axes[1] * if k & 2 == 0 { -half } else { half }
                            + f.axes[2] * if k & 4 == 0 { -half } else { half }
                    };
                    for (a, b) in [
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
                    ] {
                        line(corner(a), corner(b), c);
                    }
                };
            for (i, &axis_color) in axis_colors.iter().enumerate() {
                let c = pick(Handle::ScaleAxis(i), axis_color);
                let end = f.origin + f.axes[i] * s;
                line(f.origin + f.axes[i] * 0.12 * s, end, c);
                cube(&mut line, end, 0.05 * s, c);
            }
            let c = pick(Handle::ScaleUniform, srgb(t.text_strong));
            cube(&mut line, f.origin, 0.07 * s, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> GizmoView {
        let mut camera = EditorCamera::default();
        camera.orbit.target = Vec3::ZERO;
        camera.orbit.distance = 10.0;
        camera.orbit.yaw = 0.6;
        camera.orbit.pitch = 0.4;
        GizmoView {
            camera,
            size: Vec2::new(800.0, 600.0),
        }
    }

    #[test]
    fn projection_and_ray_agree() {
        let v = view();
        let p = Vec3::new(1.0, 0.5, -2.0);
        let s = v.project(p).unwrap();
        let ray = v.ray(s);
        let closest = ray.origin + ray.direction * (p - ray.origin).dot(ray.direction);
        assert!(closest.distance(p) < 1e-3, "{closest} vs {p}");
    }

    #[test]
    fn hit_test_finds_axes_planes_rings() {
        let v = view();
        let f = GizmoFrame::new(
            &Affine3A::IDENTITY,
            Space::World,
            TransformTool::Translate,
            &v,
        );
        let on_x = v.project(f.origin + f.axes[0] * f.size * 0.6).unwrap();
        assert_eq!(
            hit_test(TransformTool::Translate, &f, &v, on_x),
            Some(Handle::Axis(0))
        );
        let plane_mid = f.origin + (f.axes[0] + f.axes[2]) * f.size * 0.32;
        let on_xz = v.project(plane_mid).unwrap();
        assert_eq!(
            hit_test(TransformTool::Translate, &f, &v, on_xz),
            Some(Handle::Plane(1))
        );
        let far = Vec2::new(5.0, 5.0);
        assert_eq!(hit_test(TransformTool::Translate, &f, &v, far), None);
        let on_ring = v.project(f.ring_point(1, 0.3)).unwrap();
        assert_eq!(
            hit_test(TransformTool::Rotate, &f, &v, on_ring),
            Some(Handle::Ring(1))
        );
        let centre = v.project(f.origin).unwrap();
        assert_eq!(
            hit_test(TransformTool::Scale, &f, &v, centre),
            Some(Handle::ScaleUniform)
        );
    }

    fn drag(handle: Handle, from: Vec3, to: Vec3, snap: Snap) -> Transform {
        let v = view();
        let mut world = World::new();
        let uid = Uid::from_u128(1);
        let e = world
            .spawn_saved(uid, "Box", (Transform::default(),))
            .unwrap();
        box_core::transform::propagate_transforms(&mut world);
        let tool = match handle {
            Handle::Ring(_) => TransformTool::Rotate,
            Handle::ScaleAxis(_) | Handle::ScaleUniform => TransformTool::Scale,
            _ => TransformTool::Translate,
        };
        let f = GizmoFrame::new(&Affine3A::IDENTITY, Space::World, tool, &v);
        let (a, b) = (v.project(from).unwrap(), v.project(to).unwrap());
        GizmoDrag::begin(&world, &[(e, uid)], handle, f, &v, a)
            .unwrap()
            .transforms_at(&v, b, &snap)[0]
            .1
    }

    #[test]
    fn axis_drag_moves_along_the_axis_only() {
        let t = drag(
            Handle::Axis(0),
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Snap::default(),
        );
        assert!(
            (t.translation - Vec3::new(1.5, 0.0, 0.0)).length() < 1e-3,
            "{}",
            t.translation
        );
        let snapped = drag(
            Handle::Axis(0),
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(2.2, 0.0, 0.0),
            Snap {
                enabled: true,
                translate: 1.0,
                ..Default::default()
            },
        );
        assert!((snapped.translation.x - 2.0).abs() < 1e-4);
    }

    #[test]
    fn plane_drag_and_rotation_and_scale() {
        let t = drag(
            Handle::Plane(1),
            Vec3::new(0.3, 0.0, 0.3),
            Vec3::new(1.3, 0.0, -0.7),
            Snap::default(),
        );
        assert!(
            (t.translation - Vec3::new(1.0, 0.0, -1.0)).length() < 1e-3,
            "{}",
            t.translation
        );

        let r = drag(
            Handle::Ring(1),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
            Snap::default(),
        );
        let angle = r.rotation.angle_between(Quat::IDENTITY);
        assert!(
            (angle - std::f32::consts::FRAC_PI_2).abs() < 1e-2,
            "{angle}"
        );
        assert!((r.rotation * Vec3::X - Vec3::NEG_Z).length() < 1e-2);

        let s = drag(
            Handle::ScaleAxis(2),
            Vec3::new(0.0, 0.0, 0.5),
            Vec3::new(0.0, 0.0, 1.0),
            Snap::default(),
        );
        assert!(
            (s.scale - Vec3::new(1.0, 1.0, 2.0)).length() < 1e-3,
            "{}",
            s.scale
        );
    }
}
