//! The [`Stair`] generator: a straight flight between two anchor points.
//!
//! The flight climbs from the lower of `start` and `end` (the foot of the
//! first riser, centred across the width) to the higher (the back of the
//! top tread or landing). The rise is divided into equal risers no taller
//! than `max_riser`; the run (minus the landing) into equal goings. A run
//! too short for that many risers (steeper than [`MIN_GOING`] per step) or
//! very long for its rise warns; nothing panics.

use bevy_reflect::Reflect;
use box_core::reflect::{Range, ReflectComponent, ReflectDefault, Step, Unit};
use box_proc::generator::{GenContext, GenOutput, Generator};
use box_proc::poly::{self, Polygon};
use glam::{Vec2, Vec3};

use crate::consts::*;
use crate::mesh::ArchMesh;

/// Material slots of [`Stair`].
pub mod slot {
    pub const TREAD: u32 = 0;
    pub const RISER: u32 = 1;
    pub const SIDE: u32 = 2;
}

/// Open-tread plates and stringer boards.
const TREAD_THICKNESS: f32 = 0.06;
const STRINGER_THICKNESS: f32 = 0.06;
const STRINGER_DEPTH: f32 = 0.28;

/// How the flight is built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum StairStyle {
    /// Solid masonry down to the start's level.
    #[default]
    Solid,
    /// Tread plates between two stringer boards.
    Open,
}

/// A straight flight of steps between two points (entity-local). Material
/// slots: Tread, Riser, Side (sides, bottom, stringers).
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct Stair {
    /// One end: the foot of the flight if it is the lower one.
    #[reflect(@Unit("m"), @Step(0.01))]
    pub start: Vec3,
    /// The other end: the back of the top tread if it is the higher one.
    #[reflect(@Unit("m"), @Step(0.01))]
    pub end: Vec3,
    #[reflect(@Range { min: 0.2, max: 20.0 }, @Unit("m"), @Step(0.01))]
    pub width: f32,
    /// The tallest riser allowed; the rise is split into equal risers.
    #[reflect(@Range { min: 0.05, max: 0.5 }, @Unit("m"), @Step(0.005))]
    pub max_riser: f32,
    /// A flat landing at the top, part of the run.
    #[reflect(@Range { min: 0.0, max: 20.0 }, @Unit("m"), @Step(0.01))]
    pub landing: f32,
    pub style: StairStyle,
    /// Strength of the baked ambient occlusion (0 turns it off).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.05))]
    pub ao: f32,
}

impl Default for Stair {
    fn default() -> Self {
        Self {
            start: Vec3::ZERO,
            end: Vec3::new(0.0, 1.2, -1.68),
            width: 1.2,
            max_riser: 0.2,
            landing: 0.0,
            style: StairStyle::Solid,
            ao: 1.0,
        }
    }
}

/// The resolved layout of a flight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StairLayout {
    /// The foot (lower point).
    pub foot: Vec3,
    /// Unit horizontal direction of climb.
    pub along: Vec3,
    pub risers: u32,
    pub riser: f32,
    pub going: f32,
    pub landing: f32,
}

impl StairLayout {
    /// Total horizontal length.
    pub fn length(&self) -> f32 {
        self.going * self.risers as f32 + self.landing
    }
}

impl Stair {
    /// The flight's layout and warnings; `None` when nothing can be built.
    pub fn layout(&self) -> (Option<StairLayout>, Vec<String>) {
        let mut warnings = Vec::new();
        if !(self.start.is_finite() && self.end.is_finite()) {
            return (None, vec!["start and end must be finite".into()]);
        }
        let (foot, top) = if self.end.y >= self.start.y {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        };
        let rise = top.y - foot.y;
        let flat = Vec2::new(top.x - foot.x, top.z - foot.z);
        let len = flat.length();
        if rise < 0.01 {
            return (
                None,
                vec![
                    "start and end are at the same height: nothing to climb (use a Platform)"
                        .into(),
                ],
            );
        }
        if len < 0.01 {
            return (
                None,
                vec![
                    "start and end are directly above each other: a stair needs a horizontal run"
                        .into(),
                ],
            );
        }
        let max_riser = if self.max_riser.is_finite() && self.max_riser >= 0.05 {
            self.max_riser
        } else {
            warnings.push("max riser must be at least 0.05 m; using 0.2".into());
            0.2
        };
        let mut landing = if self.landing.is_finite() {
            self.landing.max(0.0)
        } else {
            0.0
        };
        if landing > len - 0.05 {
            warnings.push(format!(
                "the landing ({landing:.2} m) is longer than the run ({len:.2} m); shortened"
            ));
            landing = (len * 0.5).max(0.0);
        }
        let risers_f = (rise / max_riser).ceil();
        if risers_f > MAX_STEPS as f32 {
            return (
                None,
                vec![format!(
                    "{risers_f} risers needed; at most {MAX_STEPS} are built"
                )],
            );
        }
        let risers = (risers_f as u32).max(1);
        let riser = rise / risers as f32;
        let going = (len - landing) / risers as f32;
        if going < MIN_GOING {
            let need = MIN_GOING * risers as f32 + landing;
            warnings.push(format!(
                "too steep: {risers} risers of {riser:.3} m need at least {need:.2} m of run, the ends are {len:.2} m apart (going {going:.3} m < {MIN_GOING} m)"
            ));
        } else if going > MAX_GOING {
            warnings.push(format!(
                "very shallow: going {going:.2} m > {MAX_GOING} m; add a landing or move the ends closer"
            ));
        }
        let along = Vec3::new(flat.x, 0.0, flat.y) / len;
        (
            Some(StairLayout {
                foot,
                along,
                risers,
                riser,
                going,
                landing,
            }),
            warnings,
        )
    }

    pub fn build(&self) -> GenOutput {
        let (layout, mut warnings) = self.layout();
        let Some(l) = layout else {
            return GenOutput::empty().with_warnings(warnings);
        };
        let width = if self.width.is_finite() && self.width >= 0.2 {
            self.width
        } else {
            warnings.push("width must be at least 0.2 m; using 1.2".into());
            1.2
        };
        let ao = if self.ao.is_finite() {
            self.ao.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut mb = ArchMesh::new(ao);
        let across = Vec3::new(-l.along.z, 0.0, l.along.x);
        let hw = width * 0.5;
        let p = |s: f32, y: f32, x: f32| l.foot + l.along * s + Vec3::Y * y + across * x;
        let n = l.risers as usize;
        let (g, r) = (l.going, l.riser);
        let total = l.length();
        let base = |y: f32| 1.0 - BASE_AO * (1.0 - (y / BASE_AO_HEIGHT).clamp(0.0, 1.0));
        match self.style {
            StairStyle::Solid => {
                // Risers and treads.
                for i in 0..n {
                    let (s, y0, y1) = (i as f32 * g, i as f32 * r, (i + 1) as f32 * r);
                    let foot_ao = if i == 0 { base(0.0) } else { RISER_AO };
                    mb.quad(
                        slot::RISER,
                        [p(s, y0, -hw), p(s, y0, hw), p(s, y1, hw), p(s, y1, -hw)],
                        [foot_ao, foot_ao, 1.0, 1.0],
                        -l.along,
                    );
                    let s1 = if i + 1 == n {
                        total
                    } else {
                        (i + 1) as f32 * g
                    };
                    let back = if i + 1 == n { 1.0 } else { RISER_AO };
                    mb.quad(
                        slot::TREAD,
                        [p(s, y1, -hw), p(s, y1, hw), p(s1, y1, hw), p(s1, y1, -hw)],
                        [1.0, 1.0, back, back],
                        Vec3::Y,
                    );
                }
                // Back and bottom.
                let top = n as f32 * r;
                mb.quad(
                    slot::SIDE,
                    [
                        p(total, 0.0, -hw),
                        p(total, 0.0, hw),
                        p(total, top, hw),
                        p(total, top, -hw),
                    ],
                    [base(0.0), base(0.0), 1.0, 1.0],
                    l.along,
                );
                mb.quad(
                    slot::SIDE,
                    [
                        p(0.0, 0.0, -hw),
                        p(total, 0.0, -hw),
                        p(total, 0.0, hw),
                        p(0.0, 0.0, hw),
                    ],
                    [0.8; 4],
                    -Vec3::Y,
                );
                // Sides: the stepped profile.
                let mut ring = vec![Vec2::ZERO];
                for i in 0..n {
                    let s = i as f32 * g;
                    if i > 0 {
                        ring.push(Vec2::new(s, i as f32 * r));
                    }
                    ring.push(Vec2::new(s, (i + 1) as f32 * r));
                }
                ring.push(Vec2::new(total, n as f32 * r));
                ring.push(Vec2::new(total, 0.0));
                let t = poly::triangulate(&Polygon::new(ring));
                for side in [-1.0f32, 1.0] {
                    for tri in t.indices.as_chunks::<3>().0 {
                        let q = [0, 1, 2].map(|k| t.points[tri[k] as usize]);
                        mb.tri(
                            slot::SIDE,
                            q.map(|v| p(v.x, v.y, hw * side)),
                            q.map(|v| base(v.y)),
                            across * side,
                        );
                    }
                }
            }
            StairStyle::Open => {
                let inner = hw - STRINGER_THICKNESS;
                for i in 0..n {
                    let y1 = (i + 1) as f32 * r;
                    let s0 = i as f32 * g;
                    let s1 = if i + 1 == n {
                        total
                    } else {
                        (i + 1) as f32 * g
                    };
                    let y0 = y1 - TREAD_THICKNESS.min(r * 0.8);
                    box_between(
                        &mut mb,
                        &p,
                        s0,
                        s1,
                        y0,
                        y1,
                        -inner,
                        inner,
                        slot::TREAD,
                        slot::RISER,
                    );
                }
                // Stringers: top edge through the tread noses, depth below.
                let d = STRINGER_DEPTH;
                let top_n = n as f32 * r;
                let s_last = (n - 1) as f32 * g;
                let s0 = if d > r { (d - r) * g / r } else { 0.0 };
                let ring = if n == 1 {
                    // One riser: the last nose is the first, so the stepped
                    // outline would double back along s = 0. A plain band.
                    let bottom = (top_n - d).max(0.0);
                    vec![
                        Vec2::new(0.0, bottom),
                        Vec2::new(total, bottom),
                        Vec2::new(total, top_n),
                        Vec2::new(0.0, top_n),
                    ]
                } else {
                    let mut ring = vec![Vec2::new(s0.min(s_last * 0.99), 0.0)];
                    if d <= r {
                        ring.insert(0, Vec2::ZERO);
                    }
                    ring.push(Vec2::new(s_last, top_n - d));
                    ring.push(Vec2::new(total, top_n - d));
                    ring.push(Vec2::new(total, top_n));
                    ring.push(Vec2::new(s_last, top_n));
                    ring.push(Vec2::new(0.0, r));
                    if d > r {
                        ring.push(Vec2::ZERO);
                    }
                    ring
                };
                for x in [inner, -hw] {
                    prism(
                        &mut mb,
                        &p,
                        &ring,
                        x,
                        x + STRINGER_THICKNESS,
                        slot::SIDE,
                        &base,
                    );
                }
            }
        }
        let (m, w) = mb.build();
        warnings.extend(w);
        GenOutput {
            mesh: (m.triangle_count() > 0).then_some(m),
            warnings,
            ..GenOutput::default()
        }
    }
}

trait WithWarnings {
    fn with_warnings(self, w: Vec<String>) -> Self;
}

impl WithWarnings for GenOutput {
    fn with_warnings(mut self, w: Vec<String>) -> Self {
        self.warnings.extend(w);
        self
    }
}

/// A box in (s, y, x) stair space.
fn box_between(
    mb: &mut ArchMesh,
    p: &impl Fn(f32, f32, f32) -> Vec3,
    s0: f32,
    s1: f32,
    y0: f32,
    y1: f32,
    x0: f32,
    x1: f32,
    top_slot: u32,
    side_slot: u32,
) {
    let c = |s, y, x| p(s, y, x);
    let mid = c((s0 + s1) * 0.5, (y0 + y1) * 0.5, (x0 + x1) * 0.5);
    let faces = [
        [c(s0, y1, x0), c(s0, y1, x1), c(s1, y1, x1), c(s1, y1, x0)],
        [c(s0, y0, x0), c(s1, y0, x0), c(s1, y0, x1), c(s0, y0, x1)],
        [c(s0, y0, x0), c(s0, y0, x1), c(s0, y1, x1), c(s0, y1, x0)],
        [c(s1, y0, x0), c(s1, y1, x0), c(s1, y1, x1), c(s1, y0, x1)],
        [c(s0, y0, x0), c(s0, y1, x0), c(s1, y1, x0), c(s1, y0, x0)],
        [c(s0, y0, x1), c(s1, y0, x1), c(s1, y1, x1), c(s0, y1, x1)],
    ];
    for (k, q) in faces.into_iter().enumerate() {
        let centre = (q[0] + q[1] + q[2] + q[3]) * 0.25;
        let slot = if k == 0 { top_slot } else { side_slot };
        let ao = if k == 1 { [0.8; 4] } else { [1.0; 4] };
        mb.quad(slot, q, ao, centre - mid);
    }
}

/// A prism of the (s, y) polygon `ring` between x0 and x1.
fn prism(
    mb: &mut ArchMesh,
    p: &impl Fn(f32, f32, f32) -> Vec3,
    ring: &[Vec2],
    x0: f32,
    x1: f32,
    slot: u32,
    ao: &impl Fn(f32) -> f32,
) {
    let polygon = Polygon::new(ring.to_vec()).normalized();
    let t = poly::triangulate(&polygon);
    let side = p(0.0, 0.0, 1.0) - p(0.0, 0.0, 0.0);
    for (x, f) in [(x0, -side), (x1, side)] {
        for tri in t.indices.as_chunks::<3>().0 {
            let q = [0, 1, 2].map(|k| t.points[tri[k] as usize]);
            mb.tri(slot, q.map(|v| p(v.x, v.y, x)), q.map(|v| ao(v.y)), f);
        }
    }
    let r = &polygon.outer;
    for k in 0..r.len() {
        let (a, b) = (r[k], r[(k + 1) % r.len()]);
        let d = (b - a).normalize_or_zero();
        let n2 = Vec2::new(d.y, -d.x);
        let out = p(n2.x, n2.y, 0.0) - p(0.0, 0.0, 0.0);
        mb.quad(
            slot,
            [
                p(a.x, a.y, x0),
                p(b.x, b.y, x0),
                p(b.x, b.y, x1),
                p(a.x, a.y, x1),
            ],
            [ao(a.y), ao(b.y), ao(b.y), ao(a.y)],
            out,
        );
    }
}

impl Generator for Stair {
    /// 2: one-riser open stairs get a plain stringer band (the stepped
    /// outline doubled back on itself).
    const VERSION: u32 = 2;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Tread", "Riser", "Side"];
    const CATEGORY: &'static str = "Architecture";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        self.build()
    }
}
