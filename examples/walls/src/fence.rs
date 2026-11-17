//! Fences: the second user of the adjacency ([`crate::adjacency`]), with a
//! different resolution and a different generator. This is the generality
//! check of docs/PROCEDURAL_DIRECTION.md §7: the fence needed no engine
//! change; reacting to walls uses the M6 `rule_reads` (a rule reading a
//! second intent type).
//!
//! The `fences` rule reads every `FencePath`, and every `WallPath`
//! read-only (`app.rule_reads::<WallPath>("fences")`):
//!
//! | Situation | Result |
//! |---|---|
//! | Free end | An end post |
//! | Ends meeting (any number, any angle) | One shared corner post, built by the first fence in uid order; the others snap to it and skip theirs |
//! | End touching another fence's line (T) | Snapped onto that line, with its own post |
//! | Free end inside a wall's footprint (within its half thickness + 10 cm of the centreline) | Butts the wall: the rails run to the wall's face on the fence's side and the end gets no post |
//! | End running along a wall (within 11° of parallel) | Keeps its post; a warning |
//! | Crossing or overlapping fences | A warning on both |
//!
//! There are no mitres and no bevels: rails stop at post centres, so sharp
//! corners need no special case.

use box_engine::box_core::derived::PartKey;
use box_engine::box_proc::derive::Part;
use box_engine::box_proc::mesh_builder::MeshBuilder;
use box_engine::box_proc::rule::{RuleInput, RuleOutput};
use box_engine::box_proc::spline::{Orientation, Path};
use box_engine::box_proc::split::{Anchor, Remainder, divide_count, divide_size, place_bays};
use box_engine::box_proc::sweep::{JointEnd, Profile, SweepParams, sweep_into};
use box_engine::prelude::*;

use crate::adjacency::{
    End, EndLink, MIN_SEGMENT, PathShape, SNAP, analyse, clean_polyline, flat, perp, snap_ends,
};
use crate::intent::{FencePath, WallPath};
use crate::plan::WallIntent;
use crate::wall_gen::positive;

/// The rule's name.
pub const RULE: &str = "fences";
/// The part role: one part per fence, key `<uid>/fence`.
pub const ROLE: &str = "fence";
/// The most rails a fence builds.
pub const MAX_RAILS: u32 = 8;

/// One fence run as the `fences` rule derives it. Slots: posts and rails.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct FenceGen {
    /// Fence line (owner-local), ends snapped to their junction points.
    pub points: Vec<Vec3>,
    #[reflect(@Unit("m"))]
    pub height: f32,
    #[reflect(@Unit("m"))]
    pub post_spacing: f32,
    #[reflect(@Unit("m"))]
    pub post_size: f32,
    pub rail_count: u32,
    /// False when another fence builds the post at this end.
    pub start_post: bool,
    pub end_post: bool,
}

impl Default for FenceGen {
    fn default() -> Self {
        let p = FencePath::default();
        Self {
            points: p.points,
            height: p.height,
            post_spacing: p.post_spacing,
            post_size: p.post_size,
            rail_count: p.rail_count,
            start_post: true,
            end_post: true,
        }
    }
}

const POSTS: u32 = 0;
const RAILS: u32 = 1;

impl Generator for FenceGen {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Posts", "Rails"];
    const CATEGORY: &'static str = "Walls";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let mut warnings = Vec::new();
        let height = positive("height", self.height, 1.1, &mut warnings);
        let size = positive("post size", self.post_size, 0.14, &mut warnings);
        let mut spacing = positive("post spacing", self.post_spacing, 2.0, &mut warnings);
        if spacing < size * 2.0 {
            spacing = size * 2.0;
            warnings.push(format!(
                "post spacing is less than two posts wide; using {spacing:.2} m"
            ));
        }
        let rails = if self.rail_count > MAX_RAILS {
            warnings.push(format!(
                "{} rails is too many; building {MAX_RAILS}",
                self.rail_count
            ));
            MAX_RAILS
        } else {
            self.rail_count
        };
        let (pts, _) = clean_polyline(&self.points);
        if pts.len() < 2 {
            warnings.push("a fence needs at least two distinct points".into());
            return GenOutput {
                warnings,
                ..GenOutput::empty()
            };
        }

        let mut mb = MeshBuilder::new().with_tangents();
        let last = pts.len() - 1;
        for (k, p) in pts.iter().enumerate() {
            let skip = (k == 0 && !self.start_post) || (k == last && !self.end_post);
            if !skip {
                let dir = if k < last {
                    pts[k + 1] - *p
                } else {
                    *p - pts[k - 1]
                };
                post_into(&mut mb, *p, dir, size, height);
            }
        }
        let levels = divide_count(height * 0.65, rails);
        let rail_w = size * 0.35;
        let rail_h = size * 0.6;
        for seg in pts.windows(2) {
            let path = Path::from_polyline(seg, false, Orientation::Up(Vec3::Y));
            let bays = divide_size(path.length, spacing, Remainder::Stretch);
            for f in place_bays(&path, bays.get(1..).unwrap_or(&[]), Anchor::Start) {
                post_into(&mut mb, f.position, f.forward, size, height);
            }
            for level in &levels {
                let y = height * 0.2 + level.centre();
                let (x, h) = (rail_w * 0.5, rail_h * 0.5);
                let profile = Profile::new(
                    vec![
                        Vec2::new(-x, y - h),
                        Vec2::new(x, y - h),
                        Vec2::new(x, y + h),
                        Vec2::new(-x, y + h),
                    ],
                    true,
                )
                .with_slots(vec![RAILS; 4]);
                sweep_into(
                    &mut mb,
                    &path,
                    &SweepParams {
                        profile,
                        start: JointEnd::Cap,
                        end: JointEnd::Cap,
                        cap_slot: RAILS,
                        ..SweepParams::default()
                    },
                );
            }
        }
        let built = mb.build();
        warnings.extend(built.warnings);
        GenOutput {
            mesh: Some(built.mesh),
            warnings,
            ..GenOutput::default()
        }
    }
}

/// A square post at `base` turned to `dir`, with a low pyramid top.
fn post_into(mb: &mut MeshBuilder, base: Vec3, dir: Vec3, size: f32, height: f32) {
    let f = flat(dir).normalize_or(Vec3::X) * (size * 0.5);
    let s = perp(f);
    let top = (height - size * 0.4).max(height * 0.5);
    let ring = |y: f32| {
        [f + s, f - s, -f - s, -f + s].map(|c| base + c + Vec3::Y * y) // counter-clockwise from above
    };
    let (b, t) = (ring(0.0), ring(top));
    let apex = base + Vec3::Y * height;
    for i in 0..4 {
        let j = (i + 1) % 4;
        let w = b[i].distance(b[j]);
        mb.flat_quad(
            POSTS,
            [b[j], b[i], t[i], t[j]],
            [
                Vec2::ZERO,
                Vec2::new(w, 0.0),
                Vec2::new(w, top),
                Vec2::new(0.0, top),
            ],
        );
        mb.flat_triangle(
            POSTS,
            [t[j], t[i], apex],
            [Vec2::ZERO, Vec2::new(w, 0.0), Vec2::new(w * 0.5, size)],
        );
    }
    mb.flat_quad(
        POSTS,
        [b[0], b[1], b[2], b[3]],
        [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
    );
}

/// The material every fence part draws with.
pub fn fence_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::hex(0x8a6a4a),
        roughness: 0.9,
        ..Default::default()
    }
}

/// One fence as the rule reads it.
#[derive(Clone, Copy, Debug)]
pub struct FenceIntent<'a> {
    pub uid: Uid,
    pub path: &'a FencePath,
    pub transform: Affine3A,
}

/// What the rule decided for one fence.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedFence {
    pub uid: Uid,
    pub generator: Option<FenceGen>,
    pub warnings: Vec<String>,
}

/// The `fences` rule, as `app.add_rule` takes it (with
/// `app.rule_reads::<WallPath>(RULE)` so fences butt walls).
pub fn plan(input: &RuleInput<FencePath>) -> RuleOutput {
    let intents: Vec<FenceIntent> = input
        .intents
        .iter()
        .map(|i| FenceIntent {
            uid: i.uid,
            path: &i.value,
            transform: i.transform,
        })
        .collect();
    let walls: Vec<WallIntent> = input
        .read::<WallPath>()
        .iter()
        .map(|i| WallIntent {
            uid: i.uid,
            path: &i.value,
            transform: i.transform,
        })
        .collect();
    let mut out = RuleOutput::default();
    for f in plan_fences_with(&intents, &walls) {
        for m in f.warnings {
            out.warn(f.uid, m);
        }
        if let Some(generator) = f.generator {
            out.push(
                Part::new(PartKey::new(f.uid, ROLE))
                    .with(generator)
                    .with(fence_material()),
            );
        }
    }
    out
}

fn at(p: Vec3) -> String {
    format!("({:.2}, {:.2})", p.x, p.z)
}

/// The pure rule without walls (see the module docs), in uid order.
pub fn plan_fences(intents: &[FenceIntent]) -> Vec<PlannedFence> {
    plan_fences_with(intents, &[])
}

/// A wall as fences see it: its world centreline and half thickness.
struct WallFace {
    uid: Uid,
    points: Vec<Vec3>,
    half: f32,
}

fn wall_faces(walls: &[WallIntent]) -> Vec<WallFace> {
    let mut out: Vec<WallFace> = walls
        .iter()
        .filter(|w| w.transform.is_finite())
        .filter_map(|w| {
            let (local, _) = clean_polyline(&w.path.points);
            (local.len() >= 2).then(|| WallFace {
                uid: w.uid,
                points: local
                    .iter()
                    .map(|p| w.transform.transform_point3(*p))
                    .collect(),
                half: positive("thickness", w.path.thickness, 0.3, &mut Vec::new()) * 0.5,
            })
        })
        .collect();
    out.sort_by_key(|w| w.uid);
    out
}

/// Fences more parallel to a wall than this (|cos| of the angle between
/// the fence's end segment and the wall's normal) run along it, not into it.
const BUTT_MIN_COS: f32 = 0.2; // about 11.5°

/// Where a fence end at `end` (its next point `next`) butts a wall, if it
/// lies in a wall's footprint: the point on the wall's face on the fence's
/// side, along the fence's end segment. `Err` for an end running along a
/// wall. The nearest wall wins (then the lower uid).
fn wall_butt(end: Vec3, next: Vec3, walls: &[WallFace]) -> Option<Result<Vec3, Uid>> {
    let mut best: Option<(f32, &WallFace, Vec3, Vec3)> = None; // (distance, wall, closest, segment dir)
    for w in walls {
        for seg in w.points.windows(2) {
            let (a, b) = (flat(seg[0]), flat(seg[1]));
            let ab = b - a;
            let len2 = ab.length_squared();
            if len2 <= MIN_SEGMENT * MIN_SEGMENT {
                continue;
            }
            let p = flat(end);
            let c = a + ab * ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
            let d = p.distance(c);
            if d > w.half + SNAP {
                continue;
            }
            if best.is_none_or(|(bd, ..)| d < bd - 1e-6) {
                best = Some((d, w, c, ab / len2.sqrt()));
            }
        }
    }
    let (_, wall, c, dir) = best?;
    let arm = flat(next - end).normalize_or_zero();
    let n = perp(dir);
    let denom = arm.dot(n);
    if denom.abs() < BUTT_MIN_COS || arm == Vec3::ZERO {
        return Some(Err(wall.uid));
    }
    // The fence's side of the wall: where its next point is.
    let side_of = |q: Vec3| (flat(q) - c).dot(n);
    let side = match side_of(next) {
        s if s.abs() > 1e-4 => s.signum(),
        _ => side_of(end).signum(),
    };
    let t = (side * wall.half - side_of(end)) / denom;
    Some(Ok(end + flat(next - end).normalize_or_zero() * t))
}

/// The pure rule (see the module docs), in uid order: fences, and the walls
/// they may butt.
pub fn plan_fences_with(intents: &[FenceIntent], walls: &[WallIntent]) -> Vec<PlannedFence> {
    let walls = wall_faces(walls);
    let mut sorted: Vec<&FenceIntent> = intents.iter().collect();
    sorted.sort_by_key(|i| i.uid);

    struct Work {
        uid: Uid,
        transform: Affine3A,
        local: Vec<Vec3>,
        shape: Option<usize>,
        posts: [bool; 2],
        snap: [Option<Vec3>; 2],
        warnings: Vec<String>,
        size: f32,
    }
    let mut work = Vec::with_capacity(sorted.len());
    let mut shapes = Vec::new();
    let mut owner_of = Vec::new();
    for i in &sorted {
        let mut warnings = Vec::new();
        let size = positive("post size", i.path.post_size, 0.14, &mut warnings);
        let height = positive("height", i.path.height, 1.1, &mut warnings);
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
                half_width: size * 0.5,
                height,
            });
            owner_of.push(work.len());
            Some(shapes.len() - 1)
        } else {
            warnings.push(if finite {
                "a fence needs at least two distinct points; nothing built".into()
            } else {
                "the fence's transform is not finite; nothing built".into()
            });
            None
        };
        work.push(Work {
            uid: i.uid,
            transform: i.transform,
            local,
            shape,
            posts: [true; 2],
            snap: [None; 2],
            warnings,
            size,
        });
    }

    let adj = analyse(&shapes);
    for node in &adj.nodes {
        for id in &node.ends {
            work[owner_of[id.path]].snap[id.end.index()] = Some(node.point);
        }
    }
    for (path, links) in adj.links.iter().enumerate() {
        for end in End::BOTH {
            if let EndLink::Body(c) = links[end.index()] {
                work[owner_of[path]].snap[end.index()] = Some(c.point);
            }
        }
    }
    // A fence shorter than the snap distance collapses onto its junction.
    let collapsed: Vec<bool> = shapes
        .iter()
        .enumerate()
        .map(|(path, s)| {
            let w = &work[owner_of[path]];
            let pts = snap_ends(&s.points, w.snap[0], w.snap[1]);
            pts.len() < 2 || pts[0].distance(pts[1]) < MIN_SEGMENT
        })
        .collect();
    for node in &adj.nodes {
        // One shared post, built by the first standing fence in uid order.
        let owner = node.ends.iter().find(|id| !collapsed[id.path]);
        for id in &node.ends {
            work[owner_of[id.path]].posts[id.end.index()] = Some(id) == owner;
        }
    }
    // Ends that landed on the same spot without forming a junction (a T
    // snapped onto another fence's end) share the first post too.
    let mut placed: Vec<Vec3> = Vec::new();
    for (path, s) in shapes.iter().enumerate() {
        if collapsed[path] {
            continue;
        }
        let w = &mut work[owner_of[path]];
        for end in End::BOTH {
            let at = w.snap[end.index()].unwrap_or(s.end_point(end));
            if !w.posts[end.index()] {
                continue;
            }
            if placed.iter().any(|p| p.distance(at) <= SNAP) {
                w.posts[end.index()] = false;
            } else {
                placed.push(at);
            }
        }
    }
    // Free ends inside a wall's footprint butt its face, without a post.
    if !walls.is_empty() {
        for (path, s) in shapes.iter().enumerate() {
            if collapsed[path] {
                continue;
            }
            let w = &work[owner_of[path]];
            let pts = snap_ends(&s.points, w.snap[0], w.snap[1]);
            let n = pts.len();
            let mut butts = [None, None];
            for end in End::BOTH {
                if adj.links[path][end.index()] != EndLink::Free {
                    continue;
                }
                let (p, next) = match end {
                    End::Start => (pts[0], pts[1]),
                    End::End => (pts[n - 1], pts[n - 2]),
                };
                butts[end.index()] = wall_butt(p, next, &walls);
            }
            let w = &mut work[owner_of[path]];
            for end in End::BOTH {
                match butts[end.index()] {
                    Some(Ok(p)) => {
                        let (from, next) = match end {
                            End::Start => (pts[0], pts[1]),
                            End::End => (pts[n - 1], pts[n - 2]),
                        };
                        // Not past the next point: an end segment shorter
                        // than the wall is thick keeps its end and post.
                        let along = (p - from).dot((next - from).normalize_or_zero());
                        if along < from.distance(next) - MIN_SEGMENT {
                            w.snap[end.index()] = Some(p);
                            w.posts[end.index()] = false;
                        } else {
                            w.warnings.push(
                                "an end segment is shorter than the wall it meets is thick; \
                                 kept its post"
                                    .into(),
                            );
                        }
                    }
                    Some(Err(_)) => {
                        let m = "an end runs along a wall instead of meeting it; kept its post"
                            .to_owned();
                        if !w.warnings.contains(&m) {
                            w.warnings.push(m);
                        }
                    }
                    None => {}
                }
            }
        }
    }
    for (path, c) in collapsed.iter().enumerate() {
        if *c {
            let w = &mut work[owner_of[path]];
            w.shape = None;
            w.warnings.push(
                "the fence is shorter than the snap distance and collapses onto a junction; nothing built"
                    .into(),
            );
        }
    }
    let mut warn = |path: usize, m: &str| {
        let w: &mut Work = &mut work[owner_of[path]];
        if !w.warnings.iter().any(|x| x == m) {
            w.warnings.push(m.to_owned());
        }
    };
    for c in &adj.crossings {
        let m = format!("crosses another fence at {}", at(c.point));
        warn(c.a, &m);
        warn(c.b, &m);
    }
    for o in &adj.overlaps {
        let m = format!("runs along another fence for {:.2} m", o.length);
        warn(o.a, &m);
        warn(o.b, &m);
    }

    sorted
        .iter()
        .zip(work)
        .map(|(intent, w)| {
            let generator = w.shape.map(|_| {
                let inv = w.transform.inverse();
                let points = snap_ends(
                    &w.local,
                    w.snap[0].map(|p| inv.transform_point3(p)),
                    w.snap[1].map(|p| inv.transform_point3(p)),
                );
                FenceGen {
                    points,
                    height: intent.path.height,
                    post_spacing: intent.path.post_spacing,
                    post_size: w.size,
                    rail_count: intent.path.rail_count,
                    start_post: w.posts[0],
                    end_post: w.posts[1],
                }
            });
            let warnings = w.warnings;
            PlannedFence {
                uid: w.uid,
                generator,
                warnings,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence(points: &[[f32; 2]]) -> FencePath {
        FencePath {
            points: points.iter().map(|p| Vec3::new(p[0], 0.0, p[1])).collect(),
            ..FencePath::default()
        }
    }

    #[test]
    fn meeting_fences_share_one_post() {
        let (a, b, c) = (
            fence(&[[0.0, 0.0], [4.0, 0.0]]),
            fence(&[[4.05, 0.0], [4.0, -3.0]]),
            fence(&[[4.0, 0.08], [7.0, 2.0]]),
        );
        let intents = [(3u128, &c), (1, &a), (2, &b)].map(|(id, p)| FenceIntent {
            uid: Uid::from_u128(id),
            path: p,
            transform: Affine3A::IDENTITY,
        });
        let out = plan_fences(&intents);
        let g: Vec<&FenceGen> = out.iter().map(|f| f.generator.as_ref().unwrap()).collect();
        assert_eq!(
            g.iter()
                .map(|g| (g.start_post, g.end_post))
                .collect::<Vec<_>>(),
            [(true, true), (false, true), (false, true)]
        );
        // Everyone meets at the first fence's end.
        assert_eq!(g[1].points[0], Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(g[2].points[0], Vec3::new(4.0, 0.0, 0.0));
        let mesh = g[0].generate(&GenContext::new(0)).mesh.unwrap();
        assert!(mesh.triangle_count() > 0);
    }

    #[test]
    fn a_fence_end_meeting_a_wall_butts_its_face_without_a_post() {
        // A wall along +Z at x = 0, 0.3 thick: faces at x = ±0.15.
        let wall = WallPath {
            points: vec![Vec3::ZERO, Vec3::new(0.0, 0.0, 4.0)],
            ..WallPath::default()
        };
        let walls = [WallIntent {
            uid: Uid::from_u128(9),
            path: &wall,
            transform: Affine3A::IDENTITY,
        }];
        let plan_one = |f: &FencePath| {
            let intent = FenceIntent {
                uid: Uid::from_u128(1),
                path: f,
                transform: Affine3A::IDENTITY,
            };
            plan_fences_with(&[intent], &walls).remove(0)
        };
        // Ends on the centreline, inside the wall, and 8 cm off its face
        // (within the 10 cm snap) on either side: all butt the near face.
        for (x, face) in [(0.0, 0.15), (0.1, 0.15), (0.23, 0.15), (-0.2, -0.15)] {
            let dir = if face > 0.0 { 1.0 } else { -1.0 };
            let f = fence(&[[x, 2.0], [x + dir * 5.0, 2.0]]);
            let out = plan_one(&f);
            let g = out.generator.unwrap();
            assert!(!g.start_post && g.end_post, "x {x}");
            assert!(
                (g.points[0].x - face).abs() < 1e-5,
                "x {x}: {:?}",
                g.points[0]
            );
            assert_eq!(g.points[0].z, 2.0);
            assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        }
        // At an angle: still on the face, along the fence's own line.
        let f = fence(&[[0.0, 2.0], [3.0, 5.0]]);
        let g = plan_one(&f).generator.unwrap();
        assert!(!g.start_post);
        assert!((g.points[0].x - 0.15).abs() < 1e-5 && (g.points[0].z - 2.15).abs() < 1e-5);
        // Out of reach (0.15 + 0.1 < 0.3): a free end with its post.
        let f = fence(&[[0.3, 2.0], [5.0, 2.0]]);
        let g = plan_one(&f).generator.unwrap();
        assert!(g.start_post);
        assert_eq!(g.points[0], Vec3::new(0.3, 0.0, 2.0));
        // Running along the wall: keeps its post, with a warning.
        let f = fence(&[[0.1, 1.0], [0.1, 3.0], [3.0, 3.0]]);
        let out = plan_one(&f);
        assert!(out.generator.unwrap().start_post);
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        // Without walls (plan_fences): unchanged M5 behaviour.
        let f = fence(&[[0.1, 2.0], [5.0, 2.0]]);
        let intent = FenceIntent {
            uid: Uid::from_u128(1),
            path: &f,
            transform: Affine3A::IDENTITY,
        };
        assert!(
            plan_fences(&[intent])[0]
                .generator
                .as_ref()
                .unwrap()
                .start_post
        );
    }
}
