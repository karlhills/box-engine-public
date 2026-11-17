//! The engine's built-in generators, listed under Add ▸ Shapes:
//!
//! - [`Primitive`] (box_core's box, sphere, cylinder, capsule and plane):
//!   its `Generator` impl lives here, so primitives go through the same
//!   runtime as every generator (stamp, throttled flush, cache key,
//!   warnings, bake) while scenes keep the `Primitive` component unchanged.
//! - [`Lathe`]: a profile turned around the entity's Y axis (vases,
//!   columns, domes, towers), from a preset or custom points.
//! - [`Extrude`]: an outline (with an optional hole) pushed up into a
//!   solid, with a bevel on its top and bottom edges.
//!
//! [`register_builtin_generators`] registers them (`register_proc_types`
//! calls it, so `box_engine::engine_registry`, the editor and the CLI know
//! them). The runtime always runs `Primitive`, even in worlds whose
//! registry lacks the generator type data (`World::new()`), so primitives
//! draw everywhere they did before.

use box_core::pmath;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use box_core::primitive::Primitive;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Registry, Step, Unit};
use glam::Vec2;

use crate::extrude::{ExtrudeParams, extrude_into};
use crate::generator::{GenContext, GenOutput, Generator, GeneratorRegistry};
use crate::lathe::{LatheParams, lathe_into};
use crate::mesh_builder::{MeshBuilder, PlaneFrame};
use crate::poly::{self, Join, Polygon};
use crate::sweep::Profile;

/// Register the built-in generators (`Primitive`, `Lathe`, `Extrude`) with
/// their generator type data.
pub fn register_builtin_generators(registry: &mut Registry) {
    let results = [
        registry.register_generator::<Primitive>(),
        registry.register_generator::<Lathe>(),
        registry.register_generator::<Extrude>(),
    ];
    for r in results {
        if let Err(e) = r {
            tracing::error!("built-in generator registration failed: {e}");
        }
    }
}

// ------------------------------------------------------------ Primitive ----

impl Generator for Primitive {
    const VERSION: u32 = 1;
    const CATEGORY: &'static str = "Shapes";

    /// The shape's mesh (`Primitive::mesh`, with tangents). Out-of-range
    /// parameters are clamped, with a warning.
    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let out = GenOutput::mesh(self.mesh());
        if self.shape.clamped() == self.shape {
            out
        } else {
            out.warn("shape parameters outside their ranges were clamped")
        }
    }
}

// ---------------------------------------------------------------- Lathe ----

/// A surface of revolution: a profile of (radius, height) points turned
/// around the entity's Y axis. Pick a preset shape, or Custom for your own
/// profile. Material slots: Surface, and Cap for the ends of a partial turn.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Lathe {
    /// The profile that is turned: a preset with a few sizes, or Custom points.
    pub shape: LatheShape,
    /// Segments around the axis (more is rounder).
    #[reflect(@Range { min: 3.0, max: 256.0 })]
    pub segments: u32,
    /// Where the turn starts, counter-clockwise from +X seen from above.
    #[reflect(@Range { min: 0.0, max: 6.3 }, @Unit("rad"), @Step(0.01))]
    pub start_angle: f32,
    /// How far the profile turns; 2π (6.28) is a full revolution.
    #[reflect(@Range { min: 0.0, max: 6.3 }, @Unit("rad"), @Step(0.01))]
    pub sweep_angle: f32,
    /// Close the two ends of a partial turn (Cap material slot).
    pub caps: bool,
    /// Profile corners that turn by at most this angle shade smooth; sharper
    /// corners stay hard.
    #[reflect(@Range { min: 0.0, max: 3.15 }, @Unit("rad"), @Step(0.01))]
    pub smooth_angle: f32,
}

impl Default for Lathe {
    fn default() -> Self {
        Self {
            shape: LatheShape::default(),
            segments: 32,
            start_angle: 0.0,
            sweep_angle: TAU,
            caps: true,
            smooth_angle: 0.6,
        }
    }
}

/// The profile of a [`Lathe`]. Presets stand on the entity's origin (y = 0)
/// and close at the axis, so they make solids.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum LatheShape {
    /// A vase with a foot, a belly, a neck and an open mouth.
    Vase {
        /// Total height.
        #[reflect(@Range { min: 0.01, max: 100.0 }, @Unit("m"), @Step(0.01))]
        height: f32,
        /// Radius at the widest point.
        #[reflect(@Range { min: 0.01, max: 100.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
    },
    /// A classical column: plinth, base moulding, tapering shaft, capital.
    Column {
        /// Total height.
        #[reflect(@Range { min: 0.01, max: 100.0 }, @Unit("m"), @Step(0.01))]
        height: f32,
        /// Radius of the shaft at its foot.
        #[reflect(@Range { min: 0.01, max: 100.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
    },
    /// A solid half sphere standing on its flat side.
    Dome {
        /// Radius.
        #[reflect(@Range { min: 0.01, max: 100.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Profile points from the rim to the top.
        #[reflect(@Range { min: 2.0, max: 128.0 })]
        rings: u32,
    },
    /// Your own profile: (radius, height) points, drawn bottom to top. The
    /// surface faces right of the drawing direction, so an outline drawn
    /// up the outside faces outward; start and end on the axis (radius 0)
    /// for a closed solid.
    Custom {
        /// (radius, height) points in metres.
        profile: Vec<Vec2>,
        /// Join the last point back to the first (a ring, like a torus).
        closed: bool,
    },
}

impl Default for LatheShape {
    fn default() -> Self {
        Self::Vase {
            height: 1.0,
            radius: 0.35,
        }
    }
}

/// Catmull-Rom through `points` (end tangents from the end segments),
/// `steps` samples per span; the input points are kept.
fn smooth(points: &[Vec2], steps: usize) -> Vec<Vec2> {
    if points.len() < 3 || steps < 2 {
        return points.to_vec();
    }
    let n = points.len();
    let get = |i: isize| -> Vec2 {
        if i < 0 {
            2.0 * points[0] - points[1]
        } else if i as usize >= n {
            2.0 * points[n - 1] - points[n - 2]
        } else {
            points[i as usize]
        }
    };
    let mut out = Vec::with_capacity((n - 1) * steps + 1);
    for i in 0..n - 1 {
        let (p0, p1, p2, p3) = (
            get(i as isize - 1),
            get(i as isize),
            get(i as isize + 1),
            get(i as isize + 2),
        );
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let (t2, t3) = (t * t, t * t * t);
            out.push(
                0.5 * (2.0 * p1
                    + (p2 - p0) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t3),
            );
        }
    }
    out.push(points[n - 1]);
    out
}

/// Finite and inside `min..=max`; `fallback` when not finite.
fn finite_clamp(v: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        fallback
    }
}

impl LatheShape {
    /// The profile points and whether the profile is closed.
    pub fn profile(&self) -> (Vec<Vec2>, bool) {
        match self {
            Self::Vase { height, radius } => {
                let (h, r) = (
                    finite_clamp(*height, 0.01, 100.0, 1.0),
                    finite_clamp(*radius, 0.01, 100.0, 0.35),
                );
                // Unit profile: x in radii, y in heights.
                let outside = smooth(
                    &[
                        Vec2::new(0.52, 0.06),
                        Vec2::new(0.78, 0.22),
                        Vec2::new(1.0, 0.42),
                        Vec2::new(0.9, 0.6),
                        Vec2::new(0.58, 0.78),
                        Vec2::new(0.4, 0.89),
                        Vec2::new(0.45, 0.96),
                        Vec2::new(0.56, 1.0),
                    ],
                    5,
                );
                let mut unit = vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(0.5, 0.0),
                    Vec2::new(0.56, 0.03),
                ];
                unit.extend(outside);
                unit.extend([
                    Vec2::new(0.49, 1.0),
                    Vec2::new(0.36, 0.93),
                    Vec2::new(0.33, 0.85),
                    Vec2::new(0.0, 0.8),
                ]);
                (unit.iter().map(|p| *p * Vec2::new(r, h)).collect(), false)
            }
            Self::Column { height, radius } => {
                let (h, r) = (
                    finite_clamp(*height, 0.01, 100.0, 3.0),
                    finite_clamp(*radius, 0.01, 100.0, 0.25),
                );
                let unit = [
                    (0.0, 0.0),
                    (1.4, 0.0),
                    (1.4, 0.05),
                    (1.26, 0.05),
                    (1.26, 0.07),
                    (1.12, 0.095),
                    (1.0, 0.11),
                    (0.86, 0.885),
                    (0.98, 0.9),
                    (1.12, 0.925),
                    (1.32, 0.95),
                    (1.4, 0.95),
                    (1.4, 1.0),
                    (0.0, 1.0),
                ];
                (
                    unit.iter().map(|&(x, y)| Vec2::new(x * r, y * h)).collect(),
                    false,
                )
            }
            Self::Dome { radius, rings } => {
                let r = finite_clamp(*radius, 0.01, 100.0, 1.0);
                let n = (*rings).clamp(2, 128);
                let mut pts = vec![Vec2::ZERO];
                pts.extend((0..=n).map(|i| {
                    let a = FRAC_PI_2 * i as f32 / n as f32;
                    // The top point exactly on the axis (a pole).
                    let x = if i == n { 0.0 } else { r * pmath::cos(a) };
                    Vec2::new(x, r * pmath::sin(a))
                }));
                (pts, false)
            }
            Self::Custom { profile, closed } => (profile.clone(), *closed),
        }
    }
}

impl Generator for Lathe {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Surface", "Cap"];
    const CATEGORY: &'static str = "Shapes";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let (points, closed) = self.shape.profile();
        let mut warnings = Vec::new();
        if matches!(self.shape, LatheShape::Custom { .. }) && points.len() < 2 {
            return GenOutput::empty()
                .warn("the custom profile needs at least two (radius, height) points");
        }
        if points.iter().any(|p| !p.is_finite()) {
            return GenOutput::empty().warn("the profile has non-finite points");
        }
        let smooth_angle = finite_clamp(self.smooth_angle, 0.0, PI, 0.6);
        let params = LatheParams {
            profile: Profile {
                points,
                closed,
                edge_slots: Vec::new(),
                smooth_angle,
            },
            segments: self.segments.clamp(3, 256),
            start_angle: self.start_angle,
            sweep_angle: self.sweep_angle,
            caps: self.caps,
            cap_slot: 1,
        };
        let mut mb = MeshBuilder::new().with_tangents();
        lathe_into(&mut mb, &params);
        let built = mb.build();
        warnings.extend(built.warnings);
        output(built.mesh, warnings)
    }
}

/// A mesh with warnings; an empty mesh draws nothing.
fn output(mesh: box_core::mesh::MeshData, warnings: Vec<String>) -> GenOutput {
    let mut out = if mesh.triangle_count() == 0 {
        GenOutput::empty()
    } else {
        GenOutput::mesh(mesh)
    };
    out.warnings = warnings;
    out
}

// -------------------------------------------------------------- Extrude ----

/// A flat outline pushed up along the entity's Y axis into a solid
/// (a slab, a plaque, a star, a frame), standing on the origin. Pick an
/// outline and an optional hole; the bevel rounds or chamfers the top and
/// bottom edges. Material slots: Sides, Caps (top and bottom), Bevel.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Extrude {
    /// The outline, seen from above: x to the right (+X), y away (−Z).
    pub outline: Outline,
    /// A hole through the solid.
    pub hole: Hole,
    /// Height of the solid.
    #[reflect(@Range { min: 0.001, max: 100.0 }, @Unit("m"), @Step(0.01))]
    pub depth: f32,
    /// Width of the bevel on the top and bottom edges (0: none). Reduced,
    /// with a warning, where the outline is too thin for it.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"), @Step(0.005))]
    pub bevel_width: f32,
    /// Bevel steps: 1 is a flat chamfer, more is a rounded edge.
    #[reflect(@Range { min: 1.0, max: 16.0 })]
    pub bevel_segments: u32,
    /// Close the top and bottom.
    pub caps: bool,
    /// Outline corners that turn by at most this angle shade smooth;
    /// sharper corners stay hard.
    #[reflect(@Range { min: 0.0, max: 3.15 }, @Unit("rad"), @Step(0.01))]
    pub smooth_angle: f32,
}

impl Default for Extrude {
    fn default() -> Self {
        Self {
            outline: Outline::default(),
            hole: Hole::None,
            depth: 0.3,
            bevel_width: 0.04,
            bevel_segments: 3,
            caps: true,
            smooth_angle: 0.6,
        }
    }
}

/// The outline of an [`Extrude`], centred on the origin.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum Outline {
    /// A rectangle, optionally with rounded corners.
    Rectangle {
        /// Width (x) and length (y).
        #[reflect(@Range { min: 0.01, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        size: Vec2,
        /// Radius of the rounded corners (0: sharp).
        #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m"), @Step(0.01))]
        corner_radius: f32,
    },
    /// A circle.
    Circle {
        /// Radius.
        #[reflect(@Range { min: 0.01, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Points around the circle.
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
    },
    /// A regular polygon with a flat side at the front.
    Polygon {
        /// Number of sides.
        #[reflect(@Range { min: 3.0, max: 64.0 })]
        sides: u32,
        /// Distance from the centre to a corner.
        #[reflect(@Range { min: 0.01, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
    },
    /// A star with one point at the back.
    Star {
        /// Number of points.
        #[reflect(@Range { min: 3.0, max: 64.0 })]
        points: u32,
        /// Distance from the centre to a point.
        #[reflect(@Range { min: 0.01, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        outer_radius: f32,
        /// Distance from the centre to the inner corners.
        #[reflect(@Range { min: 0.005, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        inner_radius: f32,
    },
    /// Your own outline: (x, y) points in metres, in either winding.
    Custom {
        /// (x, y) points in metres.
        points: Vec<Vec2>,
    },
}

impl Default for Outline {
    fn default() -> Self {
        Self::Rectangle {
            size: Vec2::new(1.0, 1.0),
            corner_radius: 0.15,
        }
    }
}

/// A hole through an [`Extrude`].
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum Hole {
    /// No hole.
    #[default]
    None,
    /// The outline shrunk by `width`: a frame or a ring.
    Inset {
        /// Width of the remaining rim.
        #[reflect(@Range { min: 0.005, max: 100.0 }, @Unit("m"), @Step(0.01))]
        width: f32,
    },
    /// A round hole.
    Circle {
        /// Radius.
        #[reflect(@Range { min: 0.005, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Points around the circle.
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
        /// Centre of the hole.
        #[reflect(@Unit("m"), @Step(0.01))]
        offset: Vec2,
    },
    /// A rectangular hole.
    Rectangle {
        /// Width (x) and length (y).
        #[reflect(@Range { min: 0.005, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        size: Vec2,
        /// Centre of the hole.
        #[reflect(@Unit("m"), @Step(0.01))]
        offset: Vec2,
    },
    /// Your own holes: one list of (x, y) points per hole.
    Custom {
        /// Holes as (x, y) point lists in metres.
        rings: Vec<Vec<Vec2>>,
    },
}

impl Outline {
    /// The outline ring (counter-clockwise for the presets).
    pub fn ring(&self) -> Vec<Vec2> {
        match self {
            Self::Rectangle {
                size,
                corner_radius,
            } => {
                let half = Vec2::new(
                    finite_clamp(size.x, 0.01, 1000.0, 1.0),
                    finite_clamp(size.y, 0.01, 1000.0, 1.0),
                ) * 0.5;
                let rect = Polygon::rect(-half, half).outer;
                let r = finite_clamp(*corner_radius, 0.0, half.min_element(), 0.0);
                if r > 1e-4 {
                    poly::round_corners(&rect, r, 6)
                } else {
                    rect
                }
            }
            Self::Circle { radius, segments } => {
                Polygon::circle(
                    Vec2::ZERO,
                    finite_clamp(*radius, 0.01, 1000.0, 0.5),
                    (*segments).clamp(3, 256),
                )
                .outer
            }
            Self::Polygon { sides, radius } => {
                let n = (*sides).clamp(3, 64);
                let r = finite_clamp(*radius, 0.01, 1000.0, 0.5);
                // A flat side at the front (−y, towards +Z).
                let start = -FRAC_PI_2 + PI / n as f32;
                (0..n)
                    .map(|i| {
                        let a = start + TAU * i as f32 / n as f32;
                        Vec2::new(pmath::cos(a), pmath::sin(a)) * r
                    })
                    .collect()
            }
            Self::Star {
                points,
                outer_radius,
                inner_radius,
            } => {
                let n = (*points).clamp(3, 64);
                let outer = finite_clamp(*outer_radius, 0.01, 1000.0, 0.5);
                let inner = finite_clamp(*inner_radius, 0.005, 1000.0, 0.25);
                (0..2 * n)
                    .map(|i| {
                        let a = FRAC_PI_2 + PI * i as f32 / n as f32;
                        let r = if i % 2 == 0 { outer } else { inner };
                        Vec2::new(pmath::cos(a), pmath::sin(a)) * r
                    })
                    .collect()
            }
            Self::Custom { points } => points.clone(),
        }
    }
}

impl Hole {
    /// The hole polygons for `outline`.
    fn polygons(&self, outline: &Polygon) -> Vec<Polygon> {
        match self {
            Self::None => Vec::new(),
            Self::Inset { width } => {
                let w = finite_clamp(*width, 0.005, 100.0, 0.1);
                poly::offset(
                    std::slice::from_ref(outline),
                    -w,
                    Join::Miter { limit: 4.0 },
                )
            }
            Self::Circle {
                radius,
                segments,
                offset,
            } => vec![Polygon::circle(
                finite_vec(*offset),
                finite_clamp(*radius, 0.005, 1000.0, 0.25),
                (*segments).clamp(3, 256),
            )],
            Self::Rectangle { size, offset } => {
                let half = Vec2::new(
                    finite_clamp(size.x, 0.005, 1000.0, 0.5),
                    finite_clamp(size.y, 0.005, 1000.0, 0.5),
                ) * 0.5;
                let c = finite_vec(*offset);
                vec![Polygon::rect(c - half, c + half)]
            }
            Self::Custom { rings } => rings
                .iter()
                .filter(|r| r.len() >= 3)
                .map(|r| Polygon::new(r.clone()))
                .collect(),
        }
    }
}

fn finite_vec(v: Vec2) -> Vec2 {
    if v.is_finite() { v } else { Vec2::ZERO }
}

impl Generator for Extrude {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Sides", "Caps", "Bevel"];
    const CATEGORY: &'static str = "Shapes";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let ring = self.outline.ring();
        if ring.iter().any(|p| !p.is_finite()) {
            return GenOutput::empty().warn("the outline has non-finite points");
        }
        if ring.len() < 3 {
            return GenOutput::empty().warn("the outline needs at least three (x, y) points");
        }
        let outline = Polygon::new(ring);
        let solid = poly::make_valid(std::slice::from_ref(&outline));
        let area = poly::total_area(&solid);
        if area <= 1e-8 {
            return GenOutput::empty().warn("the outline has no area");
        }
        let mut warnings = Vec::new();
        let holes = self.hole.polygons(&outline);
        let shape = if matches!(self.hole, Hole::None) {
            solid
        } else if holes.is_empty() {
            warnings.push("the hole leaves no rim (the outline is too thin for it)".to_string());
            solid
        } else {
            let cut = poly::difference(&solid, &holes);
            let left = poly::total_area(&cut);
            if left <= 1e-8 {
                return GenOutput::empty().warn("the hole removes the whole outline");
            }
            if (left - area).abs() <= area * 1e-5 {
                warnings.push("the hole lies outside the outline".to_string());
            }
            cut
        };
        let params = ExtrudeParams {
            depth: finite_clamp(self.depth, -100.0, 100.0, 0.3),
            frame: PlaneFrame::PLAN,
            bevel_width: finite_clamp(self.bevel_width, 0.0, 1.0, 0.0),
            bevel_segments: self.bevel_segments.clamp(1, 16),
            cap_start: self.caps,
            cap_end: self.caps,
            smooth_angle: finite_clamp(self.smooth_angle, 0.0, PI, 0.6),
            side_slot: 0,
            cap_slot: 1,
            bevel_slot: 2,
            uv_scale: 1.0,
        };
        let mut mb = MeshBuilder::new().with_tangents();
        for p in &shape {
            extrude_into(&mut mb, p, &params);
        }
        let built = mb.build();
        // Several parts warn alike (each part reduces its bevel).
        for w in built.warnings {
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        output(built.mesh, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_builder::{edge_report, inward_triangles, signed_volume};

    fn run<G: Generator>(g: &G) -> GenOutput {
        g.generate(&GenContext::new(0))
    }

    #[test]
    fn primitive_generates_its_mesh_and_warns_when_clamped() {
        let p = Primitive::sphere(0.5);
        let out = run(&p);
        assert_eq!(out.mesh.as_ref(), Some(&p.mesh()));
        assert!(out.warnings.is_empty());
        let bad = Primitive::sphere(-1.0);
        assert_eq!(run(&bad).warnings.len(), 1);
    }

    #[test]
    fn lathe_presets_are_closed_outward_solids() {
        for shape in [
            LatheShape::default(),
            LatheShape::Column {
                height: 3.0,
                radius: 0.25,
            },
            LatheShape::Dome {
                radius: 1.0,
                rings: 12,
            },
        ] {
            let out = run(&Lathe {
                shape: shape.clone(),
                ..Lathe::default()
            });
            assert!(out.warnings.is_empty(), "{shape:?}: {:?}", out.warnings);
            let m = out.mesh.expect("mesh");
            m.validate().unwrap();
            assert!(
                edge_report(&m, 1e-4).is_closed_manifold(),
                "{shape:?} is not closed"
            );
            assert_eq!(inward_triangles(&m), 0, "{shape:?}");
            assert!(signed_volume(&m) > 0.0, "{shape:?}");
            assert!(m.bounds().min.y.abs() < 1e-5, "{shape:?} stands on y = 0");
        }
    }

    #[test]
    fn partial_lathe_has_caps_in_slot_1() {
        let out = run(&Lathe {
            sweep_angle: PI,
            ..Lathe::default()
        });
        let m = out.mesh.unwrap();
        assert!(m.groups.iter().any(|g| g.material_slot == 1));
        assert!(edge_report(&m, 1e-4).is_closed_manifold());
    }

    #[test]
    fn empty_custom_profile_warns_and_draws_nothing() {
        let out = run(&Lathe {
            shape: LatheShape::Custom {
                profile: Vec::new(),
                closed: false,
            },
            ..Lathe::default()
        });
        assert!(out.mesh.is_none());
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn extrude_with_hole_and_bevel_is_closed_and_uses_three_slots() {
        let e = Extrude {
            outline: Outline::Rectangle {
                size: Vec2::new(2.0, 1.0),
                corner_radius: 0.2,
            },
            hole: Hole::Circle {
                radius: 0.3,
                segments: 24,
                offset: Vec2::ZERO,
            },
            ..Extrude::default()
        };
        let out = run(&e);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let m = out.mesh.unwrap();
        assert!(edge_report(&m, 1e-4).is_closed_manifold());
        assert_eq!(inward_triangles(&m), 0);
        let mut slots: Vec<u32> = m.groups.iter().map(|g| g.material_slot).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots, [0, 1, 2]);
        // Volume: (rounded rectangle − circle) × depth, a bit less for the bevel.
        let full = (2.0 - (4.0 - PI) * 0.04) - PI * 0.09;
        let v = signed_volume(&m) / 0.3;
        assert!(v < full && v > full * 0.9, "{v} vs {full}");
        assert!(m.bounds().min.y.abs() < 1e-6 && (m.bounds().max.y - 0.3).abs() < 1e-5);
    }

    #[test]
    fn extrude_presets_and_holes_degrade_with_warnings() {
        for outline in [
            Outline::default(),
            Outline::Circle {
                radius: 0.5,
                segments: 32,
            },
            Outline::Polygon {
                sides: 6,
                radius: 0.5,
            },
            Outline::Star {
                points: 5,
                outer_radius: 0.5,
                inner_radius: 0.22,
            },
        ] {
            for hole in [Hole::None, Hole::Inset { width: 0.1 }] {
                // The bevel insets each ring on its own, so it must stay
                // well under half the rim of an inset hole.
                let bevel_width = if hole == Hole::None { 0.04 } else { 0.02 };
                let out = run(&Extrude {
                    outline: outline.clone(),
                    hole: hole.clone(),
                    bevel_width,
                    ..Extrude::default()
                });
                let m = out.mesh.expect("mesh");
                let report = edge_report(&m, 1e-4);
                assert!(
                    report.is_closed_manifold(),
                    "{outline:?} {hole:?}: {report:?} {:?}",
                    out.warnings
                );
                assert_eq!(inward_triangles(&m), 0, "{outline:?} {hole:?}");
            }
        }
        let outside = run(&Extrude {
            hole: Hole::Circle {
                radius: 0.1,
                segments: 8,
                offset: Vec2::new(5.0, 0.0),
            },
            ..Extrude::default()
        });
        assert!(outside.mesh.is_some());
        assert_eq!(outside.warnings, ["the hole lies outside the outline"]);
        let all = run(&Extrude {
            hole: Hole::Rectangle {
                size: Vec2::splat(3.0),
                offset: Vec2::ZERO,
            },
            ..Extrude::default()
        });
        assert!(all.mesh.is_none() && all.warnings.len() == 1);
        let empty = run(&Extrude {
            outline: Outline::Custom { points: Vec::new() },
            ..Extrude::default()
        });
        assert!(empty.mesh.is_none() && empty.warnings.len() == 1);
    }

    #[test]
    fn builtins_are_registered_under_shapes() {
        let mut r = Registry::with_core_types();
        register_builtin_generators(&mut r);
        for (name, slots) in [
            ("Primitive", &["Surface"][..]),
            ("Lathe", &["Surface", "Cap"][..]),
            ("Extrude", &["Sides", "Caps", "Bevel"][..]),
        ] {
            let g = r.generator(name).expect(name);
            assert_eq!(g.category(), "Shapes");
            assert_eq!(g.material_slots(), slots);
            assert!(g.doc().is_some(), "{name} has a tooltip");
        }
    }
}
