//! The `walls` rule: a pure function from every `WallPath` to one
//! `WallGen` part per wall, with the junction between walls written into
//! the end parameters (docs/PROCEDURAL_DIRECTION.md §7).
//!
//! | Situation | Result |
//! |---|---|
//! | Free end | `Cap` |
//! | Meets another wall's end, same thickness and height | `Mitre` on both (the shared joint line) |
//! | Meets another wall's end otherwise | Butt: the owner runs to the other's outer face and caps; the other is cut on the owner's inner face |
//! | Meets a wall's body (T) | Cut on that body's near face line; the body runs through |
//! | Three or more ends at one point | The longest pair joins as above; the others butt against the pair's faces; a warning when one still overlaps |
//! | Corner sharper than the mitre limit (29°) | Both capped, a bevel fill on the owner, a warning |
//!
//! **Ownership** is deterministic and uses only saved data: the taller wall
//! owns a butt junction (a lower owner would leave the taller wall's end
//! open above it, spike S6 rule J3), then the lower `Uid`.
//!
//! **Snapping.** Ends within [`SNAP`](crate::adjacency::SNAP) of each other meet at the position of
//! the first end in uid order; the others are moved there in their
//! `WallGen.points`. The threshold is fixed: hysteresis belongs to the tool
//! that writes the points ([`crate::snap`]), so the junctions are a pure
//! function of the saved scene and undo and reload reproduce them exactly.
//!
//! Everything is computed in world space (owners may be moved and rotated)
//! and written back owner-local. The rule's output does not depend on the
//! order of its input.

use box_engine::box_core::derived::PartKey;
use box_engine::box_proc::derive::Part;
use box_engine::box_proc::rule::{RuleInput, RuleOutput};
use box_engine::prelude::*;

use crate::adjacency::{
    Adjacency, BodyContact, End, EndId, EndLink, MIN_SEGMENT, PathShape, analyse, clean_polyline,
    flat, perp, snap_ends,
};
use crate::intent::WallPath;
use crate::wall_gen::{MITRE_LIMIT, WallEnd, WallGen, positive};

/// The rule's name (`Derived::rule`, warnings, the rule list).
pub const RULE: &str = "walls";
/// The part role: one part per wall, key `<uid>/wall`.
pub const ROLE: &str = "wall";
/// Thickness and height differences below this count as equal (metres).
const SAME: f32 = 1e-3;
/// Arms closer than this to a straight line continue in line.
const STRAIGHT_COS: f32 = -0.99985; // 179°
/// Arms closer than this fold back onto each other.
const FOLDED_COS: f32 = 0.99985; // 1°

/// The interior angle below which two walls cannot mitre (the sweep's
/// mitre limit: about 29° for a limit of 4).
pub fn min_mitre_angle() -> f32 {
    2.0 * (1.0 / MITRE_LIMIT).asin()
}

/// One wall as the rule reads it.
#[derive(Clone, Copy, Debug)]
pub struct WallIntent<'a> {
    pub uid: Uid,
    pub path: &'a WallPath,
    /// The owner's world transform.
    pub transform: Affine3A,
}

/// What the rule decided for one wall.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedWall {
    pub uid: Uid,
    /// `None` when the path cannot make a wall (fewer than two points).
    pub generator: Option<WallGen>,
    /// Shown on the owner.
    pub warnings: Vec<String>,
}

/// The material every wall part draws with.
pub fn wall_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::hex(0xe4d9c6),
        roughness: 0.85,
        ..Default::default()
    }
}

/// The `walls` rule, as `app.add_rule` takes it.
pub fn plan(input: &RuleInput<WallPath>) -> RuleOutput {
    let intents: Vec<WallIntent> = input
        .intents
        .iter()
        .map(|i| WallIntent {
            uid: i.uid,
            path: &i.value,
            transform: i.transform,
        })
        .collect();
    to_output(plan_walls(&intents))
}

/// Parts and warnings from planned walls.
pub fn to_output(walls: Vec<PlannedWall>) -> RuleOutput {
    let mut out = RuleOutput::default();
    for w in walls {
        for m in w.warnings {
            out.warn(w.uid, m);
        }
        if let Some(generator) = w.generator {
            out.push(
                Part::new(PartKey::new(w.uid, ROLE))
                    .with(generator)
                    .with(wall_material()),
            );
        }
    }
    out
}

/// Per-wall working state, world space.
struct Work {
    uid: Uid,
    transform: Affine3A,
    inverse: Affine3A,
    /// Cleaned owner-local points.
    local: Vec<Vec3>,
    height: f32,
    thickness: f32,
    /// Index into the shapes, when the wall is valid.
    shape: Option<usize>,
    ends: [WallEnd; 2],
    /// World-space junction point each end snaps to.
    snap: [Option<Vec3>; 2],
    /// Bevel triangles, world space.
    fills: Vec<Vec3>,
    warnings: Vec<String>,
}

fn at(p: Vec3) -> String {
    format!("({:.2}, {:.2})", p.x, p.z)
}

fn degrees(cos: f32) -> f32 {
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

/// The pure rule (see the module docs): one [`PlannedWall`] per intent, in
/// uid order.
pub fn plan_walls(intents: &[WallIntent]) -> Vec<PlannedWall> {
    let mut sorted: Vec<&WallIntent> = intents.iter().collect();
    sorted.sort_by_key(|i| i.uid);

    let mut work: Vec<Work> = Vec::with_capacity(sorted.len());
    let mut shapes: Vec<PathShape> = Vec::new();
    let mut owner_of: Vec<usize> = Vec::new(); // shape -> work index
    for i in &sorted {
        let mut warnings = Vec::new();
        let height = positive("height", i.path.height, 2.5, &mut warnings);
        let thickness = positive("thickness", i.path.thickness, 0.3, &mut warnings);
        let (local, dropped) = clean_polyline(&i.path.points);
        if dropped > 0 {
            warnings.push(format!(
                "{dropped} zero-length segment(s) or invalid point(s) ignored"
            ));
        }
        let finite = i.transform.is_finite();
        let shape = if local.len() >= 2 && finite {
            shapes.push(PathShape {
                uid: i.uid,
                points: local
                    .iter()
                    .map(|p| i.transform.transform_point3(*p))
                    .collect(),
                half_width: thickness * 0.5,
                height,
            });
            owner_of.push(work.len());
            Some(shapes.len() - 1)
        } else {
            warnings.push(if finite {
                "a wall needs at least two distinct points; nothing built".into()
            } else {
                "the wall's transform is not finite; nothing built".into()
            });
            None
        };
        work.push(Work {
            uid: i.uid,
            transform: i.transform,
            inverse: if finite {
                i.transform.inverse()
            } else {
                Affine3A::IDENTITY
            },
            local,
            height,
            thickness,
            shape,
            ends: [WallEnd::Cap; 2],
            snap: [None; 2],
            fills: Vec::new(),
            warnings,
        });
    }

    let adjacency = analyse(&shapes);
    let mut r = Resolver {
        shapes: &mut shapes,
        work: &mut work,
        owner_of: &owner_of,
        adjacency: &adjacency,
    };
    r.resolve();

    work.into_iter().map(finish).collect()
}

struct Resolver<'a> {
    shapes: &'a mut Vec<PathShape>,
    work: &'a mut Vec<Work>,
    owner_of: &'a [usize],
    adjacency: &'a Adjacency,
}

impl Resolver<'_> {
    fn w(&mut self, path: usize) -> &mut Work {
        &mut self.work[self.owner_of[path]]
    }

    fn thickness(&self, path: usize) -> f32 {
        self.work[self.owner_of[path]].thickness
    }

    fn height(&self, path: usize) -> f32 {
        self.work[self.owner_of[path]].height
    }

    fn uid(&self, path: usize) -> Uid {
        self.shapes[path].uid
    }

    fn set_end(&mut self, id: EndId, end: WallEnd) {
        self.w(id.path).ends[id.end.index()] = end;
    }

    fn warn(&mut self, path: usize, message: String) {
        let w = self.w(path);
        if !w.warnings.contains(&message) {
            w.warnings.push(message);
        }
    }

    fn resolve(&mut self) {
        let adj = self.adjacency;
        // Snap every node's ends to its point (in the shapes too, so arms
        // and neighbour directions use the snapped polylines).
        for node in &adj.nodes {
            for id in &node.ends {
                self.w(id.path).snap[id.end.index()] = Some(node.point);
            }
        }
        for path in 0..self.shapes.len() {
            let snap = self.work[self.owner_of[path]].snap;
            let s = &mut self.shapes[path];
            s.points = snap_ends(&s.points, snap[0], snap[1]);
        }
        // A wall shorter than the snap distance can collapse to a point.
        let collapsed: Vec<usize> = (0..self.shapes.len())
            .filter(|p| {
                let s = &self.shapes[*p];
                s.points.len() < 2 || s.points[0].distance(s.points[1]) < MIN_SEGMENT
            })
            .collect();
        for p in &collapsed {
            self.warn(
                *p,
                "the wall is shorter than the snap distance and collapses onto a junction; nothing built".into(),
            );
            self.w(*p).shape = None;
        }
        let alive = |id: &EndId| !collapsed.contains(&id.path);

        for node in &adj.nodes {
            let ends: Vec<EndId> = node.ends.iter().copied().filter(alive).collect();
            match ends.len() {
                0 => {}
                1 => self.set_end(ends[0], WallEnd::Cap),
                2 => self.pair(ends[0], ends[1], node.point),
                _ => self.many(&ends, node.point),
            }
        }
        for path in 0..self.shapes.len() {
            if collapsed.contains(&path) {
                continue;
            }
            for end in End::BOTH {
                let id = EndId { path, end };
                if let EndLink::Body(c) = adj.link(id) {
                    if collapsed.contains(&c.path) {
                        self.set_end(id, WallEnd::Cap);
                    } else {
                        self.body(id, c);
                    }
                }
            }
            self.interior_corners(path);
        }
        for c in &adj.crossings {
            let m = format!(
                "crosses another wall at {}; split one of them there to make a junction",
                at(c.point)
            );
            self.warn(c.a, m.clone());
            self.warn(c.b, m);
        }
        for o in &adj.overlaps {
            let m = format!(
                "runs along another wall for {:.2} m (overlapping faces); move or shorten one",
                o.length
            );
            self.warn(o.a, m.clone());
            self.warn(o.b, m);
        }
    }

    /// The owner of a junction between two walls: the taller, then the
    /// lower uid.
    fn owner_first(&self, a: EndId, b: EndId) -> (EndId, EndId) {
        let (ha, hb) = (self.height(a.path), self.height(b.path));
        if (ha - hb).abs() > SAME {
            if ha > hb { (a, b) } else { (b, a) }
        } else if (self.uid(a.path), a.end) <= (self.uid(b.path), b.end) {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn same_section(&self, a: usize, b: usize) -> bool {
        (self.thickness(a) - self.thickness(b)).abs() <= SAME
            && (self.height(a) - self.height(b)).abs() <= SAME
    }

    /// The mitre neighbour direction for `a` (see `JointEnd::Mitre`): at
    /// its end, the neighbour's segment leaving the point; at its start,
    /// the segment arriving at it.
    fn mitre_dir(&self, a: EndId, b: EndId, p: Vec3) -> Vec3 {
        let q = self.shapes[b.path].adjacent(b.end);
        match a.end {
            End::End => q - p,
            End::Start => p - q,
        }
    }

    /// Two ends meeting at `p`.
    fn pair(&mut self, a: EndId, b: EndId, p: Vec3) {
        let ua = self.shapes[a.path].arm(a.end);
        let ub = self.shapes[b.path].arm(b.end);
        let cos = ua.dot(ub);
        let acute_cos = 1.0 - 2.0 / (MITRE_LIMIT * MITRE_LIMIT);
        let same = self.same_section(a.path, b.path);
        if cos < STRAIGHT_COS {
            // In line.
            if same {
                self.mitre(a, b, p);
            } else {
                let (o, s) = self.dominant(a, b);
                self.set_end(o, WallEnd::Cap);
                self.set_end(s, WallEnd::Open);
            }
        } else if cos > FOLDED_COS {
            self.set_end(a, WallEnd::Cap);
            self.set_end(b, WallEnd::Cap);
            let m = format!(
                "doubles back over another wall at {}; both ends left capped",
                at(p)
            );
            self.warn(a.path, m.clone());
            self.warn(b.path, m);
        } else if cos > acute_cos {
            self.bevel(a, b, p, cos);
        } else if same {
            self.mitre(a, b, p);
        } else {
            self.butt(a, b, p);
        }
    }

    fn mitre(&mut self, a: EndId, b: EndId, p: Vec3) {
        let da = self.mitre_dir(a, b, p);
        let db = self.mitre_dir(b, a, p);
        self.set_end(a, WallEnd::Mitre { neighbour_dir: da });
        self.set_end(b, WallEnd::Mitre { neighbour_dir: db });
    }

    /// For walls continuing in line with different sections: the larger
    /// one (taller, then thicker, then lower uid) caps, the other is open
    /// against its cap.
    fn dominant(&self, a: EndId, b: EndId) -> (EndId, EndId) {
        let (ha, hb) = (self.height(a.path), self.height(b.path));
        let (ta, tb) = (self.thickness(a.path), self.thickness(b.path));
        let a_first = if (ha - hb).abs() > SAME {
            ha > hb
        } else if (ta - tb).abs() > SAME {
            ta > tb
        } else {
            (self.uid(a.path), a.end) <= (self.uid(b.path), b.end)
        };
        if a_first { (a, b) } else { (b, a) }
    }

    /// Owner runs through to the subordinate's outer face and caps; the
    /// subordinate is cut on the owner's inner face without a cap (S6 J2).
    fn butt(&mut self, a: EndId, b: EndId, p: Vec3) {
        let (o, s) = self.owner_first(a, b);
        let uo = self.shapes[o.path].arm(o.end);
        let us = self.shapes[s.path].arm(s.end);
        let (to, ts) = (self.thickness(o.path), self.thickness(s.path));
        // The subordinate's face away from the owner's body.
        let ns = perp(us);
        let ns = if ns.dot(uo) > 0.0 { -ns } else { ns };
        self.set_end(
            o,
            WallEnd::Butt {
                point: p + ns * (ts * 0.5),
                normal: us.cross(Vec3::Y),
                cap: true,
            },
        );
        // The owner's face toward the subordinate's body.
        let no = perp(uo);
        let no = if no.dot(us) < 0.0 { -no } else { no };
        self.set_end(
            s,
            WallEnd::Butt {
                point: p + no * (to * 0.5),
                normal: uo.cross(Vec3::Y),
                cap: false,
            },
        );
    }

    /// Both capped at the point, the outside notch filled by a triangle on
    /// the owner, a warning on both.
    fn bevel(&mut self, a: EndId, b: EndId, p: Vec3, cos: f32) {
        let (o, s) = self.owner_first(a, b);
        self.set_end(o, WallEnd::Cap);
        self.set_end(s, WallEnd::Cap);
        let uo = self.shapes[o.path].arm(o.end);
        let us = self.shapes[s.path].arm(s.end);
        let outer = |u: Vec3, other: Vec3, half: f32| {
            let n = perp(u);
            p + if n.dot(other) > 0.0 { -n } else { n } * half
        };
        let co = outer(uo, us, self.thickness(o.path) * 0.5);
        let cs = outer(us, uo, self.thickness(s.path) * 0.5);
        self.w(o.path).fills.extend([p, co, cs]);
        let m = format!(
            "corner at {} is sharper than the mitre limit ({:.0}° < {:.0}°); bevelled",
            at(p),
            degrees(cos),
            min_mitre_angle().to_degrees()
        );
        self.warn(o.path, m.clone());
        self.warn(s.path, m);
    }

    /// Three or more ends: the longest pair joins as two ends; every other
    /// end butts against the face of the pair arm it is most perpendicular
    /// to (capped, since the pair's own joint may leave part of the cut
    /// uncovered), with a warning when it still overlaps another wall.
    fn many(&mut self, ends: &[EndId], p: Vec3) {
        let mut by_length: Vec<EndId> = ends.to_vec();
        by_length.sort_by(|x, y| {
            let (lx, ly) = (self.shapes[x.path].length(), self.shapes[y.path].length());
            ly.total_cmp(&lx)
                .then((self.uid(x.path), x.end).cmp(&(self.uid(y.path), y.end)))
        });
        let (a, b) = (by_length[0], by_length[1]);
        self.pair(a, b, p);
        let limit = min_mitre_angle().sin();
        for &e in &by_length[2..] {
            let ue = self.shapes[e.path].arm(e.end);
            let (x, sin) = [a, b]
                .into_iter()
                .map(|x| (x, ue.cross(self.shapes[x.path].arm(x.end)).length()))
                .fold(None::<(EndId, f32)>, |best, c| match best {
                    Some(b) if b.1 >= c.1 => Some(b),
                    _ => Some(c),
                })
                .expect("a pair");
            let unresolved = format!(
                "{} walls meet at {}: this wall overlaps another (unresolved); give it its own junction",
                ends.len(),
                at(p)
            );
            if sin < limit {
                self.set_end(e, WallEnd::Cap);
                self.warn(e.path, unresolved);
                continue;
            }
            let ux = self.shapes[x.path].arm(x.end);
            let tx = self.thickness(x.path);
            let n = perp(ux);
            let n = if n.dot(ue) < 0.0 { -n } else { n };
            let point = p + n * (tx * 0.5);
            let normal = ux.cross(Vec3::Y);
            self.set_end(
                e,
                WallEnd::Butt {
                    point,
                    normal,
                    cap: true,
                },
            );
            // Do the cut corners of `e` land inside another wall at the node?
            let te = self.thickness(e.path);
            let corners = [-0.5f32, 0.5].map(|side| {
                let c = p + perp(ue) * (te * side);
                let t = (point - c).dot(normal) / ue.dot(normal);
                c + ue * t
            });
            let overlapped = ends.iter().filter(|y| **y != e && **y != x).any(|y| {
                let s = &self.shapes[y.path];
                let uy = s.arm(y.end);
                let reach = s.adjacent(y.end).distance(p);
                let half = self.thickness(y.path) * 0.5;
                corners.iter().any(|c| {
                    let d = flat(*c - p);
                    let along = d.dot(uy);
                    along > 1e-3 && along < reach && d.dot(perp(uy)).abs() < half - 1e-3
                })
            });
            if overlapped {
                self.warn(e.path, unresolved);
            }
        }
    }

    /// An end touching another wall's body: cut on that body's near face.
    fn body(&mut self, id: EndId, c: BodyContact) {
        let ue = self.shapes[id.path].arm(id.end);
        let (b0, b1) = (
            self.shapes[c.path].points[c.segment],
            self.shapes[c.path].points[c.segment + 1],
        );
        let db = flat(b1 - b0).normalize_or(Vec3::X);
        let sin = ue.cross(db).length();
        let p = self.shapes[id.path].end_point(id.end);
        if sin < min_mitre_angle().sin() {
            self.set_end(id, WallEnd::Cap);
            self.warn(
                id.path,
                format!(
                    "meets another wall's side at {} at a sharp angle ({:.0}°); left capped",
                    at(p),
                    sin.asin().to_degrees()
                ),
            );
            return;
        }
        let n = perp(db);
        let n = if n.dot(ue) < 0.0 { -n } else { n };
        let tb = self.thickness(c.path);
        // Covered by the body's face, unless taller than the body or at one
        // of its corners or ends (the face may stop short of the cut).
        let body = &self.shapes[c.path];
        let near_vertex = c.at_vertex
            || body
                .points
                .iter()
                .any(|v| flat(*v - c.point).length() < self.thickness(id.path) * 0.5 + 1e-3);
        let cap = self.height(id.path) > self.height(c.path) + SAME || near_vertex;
        self.set_end(
            id,
            WallEnd::Butt {
                point: c.point + n * (tb * 0.5),
                normal: db.cross(Vec3::Y),
                cap,
            },
        );
    }

    /// Warns about corners inside one wall that the sweep will bevel.
    fn interior_corners(&mut self, path: usize) {
        let pts = self.shapes[path].points.clone();
        let limit = 1.0 / MITRE_LIMIT;
        for k in 1..pts.len().saturating_sub(1) {
            let din = (pts[k] - pts[k - 1]).normalize_or_zero();
            let dout = (pts[k + 1] - pts[k]).normalize_or_zero();
            let cos_half = ((1.0 + din.dot(dout)) * 0.5).max(0.0).sqrt();
            if cos_half < limit {
                self.warn(
                    path,
                    format!(
                        "corner at {} is sharper than the mitre limit ({:.0}° < {:.0}°); bevelled",
                        at(pts[k]),
                        degrees(-din.dot(dout)),
                        min_mitre_angle().to_degrees()
                    ),
                );
            }
        }
    }
}

/// World-space decisions back to the owner's space.
fn finish(w: Work) -> PlannedWall {
    let Some(_) = w.shape else {
        return PlannedWall {
            uid: w.uid,
            generator: None,
            warnings: w.warnings,
        };
    };
    let inv = w.inverse;
    // Normals go back with the transpose of the forward linear part.
    let normal_back = w.transform.matrix3.transpose();
    let back = |e: WallEnd| match e {
        WallEnd::Cap | WallEnd::Open => e,
        WallEnd::Mitre { neighbour_dir } => WallEnd::Mitre {
            neighbour_dir: inv.transform_vector3(neighbour_dir),
        },
        WallEnd::Butt { point, normal, cap } => WallEnd::Butt {
            point: inv.transform_point3(point),
            normal: Vec3::from(normal_back * Vec3A::from(normal)).normalize_or(normal),
            cap,
        },
    };
    let points = snap_ends(
        &w.local,
        w.snap[0].map(|p| inv.transform_point3(p)),
        w.snap[1].map(|p| inv.transform_point3(p)),
    );
    PlannedWall {
        uid: w.uid,
        generator: Some(WallGen {
            points,
            height: w.height,
            thickness: w.thickness,
            start: back(w.ends[0]),
            end: back(w.ends[1]),
            fills: w.fills.iter().map(|p| inv.transform_point3(*p)).collect(),
        }),
        warnings: w.warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall(points: &[[f32; 2]]) -> WallPath {
        WallPath {
            points: points.iter().map(|p| Vec3::new(p[0], 0.0, p[1])).collect(),
            ..WallPath::default()
        }
    }

    fn run(walls: &[(u128, WallPath)]) -> Vec<PlannedWall> {
        let intents: Vec<WallIntent> = walls
            .iter()
            .map(|(id, p)| WallIntent {
                uid: Uid::from_u128(*id),
                path: p,
                transform: Affine3A::IDENTITY,
            })
            .collect();
        plan_walls(&intents)
    }

    fn kinds(w: &PlannedWall) -> [&'static str; 2] {
        let g = w.generator.as_ref().unwrap();
        [g.start.kind(), g.end.kind()]
    }

    #[test]
    fn free_ends_cap_and_touching_ends_mitre() {
        let out = run(&[(1, wall(&[[0.0, 0.0], [4.0, 0.0]]))]);
        assert_eq!(kinds(&out[0]), ["cap", "cap"]);
        let out = run(&[
            (1, wall(&[[0.0, 0.0], [4.0, 0.0]])),
            (2, wall(&[[4.0, 0.0], [4.0, -3.0]])),
        ]);
        assert_eq!(kinds(&out[0]), ["cap", "mitre"]);
        assert_eq!(kinds(&out[1]), ["mitre", "cap"]);
        assert!(out.iter().all(|w| w.warnings.is_empty()));
    }

    #[test]
    fn different_thickness_butts_with_the_lower_uid_owning() {
        let mut b = wall(&[[4.0, 0.0], [4.0, -3.0]]);
        b.thickness = 0.2;
        let out = run(&[(1, wall(&[[0.0, 0.0], [4.0, 0.0]])), (2, b)]);
        assert_eq!(kinds(&out[0]), ["cap", "butt-owner"]);
        assert_eq!(kinds(&out[1]), ["butt", "cap"]);
    }
}
