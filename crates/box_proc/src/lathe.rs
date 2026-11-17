//! Lathe: revolve a 2D profile around the Y axis.
//!
//! The profile is given in (radius, height) and uses the [`Profile`] type of
//! sweeps: open or closed, per-edge material slots, a smoothing angle. The
//! surface faces the right of the profile's direction of travel, so an open
//! profile drawn bottom to top at positive radius faces outward, and closed
//! profiles are made counter-clockwise (outward) automatically.
//!
//! Angles run counter-clockwise seen from above, starting at +X:
//! `(r, y, θ) → (r cos θ, y, −r sin θ)`, like the built-in cylinder.
//!
//! - **Poles:** profile points on the axis (radius ≤ 1e-6) become one
//!   triangle per segment (no degenerate triangles are emitted); an edge
//!   lying on the axis produces nothing.
//! - **Smooth vs hard:** along the profile by `profile.smooth_angle`; around
//!   the axis, segments are smooth unless one segment spans more than
//!   `profile.smooth_angle` (so a 6-sided pillar is faceted).
//! - **Partial sweeps** get planar caps at both ends when `caps` is set: the
//!   profile itself if closed, or the region between an open profile and
//!   the axis.
//!
//! UVs: U = angle / 2π (0..1 for a full turn), V = length along the
//! profile (metres). Caps use (radius, height).

use box_core::pmath;
use std::f32::consts::TAU;

use glam::{Vec2, Vec3};

use crate::mesh_builder::{BuiltMesh, MeshBuilder, Vertex, ring_columns};
use crate::poly::{self, Polygon};
use crate::sweep::Profile;

/// Lathe parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct LatheParams {
    /// Points are (radius, height). Negative radii are clamped to 0.
    pub profile: Profile,
    /// Segments around the axis (clamped to 3..=1024 for a full turn,
    /// 1..=1024 for a partial one).
    pub segments: u32,
    /// Radians, from +X counter-clockwise seen from above.
    pub start_angle: f32,
    /// Radians; 2π (or more) is a full revolution. Negative sweeps the
    /// other way.
    pub sweep_angle: f32,
    /// Cap the two ends of a partial sweep.
    pub caps: bool,
    pub cap_slot: u32,
}

impl Default for LatheParams {
    fn default() -> Self {
        Self {
            profile: Profile::new(vec![Vec2::new(0.5, 0.0), Vec2::new(0.5, 1.0)], false),
            segments: 32,
            start_angle: 0.0,
            sweep_angle: TAU,
            caps: true,
            cap_slot: 1,
        }
    }
}

/// Revolves `params.profile` into a new mesh.
pub fn lathe(params: &LatheParams) -> BuiltMesh {
    let mut mb = MeshBuilder::new();
    lathe_into(&mut mb, params);
    mb.build()
}

const POLE: f32 = 1e-6;

fn at(p: Vec2, theta: f32) -> Vec3 {
    if p.x == 0.0 {
        return Vec3::new(0.0, p.y, 0.0);
    }
    Vec3::new(p.x * pmath::cos(theta), p.y, -p.x * pmath::sin(theta))
}

/// Revolves into an existing builder (warnings go to the builder).
pub fn lathe_into(mb: &mut MeshBuilder, params: &LatheParams) {
    let mut prof = params.profile.clone();
    let mut clamped = false;
    for p in &mut prof.points {
        if p.x.abs() <= POLE {
            p.x = 0.0;
        } else if p.x < 0.0 {
            clamped = true;
            p.x = 0.0;
        }
    }
    if clamped {
        mb.warn("lathe: negative radii clamped to the axis");
    }
    let (pts, slots, closed) = prof.cleaned();
    if pts.len() < 2 {
        mb.warn("lathe: profile needs at least two distinct points");
        return;
    }
    let (mut start, mut sweep) = (params.start_angle, params.sweep_angle);
    if !start.is_finite() {
        start = 0.0;
    }
    if !sweep.is_finite() {
        mb.warn("lathe: non-finite sweep angle; full revolution");
        sweep = TAU;
    }
    if sweep < 0.0 {
        start += sweep;
        sweep = -sweep;
    }
    if sweep < 1e-5 {
        mb.warn("lathe: zero sweep angle");
        return;
    }
    let full = sweep >= TAU - 1e-5;
    if full {
        sweep = TAU;
    }
    let segs = params.segments.clamp(if full { 3 } else { 1 }, 1024) as usize;
    let smooth = if prof.smooth_angle.is_finite() {
        prof.smooth_angle.max(0.0)
    } else {
        0.0
    };
    let hard_around = sweep / segs as f32 > smooth;
    let cols = ring_columns(&pts, closed, smooth, &slots);
    let theta = |j: usize| start + sweep * j as f32 / segs as f32;
    // A full turn reuses angle 0 for the last row so the seam is exact.
    let pos_theta = |j: usize| if full && j == segs { start } else { theta(j) };
    let row = |mb: &mut MeshBuilder, j: usize, computed: bool| -> Vec<u32> {
        let (t, tp) = (theta(j), pos_theta(j));
        cols.columns
            .iter()
            .map(|c| {
                let n = if computed {
                    Vec3::ZERO
                } else {
                    Vec3::new(
                        c.normal.x * pmath::cos(tp),
                        c.normal.y,
                        -c.normal.x * pmath::sin(tp),
                    )
                    .normalize_or_zero()
                };
                let uv = Vec2::new((t - start) / TAU, c.along);
                mb.vertex(c.slot, Vertex::new(at(pts[c.point], tp), n, uv))
            })
            .collect()
    };
    let connect = |mb: &mut MeshBuilder, a: &[u32], b: &[u32]| {
        for (e, &[ca, cb]) in cols.edges.iter().enumerate() {
            let s = cols.slots[e];
            // Pole quads collapse to one triangle; axis edges vanish.
            mb.try_triangle_indices(s, [b[ca], b[cb], a[cb]]);
            mb.try_triangle_indices(s, [b[ca], a[cb], a[ca]]);
        }
    };
    if hard_around {
        for j in 0..segs {
            let (a, b) = (row(mb, j, true), row(mb, j + 1, true));
            connect(mb, &a, &b);
        }
    } else {
        let mut a = row(mb, 0, false);
        for j in 0..segs {
            let b = row(mb, j + 1, false);
            connect(mb, &a, &b);
            a = b;
        }
    }
    if full || !params.caps {
        return;
    }
    // Partial caps.
    let mut outline = pts.clone();
    if !closed {
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        if last.x > 0.0 {
            outline.push(Vec2::new(0.0, last.y));
        }
        if first.x > 0.0 {
            outline.push(Vec2::new(0.0, first.y));
        }
    }
    let tri = poly::triangulate(&Polygon::new(outline));
    if tri.indices.is_empty() {
        return;
    }
    for (angle, sign) in [(start, -1.0f32), (start + sweep, 1.0)] {
        // Facing along ±dθ (the direction of increasing angle).
        let facing = Vec3::new(-pmath::sin(angle), 0.0, -pmath::cos(angle)) * sign;
        let ids: Vec<u32> = tri
            .points
            .iter()
            .map(|q| {
                let uv = Vec2::new(q.x * -sign, q.y);
                mb.vertex(params.cap_slot, Vertex::new(at(*q, angle), facing, uv))
            })
            .collect();
        for t in tri.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
            let (pa, pb, pc) = (
                at(tri.points[a], angle),
                at(tri.points[b], angle),
                at(tri.points[c], angle),
            );
            let flip = (pb - pa).cross(pc - pa).dot(facing) < 0.0;
            let tri3 = if flip {
                [ids[a], ids[c], ids[b]]
            } else {
                [ids[a], ids[b], ids[c]]
            };
            mb.triangle_indices(params.cap_slot, tri3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_builder::{edge_report, inward_triangles, signed_volume};

    #[test]
    fn closed_ring_profile_volume_matches_pappus() {
        // A square ring profile (torus with square section): V = N sin(2π/N) A r̄.
        let prof = Profile::new(
            vec![
                Vec2::new(1.0, 0.0),
                Vec2::new(2.0, 0.0),
                Vec2::new(2.0, 1.0),
                Vec2::new(1.0, 1.0),
            ],
            true,
        );
        let n = 24;
        let b = lathe(&LatheParams {
            profile: prof,
            segments: n,
            ..Default::default()
        });
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        let m = &b.mesh;
        assert!(edge_report(m, 1e-5).is_closed_manifold());
        assert_eq!(inward_triangles(m), 0);
        let want = n as f32 * pmath::sin(TAU / n as f32) * 1.0 * 1.5;
        assert!(
            (signed_volume(m) - want).abs() < 1e-3,
            "{} {want}",
            signed_volume(m)
        );
    }

    #[test]
    fn poles_have_no_degenerate_triangles() {
        // A cylinder closed at both poles through its profile.
        let prof = Profile::new(
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 2.0),
                Vec2::new(0.0, 2.0),
            ],
            false,
        );
        let b = lathe(&LatheParams {
            profile: prof,
            segments: 16,
            ..Default::default()
        });
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        let m = &b.mesh;
        // 16 pole triangles at each end plus 32 side triangles.
        assert_eq!(m.triangle_count(), 64);
        assert!(edge_report(m, 1e-5).is_closed_manifold());
        assert_eq!(inward_triangles(m), 0);
    }
}
