//! 2D polygon kernel: polygons with holes, booleans and offsets (i_overlay),
//! triangulation (earcut), and small helpers (area, orientation,
//! point-in-polygon, simplify, snapping, corner chamfers and fillets).
//!
//! # Conventions and robustness
//!
//! - A [`Polygon`] is one outer ring plus holes. Rings are implicit loops
//!   (the first point is not repeated). **Normalized** polygons have a
//!   counter-clockwise outer ring and clockwise holes (positive area in a
//!   y-up 2D system).
//! - Every function accepts any input without panicking. Non-finite points
//!   are dropped, consecutive duplicate points are merged, rings with fewer
//!   than three distinct points or zero area are dropped, and orientation is
//!   fixed, before anything else happens ([`Polygon::normalized`]).
//! - **Booleans, offsets and [`make_valid`] use the non-zero fill rule** after
//!   normalization, so self-intersecting rings fill every lobe, overlapping
//!   polygons in one input are unioned, and a hole cuts whatever it covers.
//!   Their output is always valid: simple, non-overlapping, normalized.
//! - i_overlay computes on an integer grid (about 2^-30 of the input extent),
//!   so its output is not bit-exact. Boolean output is **snapped back to input
//!   vertices** within a small relative tolerance ([`snap_tolerance`]), so
//!   edges shared with other geometry stay bit-identical (spike S6, rule J6)
//!   and no slivers appear.
//! - [`triangulate`] expects a valid polygon; run [`make_valid`] first if the
//!   input may self-intersect. On invalid input earcut still returns
//!   triangles without panicking, but they may not cover the region exactly.
//! - Computation is in f64; the public types are f32 (`glam::Vec2`).

use box_core::pmath;
use std::collections::HashMap;

use glam::{DVec2, Vec2};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

/// The fill rule every boolean, offset and `make_valid` uses.
const FILL: FillRule = FillRule::NonZero;

/// Points with a coordinate beyond ±1e9 m are treated like non-finite ones
/// (dropped), so areas and the integer overlay grid stay representable.
pub const MAX_COORD: f32 = 1e9;

fn usable(p: Vec2) -> bool {
    p.is_finite() && p.abs().max_element() <= MAX_COORD
}

/// A polygon with holes. See the module docs for conventions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polygon {
    pub outer: Vec<Vec2>,
    pub holes: Vec<Vec<Vec2>>,
}

/// Several disjoint polygons (the output of booleans and offsets).
pub type MultiPolygon = Vec<Polygon>;

/// Boolean operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    /// `a` minus `b`.
    Difference,
    Intersection,
    Xor,
}

/// Corner style for [`offset`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Join {
    /// Sharp corners, bevelled where the mitre would exceed `limit` times the
    /// offset distance (the SVG `stroke-miterlimit` convention; clamped to
    /// at least 1.0).
    Miter { limit: f32 },
    /// Arcs whose chords deviate from the true arc by at most `tolerance`
    /// metres.
    Round { tolerance: f32 },
    /// Corners cut flat.
    Bevel,
}

/// Triangles over a polygon: `indices` (counter-clockwise triples) into
/// `points`, which are the polygon's own (normalized) vertices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Triangulation {
    pub points: Vec<Vec2>,
    pub indices: Vec<u32>,
}

impl Triangulation {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Total triangle area.
    pub fn area(&self) -> f32 {
        self.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.points[i as usize].as_dvec2());
                0.5 * (b - a).perp_dot(c - a)
            })
            .sum::<f64>() as f32
    }
}

// ------------------------------------------------------------------ rings

/// Signed area of a ring (shoelace): positive when counter-clockwise.
pub fn signed_area(ring: &[Vec2]) -> f32 {
    signed_area_d(ring) as f32
}

fn signed_area_d(ring: &[Vec2]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0f64;
    for i in 0..n {
        let (p, q) = (ring[i].as_dvec2(), ring[(i + 1) % n].as_dvec2());
        a += p.x * q.y - q.x * p.y;
    }
    if a.is_finite() { a * 0.5 } else { 0.0 }
}

/// True when the ring is counter-clockwise (positive signed area).
pub fn is_ccw(ring: &[Vec2]) -> bool {
    signed_area_d(ring) > 0.0
}

/// Winding-number point-in-ring test (non-zero rule). Points exactly on an
/// edge may report either result.
pub fn ring_contains(ring: &[Vec2], p: Vec2) -> bool {
    let n = ring.len();
    if n < 3 || !p.is_finite() {
        return false;
    }
    let p = p.as_dvec2();
    let mut wn = 0i32;
    for i in 0..n {
        let (a, b) = (ring[i].as_dvec2(), ring[(i + 1) % n].as_dvec2());
        let side = (b - a).perp_dot(p - a);
        if a.y <= p.y {
            if b.y > p.y && side > 0.0 {
                wn += 1;
            }
        } else if b.y <= p.y && side < 0.0 {
            wn -= 1;
        }
    }
    wn != 0
}

/// Drops non-finite points (and points beyond [`MAX_COORD`]) and merges
/// consecutive points closer than
/// `tolerance` (including the closing pair).
pub fn sanitize_ring(ring: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    let tol2 = (tolerance.max(0.0) as f64).powi(2);
    let mut out: Vec<Vec2> = Vec::with_capacity(ring.len());
    for &p in ring {
        if !usable(p) {
            continue;
        }
        if let Some(&last) = out.last()
            && last.as_dvec2().distance_squared(p.as_dvec2()) <= tol2
        {
            continue;
        }
        out.push(p);
    }
    while out.len() > 1
        && out[0]
            .as_dvec2()
            .distance_squared(out[out.len() - 1].as_dvec2())
            <= tol2
    {
        out.pop();
    }
    out
}

/// Sanitizes the ring, then removes vertices that deviate less than
/// `tolerance` from the line through their neighbours (collinear and
/// near-collinear points), repeatedly. The result has at least three points
/// or is empty.
pub fn simplify_ring(ring: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    let tol = tolerance.max(0.0) as f64;
    let mut r = sanitize_ring(ring, tolerance);
    loop {
        let n = r.len();
        if n < 3 || signed_area_d(&r) == 0.0 {
            return Vec::new();
        }
        let mut out: Vec<Vec2> = Vec::with_capacity(n);
        let mut removed = 0;
        for i in 0..n {
            // Measured against the last kept vertex, so a run of collinear
            // points collapses to its ends.
            let prev = out.last().copied().unwrap_or(r[n - 1]);
            let (a, p, b) = (prev.as_dvec2(), r[i].as_dvec2(), r[(i + 1) % n].as_dvec2());
            let ab = b - a;
            let len = ab.length();
            let dev = if len > 0.0 {
                ab.perp_dot(p - a).abs() / len
            } else {
                (p - a).length()
            };
            if dev <= tol && n - removed > 3 {
                removed += 1;
            } else {
                out.push(r[i]);
            }
        }
        if removed == 0 {
            return out;
        }
        r = out;
    }
}

/// A tolerance relative to the extent of the given points: used for
/// snapping boolean output back to input vertices.
pub fn snap_tolerance(points: impl IntoIterator<Item = Vec2>) -> f32 {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    let mut any = false;
    for p in points {
        if usable(p) {
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
    }
    if !any {
        return 1e-6;
    }
    let extent = (hi - lo).max_element();
    (extent * 1e-6).max(1e-6)
}

// --------------------------------------------------------------- polygons

impl Polygon {
    pub fn new(outer: Vec<Vec2>) -> Self {
        Self {
            outer,
            holes: Vec::new(),
        }
    }

    pub fn with_hole(mut self, hole: Vec<Vec2>) -> Self {
        self.holes.push(hole);
        self
    }

    /// Axis-aligned rectangle between two corners (any order).
    pub fn rect(a: Vec2, b: Vec2) -> Self {
        let (lo, hi) = (a.min(b), a.max(b));
        Self::new(vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)])
    }

    /// Regular polygon approximating a circle (`segments` clamped to 3..=4096).
    pub fn circle(centre: Vec2, radius: f32, segments: u32) -> Self {
        let n = segments.clamp(3, 4096);
        Self::new(
            (0..n)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / n as f32;
                    centre + Vec2::new(pmath::cos(a), pmath::sin(a)) * radius
                })
                .collect(),
        )
    }

    /// All rings: outer first, then holes.
    pub fn rings(&self) -> impl Iterator<Item = &Vec<Vec2>> {
        std::iter::once(&self.outer).chain(self.holes.iter())
    }

    pub fn vertex_count(&self) -> usize {
        self.rings().map(|r| r.len()).sum()
    }

    /// True when the normalized polygon has no area.
    pub fn is_empty(&self) -> bool {
        self.normalized().outer.is_empty()
    }

    /// Area of the normalized polygon: outer minus holes (holes are assumed
    /// inside the outer ring and disjoint; use [`make_valid`] otherwise).
    pub fn area(&self) -> f32 {
        let n = self.normalized();
        n.rings().map(|r| signed_area_d(r)).sum::<f64>() as f32
    }

    /// Non-zero point-in-polygon test (inside the outer ring, outside every hole).
    pub fn contains(&self, p: Vec2) -> bool {
        ring_contains(&self.outer, p) && !self.holes.iter().any(|h| ring_contains(h, p))
    }

    /// Bounding box `(min, max)` of the finite points, or `None`.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut it = self.rings().flatten().copied().filter(|p| usable(*p));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Sanitized copy: non-finite and duplicate points removed, degenerate
    /// rings dropped (an empty outer ring drops the holes too), outer ring
    /// counter-clockwise, holes clockwise. Does not resolve
    /// self-intersections (see [`make_valid`]).
    pub fn normalized(&self) -> Polygon {
        let tol = snap_tolerance(self.rings().flatten().copied()) * 0.01;
        let fix = |ring: &[Vec2], ccw: bool| -> Option<Vec<Vec2>> {
            let mut r = sanitize_ring(ring, tol);
            if r.len() < 3 {
                return None;
            }
            let a = signed_area_d(&r);
            if a == 0.0 {
                return None;
            }
            if (a > 0.0) != ccw {
                r.reverse();
            }
            Some(r)
        };
        let Some(outer) = fix(&self.outer, true) else {
            return Polygon::default();
        };
        Polygon {
            outer,
            holes: self.holes.iter().filter_map(|h| fix(h, false)).collect(),
        }
    }

    /// The polygon with every point moved by `d`.
    pub fn translated(&self, d: Vec2) -> Polygon {
        Polygon {
            outer: self.outer.iter().map(|p| *p + d).collect(),
            holes: self
                .holes
                .iter()
                .map(|h| h.iter().map(|p| *p + d).collect())
                .collect(),
        }
    }
}

/// Total area of several polygons (assumed disjoint, as boolean output is).
pub fn total_area(polys: &[Polygon]) -> f32 {
    polys.iter().map(|p| p.area() as f64).sum::<f64>() as f32
}

type Shape = Vec<Vec<[f64; 2]>>;

/// Rings for i_overlay: sanitized and oriented (outer counter-clockwise,
/// holes clockwise), but zero-area rings are kept so self-intersecting
/// rings such as a bow tie fill every lobe under the non-zero rule.
fn to_shapes(polys: &[Polygon]) -> Vec<Shape> {
    let conv = |r: &[Vec2], ccw: bool| -> Option<Vec<[f64; 2]>> {
        let mut r = sanitize_ring(r, 0.0);
        if r.len() < 3 {
            return None;
        }
        let a = signed_area_d(&r);
        if a != 0.0 && (a > 0.0) != ccw {
            r.reverse();
        }
        Some(r.iter().map(|q| [q.x as f64, q.y as f64]).collect())
    };
    polys
        .iter()
        .filter_map(|p| {
            let mut shape = vec![conv(&p.outer, true)?];
            shape.extend(p.holes.iter().filter_map(|h| conv(h, false)));
            Some(shape)
        })
        .collect()
}

fn from_shapes(shapes: Vec<Shape>, snap_to: &[Vec2], tol: f32) -> MultiPolygon {
    let snapper = Snapper::new(snap_to, tol);
    let conv = |c: &Vec<[f64; 2]>| -> Vec<Vec2> {
        c.iter()
            .map(|p| snapper.snap(Vec2::new(p[0] as f32, p[1] as f32)))
            .collect()
    };
    shapes
        .iter()
        .filter_map(|s| {
            let mut rings = s.iter();
            let outer = conv(rings.next()?);
            let p = Polygon {
                outer,
                holes: rings.map(conv).collect(),
            }
            .normalized();
            (!p.outer.is_empty()).then_some(p)
        })
        .collect()
}

/// Spatial hash of reference points for snapping.
struct Snapper {
    cell: f32,
    grid: HashMap<(i64, i64), Vec<Vec2>>,
}

impl Snapper {
    fn new(points: &[Vec2], tol: f32) -> Self {
        let cell = tol.max(1e-12);
        let mut grid: HashMap<(i64, i64), Vec<Vec2>> = HashMap::new();
        for &p in points.iter().filter(|p| p.is_finite()) {
            grid.entry(Self::key(p, cell)).or_default().push(p);
        }
        Self { cell, grid }
    }

    fn key(p: Vec2, cell: f32) -> (i64, i64) {
        ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64)
    }

    /// The nearest reference point within the tolerance, else `p`.
    fn snap(&self, p: Vec2) -> Vec2 {
        let (kx, ky) = Self::key(p, self.cell);
        let mut best = (p, self.cell * self.cell);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(pts) = self.grid.get(&(kx + dx, ky + dy)) {
                    for &q in pts {
                        let d = q.distance_squared(p);
                        if d <= best.1 {
                            best = (q, d);
                        }
                    }
                }
            }
        }
        best.0
    }
}

/// Moves every point of `polys` that lies within `tolerance` of a
/// `reference` point onto it, then re-normalizes (dropping rings that
/// collapse).
pub fn snap_to_vertices(polys: &mut MultiPolygon, reference: &[Vec2], tolerance: f32) {
    let s = Snapper::new(reference, tolerance);
    for p in polys.iter_mut() {
        for r in std::iter::once(&mut p.outer).chain(p.holes.iter_mut()) {
            for q in r.iter_mut() {
                *q = s.snap(*q);
            }
        }
        *p = p.normalized();
    }
    polys.retain(|p| !p.outer.is_empty());
}

fn all_points(a: &[Polygon], b: &[Polygon]) -> Vec<Vec2> {
    a.iter()
        .chain(b)
        .flat_map(|p| p.rings().flatten().copied())
        .filter(|p| usable(*p))
        .collect()
}

/// Resolves self-intersections, overlaps and holes outside their outer ring
/// (non-zero rule): the output is valid and normalized.
pub fn make_valid(polys: &[Polygon]) -> MultiPolygon {
    let shapes = to_shapes(polys);
    if shapes.is_empty() {
        return Vec::new();
    }
    let out = shapes.simplify_shape(FILL);
    let pts = all_points(polys, &[]);
    let tol = snap_tolerance(pts.iter().copied());
    from_shapes(out, &pts, tol)
}

/// Boolean operation between two polygon sets. Output vertices that lie
/// within [`snap_tolerance`] of an input vertex are snapped onto it.
pub fn boolean(a: &[Polygon], b: &[Polygon], op: BoolOp) -> MultiPolygon {
    let (sa, sb) = (to_shapes(a), to_shapes(b));
    match (sa.is_empty(), sb.is_empty(), op) {
        (true, true, _) => return Vec::new(),
        (true, false, BoolOp::Union | BoolOp::Xor) => return make_valid(b),
        (true, false, _) => return Vec::new(),
        (false, true, BoolOp::Intersection) => return Vec::new(),
        (false, true, _) => return make_valid(a),
        _ => {}
    }
    let rule = match op {
        BoolOp::Union => OverlayRule::Union,
        BoolOp::Difference => OverlayRule::Difference,
        BoolOp::Intersection => OverlayRule::Intersect,
        BoolOp::Xor => OverlayRule::Xor,
    };
    let out = sa.overlay(&sb, rule, FILL);
    let pts = all_points(a, b);
    let tol = snap_tolerance(pts.iter().copied());
    from_shapes(out, &pts, tol)
}

pub fn union(a: &[Polygon], b: &[Polygon]) -> MultiPolygon {
    boolean(a, b, BoolOp::Union)
}

pub fn difference(a: &[Polygon], b: &[Polygon]) -> MultiPolygon {
    boolean(a, b, BoolOp::Difference)
}

pub fn intersection(a: &[Polygon], b: &[Polygon]) -> MultiPolygon {
    boolean(a, b, BoolOp::Intersection)
}

pub fn xor(a: &[Polygon], b: &[Polygon]) -> MultiPolygon {
    boolean(a, b, BoolOp::Xor)
}

/// Grows (`distance > 0`) or shrinks (`distance < 0`) polygons by
/// `distance` metres. Holes shrink when the polygon grows and vice versa;
/// parts thinner than `2 * -distance` vanish when shrinking. A zero or
/// non-finite distance returns [`make_valid`] of the input.
pub fn offset(polys: &[Polygon], distance: f32, join: Join) -> MultiPolygon {
    if !distance.is_finite() || distance == 0.0 {
        return make_valid(polys);
    }
    let shapes = to_shapes(polys);
    if shapes.is_empty() {
        return Vec::new();
    }
    let distance = distance.clamp(-MAX_COORD, MAX_COORD);
    let r = distance.abs() as f64;
    let line_join = match join {
        Join::Miter { limit } => {
            let limit = if limit.is_finite() {
                (limit as f64).max(1.0001)
            } else {
                4.0
            };
            LineJoin::Miter(2.0 * (1.0 / limit).asin())
        }
        Join::Round { tolerance } => {
            let tol = if tolerance.is_finite() && tolerance > 0.0 {
                tolerance as f64
            } else {
                r * 0.01
            };
            let step = 2.0 * pmath::f64::acos(1.0 - (tol / r).min(1.0));
            LineJoin::Round(step.clamp(0.01 * std::f64::consts::PI, 0.25 * std::f64::consts::PI))
        }
        Join::Bevel => LineJoin::Bevel,
    };
    let style = OutlineStyle::new(distance as f64).line_join(line_join);
    let out = shapes.outline(&style);
    // Offset output has new vertices; snap only to exact-ish input matches.
    let pts = all_points(polys, &[]);
    let tol = snap_tolerance(pts.iter().copied()) * 0.1;
    from_shapes(out, &pts, tol)
}

/// Triangulates one polygon with holes (earcut). The result's points are
/// the normalized polygon's vertices; triangles are counter-clockwise.
/// Zero-area triangles are dropped. See the module docs for invalid input.
pub fn triangulate(polygon: &Polygon) -> Triangulation {
    let p = polygon.normalized();
    let mut out = Triangulation::default();
    if p.outer.is_empty() {
        return out;
    }
    let mut flat: Vec<[f64; 2]> = Vec::with_capacity(p.vertex_count());
    let mut hole_starts: Vec<u32> = Vec::with_capacity(p.holes.len());
    for (i, r) in p.rings().enumerate() {
        if i > 0 {
            hole_starts.push(flat.len() as u32);
        }
        flat.extend(r.iter().map(|q| [q.x as f64, q.y as f64]));
        out.points.extend_from_slice(r);
    }
    // Earcut's node storage is limited to u32 offsets; refuse absurd sizes.
    if flat.len() > 10_000_000 {
        out.points.clear();
        return out;
    }
    let mut idx: Vec<u32> = Vec::new();
    earcut::Earcut::<f64>::new().earcut(flat.iter().copied(), &hole_starts, &mut idx);
    out.indices.reserve(idx.len());
    for t in idx.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| DVec2::from(flat[i as usize]));
        let s = (b - a).perp_dot(c - a);
        if s > 0.0 {
            out.indices.extend_from_slice(&[t[0], t[1], t[2]]);
        } else if s < 0.0 {
            out.indices.extend_from_slice(&[t[0], t[2], t[1]]);
        }
    }
    out
}

/// Triangulates several polygons into one point/index set.
pub fn triangulate_all(polys: &[Polygon]) -> Triangulation {
    let mut out = Triangulation::default();
    for p in polys {
        let t = triangulate(p);
        let base = out.points.len() as u32;
        out.points.extend(t.points);
        out.indices.extend(t.indices.iter().map(|i| i + base));
    }
    out
}

// ---------------------------------------------------------- corner shaping

fn corner_cut(prev: Vec2, p: Vec2, next: Vec2, distance: f32) -> Option<(Vec2, Vec2, f32)> {
    let (a, b) = (prev - p, next - p);
    let (la, lb) = (a.length(), b.length());
    if la <= 0.0 || lb <= 0.0 {
        return None;
    }
    let d = distance.min(la * 0.5).min(lb * 0.5);
    if d <= 0.0 {
        return None;
    }
    Some((p + a / la * d, p + b / lb * d, d))
}

/// Cuts every corner of a closed ring flat, `distance` metres back along
/// both edges (clamped to half of each edge so cuts never cross). Straight
/// (collinear) vertices are kept as they are.
pub fn chamfer_corners(ring: &[Vec2], distance: f32) -> Vec<Vec2> {
    let r = sanitize_ring(ring, 0.0);
    if r.len() < 3 || !(distance.is_finite() && distance > 0.0) {
        return r;
    }
    let n = r.len();
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let (prev, p, next) = (r[(i + n - 1) % n], r[i], r[(i + 1) % n]);
        let straight =
            (p - prev).perp_dot(next - p).abs() <= 1e-6 * (p - prev).length() * (next - p).length();
        match corner_cut(prev, p, next, distance) {
            Some((a, b, _)) if !straight => {
                out.push(a);
                out.push(b);
            }
            _ => out.push(p),
        }
    }
    sanitize_ring(&out, 0.0)
}

/// Rounds every corner of a closed ring with an arc of `radius` metres
/// (reduced where the edges are too short), `segments` chords per corner
/// (clamped to 1..=64).
pub fn round_corners(ring: &[Vec2], radius: f32, segments: u32) -> Vec<Vec2> {
    let r = sanitize_ring(ring, 0.0);
    if r.len() < 3 || !(radius.is_finite() && radius > 0.0) {
        return r;
    }
    let segs = segments.clamp(1, 64);
    let n = r.len();
    let mut out = Vec::with_capacity(n * (segs as usize + 1));
    for i in 0..n {
        let (prev, p, next) = (r[(i + n - 1) % n], r[i], r[(i + 1) % n]);
        let (da, db) = (
            (prev - p).normalize_or_zero(),
            (next - p).normalize_or_zero(),
        );
        let cos = da.dot(db).clamp(-1.0, 1.0);
        let half = pmath::acos(cos) * 0.5; // half the interior angle
        if pmath::sin(half) < 1e-4 || (std::f32::consts::FRAC_PI_2 - half) < 1e-4 {
            out.push(p); // straight or folded back
            continue;
        }
        // Distance from the corner to the tangent points for this radius.
        let want = radius / pmath::tan(half);
        let Some((a, b, d)) = corner_cut(prev, p, next, want) else {
            out.push(p);
            continue;
        };
        let rad = d * pmath::tan(half);
        let centre = p + (da + db).normalize_or_zero() * (rad / pmath::sin(half));
        let (a0, a1) = ((a - centre).to_angle(), (b - centre).to_angle());
        let mut sweep = a1 - a0;
        // The arc turns the same way as the corner.
        let turn = (p - prev).perp_dot(next - p);
        if turn > 0.0 && sweep < 0.0 {
            sweep += std::f32::consts::TAU;
        } else if turn < 0.0 && sweep > 0.0 {
            sweep -= std::f32::consts::TAU;
        }
        for k in 0..=segs {
            let t = a0 + sweep * k as f32 / segs as f32;
            out.push(centre + Vec2::from_angle(t) * rad);
        }
    }
    sanitize_ring(&out, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x: f32, y: f32, s: f32) -> Polygon {
        Polygon::rect(Vec2::new(x, y), Vec2::new(x + s, y + s))
    }

    #[test]
    fn area_orientation_contains() {
        let p = sq(0.0, 0.0, 2.0).with_hole(sq(0.5, 0.5, 1.0).outer);
        assert!((p.area() - 3.0).abs() < 1e-6);
        let n = p.normalized();
        assert!(is_ccw(&n.outer) && !is_ccw(&n.holes[0]));
        assert!(p.contains(Vec2::new(0.25, 0.25)));
        assert!(!p.contains(Vec2::new(1.0, 1.0)));
        assert!(!p.contains(Vec2::new(3.0, 1.0)));
        // Clockwise input is fixed.
        let mut cw = sq(0.0, 0.0, 1.0);
        cw.outer.reverse();
        assert!((cw.area() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn booleans_of_overlapping_squares() {
        let (a, b) = (vec![sq(0.0, 0.0, 2.0)], vec![sq(1.0, 1.0, 2.0)]);
        assert!((total_area(&union(&a, &b)) - 7.0).abs() < 1e-4);
        assert!((total_area(&intersection(&a, &b)) - 1.0).abs() < 1e-4);
        assert!((total_area(&difference(&a, &b)) - 3.0).abs() < 1e-4);
        assert!((total_area(&xor(&a, &b)) - 6.0).abs() < 1e-4);
        // Output vertices are snapped to input vertices exactly.
        let d = difference(&a, &b);
        let inputs = all_points(&a, &b);
        for q in d[0].outer.iter() {
            let on_grid = [0.0, 1.0, 2.0].contains(&q.x) && [0.0, 1.0, 2.0].contains(&q.y);
            assert!(on_grid || inputs.contains(q), "{q}");
        }
        // A hole from a difference.
        let holed = difference(&[sq(0.0, 0.0, 4.0)], &[sq(1.0, 1.0, 1.0)]);
        assert_eq!(holed.len(), 1);
        assert_eq!(holed[0].holes.len(), 1);
        assert!((total_area(&holed) - 15.0).abs() < 1e-4);
    }

    #[test]
    fn self_intersection_is_resolved() {
        // A bow tie: both lobes kept (non-zero), total area 2 * 0.25.
        let bow = Polygon::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
        ]);
        let v = make_valid(&[bow]);
        assert_eq!(v.len(), 2);
        assert!((total_area(&v) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn offsets_grow_and_shrink() {
        let a = vec![sq(0.0, 0.0, 2.0)];
        let grown = offset(&a, 0.5, Join::Miter { limit: 4.0 });
        assert!((total_area(&grown) - 9.0).abs() < 1e-3);
        let shrunk = offset(&a, -0.5, Join::Miter { limit: 4.0 });
        assert!((total_area(&shrunk) - 1.0).abs() < 1e-3);
        let bevel = offset(&a, 0.5, Join::Bevel);
        assert!((total_area(&bevel) - (9.0 - 4.0 * 0.125)).abs() < 1e-3);
        let round = offset(&a, 0.5, Join::Round { tolerance: 0.001 });
        let exact = 4.0 + 4.0 * 2.0 * 0.5 + std::f32::consts::PI * 0.25;
        assert!((total_area(&round) - exact).abs() < 0.01);
        assert!(offset(&a, -1.5, Join::Bevel).is_empty());
    }

    #[test]
    fn triangulation_with_hole_covers_area() {
        let p = sq(0.0, 0.0, 4.0).with_hole(sq(1.0, 1.0, 2.0).outer);
        let t = triangulate(&p);
        assert_eq!(t.triangle_count(), 8);
        assert!((t.area() - 12.0).abs() < 1e-5);
    }

    #[test]
    fn degenerate_input() {
        let nan = Vec2::splat(f32::NAN);
        let junk = Polygon::new(vec![nan, Vec2::ZERO, Vec2::ZERO, Vec2::X]);
        assert!(junk.is_empty());
        assert_eq!(junk.area(), 0.0);
        assert!(triangulate(&junk).indices.is_empty());
        assert!(union(std::slice::from_ref(&junk), &[]).is_empty());
        assert!(offset(&[junk], 1.0, Join::Bevel).is_empty());
        assert_eq!(
            simplify_ring(&[Vec2::ZERO, Vec2::X, Vec2::X * 2.0], 1e-6),
            Vec::<Vec2>::new()
        );
    }

    #[test]
    fn simplify_removes_collinear_points() {
        let r = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(1.0, 2.0 + 1e-7),
            Vec2::new(0.0, 2.0),
        ];
        let s = simplify_ring(&r, 1e-4);
        assert_eq!(s.len(), 4);
        assert!((signed_area(&s) - 4.0).abs() < 1e-5);
    }

    #[test]
    fn chamfer_and_round_corners() {
        let r = sq(0.0, 0.0, 2.0).outer;
        let c = chamfer_corners(&r, 0.5);
        assert_eq!(c.len(), 8);
        assert!((signed_area(&c) - (4.0 - 4.0 * 0.125)).abs() < 1e-5);
        let f = round_corners(&r, 0.5, 8);
        assert_eq!(f.len(), 4 * 9);
        let exact = 4.0 - 4.0 * 0.25 + std::f32::consts::PI * 0.25;
        assert!((signed_area(&f) - exact).abs() < 0.01);
        // Oversized radius is clamped to half the edges.
        let big = chamfer_corners(&r, 10.0);
        assert!((signed_area(&big) - 2.0).abs() < 1e-5);
    }
}
