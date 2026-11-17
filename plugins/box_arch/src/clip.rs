//! 2D helpers for elevation and plan polygons: conforming slicing of
//! triangles and convex pieces, convex clipping, and "outline minus holes"
//! triangulated with the box_proc polygon kernel (spike S6, rule J6).
//!
//! Slicing is **conforming**: a split point on an edge is computed from the
//! edge's endpoints in a canonical order, so two triangles sharing the edge
//! get the bit-identical point and no T-junction cracks appear.

use box_proc::poly::{self, Polygon};
use glam::Vec2;

pub(crate) type Tri = [Vec2; 3];

/// Point where segment `a`–`b` crosses `axis == value` (canonical order).
pub(crate) fn cross_at(a: Vec2, b: Vec2, axis: usize, value: f32) -> Vec2 {
    let (a, b) = if (a.x, a.y) <= (b.x, b.y) {
        (a, b)
    } else {
        (b, a)
    };
    let d = b[axis] - a[axis];
    let t = if d.abs() > 0.0 {
        (value - a[axis]) / d
    } else {
        0.0
    };
    let mut p = a + (b - a) * t;
    p[axis] = value;
    p
}

/// Sutherland–Hodgman clip of a convex polygon against `axis >= value`
/// (`keep_ge`) or `axis <= value`.
pub(crate) fn clip_convex(poly: &[Vec2], axis: usize, value: f32, keep_ge: bool) -> Vec<Vec2> {
    const E: f32 = 1e-6;
    let inside = |p: Vec2| {
        if keep_ge {
            p[axis] >= value - E
        } else {
            p[axis] <= value + E
        }
    };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (ia, ib) = (inside(a), inside(b));
        if ia {
            out.push(a);
        }
        if ia != ib {
            let on_a = (a[axis] - value).abs() <= E;
            let on_b = (b[axis] - value).abs() <= E;
            if !on_a && !on_b {
                out.push(cross_at(a, b, axis, value));
            }
        }
    }
    out
}

/// Twice the signed area, summed relative to the first point: the
/// shoelace over absolute coordinates cancels catastrophically for small
/// pieces away from the origin (a real 0.1 mm piece at U = 0.6 came out
/// exactly 0 and was dropped, leaving a hole).
fn area2(poly: &[Vec2]) -> f32 {
    let Some(&o) = poly.first() else {
        return 0.0;
    };
    let mut a = 0.0;
    for i in 1..poly.len().saturating_sub(1) {
        a += (poly[i] - o).perp_dot(poly[i + 1] - o);
    }
    a
}

fn fan(poly: &[Vec2], out: &mut Vec<Tri>) {
    if poly.len() < 3 {
        return;
    }
    // Only truly flat pieces are dropped (relative to their size): small
    // real pieces next to a slice keep the slice conforming.
    let e = poly
        .iter()
        .map(|p| (*p - poly[0]).length_squared())
        .fold(0.0f32, f32::max);
    if area2(poly).abs() <= 1e-9 * e {
        return;
    }
    for i in 1..poly.len() - 1 {
        out.push([poly[0], poly[i], poly[i + 1]]);
    }
}

/// Splits triangles along `axis == value` (conforming).
pub(crate) fn slice_tris(tris: Vec<Tri>, axis: usize, value: f32) -> Vec<Tri> {
    const E: f32 = 1e-6;
    let mut out = Vec::with_capacity(tris.len() + 8);
    for t in tris {
        let lo = t.iter().all(|p| p[axis] <= value + E);
        let hi = t.iter().all(|p| p[axis] >= value - E);
        if lo || hi {
            out.push(t);
            continue;
        }
        fan(&clip_convex(&t, axis, value, false), &mut out);
        fan(&clip_convex(&t, axis, value, true), &mut out);
    }
    out
}

/// Splits a convex polygon at every value of `cuts` along `axis`.
pub(crate) fn split_convex(poly: Vec<Vec2>, axis: usize, cuts: &[f32]) -> Vec<Vec<Vec2>> {
    let mut pieces = vec![poly];
    for &c in cuts {
        let mut next = Vec::with_capacity(pieces.len() + 1);
        for p in pieces {
            let lo = p.iter().all(|q| q[axis] <= c + 1e-6);
            let hi = p.iter().all(|q| q[axis] >= c - 1e-6);
            if lo || hi {
                next.push(p);
                continue;
            }
            let a = clip_convex(&p, axis, c, false);
            let b = clip_convex(&p, axis, c, true);
            if a.len() >= 3 {
                next.push(a);
            }
            if b.len() >= 3 {
                next.push(b);
            }
        }
        pieces = next;
    }
    pieces
}

/// `outline` minus `holes`, triangulated. The kernel snaps boolean output
/// back onto input vertices, so edges shared with reveals stay
/// bit-identical. Without holes a convex-or-not outline is triangulated
/// directly.
pub(crate) fn difference_tris(outline: &[Vec2], holes: &[Vec<Vec2>]) -> Vec<Tri> {
    let subject = [Polygon::new(outline.to_vec())];
    let regions = if holes.is_empty() {
        vec![subject[0].clone()]
    } else {
        let clip: Vec<Polygon> = holes.iter().map(|h| Polygon::new(h.clone())).collect();
        poly::difference(&subject, &clip)
    };
    let mut tris = Vec::new();
    for r in &regions {
        let t = poly::triangulate(r);
        for i in t.indices.as_chunks::<3>().0 {
            tris.push([
                t.points[i[0] as usize],
                t.points[i[1] as usize],
                t.points[i[2] as usize],
            ]);
        }
    }
    tris
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicing_is_conforming_on_shared_edges() {
        let a = [Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(0.3, 1.7)];
        let b = [
            Vec2::new(1.0, 0.0),
            Vec2::new(1.2, 1.9),
            Vec2::new(0.3, 1.7),
        ];
        let out = slice_tris(vec![a, b], 1, 0.77);
        // Every point on y = 0.77 appears identically in both halves.
        let on: Vec<Vec2> = out
            .iter()
            .flatten()
            .copied()
            .filter(|p| (p.y - 0.77).abs() < 1e-6)
            .collect();
        for p in &on {
            assert!(on.iter().filter(|q| *q == p).count() >= 2, "{p:?}");
        }
    }

    /// A real sub-millimetre piece away from the origin is kept (found by
    /// the wall property test: dropping it left a hole in the face).
    #[test]
    fn small_pieces_away_from_the_origin_are_kept() {
        let piece = [
            Vec2::new(0.608_403_9, 0.65),
            Vec2::new(0.608_560_56, 0.649_883_15),
            Vec2::new(0.608_560_56, 0.65),
        ];
        assert!((area2(&piece) - 1.83e-8).abs() < 1e-10, "{}", area2(&piece));
        let mut out = Vec::new();
        fan(&piece, &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_hole_removes_its_area() {
        let outline = [
            Vec2::ZERO,
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 3.0),
            Vec2::new(0.0, 3.0),
        ];
        let hole = vec![
            Vec2::new(1.0, 1.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(1.0, 2.0),
        ];
        let tris = difference_tris(&outline, &[hole]);
        let area: f32 = tris
            .iter()
            .map(|t| 0.5 * (t[1] - t[0]).perp_dot(t[2] - t[0]).abs())
            .sum();
        assert!((area - 11.0).abs() < 1e-4, "{area}");
    }
}
