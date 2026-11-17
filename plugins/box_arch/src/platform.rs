//! The [`Platform`] generator: a slab from a footprint polygon (floors,
//! terraces, landings), with an optional raised lip or a trim band round
//! its edge. Its walking surface is at y = 0; the slab hangs below.

use bevy_reflect::Reflect;
use box_core::reflect::{Range, ReflectComponent, ReflectDefault, Step, Unit};
use box_proc::generator::{GenContext, GenOutput, Generator};
use box_proc::poly::{self, Join, Polygon};
use glam::{Vec2, Vec3};

use crate::mesh::ArchMesh;

/// Material slots of [`Platform`].
pub mod slot {
    pub const TOP: u32 = 0;
    pub const SIDES: u32 = 1;
    pub const TRIM: u32 = 2;
}

/// Occlusion at the foot of a lip, fading over [`LIP_AO_WIDTH`].
const LIP_AO: f32 = 0.55;
const LIP_AO_WIDTH: f32 = 0.45;

/// A platform's outline in plan (x, z), centred on the entity's origin.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum Footprint {
    Rectangle {
        #[reflect(@Unit("m"), @Step(0.01))]
        size: Vec2,
    },
    Circle {
        #[reflect(@Range { min: 0.05, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
    },
    /// Your own outline: plan points (x, z), any winding.
    Polygon { points: Vec<Vec2> },
}

impl Default for Footprint {
    fn default() -> Self {
        Self::Rectangle {
            size: Vec2::new(4.0, 3.0),
        }
    }
}

impl Footprint {
    /// The outline as a normalized polygon (x, z) and a warning when a
    /// self-crossing outline had to be reduced to its largest piece.
    pub fn polygon(&self) -> Result<(Polygon, Option<String>), String> {
        let p = match self {
            Self::Rectangle { size } => {
                if !(size.x > 0.0 && size.y > 0.0 && size.is_finite()) {
                    return Err("rectangle size must be above 0".into());
                }
                Polygon::rect(-*size * 0.5, *size * 0.5)
            }
            Self::Circle { radius, segments } => {
                if !(*radius > 0.0 && radius.is_finite()) {
                    return Err("circle radius must be above 0".into());
                }
                Polygon::circle(Vec2::ZERO, *radius, (*segments).clamp(3, 256))
            }
            Self::Polygon { points } => Polygon::new(points.clone()),
        };
        let mut valid = poly::make_valid(&[p]);
        let many = valid.len() > 1;
        valid.sort_by(|a, b| b.area().total_cmp(&a.area()));
        match valid.into_iter().next() {
            None => Err("the footprint has no area".into()),
            Some(p) => Ok((
                tidy(vec![p]).into_iter().next().unwrap_or_default(),
                many.then(|| {
                    "the footprint crosses itself into several pieces; keeping the largest".into()
                }),
            )),
        }
    }
}

/// What runs round the platform's edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum PlatformEdge {
    #[default]
    None,
    /// A band round the outside of the slab, flush with the top.
    Trim {
        #[reflect(@Range { min: 0.01, max: 5.0 }, @Unit("m"), @Step(0.01))]
        height: f32,
        #[reflect(@Range { min: 0.005, max: 1.0 }, @Unit("m"), @Step(0.005))]
        proud: f32,
    },
    /// A raised rim (a kerb or a low parapet) inside the outline.
    Lip {
        #[reflect(@Range { min: 0.01, max: 5.0 }, @Unit("m"), @Step(0.01))]
        height: f32,
        #[reflect(@Range { min: 0.01, max: 5.0 }, @Unit("m"), @Step(0.01))]
        width: f32,
    },
}

/// A slab from a footprint: floors, terraces, landings. The top is at
/// y = 0. Material slots: Top, Sides (sides and bottom), Trim.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct Platform {
    pub footprint: Footprint,
    #[reflect(@Range { min: 0.01, max: 50.0 }, @Unit("m"), @Step(0.01))]
    pub thickness: f32,
    pub edge: PlatformEdge,
    /// Strength of the baked ambient occlusion (0 turns it off).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.05))]
    pub ao: f32,
}

impl Default for Platform {
    fn default() -> Self {
        Self {
            footprint: Footprint::default(),
            thickness: 0.3,
            edge: PlatformEdge::None,
            ao: 1.0,
        }
    }
}

fn to3(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

/// Vertical walls along every edge of `ring` from `y0` to `y1`, facing away
/// from the solid on the ring's left (normalized rings: outer
/// counter-clockwise, holes clockwise).
fn ring_walls(
    mb: &mut ArchMesh,
    slot: u32,
    ring: &[Vec2],
    y0: f32,
    y1: f32,
    ao: [f32; 2],
    inward: bool,
) {
    for k in 0..ring.len() {
        let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
        let d = (b - a).normalize_or_zero();
        let mut n = Vec2::new(d.y, -d.x);
        if inward {
            n = -n;
        }
        mb.quad(
            slot,
            [to3(a, y0), to3(b, y0), to3(b, y1), to3(a, y1)],
            [ao[0], ao[0], ao[1], ao[1]],
            to3(n, 0.0),
        );
    }
}

/// A horizontal polygon (holes allowed) at height `y` facing up or down,
/// with occlusion per vertex.
fn cap(
    mb: &mut ArchMesh,
    slot: u32,
    polygon: &Polygon,
    y: f32,
    up: bool,
    ao: impl Fn(Vec2) -> f32,
) {
    let t = poly::triangulate(polygon);
    let facing = if up { Vec3::Y } else { -Vec3::Y };
    for i in t.indices.as_chunks::<3>().0 {
        let q = [0, 1, 2].map(|k| t.points[i[k] as usize]);
        mb.tri_n(slot, q.map(|p| to3(p, y)), [facing; 3], q.map(&ao), facing);
    }
}

/// Rings without collinear points: triangulation may skip a collinear
/// vertex that the side walls keep, leaving a T-junction.
fn tidy(polys: Vec<Polygon>) -> Vec<Polygon> {
    polys
        .into_iter()
        .map(|p| {
            Polygon {
                outer: poly::simplify_ring(&p.outer, 1e-5),
                holes: p
                    .holes
                    .iter()
                    .map(|h| poly::simplify_ring(h, 1e-5))
                    .filter(|h| h.len() >= 3)
                    .collect(),
            }
            .normalized()
        })
        .filter(|p| !p.outer.is_empty())
        .collect()
}

/// `outer` minus `inner`, where `inner` lies inside `outer` (an inset or
/// an outset of it): one polygon with a hole when both are single simple
/// polygons (no boolean needed), else the boolean difference.
fn between(outer: &[Polygon], inner: &[Polygon]) -> Vec<Polygon> {
    match (outer, inner) {
        ([o], [i]) if o.holes.is_empty() && i.holes.is_empty() => vec![
            Polygon {
                outer: o.outer.clone(),
                holes: vec![i.outer.clone()],
            }
            .normalized(),
        ],
        _ => tidy(poly::difference(outer, inner)),
    }
}

impl Platform {
    pub fn build(&self) -> GenOutput {
        let mut warnings = Vec::new();
        let fp = match self.footprint.polygon() {
            Ok((p, w)) => {
                warnings.extend(w);
                p
            }
            Err(e) => return GenOutput::empty().warn(e),
        };
        let thick = if self.thickness.is_finite() && self.thickness >= 0.01 {
            self.thickness
        } else {
            warnings.push("thickness must be at least 0.01 m; using 0.3".into());
            0.3
        };
        let ao = if self.ao.is_finite() {
            self.ao.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let mut mb = ArchMesh::new(ao);
        // Slab sides and bottom.
        for (i, ring) in fp.rings().enumerate() {
            ring_walls(&mut mb, slot::SIDES, ring, -thick, 0.0, [1.0, 1.0], i > 0);
        }
        cap(&mut mb, slot::SIDES, &fp, -thick, false, |_| 0.8);
        match self.edge {
            PlatformEdge::None => cap(&mut mb, slot::TOP, &fp, 0.0, true, |_| 1.0),
            PlatformEdge::Trim { height, proud } => {
                cap(&mut mb, slot::TOP, &fp, 0.0, true, |_| 1.0);
                let (height, proud) = (height.clamp(0.01, 5.0), proud.clamp(0.005, 1.0));
                let grown = tidy(poly::offset(
                    std::slice::from_ref(&fp),
                    proud,
                    Join::Miter { limit: 4.0 },
                ));
                let band = between(&grown, std::slice::from_ref(&fp));
                for p in &band {
                    for (i, ring) in p.rings().enumerate() {
                        // Outer rings of the band face out, its holes (the
                        // slab's outline) face in against the slab: skip them.
                        if i == 0 {
                            ring_walls(&mut mb, slot::TRIM, ring, -height, 0.0, [0.9, 1.0], false);
                        }
                    }
                    cap(&mut mb, slot::TRIM, p, 0.0, true, |_| 1.0);
                    cap(&mut mb, slot::TRIM, p, -height, false, |_| 0.7);
                }
            }
            PlatformEdge::Lip { height, width } => {
                let (height, width) = (height.clamp(0.01, 5.0), width.clamp(0.01, 5.0));
                let inner = tidy(poly::offset(
                    std::slice::from_ref(&fp),
                    -width,
                    Join::Miter { limit: 4.0 },
                ));
                if inner.is_empty() {
                    warnings
                        .push("the lip is wider than the platform; drawn as a raised slab".into());
                    cap(&mut mb, slot::TRIM, &fp, height, true, |_| 1.0);
                    for (i, ring) in fp.rings().enumerate() {
                        ring_walls(&mut mb, slot::TRIM, ring, 0.0, height, [1.0, 1.0], i > 0);
                    }
                } else {
                    // Floor inside the lip, darker at its foot.
                    // Band vertices lie on the lip's foot (darkest) or on the
                    // core's edge (open).
                    let foot: std::collections::HashSet<(u32, u32)> = inner
                        .iter()
                        .flat_map(|p| p.rings())
                        .flatten()
                        .map(|q| (q.x.to_bits(), q.y.to_bits()))
                        .collect();
                    let occl = |p: Vec2| {
                        if foot.contains(&(p.x.to_bits(), p.y.to_bits())) {
                            LIP_AO
                        } else {
                            1.0
                        }
                    };
                    let core = if ao > 0.0 {
                        tidy(poly::offset(
                            &inner,
                            -LIP_AO_WIDTH,
                            Join::Miter { limit: 4.0 },
                        ))
                    } else {
                        Vec::new()
                    };
                    let band = if core.is_empty() {
                        inner.clone()
                    } else {
                        between(&inner, &core)
                    };
                    for p in &band {
                        cap(&mut mb, slot::TOP, p, 0.0, true, occl);
                    }
                    for p in &core {
                        cap(&mut mb, slot::TOP, p, 0.0, true, |_| 1.0);
                    }
                    // The rim: top, outer walls up from the slab, inner walls.
                    let rim = between(std::slice::from_ref(&fp), &inner);
                    for p in &rim {
                        cap(&mut mb, slot::TRIM, p, height, true, |_| 1.0);
                    }
                    for (i, ring) in fp.rings().enumerate() {
                        ring_walls(&mut mb, slot::TRIM, ring, 0.0, height, [1.0, 1.0], i > 0);
                    }
                    for p in &inner {
                        for (i, ring) in p.rings().enumerate() {
                            // Facing into the floor area.
                            ring_walls(
                                &mut mb,
                                slot::TRIM,
                                ring,
                                0.0,
                                height,
                                [LIP_AO, 1.0],
                                i == 0,
                            );
                        }
                    }
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

impl Generator for Platform {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Top", "Sides", "Trim"];
    const CATEGORY: &'static str = "Architecture";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        self.build()
    }
}
