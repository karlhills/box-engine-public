//! Edit Spline (toolbar "Spline", palette "Edit Spline Tool"): the M5
//! spline editor, built on the viewport tool extension point
//! ([`crate::tools`]), and the thin display line of selected splines.
//!
//! The tool edits the `Spline` of the primary selection, or the spline a
//! selected `Sweep` reads from another entity. While active:
//!
//! - control points show as discs with their index, Bézier tangent handles
//!   as small squares joined to their point;
//! - a press on a point (or handle) selects it and a drag moves it on the
//!   horizontal plane through it (a plane facing the camera when the view is
//!   nearly level); **Shift** moves it up and down only. Each drag is one
//!   undo step. Dragging a handle of a smooth point turns both of its
//!   tangents; a point whose tangents differ (broken in the inspector) keeps
//!   the other one;
//! - **Cmd-click** (Ctrl-click elsewhere) on the curve inserts a point
//!   there (the shape is kept exactly in Bézier and Polyline modes);
//! - **Delete / Backspace** removes the selected point (a spline keeps at
//!   least two);
//! - **Enter** or **Escape** finishes.
//!
//! Every change goes through the `EditStack` (one step per drag, insert or
//! delete). Sweeps that read the spline regenerate during the drag through
//! the generator runtime's throttled flush (at most once per 30 ms).
//!
//! Cost per frame while active: the curve sampled at 24 points per segment
//! for the overlay (well under 0.1 ms for tens of points); hit tests and the
//! insert preview run only when the pointer, the camera or the scene change.

use box_core::color::Color;
use box_core::identity::Uid;
use box_core::math::Ray;
use box_core::prelude::{Entity, GlobalTransform};
use box_engine::box_proc::spline::{ControlPoint, Spline, SplineCurve, SplineMode};
use box_engine::box_proc::sweep_gen::Sweep;
use box_render::{DebugDraw, LineLayer};
use glam::{Affine3A, Vec2, Vec3};

use crate::gizmo::GizmoView;
use crate::shortcuts::{self, Command};
use crate::state::EditorState;
use crate::theme::Tokens;
use crate::tools::{ToolEvent, ToolEventKind, ToolResponse, ViewportTool};

/// Pick radius of points and handles, in points.
const HIT: f32 = 9.0;
/// How close to the curve a Cmd-click inserts, in points.
const INSERT_HIT: f32 = 12.0;
/// Overlay samples per curve segment.
const OVERLAY_SAMPLES: u32 = 24;

/// What a press can grab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    Point(usize),
    /// The handle at `position - in_tangent / 3`.
    In(usize),
    /// The handle at `position + out_tangent / 3`.
    Out(usize),
}

impl Grab {
    fn index(self) -> usize {
        match self {
            Self::Point(i) | Self::In(i) | Self::Out(i) => i,
        }
    }
}

/// The spline being edited.
pub(crate) struct Target {
    pub uid: Uid,
    pub entity: Entity,
    pub spline: Spline,
    /// The spline entity's world transform (its points are local to it).
    pub global: Affine3A,
}

impl Target {
    fn world(&self, local: Vec3) -> Vec3 {
        self.global.transform_point3(local)
    }

    /// The curve, and whether its segments map 1:1 to the control points
    /// (no duplicate or non-finite point was dropped), which handle and
    /// insert positions need.
    fn curve(&self) -> (SplineCurve, bool) {
        let curve = self.spline.curve();
        let n = self.spline.points.len();
        let expected = match n {
            0 | 1 => 0,
            n if self.spline.closed && n >= 3 => n,
            n => n - 1,
        };
        let aligned = curve.segment_count() == expected;
        (curve, aligned)
    }

    /// Points and Bézier handles with their world positions.
    fn handles(&self) -> Vec<(Grab, Vec3)> {
        let mut out: Vec<(Grab, Vec3)> = self
            .spline
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| (Grab::Point(i), self.world(p.position)))
            .collect();
        if self.spline.mode != SplineMode::Bezier {
            return out;
        }
        let (curve, aligned) = self.curve();
        if !aligned {
            return out;
        }
        let n = self.spline.points.len();
        for (k, seg) in curve.segments().iter().enumerate() {
            out.push((Grab::Out(k), self.world(seg.p[1])));
            out.push((Grab::In((k + 1) % n), self.world(seg.p[2])));
        }
        out
    }

    /// World polyline of the curve for overlays.
    fn polyline(&self, per_segment: u32) -> Vec<Vec3> {
        let curve = self.spline.curve();
        let segs = curve.segment_count();
        if segs == 0 {
            return Vec::new();
        }
        let steps = segs * per_segment as usize;
        (0..=steps)
            .map(|j| self.world(curve.position(j as f32 / per_segment as f32)))
            .collect()
    }
}

/// The spline the primary selection edits: its own `Spline`, else the one
/// its `Sweep` reads from another entity.
pub(crate) fn target(state: &EditorState) -> Option<Target> {
    let e = state.primary_entity()?;
    let uid = if state.world.has::<Spline>(e) {
        state.world.uid(e)?
    } else {
        state.world.get::<Sweep>(e)?.spline?
    };
    target_of(state, uid)
}

fn target_of(state: &EditorState, uid: Uid) -> Option<Target> {
    let entity = state.world.entity_by_uid(uid)?;
    let spline = Spline::clone(&*state.world.get::<Spline>(entity)?);
    let global = state
        .world
        .get::<GlobalTransform>(entity)
        .map_or(Affine3A::IDENTITY, |g| g.0);
    Some(Target {
        uid,
        entity,
        spline,
        global,
    })
}

/// A drag in progress.
#[derive(Clone, Copy, Debug)]
struct Drag {
    uid: Uid,
    grab: Grab,
    /// Shift: up and down only.
    vertical: bool,
    /// The handle's world position when the constraint was last anchored,
    /// and where the cursor's ray met the constraint then.
    anchor: Vec3,
    anchor_hit: Option<Vec3>,
    /// The transaction is open (the handle moved).
    moved: bool,
}

/// Where a Cmd-click would insert a point.
#[derive(Clone, Debug, PartialEq)]
struct Insert {
    uid: Uid,
    /// The new spline (the point inserted, neighbouring tangents adjusted).
    points: Vec<ControlPoint>,
    index: usize,
    world: Vec3,
}

/// The Edit Spline tool (see the module docs).
#[derive(Debug, Default)]
pub struct SplineTool {
    /// The selected point: (spline entity, index).
    selected: Option<(Uid, usize)>,
    hover: Option<Grab>,
    drag: Option<Drag>,
    insert: Option<Insert>,
}

impl SplineTool {
    fn selected_in(&self, t: &Target) -> Option<usize> {
        self.selected
            .filter(|(u, i)| *u == t.uid && *i < t.spline.points.len())
            .map(|(_, i)| i)
    }
}

/// The handle under `cursor` (points before handles), within [`HIT`].
fn hit(t: &Target, view: &GizmoView, cursor: Vec2) -> Option<Grab> {
    let mut best: Option<(Grab, f32, bool)> = None;
    for (g, w) in t.handles() {
        let Some(s) = view.project(w) else { continue };
        let d = s.distance(cursor);
        if d > HIT {
            continue;
        }
        let point = matches!(g, Grab::Point(_));
        let better = match best {
            None => true,
            Some((_, bd, bp)) => (point && !bp) || (point == bp && d < bd),
        };
        if better {
            best = Some((g, d, point));
        }
    }
    best.map(|(g, ..)| g)
}

/// Where `ray` meets the drag constraint through `anchor`: the horizontal
/// plane (or, for a nearly level view, the plane facing the camera), or
/// with `vertical` the vertical line through it.
pub(crate) fn constrained(
    ray: &Ray,
    anchor: Vec3,
    vertical: bool,
    view: &GizmoView,
) -> Option<Vec3> {
    let (o, d) = (ray.origin, ray.direction);
    if vertical {
        // Closest point on the line anchor + Y·s to the ray.
        let w = anchor - o;
        let b = Vec3::Y.dot(d);
        let denom = 1.0 - b * b;
        if denom < 1e-6 {
            return None;
        }
        let s = (b * d.dot(w) - Vec3::Y.dot(w)) / denom;
        return Some(anchor + Vec3::Y * s);
    }
    let normal = if d.y.abs() > 0.08 {
        Vec3::Y
    } else {
        -view.camera.orbit.transform().forward()
    };
    let denom = d.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (anchor - o).dot(normal) / denom;
    (t > 0.0).then(|| o + d * t)
}

/// Point `i` with `grab` moved to the local position `local`.
fn moved_point(p: &ControlPoint, grab: Grab, local: Vec3, effective: (Vec3, Vec3)) -> ControlPoint {
    let mut p = *p;
    // Smooth: no tangents, one (mirrored) or two equal ones.
    let smooth = match (p.in_tangent, p.out_tangent) {
        (Some(a), Some(b)) => a.abs_diff_eq(b, 1e-5),
        _ => true,
    };
    match grab {
        Grab::Point(_) => p.position = local,
        Grab::Out(_) => {
            let t = (local - p.position) * 3.0;
            p.out_tangent = Some(t);
            p.in_tangent = Some(if smooth {
                t
            } else {
                p.in_tangent.unwrap_or(effective.0)
            });
        }
        Grab::In(_) => {
            let t = (p.position - local) * 3.0;
            p.in_tangent = Some(t);
            p.out_tangent = Some(if smooth {
                t
            } else {
                p.out_tangent.unwrap_or(effective.1)
            });
        }
    }
    p
}

/// The tangents point `i` has now, as drawn: (in, out).
fn effective_tangents(t: &Target, i: usize) -> (Vec3, Vec3) {
    let (curve, aligned) = t.curve();
    let p = t.spline.points[i];
    let n = t.spline.points.len();
    let segs = curve.segments();
    let out = (aligned && i < segs.len()).then(|| (segs[i].p[1] - segs[i].p[0]) * 3.0);
    let prev = if i > 0 {
        Some(i - 1)
    } else if t.spline.closed && n >= 3 {
        Some(n - 1)
    } else {
        None
    };
    let inn = prev
        .filter(|k| aligned && *k < segs.len())
        .map(|k| (segs[k].p[3] - segs[k].p[2]) * 3.0);
    let fallback = p.out_tangent.or(p.in_tangent).unwrap_or(Vec3::ZERO);
    let inn = inn.or(p.in_tangent).or(out).unwrap_or(fallback);
    let out = out.or(p.out_tangent).unwrap_or(inn);
    (inn, out)
}

/// The insert position nearest `cursor` on the curve, within [`INSERT_HIT`].
fn insertion(t: &Target, view: &GizmoView, cursor: Vec2) -> Option<Insert> {
    let (curve, aligned) = t.curve();
    if !aligned || curve.segment_count() == 0 {
        return None;
    }
    const PER: usize = 32;
    let segs = curve.segment_count();
    let mut best: Option<(f32, f32)> = None; // (distance, t)
    let mut prev: Option<(f32, Vec2)> = None;
    for j in 0..=segs * PER {
        let tt = j as f32 / PER as f32;
        let Some(s) = view.project(t.world(curve.position(tt))) else {
            prev = None;
            continue;
        };
        if let Some((pt, ps)) = prev {
            let seg = s - ps;
            let f = if seg.length_squared() > 0.0 {
                ((cursor - ps).dot(seg) / seg.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = (ps + seg * f).distance(cursor);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, pt + (tt - pt) * f));
            }
        }
        prev = Some((tt, s));
    }
    let (d, tt) = best?;
    if d > INSERT_HIT {
        return None;
    }
    let (k, u) = curve.locate(tt);
    // Not on a control point: nothing to insert there.
    if !(1e-3..=1.0 - 1e-3).contains(&u) {
        return None;
    }
    let n = t.spline.points.len();
    let (a, b) = (k, (k + 1) % n);
    let mut points = t.spline.points.clone();
    let mut new = ControlPoint::new(curve.position(tt))
        .with_roll(curve.roll(tt))
        .with_scale(curve.scale(tt));
    if t.spline.mode == SplineMode::Bezier {
        // de Casteljau split: both halves keep the segment's exact shape.
        let [p0, p1, p2, p3] = curve.segments()[k].p;
        let lerp = |x: Vec3, y: Vec3| x + (y - x) * u;
        let (q0, q1, q2) = (lerp(p0, p1), lerp(p1, p2), lerp(p2, p3));
        let (r0, r1) = (lerp(q0, q1), lerp(q1, q2));
        let s = lerp(r0, r1);
        let (a_in, _) = effective_tangents(t, a);
        let (_, b_out) = effective_tangents(t, b);
        points[a].in_tangent = Some(a_in);
        points[a].out_tangent = Some((q0 - p0) * 3.0);
        points[b].in_tangent = Some((p3 - q2) * 3.0);
        points[b].out_tangent = Some(b_out);
        new.position = s;
        new.in_tangent = Some((s - r0) * 3.0);
        new.out_tangent = Some((r1 - s) * 3.0);
    }
    let index = k + 1;
    points.insert(index, new);
    Some(Insert {
        uid: t.uid,
        points,
        index,
        world: t.world(curve.position(tt)),
    })
}

fn cmd_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}

impl SplineTool {
    fn press(&mut self, state: &mut EditorState, e: &ToolEvent) {
        let (Some(t), Some(view)) = (target(state), state.gizmo_view()) else {
            return;
        };
        let Some(grab) = hit(&t, &view, e.cursor) else {
            return;
        };
        self.selected = Some((t.uid, grab.index()));
        let anchor = t
            .handles()
            .into_iter()
            .find(|(g, _)| *g == grab)
            .map_or(Vec3::ZERO, |(_, w)| w);
        let vertical = e.modifiers.shift;
        self.drag = Some(Drag {
            uid: t.uid,
            grab,
            vertical,
            anchor,
            anchor_hit: constrained(&e.ray, anchor, vertical, &view),
            moved: false,
        });
        self.insert = None;
    }

    fn drag_to(&mut self, state: &mut EditorState, e: &ToolEvent) {
        let Some(mut d) = self.drag else { return };
        let (Some(t), Some(view)) = (target_of(state, d.uid), state.gizmo_view()) else {
            return;
        };
        let i = d.grab.index();
        let Some(p) = t.spline.points.get(i).copied() else {
            return;
        };
        let current = t
            .handles()
            .into_iter()
            .find(|(g, _)| *g == d.grab)
            .map(|(_, w)| w);
        // Shift pressed or released: continue from where the handle is.
        if e.modifiers.shift != d.vertical
            && let Some(c) = current
        {
            d.vertical = e.modifiers.shift;
            d.anchor = c;
            d.anchor_hit = constrained(&e.ray, c, d.vertical, &view);
        }
        if d.anchor_hit.is_none() {
            d.anchor_hit = constrained(&e.ray, d.anchor, d.vertical, &view);
        }
        let (Some(start), Some(now)) = (
            d.anchor_hit,
            constrained(&e.ray, d.anchor, d.vertical, &view),
        ) else {
            self.drag = Some(d);
            return;
        };
        let world = d.anchor + (now - start);
        let local = t.global.inverse().transform_point3(world);
        let new = moved_point(&p, d.grab, local, effective_tangents(&t, i));
        if new != p && new.position.is_finite() {
            if !d.moved {
                let what = match d.grab {
                    Grab::Point(_) => format!("Move Spline Point {i}"),
                    Grab::In(_) | Grab::Out(_) => format!("Turn Spline Tangent {i}"),
                };
                state.begin_edit(what);
                d.moved = true;
            }
            state.set_field(d.uid, &format!("Spline.points[{i}]"), &new);
        }
        self.drag = Some(d);
    }

    fn release(&mut self, state: &mut EditorState) {
        if let Some(d) = self.drag.take()
            && d.moved
        {
            state.finish_edit();
        }
    }

    fn click(&mut self, state: &mut EditorState, e: &ToolEvent) {
        let had_drag = self.drag.is_some();
        self.release(state);
        if had_drag {
            // A press on a handle without a drag only selected it.
            return;
        }
        if e.modifiers.command {
            let insert = match (target(state), state.gizmo_view()) {
                (Some(t), Some(view)) => insertion(&t, &view, e.cursor),
                _ => None,
            };
            if let Some(ins) = insert {
                state.transaction(format!("Add Spline Point {}", ins.index), |s| {
                    s.set_field(ins.uid, "Spline.points", &ins.points);
                });
                self.selected = Some((ins.uid, ins.index));
            }
            self.insert = None;
            return;
        }
        self.selected = None;
    }

    fn delete_selected(&mut self, state: &mut EditorState) {
        let Some(t) = target(state) else { return };
        let Some(i) = self.selected_in(&t) else {
            state.notify("Click a point to select it, then press Delete to remove it");
            return;
        };
        if t.spline.points.len() <= 2 {
            state.notify("A spline needs at least two points");
            return;
        }
        let mut points = t.spline.points.clone();
        points.remove(i);
        state.transaction(format!("Delete Spline Point {i}"), |s| {
            s.set_field(t.uid, "Spline.points", &points);
        });
        self.selected = Some((t.uid, i.min(points.len() - 1)));
    }
}

impl ViewportTool for SplineTool {
    fn id(&self) -> &'static str {
        "edit_spline"
    }

    fn label(&self) -> &'static str {
        "Spline"
    }

    fn name(&self) -> &'static str {
        "Edit Spline"
    }

    fn tooltip(&self) -> &'static str {
        "Shape the selected spline: drag its points (Shift: up and down), \
         Cmd/Ctrl-click the curve to add one, Delete removes one. Enter or Esc when done."
    }

    fn wants_input(&self, state: &EditorState) -> bool {
        target(state).is_some_and(|t| !state.blocked_in_viewport(t.entity))
    }

    fn on_pointer(&mut self, state: &mut EditorState, e: &ToolEvent) -> ToolResponse {
        match e.kind {
            ToolEventKind::Hover => {
                if self.drag.is_some() && e.primary_down {
                    self.drag_to(state, e);
                } else {
                    let (t, view) = (target(state), state.gizmo_view());
                    let (t, view) = match (t, view) {
                        (Some(t), Some(v)) => (t, v),
                        _ => return ToolResponse::Handled,
                    };
                    self.hover = hit(&t, &view, e.cursor);
                    self.insert = if e.modifiers.command && self.hover.is_none() {
                        insertion(&t, &view, e.cursor)
                    } else {
                        None
                    };
                }
            }
            ToolEventKind::Press => self.press(state, e),
            ToolEventKind::Release => self.release(state),
            ToolEventKind::Click => self.click(state, e),
        }
        ToolResponse::Handled
    }

    fn on_key(
        &mut self,
        state: &mut EditorState,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        let delete = shortcuts::shortcut(Command::Delete)
            .keys
            .iter()
            .any(|k| k.logical_key == key && modifiers.matches_exact(k.modifiers));
        if delete && self.drag.is_none() && self.wants_input(state) {
            self.delete_selected(state);
            return true;
        }
        false
    }

    fn cancel(&mut self, state: &mut EditorState) {
        if let Some(d) = self.drag.take()
            && d.moved
        {
            state.rollback_edit();
        }
        self.hover = None;
        self.insert = None;
    }

    fn draw_overlay(&self, state: &EditorState, dd: &mut DebugDraw, tokens: &Tokens) {
        let Some(t) = target(state) else { return };
        let accent = crate::tools::color(tokens.accent);
        draw_curve(dd, &t.polyline(OVERLAY_SAMPLES), accent);
        if t.spline.mode == SplineMode::Bezier {
            let handle_line = Color::srgb(0.08, 0.08, 0.08).with_alpha(0.8);
            let points: Vec<Vec3> = t
                .spline
                .points
                .iter()
                .map(|p| t.world(p.position))
                .collect();
            for (g, w) in t.handles() {
                let i = match g {
                    Grab::In(i) | Grab::Out(i) => i,
                    Grab::Point(_) => continue,
                };
                dd.line_on(LineLayer::DisplayOverlay, points[i], w, handle_line);
            }
        }
    }

    fn paint(&self, state: &EditorState, painter: &egui::Painter, viewport: egui::Rect) {
        let (Some(t), Some(view)) = (target(state), state.gizmo_view()) else {
            return;
        };
        let tk = crate::theme::tokens(painter.ctx());
        let selected = self.selected_in(&t);
        let at = |w: Vec3| view.project(w).map(|s| viewport.min + egui::vec2(s.x, s.y));
        let dragged = self.drag.map(|d| d.grab);
        for (g, w) in t.handles() {
            let Some(p) = at(w) else { continue };
            let lit = self.hover == Some(g) || dragged == Some(g);
            match g {
                Grab::Point(i) => {
                    let is_sel = selected == Some(i);
                    let r = if lit { 6.5 } else { 5.5 };
                    let fill = if is_sel {
                        tk.accent
                    } else {
                        egui::Color32::WHITE
                    };
                    painter.circle_filled(p, r, fill);
                    painter.circle_stroke(
                        p,
                        r,
                        egui::Stroke::new(1.5, egui::Color32::from_black_alpha(200)),
                    );
                    // The index on a small dark pill, readable on any surface.
                    let g = painter.layout_no_wrap(
                        i.to_string(),
                        egui::FontId::proportional(11.0),
                        tk.overlay_text,
                    );
                    let r = egui::Rect::from_min_size(
                        p + egui::vec2(7.0, -7.0 - g.size().y - 2.0),
                        g.size() + egui::vec2(6.0, 2.0),
                    );
                    painter.rect_filled(r, 3.0, tk.overlay_bg);
                    painter.galley(r.min + egui::vec2(3.0, 1.0), g, tk.overlay_text);
                }
                Grab::In(_) | Grab::Out(_) => {
                    let s = if lit { 4.5 } else { 3.5 };
                    let r = egui::Rect::from_center_size(p, egui::vec2(s * 2.0, s * 2.0));
                    painter.rect_filled(
                        r,
                        1.0,
                        if lit {
                            tk.accent
                        } else {
                            egui::Color32::from_gray(225)
                        },
                    );
                    painter.rect_stroke(
                        r,
                        1.0,
                        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(200)),
                        egui::StrokeKind::Outside,
                    );
                }
            }
        }
        if let Some(ins) = &self.insert
            && ins.uid == t.uid
            && let Some(p) = at(ins.world)
        {
            painter.circle_stroke(p, 6.0, egui::Stroke::new(1.5, tk.accent));
            painter.line_segment(
                [p - egui::vec2(3.5, 0.0), p + egui::vec2(3.5, 0.0)],
                egui::Stroke::new(1.5, tk.accent),
            );
            painter.line_segment(
                [p - egui::vec2(0.0, 3.5), p + egui::vec2(0.0, 3.5)],
                egui::Stroke::new(1.5, tk.accent),
            );
        }
    }

    fn hint(&self, state: &EditorState) -> Option<String> {
        let cmd = cmd_name();
        let Some(t) = target(state) else {
            return Some(
                "Edit Spline  ·  select a spline (or a sweep) to edit  ·  Esc: Move".into(),
            );
        };
        let n = t.spline.points.len();
        let which = match self.selected_in(&t) {
            Some(i) => format!("point {i} of {n}"),
            None => format!("{n} points"),
        };
        Some(format!(
            "Edit Spline  ·  {which}  ·  drag a point (Shift: height)  ·  {cmd}click the curve to add  ·  Del removes  ·  Enter: done"
        ))
    }
}

fn draw_curve(dd: &mut DebugDraw, line: &[Vec3], color: Color) {
    for w in line.windows(2) {
        dd.line_on(LineLayer::DisplayOverlay, w[0], w[1], color);
    }
}

/// Thin lines along the selected splines (and the splines selected sweeps
/// read) while the Edit Spline tool is not editing: 16 samples per segment,
/// only for the selection.
pub(crate) fn draw_selected_splines(state: &EditorState, dd: &mut DebugDraw, tokens: &Tokens) {
    if state.active_tool() == Some("edit_spline") && state.tool_wants_input() {
        return;
    }
    let color = crate::tools::color(tokens.accent).with_alpha(0.6);
    let mut drawn: Vec<Uid> = Vec::new();
    for &uid in &state.selection {
        let Some(e) = state.world.entity_by_uid(uid) else {
            continue;
        };
        let spline_uid = if state.world.has::<Spline>(e) {
            Some(uid)
        } else {
            state.world.get::<Sweep>(e).and_then(|s| s.spline)
        };
        let Some(spline_uid) = spline_uid.filter(|u| !drawn.contains(u)) else {
            continue;
        };
        drawn.push(spline_uid);
        if let Some(t) = target_of(state, spline_uid) {
            draw_curve(dd, &t.polyline(16), color);
        }
    }
}
