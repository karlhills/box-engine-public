//! [`ArchMesh`]: the triangle sink every box_arch generator writes into.
//!
//! A thin layer over `box_proc::mesh_builder::MeshBuilder` that adds what
//! architectural pieces need and the general builder does not do:
//!
//! - **Winding by intent.** Every triangle and quad is given the direction
//!   it should face; the winding is fixed to match, so generators place
//!   surfaces in (U, v, w) without tracking orientation through mirrored
//!   mappings.
//! - **Analytic ambient occlusion** in the vertex colour (grey, multiplied
//!   into the base colour by the renderer): callers pass an occlusion
//!   factor per vertex (1 = open, 0 = fully occluded) and the mesh applies
//!   the generator's `ao` strength, so strength 0 gives white vertices.
//! - **Two UV sets.** Set 0 is in metres (arc length along walls, a box
//!   projection elsewhere). Set 1 carries weathering data: on walls
//!   (height above the wall foot, depth below the wall top) in metres;
//!   elsewhere (height above the entity origin, 0).
//! - **Exactly zero-area triangles are skipped silently** (collinear points
//!   from slicing at a kink); anything else goes to the builder, which
//!   drops and reports degenerate triangles, so a property test can assert
//!   that a generator never produces any.

use box_core::mesh::MeshData;
use box_proc::mesh_builder::{MeshBuilder, Vertex, uv};
use glam::{Vec2, Vec3};

/// One vertex as the helpers take it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct V {
    pub p: Vec3,
    pub n: Vec3,
    pub ao: f32,
    pub uv: Vec2,
    pub uv1: Vec2,
}

pub(crate) struct ArchMesh {
    mb: MeshBuilder,
    strength: f32,
}

/// Zero-area test: collinear or repeated points (relative to the longest
/// edge, the builder's own criterion: thin slivers from slicing are kept
/// so edges stay shared).
fn zero_area(a: Vec3, b: Vec3, c: Vec3) -> bool {
    let e = (b - a)
        .length_squared()
        .max((c - b).length_squared())
        .max((a - c).length_squared());
    (b - a).cross(c - a).length() <= 1e-9 * e
}

impl ArchMesh {
    /// A mesh whose occlusion factors are applied with `strength` (0..=1;
    /// 0 disables ambient occlusion).
    pub fn new(strength: f32) -> Self {
        let strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            1.0
        };
        Self {
            mb: MeshBuilder::new().with_uv_sets(2).with_colors(),
            strength,
        }
    }

    fn color(&self, ao: f32) -> [f32; 4] {
        let ao = if ao.is_finite() {
            ao.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let g = 1.0 - self.strength * (1.0 - ao);
        [g, g, g, 1.0]
    }

    fn vertex(&self, v: V) -> Vertex {
        Vertex::new(v.p, v.n, v.uv)
            .with_uv(1, v.uv1)
            .with_color(self.color(v.ao))
    }

    /// A triangle with explicit vertices, wound to face along `facing`.
    pub fn tri_v(&mut self, slot: u32, v: [V; 3], facing: Vec3) {
        let [a, b, c] = v.map(|v| v.p);
        if zero_area(a, b, c) {
            return;
        }
        let flip = (b - a).cross(c - a).dot(facing) < 0.0;
        let order = if flip { [0, 2, 1] } else { [0, 1, 2] };
        let verts = order.map(|i| self.vertex(v[i]));
        self.mb.triangle(slot, verts);
    }

    /// A triangle with per-vertex normals and occlusion; UV set 0 is a box
    /// projection in metres, set 1 (height, 0).
    pub fn tri_n(&mut self, slot: u32, p: [Vec3; 3], n: [Vec3; 3], ao: [f32; 3], facing: Vec3) {
        let v = [0, 1, 2].map(|i| V {
            p: p[i],
            n: n[i],
            ao: ao[i],
            uv: uv::box_project(p[i], facing, 1.0),
            uv1: Vec2::new(p[i].y, 0.0),
        });
        self.tri_v(slot, v, facing);
    }

    /// A flat triangle facing `facing` (its geometric normal, oriented).
    pub fn tri(&mut self, slot: u32, p: [Vec3; 3], ao: [f32; 3], facing: Vec3) {
        let mut n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        if n.dot(facing) < 0.0 {
            n = -n;
        }
        self.tri_n(slot, p, [n; 3], ao, n);
    }

    /// A flat convex quad `a b c d` (in order around its rim).
    pub fn quad(&mut self, slot: u32, p: [Vec3; 4], ao: [f32; 4], facing: Vec3) {
        let n = (p[2] - p[0]).cross(p[3] - p[1]).normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let n = if n.dot(facing) < 0.0 { -n } else { n };
        self.tri_n(slot, [p[0], p[1], p[2]], [n; 3], [ao[0], ao[1], ao[2]], n);
        self.tri_n(slot, [p[0], p[2], p[3]], [n; 3], [ao[0], ao[2], ao[3]], n);
    }

    /// A flat convex polygon as a fan.
    pub fn poly(&mut self, slot: u32, p: &[Vec3], ao: &[f32], facing: Vec3) {
        for i in 1..p.len().saturating_sub(1) {
            self.tri(
                slot,
                [p[0], p[i], p[i + 1]],
                [ao[0], ao[i], ao[i + 1]],
                facing,
            );
        }
    }

    /// A box with its axis from `a` to `b`, `width` across and `height`
    /// along `up` (glazing bars, stringers, finials).
    pub fn beam(
        &mut self,
        slot: u32,
        a: Vec3,
        b: Vec3,
        width: f32,
        height: f32,
        up: Vec3,
        ao: f32,
    ) {
        let ax = (b - a).normalize_or_zero();
        if ax == Vec3::ZERO {
            return;
        }
        let side = ax.cross(up).normalize_or_zero() * (width * 0.5);
        if side == Vec3::ZERO {
            return;
        }
        let upv = side.cross(ax).normalize_or_zero() * height;
        let upv = if upv.dot(up) < 0.0 { -upv } else { upv };
        let c = |p: Vec3, sx: f32, uy: f32| p + side * sx + upv * uy;
        let (a0, a1, a2, a3) = (
            c(a, -1.0, 0.0),
            c(a, 1.0, 0.0),
            c(a, 1.0, 1.0),
            c(a, -1.0, 1.0),
        );
        let (b0, b1, b2, b3) = (
            c(b, -1.0, 0.0),
            c(b, 1.0, 0.0),
            c(b, 1.0, 1.0),
            c(b, -1.0, 1.0),
        );
        let q = [ao; 4];
        self.quad(slot, [a3, a2, b2, b3], q, upv);
        self.quad(slot, [a0, a1, b1, b0], q, -upv);
        self.quad(slot, [a1, b1, b2, a2], q, side);
        self.quad(slot, [a0, b0, b3, a3], q, -side);
        self.quad(slot, [a0, a1, a2, a3], q, -ax);
        self.quad(slot, [b0, b1, b2, b3], q, ax);
    }

    /// A board along the edge `p0`–`p1`, standing `thick` out along the
    /// horizontal `out`, from `below` under the edge to `above` over it:
    /// fascia and barge boards.
    pub fn plank(
        &mut self,
        slot: u32,
        p0: Vec3,
        p1: Vec3,
        out: Vec3,
        thick: f32,
        below: f32,
        above: f32,
        ao: f32,
    ) {
        let o = out.normalize_or_zero() * thick;
        if o == Vec3::ZERO {
            return;
        }
        let (dn, up) = (Vec3::Y * -below, Vec3::Y * above);
        let c = |p: Vec3, oo: bool, top: bool| {
            p + if oo { o } else { Vec3::ZERO } + if top { up } else { dn }
        };
        let ax = p1 - p0;
        let q = [ao, ao, 1.0, 1.0];
        let topn = ax.cross(o).normalize_or_zero();
        let topn = if topn.y < 0.0 { -topn } else { topn };
        let faces: [([Vec3; 4], [f32; 4], Vec3); 6] = [
            (
                [
                    c(p0, true, false),
                    c(p1, true, false),
                    c(p1, true, true),
                    c(p0, true, true),
                ],
                q,
                o,
            ),
            (
                [
                    c(p0, false, false),
                    c(p1, false, false),
                    c(p1, false, true),
                    c(p0, false, true),
                ],
                q,
                -o,
            ),
            (
                [
                    c(p0, false, true),
                    c(p1, false, true),
                    c(p1, true, true),
                    c(p0, true, true),
                ],
                [1.0; 4],
                topn,
            ),
            (
                [
                    c(p0, false, false),
                    c(p1, false, false),
                    c(p1, true, false),
                    c(p0, true, false),
                ],
                [ao; 4],
                -topn,
            ),
            (
                [
                    c(p0, false, false),
                    c(p0, true, false),
                    c(p0, true, true),
                    c(p0, false, true),
                ],
                q,
                -ax,
            ),
            (
                [
                    c(p1, false, false),
                    c(p1, true, false),
                    c(p1, true, true),
                    c(p1, false, true),
                ],
                q,
                ax,
            ),
        ];
        for (p, a, f) in faces {
            self.quad(slot, p, a, f);
        }
    }

    /// A low triangular roll along `a`–`b` (ridge and hip tiles): feet sunk
    /// `sink` into the slopes, apex `height` up.
    pub fn tent(
        &mut self,
        slot: u32,
        a: Vec3,
        b: Vec3,
        width: f32,
        height: f32,
        sink: f32,
        ao: f32,
    ) {
        let ax = (b - a).normalize_or_zero();
        let side = ax.cross(Vec3::Y).normalize_or_zero() * (width * 0.5);
        if ax == Vec3::ZERO || side == Vec3::ZERO {
            return;
        }
        let upv = side.cross(ax).normalize_or_zero();
        let upv = if upv.y < 0.0 { -upv } else { upv };
        let l = |p: Vec3| p + side - upv * sink;
        let r = |p: Vec3| p - side - upv * sink;
        let t = |p: Vec3| p + upv * height;
        self.quad(
            slot,
            [l(a), l(b), t(b), t(a)],
            [ao, ao, 1.0, 1.0],
            side + upv * width,
        );
        self.quad(
            slot,
            [r(a), r(b), t(b), t(a)],
            [ao, ao, 1.0, 1.0],
            -side + upv * width,
        );
        self.tri(slot, [l(a), r(a), t(a)], [ao, ao, 1.0], -ax);
        self.tri(slot, [l(b), r(b), t(b)], [ao, ao, 1.0], ax);
    }

    /// Adds a warning (deduplicated by the builder).
    pub fn warn(&mut self, message: impl Into<String>) {
        self.mb.warn(message);
    }

    /// The finished mesh and every warning (the builder's included).
    pub fn build(self) -> (MeshData, Vec<String>) {
        let built = self.mb.build();
        (built.mesh, built.warnings)
    }
}
