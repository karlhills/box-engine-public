//! [`MeshBuilder`]: accumulates triangles, quads, polygons and strips into a
//! [`MeshData`] with one material group per slot.
//!
//! Conventions:
//! - **Front faces are counter-clockwise** seen from the side the normal
//!   points to (the engine's winding).
//! - **Material slot = group index.** Geometry added to slot `s` lands in the
//!   [`MaterialGroup`] with `material_slot == s`; `finish` emits groups in slot
//!   order and skips empty slots.
//! - **Normals:** a [`Vertex`] with an explicit normal keeps it. A vertex
//!   whose normal is `Vec3::ZERO` gets the area-weighted average of the
//!   triangles that use it, so vertices *shared* between triangles shade
//!   smoothly and unshared ones shade flat. Hard edges come from separate
//!   vertices (the `flat_*` helpers never share); smooth sections come from
//!   [`MeshBuilder::strip`] / [`MeshBuilder::grid`], which share vertices.
//! - **Robustness:** nothing here panics on bad input. Triangles with
//!   non-finite positions, out-of-range indices or (near) zero area are
//!   dropped and counted; non-finite normals, UVs and colours are replaced.
//!   `finish` always returns a mesh that passes [`MeshData::validate`]
//!   (possibly empty) and the warnings say what was dropped.
//!
//! Units are metres; UV helpers return metres times a scale (world-scale
//! texturing, docs/PROCEDURAL.md section 7).

use box_core::pmath;
use std::collections::HashMap;

use box_core::mesh::{MaterialGroup, MeshData};
use glam::{Affine3A, Vec2, Vec3};

use crate::poly::{self, Polygon};

/// Highest material slot index + 1. Larger slots are clamped (with a warning)
/// so a bad slot id cannot allocate unbounded groups.
pub const MAX_SLOTS: u32 = 64;

/// One vertex. `normal == Vec3::ZERO` means "compute it" (see module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub normal: Vec3,
    /// UV sets; only the first `uv_sets` of the builder are emitted.
    pub uv: [Vec2; MeshData::MAX_UV_SETS],
    /// Linear RGBA. Emitted only when the builder has colours enabled.
    pub color: [f32; 4],
}

impl Default for Vertex {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            normal: Vec3::ZERO,
            uv: [Vec2::ZERO; MeshData::MAX_UV_SETS],
            color: [1.0; 4],
        }
    }
}

impl Vertex {
    pub fn new(position: Vec3, normal: Vec3, uv: Vec2) -> Self {
        let mut v = Self {
            position,
            normal,
            ..Default::default()
        };
        v.uv[0] = uv;
        v
    }

    /// A vertex whose normal is computed by `finish`.
    pub fn at(position: Vec3, uv: Vec2) -> Self {
        Self::new(position, Vec3::ZERO, uv)
    }

    /// Sets UV set `set` (0..4; out-of-range sets are ignored).
    pub fn with_uv(mut self, set: usize, uv: Vec2) -> Self {
        if let Some(slot) = self.uv.get_mut(set) {
            *slot = uv;
        }
        self
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }
}

/// A plane with a 2D coordinate system: `point(p) = origin + u * p.x + v * p.y`.
/// The plane's front normal is `u × v`, so a counter-clockwise 2D polygon
/// mapped through the frame faces `normal()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneFrame {
    pub origin: Vec3,
    pub u: Vec3,
    pub v: Vec3,
}

impl PlaneFrame {
    /// The XY plane facing +Z: `(x, y) → (x, y, 0)`.
    pub const XY: Self = Self {
        origin: Vec3::ZERO,
        u: Vec3::X,
        v: Vec3::Y,
    };
    /// The ground plane facing +Y, seen from above with −Z (forward) up:
    /// `(x, y) → (x, 0, −y)`. Plan polygons (footprints) use this frame.
    pub const PLAN: Self = Self {
        origin: Vec3::ZERO,
        u: Vec3::X,
        v: Vec3::NEG_Z,
    };

    pub fn new(origin: Vec3, u: Vec3, v: Vec3) -> Self {
        Self { origin, u, v }
    }

    /// Unit front normal (`u × v`), or +Y when the axes are degenerate.
    pub fn normal(&self) -> Vec3 {
        self.u.cross(self.v).try_normalize().unwrap_or(Vec3::Y)
    }

    pub fn point(&self, p: Vec2) -> Vec3 {
        self.origin + self.u * p.x + self.v * p.y
    }

    /// The same frame moved by `d`.
    pub fn translated(self, d: Vec3) -> Self {
        Self {
            origin: self.origin + d,
            ..self
        }
    }

    /// True when origin and axes are finite and the axes span a plane.
    pub fn is_valid(&self) -> bool {
        self.origin.is_finite()
            && self.u.is_finite()
            && self.v.is_finite()
            && self.u.cross(self.v).length_squared() > 1e-12
    }
}

/// UV helpers. All return metres times `scale`; V grows downward on
/// vertical surfaces (the glTF convention the engine uses).
pub mod uv {
    use glam::{Vec2, Vec3};

    use super::PlaneFrame;

    /// Planar projection onto a frame's axes.
    pub fn planar(p: Vec3, frame: &PlaneFrame, scale: f32) -> Vec2 {
        let d = p - frame.origin;
        let (lu, lv) = (frame.u.length_squared(), frame.v.length_squared());
        let x = if lu > 0.0 { d.dot(frame.u) / lu } else { 0.0 };
        let y = if lv > 0.0 { d.dot(frame.v) / lv } else { 0.0 };
        Vec2::new(x, y) * scale
    }

    /// Box (triplanar-style) projection: picks the plane of the normal's
    /// dominant axis, oriented so no face is mirrored.
    pub fn box_project(p: Vec3, normal: Vec3, scale: f32) -> Vec2 {
        let a = normal.abs();
        let uv = if a.x >= a.y && a.x >= a.z {
            Vec2::new(-p.z * normal.x.signum(), -p.y)
        } else if a.y >= a.z {
            Vec2::new(p.x, p.z * normal.y.signum())
        } else {
            Vec2::new(p.x * normal.z.signum(), -p.y)
        };
        uv * scale
    }
}

/// A finished mesh plus the warnings produced while building it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuiltMesh {
    pub mesh: MeshData,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct Group {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
}

/// Accumulates geometry into per-slot groups. See the module docs.
#[derive(Clone, Debug)]
pub struct MeshBuilder {
    groups: Vec<Group>,
    uv_sets: usize,
    colors: bool,
    tangents: bool,
    weld: Option<f32>,
    color: [f32; 4],
    warnings: Vec<String>,
    dropped_degenerate: usize,
    dropped_invalid: usize,
    clamped_slots: bool,
}

impl Default for MeshBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Twice the triangle area below which (relative to its longest edge squared)
/// a triangle counts as degenerate.
const DEGENERATE_RATIO: f32 = 1e-9;

fn degenerate(a: Vec3, b: Vec3, c: Vec3) -> bool {
    let e = (b - a)
        .length_squared()
        .max((c - b).length_squared())
        .max((a - c).length_squared());
    if e.is_nan() || e < 1e-20 {
        return true;
    }
    let cross = (b - a).cross(c - a).length();
    let ok = cross.is_finite() && cross > DEGENERATE_RATIO * e;
    !ok
}

fn finite_or(v: Vec2, d: Vec2) -> Vec2 {
    if v.is_finite() { v } else { d }
}

fn clean_vertex(mut v: Vertex) -> Vertex {
    if !v.normal.is_finite() {
        v.normal = Vec3::ZERO;
    }
    for uv in &mut v.uv {
        *uv = finite_or(*uv, Vec2::ZERO);
    }
    for c in &mut v.color {
        if !c.is_finite() {
            *c = 1.0;
        }
    }
    v
}

impl MeshBuilder {
    /// One UV set, no colours, no tangents, no welding.
    pub fn new() -> Self {
        Self {
            groups: Vec::new(),
            uv_sets: 1,
            colors: false,
            tangents: false,
            weld: None,
            color: [1.0; 4],
            warnings: Vec::new(),
            dropped_degenerate: 0,
            dropped_invalid: 0,
            clamped_slots: false,
        }
    }

    /// Emit `n` UV sets (clamped to 1..=4).
    pub fn with_uv_sets(mut self, n: usize) -> Self {
        self.uv_sets = n.clamp(1, MeshData::MAX_UV_SETS);
        self
    }

    /// Emit vertex colours.
    pub fn with_colors(mut self) -> Self {
        self.colors = true;
        self
    }

    /// Generate tangents from UV set 0 in `finish`
    /// ([`MeshData::generate_tangents`]).
    pub fn with_tangents(mut self) -> Self {
        self.tangents = true;
        self
    }

    /// Merge vertices of the same group whose position, normal, UVs and
    /// colour agree within `tolerance` (metres; other channels use the same
    /// number). Welding runs before missing normals are computed, so welded
    /// vertices with a zero normal shade smoothly across the weld.
    pub fn with_weld(mut self, tolerance: f32) -> Self {
        self.weld = (tolerance.is_finite() && tolerance > 0.0).then_some(tolerance);
        self
    }

    /// The colour the convenience helpers (`flat_*`, `fill_polygon`) give
    /// their vertices. Setting it enables colours.
    pub fn set_color(&mut self, color: [f32; 4]) {
        self.color = color;
        self.colors = true;
    }

    pub fn color(&self) -> [f32; 4] {
        self.color
    }

    /// Record a warning (shown on the owner in the editor).
    pub fn warn(&mut self, message: impl Into<String>) {
        let m = message.into();
        if !self.warnings.contains(&m) {
            self.warnings.push(m);
        }
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn vertex_count(&self) -> usize {
        self.groups.iter().map(|g| g.vertices.len()).sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.groups.iter().map(|g| g.indices.len() / 3).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.triangle_count() == 0
    }

    fn group(&mut self, slot: u32) -> &mut Group {
        let slot = if slot >= MAX_SLOTS {
            self.clamped_slots = true;
            MAX_SLOTS - 1
        } else {
            slot
        } as usize;
        if self.groups.len() <= slot {
            self.groups.resize_with(slot + 1, Group::default);
        }
        &mut self.groups[slot]
    }

    /// Adds a vertex to `slot` and returns its index within that slot, for
    /// [`triangle_indices`](Self::triangle_indices).
    pub fn vertex(&mut self, slot: u32, v: Vertex) -> u32 {
        let g = self.group(slot);
        g.vertices.push(clean_vertex(v));
        (g.vertices.len() - 1) as u32
    }

    /// A triangle from vertices already in `slot`, counter-clockwise.
    /// Returns false (and counts it) when it was dropped.
    pub fn triangle_indices(&mut self, slot: u32, tri: [u32; 3]) -> bool {
        let g = self.group(slot);
        let n = g.vertices.len() as u32;
        if tri.iter().any(|&i| i >= n) {
            self.dropped_invalid += 1;
            return false;
        }
        let [a, b, c] = tri.map(|i| g.vertices[i as usize].position);
        if !(a.is_finite() && b.is_finite() && c.is_finite()) {
            self.dropped_invalid += 1;
            return false;
        }
        if degenerate(a, b, c) {
            self.dropped_degenerate += 1;
            return false;
        }
        g.indices.extend_from_slice(&tri);
        true
    }

    /// Like [`triangle_indices`](Self::triangle_indices), but a degenerate
    /// triangle is skipped silently: for generators that knowingly produce
    /// collapsed quads (lathe poles, bevel bridges). Invalid indices and
    /// non-finite positions are still counted.
    pub fn try_triangle_indices(&mut self, slot: u32, tri: [u32; 3]) -> bool {
        let before = self.dropped_degenerate;
        let ok = self.triangle_indices(slot, tri);
        self.dropped_degenerate = before;
        ok
    }

    /// A triangle with its own three vertices, counter-clockwise.
    pub fn triangle(&mut self, slot: u32, v: [Vertex; 3]) {
        let [a, b, c] = v.map(|v| v.position);
        if !(a.is_finite() && b.is_finite() && c.is_finite()) {
            self.dropped_invalid += 1;
            return;
        }
        if degenerate(a, b, c) {
            self.dropped_degenerate += 1;
            return;
        }
        let i = v.map(|v| self.vertex(slot, v));
        self.group(slot).indices.extend_from_slice(&i);
    }

    /// A quad `a b c d` (counter-clockwise) with four shared vertices,
    /// split along `a–c`.
    pub fn quad(&mut self, slot: u32, v: [Vertex; 4]) {
        if v.iter().any(|v| !v.position.is_finite()) {
            self.dropped_invalid += 2;
            return;
        }
        let [p0, p1, p2, p3] = v.map(|v| v.position);
        let (t0, t1) = (!degenerate(p0, p1, p2), !degenerate(p0, p2, p3));
        self.dropped_degenerate += usize::from(!t0) + usize::from(!t1);
        if !t0 && !t1 {
            return;
        }
        let i = v.map(|v| self.vertex(slot, v));
        let g = self.group(slot);
        if t0 {
            g.indices.extend_from_slice(&[i[0], i[1], i[2]]);
        }
        if t1 {
            g.indices.extend_from_slice(&[i[0], i[2], i[3]]);
        }
    }

    /// A flat-shaded triangle (geometric normal, current colour).
    pub fn flat_triangle(&mut self, slot: u32, p: [Vec3; 3], uv: [Vec2; 3]) {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
        let c = self.color;
        self.triangle(
            slot,
            [0, 1, 2].map(|i| Vertex::new(p[i], n, uv[i]).with_color(c)),
        );
    }

    /// A flat-shaded quad (normal from the diagonals, current colour).
    pub fn flat_quad(&mut self, slot: u32, p: [Vec3; 4], uv: [Vec2; 4]) {
        let n = (p[2] - p[0]).cross(p[3] - p[1]).normalize_or_zero();
        let c = self.color;
        self.quad(
            slot,
            [0, 1, 2, 3].map(|i| Vertex::new(p[i], n, uv[i]).with_color(c)),
        );
    }

    /// A flat polygon given by its 3D outline (counter-clockwise seen from
    /// the front, roughly planar). Triangulated with earcut on its best-fit
    /// plane (Newell normal), so concave outlines work. UVs are a planar
    /// projection in metres times `uv_scale`.
    pub fn flat_polygon(&mut self, slot: u32, points: &[Vec3], uv_scale: f32) {
        let pts: Vec<Vec3> = points.iter().copied().filter(|p| p.is_finite()).collect();
        if pts.len() < points.len() {
            self.dropped_invalid += 1;
        }
        if pts.len() < 3 {
            return;
        }
        let mut n = Vec3::ZERO;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            n += Vec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }
        let Some(n) = n.try_normalize() else {
            self.dropped_degenerate += 1;
            return;
        };
        let u = n.any_orthonormal_vector();
        let frame = PlaneFrame::new(pts[0], u, n.cross(u));
        let ring: Vec<Vec2> = pts.iter().map(|p| uv::planar(*p, &frame, 1.0)).collect();
        // Keep the 3D points: map triangulated 2D points back by index.
        let tri = poly::triangulate(&Polygon::new(ring.clone()));
        let c = self.color;
        for t in tri.indices.as_chunks::<3>().0 {
            let q = [0, 1, 2].map(|k| tri.points[t[k] as usize]);
            let p3 = q.map(|q| {
                ring.iter()
                    .position(|r| *r == q)
                    .map(|i| pts[i])
                    .unwrap_or_else(|| frame.point(q))
            });
            self.triangle(
                slot,
                [0, 1, 2].map(|k| Vertex::new(p3[k], n, q[k] * uv_scale).with_color(c)),
            );
        }
    }

    /// Fills a 2D polygon (holes allowed) mapped through `frame`, flat
    /// shaded. It faces `frame.normal()`, or the opposite side when `flip`.
    /// UVs are the 2D coordinates times `uv_scale` (mirrored in U when
    /// flipped, so the texture reads correctly from the back).
    pub fn fill_polygon(
        &mut self,
        slot: u32,
        polygon: &Polygon,
        frame: &PlaneFrame,
        flip: bool,
        uv_scale: f32,
    ) {
        if !frame.is_valid() {
            self.warn("fill_polygon: degenerate plane frame, nothing drawn");
            return;
        }
        let tri = poly::triangulate(polygon);
        let n = if flip {
            -frame.normal()
        } else {
            frame.normal()
        };
        let c = self.color;
        let base = tri
            .points
            .iter()
            .map(|p| {
                let uv = if flip { Vec2::new(-p.x, p.y) } else { *p } * uv_scale;
                self.vertex(slot, Vertex::new(frame.point(*p), n, uv).with_color(c))
            })
            .collect::<Vec<_>>();
        for t in tri.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| base[i as usize]);
            if flip {
                self.triangle_indices(slot, [a, c, b]);
            } else {
                self.triangle_indices(slot, [a, b, c]);
            }
        }
    }

    /// Connects two rows of equal length with quads that share the row
    /// vertices: quad `i` is `a[i], a[i+1], b[i+1], b[i]` (counter-clockwise
    /// from the front). Rows of different length are truncated to the
    /// shorter one (with a warning).
    pub fn strip(&mut self, slot: u32, a: &[Vertex], b: &[Vertex]) {
        self.grid(slot, &[a, b]);
    }

    /// Like [`strip`](Self::strip) for several rows: row `r` is joined to row
    /// `r + 1`, and inner rows are shared by both strips (smooth along the
    /// rows' direction when normals are computed).
    pub fn grid(&mut self, slot: u32, rows: &[&[Vertex]]) {
        let Some(len) = rows.iter().map(|r| r.len()).min() else {
            return;
        };
        if rows.iter().any(|r| r.len() != len) {
            self.warn("strip rows have different lengths; truncated");
        }
        if rows.len() < 2 || len < 2 {
            return;
        }
        let ids: Vec<Vec<u32>> = rows
            .iter()
            .map(|r| r[..len].iter().map(|v| self.vertex(slot, *v)).collect())
            .collect();
        for w in ids.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            for i in 0..len - 1 {
                self.triangle_indices(slot, [a[i], a[i + 1], b[i + 1]]);
                self.triangle_indices(slot, [a[i], b[i + 1], b[i]]);
            }
        }
    }

    /// Appends a whole mesh (all its groups keep their slots), transformed.
    /// Missing channels are filled with defaults.
    pub fn append(&mut self, mesh: &MeshData, transform: Affine3A) {
        if mesh.validate().is_err() {
            self.warn("append: invalid mesh skipped");
            return;
        }
        let groups = if mesh.groups.is_empty() {
            vec![MaterialGroup {
                first_index: 0,
                index_count: mesh.indices.len() as u32,
                material_slot: 0,
            }]
        } else {
            mesh.groups.clone()
        };
        let normal_m = transform.matrix3.inverse().transpose();
        for g in groups {
            let start = g.first_index as usize;
            let end = (start + g.index_count as usize).min(mesh.indices.len());
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut tri = Vec::with_capacity(3);
            for &i in &mesh.indices[start.min(end)..end] {
                let ni = match remap.get(&i) {
                    Some(&ni) => ni,
                    None => {
                        let k = i as usize;
                        let mut v = Vertex::new(
                            transform.transform_point3(mesh.positions[k]),
                            (normal_m * mesh.normals[k]).normalize_or_zero(),
                            Vec2::ZERO,
                        );
                        for (s, set) in mesh.uvs.iter().enumerate().take(MeshData::MAX_UV_SETS) {
                            v.uv[s] = set[k];
                        }
                        if let Some(c) = mesh.colors.get(k) {
                            v.color = *c;
                        }
                        let ni = self.vertex(g.material_slot, v);
                        remap.insert(i, ni);
                        ni
                    }
                };
                tri.push(ni);
                if tri.len() == 3 {
                    self.triangle_indices(g.material_slot, [tri[0], tri[1], tri[2]]);
                    tri.clear();
                }
            }
            if !mesh.colors.is_empty() {
                self.colors = true;
            }
            self.uv_sets = self.uv_sets.max(mesh.uvs.len().min(MeshData::MAX_UV_SETS));
        }
    }

    /// Builds the mesh, discarding warnings. See [`build`](Self::build).
    pub fn finish(self) -> MeshData {
        self.build().mesh
    }

    /// Welds (if enabled), computes missing normals, concatenates groups in
    /// slot order, generates tangents (if enabled) and validates. Always
    /// returns a valid mesh; an empty one when nothing survived.
    pub fn build(mut self) -> BuiltMesh {
        if self.dropped_degenerate > 0 {
            let m = format!("dropped {} degenerate triangle(s)", self.dropped_degenerate);
            self.warn(m);
        }
        if self.dropped_invalid > 0 {
            let m = format!(
                "dropped {} triangle(s) with non-finite or out-of-range vertices",
                self.dropped_invalid
            );
            self.warn(m);
        }
        if self.clamped_slots {
            let m = format!("material slots above {} were clamped", MAX_SLOTS - 1);
            self.warn(m);
        }
        let weld = self.weld;
        let mut mesh = MeshData::default();
        let mut uvs: Vec<Vec<Vec2>> = vec![Vec::new(); self.uv_sets];
        for (slot, mut g) in self.groups.into_iter().enumerate() {
            if let Some(tol) = weld {
                weld_group(&mut g, tol);
            }
            if g.indices.is_empty() {
                continue;
            }
            // Keep only referenced vertices, in first-use order.
            let mut remap = vec![u32::MAX; g.vertices.len()];
            let mut verts: Vec<Vertex> = Vec::new();
            for i in &mut g.indices {
                let r = &mut remap[*i as usize];
                if *r == u32::MAX {
                    *r = verts.len() as u32;
                    verts.push(g.vertices[*i as usize]);
                }
                *i = *r;
            }
            compute_missing_normals(&mut verts, &g.indices);
            let base = mesh.positions.len() as u32;
            let first = mesh.indices.len() as u32;
            for v in &verts {
                mesh.positions.push(v.position);
                mesh.normals.push(v.normal);
                for (s, set) in uvs.iter_mut().enumerate() {
                    set.push(v.uv[s]);
                }
                if self.colors {
                    mesh.colors.push(v.color);
                }
            }
            mesh.indices.extend(g.indices.iter().map(|i| i + base));
            mesh.groups.push(MaterialGroup {
                first_index: first,
                index_count: g.indices.len() as u32,
                material_slot: slot as u32,
            });
        }
        mesh.uvs = uvs;
        if self.tangents {
            mesh.generate_tangents();
        }
        if let Err(e) = mesh.validate() {
            self.warnings
                .push(format!("internal: built mesh invalid ({e}); emptied"));
            mesh = MeshData::default();
        }
        BuiltMesh {
            mesh,
            warnings: self.warnings,
        }
    }
}

fn quantize(x: f32, tol: f32) -> i64 {
    (x / tol).round() as i64
}

fn weld_group(g: &mut Group, tol: f32) {
    let mut map: HashMap<Vec<i64>, u32> = HashMap::new();
    let mut remap = Vec::with_capacity(g.vertices.len());
    let mut out: Vec<Vertex> = Vec::new();
    for v in &g.vertices {
        let mut key = Vec::with_capacity(18);
        key.extend(v.position.to_array().map(|x| quantize(x, tol)));
        key.extend(v.normal.to_array().map(|x| quantize(x, tol)));
        for uv in &v.uv {
            key.extend(uv.to_array().map(|x| quantize(x, tol)));
        }
        key.extend(v.color.map(|x| quantize(x, tol)));
        let id = *map.entry(key).or_insert_with(|| {
            out.push(*v);
            (out.len() - 1) as u32
        });
        remap.push(id);
    }
    let mut idx = Vec::with_capacity(g.indices.len());
    for t in g.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| remap[i as usize]);
        if a != b && b != c && a != c {
            idx.extend_from_slice(&[a, b, c]);
        }
    }
    g.vertices = out;
    g.indices = idx;
}

fn compute_missing_normals(verts: &mut [Vertex], indices: &[u32]) {
    if verts.iter().all(|v| v.normal != Vec3::ZERO) {
        for v in verts.iter_mut() {
            v.normal = v.normal.normalize_or(Vec3::Y);
        }
        return;
    }
    let mut acc = vec![Vec3::ZERO; verts.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
        let n =
            (verts[b].position - verts[a].position).cross(verts[c].position - verts[a].position);
        for i in [a, b, c] {
            acc[i] += n;
        }
    }
    for (v, n) in verts.iter_mut().zip(acc) {
        v.normal = if v.normal == Vec3::ZERO {
            n.normalize_or(Vec3::Y)
        } else {
            v.normal.normalize_or(Vec3::Y)
        };
    }
}

// ------------------------------------------------------------ ring columns

/// One column of vertices when a 2D outline (a profile or a ring) is
/// extruded, swept or revolved: the outline point it comes from, its 2D
/// shading normal, its length along the outline (for V or U) and its slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Column {
    pub point: usize,
    pub normal: Vec2,
    pub along: f32,
    pub slot: u32,
}

/// Columns of an outline plus, per edge, its start and end column.
#[derive(Clone, Debug, Default)]
pub(crate) struct Columns {
    pub columns: Vec<Column>,
    pub edges: Vec<[usize; 2]>,
    pub slots: Vec<u32>,
}

/// Splits an outline into columns. Edge normals point to the right of the
/// edge direction (outward for counter-clockwise rings). A vertex whose
/// edges turn by at most `smooth_angle` radians and share a slot gets one
/// column with the averaged normal (smooth); otherwise one column per edge
/// (hard). A closed outline always splits at point 0 so the length
/// parameter can run from 0 to the perimeter. Points must have no
/// consecutive duplicates.
pub(crate) fn ring_columns(
    points: &[Vec2],
    closed: bool,
    smooth_angle: f32,
    edge_slots: &[u32],
) -> Columns {
    let n = points.len();
    let m = if closed { n } else { n.saturating_sub(1) };
    let mut out = Columns::default();
    if n < 2 || m == 0 {
        return out;
    }
    let slot = |e: usize| edge_slots.get(e).copied().unwrap_or(0);
    let dirs: Vec<Vec2> = (0..m)
        .map(|e| (points[(e + 1) % n] - points[e]).normalize_or_zero())
        .collect();
    let nrm: Vec<Vec2> = dirs.iter().map(|d| Vec2::new(d.y, -d.x)).collect();
    let mut along = vec![0.0f32; n + 1];
    for e in 0..m {
        along[e + 1] = along[e] + (points[(e + 1) % n] - points[e]).length();
    }
    let cos_limit = pmath::cos(smooth_angle.clamp(0.0, std::f32::consts::PI));
    // Vertex v joins edge v-1 (incoming) and edge v (outgoing).
    let incoming = |v: usize| -> Option<usize> {
        if v > 0 {
            Some(v - 1)
        } else if closed {
            Some(m - 1)
        } else {
            None
        }
    };
    let outgoing = |v: usize| -> Option<usize> { (v < m).then_some(v) };
    let smooth = |v: usize| -> bool {
        match (incoming(v), outgoing(v % n)) {
            (Some(a), Some(b)) => slot(a) == slot(b) && dirs[a].dot(dirs[b]) >= cos_limit - 1e-6,
            _ => false,
        }
    };
    let avg = |v: usize, fallback: Vec2| -> Vec2 {
        match (incoming(v), outgoing(v % n)) {
            (Some(a), Some(b)) => (nrm[a] + nrm[b]).try_normalize().unwrap_or(fallback),
            _ => fallback,
        }
    };
    let mut prev_end: Option<usize> = None;
    for e in 0..m {
        let (a, b) = (e, (e + 1) % n);
        let start = match prev_end {
            Some(c) => c,
            None => {
                out.columns.push(Column {
                    point: a,
                    normal: if smooth(a) { avg(a, nrm[e]) } else { nrm[e] },
                    along: along[e],
                    slot: slot(e),
                });
                out.columns.len() - 1
            }
        };
        let seam = closed && e == m - 1;
        let sb = smooth(b);
        out.columns.push(Column {
            point: b,
            normal: if sb { avg(b, nrm[e]) } else { nrm[e] },
            along: along[e + 1],
            slot: slot(e),
        });
        let end = out.columns.len() - 1;
        prev_end = (sb && !seam).then_some(end);
        out.edges.push([start, end]);
        out.slots.push(slot(e));
    }
    out
}

// ----------------------------------------------------------------- metrics

/// Topology of a mesh after merging positions within a tolerance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EdgeReport {
    /// Undirected edges used by exactly one triangle.
    pub boundary: usize,
    /// Undirected edges used by three or more triangles.
    pub non_manifold: usize,
    /// Edges used twice in the same direction (inconsistent winding).
    pub inconsistent: usize,
}

impl EdgeReport {
    /// Every edge shared by exactly two consistently wound triangles.
    pub fn is_closed_manifold(&self) -> bool {
        self.boundary == 0 && self.non_manifold == 0 && self.inconsistent == 0
    }
}

/// Edge statistics over all groups, with positions merged within
/// `tolerance` (so hard-edge vertex splits do not count as boundaries).
pub fn edge_report(mesh: &MeshData, tolerance: f32) -> EdgeReport {
    let tol = if tolerance > 0.0 { tolerance } else { 1e-5 };
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let pid: Vec<u32> = mesh
        .positions
        .iter()
        .map(|p| {
            let k = p.to_array().map(|x| quantize(x, tol));
            let n = ids.len() as u32;
            *ids.entry(k).or_insert(n)
        })
        .collect();
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    for t in mesh.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| pid[i as usize]);
        if a == b || b == c || a == c {
            continue;
        }
        for e in [(a, b), (b, c), (c, a)] {
            *directed.entry(e).or_insert(0) += 1;
        }
    }
    let mut r = EdgeReport::default();
    for (&(a, b), &n) in &directed {
        if n > 1 {
            r.inconsistent += 1;
        }
        let back = directed.get(&(b, a)).copied().unwrap_or(0);
        if a < b || back == 0 {
            match n + back {
                1 => r.boundary += 1,
                2 => {}
                _ => r.non_manifold += 1,
            }
        }
    }
    r
}

/// Sum of triangle areas.
pub fn surface_area(mesh: &MeshData) -> f32 {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize]);
            0.5 * (b - a).cross(c - a).length()
        })
        .sum()
}

/// Signed enclosed volume (positive for closed, outward-wound meshes).
pub fn signed_volume(mesh: &MeshData) -> f32 {
    let v: f64 = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.positions[i as usize].as_dvec3());
            a.dot(b.cross(c))
        })
        .sum();
    (v / 6.0) as f32
}

/// Triangles whose winding disagrees with their vertex normals.
pub fn inward_triangles(mesh: &MeshData) -> usize {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize);
            let g = (mesh.positions[b] - mesh.positions[a])
                .cross(mesh.positions[c] - mesh.positions[a]);
            let n = mesh.normals[a] + mesh.normals[b] + mesh.normals[c];
            g.dot(n) < 0.0
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_columns_split_hard_corners() {
        // A square: 4 hard corners plus the seam -> 8 columns.
        let sq = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
        let c = ring_columns(&sq, true, 0.3, &[]);
        assert_eq!((c.columns.len(), c.edges.len()), (8, 4));
        assert_eq!(c.columns[c.edges[0][0]].normal, Vec2::new(0.0, -1.0));
        assert!((c.columns[c.edges[3][1]].along - 4.0).abs() < 1e-6);
        // A 16-gon: smooth everywhere except the seam -> 17 columns.
        let circle: Vec<Vec2> = (0..16)
            .map(|i| Vec2::from_angle(std::f32::consts::TAU * i as f32 / 16.0))
            .collect();
        let c = ring_columns(&circle, true, 0.5, &[]);
        assert_eq!(c.columns.len(), 17);
        for col in &c.columns {
            assert!(col.normal.dot(circle[col.point]) > 0.99);
        }
        // Different slots force a split; open outlines have no seam.
        let c = ring_columns(&circle[..5], false, 0.5, &[0, 1, 1, 1]);
        assert_eq!(c.columns.len(), 6);
    }

    #[test]
    fn groups_follow_slots_and_skip_empty() {
        let mut mb = MeshBuilder::new();
        mb.flat_quad(
            2,
            [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y],
            [Vec2::ZERO; 4],
        );
        mb.flat_triangle(0, [Vec3::ZERO, Vec3::X, Vec3::Y], [Vec2::ZERO; 3]);
        let m = mb.finish();
        m.validate().unwrap();
        assert_eq!(m.groups.len(), 2);
        assert_eq!(m.groups[0].material_slot, 0);
        assert_eq!(m.groups[0].index_count, 3);
        assert_eq!(m.groups[1].material_slot, 2);
        assert_eq!(m.groups[1].index_count, 6);
        assert!(m.normals.iter().all(|n| (*n - Vec3::Z).length() < 1e-6));
        assert_eq!(inward_triangles(&m), 0);
    }

    #[test]
    fn bad_input_never_panics() {
        let mut mb = MeshBuilder::new().with_tangents().with_weld(1e-4);
        let nan = Vec3::splat(f32::NAN);
        mb.flat_triangle(0, [nan, Vec3::X, Vec3::Y], [Vec2::ZERO; 3]);
        mb.flat_triangle(0, [Vec3::ZERO, Vec3::X, Vec3::X * 2.0], [Vec2::ZERO; 3]);
        mb.triangle_indices(0, [0, 1, 99]);
        mb.flat_polygon(1, &[Vec3::ZERO, Vec3::INFINITY], 1.0);
        mb.fill_polygon(0, &Polygon::new(vec![]), &PlaneFrame::XY, false, 1.0);
        mb.vertex(70, Vertex::at(Vec3::ZERO, Vec2::ZERO));
        let b = mb.build();
        b.mesh.validate().unwrap();
        assert_eq!(b.mesh.triangle_count(), 0);
        assert!(b.warnings.iter().any(|w| w.contains("degenerate")));
        assert!(b.warnings.iter().any(|w| w.contains("non-finite")));
    }

    #[test]
    fn strip_shares_vertices_and_smooths() {
        // A half cylinder from strips: shared vertices, computed normals point outward.
        let ring = |y: f32| -> Vec<Vertex> {
            (0..=8)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 8.0;
                    Vertex::at(
                        Vec3::new(pmath::cos(a), y, -pmath::sin(a)),
                        Vec2::new(i as f32, y),
                    )
                })
                .collect()
        };
        let (lo, hi) = (ring(0.0), ring(1.0));
        let mut mb = MeshBuilder::new();
        mb.strip(0, &lo, &hi);
        let m = mb.finish();
        assert_eq!(m.vertex_count(), 18);
        assert_eq!(m.triangle_count(), 16);
        assert_eq!(inward_triangles(&m), 0);
        for (p, n) in m.positions.iter().zip(&m.normals) {
            let radial = Vec3::new(p.x, 0.0, p.z).normalize();
            assert!(n.dot(radial) > 0.95, "{p} {n}");
        }
    }

    #[test]
    fn weld_merges_and_metrics_detect_closed_cube() {
        let mut mb = MeshBuilder::new().with_weld(1e-5);
        mb.append(&MeshData::cuboid(Vec3::ONE), Affine3A::IDENTITY);
        let m = mb.finish();
        assert_eq!(m.vertex_count(), 24);
        assert!(edge_report(&m, 1e-5).is_closed_manifold());
        assert!((signed_volume(&m) - 1.0).abs() < 1e-5);
        assert!((surface_area(&m) - 6.0).abs() < 1e-5);
        // An open box (top removed) has 4 boundary edges.
        let mut open = MeshData::cuboid(Vec3::ONE);
        open.indices.drain(12..18);
        let r = edge_report(&open, 1e-5);
        assert_eq!(r.boundary, 4);
        assert!(!r.is_closed_manifold());
    }

    #[test]
    fn fill_polygon_with_hole_faces_frame_normal() {
        let p = Polygon::rect(Vec2::ZERO, Vec2::splat(2.0))
            .with_hole(Polygon::rect(Vec2::splat(0.5), Vec2::splat(1.5)).outer);
        for flip in [false, true] {
            let mut mb = MeshBuilder::new();
            mb.fill_polygon(0, &p, &PlaneFrame::PLAN, flip, 1.0);
            let m = mb.finish();
            assert!((surface_area(&m) - 3.0).abs() < 1e-5);
            assert_eq!(inward_triangles(&m), 0);
            let want = if flip { -1.0 } else { 1.0 };
            assert!(m.normals.iter().all(|n| (n.y - want).abs() < 1e-6));
        }
    }
}
