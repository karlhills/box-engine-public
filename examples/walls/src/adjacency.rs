//! Path-end adjacency, shared by walls and fences: which path ends meet
//! (junction nodes), which ends touch another path's body (T junctions),
//! which bodies cross and which run on top of each other.
//!
//! Pure and deterministic: the result depends only on the shapes, never on
//! their order (everything is decided in uid order), and every distance
//! test uses the fixed [`SNAP`] threshold, so the same saved intent always
//! gives the same junctions (undo and reload reproduce them exactly).
//!
//! Hysteresis is deliberately *not* here: a pure function of saved data
//! cannot remember that an end was connected a moment ago. It lives in the
//! interaction that writes the intent ([`crate::snap::Snapper`]), which
//! writes the snapped position, so what is saved is already coincident.
//!
//! Both rules call [`analyse`] with their own shapes and resolve the result
//! differently: walls mitre or butt ([`crate::plan`]), fences share a post
//! ([`crate::fence`]).

use std::collections::HashMap;

use box_engine::prelude::*;

/// Ends closer than this connect (metres): to each other, or to a body
/// within its half width plus this.
/// The same distance the tools' `Snapper` connects at.
pub const SNAP: f32 = box_engine::box_proc::snap::SNAP;
/// Points closer than this to the previous one are dropped (a zero-length
/// segment).
pub const MIN_SEGMENT: f32 = 1e-3;
/// Segments within this angle of each other count as parallel (radians).
const PARALLEL: f32 = 0.0175; // 1°

/// Which end of a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum End {
    Start,
    End,
}

impl End {
    pub const BOTH: [End; 2] = [End::Start, End::End];

    pub fn index(self) -> usize {
        match self {
            End::Start => 0,
            End::End => 1,
        }
    }
}

/// One end of one path (an index into the shapes given to [`analyse`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EndId {
    pub path: usize,
    pub end: End,
}

/// A path as the adjacency sees it: a cleaned world-space polyline (at
/// least two points, no zero-length segments) and its footprint.
#[derive(Clone, Debug, PartialEq)]
pub struct PathShape {
    pub uid: Uid,
    pub points: Vec<Vec3>,
    /// Half the footprint width (half a wall's thickness, half a post).
    pub half_width: f32,
    /// Height above the points (for crossing and overlap tests).
    pub height: f32,
}

impl PathShape {
    pub fn end_point(&self, end: End) -> Vec3 {
        match end {
            End::Start => self.points[0],
            End::End => self.points[self.points.len() - 1],
        }
    }

    /// The point next to `end` along the path.
    pub fn adjacent(&self, end: End) -> Vec3 {
        match end {
            End::Start => self.points[1],
            End::End => self.points[self.points.len() - 2],
        }
    }

    /// Unit horizontal direction from the end point into the path's body.
    pub fn arm(&self, end: End) -> Vec3 {
        flat(self.adjacent(end) - self.end_point(end)).normalize_or(Vec3::X)
    }

    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    fn segment(&self, k: usize) -> (Vec3, Vec3) {
        (self.points[k], self.points[k + 1])
    }

    fn segments(&self) -> usize {
        self.points.len() - 1
    }
}

/// `v` with its vertical component removed.
pub fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// The horizontal perpendicular of a horizontal direction (to its left
/// seen from above, +Y up).
pub fn perp(d: Vec3) -> Vec3 {
    Vec3::new(d.z, 0.0, -d.x)
}

/// Drops non-finite points and points within [`MIN_SEGMENT`] of the
/// previous kept one. Returns the kept points and how many were dropped.
pub fn clean_polyline(points: &[Vec3]) -> (Vec<Vec3>, usize) {
    let mut out: Vec<Vec3> = Vec::with_capacity(points.len());
    let mut dropped = 0;
    for p in points {
        if !p.is_finite() || out.last().is_some_and(|q| q.distance(*p) < MIN_SEGMENT) {
            dropped += 1;
            continue;
        }
        out.push(*p);
    }
    (out, dropped)
}

/// `points` with its ends replaced by `start` / `end` (snapped junction
/// points) and interior points within [`MIN_SEGMENT`] of their neighbours
/// dropped, so snapping never leaves a zero-length segment. The ends are
/// kept exactly.
pub fn snap_ends(points: &[Vec3], start: Option<Vec3>, end: Option<Vec3>) -> Vec<Vec3> {
    let n = points.len();
    if n < 2 {
        return points.to_vec();
    }
    let first = start.unwrap_or(points[0]);
    let last = end.unwrap_or(points[n - 1]);
    let mut out = vec![first];
    for p in &points[1..n - 1] {
        let prev = out[out.len() - 1];
        if prev.distance(*p) >= MIN_SEGMENT && last.distance(*p) >= MIN_SEGMENT {
            out.push(*p);
        }
    }
    if out[out.len() - 1].distance(last) >= MIN_SEGMENT || out.len() == 1 {
        out.push(last);
    }
    out
}

/// What one path end connects to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EndLink {
    /// Nothing within reach.
    Free,
    /// Meets other ends at [`Adjacency::nodes`]`[i]`.
    Node(usize),
    /// Touches another path's body (a T junction).
    Body(BodyContact),
}

/// An end touching a body: the body's path, its segment and the closest
/// point on its centreline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyContact {
    pub path: usize,
    pub segment: usize,
    pub point: Vec3,
    /// The closest point is a vertex of the body (a corner or an end), so
    /// the segment's face may not cover the whole end.
    pub at_vertex: bool,
}

/// Two or more ends meeting. `point` is where they meet: the position of
/// the first end in uid order; the others snap to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub point: Vec3,
    /// In uid order (start before end on one path).
    pub ends: Vec<EndId>,
}

/// Two bodies crossing away from their ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crossing {
    pub a: usize,
    pub b: usize,
    pub point: Vec3,
}

/// Two parallel segments whose footprints overlap along their length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlap {
    pub a: usize,
    pub b: usize,
    /// Length of the shared stretch.
    pub length: f32,
}

/// The result of [`analyse`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adjacency {
    /// Per path, `[start, end]`.
    pub links: Vec<[EndLink; 2]>,
    pub nodes: Vec<Node>,
    pub crossings: Vec<Crossing>,
    pub overlaps: Vec<Overlap>,
}

impl Adjacency {
    pub fn link(&self, id: EndId) -> EndLink {
        self.links[id.path][id.end.index()]
    }

    /// Paths connected to `path` by a node or a body contact (either way).
    pub fn neighbours(&self, path: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for end in End::BOTH {
            match self.links[path][end.index()] {
                EndLink::Node(n) => out.extend(self.nodes[n].ends.iter().map(|e| e.path)),
                EndLink::Body(c) => out.push(c.path),
                EndLink::Free => {}
            }
        }
        for (other, links) in self.links.iter().enumerate() {
            if links
                .iter()
                .any(|l| matches!(l, EndLink::Body(c) if c.path == path))
            {
                out.push(other);
            }
        }
        out.retain(|p| *p != path);
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Finds junctions, T contacts, crossings and overlaps (see the module
/// docs). Every shape must have at least two points.
///
/// Neighbour queries go through a uniform grid over the plan (a spatial
/// hash), so the cost grows with the number of paths, not its square.
pub fn analyse(shapes: &[PathShape]) -> Adjacency {
    let mut order: Vec<usize> = (0..shapes.len()).collect();
    order.sort_by_key(|i| shapes[*i].uid);
    // rank[path] = its position in uid order (tie-breaks follow uids).
    let mut rank = vec![0usize; shapes.len()];
    for (r, &i) in order.iter().enumerate() {
        rank[i] = r;
    }
    let grid = Grid::new(shapes);

    // Nodes: ends in uid order join the closest existing node within SNAP
    // of its point, or start one. Members are always within SNAP of the
    // node point, and the result does not depend on the input order.
    let mut nodes: Vec<Node> = Vec::new();
    let mut node_cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    let mut node_of: Vec<[Option<usize>; 2]> = vec![[None, None]; shapes.len()];
    for &i in &order {
        for end in End::BOTH {
            let p = shapes[i].end_point(end);
            let (cx, cz) = grid.cell(p);
            // The closest node within SNAP; ties go to the older node.
            let mut best: Option<(usize, f32)> = None;
            for dx in -1..=1 {
                for dz in -1..=1 {
                    for &n in node_cells.get(&(cx + dx, cz + dz)).into_iter().flatten() {
                        let d = nodes[n].point.distance(p);
                        let better = best.is_none_or(|(bn, bd)| d < bd || (d == bd && n < bn));
                        if d <= SNAP && better {
                            best = Some((n, d));
                        }
                    }
                }
            }
            let id = EndId { path: i, end };
            let n = match best {
                Some((n, _)) => {
                    nodes[n].ends.push(id);
                    n
                }
                None => {
                    nodes.push(Node {
                        point: p,
                        ends: vec![id],
                    });
                    node_cells
                        .entry(grid.cell(p))
                        .or_default()
                        .push(nodes.len() - 1);
                    nodes.len() - 1
                }
            };
            node_of[i][end.index()] = Some(n);
        }
    }
    // Keep nodes with two or more ends.
    let mut remap = vec![None; nodes.len()];
    let mut kept = Vec::new();
    for (n, node) in nodes.into_iter().enumerate() {
        if node.ends.len() >= 2 {
            remap[n] = Some(kept.len());
            kept.push(node);
        }
    }

    let mut links = vec![[EndLink::Free; 2]; shapes.len()];
    for &i in &order {
        for end in End::BOTH {
            let n = node_of[i][end.index()].and_then(|n| remap[n]);
            links[i][end.index()] = match n {
                Some(n) => EndLink::Node(n),
                None => {
                    body_contact(shapes, &grid, &rank, i, end).map_or(EndLink::Free, EndLink::Body)
                }
            };
        }
    }

    // Path pairs sharing a cell, in uid order.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for run in grid.runs() {
        for (x, &(_, a, _)) in run.iter().enumerate() {
            for &(_, b, _) in &run[x + 1..] {
                if a != b {
                    let (ra, rb) = (rank[a], rank[b]);
                    pairs.push((ra.min(rb), ra.max(rb)));
                }
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut crossings = Vec::new();
    let mut overlaps = Vec::new();
    for (ra, rb) in pairs {
        let (a, b) = (order[ra], order[rb]);
        if let Some(c) = crossing(&shapes[a], &shapes[b]) {
            crossings.push(Crossing { a, b, point: c });
        }
        if let Some(length) = overlap(&shapes[a], &shapes[b]) {
            overlaps.push(Overlap { a, b, length });
        }
    }
    Adjacency {
        links,
        nodes: kept,
        crossings,
        overlaps,
    }
}

/// A uniform grid over the plan holding every segment (path, segment) in
/// the cells within reach of it (the widest half width plus [`SNAP`]):
/// entries sorted by cell, so a cell is a run found by binary search.
struct Grid {
    size: f32,
    /// (cell, path, segment), sorted.
    entries: Vec<((i32, i32), usize, usize)>,
}

impl Grid {
    fn new(shapes: &[PathShape]) -> Self {
        let reach = shapes.iter().map(|s| s.half_width).fold(0.0f32, f32::max) + SNAP;
        let size = (4.0 * reach).max(1.0);
        let mut grid = Self {
            size,
            entries: Vec::new(),
        };
        let mut marked: Vec<(i32, i32)> = Vec::new();
        for (i, s) in shapes.iter().enumerate() {
            for k in 0..s.segments() {
                // Pieces at most a cell long; each marks the cells its
                // bounding box grown by the reach overlaps.
                let (a, b) = s.segment(k);
                let pieces = (flat(b - a).length() / size).ceil().max(1.0) as usize;
                marked.clear();
                for t in 0..pieces {
                    let p0 = a.lerp(b, t as f32 / pieces as f32);
                    let p1 = a.lerp(b, (t + 1) as f32 / pieces as f32);
                    let lo = grid.cell(p0.min(p1) - Vec3::splat(reach));
                    let hi = grid.cell(p0.max(p1) + Vec3::splat(reach));
                    for cx in lo.0..=hi.0 {
                        for cz in lo.1..=hi.1 {
                            marked.push((cx, cz));
                        }
                    }
                }
                marked.sort_unstable();
                marked.dedup();
                grid.entries.extend(marked.iter().map(|c| (*c, i, k)));
            }
        }
        grid.entries.sort_unstable();
        grid
    }

    fn cell(&self, p: Vec3) -> (i32, i32) {
        let c = |v: f32| {
            (v / self.size)
                .floor()
                .clamp(i32::MIN as f32, i32::MAX as f32) as i32
        };
        (c(p.x), c(p.z))
    }

    /// The entries of one cell.
    fn at(&self, cell: (i32, i32)) -> &[((i32, i32), usize, usize)] {
        let lo = self.entries.partition_point(|e| e.0 < cell);
        let hi = self.entries.partition_point(|e| e.0 <= cell);
        &self.entries[lo..hi]
    }

    /// The entries grouped by cell.
    fn runs(&self) -> impl Iterator<Item = &[((i32, i32), usize, usize)]> {
        self.entries.chunk_by(|a, b| a.0 == b.0)
    }
}

/// Closest point on segment `(a, b)` to `p`, and its parameter.
fn closest_on_segment(p: Vec3, a: Vec3, b: Vec3) -> (Vec3, f32) {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t, t)
}

/// The body (of another path) the end of `i` touches, if any: the closest
/// centreline within the body's half width plus [`SNAP`] (ties: lower uid,
/// then lower segment).
fn body_contact(
    shapes: &[PathShape],
    grid: &Grid,
    rank: &[usize],
    i: usize,
    end: End,
) -> Option<BodyContact> {
    let p = shapes[i].end_point(end);
    let mut candidates: Vec<(usize, usize)> = grid
        .at(grid.cell(p))
        .iter()
        .filter(|(_, j, _)| *j != i)
        .map(|(_, j, k)| (*j, *k))
        .collect();
    candidates.sort_unstable_by_key(|(j, k)| (rank[*j], *k));
    // In reach, by distance; distances within 10 µm are a tie (an
    // end at a body's corner is equally close to both segments, and float
    // noise must not pick one), broken by the segment the end is most
    // perpendicular to, then by uid and segment order.
    let arm = shapes[i].arm(end);
    let mut found: Vec<(f32, f32, BodyContact)> = Vec::new();
    for (j, k) in candidates {
        let s = &shapes[j];
        let (a, b) = s.segment(k);
        let (c, t) = closest_on_segment(p, a, b);
        let d = c.distance(p);
        if d <= s.half_width + SNAP {
            let across = arm.cross(flat(b - a).normalize_or_zero()).length();
            found.push((
                d,
                across,
                BodyContact {
                    path: j,
                    segment: k,
                    point: c,
                    at_vertex: t <= 0.0 || t >= 1.0,
                },
            ));
        }
    }
    let nearest = found.iter().map(|f| f.0).fold(f32::INFINITY, f32::min);
    found
        .into_iter()
        .filter(|f| f.0 <= nearest + 1e-5)
        .fold(
            None::<(f32, BodyContact)>,
            |best, (_, across, c)| match best {
                Some((b, _)) if b >= across - 1e-6 => best,
                _ => Some((across, c)),
            },
        )
        .map(|(_, c)| c)
}

fn y_ranges_overlap(a: &PathShape, ya: f32, b: &PathShape, yb: f32) -> bool {
    ya < yb + b.height && yb < ya + a.height
}

/// Where the bodies of `a` and `b` cross in plan, away from both paths'
/// ends (an end near the other body is a T junction, not a crossing).
fn crossing(a: &PathShape, b: &PathShape) -> Option<Vec3> {
    for ka in 0..a.segments() {
        let (a0, a1) = a.segment(ka);
        for kb in 0..b.segments() {
            let (b0, b1) = b.segment(kb);
            let r = flat(a1 - a0);
            let s = flat(b1 - b0);
            let denom = r.x * s.z - r.z * s.x;
            if denom.abs() < 1e-9 {
                continue;
            }
            let q = flat(b0 - a0);
            let t = (q.x * s.z - q.z * s.x) / denom;
            let u = (q.x * r.z - q.z * r.x) / denom;
            if !(0.0..=1.0).contains(&t) || !(0.0..=1.0).contains(&u) {
                continue;
            }
            let x = a0 + (a1 - a0) * t;
            let xb = b0 + (b1 - b0) * u;
            if !y_ranges_overlap(a, x.y, b, xb.y) {
                continue;
            }
            let near_end = |shape: &PathShape, reach: f32| {
                End::BOTH
                    .iter()
                    .any(|e| flat(shape.end_point(*e) - x).length() <= reach)
            };
            if near_end(a, b.half_width + SNAP) || near_end(b, a.half_width + SNAP) {
                continue;
            }
            return Some(x);
        }
    }
    None
}

/// The longest stretch along which a segment of `a` and a parallel segment
/// of `b` overlap in plan (footprints touching), if longer than [`SNAP`].
fn overlap(a: &PathShape, b: &PathShape) -> Option<f32> {
    let mut longest = 0.0f32;
    for ka in 0..a.segments() {
        let (a0, a1) = a.segment(ka);
        let da = flat(a1 - a0);
        let la = da.length();
        let ua = da / la;
        for kb in 0..b.segments() {
            let (b0, b1) = b.segment(kb);
            let db = flat(b1 - b0);
            let ub = db / db.length();
            if ua.cross(ub).length() > PARALLEL.sin() {
                continue;
            }
            // Distance between the lines, and the overlap of the projections.
            let off = flat(b0 - a0);
            let across = (off - ua * off.dot(ua)).length();
            if across >= a.half_width + b.half_width - 1e-4 {
                continue;
            }
            let (t0, t1) = (off.dot(ua), flat(b1 - a0).dot(ua));
            let (lo, hi) = (t0.min(t1).max(0.0), t0.max(t1).min(la));
            if hi - lo > SNAP && y_ranges_overlap(a, a0.y, b, b0.y) {
                longest = longest.max(hi - lo);
            }
        }
    }
    (longest > 0.0).then_some(longest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(id: u128, pts: &[[f32; 2]]) -> PathShape {
        PathShape {
            uid: Uid::from_u128(id),
            points: pts.iter().map(|p| Vec3::new(p[0], 0.0, p[1])).collect(),
            half_width: 0.15,
            height: 2.5,
        }
    }

    #[test]
    fn ends_within_snap_meet_at_the_first_ends_point() {
        let a = shape(1, &[[0.0, 0.0], [4.0, 0.0]]);
        let b = shape(2, &[[4.08, 0.0], [4.08, 3.0]]);
        let adj = analyse(&[b.clone(), a.clone()]);
        assert_eq!(adj.nodes.len(), 1);
        assert_eq!(adj.nodes[0].point, Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(
            adj.nodes[0].ends,
            [
                EndId {
                    path: 1,
                    end: End::End
                },
                EndId {
                    path: 0,
                    end: End::Start
                }
            ]
        );
        // 12 cm between ends, but within reach of the body: a T contact.
        let b = shape(2, &[[4.12, 0.0], [4.12, 3.0]]);
        let adj = analyse(&[a.clone(), b]);
        assert!(adj.nodes.is_empty());
        assert!(matches!(adj.links[0][1], EndLink::Body(c) if c.path == 1));
        // Beyond the body's half width plus SNAP: free (hysteresis is the
        // tool's job, see `snap`).
        let b = shape(2, &[[4.26, 0.0], [4.26, 3.0]]);
        let adj = analyse(&[a, b]);
        assert_eq!(adj.links[0][1], EndLink::Free);
    }

    #[test]
    fn t_contact_crossing_and_overlap() {
        let a = shape(1, &[[0.0, 0.0], [6.0, 0.0]]);
        let t = shape(2, &[[3.0, 0.15], [3.0, 4.0]]);
        let adj = analyse(&[a.clone(), t]);
        assert!(matches!(adj.links[1][0], EndLink::Body(c) if c.path == 0));
        assert_eq!(adj.neighbours(0), [1]);
        assert!(adj.crossings.is_empty());

        let x = shape(3, &[[3.0, -2.0], [3.0, 2.0]]);
        let adj = analyse(&[a.clone(), x]);
        assert_eq!(adj.crossings.len(), 1);
        assert!(adj.crossings[0].point.distance(Vec3::new(3.0, 0.0, 0.0)) < 1e-5);

        let o = shape(4, &[[2.0, 0.0], [9.0, 0.0]]);
        let adj = analyse(&[a, o]);
        assert_eq!(adj.overlaps.len(), 1);
        assert!((adj.overlaps[0].length - 4.0).abs() < 1e-5);
    }

    #[test]
    fn cleaning_drops_zero_length_segments() {
        let (pts, dropped) = clean_polyline(&[
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 0.0005),
            Vec3::X,
            Vec3::X,
            Vec3::splat(f32::NAN),
        ]);
        assert_eq!(pts, [Vec3::ZERO, Vec3::X]);
        assert_eq!(dropped, 3);
    }
}
