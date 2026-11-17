//! Extrude: a 2D polygon (with holes) to a prism, with optional caps and a
//! chamfer or rounded bevel on both cap edges.
//!
//! The profile lies in `params.frame` and is extruded `depth` metres along
//! the frame's unit normal (`u × v`); the default frame is the ground plane
//! ([`PlaneFrame::PLAN`]), so footprints extrude upward. Bevels are done on
//! the 2D profile (docs/PROCEDURAL.md section 3): each bevel step is the
//! profile inset by a per-vertex mitre offset, which keeps the vertex
//! correspondence so rings stitch with quads. To chamfer or round the
//! vertical edges instead, shape the profile first with
//! [`poly::chamfer_corners`] or [`poly::round_corners`].
//!
//! UVs (metres × `uv_scale`): sides U = length along the ring, V = distance
//! from the bottom edge along the side (bevels included); caps use the 2D
//! profile coordinates (mirrored in U on the bottom cap).

use box_core::pmath;
use glam::{Vec2, Vec3};

use crate::mesh_builder::{BuiltMesh, MeshBuilder, PlaneFrame, Vertex, ring_columns};
use crate::poly::{self, Polygon};

/// Extrusion parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtrudeParams {
    /// Metres along the frame normal. Negative extrudes the other way.
    pub depth: f32,
    pub frame: PlaneFrame,
    /// Bevel width on both cap edges, metres (0 = none). Reduced (with a
    /// warning) when the profile is too thin for it or it exceeds half the
    /// depth.
    pub bevel_width: f32,
    /// 1 = chamfer; more = a rounded (quarter-circle) bevel. Clamped to 1..=16.
    pub bevel_segments: u32,
    pub cap_start: bool,
    pub cap_end: bool,
    /// Ring vertices where the outline turns by at most this many radians
    /// are smoothed (default 30°: circles smooth, boxes hard).
    pub smooth_angle: f32,
    pub side_slot: u32,
    pub cap_slot: u32,
    pub bevel_slot: u32,
    pub uv_scale: f32,
}

impl Default for ExtrudeParams {
    fn default() -> Self {
        Self {
            depth: 1.0,
            frame: PlaneFrame::PLAN,
            bevel_width: 0.0,
            bevel_segments: 1,
            cap_start: true,
            cap_end: true,
            smooth_angle: 30f32.to_radians(),
            side_slot: 0,
            cap_slot: 1,
            bevel_slot: 0,
            uv_scale: 1.0,
        }
    }
}

/// Extrudes `profile` into a new mesh.
pub fn extrude(profile: &Polygon, params: &ExtrudeParams) -> BuiltMesh {
    let mut mb = MeshBuilder::new();
    extrude_into(&mut mb, profile, params);
    mb.build()
}

/// Unit-width inset vectors (to the left of each edge, i.e. into the
/// material for normalized rings). `None` at a vertex that folds back.
fn inset_vectors(ring: &[Vec2]) -> Vec<Option<Vec2>> {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, p, b) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let n1 = (p - a).normalize_or_zero().perp();
            let n2 = (b - p).normalize_or_zero().perp();
            let k = 1.0 + n1.dot(n2);
            (k > 0.05).then(|| (n1 + n2) / k)
        })
        .collect()
}

/// True when insetting every ring by `w` keeps every edge's direction.
fn inset_ok(rings: &[Vec<Vec2>], offs: &[Vec<Option<Vec2>>], w: f32) -> bool {
    rings.iter().zip(offs).all(|(r, o)| {
        let n = r.len();
        (0..n).all(|i| {
            let j = (i + 1) % n;
            match (o[i], o[j]) {
                (Some(a), Some(b)) => {
                    let old = r[j] - r[i];
                    let new = (r[j] + b * w) - (r[i] + a * w);
                    new.dot(old) > 0.0
                }
                _ => false,
            }
        })
    })
}

/// Extrudes into an existing builder (warnings go to the builder).
pub fn extrude_into(mb: &mut MeshBuilder, profile: &Polygon, params: &ExtrudeParams) {
    let p = profile.normalized();
    if p.outer.is_empty() {
        mb.warn("extrude: profile has no area");
        return;
    }
    if !params.frame.is_valid() {
        mb.warn("extrude: degenerate frame");
        return;
    }
    let mut frame = params.frame;
    let axis = frame.normal();
    let mut depth = if params.depth.is_finite() {
        params.depth
    } else {
        mb.warn("extrude: non-finite depth treated as 0");
        0.0
    };
    if depth < 0.0 {
        frame = frame.translated(axis * depth);
        depth = -depth;
    }
    let uv_scale = if params.uv_scale.is_finite() {
        params.uv_scale
    } else {
        1.0
    };
    if depth == 0.0 {
        mb.warn("extrude: zero depth, only the polygon is drawn");
        mb.fill_polygon(params.cap_slot, &p, &frame, false, uv_scale);
        return;
    }
    let rings: Vec<Vec<Vec2>> = p.rings().cloned().collect();
    let offs: Vec<Vec<Option<Vec2>>> = rings.iter().map(|r| inset_vectors(r)).collect();

    // Bevel width: at most just under half the depth, reduced until the
    // inset profile stays valid.
    let mut w = if params.bevel_width.is_finite() {
        params.bevel_width.max(0.0)
    } else {
        0.0
    };
    if w > depth * 0.49 {
        w = depth * 0.49;
        mb.warn("extrude: bevel wider than half the depth; reduced");
    }
    if w > 0.0 && !inset_ok(&rings, &offs, w) {
        let requested = w;
        while w > requested / 256.0 && !inset_ok(&rings, &offs, w) {
            w *= 0.5;
        }
        if inset_ok(&rings, &offs, w) {
            mb.warn(format!(
                "extrude: bevel reduced from {requested:.3} m to {w:.3} m to fit the profile"
            ));
        } else {
            w = 0.0;
            mb.warn("extrude: profile too thin or sharp for a bevel; bevel dropped");
        }
    }
    let segs = params.bevel_segments.clamp(1, 16);

    // Layers bottom to top: (inset, height, band kind of the band *below*).
    #[derive(Clone, Copy, PartialEq)]
    enum Band {
        BevelBottom,
        Side,
        BevelTop,
    }
    let mut layers: Vec<(f32, f32, f32)> = Vec::new(); // (inset, height, arc angle)
    let mut bands: Vec<Band> = Vec::new();
    if w > 0.0 {
        for j in 0..=segs {
            let phi = std::f32::consts::FRAC_PI_2 * j as f32 / segs as f32;
            layers.push((
                w * (1.0 - pmath::sin(phi)),
                w * (1.0 - pmath::cos(phi)),
                phi,
            ));
            if j > 0 {
                bands.push(Band::BevelBottom);
            }
        }
        bands.push(Band::Side);
        for j in (0..=segs).rev() {
            let phi = std::f32::consts::FRAC_PI_2 * j as f32 / segs as f32;
            layers.push((
                w * (1.0 - pmath::sin(phi)),
                depth - w * (1.0 - pmath::cos(phi)),
                phi,
            ));
            if j < segs {
                bands.push(Band::BevelTop);
            }
        }
    } else {
        layers.push((0.0, 0.0, std::f32::consts::FRAC_PI_2));
        layers.push((0.0, depth, std::f32::consts::FRAC_PI_2));
        bands.push(Band::Side);
    }
    // V: distance along the side, bottom to top.
    let mut vs = vec![0.0f32];
    for k in 1..layers.len() {
        let (a, b) = (layers[k - 1], layers[k]);
        vs.push(vs[k - 1] + Vec2::new(b.0 - a.0, b.1 - a.1).length());
    }
    let layer_pts: Vec<Vec<Vec<Vec2>>> = layers
        .iter()
        .map(|&(inset, _, _)| {
            rings
                .iter()
                .zip(&offs)
                .map(|(r, o)| {
                    r.iter()
                        .zip(o)
                        .map(|(q, off)| *q + off.unwrap_or(Vec2::ZERO) * inset)
                        .collect()
                })
                .collect()
        })
        .collect();
    let pos = |q: Vec2, h: f32| frame.point(q) + axis * h;
    let smooth = if params.smooth_angle.is_finite() {
        params.smooth_angle
    } else {
        0.0
    };
    let side_normal = |n: Vec2| -> Vec3 {
        let t = frame.u * -n.y + frame.v * n.x;
        t.cross(axis).normalize_or_zero()
    };

    // Sides and bevels.
    for (ri, ring) in rings.iter().enumerate() {
        let cols = ring_columns(ring, true, smooth, &[]);
        for (k, band) in bands.iter().enumerate() {
            let (lo, hi) = (k, k + 1);
            if (layers[hi].1 - layers[lo].1).abs() < 1e-7
                && (layers[hi].0 - layers[lo].0).abs() < 1e-7
            {
                continue;
            }
            let slot = if *band == Band::Side {
                params.side_slot
            } else {
                params.bevel_slot
            };
            let row = |mb: &mut MeshBuilder, l: usize| -> Vec<u32> {
                let (_, h, phi) = layers[l];
                cols.columns
                    .iter()
                    .map(|c| {
                        let q = layer_pts[l][ri][c.point];
                        let sn = side_normal(c.normal);
                        let normal = match band {
                            Band::Side => sn,
                            _ if segs == 1 => Vec3::ZERO, // chamfer: flat, computed
                            Band::BevelBottom => {
                                (sn * pmath::sin(phi) - axis * pmath::cos(phi)).normalize_or_zero()
                            }
                            Band::BevelTop => {
                                (sn * pmath::sin(phi) + axis * pmath::cos(phi)).normalize_or_zero()
                            }
                        };
                        let uv = Vec2::new(c.along, vs[l]) * uv_scale;
                        mb.vertex(slot, Vertex::new(pos(q, h), normal, uv))
                    })
                    .collect()
            };
            let (a, b) = (row(mb, lo), row(mb, hi));
            for &[ca, cb] in &cols.edges {
                mb.try_triangle_indices(slot, [a[ca], a[cb], b[cb]]);
                mb.try_triangle_indices(slot, [a[ca], b[cb], b[ca]]);
            }
        }
    }

    // Caps from the innermost layers (bottom and top).
    for (top, enabled) in [(false, params.cap_start), (true, params.cap_end)] {
        if !enabled {
            continue;
        }
        let l = if top { layers.len() - 1 } else { 0 };
        let lp = &layer_pts[l];
        let cap = Polygon {
            outer: lp[0].clone(),
            holes: lp[1..].to_vec(),
        };
        let tri = poly::triangulate(&cap);
        let h = layers[l].1;
        let n = if top { axis } else { -axis };
        let ids: Vec<u32> = tri
            .points
            .iter()
            .map(|q| {
                let uv = if top { *q } else { Vec2::new(-q.x, q.y) } * uv_scale;
                mb.vertex(params.cap_slot, Vertex::new(pos(*q, h), n, uv))
            })
            .collect();
        for t in tri.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| ids[i as usize]);
            if top {
                mb.triangle_indices(params.cap_slot, [a, b, c]);
            } else {
                mb.triangle_indices(params.cap_slot, [a, c, b]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_builder::{edge_report, inward_triangles, signed_volume};

    #[test]
    fn box_with_hole() {
        let p = Polygon::rect(Vec2::splat(-1.0), Vec2::splat(1.0))
            .with_hole(Polygon::rect(Vec2::splat(-0.5), Vec2::splat(0.5)).outer);
        let b = extrude(
            &p,
            &ExtrudeParams {
                depth: 2.0,
                ..Default::default()
            },
        );
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        let m = &b.mesh;
        assert!(edge_report(m, 1e-5).is_closed_manifold());
        assert!((signed_volume(m) - 3.0 * 2.0).abs() < 1e-4);
        assert_eq!(inward_triangles(m), 0);
        assert_eq!(m.groups.len(), 2);
        let bb = m.bounds();
        assert!((bb.max - Vec3::new(1.0, 2.0, 1.0)).length() < 1e-6);
    }

    #[test]
    fn chamfer_and_round_bevel_stay_closed() {
        let p = Polygon::rect(Vec2::splat(-1.0), Vec2::splat(1.0));
        for segs in [1, 4] {
            let b = extrude(
                &p,
                &ExtrudeParams {
                    depth: 1.0,
                    bevel_width: 0.2,
                    bevel_segments: segs,
                    ..Default::default()
                },
            );
            assert!(b.warnings.is_empty(), "{:?}", b.warnings);
            assert!(edge_report(&b.mesh, 1e-5).is_closed_manifold());
            assert_eq!(inward_triangles(&b.mesh), 0);
            let v = signed_volume(&b.mesh);
            assert!(v < 4.0 && v > 3.6, "{v}");
        }
    }
}
