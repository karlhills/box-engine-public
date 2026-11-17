//! Paint Scatter (toolbar "Paint", palette "Paint Scatter Tool"): a brush
//! that paints the mask of the selected `Scatter` (M6), built on the
//! viewport tool extension point ([`crate::tools`]).
//!
//! While active, with an entity that has a `Scatter` as the primary
//! selection:
//!
//! - a circle on the scatter's ground follows the cursor (its radius is the
//!   brush; the inner circle shows the strength), in the accent colour
//!   when adding and the warning colour when erasing;
//! - **drag** paints (cells move towards full weight by the strength, with
//!   a smooth falloff); **Shift** while dragging, or **X** to switch,
//!   erases; **[ / ]** change the radius, **− / =** the strength;
//! - **Escape** cancels a stroke in progress (and leaves the tool).
//!
//! **Stamp and flush.** Each pointer move stamps dabs (spaced a quarter of
//! the radius along the path) into a working copy of the mask held by the
//! tool: O(cells under the brush) per dab, no world access. The working
//! mask is written to the `Scatter` (`Scatter.mask` through the
//! `EditStack`) at most once per regeneration throttle (30 ms, the clock
//! of `ProcSettings`) and once more on release, and the scatter runtime
//! then places again only the chunks whose tiles changed (its own throttled
//! flush). A whole stroke is one undo step ("Paint Scatter" or "Erase
//! Scatter"). Per-frame cost while dragging: the dabs, plus one mask write
//! and the changed chunks' placement every other frame at 60 Hz
//! (`crates/box_editor/tests/scatter_brush.rs` prints the numbers); the
//! overlay is a 48-point ring sampled on the ground.

use std::time::{Duration, Instant};

use box_core::identity::Uid;
use box_core::math::Ray;
use box_core::prelude::{Entity, GlobalTransform};
use box_engine::box_proc::scatter::{Scatter, ScatterMask, ScatterSurface};
use box_engine::box_proc::{ProcClock, ProcSettings};
use box_engine::scatter_glue::ScatterRuntime;
use box_render::{DebugDraw, LineLayer};
use glam::{Affine3A, Vec2, Vec3};

use crate::shortcuts::{self, Command};
use crate::state::EditorState;
use crate::theme::Tokens;
use crate::tools::{ToolEvent, ToolEventKind, ToolResponse, ViewportTool};

/// Brush radius limits, metres.
const RADIUS: (f32, f32) = (0.1, 50.0);
/// Ring points of the cursor.
const RING: usize = 48;

/// Where the brush is on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hit {
    uid: Uid,
    /// Owner-local XZ point.
    local: Vec2,
    world: Vec3,
}

/// A stroke in progress.
#[derive(Debug)]
struct Stroke {
    uid: Uid,
    erase: bool,
    /// The working mask, ahead of the world's.
    mask: ScatterMask,
    area: Vec<Vec2>,
    /// The last dab (owner-local).
    last: Vec2,
    /// Dabs not yet written to the world.
    dirty: bool,
    /// When the mask was last written (the throttle's clock).
    written: Option<Duration>,
    /// The transaction is open.
    open: bool,
    dabs: usize,
}

/// The Paint Scatter tool (see the module docs).
#[derive(Debug)]
pub struct ScatterBrush {
    /// Brush radius in metres.
    pub radius: f32,
    /// How far one dab moves cells towards full (or zero), 0 to 1.
    pub strength: f32,
    /// Erase instead of add (Shift inverts while dragging).
    pub erase: bool,
    hover: Option<Hit>,
    /// Shift as of the last pointer event (the overlay's colour).
    shift: bool,
    stroke: Option<Stroke>,
    epoch: Instant,
}

impl Default for ScatterBrush {
    fn default() -> Self {
        Self {
            radius: 2.0,
            strength: 0.5,
            erase: false,
            hover: None,
            shift: false,
            stroke: None,
            epoch: Instant::now(),
        }
    }
}

/// The scatter the brush paints: the primary selection's.
fn target(state: &EditorState) -> Option<(Uid, Entity)> {
    let e = state.primary_entity()?;
    state
        .world
        .has::<Scatter>(e)
        .then(|| state.world.uid(e).map(|u| (u, e)))
        .flatten()
}

fn global(state: &EditorState, e: Entity) -> Affine3A {
    state
        .world
        .get::<GlobalTransform>(e)
        .map_or(Affine3A::IDENTITY, |g| g.0)
}

/// Runs `f` with the scatter's ground (the plane before its first flush).
fn with_surface<R>(state: &EditorState, e: Entity, f: impl FnOnce(&ScatterSurface) -> R) -> R {
    match state
        .world
        .get_resource::<ScatterRuntime>()
        .and_then(|rt| rt.surface(e))
    {
        Some(s) => f(s),
        None => f(&ScatterSurface::Flat),
    }
}

/// Where `ray` meets the ground of the scatter on `e`.
fn hit(state: &EditorState, uid: Uid, e: Entity, ray: &Ray) -> Option<Hit> {
    let g = global(state, e);
    let inv = g.inverse();
    let o = inv.transform_point3(ray.origin);
    let d = inv.transform_vector3(ray.direction);
    let (t, _) = with_surface(state, e, |s| s.raycast(o, d))?;
    let local = o + d * t;
    Some(Hit {
        uid,
        local: Vec2::new(local.x, local.z),
        world: g.transform_point3(local),
    })
}

impl ScatterBrush {
    fn now(&self, state: &EditorState) -> (Duration, Duration) {
        let settings = state
            .world
            .get_resource::<ProcSettings>()
            .cloned()
            .unwrap_or_default();
        let now = match settings.clock {
            ProcClock::Manual(t) => t,
            ProcClock::Real => self.epoch.elapsed(),
        };
        (now, settings.throttle)
    }

    /// Stamps dabs from the last one to `to`, a quarter radius apart.
    fn stamp_to(&mut self, to: Vec2) {
        let (radius, strength) = (self.radius, self.strength);
        let Some(s) = self.stroke.as_mut() else {
            return;
        };
        let spacing = (radius * 0.25).max(0.01);
        let span = to - s.last;
        let steps = (span.length() / spacing).floor() as usize;
        for k in 1..=steps.min(4096) {
            let p = s.last + span * (k as f32 * spacing / span.length());
            s.dirty |= !s
                .mask
                .paint(&s.area, p, radius, strength, s.erase)
                .is_empty();
            s.dabs += 1;
        }
        if steps > 0 {
            s.last += span * (steps as f32 * spacing / span.length());
        }
    }

    /// Writes the working mask to the world when due (`force`: now).
    fn flush(&mut self, state: &mut EditorState, force: bool) {
        let (now, throttle) = self.now(state);
        let Some(s) = self.stroke.as_mut() else {
            return;
        };
        if !s.dirty {
            return;
        }
        let due = s.written.is_none_or(|w| now.saturating_sub(w) >= throttle);
        if !force && !due {
            return;
        }
        if !s.open {
            state.begin_edit(if s.erase {
                "Erase Scatter"
            } else {
                "Paint Scatter"
            });
            s.open = true;
        }
        state.set_field(s.uid, "Scatter.mask", &s.mask);
        s.dirty = false;
        s.written = Some(now);
    }

    fn press(&mut self, state: &mut EditorState, e: &ToolEvent) {
        let Some((uid, entity)) = target(state) else {
            return;
        };
        let Some(h) = hit(state, uid, entity, &e.ray) else {
            return;
        };
        let Some(scatter) = state.world.get::<Scatter>(entity).map(|s| (*s).clone()) else {
            return;
        };
        let erase = self.erase != e.modifiers.shift;
        let mut mask = scatter.mask;
        let dirty = !mask
            .paint(&scatter.area, h.local, self.radius, self.strength, erase)
            .is_empty();
        self.stroke = Some(Stroke {
            uid,
            erase,
            mask,
            area: scatter.area,
            last: h.local,
            dirty,
            written: None,
            open: false,
            dabs: 1,
        });
        self.hover = Some(h);
        // The first dab shows at once.
        self.flush(state, true);
    }

    fn drag(&mut self, state: &mut EditorState, e: &ToolEvent) {
        let Some(uid) = self.stroke.as_ref().map(|s| s.uid) else {
            return;
        };
        let Some(entity) = state.world.entity_by_uid(uid) else {
            return;
        };
        if let Some(h) = hit(state, uid, entity, &e.ray) {
            self.stamp_to(h.local);
            self.hover = Some(h);
        }
        self.flush(state, false);
    }

    fn release(&mut self, state: &mut EditorState) {
        self.flush(state, true);
        if let Some(s) = self.stroke.take()
            && s.open
        {
            state.finish_edit();
        }
    }

    fn erasing(&self, shift: bool) -> bool {
        match &self.stroke {
            Some(s) => s.erase,
            None => self.erase != shift,
        }
    }
}

impl ViewportTool for ScatterBrush {
    fn id(&self) -> &'static str {
        "paint_scatter"
    }

    fn label(&self) -> &'static str {
        "Paint"
    }

    fn name(&self) -> &'static str {
        "Paint Scatter"
    }

    fn tooltip(&self) -> &'static str {
        "Paint where the selected scatter places its instances: drag to paint, \
         Shift-drag (or X) to erase, [ ] radius, − = strength. Esc returns to Move."
    }

    fn wants_input(&self, state: &EditorState) -> bool {
        target(state).is_some_and(|(_, e)| !state.blocked_in_viewport(e))
    }

    fn on_pointer(&mut self, state: &mut EditorState, e: &ToolEvent) -> ToolResponse {
        self.shift = e.modifiers.shift;
        match e.kind {
            ToolEventKind::Hover => {
                if self.stroke.is_some() && e.primary_down {
                    self.drag(state, e);
                } else {
                    self.hover = target(state).and_then(|(u, x)| hit(state, u, x, &e.ray));
                }
            }
            ToolEventKind::Press => self.press(state, e),
            ToolEventKind::Release | ToolEventKind::Click => self.release(state),
        }
        ToolResponse::Handled
    }

    fn on_key(
        &mut self,
        _state: &mut EditorState,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        let is = |c: Command| {
            shortcuts::shortcut(c)
                .keys
                .iter()
                .any(|k| k.logical_key == key && modifiers.matches_exact(k.modifiers))
        };
        if is(Command::BrushSmaller) {
            self.radius = (self.radius * 0.8).clamp(RADIUS.0, RADIUS.1);
        } else if is(Command::BrushLarger) {
            self.radius = (self.radius * 1.25).clamp(RADIUS.0, RADIUS.1);
        } else if is(Command::BrushWeaker) {
            self.strength = ((self.strength - 0.1) * 10.0).round() / 10.0;
            self.strength = self.strength.clamp(0.1, 1.0);
        } else if is(Command::BrushStronger) {
            self.strength = ((self.strength + 0.1) * 10.0).round() / 10.0;
            self.strength = self.strength.clamp(0.1, 1.0);
        } else if is(Command::BrushErase) && self.stroke.is_none() {
            self.erase = !self.erase;
        } else {
            return false;
        }
        true
    }

    fn cancel(&mut self, state: &mut EditorState) {
        if let Some(s) = self.stroke.take()
            && s.open
        {
            state.rollback_edit();
        }
        self.hover = None;
    }

    fn draw_overlay(&self, state: &EditorState, dd: &mut DebugDraw, tokens: &Tokens) {
        let Some(h) = self.hover else { return };
        let Some(e) = state.world.entity_by_uid(h.uid) else {
            return;
        };
        let color = crate::tools::color(if self.erasing(self.shift) {
            tokens.warn
        } else {
            tokens.accent
        });
        let g = global(state, e);
        // Rings sampled on the ground, lifted a little to stay visible.
        with_surface(state, e, |surface| {
            let center_y = surface.sample(h.local).map_or(0.0, |s| s.height);
            let ring = |r: f32| -> Vec<Vec3> {
                (0..=RING)
                    .map(|k| {
                        let a = k as f32 / RING as f32 * std::f32::consts::TAU;
                        let p = h.local + Vec2::new(a.cos(), a.sin()) * r;
                        let y = surface.sample(p).map_or(center_y, |s| s.height);
                        g.transform_point3(Vec3::new(p.x, y + 0.04, p.y))
                    })
                    .collect()
            };
            for (r, c) in [
                (self.radius, color),
                (self.radius * self.strength, color.with_alpha(0.55)),
            ] {
                for w in ring(r).windows(2) {
                    dd.line_on(LineLayer::DisplayOverlay, w[0], w[1], c);
                }
            }
        });
        let up = g.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
        dd.line_on(
            LineLayer::DisplayOverlay,
            h.world,
            h.world + up * (self.radius * 0.25).clamp(0.1, 1.0),
            color,
        );
    }

    fn hint(&self, state: &EditorState) -> Option<String> {
        if !self.wants_input(state) {
            return Some(
                "Paint Scatter  ·  select an entity with a Scatter to paint  ·  Esc: Move".into(),
            );
        }
        let mode = if self.erasing(self.shift) {
            "Erase"
        } else {
            "Add"
        };
        Some(format!(
            "Paint Scatter  ·  {mode}  ·  radius {:.1} m  ·  strength {:.0}%  ·  drag to paint (Shift: erase)  ·  [ ] radius  ·  − = strength  ·  X: add/erase  ·  Esc: Move",
            self.radius,
            self.strength * 100.0
        ))
    }
}
