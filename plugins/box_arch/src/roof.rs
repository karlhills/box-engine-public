//! The [`Roof`] generator: closed-form gable, hip and shed roofs over a
//! rectangle, cone and dome roofs over a circle.
//!
//! **Local frame.** The roof stands over its footprint centred on the
//! entity's origin; the underside passes through y = 0 along the outer wall
//! faces (the wall plate), so the entity is placed at the walls' eave
//! height. Eaves hang below that by the overhang times the slope.
//!
//! **Slab.** Tiles are a slab of constant *vertical* thickness, which keeps
//! ridge and hip lines at the same plan position on top and underneath
//! (spike S6, rule J11). Fascia and barge boards are planks; the eave
//! boards along the ridge own the corners. Ridge and hip rolls are low
//! "tents" sunk into the tiles.
//!
//! **Abutting ends** (`RoofEnd::Abut`, rule J9): an end against a taller
//! neighbour's wall has no overhang, verge or end board and runs
//! [`ABUT_PENETRATION`] into that wall; a hip end that abuts becomes a
//! gable end against the wall.
//!
//! **Walls follow the roof** (rule J8): [`Roof::underside`] returns the
//! underside as plain data ([`RoofUnderside`]); a game rule transforms it
//! into a wall's space and writes it into `Wall::roof`.

use box_core::pmath;
use std::f32::consts::{FRAC_PI_2, TAU};

use bevy_reflect::Reflect;
use box_core::reflect::{
    Range, ReflectComponent, ReflectDefault, ReflectVariantDefaults, Step, Unit, VariantDefaults,
};
use box_proc::generator::{GenContext, GenOutput, Generator};
use glam::{Vec2, Vec3};

use crate::clip;
use crate::consts::*;
use crate::mesh::ArchMesh;
use crate::underside::{RoofUnderside, UndersideSurface};

/// Material slots of [`Roof`].
pub mod slot {
    pub const ROOF: u32 = 0;
    pub const UNDERSIDE: u32 = 1;
    pub const TRIM: u32 = 2;
    pub const RIDGE: u32 = 3;
}

/// Which plan axis a rectangular roof's ridge runs along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum RidgeAxis {
    /// Along the longer side.
    #[default]
    Longer,
    X,
    Z,
}

/// How an end of a rectangular roof finishes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum RoofEnd {
    /// A gable or hip end (per shape) with overhang and boards.
    #[default]
    Free,
    /// Against a taller neighbour's wall standing on the footprint's edge:
    /// no overhang, no verge, no board; runs into the wall a little.
    Abut,
}

/// The roof's form and footprint. Rectangles are the outer wall faces,
/// x by z.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq, VariantDefaults)]
#[type_path = "box_arch"]
pub enum RoofShape {
    /// Two slopes meeting at a ridge; gable walls rise to them.
    Gable {
        #[reflect(@Unit("m"), @Step(0.01))]
        size: Vec2,
        ridge: RidgeAxis,
    },
    /// Four slopes; the ends slope too.
    Hip {
        #[reflect(@Unit("m"), @Step(0.01))]
        size: Vec2,
        ridge: RidgeAxis,
    },
    /// One slope rising across the ridge axis towards +z (ridge along x)
    /// or +x (ridge along z).
    Shed {
        #[reflect(@Unit("m"), @Step(0.01))]
        size: Vec2,
        ridge: RidgeAxis,
        /// The high edge leans on a taller wall (no overhang there).
        high_abuts: bool,
    },
    /// A cone over a round footprint (towers).
    Cone {
        #[reflect(@Range { min: 0.1, max: 100.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
    },
    /// A dome over a round footprint.
    Dome {
        #[reflect(@Range { min: 0.1, max: 100.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Height of the dome above its eave ring.
        #[reflect(@Range { min: 0.05, max: 100.0 }, @Unit("m"), @Step(0.01))]
        rise: f32,
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
        #[reflect(@Range { min: 2.0, max: 64.0 })]
        rings: u32,
    },
}

/// The default 5 × 4 m footprint for the sloped shapes; a 2 m cone of 32
/// segments and a 3 m dome rising 2 m (32 segments, 8 rings) for towers.
impl VariantDefaults for RoofShape {
    fn variant_default(variant: &str) -> Option<Self> {
        let size = Vec2::new(5.0, 4.0);
        let ridge = RidgeAxis::Longer;
        Some(match variant {
            "Gable" => Self::Gable { size, ridge },
            "Hip" => Self::Hip { size, ridge },
            "Shed" => Self::Shed {
                size,
                ridge,
                high_abuts: false,
            },
            "Cone" => Self::Cone {
                radius: 2.0,
                segments: 32,
            },
            "Dome" => Self::Dome {
                radius: 3.0,
                rise: 2.0,
                segments: 32,
                rings: 8,
            },
            _ => return None,
        })
    }
}

impl Default for RoofShape {
    fn default() -> Self {
        Self::Gable {
            size: Vec2::new(5.0, 4.0),
            ridge: RidgeAxis::Longer,
        }
    }
}

/// A roof: gable, hip or shed over a rectangle, cone or dome over a
/// circle, with slab thickness, overhang, soffit, fascia and barge boards,
/// ridge rolls and abutting ends. Material slots: Roof (tiles and slab
/// edges), Underside (soffit), Trim (boards), Ridge (rolls).
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct Roof {
    pub shape: RoofShape,
    /// Slope of gable, hip, shed and cone roofs.
    #[reflect(@Range { min: 0.05, max: 1.4 }, @Unit("rad"), @Step(0.01))]
    pub pitch: f32,
    /// How far the eaves and verges reach past the walls.
    #[reflect(@Range { min: 0.0, max: 5.0 }, @Unit("m"), @Step(0.01))]
    pub overhang: f32,
    /// Vertical thickness of the tile slab.
    #[reflect(@Range { min: 0.02, max: 2.0 }, @Unit("m"), @Step(0.01))]
    pub thickness: f32,
    /// The end at the low side of the ridge axis (rectangles).
    pub start: RoofEnd,
    /// The end at the high side of the ridge axis (rectangles).
    pub end: RoofEnd,
    /// Fascia and barge boards along free edges.
    pub boards: bool,
    /// Ridge and hip rolls.
    pub ridge_rolls: bool,
    /// Strength of the baked ambient occlusion (0 turns it off).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.05))]
    pub ao: f32,
}

impl Default for Roof {
    fn default() -> Self {
        Self {
            shape: RoofShape::default(),
            pitch: 27f32.to_radians(),
            overhang: 0.35,
            thickness: 0.2,
            start: RoofEnd::Free,
            end: RoofEnd::Free,
            boards: true,
            ridge_rolls: true,
            ao: 1.0,
        }
    }
}

/// A rectangle roof's frame: `a` runs along the ridge, `b` across.
#[derive(Clone, Copy)]
struct Frame {
    along_x: bool,
}

impl Frame {
    fn w(&self, a: f32, b: f32, y: f32) -> Vec3 {
        if self.along_x {
            Vec3::new(a, y, b)
        } else {
            Vec3::new(b, y, a)
        }
    }
    fn p2(&self, a: f32, b: f32) -> Vec2 {
        if self.along_x {
            Vec2::new(a, b)
        } else {
            Vec2::new(b, a)
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum RectKind {
    Gable,
    Hip,
    Shed { high_abuts: bool },
}

/// Validated parameters.
struct Clean {
    pitch: f32,
    overhang: f32,
    thick: f32,
    ao: f32,
}

fn finite_clamp(v: f32, lo: f32, hi: f32, fallback: f32, name: &str, w: &mut Vec<String>) -> f32 {
    if !v.is_finite() {
        w.push(format!("{name} is not a number; using {fallback}"));
        fallback
    } else if v < lo || v > hi {
        w.push(format!("{name} clamped to {lo}..{hi}"));
        v.clamp(lo, hi)
    } else {
        v
    }
}

impl Roof {
    fn clean(&self, w: &mut Vec<String>) -> Clean {
        Clean {
            pitch: finite_clamp(self.pitch, 0.05, 1.4, 0.47, "pitch", w),
            overhang: finite_clamp(self.overhang, 0.0, 5.0, 0.35, "overhang", w),
            thick: finite_clamp(self.thickness, 0.02, 2.0, 0.2, "thickness", w),
            ao: if self.ao.is_finite() {
                self.ao.clamp(0.0, 1.0)
            } else {
                1.0
            },
        }
    }

    /// The rectangle (half extents along and across the ridge), the frame
    /// and the kind, for rectangular shapes.
    fn rect(&self, w: &mut Vec<String>) -> Option<(f32, f32, Frame, RectKind)> {
        let (size, ridge, kind) = match &self.shape {
            RoofShape::Gable { size, ridge } => (*size, *ridge, RectKind::Gable),
            RoofShape::Hip { size, ridge } => (*size, *ridge, RectKind::Hip),
            RoofShape::Shed {
                size,
                ridge,
                high_abuts,
            } => (
                *size,
                *ridge,
                RectKind::Shed {
                    high_abuts: *high_abuts,
                },
            ),
            _ => return None,
        };
        let size = Vec2::new(
            finite_clamp(size.x, 0.1, 1000.0, 4.0, "size x", w),
            finite_clamp(size.y, 0.1, 1000.0, 4.0, "size z", w),
        );
        let mut along_x = match ridge {
            RidgeAxis::Longer => size.x >= size.y,
            RidgeAxis::X => true,
            RidgeAxis::Z => false,
        };
        // Equal-pitch hips cannot meet along the shorter side.
        let shorter = if along_x {
            size.x < size.y
        } else {
            size.y < size.x
        };
        if kind == RectKind::Hip && shorter {
            w.push("a hip roof's ridge runs along the longer side; ridge axis ignored".into());
            along_x = !along_x;
        }
        let (ha, hb) = if along_x {
            (size.x * 0.5, size.y * 0.5)
        } else {
            (size.y * 0.5, size.x * 0.5)
        };
        Some((ha, hb, Frame { along_x }, kind))
    }

    /// Whether a rectangular roof's ridge (a shed's high edge) runs along
    /// x; `None` for round roofs.
    pub fn ridge_along_x(&self) -> Option<bool> {
        self.rect(&mut Vec::new()).map(|(_, _, f, _)| f.along_x)
    }

    /// The roof's underside in its local space, plus half the slab
    /// thickness: what a wall under it rises to (`Wall::roof`), after
    /// [`RoofUnderside::transformed`] into the wall's space.
    pub fn underside(&self) -> RoofUnderside {
        let mut w = Vec::new();
        let c = self.clean(&mut w);
        let tan = pmath::tan(c.pitch);
        let lift = c.thick * 0.5;
        if let Some((ha, hb, f, kind)) = self.rect(&mut w) {
            let plane = |oa: f32, ob: f32, ia: f32, ib: f32| UndersideSurface::Plane {
                origin: f.p2(oa, ob),
                inward: f.p2(ia, ib),
                height: 0.0,
                slope: tan,
            };
            let mut surfaces = vec![plane(0.0, -hb, 0.0, 1.0)];
            match kind {
                RectKind::Shed { .. } => {}
                RectKind::Gable => surfaces.push(plane(0.0, hb, 0.0, -1.0)),
                RectKind::Hip => {
                    surfaces.push(plane(0.0, hb, 0.0, -1.0));
                    if self.start == RoofEnd::Free {
                        surfaces.push(plane(-ha, 0.0, 1.0, 0.0));
                    }
                    if self.end == RoofEnd::Free {
                        surfaces.push(plane(ha, 0.0, -1.0, 0.0));
                    }
                }
            }
            return RoofUnderside { surfaces, lift };
        }
        let surface = match self.shape {
            RoofShape::Cone { radius, .. } => UndersideSurface::Cone {
                centre: Vec2::ZERO,
                radius: radius.max(0.1),
                height: 0.0,
                slope: tan,
            },
            RoofShape::Dome { radius, rise, .. } => {
                let r = radius.max(0.1);
                let re = r + c.overhang;
                let rise = rise.max(0.05);
                UndersideSurface::Dome {
                    centre: Vec2::ZERO,
                    radius: re,
                    height: -rise * (1.0 - (r / re).powi(2)).sqrt(),
                    rise,
                }
            }
            _ => unreachable!("rectangles handled above"),
        };
        RoofUnderside {
            surfaces: vec![surface],
            lift,
        }
    }

    /// Builds the roof mesh.
    pub fn build(&self) -> GenOutput {
        let mut warnings = Vec::new();
        let c = self.clean(&mut warnings);
        let mut mb = ArchMesh::new(c.ao);
        if let Some((ha, hb, f, kind)) = self.rect(&mut warnings) {
            self.rect_roof(&mut mb, &c, ha, hb, f, kind);
        } else {
            match self.shape {
                RoofShape::Cone { radius, segments } => {
                    let r = finite_clamp(radius, 0.1, 100.0, 2.0, "radius", &mut warnings);
                    let n = segments.clamp(3, 256) as usize;
                    let tan = pmath::tan(c.pitch);
                    let re = r + c.overhang;
                    let ye = -c.overhang * tan;
                    let apex = ye + re * tan;
                    let profile: Vec<Vec2> = vec![Vec2::new(re, ye), Vec2::new(0.0, apex)];
                    self.round_roof(&mut mb, &c, r, n, &profile);
                }
                RoofShape::Dome {
                    radius,
                    rise,
                    segments,
                    rings,
                } => {
                    let r = finite_clamp(radius, 0.1, 100.0, 2.0, "radius", &mut warnings);
                    let rise = finite_clamp(rise, 0.05, 100.0, r, "rise", &mut warnings);
                    let n = segments.clamp(3, 256) as usize;
                    let rings = rings.clamp(2, 64) as usize;
                    let re = r + c.overhang;
                    let ye = -rise * (1.0 - (r / re).powi(2)).sqrt();
                    let profile: Vec<Vec2> = (0..=rings)
                        .map(|k| {
                            let phi = FRAC_PI_2 * k as f32 / rings as f32;
                            Vec2::new(re * pmath::cos(phi), ye + rise * pmath::sin(phi))
                        })
                        .collect();
                    self.round_roof(&mut mb, &c, r, n, &profile);
                }
                _ => {}
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

    fn rect_roof(&self, mb: &mut ArchMesh, c: &Clean, ha: f32, hb: f32, f: Frame, kind: RectKind) {
        let (wa0, wa1, wb0, wb1) = (-ha, ha, -hb, hb);
        let tan = pmath::tan(c.pitch);
        let ov = c.overhang;
        let up = c.thick;
        let ye = -ov * tan;
        let a0 = match self.start {
            RoofEnd::Free => wa0 - ov,
            RoofEnd::Abut => wa0 - ABUT_PENETRATION,
        };
        let a1 = match self.end {
            RoofEnd::Free => wa1 + ov,
            RoofEnd::Abut => wa1 + ABUT_PENETRATION,
        };
        let be0 = wb0 - ov;
        // Faces as plan polygons (a, b) with a height function.
        struct Face {
            poly: Vec<Vec2>,
            origin: Vec2,
            inward: Vec2,
        }
        let mut faces = Vec::new();
        let (bc, yr, ra, rb, hip0, hip1, be1);
        match kind {
            RectKind::Shed { high_abuts } => {
                be1 = if high_abuts {
                    wb1 + ABUT_PENETRATION
                } else {
                    wb1 + ov
                };
                faces.push(Face {
                    poly: vec![
                        Vec2::new(a0, be0),
                        Vec2::new(a1, be0),
                        Vec2::new(a1, be1),
                        Vec2::new(a0, be1),
                    ],
                    origin: Vec2::new(0.0, be0),
                    inward: Vec2::Y,
                });
                bc = be1;
                yr = ye + (be1 - be0) * tan;
                (ra, rb, hip0, hip1) = (a0, a1, false, false);
            }
            RectKind::Gable | RectKind::Hip => {
                be1 = wb1 + ov;
                bc = 0.0;
                let hspan = hb + ov;
                yr = ye + hspan * tan;
                hip0 = kind == RectKind::Hip && self.start == RoofEnd::Free;
                hip1 = kind == RectKind::Hip && self.end == RoofEnd::Free;
                let mut r0 = if hip0 { a0 + hspan } else { a0 };
                let mut r1 = if hip1 { a1 - hspan } else { a1 };
                if r0 > r1 {
                    let m = (r0 + r1) * 0.5;
                    r0 = m;
                    r1 = m;
                }
                (ra, rb) = (r0, r1);
                faces.push(Face {
                    poly: vec![
                        Vec2::new(a0, be0),
                        Vec2::new(a1, be0),
                        Vec2::new(rb, bc),
                        Vec2::new(ra, bc),
                    ],
                    origin: Vec2::new(0.0, be0),
                    inward: Vec2::Y,
                });
                faces.push(Face {
                    poly: vec![
                        Vec2::new(a1, be1),
                        Vec2::new(a0, be1),
                        Vec2::new(ra, bc),
                        Vec2::new(rb, bc),
                    ],
                    origin: Vec2::new(0.0, be1),
                    inward: -Vec2::Y,
                });
                if hip0 {
                    faces.push(Face {
                        poly: vec![Vec2::new(a0, be1), Vec2::new(a0, be0), Vec2::new(ra, bc)],
                        origin: Vec2::new(a0, 0.0),
                        inward: Vec2::X,
                    });
                }
                if hip1 {
                    faces.push(Face {
                        poly: vec![Vec2::new(a1, be0), Vec2::new(a1, be1), Vec2::new(rb, bc)],
                        origin: Vec2::new(a1, 0.0),
                        inward: -Vec2::X,
                    });
                }
            }
        }
        for face in &mut faces {
            face.poly.dedup_by(|a, b| a.distance_squared(*b) < 1e-10);
            if face.poly.len() > 2
                && face.poly[0].distance_squared(face.poly[face.poly.len() - 1]) < 1e-10
            {
                face.poly.pop();
            }
        }
        let height = |face: &Face, q: Vec2| ye + tan * (q - face.origin).dot(face.inward);
        // Soffit occlusion: darkest at the wall line, lighter toward the eave.
        let free_lo = self.start == RoofEnd::Free;
        let free_hi = self.end == RoofEnd::Free;
        let shed_high_free = !matches!(kind, RectKind::Shed { high_abuts: true });
        let soffit_ao = |q: Vec2| {
            let db = (wb0 - q.y)
                .max(if shed_high_free { q.y - wb1 } else { 0.0 })
                .max(0.0);
            let lo = if free_lo { wa0 - q.x } else { 0.0 };
            let hi = if free_hi { q.x - wa1 } else { 0.0 };
            let d = db.max(lo.max(hi).max(0.0));
            SOFFIT_AO + (0.9 - SOFFIT_AO) * (d / ov.max(0.01)).min(1.0)
        };
        // Outer boundary of the slab: both ends of an edge on the same side.
        let side_of = |q: Vec2| -> u8 {
            let e = 1e-5;
            let mut s = 0u8;
            if (q.x - a0).abs() < e {
                s |= 1;
            }
            if (q.x - a1).abs() < e {
                s |= 2;
            }
            if (q.y - be0).abs() < e {
                s |= 4;
            }
            if (q.y - be1).abs() < e {
                s |= 8;
            }
            s
        };
        for face in &faces {
            let n_up = {
                let g = face.inward * -tan;
                f.w(g.x, g.y, 1.0).normalize()
            };
            // Tiles, soffit and slab edges all split at the wall lines (so
            // the soffit gradient lands there) the same way: no T-junctions.
            let mut cuts_b = vec![wb0];
            if shed_high_free {
                cuts_b.push(wb1);
            }
            let mut cuts_a = Vec::new();
            if free_lo {
                cuts_a.push(wa0);
            }
            if free_hi {
                cuts_a.push(wa1);
            }
            for pc in clip::split_convex(face.poly.clone(), 1, &cuts_b) {
                for pc in clip::split_convex(pc, 0, &cuts_a) {
                    let top: Vec<Vec3> = pc
                        .iter()
                        .map(|&q| f.w(q.x, q.y, height(face, q) + up))
                        .collect();
                    mb.poly(slot::ROOF, &top, &vec![1.0; top.len()], n_up);
                    let pts: Vec<Vec3> =
                        pc.iter().map(|&q| f.w(q.x, q.y, height(face, q))).collect();
                    let ao: Vec<f32> = pc.iter().map(|&q| soffit_ao(q)).collect();
                    mb.poly(slot::UNDERSIDE, &pts, &ao, -n_up);
                    // Slab edges on the outer boundary.
                    let n = pc.len();
                    for k in 0..n {
                        let (p, q) = (pc[k], pc[(k + 1) % n]);
                        let side = side_of(p) & side_of(q);
                        if side == 0 {
                            continue;
                        }
                        let out = match side.trailing_zeros() {
                            0 => Vec2::new(-1.0, 0.0),
                            1 => Vec2::new(1.0, 0.0),
                            2 => Vec2::new(0.0, -1.0),
                            _ => Vec2::new(0.0, 1.0),
                        };
                        let (hp, hq) = (height(face, p), height(face, q));
                        let quad = [
                            f.w(p.x, p.y, hp),
                            f.w(q.x, q.y, hq),
                            f.w(q.x, q.y, hq + up),
                            f.w(p.x, p.y, hp + up),
                        ];
                        mb.quad(
                            slot::ROOF,
                            quad,
                            [0.85, 0.85, 1.0, 1.0],
                            f.w(out.x, out.y, 0.0),
                        );
                    }
                }
            }
        }
        if self.boards {
            // Fascia along the eaves, barge boards along the verges; the
            // eave boards along the ridge own the corners.
            let bt = BOARD_THICKNESS;
            let (below, above) = (BOARD_BELOW, up + BOARD_ABOVE);
            let e = |a: f32, b: f32| f.w(a, b, ye);
            let ext0 = if free_lo { bt } else { 0.0 };
            let ext1 = if free_hi { bt } else { 0.0 };
            mb.plank(
                slot::TRIM,
                e(a0 - ext0, be0),
                e(a1 + ext1, be0),
                f.w(0.0, -1.0, 0.0),
                bt,
                below,
                above,
                0.75,
            );
            let high_y = ye + (be1 - be0) * tan;
            match kind {
                RectKind::Shed { high_abuts } => {
                    if !high_abuts {
                        let h = |a: f32| f.w(a, be1, high_y);
                        mb.plank(
                            slot::TRIM,
                            h(a0 - ext0),
                            h(a1 + ext1),
                            f.w(0.0, 1.0, 0.0),
                            bt,
                            below,
                            above,
                            0.75,
                        );
                    }
                    for (free, at, sgn) in [(free_lo, a0, -1.0f32), (free_hi, a1, 1.0)] {
                        if free {
                            mb.plank(
                                slot::TRIM,
                                e(at, be0),
                                f.w(at, be1, high_y),
                                f.w(sgn, 0.0, 0.0),
                                bt,
                                below,
                                above,
                                0.8,
                            );
                        }
                    }
                }
                RectKind::Gable | RectKind::Hip => {
                    mb.plank(
                        slot::TRIM,
                        e(a0 - ext0, be1),
                        e(a1 + ext1, be1),
                        f.w(0.0, 1.0, 0.0),
                        bt,
                        below,
                        above,
                        0.75,
                    );
                    for (free, at, sgn, hip) in
                        [(free_lo, a0, -1.0f32, hip0), (free_hi, a1, 1.0, hip1)]
                    {
                        let out = f.w(sgn, 0.0, 0.0);
                        if hip {
                            mb.plank(
                                slot::TRIM,
                                e(at, be0),
                                e(at, be1),
                                out,
                                bt,
                                below,
                                above,
                                0.75,
                            );
                        } else if free {
                            let r = f.w(at, bc, yr);
                            mb.plank(slot::TRIM, e(at, be0), r, out, bt, below, above, 0.8);
                            mb.plank(slot::TRIM, r, e(at, be1), out, bt, below, above, 0.8);
                        }
                    }
                }
            }
        }
        if self.ridge_rolls && !matches!(kind, RectKind::Shed { .. }) {
            if rb - ra > 1e-3 {
                mb.tent(
                    slot::RIDGE,
                    f.w(ra, bc, yr + up),
                    f.w(rb, bc, yr + up),
                    RIDGE_WIDTH,
                    RIDGE_HEIGHT,
                    RIDGE_SINK,
                    0.8,
                );
            }
            for (hip, at, r) in [(hip0, a0, ra), (hip1, a1, rb)] {
                if !hip {
                    continue;
                }
                for be in [be0, be1] {
                    // Start a little in from the eave corner so the roll's
                    // end stays on the tiles.
                    let c0 = f.w(at, be, ye + up);
                    let top = f.w(r, bc, yr + up);
                    let a = c0 + (top - c0).normalize_or_zero() * 0.12;
                    mb.tent(
                        slot::RIDGE,
                        a,
                        top,
                        RIDGE_WIDTH * 0.85,
                        RIDGE_HEIGHT,
                        RIDGE_SINK,
                        0.8,
                    );
                }
            }
        }
    }

    /// A slab of revolution: `profile` is the underside (d, y) from the
    /// eave ring (d = radius + overhang) to the apex (d = 0); the top is the
    /// same raised by the thickness.
    fn round_roof(&self, mb: &mut ArchMesh, c: &Clean, r: f32, n: usize, profile: &[Vec2]) {
        let up = c.thick;
        let re = profile[0].x;
        let dir = |k: usize| {
            let a = TAU * k as f32 / n as f32;
            Vec2::new(pmath::cos(a), pmath::sin(a))
        };
        // Profile normals (outward-up), averaged at inner profile points.
        let seg_n = |i: usize| {
            let t = profile[i + 1] - profile[i];
            Vec2::new(-t.y, t.x).normalize_or(Vec2::Y) * if t.x < 0.0 { -1.0 } else { 1.0 }
        };
        let pn: Vec<Vec2> = (0..profile.len())
            .map(|i| {
                let a = if i > 0 { seg_n(i - 1) } else { seg_n(0) };
                let b = if i + 1 < profile.len() { seg_n(i) } else { a };
                let m = (a + b).normalize_or(Vec2::Y);
                if m.y < 0.0 { -m } else { m }
            })
            .collect();
        let soffit_ao = |d: f32| {
            if d >= r {
                SOFFIT_AO + (0.9 - SOFFIT_AO) * ((d - r) / c.overhang.max(0.01)).min(1.0)
            } else {
                SOFFIT_AO
            }
        };
        let p3 = |k: usize, q: Vec2, y: f32| {
            let d = dir(k % n);
            Vec3::new(d.x * q.x, y, d.y * q.x)
        };
        let n3 = |k: usize, m: Vec2| {
            let d = dir(k % n);
            Vec3::new(d.x * m.x, m.y, d.y * m.x).normalize_or(Vec3::Y)
        };
        for k in 0..n {
            for i in 0..profile.len() - 1 {
                let (q0, q1) = (profile[i], profile[i + 1]);
                // Top.
                let t = [
                    p3(k, q0, q0.y + up),
                    p3(k + 1, q0, q0.y + up),
                    p3(k + 1, q1, q1.y + up),
                    p3(k, q1, q1.y + up),
                ];
                let tn = [
                    n3(k, pn[i]),
                    n3(k + 1, pn[i]),
                    n3(k + 1, pn[i + 1]),
                    n3(k, pn[i + 1]),
                ];
                let facing = tn[0] + tn[1] + tn[2] + tn[3];
                mb.tri_n(
                    slot::ROOF,
                    [t[0], t[1], t[2]],
                    [tn[0], tn[1], tn[2]],
                    [1.0; 3],
                    facing,
                );
                mb.tri_n(
                    slot::ROOF,
                    [t[0], t[2], t[3]],
                    [tn[0], tn[2], tn[3]],
                    [1.0; 3],
                    facing,
                );
                // Underside (faces down/in).
                let s = [
                    p3(k, q0, q0.y),
                    p3(k + 1, q0, q0.y),
                    p3(k + 1, q1, q1.y),
                    p3(k, q1, q1.y),
                ];
                let sn = tn.map(|v| -v);
                let ao = [
                    soffit_ao(q0.x),
                    soffit_ao(q0.x),
                    soffit_ao(q1.x),
                    soffit_ao(q1.x),
                ];
                mb.tri_n(
                    slot::UNDERSIDE,
                    [s[0], s[1], s[2]],
                    [sn[0], sn[1], sn[2]],
                    [ao[0], ao[1], ao[2]],
                    -facing,
                );
                mb.tri_n(
                    slot::UNDERSIDE,
                    [s[0], s[2], s[3]],
                    [sn[0], sn[2], sn[3]],
                    [ao[0], ao[2], ao[3]],
                    -facing,
                );
            }
            // Eave edge.
            let q = profile[0];
            let out = n3(k, Vec2::X) + n3(k + 1, Vec2::X);
            let e = [
                p3(k, q, q.y),
                p3(k + 1, q, q.y),
                p3(k + 1, q, q.y + up),
                p3(k, q, q.y + up),
            ];
            mb.quad(slot::ROOF, e, [0.8, 0.8, 1.0, 1.0], out);
        }
        if self.boards {
            // A fascia ring just outside the eave edge.
            let q = profile[0];
            let rb = re + BOARD_THICKNESS;
            for k in 0..n {
                let (y0, y1) = (q.y - BOARD_BELOW, q.y + up + BOARD_ABOVE);
                let pa = |k: usize, d: f32, y: f32| {
                    let dd = dir(k % n);
                    Vec3::new(dd.x * d, y, dd.y * d)
                };
                let out = n3(k, Vec2::X) + n3(k + 1, Vec2::X);
                mb.quad(
                    slot::TRIM,
                    [
                        pa(k, rb, y0),
                        pa(k + 1, rb, y0),
                        pa(k + 1, rb, y1),
                        pa(k, rb, y1),
                    ],
                    [0.75, 0.75, 1.0, 1.0],
                    out,
                );
                mb.quad(
                    slot::TRIM,
                    [
                        pa(k, re, y1),
                        pa(k + 1, re, y1),
                        pa(k + 1, rb, y1),
                        pa(k, rb, y1),
                    ],
                    [1.0; 4],
                    Vec3::Y,
                );
                mb.quad(
                    slot::TRIM,
                    [
                        pa(k, re, y0),
                        pa(k + 1, re, y0),
                        pa(k + 1, rb, y0),
                        pa(k, rb, y0),
                    ],
                    [0.7; 4],
                    -Vec3::Y,
                );
            }
        }
    }
}

impl Generator for Roof {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Roof", "Underside", "Trim", "Ridge"];
    const CATEGORY: &'static str = "Architecture";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        self.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underside_passes_through_the_wall_plate() {
        for shape in [
            RoofShape::Gable {
                size: Vec2::new(6.0, 4.0),
                ridge: RidgeAxis::Longer,
            },
            RoofShape::Hip {
                size: Vec2::new(6.0, 4.0),
                ridge: RidgeAxis::Longer,
            },
        ] {
            let roof = Roof {
                shape,
                ..Roof::default()
            };
            let u = roof.underside();
            // Middle of the long wall's outer face.
            assert!((u.at(Vec2::new(0.0, -2.0)) - roof.thickness * 0.5).abs() < 1e-4);
        }
        let cone = Roof {
            shape: RoofShape::Cone {
                radius: 2.0,
                segments: 24,
            },
            ..Roof::default()
        };
        assert!((cone.underside().at(Vec2::new(2.0, 0.0)) - 0.1).abs() < 1e-4);
        let dome = Roof {
            shape: RoofShape::Dome {
                radius: 2.0,
                rise: 1.5,
                segments: 24,
                rings: 8,
            },
            ..Roof::default()
        };
        assert!((dome.underside().at(Vec2::new(0.0, 2.0)) - 0.1).abs() < 1e-4);
    }
}
