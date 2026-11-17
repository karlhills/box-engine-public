//! Sweep: a 2D [`Profile`] carried along a [`Path`].
//!
//! This is the spike S6 wall model, generalised to 3D paths:
//!
//! - **Joints are planes.** Every profile point travels along an offset line
//!   (parallel to the path inside a straight segment). At a polyline corner
//!   all offset lines are cut by the corner's **mitre plane** (the bisector
//!   of the two directions), so both segments share bit-identical vertices
//!   and every layer of a wall meets cleanly at any angle (rule J1). Inside
//!   a segment the mapping stays perpendicular, so U (arc length) is not
//!   skewed near corners.
//! - **The ends are parameters** ([`JointEnd`]): a square end with or
//!   without a cap, an extension, a mitre with a neighbouring path (both
//!   paths compute the same plane from the same two directions), or a butt
//!   against any plane (a neighbour's face). This is how a rule system
//!   connects separate walls (rules J2, J3).
//! - Corners sharper than `miter_limit` are **bevelled**: both segments end
//!   square and a bridge closes the outside of the turn (with a warning).
//! - Smooth stations (sampled splines) get perpendicular sections and
//!   smooth normals along the path. Tight curvature only warns.
//!
//! UVs: U = arc length along the path (metres, perpendicular mapping), V =
//! length along the profile (metres), both times `uv_scale`. Caps use the
//! profile's 2D coordinates. Profile edges carry material slots; caps use
//! `cap_slot`.

use box_core::pmath;
use std::collections::HashMap;

use glam::{Vec2, Vec3};

use crate::mesh_builder::{BuiltMesh, MeshBuilder, Vertex, ring_columns};
use crate::poly::{self, Polygon};
use crate::spline::{Frame, Path};

/// A 2D cross-section: x across the path (positive = right of travel), y up.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub points: Vec<Vec2>,
    /// Closed profiles make solids (and can be capped); open ones make
    /// surfaces (roads, mouldings). An open profile's surface faces the
    /// right of its direction of travel in (x, y).
    pub closed: bool,
    /// Material slot of edge `i` (from point `i` to `i + 1`, the last edge of
    /// a closed profile closing the loop). Missing entries use slot 0.
    pub edge_slots: Vec<u32>,
    /// Profile vertices where the edges turn by at most this many radians
    /// (and share a slot) are smoothed.
    pub smooth_angle: f32,
}

impl Default for Profile {
    fn default() -> Self {
        Self::rect(0.3, 3.0)
    }
}

impl Profile {
    pub fn new(points: Vec<Vec2>, closed: bool) -> Self {
        Self {
            points,
            closed,
            edge_slots: Vec::new(),
            smooth_angle: 0.5,
        }
    }

    /// A rectangle centred on the path, standing on it: x in ±width/2, y in
    /// 0..height. Edges: 0 bottom, 1 right, 2 top, 3 left. The wall profile.
    pub fn rect(width: f32, height: f32) -> Self {
        let w = width * 0.5;
        Self::new(
            vec![
                Vec2::new(-w, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, height),
                Vec2::new(-w, height),
            ],
            true,
        )
    }

    /// A circle centred on the path (`segments` clamped to 3..=1024),
    /// always smooth-shaded. Tubes.
    pub fn circle(radius: f32, segments: u32) -> Self {
        let n = segments.clamp(3, 1024);
        Self::new(
            (0..n)
                .map(|i| Vec2::from_angle(std::f32::consts::TAU * i as f32 / n as f32) * radius)
                .collect(),
            true,
        )
        .with_smooth_angle(std::f32::consts::TAU / n as f32 * 1.01)
    }

    pub fn with_slots(mut self, edge_slots: Vec<u32>) -> Self {
        self.edge_slots = edge_slots;
        self
    }

    pub fn with_smooth_angle(mut self, radians: f32) -> Self {
        self.smooth_angle = radians;
        self
    }

    /// Sanitized points and per-edge slots: non-finite points dropped,
    /// duplicates merged, closed profiles made counter-clockwise.
    pub(crate) fn cleaned(&self) -> (Vec<Vec2>, Vec<u32>, bool) {
        let slot = |i: usize| self.edge_slots.get(i).copied().unwrap_or(0);
        let mut pts: Vec<Vec2> = Vec::new();
        let mut slots: Vec<u32> = Vec::new();
        for (i, &p) in self.points.iter().enumerate() {
            if !p.is_finite() {
                continue;
            }
            if pts.last() == Some(&p) {
                // The zero-length edge vanishes; the merged point leaves by edge i.
                if let Some(s) = slots.last_mut() {
                    *s = slot(i);
                }
                continue;
            }
            pts.push(p);
            slots.push(slot(i));
        }
        let mut closed = self.closed;
        if closed && pts.len() > 1 && pts[0] == pts[pts.len() - 1] {
            pts.pop();
            slots.pop();
        }
        if pts.len() < 3 {
            closed = false;
        }
        if closed && poly::signed_area(&pts) < 0.0 {
            let n = pts.len();
            let rs: Vec<u32> = (0..n)
                .map(|i| {
                    if i + 1 < n {
                        slots[n - 2 - i]
                    } else {
                        slots[n - 1]
                    }
                })
                .collect();
            pts.reverse();
            slots = rs;
        }
        (pts, slots, closed)
    }
}

/// A cutting plane for a sweep end. Only the plane matters: the normal's
/// sign is irrelevant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointPlane {
    pub point: Vec3,
    pub normal: Vec3,
}

impl JointPlane {
    pub fn new(point: Vec3, normal: Vec3) -> Self {
        Self { point, normal }
    }

    /// The mitre (bisector) plane at `point` between a path arriving along
    /// `incoming` and one leaving along `outgoing`. Two paths that meet
    /// compute the identical plane when they pass the same two directions
    /// in the same order (see [`JointEnd::Mitre`]).
    pub fn mitre(point: Vec3, incoming: Vec3, outgoing: Vec3) -> Self {
        let (a, b) = (incoming.normalize_or_zero(), outgoing.normalize_or_zero());
        Self {
            point,
            normal: (a + b).try_normalize().unwrap_or(a),
        }
    }

    /// The plane through a line (`point`, `direction`) that contains `up`:
    /// e.g. a neighbouring wall's face line, for [`JointEnd::Butt`].
    pub fn through_line(point: Vec3, direction: Vec3, up: Vec3) -> Self {
        Self {
            point,
            normal: direction.cross(up).normalize_or_zero(),
        }
    }

    fn is_valid(&self) -> bool {
        self.point.is_finite() && self.normal.is_finite() && self.normal.length_squared() > 1e-12
    }
}

/// How one end of an open sweep finishes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum JointEnd {
    /// Square end with a cap. A free wall end.
    Cap,
    /// Square end, no cap: something continues in line.
    Open,
    /// Runs on `distance` metres past the end (into a solid such as a tower,
    /// rule J10), square, optionally capped.
    Extend { distance: f32, cap: bool },
    /// Mitre with a neighbouring path that meets this end, no cap.
    /// `neighbour_dir` is the neighbour's segment at the shared point *in
    /// this path's traversal sense*, as a raw vector: at this path's end,
    /// the segment the neighbour continues along (`n[1] - n[0]`); at the
    /// start, the segment the neighbour arrives along (`n[k] - n[k-1]`).
    /// Both paths cut the arriving path's offset lines with the same mitre
    /// plane, so with `Orientation::Up`, equal profiles and the same shared
    /// point the two ends have **bit-identical** vertices
    /// ([`Path::neighbour_frame`]). Falls back to a capped square end beyond
    /// the mitre limit.
    Mitre { neighbour_dir: Vec3 },
    /// Cut by an arbitrary plane (a neighbour's face line), optionally
    /// capped. The owner of a butt junction is capped; the subordinate is
    /// cut on the owner's face without a cap (rule J2).
    Butt { plane: JointPlane, cap: bool },
}

/// Sweep parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepParams {
    pub profile: Profile,
    pub start: JointEnd,
    pub end: JointEnd,
    /// Corners whose mitre would stretch the profile by more than this
    /// factor (1 / sin(half the interior angle); 4.0 ≈ 29°) are bevelled.
    pub miter_limit: f32,
    pub cap_slot: u32,
    /// Multiplies (U, V).
    pub uv_scale: Vec2,
}

impl Default for SweepParams {
    fn default() -> Self {
        Self {
            profile: Profile::default(),
            start: JointEnd::Cap,
            end: JointEnd::Cap,
            miter_limit: 4.0,
            cap_slot: 0,
            uv_scale: Vec2::ONE,
        }
    }
}

/// Sweeps `params.profile` along `path` into a new mesh.
pub fn sweep(path: &Path, params: &SweepParams) -> BuiltMesh {
    let mut mb = MeshBuilder::new();
    sweep_into(&mut mb, path, params);
    mb.build()
}

/// One ring of vertex positions at a station, with the frame used for
/// normals and U.
struct Ring {
    positions: Vec<Vec3>,
    frame: Frame,
    /// Station distance (U origin) and the station point.
    distance: f32,
    origin: Vec3,
}

enum Cut {
    Square,
    Plane(JointPlane),
}

fn ring(frame: &Frame, pts: &[Vec2], shift: f32, cut: &Cut, warn: &mut Vec<String>) -> Vec<Vec3> {
    let f = frame.forward;
    pts.iter()
        .map(|p| {
            let o = frame.point(*p) + f * shift;
            match cut {
                Cut::Square => o,
                Cut::Plane(pl) => {
                    let denom = f.dot(pl.normal);
                    if denom.abs() < 1e-4 {
                        warn.push("joint plane is parallel to the path; end left square".into());
                        o
                    } else {
                        o + f * ((pl.point - o).dot(pl.normal) / denom)
                    }
                }
            }
        })
        .collect()
}

/// Sweeps into an existing builder (warnings go to the builder).
pub fn sweep_into(mb: &mut MeshBuilder, path: &Path, params: &SweepParams) {
    let (pts, slots, closed_profile) = params.profile.cleaned();
    if pts.len() < 2 {
        mb.warn("sweep: profile needs at least two distinct points");
        return;
    }
    if path.is_empty() {
        mb.warn("sweep: path needs at least two distinct points");
        return;
    }
    let smooth = if params.profile.smooth_angle.is_finite() {
        params.profile.smooth_angle
    } else {
        0.0
    };
    let cols = ring_columns(&pts, closed_profile, smooth, &slots);
    let uv_scale = if params.uv_scale.is_finite() {
        params.uv_scale
    } else {
        Vec2::ONE
    };
    let miter_limit = if params.miter_limit.is_finite() {
        params.miter_limit.max(1.0)
    } else {
        4.0
    };
    let min_cos_half = 1.0 / miter_limit;
    let st = &path.stations;
    let n = st.len();
    let segs = path.segment_count();
    let mut warnings: Vec<String> = Vec::new();
    let reach = pts.iter().map(|p| p.length()).fold(0.0f32, f32::max);

    // Cap triangulation (profile indices), shared by both ends.
    let cap_tris: Vec<[usize; 3]> = if closed_profile {
        let t = poly::triangulate(&Polygon::new(pts.clone()));
        let index: HashMap<(u32, u32), usize> = pts
            .iter()
            .enumerate()
            .map(|(i, p)| ((p.x.to_bits(), p.y.to_bits()), i))
            .collect();
        t.indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter_map(|c| {
                let m = |k: u32| {
                    let q = t.points[k as usize];
                    index.get(&(q.x.to_bits(), q.y.to_bits())).copied()
                };
                Some([m(c[0])?, m(c[1])?, m(c[2])?])
            })
            .collect()
    } else {
        Vec::new()
    };

    // How an open end is cut: (cut, shift along the path, cap?, the frame
    // whose offset lines are cut when it is not the end's own).
    let end_cut = |end: &JointEnd,
                   frame: &Frame,
                   at_start: bool,
                   warnings: &mut Vec<String>|
     -> (Cut, f32, bool, Option<Frame>) {
        match *end {
            JointEnd::Cap => (Cut::Square, 0.0, true, None),
            JointEnd::Open => (Cut::Square, 0.0, false, None),
            JointEnd::Extend { distance, cap } => {
                let d = if distance.is_finite() { distance } else { 0.0 };
                (Cut::Square, if at_start { -d } else { d }, cap, None)
            }
            JointEnd::Mitre { neighbour_dir } => match path.neighbour_frame(frame, neighbour_dir) {
                None => {
                    warnings.push("mitre end without a neighbour direction; capped square".into());
                    (Cut::Square, 0.0, true, None)
                }
                Some(nf) => {
                    // The arriving path's offset lines are cut, by both paths alike.
                    let (fin, fout) = if at_start { (nf, *frame) } else { (*frame, nf) };
                    let cos_half = ((1.0 + fin.forward.dot(fout.forward)) * 0.5)
                        .max(0.0)
                        .sqrt();
                    if cos_half < min_cos_half {
                        warnings
                            .push("mitre end sharper than the mitre limit; capped square".into());
                        (Cut::Square, 0.0, true, None)
                    } else {
                        let pl = JointPlane::mitre(frame.position, fin.forward, fout.forward);
                        (Cut::Plane(pl), 0.0, false, Some(fin))
                    }
                }
            },
            JointEnd::Butt { plane, cap } => {
                if plane.is_valid() {
                    (Cut::Plane(plane), 0.0, cap, None)
                } else {
                    warnings.push("butt end with an invalid plane; left square".into());
                    (Cut::Square, 0.0, cap, None)
                }
            }
        }
    };

    let mut incoming: Vec<Option<Ring>> = (0..n).map(|_| None).collect();
    let mut outgoing: Vec<Option<Ring>> = (0..n).map(|_| None).collect();
    let mut bevel = vec![false; n];
    // (start?, ring positions, cap normal)
    let mut caps: Vec<(bool, Vec<Vec3>, Vec3)> = Vec::new();
    for k in 0..n {
        let s = &st[k];
        let first = k == 0 && !path.closed;
        let last = k == n - 1 && !path.closed;
        let mk = |frame: &Frame, positions: Vec<Vec3>| Ring {
            positions,
            frame: *frame,
            distance: frame.distance,
            origin: frame.position,
        };
        if first || last {
            let frame = if first { &s.outgoing } else { &s.incoming };
            let end = if first { &params.start } else { &params.end };
            let (cut, shift, cap, line_frame) = end_cut(end, frame, first, &mut warnings);
            let normal = match &cut {
                Cut::Plane(pl) => pl.normal,
                Cut::Square => frame.forward,
            };
            let positions = ring(
                line_frame.as_ref().unwrap_or(frame),
                &pts,
                shift,
                &cut,
                &mut warnings,
            );
            if cap && closed_profile {
                let facing = if first { -frame.forward } else { frame.forward };
                let n = if normal.dot(facing) < 0.0 {
                    -normal
                } else {
                    normal
                }
                .normalize_or(facing);
                caps.push((first, positions.clone(), n));
            }
            // A cut deeper than the neighbouring station folds the geometry.
            let other = if first {
                st.get(1).map(|s| s.incoming.position)
            } else {
                st.get(n.saturating_sub(2)).map(|s| s.outgoing.position)
            };
            if let Some(o) = other {
                let span = (o - frame.position).dot(frame.forward).abs();
                if positions.iter().any(|q| {
                    (*q - frame.position).dot(frame.forward).abs() > span + reach * 1e-3
                        && ((*q - frame.position).dot(frame.forward)
                            * (o - frame.position).dot(frame.forward))
                            > 0.0
                }) {
                    warnings.push(
                        "end joint cuts past the neighbouring station; geometry may fold".into(),
                    );
                }
            }
            let r = mk(frame, positions);
            if first {
                outgoing[k] = Some(r);
            } else {
                incoming[k] = Some(r);
            }
            continue;
        }
        if !s.is_corner() {
            let p = ring(&s.outgoing, &pts, 0.0, &Cut::Square, &mut warnings);
            incoming[k] = Some(mk(&s.incoming, p.clone()));
            outgoing[k] = Some(mk(&s.outgoing, p));
            continue;
        }
        let (din, dout) = (s.incoming.forward, s.outgoing.forward);
        let cos_half = ((1.0 + din.dot(dout)) * 0.5).max(0.0).sqrt();
        if cos_half < min_cos_half {
            bevel[k] = true;
            warnings.push(format!(
                "corner at {:.2} m is sharper than the mitre limit; bevelled",
                s.distance()
            ));
            incoming[k] = Some(mk(
                &s.incoming,
                ring(&s.incoming, &pts, 0.0, &Cut::Square, &mut warnings),
            ));
            outgoing[k] = Some(mk(
                &s.outgoing,
                ring(&s.outgoing, &pts, 0.0, &Cut::Square, &mut warnings),
            ));
        } else {
            let pl = JointPlane::mitre(s.position(), din, dout);
            // Computed once from the incoming segment; the outgoing one shares it.
            let p = ring(&s.incoming, &pts, 0.0, &Cut::Plane(pl), &mut warnings);
            incoming[k] = Some(mk(&s.incoming, p.clone()));
            outgoing[k] = Some(mk(&s.outgoing, p));
        }
    }
    // A closed path's last segment arrives at station 0 with U = length.
    if path.closed
        && let Some(r) = incoming[0].as_mut()
    {
        r.distance = path.length;
    }

    // Tight curvature on smooth stretches.
    for k in 1..n {
        let (a, b) = (&st[k - 1].outgoing, &st[k].incoming);
        let angle = pmath::acos(a.forward.dot(b.forward).clamp(-1.0, 1.0));
        let ds = a.position.distance(b.position);
        if !st[k].is_corner() && angle > 1e-4 && ds / angle < reach * a.scale.max_element() {
            warnings
                .push("path bends tighter than the profile; the inside of the turn folds".into());
            break;
        }
    }

    let vertex_ring = |mb: &mut MeshBuilder, r: &Ring| -> Vec<u32> {
        cols.columns
            .iter()
            .map(|c| {
                let pos = r.positions[c.point];
                let u = r.distance + (pos - r.origin).dot(r.frame.forward);
                mb.vertex(
                    c.slot,
                    Vertex::new(
                        pos,
                        r.frame.normal(c.normal),
                        Vec2::new(u, c.along) * uv_scale,
                    ),
                )
            })
            .collect()
    };
    let connect = |mb: &mut MeshBuilder, a: &[u32], b: &[u32]| {
        for (e, &[ca, cb]) in cols.edges.iter().enumerate() {
            let s = cols.slots[e];
            // Collapsed quads (bevel bridges, points on a cut line) are skipped.
            mb.try_triangle_indices(s, [b[ca], b[cb], a[cb]]);
            mb.try_triangle_indices(s, [b[ca], a[cb], a[ca]]);
        }
    };

    let mut prev: Option<Vec<u32>> = None;
    for k in 0..segs {
        let k1 = (k + 1) % n;
        let (Some(ra), Some(rb)) = (outgoing[k].as_ref(), incoming[k1].as_ref()) else {
            continue;
        };
        let a_ids = match prev.take() {
            Some(ids) => ids,
            None => vertex_ring(mb, ra),
        };
        let b_ids = vertex_ring(mb, rb);
        connect(mb, &a_ids, &b_ids);
        // Smooth stations share the ring with the next segment.
        let next_smooth = k1 != 0 && !st[k1].is_corner() && outgoing[k1].is_some();
        prev = next_smooth.then_some(b_ids);
    }
    // Bevel bridges: a short segment from the incoming to the outgoing ring.
    for k in 0..n {
        if !bevel[k] {
            continue;
        }
        if let (Some(a), Some(b)) = (incoming[k].as_ref(), outgoing[k].as_ref()) {
            let (a_ids, b_ids) = (
                vertex_ring_flat(mb, a, &cols, uv_scale),
                vertex_ring_flat(mb, b, &cols, uv_scale),
            );
            connect(mb, &a_ids, &b_ids);
        }
    }
    // Caps.
    for (first, positions, facing) in caps {
        let ids: Vec<u32> = pts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // Mirrored at the start so the texture reads correctly from outside.
                let uv = Vec2::new(if first { -p.x } else { p.x }, p.y) * uv_scale;
                mb.vertex(params.cap_slot, Vertex::new(positions[i], facing, uv))
            })
            .collect();
        for t in &cap_tris {
            // CCW profile triangles face −forward (the start cap's side).
            let tri = if first {
                [t[0], t[1], t[2]]
            } else {
                [t[0], t[2], t[1]]
            };
            mb.triangle_indices(params.cap_slot, tri.map(|i| ids[i]));
        }
    }
    for w in warnings {
        mb.warn(w);
    }
}

/// Ring vertices with computed (flat) normals, for bevel bridges.
fn vertex_ring_flat(
    mb: &mut MeshBuilder,
    r: &Ring,
    cols: &crate::mesh_builder::Columns,
    uv_scale: Vec2,
) -> Vec<u32> {
    cols.columns
        .iter()
        .map(|c| {
            let pos = r.positions[c.point];
            let u = r.distance + (pos - r.origin).dot(r.frame.forward);
            mb.vertex(c.slot, Vertex::at(pos, Vec2::new(u, c.along) * uv_scale))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_builder::{edge_report, inward_triangles, signed_volume};
    use crate::spline::Orientation;

    fn wall_path(points: &[Vec3], closed: bool) -> Path {
        Path::from_polyline(points, closed, Orientation::Up(Vec3::Y))
    }

    #[test]
    fn straight_wall_is_a_closed_box() {
        let path = wall_path(&[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)], false);
        let b = sweep(&path, &SweepParams::default());
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        let m = &b.mesh;
        assert_eq!(m.triangle_count(), 12);
        assert!(edge_report(m, 1e-5).is_closed_manifold());
        assert!((signed_volume(m) - 4.0 * 0.3 * 3.0).abs() < 1e-4);
        assert_eq!(inward_triangles(m), 0);
        let bb = m.bounds();
        assert!((bb.min - Vec3::new(0.0, 0.0, -0.15)).length() < 1e-6);
        assert!((bb.max - Vec3::new(4.0, 3.0, 0.15)).length() < 1e-6);
    }

    #[test]
    fn mitred_l_shares_corner_vertices() {
        let path = wall_path(
            &[
                Vec3::ZERO,
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, -3.0),
            ],
            false,
        );
        let m = sweep(&path, &SweepParams::default()).mesh;
        assert!(edge_report(&m, 1e-6).is_closed_manifold());
        // Volume = profile area × centreline length (centred profile, mitred corner).
        assert!((signed_volume(&m) - 7.0 * 0.9).abs() < 1e-4);
        // Outer corner vertex at (4.15, y, 0.15).
        assert!(
            m.positions
                .iter()
                .any(|p| (*p - Vec3::new(4.15, 3.0, 0.15)).length() < 1e-6)
        );
    }
}
