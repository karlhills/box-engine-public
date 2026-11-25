//! Queries on a [`NavMesh`]: the polygon under a point, heights, paths (A*
//! over polygons, then the funnel algorithm), straight-line walkability and
//! random points.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};

use glam::{Vec2, Vec3};

use crate::mesh::{FxBuild, NavMesh, NavPoly, NavTile, PolyMap, PolyRef, xz};

/// How a [`NavPath`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStatus {
    /// It reaches the goal.
    Complete,
    /// The goal cannot be reached (another island, off the mesh, or the
    /// search limit); the path goes as close as it can.
    Partial,
    /// The start is not on the navmesh (nothing within reach of it).
    NoStart,
}

/// A path: corner points from the start (snapped onto the mesh) to the
/// goal, or as close as it gets.
#[derive(Clone, Debug, PartialEq)]
pub struct NavPath {
    pub points: Vec<Vec3>,
    pub status: PathStatus,
    /// The polygons it crosses, start first.
    pub polys: Vec<PolyRef>,
}

impl NavPath {
    /// Total length along its points.
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.status == PathStatus::Complete
    }
}

/// How far from a point to look for the navmesh: 2 m sideways, 3 m up and
/// down.
pub const SEARCH_EXTENTS: Vec3 = Vec3::new(2.0, 3.0, 2.0);

/// The most polygons one path search expands before giving a partial path.
pub const MAX_SEARCH_NODES: usize = 8192;

#[derive(Clone, Copy, PartialEq)]
struct Open {
    f: f32,
    r: PolyRef,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lowest f first; ties by poly for a stable order.
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.r.cmp(&self.r))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct Node {
    g: f32,
    pos: Vec3,
    parent: Option<PolyRef>,
}

/// Twice the signed area of a, b, c on the ground plane (the funnel's
/// orientation test).
fn triarea2(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ax = b.x - a.x;
    let az = b.z - a.z;
    let bx = c.x - a.x;
    let bz = c.z - a.z;
    bx * az - ax * bz
}

fn same(a: Vec3, b: Vec3) -> bool {
    a.distance_squared(b) < 1e-10
}

fn inside_convex(points: &[Vec3], p: Vec2) -> bool {
    let n = points.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let a = xz(points[i]);
        let b = xz(points[(i + 1) % n]);
        let c = (b - a).perp_dot(p - a);
        if c.abs() < 1e-7 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// The point of segment a–b closest to `p` on the ground plane, with its
/// height interpolated.
fn closest_on_segment(a: Vec3, b: Vec3, p: Vec2) -> Vec3 {
    let ab = xz(b) - xz(a);
    let len2 = ab.length_squared();
    let t = if len2 < 1e-12 {
        0.0
    } else {
        ((p - xz(a)).dot(ab) / len2).clamp(0.0, 1.0)
    };
    a.lerp(b, t)
}

impl NavMesh {
    /// The height of the mesh at `x`, `z` on polygon `r` (its height-detail
    /// triangles; the nearest one when the point is just outside).
    pub fn height_at(&self, r: PolyRef, x: f32, z: f32) -> Option<f32> {
        let (tile, poly) = self.poly(r)?;
        Some(poly_height(tile, poly, Vec2::new(x, z)))
    }

    /// The polygon nearest to `p` within `extents` (half sizes), and the
    /// nearest point on it.
    pub fn find_poly(&self, p: Vec3, extents: Vec3) -> Option<(PolyRef, Vec3)> {
        if !p.is_finite() {
            return None;
        }
        let lo = p - extents;
        let hi = p + extents;
        let mut best: Option<(f32, PolyRef, Vec3)> = None;
        for coord in self.config().tiles_in(lo, hi) {
            let Some(tile) = self.tile(coord) else {
                continue;
            };
            if tile.max.cmplt(lo).any() || tile.min.cmpgt(hi).any() {
                continue;
            }
            for (i, poly) in tile.polys.iter().enumerate() {
                if poly.max.cmplt(lo).any() || poly.min.cmpgt(hi).any() {
                    continue;
                }
                let q = closest_on_poly(tile, poly, p);
                let d = q.distance_squared(p);
                if best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((
                        d,
                        PolyRef {
                            tile: coord,
                            index: i as u16,
                        },
                        q,
                    ));
                }
            }
        }
        best.map(|(_, r, q)| (r, q))
    }

    /// The nearest point on the navmesh to `p` (within [`SEARCH_EXTENTS`]).
    pub fn closest_point(&self, p: Vec3) -> Option<Vec3> {
        self.find_poly(p, SEARCH_EXTENTS).map(|(_, q)| q)
    }

    /// The shortest path from `start` to `end` along the navmesh. Both are
    /// snapped onto the mesh (within [`SEARCH_EXTENTS`]). When `end` cannot
    /// be reached the path ends at the reachable point nearest to it
    /// ([`PathStatus::Partial`]).
    pub fn find_path(&self, start: Vec3, end: Vec3) -> NavPath {
        let Some((start_ref, start_pt)) = self.find_poly(start, SEARCH_EXTENTS) else {
            return NavPath {
                points: Vec::new(),
                status: PathStatus::NoStart,
                polys: Vec::new(),
            };
        };
        let goal = self.find_poly(end, SEARCH_EXTENTS);
        let target = goal.map_or(end, |(_, q)| q);
        let goal_ref = goal.map(|(r, _)| r);

        let mut nodes: PolyMap<Node> = PolyMap::default();
        let mut closed: HashSet<PolyRef, FxBuild> = HashSet::default();
        let mut open = BinaryHeap::new();
        nodes.insert(
            start_ref,
            Node {
                g: 0.0,
                pos: start_pt,
                parent: None,
            },
        );
        open.push(Open {
            f: start_pt.distance(target),
            r: start_ref,
        });
        let mut best = (start_pt.distance(target), start_ref);
        let mut reached = false;
        let mut expanded = 0;
        while let Some(Open { r, .. }) = open.pop() {
            if !closed.insert(r) {
                continue;
            }
            if Some(r) == goal_ref {
                reached = true;
                best = (0.0, r);
                break;
            }
            expanded += 1;
            if expanded > MAX_SEARCH_NODES {
                break;
            }
            let (g, pos) = {
                let n = &nodes[&r];
                (n.g, n.pos)
            };
            for link in self.links(r) {
                if closed.contains(&link.to) {
                    continue;
                }
                let mid = (link.a + link.b) * 0.5;
                let mut ng = g + pos.distance(mid);
                if Some(link.to) == goal_ref {
                    ng += mid.distance(target);
                }
                let better = nodes.get(&link.to).is_none_or(|n| ng < n.g);
                if better {
                    nodes.insert(
                        link.to,
                        Node {
                            g: ng,
                            pos: mid,
                            parent: Some(r),
                        },
                    );
                    let h = mid.distance(target);
                    if h < best.0 {
                        best = (h, link.to);
                    }
                    open.push(Open {
                        f: ng + h,
                        r: link.to,
                    });
                }
            }
        }
        // The corridor of polygons, start first.
        let mut polys = vec![best.1];
        let mut cur = best.1;
        while let Some(p) = nodes.get(&cur).and_then(|n| n.parent) {
            polys.push(p);
            cur = p;
            if polys.len() > nodes.len() {
                break;
            }
        }
        polys.reverse();
        let end_pt = if reached {
            target
        } else {
            // As close to the goal as the last polygon gets.
            match self.poly(best.1) {
                Some((tile, poly)) => closest_on_poly(tile, poly, target),
                None => start_pt,
            }
        };
        let points = self.funnel(&polys, start_pt, end_pt);
        NavPath {
            points,
            status: if reached {
                PathStatus::Complete
            } else {
                PathStatus::Partial
            },
            polys,
        }
    }

    /// The string-pulled path through the corridor `polys`.
    fn funnel(&self, polys: &[PolyRef], start: Vec3, end: Vec3) -> Vec<Vec3> {
        // Portals as (left, right) seen walking from the start.
        let mut portals: Vec<(Vec3, Vec3)> = vec![(start, start)];
        let mut prev = start;
        for w in polys.windows(2) {
            let Some(link) = self
                .links(w[0])
                .iter()
                .filter(|l| l.to == w[1])
                .min_by(|a, b| {
                    let da = ((a.a + a.b) * 0.5).distance_squared(prev);
                    let db = ((b.a + b.b) * 0.5).distance_squared(prev);
                    da.total_cmp(&db)
                })
            else {
                continue;
            };
            let mid = (link.a + link.b) * 0.5;
            // The right end is to the right of the way from `prev` to the
            // portal's middle.
            let (left, right) = if triarea2(prev, mid, link.a) > 0.0 {
                (link.b, link.a)
            } else {
                (link.a, link.b)
            };
            portals.push((left, right));
            prev = mid;
        }
        portals.push((end, end));

        let mut path = vec![start];
        let (mut apex, mut left, mut right) = (start, start, start);
        let (mut left_i, mut right_i) = (0usize, 0usize);
        let mut apex_i;
        let mut i = 1;
        let mut guard = 0;
        while i < portals.len() {
            guard += 1;
            if guard > portals.len() * 4 + 16 {
                break;
            }
            let (pl, pr) = portals[i];
            if triarea2(apex, right, pr) <= 0.0 {
                if same(apex, right) || triarea2(apex, left, pr) > 0.0 {
                    right = pr;
                    right_i = i;
                } else {
                    // Right crossed left: left becomes a corner.
                    if !same(*path.last().unwrap_or(&apex), left) {
                        path.push(left);
                    }
                    apex = left;
                    apex_i = left_i;
                    left = apex;
                    right = apex;
                    left_i = apex_i;
                    right_i = apex_i;
                    i = apex_i + 1;
                    continue;
                }
            }
            if triarea2(apex, left, pl) >= 0.0 {
                if same(apex, left) || triarea2(apex, right, pl) < 0.0 {
                    left = pl;
                    left_i = i;
                } else {
                    if !same(*path.last().unwrap_or(&apex), right) {
                        path.push(right);
                    }
                    apex = right;
                    apex_i = right_i;
                    left = apex;
                    right = apex;
                    left_i = apex_i;
                    right_i = apex_i;
                    i = apex_i + 1;
                    continue;
                }
            }
            i += 1;
        }
        if !same(*path.last().unwrap_or(&start), end) {
            path.push(end);
        }
        drop_collinear(path)
    }

    /// Whether an agent can walk straight from `a` to `b`: every point of
    /// the line (sampled every cell) is on the navmesh at a height close to
    /// the line's.
    pub fn is_walkable_line(&self, a: Vec3, b: Vec3) -> bool {
        let Some((_, a)) = self.find_poly(a, SEARCH_EXTENTS) else {
            return false;
        };
        let Some((_, b)) = self.find_poly(b, SEARCH_EXTENTS) else {
            return false;
        };
        let step = self.config().cell_size;
        let len = xz(a).distance(xz(b));
        let n = (len / step).ceil().max(1.0) as usize;
        let extents = Vec3::new(step, self.config().agent_height, step);
        let mut y = a.y;
        for k in 0..=n {
            let p = a.lerp(b, k as f32 / n as f32);
            let probe = Vec3::new(p.x, y, p.z);
            let Some((_, q)) = self.find_poly(probe, extents) else {
                return false;
            };
            if xz(q).distance(xz(p)) > step * 0.5
                || (q.y - y).abs() > self.config().step_height + step
            {
                return false;
            }
            y = q.y;
        }
        true
    }

    /// A random point on the navmesh within `radius` (on the ground plane)
    /// of `center`, reachable from it; the same `seed` gives the same point.
    pub fn random_point_near(&self, center: Vec3, radius: f32, seed: u64) -> Option<Vec3> {
        let (start, at) = self.find_poly(center, SEARCH_EXTENTS)?;
        // Polygons reachable from the start whose centre is within reach.
        let mut seen = HashSet::from([start]);
        let mut queue = VecDeque::from([start]);
        let mut pool: Vec<(PolyRef, f32)> = Vec::new();
        while let Some(r) = queue.pop_front() {
            let Some((_, poly)) = self.poly(r) else {
                continue;
            };
            pool.push((r, poly.area.max(1e-4)));
            if pool.len() > 4096 {
                break;
            }
            for l in self.links(r) {
                if seen.contains(&l.to) {
                    continue;
                }
                let Some((_, p)) = self.poly(l.to) else {
                    continue;
                };
                if xz(p.center).distance(xz(center)) <= radius + p.max.x - p.min.x {
                    seen.insert(l.to);
                    queue.push_back(l.to);
                }
            }
        }
        let mut rng = seed;
        let mut next = move || {
            rng = rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut x = rng;
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            ((x ^ (x >> 31)) >> 11) as f32 / (1u64 << 53) as f32
        };
        // A few tries for a point inside the circle.
        let total: f32 = pool.iter().map(|(_, a)| a).sum();
        for _ in 0..16 {
            let mut pick = next() * total;
            let mut chosen = pool[0].0;
            for (r, a) in &pool {
                if pick <= *a {
                    chosen = *r;
                    break;
                }
                pick -= a;
            }
            let (tile, poly) = self.poly(chosen)?;
            let pts = tile.poly_points(poly);
            // A random point in a random fan triangle (by area).
            let tri_areas: Vec<f32> = (1..pts.len() - 1)
                .map(|k| triarea2(pts[0], pts[k], pts[k + 1]).abs())
                .collect();
            let sum: f32 = tri_areas.iter().sum();
            let mut t = next() * sum;
            let mut k = 1;
            for (j, a) in tri_areas.iter().enumerate() {
                if t <= *a {
                    k = j + 1;
                    break;
                }
                t -= a;
            }
            let (mut u, mut v) = (next(), next());
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let p = pts[0] + (pts[k] - pts[0]) * u + (pts[k + 1] - pts[0]) * v;
            if xz(p).distance(xz(center)) <= radius {
                let y = poly_height(tile, poly, xz(p));
                return Some(Vec3::new(p.x, y, p.z));
            }
        }
        Some(at)
    }
}

/// Removes corners that lie on the line between their neighbours (on the
/// ground plane), such as where a straight path crosses a tile corner.
fn drop_collinear(points: Vec<Vec3>) -> Vec<Vec3> {
    if points.len() < 3 {
        return points;
    }
    let mut out: Vec<Vec3> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if i > 0 && i + 1 < points.len() {
            let a = *out.last().unwrap_or(&points[0]);
            let b = points[i + 1];
            let len = xz(b).distance(xz(a)).max(1e-6);
            if triarea2(a, *p, b).abs() / len < 0.01 {
                continue;
            }
        }
        out.push(*p);
    }
    out
}

/// The nearest point to `p` on the polygon.
fn closest_on_poly(tile: &NavTile, poly: &NavPoly, p: Vec3) -> Vec3 {
    let pts = tile.poly_points(poly);
    let q = xz(p);
    if inside_convex(&pts, q) {
        return Vec3::new(p.x, poly_height(tile, poly, q), p.z);
    }
    let mut best = pts[0];
    let mut best_d = f32::INFINITY;
    for i in 0..pts.len() {
        let c = closest_on_segment(pts[i], pts[(i + 1) % pts.len()], q);
        let d = xz(c).distance_squared(q);
        if d < best_d {
            best_d = d;
            best = c;
        }
    }
    Vec3::new(best.x, poly_height(tile, poly, xz(best)), best.z)
}

/// Height at `q` from the polygon's detail triangles: the one containing
/// it, else the nearest.
fn poly_height(tile: &NavTile, poly: &NavPoly, q: Vec2) -> f32 {
    let mut nearest: Option<(f32, f32)> = None;
    for [a, b, c] in tile.detail_tris(poly) {
        let (a2, b2, c2) = (xz(a), xz(b), xz(c));
        let v0 = c2 - a2;
        let v1 = b2 - a2;
        let v2 = q - a2;
        let denom = v0.perp_dot(v1);
        if denom.abs() < 1e-12 {
            continue;
        }
        let u = v2.perp_dot(v1) / denom;
        let v = v0.perp_dot(v2) / denom;
        let w = 1.0 - u - v;
        let eps = 1e-4;
        let y = a.y + (c.y - a.y) * u + (b.y - a.y) * v;
        if u >= -eps && v >= -eps && w >= -eps {
            return y;
        }
        // Distance outside, roughly: how negative the worst weight is.
        let outside = -(u.min(v).min(w));
        if nearest.is_none_or(|(d, _)| outside < d) {
            let clamped = a.y + (c.y - a.y) * u.clamp(0.0, 1.0) + (b.y - a.y) * v.clamp(0.0, 1.0);
            nearest = Some((outside, clamped));
        }
    }
    nearest.map_or(poly.center.y, |(_, y)| y)
}
