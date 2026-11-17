//! The [`Wall`] generator: a solid wall along a centreline (a polyline
//! with optional arcs, or a [`Spline`] on this or another entity), with
//! explicit junctions at both ends, openings cut in 2D elevation, reveals,
//! panels, sills, swept bands and analytic vertex ambient occlusion.
//!
//! **One mapping for everything** (spike S6). A wall is a centreline plus
//! *joint lines* at each vertex. Every surface (faces, reveals, panels,
//! plinth, cornice, sills) is placed in (U, v, w): U is arc length along
//! the centreline from its first point, v is height above the wall foot,
//! w is the offset across it, positive on the wall's **outer** side, the
//! right-hand side walking along it (the outside of a loop drawn
//! counter-clockwise seen from above). Points are mapped through the same
//! joint lines, so every layer meets every other exactly at corners, at any
//! angle and on curves. Inside one segment the mapping is perpendicular,
//! so openings near mitred corners do not skew.
//!
//! **Junctions are parameters** ([`WallEnd`]). The generator never reads
//! its neighbours: a game rule (or [`crate::junction`]'s helpers) decides
//! mitre, butt, T or abutment and ownership and writes the result here.
//!
//! **Tops** are flat (`height`) or follow a roof's underside
//! ([`Wall::roof`], a [`RoofUnderside`]) as a function of plan position, so
//! gables and eave walls come for free and the wall-to-roof joint is hidden
//! inside the slab.

use bevy_reflect::Reflect;
use box_core::identity::Uid;
use box_core::pmath;
use box_core::reflect::{Range, ReflectComponent, ReflectDefault, Step, Unit};
use box_proc::generator::{ColliderHint, GenContext, GenOutput, Generator, HintBox};
use box_proc::inputs::Inputs;
use box_proc::spline::{Sampling, Spline};
use glam::{Affine3A, Quat, Vec2, Vec3};

use crate::clip::{self, Tri};
use crate::consts::*;
use crate::mesh::{ArchMesh, V};
use crate::underside::RoofUnderside;

/// Material slots of [`Wall`].
pub mod slot {
    pub const WALL: u32 = 0;
    pub const REVEAL: u32 = 1;
    pub const TOP: u32 = 2;
    pub const TRIM: u32 = 3;
    pub const GLASS: u32 = 4;
    pub const DOOR: u32 = 5;
}

/// One centreline point: a plan position (x, z) in the entity's local space
/// and the arc to the next point.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct WallPoint {
    /// Plan position: x, then z (the wall stands on y = 0).
    #[reflect(@Unit("m"), @Step(0.01))]
    pub position: Vec2,
    /// Arc to the next point as a DXF bulge, tan(angle / 4): 0 is straight,
    /// 1 a half circle bulging to the right, −1 to the left.
    #[reflect(@Range { min: -1.0, max: 1.0 }, @Step(0.01))]
    pub bulge: f32,
}

impl Default for WallPoint {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

impl WallPoint {
    /// A straight point at plan (x, z).
    pub fn new(x: f32, z: f32) -> Self {
        Self {
            position: Vec2::new(x, z),
            bulge: 0.0,
        }
    }

    /// The same point with an arc to the next one.
    pub fn with_bulge(mut self, bulge: f32) -> Self {
        self.bulge = bulge;
        self
    }
}

/// Where a wall's centreline comes from.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum WallPath {
    /// A polyline with optional arcs, in the entity's local space.
    Points {
        points: Vec<WallPoint>,
        /// Join the last point back to the first (a closed loop of walls).
        closed: bool,
    },
    /// The `Spline` of this entity (`entity: None`) or of another one,
    /// sampled within `tolerance`; editing or moving it regenerates the wall.
    /// The spline is flattened onto the plan (its heights are ignored).
    Spline {
        entity: Option<Uid>,
        #[reflect(@Range { min: 0.001, max: 1.0 }, @Unit("m"))]
        tolerance: f32,
    },
}

impl Default for WallPath {
    fn default() -> Self {
        Self::Points {
            points: vec![WallPoint::new(-2.0, 0.0), WallPoint::new(2.0, 0.0)],
            closed: false,
        }
    }
}

/// How one end of a wall finishes. Written by a game rule (or the
/// [`crate::junction`] helpers); the wall itself never looks at its
/// neighbours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum WallEnd {
    /// A free end: square, with a visible end face.
    #[default]
    Cap,
    /// Square, no end face: another wall continues in line and covers it.
    Open,
    /// Runs on `distance` past the end point with no end face, into a solid
    /// such as a tower (spike S6, rule J10); occlusion and opening
    /// clearances are measured from the end point, where it emerges.
    Extend {
        #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("m"))]
        distance: f32,
    },
    /// Ends square against a solid surface at the end point (a taller
    /// wall's face, a pillar): no end face, concave occlusion on both sides.
    Abut,
    /// Mitred with a neighbouring wall entity of the same thickness:
    /// `neighbour` is the neighbour's direction leaving the shared point
    /// (plan x, z). Both walls then share the bisector joint line and
    /// bit-identical corner vertices. Corners sharper than the mitre limit
    /// fall back to `Cap` with a warning.
    Mitre { neighbour: Vec2 },
    /// Cut by the vertical plane through the line `point` + t·`direction`
    /// (plan x, z): a neighbour's face line (spike S6, rule J2). `cap` draws
    /// an end face on the line (the owner of an L corner). `band_shift`
    /// moves the line into this wall for plinths (positive) or out of it
    /// (negative), so they stop at the neighbour's plinth face. `concave`
    /// says which sides (outer, inner) form a concave corner with the
    /// neighbour for occlusion: only the rule that placed the neighbour
    /// knows (an L leaves one side flush, a T makes both concave).
    Butt {
        point: Vec2,
        direction: Vec2,
        cap: bool,
        #[reflect(@Unit("m"))]
        band_shift: f32,
        concave: [bool; 2],
    },
}

/// What an opening is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum OpeningKind {
    /// Glass panel with glazing bars and a stone sill.
    #[default]
    Window,
    /// A door leaf and a threshold; no sill reveal when it starts at the
    /// wall foot.
    Door,
    /// An open passage (an archway): reveals only.
    Passage,
}

/// The shape of an opening's head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum OpeningTop {
    #[default]
    Flat,
    /// A half circle as wide as the opening.
    Round,
    /// An equilateral pointed (Gothic) arch.
    Pointed,
}

/// A door, window or passage cut through the wall.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct Opening {
    pub kind: OpeningKind,
    pub top: OpeningTop,
    /// Centre, as arc length along the centreline from its first point.
    #[reflect(@Range { min: -1000.0, max: 1000.0 }, @Unit("m"), @Step(0.01))]
    pub offset: f32,
    #[reflect(@Range { min: 0.1, max: 20.0 }, @Unit("m"), @Step(0.01))]
    pub width: f32,
    /// From the bottom to the top of the head.
    #[reflect(@Range { min: 0.1, max: 20.0 }, @Unit("m"), @Step(0.01))]
    pub height: f32,
    /// Bottom above the wall foot (0 for doors and passages at ground level).
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m"), @Step(0.01))]
    pub sill: f32,
}

impl Default for Opening {
    fn default() -> Self {
        Self::window(0.0)
    }
}

impl Opening {
    /// A window centred at `offset`: 0.8 × 1.1 m, sill at 0.95 m.
    pub fn window(offset: f32) -> Self {
        Self {
            kind: OpeningKind::Window,
            top: OpeningTop::Flat,
            offset,
            width: 0.8,
            height: 1.1,
            sill: 0.95,
        }
    }

    /// A round-headed window: 0.85 × 1.35 m, sill at 0.9 m.
    pub fn arched_window(offset: f32) -> Self {
        Self {
            top: OpeningTop::Round,
            width: 0.85,
            height: 1.35,
            sill: 0.9,
            ..Self::window(offset)
        }
    }

    /// A door at ground level: 1.0 × 2.15 m.
    pub fn door(offset: f32) -> Self {
        Self {
            kind: OpeningKind::Door,
            top: OpeningTop::Flat,
            offset,
            width: 1.0,
            height: 2.15,
            sill: 0.0,
        }
    }

    /// A round-headed door: 1.1 × 2.35 m.
    pub fn arched_door(offset: f32) -> Self {
        Self {
            top: OpeningTop::Round,
            width: 1.1,
            height: 2.35,
            ..Self::door(offset)
        }
    }

    /// An open archway: 1.4 × 2.6 m with a round head.
    pub fn archway(offset: f32) -> Self {
        Self {
            kind: OpeningKind::Passage,
            top: OpeningTop::Round,
            offset,
            width: 1.4,
            height: 2.6,
            sill: 0.0,
        }
    }

    /// The same opening with another head.
    pub fn with_top(mut self, top: OpeningTop) -> Self {
        self.top = top;
        self
    }
}

/// A wall: a solid of `thickness` along a centreline, `height` tall (or
/// up to a roof's underside), with explicit end junctions, openings,
/// optional bands and ambient occlusion. Material slots: Wall (faces and
/// end faces), Reveal, Top, Trim (plinth, cornice, coping, sills,
/// thresholds, glazing bars), Glass, Door.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct Wall {
    /// The centreline: points with arcs, or a spline.
    pub path: WallPath,
    #[reflect(@Range { min: 0.02, max: 5.0 }, @Unit("m"), @Step(0.01))]
    pub thickness: f32,
    /// Height of a flat top (ignored when `roof` is set).
    #[reflect(@Range { min: 0.05, max: 100.0 }, @Unit("m"), @Step(0.01))]
    pub height: f32,
    /// Take the top from a roof's underside instead (written by a game rule
    /// from `Roof::underside`, in this wall's space).
    pub roof: Option<RoofUnderside>,
    /// The junction at the first point.
    pub start: WallEnd,
    /// The junction at the last point (ignored for closed paths).
    pub end: WallEnd,
    pub openings: Vec<Opening>,
    /// A stone band along the foot, on both faces.
    pub plinth: bool,
    /// A band under the visible top, on the outer face.
    pub cornice: bool,
    /// A capping band over a flat top (free-standing walls).
    pub coping: bool,
    /// Strength of the baked ambient occlusion (0 turns it off).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.05))]
    pub ao: f32,
}

impl Default for Wall {
    fn default() -> Self {
        Self {
            path: WallPath::default(),
            thickness: 0.3,
            height: 3.0,
            roof: None,
            start: WallEnd::Cap,
            end: WallEnd::Cap,
            openings: Vec::new(),
            plinth: false,
            cornice: false,
            coping: false,
            ao: 1.0,
        }
    }
}

impl Wall {
    /// A straight or bent wall through plan points (x, z).
    pub fn through(points: &[Vec2]) -> Self {
        Self {
            path: WallPath::Points {
                points: points.iter().map(|p| WallPoint::new(p.x, p.y)).collect(),
                closed: false,
            },
            ..Self::default()
        }
    }

    /// A closed loop of walls through plan points (x, z).
    pub fn closed_loop(points: &[Vec2]) -> Self {
        Self {
            path: WallPath::Points {
                points: points.iter().map(|p| WallPoint::new(p.x, p.y)).collect(),
                closed: true,
            },
            ..Self::default()
        }
    }

    /// The entity whose spline this wall reads, for a wall on `owner`.
    pub fn spline_uid(&self, owner: Option<Uid>) -> Option<Uid> {
        match &self.path {
            WallPath::Points { .. } => None,
            WallPath::Spline { entity, .. } => entity.or(owner),
        }
    }

    /// The centreline in the entity's local space, arcs and splines
    /// sampled. `spline` is the spline already in this entity's space (for
    /// a `WallPath::Spline`).
    pub fn centreline(&self, spline: Option<&Spline>) -> Result<Centreline, String> {
        let (mut pts, closed) = match &self.path {
            WallPath::Points { points, closed } => (discretise(points, *closed), *closed),
            WallPath::Spline { tolerance, .. } => {
                let Some(spline) = spline else {
                    return Err("the wall's spline is missing".into());
                };
                let tol = if tolerance.is_finite() && *tolerance > 0.0 {
                    tolerance.max(1e-3)
                } else {
                    0.01
                };
                let samples = spline.curve().sample(Sampling::Adaptive {
                    tolerance: tol,
                    max_step: f32::MAX,
                });
                let pts = samples
                    .iter()
                    .map(|s| Vec2::new(s.position.x, s.position.z))
                    .collect();
                (pts, spline.closed)
            }
        };
        pts.retain(|p| p.is_finite() && p.abs().max_element() < 1e6);
        pts.dedup_by(|a, b| a.distance_squared(*b) < 1e-8);
        if closed && pts.len() > 1 && pts[0].distance_squared(pts[pts.len() - 1]) < 1e-8 {
            pts.pop();
        }
        let need = if closed { 3 } else { 2 };
        if pts.len() < need {
            return Err(format!(
                "a {} wall needs at least {need} distinct points",
                if closed { "closed" } else { "open" }
            ));
        }
        Ok(Centreline {
            points: pts,
            closed,
        })
    }

    /// Generates the wall from its parameters and, for a spline path, the
    /// spline in this entity's space.
    pub fn build(&self, spline: Option<&Spline>) -> GenOutput {
        let mut warnings = Vec::new();
        let line = match self.centreline(spline) {
            Ok(l) => l,
            Err(e) => return GenOutput::empty().warn(e),
        };
        let params = Params::clean(self, &mut warnings);
        let runs = split_runs(&line, &params, &mut warnings);
        let mut mesh = ArchMesh::new(params.ao);
        let mut boxes = Vec::new();
        let mut placed = vec![false; self.openings.len()];
        for run in &runs {
            let g = Geom::new(&params, run, &line, &mut placed);
            g.build(&mut mesh);
            if let Some(fill) = run.fill {
                g.bevel_fill(&mut mesh, fill);
            }
            boxes.extend(g.collider_boxes());
            warnings.extend(g.warnings);
        }
        for (idx, done) in placed.iter().enumerate() {
            if !done {
                warnings.push(format!(
                    "opening {idx} (at U={:.2}) is outside the wall (0 to {:.2} m); dropped",
                    self.openings[idx].offset,
                    line.length()
                ));
            }
        }
        let (m, w) = mesh.build();
        warnings.extend(w);
        dedup(&mut warnings);
        GenOutput {
            mesh: (m.triangle_count() > 0).then_some(m),
            warnings,
            collider: ColliderHint::Boxes(boxes),
        }
    }
}

fn dedup(warnings: &mut Vec<String>) {
    let mut seen = Vec::new();
    warnings.retain(|w| {
        if seen.contains(w) {
            false
        } else {
            seen.push(w.clone());
            true
        }
    });
}

/// `spline` with its points mapped by `to_local`.
pub(crate) fn transformed_spline(spline: &Spline, to_local: &Affine3A) -> Spline {
    let mut s = spline.clone();
    for p in &mut s.points {
        p.position = to_local.transform_point3(p.position);
        p.in_tangent = p.in_tangent.map(|t| to_local.transform_vector3(t));
        p.out_tangent = p.out_tangent.map(|t| to_local.transform_vector3(t));
    }
    s
}

impl Generator for Wall {
    /// 2: arcs use portable trigonometry (D17); pointed arcs moved by an ulp.
    /// 3: sub-millimetre face pieces away from the origin are no longer
    /// dropped as flat (they left holes at short arc segments).
    const VERSION: u32 = 3;
    const MATERIAL_SLOTS: &'static [&'static str] =
        &["Wall", "Reveal", "Top", "Trim", "Glass", "Door"];
    const CATEGORY: &'static str = "Architecture";

    fn inputs(&self, inputs: &mut Inputs) {
        let owner = inputs.owner();
        let Some(uid) = self.spline_uid(owner) else {
            return;
        };
        inputs.component::<Spline>(uid);
        if Some(uid) != owner {
            inputs.world_transform(uid);
            if let Some(own) = owner {
                inputs.world_transform(own);
            }
        }
    }

    fn generate(&self, ctx: &GenContext) -> GenOutput {
        let WallPath::Spline { .. } = self.path else {
            return self.build(None);
        };
        let owner = ctx.owner();
        let Some(uid) = self.spline_uid(owner) else {
            return GenOutput::empty().warn(
                "no spline: add a Spline to this entity or set the path's entity to one with a Spline",
            );
        };
        // A missing input is reported by the runtime.
        let Some(spline) = ctx.component::<Spline>(uid) else {
            return GenOutput::empty();
        };
        if Some(uid) == owner {
            return self.build(Some(spline));
        }
        let own = owner.and_then(|o| ctx.world_transform(o));
        let to_local = match (own, ctx.world_transform(uid)) {
            (Some(own), Some(theirs)) => own.inverse() * theirs,
            (None, Some(theirs)) => theirs,
            _ => Affine3A::IDENTITY,
        };
        self.build(Some(&transformed_spline(spline, &to_local)))
    }
}

// ------------------------------------------------------------ centreline ----

/// A wall's sampled centreline in the entity's local space: plan points
/// (x, z), U measured from the first point.
#[derive(Clone, Debug, PartialEq)]
pub struct Centreline {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

impl Centreline {
    pub fn segment_count(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    fn seg(&self, i: usize) -> (Vec2, Vec2) {
        (self.points[i], self.points[(i + 1) % self.points.len()])
    }

    /// Total length.
    pub fn length(&self) -> f32 {
        (0..self.segment_count())
            .map(|i| {
                let (a, b) = self.seg(i);
                a.distance(b)
            })
            .sum()
    }

    /// The plan point, unit direction and outer normal at arc length `u`
    /// (clamped to the line; wrapped on closed loops).
    pub fn at(&self, u: f32) -> (Vec2, Vec2, Vec2) {
        let total = self.length();
        let u = if self.closed && total > 0.0 {
            u.rem_euclid(total)
        } else {
            u
        };
        let mut acc = 0.0;
        let n = self.segment_count();
        for i in 0..n {
            let (a, b) = self.seg(i);
            let len = a.distance(b);
            if u <= acc + len || i == n - 1 {
                let d = (b - a) / len.max(1e-9);
                return (a + d * (u - acc), d, perp(d));
            }
            acc += len;
        }
        (self.points[0], Vec2::X, perp(Vec2::X))
    }
}

/// Polyline with arcs → plain points.
fn discretise(points: &[WallPoint], closed: bool) -> Vec<Vec2> {
    let n = points.len();
    if n < 2 {
        return points.iter().map(|p| p.position).collect();
    }
    let nseg = if closed { n } else { n - 1 };
    let mut pts = Vec::with_capacity(n * 2);
    for i in 0..nseg {
        let a = points[i].position;
        let b = points[(i + 1) % n].position;
        pts.push(a);
        let bulge = points[i].bulge;
        if bulge.is_finite() && bulge.abs() > 1e-4 && a.distance_squared(b) > 1e-8 {
            let bulge = bulge.clamp(-1.0, 1.0);
            let theta = 4.0 * pmath::atan(bulge);
            let chord = b - a;
            let r = chord.length() / (2.0 * pmath::sin(theta * 0.5));
            let mid = (a + b) * 0.5;
            let h = r * pmath::cos(theta * 0.5);
            // Positive bulges bulge to the right of travel (+w, outer).
            let centre = mid - perp(chord.normalize()) * h;
            let steps = (theta.abs() / ARC_STEP).ceil().max(2.0) as usize;
            let a0 = pmath::atan2((a - centre).y, (a - centre).x);
            for k in 1..steps {
                let ang = a0 - theta * (k as f32 / steps as f32);
                pts.push(centre + Vec2::new(pmath::cos(ang), pmath::sin(ang)) * r.abs());
            }
        }
    }
    if !closed {
        pts.push(points[n - 1].position);
    }
    pts
}

/// The outer normal: right of travel seen from above (+w).
pub(crate) fn perp(d: Vec2) -> Vec2 {
    Vec2::new(-d.y, d.x)
}

pub(crate) fn cross2(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

fn to3(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `x`, or the nearest of `features` within [`BREAK_SNAP`].
fn snap_break(x: f32, features: &[f32]) -> f32 {
    features
        .iter()
        .copied()
        .filter(|f| (f - x).abs() < BREAK_SNAP)
        .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()))
        .unwrap_or(x)
}

fn mitre_vec(n_in: Vec2, n_out: Vec2) -> Vec2 {
    let k = 1.0 + n_in.dot(n_out);
    if k < 1e-3 { n_in } else { (n_in + n_out) / k }
}

/// Section stretch of a mitre between directions `din` and `dout`.
fn mitre_stretch(din: Vec2, dout: Vec2) -> f32 {
    let c = ((1.0 + din.dot(dout)) * 0.5).max(0.0).sqrt();
    if c < 1e-6 { f32::MAX } else { 1.0 / c }
}

// ---------------------------------------------------------------- params ----

/// Validated parameters shared by every run.
pub(crate) struct Params<'a> {
    pub w: &'a Wall,
    pub t: f32,
    pub height: f32,
    pub roof: Option<&'a RoofUnderside>,
    pub ao: f32,
}

impl<'a> Params<'a> {
    fn clean(w: &'a Wall, warnings: &mut Vec<String>) -> Self {
        let t = if w.thickness.is_finite() && w.thickness >= 0.01 {
            w.thickness
        } else {
            warnings.push("thickness must be at least 0.01 m; using 0.3".into());
            0.3
        };
        let height = if w.height.is_finite() && w.height >= MIN_WALL_TOP {
            w.height
        } else {
            warnings.push(format!("height must be at least {MIN_WALL_TOP} m; using 3"));
            3.0
        };
        let roof = match &w.roof {
            Some(r) if r.surfaces.is_empty() => {
                warnings.push("the roof underside has no surfaces; using the flat height".into());
                None
            }
            other => other.as_ref(),
        };
        let ao = if w.ao.is_finite() {
            w.ao.clamp(0.0, 1.0)
        } else {
            1.0
        };
        Self {
            w,
            t,
            height,
            roof,
            ao,
        }
    }

    fn top_pt(&self, p: Vec2) -> f32 {
        match self.roof {
            None => self.height,
            Some(r) => r.at(p).clamp(MIN_WALL_TOP, 1e5),
        }
    }

    fn lift(&self) -> f32 {
        self.roof.map_or(0.0, |r| r.lift)
    }
}

/// A run of the centreline built as one mitred piece. Corners sharper than
/// the mitre limit split a wall into runs.
pub(crate) struct Run {
    pub pts: Vec<Vec2>,
    pub closed: bool,
    pub start: WallEnd,
    pub end: WallEnd,
    /// U of `pts[0]` along the whole centreline.
    pub u0: f32,
    /// A bevel fill at the run's end (split corner): plan triangle.
    pub fill: Option<[Vec2; 3]>,
}

fn split_runs(line: &Centreline, p: &Params, warnings: &mut Vec<String>) -> Vec<Run> {
    let pts = &line.points;
    let n = pts.len();
    let limit = MITRE_LIMIT;
    let acute = |k: usize| -> bool {
        let prev = pts[(k + n - 1) % n];
        let next = pts[(k + 1) % n];
        let din = (pts[k] - prev).normalize_or_zero();
        let dout = (next - pts[k]).normalize_or_zero();
        mitre_stretch(din, dout) > limit
    };
    let interior: Vec<usize> = if line.closed {
        (0..n).collect()
    } else {
        (1..n - 1).collect()
    };
    let splits: Vec<usize> = interior.into_iter().filter(|&k| acute(k)).collect();
    // U of each point along the original line.
    let mut us = vec![0.0f32; n + 1];
    for i in 0..n {
        let j = (i + 1) % n;
        us[i + 1] = us[i] + pts[i].distance(pts[j]);
    }
    for &k in &splits {
        let deg = {
            let prev = pts[(k + n - 1) % n];
            let next = pts[(k + 1) % n];
            let a = (prev - pts[k]).normalize_or_zero();
            let b = (next - pts[k]).normalize_or_zero();
            pmath::acos(a.dot(b).clamp(-1.0, 1.0)).to_degrees()
        };
        warnings.push(format!(
            "corner at ({:.2}, {:.2}) is sharper than the mitre limit ({deg:.0}° < {:.0}°); both sides end square with a bevel fill",
            pts[k].x,
            pts[k].y,
            (2.0 * pmath::asin(1.0 / limit)).to_degrees()
        ));
    }
    let fill_at = |k: usize| -> [Vec2; 3] {
        let prev = pts[(k + n - 1) % n];
        let next = pts[(k + 1) % n];
        let din = (pts[k] - prev).normalize_or_zero();
        let dout = (next - pts[k]).normalize_or_zero();
        let (ni, no) = (perp(din), perp(dout));
        let sigma = if (ni + no).dot(dout - din) < 0.0 {
            1.0
        } else {
            -1.0
        };
        let h = p.t * 0.5;
        [pts[k], pts[k] + ni * h * sigma, pts[k] + no * h * sigma]
    };
    if splits.is_empty() {
        return vec![Run {
            pts: pts.clone(),
            closed: line.closed,
            start: p.w.start,
            end: p.w.end,
            u0: 0.0,
            fill: None,
        }];
    }
    let mut runs = Vec::new();
    if line.closed {
        // Each run goes from one split vertex to the next (wrapping).
        for (i, &a) in splits.iter().enumerate() {
            let b = splits[(i + 1) % splits.len()];
            let mut run_pts = vec![pts[a]];
            let mut k = a;
            loop {
                k = (k + 1) % n;
                run_pts.push(pts[k]);
                if k == b {
                    break;
                }
            }
            runs.push(Run {
                pts: run_pts,
                closed: false,
                start: WallEnd::Cap,
                end: WallEnd::Cap,
                u0: us[a],
                fill: Some(fill_at(b)),
            });
        }
    } else {
        let mut cuts = vec![0];
        cuts.extend(&splits);
        cuts.push(n - 1);
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            runs.push(Run {
                pts: pts[a..=b].to_vec(),
                closed: false,
                start: if a == 0 { p.w.start } else { WallEnd::Cap },
                end: if b == n - 1 { p.w.end } else { WallEnd::Cap },
                u0: us[a],
                fill: (b != n - 1).then(|| fill_at(b)),
            });
        }
    }
    runs
}

// ------------------------------------------------------------- geometry ----

#[derive(Clone, Copy, Debug)]
struct Seg {
    a: Vec2,
    d: Vec2,
    n: Vec2,
    len: f32,
    u0: f32,
}

#[derive(Clone, Copy, Debug)]
struct Joint {
    pt: Vec2,
    dir: Vec2,
    smooth: bool,
    hard: bool,
    ns: Vec2,
    band_shift: f32,
    cap: bool,
    ao_pos: bool,
    ao_neg: bool,
    /// The joint's reach for opening clearances is zero (free or open end).
    free: bool,
    /// The joint's direction is fixed by its kind (not decided by geometry).
    explicit_ao: bool,
}

/// A validated opening in run-local U.
#[derive(Clone, Copy, Debug)]
struct Hole {
    kind: OpeningKind,
    top: OpeningTop,
    u: f32,
    width: f32,
    sill: f32,
    height: f32,
    /// The opening starts at the wall foot (cut below it, no sill reveal).
    at_foot: bool,
}

impl Hole {
    fn l(&self) -> f32 {
        self.u - self.width * 0.5
    }
    fn r(&self) -> f32 {
        self.u + self.width * 0.5
    }
    fn head(&self) -> f32 {
        self.sill + self.height
    }
    /// The springing line of an arched head.
    fn spring(&self) -> f32 {
        match self.top {
            OpeningTop::Flat => self.head(),
            OpeningTop::Round => self.head() - self.width * 0.5,
            OpeningTop::Pointed => self.head() - self.width * 3f32.sqrt() * 0.5,
        }
    }
}

/// The expanded geometry of one run, shared by all its parts.
pub(crate) struct Geom<'a> {
    p: &'a Params<'a>,
    segs: Vec<Seg>,
    joints: Vec<Joint>,
    closed: bool,
    start: WallEnd,
    end: WallEnd,
    u_start: f32,
    u_end: f32,
    /// Run U = original U − `u_shift`.
    pub warnings: Vec<String>,
    holes: Vec<Hole>,
    v_breaks: Vec<f32>,
    u_breaks: Vec<f32>,
    curved_top: bool,
}

impl<'a> Geom<'a> {
    fn new(p: &'a Params<'a>, run: &Run, line: &Centreline, placed: &mut [bool]) -> Self {
        let pts = &run.pts;
        let closed = run.closed;
        let mut warnings = Vec::new();
        let nseg = if closed { pts.len() } else { pts.len() - 1 };
        let mut segs = Vec::with_capacity(nseg);
        let mut u = 0.0;
        for i in 0..nseg {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            let len = (b - a).length();
            let d = (b - a) / len;
            segs.push(Seg {
                a,
                d,
                n: perp(d),
                len,
                u0: u,
            });
            u += len;
        }
        let total = u;
        let h = p.t * 0.5;
        let (mut start, mut end) = (run.start, run.end);
        // Ends that cannot work fall back to Cap, with a warning.
        for (is_start, cond) in [(true, &mut start), (false, &mut end)] {
            if closed {
                break;
            }
            let s = if is_start { segs[0] } else { segs[nseg - 1] };
            let which = if is_start { "start" } else { "end" };
            match *cond {
                WallEnd::Mitre { neighbour } => {
                    let nb = neighbour.normalize_or_zero();
                    // Traversal-sense directions at the shared point.
                    let (din, dout) = if is_start { (-nb, s.d) } else { (s.d, nb) };
                    if nb == Vec2::ZERO || !nb.is_finite() {
                        warnings.push(format!(
                            "{which}: mitre without a neighbour direction; capped"
                        ));
                        *cond = WallEnd::Cap;
                    } else if mitre_stretch(din, dout) > MITRE_LIMIT {
                        warnings.push(format!(
                            "{which}: mitre sharper than the mitre limit; capped (the rule should butt or bevel it)"
                        ));
                        *cond = WallEnd::Cap;
                    }
                }
                WallEnd::Butt {
                    point, direction, ..
                } => {
                    let ld = direction.normalize_or_zero();
                    let sin = cross2(s.d, ld).abs();
                    if ld == Vec2::ZERO || !point.is_finite() || sin < 0.25 {
                        warnings.push(format!(
                            "{which}: butt line missing or within 15° of the wall; capped"
                        ));
                        *cond = WallEnd::Cap;
                    } else {
                        let uu = cross2(point - s.a, ld) / cross2(s.d, ld);
                        // The cut must leave the end segment some length and
                        // not run on further than the section's reach.
                        let reach = h / sin + h;
                        let ok = if is_start {
                            uu < s.len - 1e-3 && uu > -reach - 1.0
                        } else {
                            uu > 1e-3 && uu < s.len + reach + 1.0
                        };
                        if !ok {
                            warnings.push(format!(
                                "{which}: butt line does not cross the end segment; capped"
                            ));
                            *cond = WallEnd::Cap;
                        }
                    }
                }
                WallEnd::Extend { distance } => {
                    if !distance.is_finite() || distance < 0.0 {
                        warnings.push(format!(
                            "{which}: extend distance must be 0 or more; capped"
                        ));
                        *cond = WallEnd::Cap;
                    }
                }
                WallEnd::Cap | WallEnd::Open | WallEnd::Abut => {}
            }
        }
        // Joints.
        let nj = if closed { nseg } else { nseg + 1 };
        let mut joints = Vec::with_capacity(nj);
        for k in 0..nj {
            let interior = closed || (k > 0 && k < nseg);
            let mut j = Joint {
                pt: pts[k % pts.len()],
                dir: Vec2::ZERO,
                smooth: false,
                hard: false,
                ns: Vec2::ZERO,
                band_shift: 0.0,
                cap: false,
                ao_pos: false,
                ao_neg: false,
                free: false,
                explicit_ao: false,
            };
            if interior {
                let si = &segs[(k + nseg - 1) % nseg];
                let so = &segs[k % nseg];
                j.dir = mitre_vec(si.n, so.n);
                let ang = pmath::acos(si.d.dot(so.d).clamp(-1.0, 1.0));
                j.smooth = ang < SMOOTH_JOINT;
                j.hard = !j.smooth;
                j.ns = if j.smooth {
                    (si.n + so.n).normalize()
                } else {
                    Vec2::ZERO
                };
            } else {
                let is_start = k == 0;
                let s = if is_start { segs[0] } else { segs[nseg - 1] };
                let cond = if is_start { start } else { end };
                j.dir = s.n;
                match cond {
                    WallEnd::Cap => {
                        j.cap = true;
                        j.free = true;
                    }
                    WallEnd::Open => j.free = true,
                    WallEnd::Extend { distance } => {
                        j.pt += if is_start { -s.d } else { s.d } * distance;
                        j.ao_pos = true;
                        j.ao_neg = true;
                        j.explicit_ao = true;
                    }
                    WallEnd::Abut => {
                        j.ao_pos = true;
                        j.ao_neg = true;
                        j.explicit_ao = true;
                    }
                    WallEnd::Mitre { neighbour } => {
                        let on = perp(neighbour.normalize());
                        j.dir = if is_start {
                            mitre_vec(-on, s.n)
                        } else {
                            mitre_vec(s.n, on)
                        };
                        j.hard = true;
                    }
                    WallEnd::Butt {
                        point,
                        direction,
                        cap,
                        band_shift,
                        concave,
                    } => {
                        let ld = direction.normalize();
                        let uu = cross2(point - s.a, ld) / cross2(s.d, ld);
                        j.pt = s.a + s.d * uu;
                        j.dir = ld;
                        j.cap = cap;
                        j.band_shift = if band_shift.is_finite() {
                            band_shift
                        } else {
                            0.0
                        };
                        j.ao_pos = concave[0];
                        j.ao_neg = concave[1];
                        j.explicit_ao = true;
                    }
                }
            }
            joints.push(j);
        }
        let mut g = Geom {
            p,
            segs,
            joints,
            closed,
            start,
            end,
            u_start: 0.0,
            u_end: total,
            warnings,
            holes: Vec::new(),
            v_breaks: Vec::new(),
            u_breaks: Vec::new(),
            curved_top: p.roof.is_some_and(|r| !r.is_planar()),
        };
        if !closed {
            // End joints moved along the axis (extend, butt): adjust the end
            // segments; U stays measured from the run's first point.
            let last = g.segs.len() - 1;
            let s0 = g.segs[0];
            let shift0 = (g.joints[0].pt - s0.a).dot(s0.d);
            let sl = g.segs[last];
            let end_len = (g.joints[last + 1].pt - sl.a).dot(sl.d);
            g.segs[last].len = end_len;
            g.segs[0].a = s0.a + s0.d * shift0;
            g.segs[0].len -= shift0;
            g.segs[0].u0 = shift0;
            let mut u = g.segs[0].u0 + g.segs[0].len;
            for s in g.segs.iter_mut().skip(1) {
                s.u0 = u;
                u += s.len;
            }
            g.u_start = shift0;
            g.u_end = u;
        }
        // Concave sides of hard joints, decided geometrically: on the
        // concave side the face is shorter than the centreline.
        for k in 0..g.joints.len() {
            if !g.joints[k].hard || g.joints[k].explicit_ao {
                continue;
            }
            let (e_pos, e_neg) = if k < g.segs.len() {
                let lo = g.joint_u(k, false, h, 0.0) - g.joint_u(k, false, 0.0, 0.0);
                (lo > 1e-4, lo < -1e-4)
            } else {
                let s = g.segs.len() - 1;
                let hi = g.joint_u(s, true, h, 0.0) - g.joint_u(s, true, 0.0, 0.0);
                (hi < -1e-4, hi > 1e-4)
            };
            g.joints[k].ao_pos = e_pos;
            g.joints[k].ao_neg = e_neg;
        }
        // A segment shorter than its corners' reach turns a face inside
        // out: the wall overlaps itself there.
        for i in 0..g.segs.len() {
            let folded = [h, -h]
                .iter()
                .any(|&w| g.joint_u(i, true, w, 0.0) - g.joint_u(i, false, w, 0.0) < 1e-4);
            if folded {
                let p = g.segs[i].a + g.segs[i].d * (g.segs[i].len * 0.5);
                g.warnings.push(format!(
                    "the wall is too thick for its corners or curve near ({:.2}, {:.2}): it overlaps itself there",
                    p.x, p.y
                ));
                break;
            }
        }
        g.validate_openings(run, line, placed);
        if p.ao > 0.0 {
            g.v_breaks = g.make_v_breaks();
            g.u_breaks = g.make_u_breaks();
        }
        g
    }

    fn total(&self) -> f32 {
        self.u_end - self.u_start
    }

    fn seg_of(&self, u: f32) -> usize {
        for (i, s) in self.segs.iter().enumerate() {
            if u <= s.u0 + s.len + 1e-5 {
                return i;
            }
        }
        self.segs.len() - 1
    }

    fn joint_index(&self, seg: usize, end: bool) -> usize {
        if end {
            if self.closed {
                (seg + 1) % self.segs.len()
            } else {
                seg + 1
            }
        } else {
            seg
        }
    }

    /// Local u (from the segment start) where the offset line at `w` meets
    /// a joint line; `shift` applies the joint's band shift.
    fn joint_u(&self, seg: usize, end: bool, w: f32, shift: f32) -> f32 {
        let s = &self.segs[seg];
        let j = &self.joints[self.joint_index(seg, end)];
        let mut pt = j.pt;
        if shift != 0.0 && j.band_shift != 0.0 {
            let mut ln = perp(j.dir);
            let toward = if end { -s.d } else { s.d };
            if ln.dot(toward) < 0.0 {
                ln = -ln;
            }
            pt += ln * j.band_shift;
        }
        let o = s.a + s.n * w;
        let c = cross2(s.d, j.dir);
        if c.abs() < 1e-6 {
            return if end { s.len } else { 0.0 };
        }
        cross2(pt - o, j.dir) / c
    }

    /// Plan point for (U, w); at joints the joint lines are used so
    /// neighbouring segments agree exactly.
    fn plan(&self, u: f32, w: f32, shift: f32) -> Vec2 {
        let i = self.seg_of(u);
        let s = &self.segs[i];
        let lu = u - s.u0;
        let lu = if lu.abs() < 1e-5 {
            self.joint_u(i, false, w, shift)
        } else if (lu - s.len).abs() < 1e-5 {
            self.joint_u(i, true, w, shift)
        } else {
            self.face_u(i, lu, w)
        };
        s.a + s.d * lu + s.n * w
    }

    /// Where segment-local U `lu` lies along the face line at `w`. Inside
    /// a segment the mapping is perpendicular (S6, J1: openings near hard
    /// corners do not skew); towards a smooth joint (arcs, splines) it
    /// blends to the joint line, so a face on the concave side of a curve
    /// and its neighbour cut an opening straddling the joint identically.
    fn face_u(&self, i: usize, lu: f32, w: f32) -> f32 {
        let (a, b) = self.face_offsets(i, w);
        let len = self.segs[i].len.max(1e-6);
        lu + a * (1.0 - lu / len) + b * (lu / len)
    }

    /// The inverse of [`face_u`](Self::face_u).
    fn face_u_inv(&self, i: usize, f: f32, w: f32) -> f32 {
        let (a, b) = self.face_offsets(i, w);
        let len = self.segs[i].len.max(1e-6);
        (f - a) / (1.0 + (b - a) / len)
    }

    /// The face's offsets at smooth ends (0 at hard ones).
    fn face_offsets(&self, i: usize, w: f32) -> (f32, f32) {
        let s = &self.segs[i];
        let a = if self.joints[self.joint_index(i, false)].smooth {
            self.joint_u(i, false, w, 0.0)
        } else {
            0.0
        };
        let b = if self.joints[self.joint_index(i, true)].smooth {
            self.joint_u(i, true, w, 0.0) - s.len
        } else {
            0.0
        };
        (a, b)
    }

    fn map(&self, u: f32, v: f32, w: f32, shift: f32) -> Vec3 {
        to3(self.plan(u, w, shift), v)
    }

    fn shade_n(&self, u: f32) -> Vec3 {
        self.shade_n_in(self.seg_of(u), u)
    }

    /// Shading normal at U within segment `i`.
    fn shade_n_in(&self, i: usize, u: f32) -> Vec3 {
        let s = &self.segs[i];
        let t = ((u - s.u0) / s.len).clamp(0.0, 1.0);
        let js = &self.joints[self.joint_index(i, false)];
        let je = &self.joints[self.joint_index(i, true)];
        let ns = if js.smooth { js.ns } else { s.n };
        let ne = if je.smooth { je.ns } else { s.n };
        to3(ns.lerp(ne, t).normalize_or(s.n), 0.0)
    }

    fn top_pt(&self, p: Vec2) -> f32 {
        self.p.top_pt(p)
    }

    fn top(&self, u: f32, w: f32) -> f32 {
        self.top_pt(self.plan(u, w, 0.0))
    }

    /// Lowest top along both faces.
    fn top_min(&self) -> f32 {
        if self.p.roof.is_none() {
            return self.p.height;
        }
        let h = self.p.t * 0.5;
        let mut m = f32::MAX;
        for (i, s) in self.segs.iter().enumerate() {
            let steps = if self.curved_top {
                ((s.len / TOP_SAMPLE_STEP).ceil() as usize).clamp(1, 400)
            } else {
                1
            };
            for w in [h, -h] {
                let mut us: Vec<f32> = (0..=steps)
                    .map(|k| s.len * k as f32 / steps as f32)
                    .collect();
                us.extend(self.kinks(i, w));
                for lu in us {
                    let lu = lu.clamp(0.0, s.len);
                    m = m.min(self.top_pt(s.a + s.d * lu + s.n * w));
                }
            }
        }
        m
    }

    /// Local u where the top along face line (seg, w) has kinks (two roof
    /// planes cross), or regular samples on curved undersides.
    fn kinks(&self, i: usize, w: f32) -> Vec<f32> {
        let Some(r) = self.p.roof else {
            return Vec::new();
        };
        let s = self.segs[i];
        let o = s.a + s.n * w;
        let mut out = Vec::new();
        if self.curved_top {
            let steps = ((s.len / TOP_SAMPLE_STEP).ceil() as usize).clamp(1, 400);
            for k in 1..steps {
                out.push(s.len * k as f32 / steps as f32);
            }
        }
        let lin: Vec<(f32, f32)> = r
            .surfaces
            .iter()
            .filter_map(|surf| surf.linear_along(o, s.d))
            .collect();
        for a in 0..lin.len() {
            for b in a + 1..lin.len() {
                let (aa, ab) = lin[a];
                let (ba, bb) = lin[b];
                if (ab - bb).abs() < 1e-6 {
                    continue;
                }
                let u = (ba - aa) / (ab - bb);
                let y = aa + ab * u + r.lift;
                if (y - r.at(o + s.d * u)).abs() < 1e-4 {
                    out.push(u);
                }
            }
        }
        out
    }

    /// Where a joint's occlusion and clearances are centred: the visible
    /// junction (for Extend ends, where the wall emerges from the solid).
    fn joint_vis_u(&self, k: usize) -> f32 {
        let last = self.joints.len() - 1;
        if !self.closed && k == 0 {
            if let WallEnd::Extend { distance } = self.start {
                return self.u_start + distance;
            }
            return self.u_start;
        }
        if !self.closed && k == last {
            if let WallEnd::Extend { distance } = self.end {
                return self.u_end - distance;
            }
            return self.u_end;
        }
        self.segs[k].u0
    }

    fn validate_openings(&mut self, run: &Run, line: &Centreline, placed: &mut [bool]) {
        let t = self.p.t;
        let line_total = line.length();
        let top_vis = self.top_min() - self.p.lift();
        let mut keep: Vec<Hole> = Vec::new();
        let mut order: Vec<usize> = (0..self.p.w.openings.len()).collect();
        order.sort_by(|&a, &b| {
            self.p.w.openings[a]
                .offset
                .total_cmp(&self.p.w.openings[b].offset)
        });
        let (vis_lo, vis_hi) = (self.joint_vis_u(0), self.joint_vis_u(self.joints.len() - 1));
        for idx in order {
            if placed[idx] {
                continue;
            }
            let o = self.p.w.openings[idx];
            let at = format!("opening {idx} (at U={:.2})", o.offset);
            if !(o.offset.is_finite()
                && o.width.is_finite()
                && o.height.is_finite()
                && o.sill.is_finite())
                || o.width < MIN_OPENING
                || o.height < MIN_OPENING
                || o.sill < 0.0
            {
                placed[idx] = true;
                self.warnings.push(format!(
                    "{at}: width and height must be at least {MIN_OPENING} m and the sill 0 or more; dropped"
                ));
                continue;
            }
            // Into run-local U.
            let mut u = o.offset - run.u0;
            if line.closed && run.u0 > 0.0 && u < -1e-5 {
                u += line_total;
            }
            let mut hole = Hole {
                kind: o.kind,
                top: o.top,
                u,
                width: o.width,
                sill: o.sill,
                height: o.height,
                at_foot: o.kind != OpeningKind::Window && o.sill < 1e-3,
            };
            if hole.at_foot {
                hole.sill = 0.0;
            }
            // Openings belong to the run that holds their centre.
            if !(u >= self.u_start - 1e-4 && u <= self.u_end + 1e-4) {
                continue;
            }
            placed[idx] = true;
            let (l, r) = (hole.l(), hole.r());
            let mut bad = None;
            if self.closed && (l < 0.0 || r > self.total()) {
                bad = Some(format!(
                    "{at} crosses the start of a closed wall; move it or start the loop elsewhere"
                ));
            }
            if !self.closed && (l < vis_lo || r > vis_hi) {
                bad = Some(format!("{at} runs past the end of the wall"));
            }
            for (k, j) in self.joints.iter().enumerate() {
                let is_end = !self.closed && (k == 0 || k == self.joints.len() - 1);
                if !(j.hard || is_end) {
                    continue;
                }
                let ju = self.joint_vis_u(k);
                let reach = if is_end && j.free {
                    0.0
                } else {
                    t * 0.5 * j.dir.length().max(1.0)
                };
                let margin = reach + OPENING_CORNER_MARGIN;
                let dist = |x: f32| {
                    let dd = (x - ju).abs();
                    if self.closed {
                        dd.min(self.total() - dd)
                    } else {
                        dd
                    }
                };
                let straddles = ju > l && ju < r;
                if dist(l) < margin || dist(r) < margin || straddles {
                    bad = Some(format!(
                        "{at} is too close to the corner or end at U={:.2} (keep {margin:.2} m clear)",
                        ju + run.u0
                    ));
                }
            }
            if hole.head() > top_vis - OPENING_TOP_MARGIN {
                bad = Some(format!(
                    "{at} is too tall: its head must stay {OPENING_TOP_MARGIN} m below the top ({top_vis:.2} m)"
                ));
            }
            if keep.iter().any(|k| {
                k.r() + OPENING_GAP > l
                    && k.l() - OPENING_GAP < r
                    && k.sill < hole.head() + OPENING_GAP
                    && hole.sill < k.head() + OPENING_GAP
            }) {
                bad = Some(format!(
                    "{at} overlaps another opening (keep {OPENING_GAP} m apart)"
                ));
            }
            if bad.is_none() && hole.top != OpeningTop::Flat && hole.spring() < hole.sill + MIN_JAMB
            {
                self.warnings.push(format!(
                    "{at} is too short for its arched head; drawn with a flat head"
                ));
                hole.top = OpeningTop::Flat;
            }
            match bad {
                Some(w) => self.warnings.push(format!("{w}; dropped")),
                None => keep.push(hole),
            }
        }
        self.holes = keep;
    }

    fn make_v_breaks(&self) -> Vec<f32> {
        let top = self.top_min();
        let mut v = vec![BASE_AO_HEIGHT];
        if self.p.roof.is_some() {
            v.push(top - EAVE_AO_DEPTH);
        }
        v.retain(|&x| x > 0.05 && x < top - 0.05);
        // An occlusion line a hair away from an opening's edge would slice
        // off sub-millimetre slivers: put it on the edge instead.
        let features: Vec<f32> = self
            .holes
            .iter()
            .flat_map(|o| self.opening_poly(o, false))
            .map(|p| p.y)
            .collect();
        for x in &mut v {
            *x = snap_break(*x, &features);
        }
        v.sort_by(f32::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < 0.05);
        v
    }

    fn make_u_breaks(&self) -> Vec<f32> {
        let mut u = Vec::new();
        for (k, j) in self.joints.iter().enumerate() {
            if !(j.ao_pos || j.ao_neg) {
                continue;
            }
            let ju = self.joint_vis_u(k);
            for du in [-CORNER_AO_BREAK, CORNER_AO_BREAK] {
                let x = ju + du;
                if self.closed {
                    u.push(x.rem_euclid(self.total()));
                } else if x > self.u_start && x < self.u_end {
                    u.push(x);
                }
            }
        }
        let mut features: Vec<f32> = self
            .holes
            .iter()
            .flat_map(|o| self.opening_poly(o, false))
            .map(|p| p.x)
            .collect();
        features.extend(self.segs.iter().map(|s| s.u0));
        for x in &mut u {
            *x = snap_break(*x, &features);
        }
        u.sort_by(f32::total_cmp);
        u
    }

    /// Occlusion factor at (U, v) on side `side` (+1 outer, −1 inner, 0 for
    /// end faces and reveals).
    fn ao(&self, u: f32, v: f32, side: f32) -> f32 {
        if self.p.ao <= 0.0 {
            return 1.0;
        }
        let mut a = 1.0 - BASE_AO * (1.0 - smoothstep(0.0, BASE_AO_HEIGHT, v));
        if self.p.roof.is_some() {
            let top = self.top(u, side * self.p.t * 0.5);
            a *= 1.0 - EAVE_AO * smoothstep(top - EAVE_AO_DEPTH, top, v);
        }
        for (k, j) in self.joints.iter().enumerate() {
            let on = (side > 0.0 && j.ao_pos) || (side < 0.0 && j.ao_neg);
            if !on {
                continue;
            }
            let ju = self.joint_vis_u(k);
            let mut dd = (u - ju).abs();
            if self.closed {
                dd = dd.min(self.total() - dd);
            }
            a *= 1.0 - CORNER_AO * pmath::exp(-dd / CORNER_AO_FALLOFF);
        }
        a
    }

    // ------------------------------------------------------------- build --

    fn build(&self, mb: &mut ArchMesh) {
        for i in 0..self.segs.len() {
            for side in [1.0f32, -1.0] {
                self.face(mb, i, side);
            }
            self.top_cap(mb, i);
        }
        if !self.closed {
            for k in [0usize, self.joints.len() - 1] {
                if self.joints[k].cap {
                    self.end_cap(mb, k == 0);
                }
            }
        }
        for o in &self.holes {
            self.reveals(mb, o);
            self.panel(mb, o);
            self.opening_trim(mb, o);
        }
        if self.p.w.plinth {
            self.plinth(mb);
        }
        if self.p.w.cornice {
            let top = self.top_min() - self.p.lift();
            self.band(
                mb,
                &Band {
                    u0: self.u_start,
                    u1: self.u_end,
                    v0: top - CORNICE_HEIGHT,
                    v1: top,
                    proud: CORNICE_PROUD,
                    both_sides: false,
                    shift_at_butt: false,
                },
                self.p.t * 0.5,
            );
        }
        if self.p.w.coping {
            if self.p.roof.is_some() {
                mb.warn("coping needs a flat top; ignored under a roof");
            } else {
                let h = self.p.height;
                self.band(
                    mb,
                    &Band {
                        u0: self.u_start,
                        u1: self.u_end,
                        v0: h - COPING_DROP,
                        v1: h + COPING_RISE,
                        proud: COPING_PROUD,
                        both_sides: true,
                        shift_at_butt: false,
                    },
                    0.0,
                );
            }
        }
    }

    fn opening_poly(&self, o: &Hole, cut: bool) -> Vec<Vec2> {
        let (l, r) = (o.l(), o.r());
        let bottom = if o.at_foot && cut { -DOOR_SINK } else { o.sill };
        let head = o.head();
        let spring = o.spring();
        let mut p = vec![Vec2::new(l, bottom), Vec2::new(r, bottom)];
        match o.top {
            OpeningTop::Flat => {
                p.push(Vec2::new(r, head));
                p.push(Vec2::new(l, head));
            }
            OpeningTop::Round => {
                let rad = o.width * 0.5;
                p.push(Vec2::new(r, spring));
                for k in 1..ARCH_SEGMENTS {
                    let a = std::f32::consts::PI * k as f32 / ARCH_SEGMENTS as f32;
                    p.push(Vec2::new(
                        o.u + pmath::cos(a) * rad,
                        spring + pmath::sin(a) * rad,
                    ));
                }
                p.push(Vec2::new(l, spring));
            }
            OpeningTop::Pointed => {
                // Two arcs of radius = width, centred on the opposite jamb.
                let rad = o.width;
                let sweep = std::f32::consts::FRAC_PI_3;
                p.push(Vec2::new(r, spring));
                for k in 1..ARCH_SEGMENTS {
                    let a = sweep * k as f32 / ARCH_SEGMENTS as f32;
                    p.push(Vec2::new(
                        l + pmath::cos(a) * rad,
                        spring + pmath::sin(a) * rad,
                    ));
                }
                p.push(Vec2::new(o.u, head));
                for k in (1..ARCH_SEGMENTS).rev() {
                    let a = sweep * k as f32 / ARCH_SEGMENTS as f32;
                    p.push(Vec2::new(
                        r - pmath::cos(a) * rad,
                        spring + pmath::sin(a) * rad,
                    ));
                }
                p.push(Vec2::new(l, spring));
            }
        }
        p
    }

    fn face(&self, mb: &mut ArchMesh, i: usize, side: f32) {
        let s = self.segs[i];
        let w = side * self.p.t * 0.5;
        let ua = self.joint_u(i, false, w, 0.0);
        let ub = self.joint_u(i, true, w, 0.0);
        if ub - ua < 1e-5 {
            return;
        }
        let tf = |u: f32| self.top_pt(s.a + s.d * u + s.n * w);
        let mut outline = vec![Vec2::new(ua, 0.0), Vec2::new(ub, 0.0)];
        outline.push(Vec2::new(ub, tf(ub)));
        let mut ks = self.top_breaks(i, w, ua, ub);
        ks.reverse();
        for k in ks {
            // A break where an occlusion line crosses a sloped top sits
            // exactly on that line, so slicing adds no sliver next to it.
            let y = tf(k);
            let y = self
                .v_breaks
                .iter()
                .copied()
                .find(|vb| (vb - y).abs() < 1e-4)
                .unwrap_or(y);
            outline.push(Vec2::new(k, y));
        }
        outline.push(Vec2::new(ua, tf(ua)));
        // Holes: openings clipped to this segment; clipped edges map onto
        // the joint lines.
        let mut holes = Vec::new();
        for o in &self.holes {
            let (l, r) = (o.l(), o.r());
            if r <= s.u0 + 1e-5 || l >= s.u0 + s.len - 1e-5 {
                continue;
            }
            let mut poly = self.opening_poly(o, true);
            let cut_lo = l < s.u0 - 1e-5;
            let cut_hi = r > s.u0 + s.len + 1e-5;
            if cut_lo {
                poly = clip::clip_convex(&poly, 0, s.u0, true);
            }
            if cut_hi {
                poly = clip::clip_convex(&poly, 0, s.u0 + s.len, false);
            }
            let local: Vec<Vec2> = poly
                .iter()
                .map(|p| {
                    let lu = p.x - s.u0;
                    let lu = if cut_lo && lu.abs() < 1e-5 {
                        ua
                    } else if cut_hi && (lu - s.len).abs() < 1e-5 {
                        ub
                    } else {
                        self.face_u(i, lu, w)
                    };
                    Vec2::new(lu, p.y)
                })
                .collect();
            holes.push(local);
        }
        let mut tris: Vec<Tri> = clip::difference_tris(&outline, &holes);
        for &vb in &self.v_breaks {
            tris = clip::slice_tris(tris, 1, vb);
        }
        for &ub_ in &self.u_breaks {
            let lu = self.face_u(i, ub_ - s.u0, w);
            if lu > ua + 1e-4 && lu < ub - 1e-4 {
                tris = clip::slice_tris(tris, 0, lu);
            }
        }
        let facing = to3(s.n * side, 0.0);
        for t in tris {
            let v = t.map(|q| {
                let u = s.u0 + self.face_u_inv(i, q.x, w);
                let plan = s.a + s.d * q.x + s.n * w;
                V {
                    p: to3(plan, q.y),
                    n: self.shade_n_in(i, u.clamp(s.u0, s.u0 + s.len)) * side,
                    ao: self.ao(u, q.y, side),
                    uv: Vec2::new(u * side, -q.y),
                    uv1: Vec2::new(q.y, (tf(q.x) - q.y).max(0.0)),
                }
            });
            mb.tri_v(slot::WALL, v, facing);
        }
    }

    /// Points along the top edge of the face at `w` (in face-line u) that
    /// the face and the top cap share: roof kinks, occlusion breaks, and
    /// where occlusion breaks cross a sloped top.
    fn top_breaks(&self, i: usize, w: f32, lo: f32, hi: f32) -> Vec<f32> {
        let s = self.segs[i];
        let mut b: Vec<f32> = Vec::new();
        let kinks = self.kinks(i, w);
        b.extend(&kinks);
        b.extend(self.u_breaks.iter().map(|u| self.face_u(i, u - s.u0, w)));
        if self.p.roof.is_some() {
            let ua = self.joint_u(i, false, w, 0.0);
            let ub = self.joint_u(i, true, w, 0.0);
            let mut pts = vec![ua];
            pts.extend(kinks.iter().copied().filter(|&k| k > ua && k < ub));
            pts.push(ub);
            pts.sort_by(f32::total_cmp);
            let tf = |u: f32| self.top_pt(s.a + s.d * u + s.n * w);
            for e in pts.windows(2) {
                let (ya, yb) = (tf(e[0]), tf(e[1]));
                for &vb in &self.v_breaks {
                    if (ya - vb) * (yb - vb) < 0.0 {
                        b.push(e[0] + (e[1] - e[0]) * (vb - ya) / (yb - ya));
                    }
                }
            }
        }
        b.retain(|&x| x > lo + 1e-4 && x < hi - 1e-4);
        b.sort_by(f32::total_cmp);
        b.dedup_by(|a, c| (*a - *c).abs() < 1e-5);
        b
    }

    fn top_cap(&self, mb: &mut ArchMesh, i: usize) {
        let s = self.segs[i];
        let h = self.p.t * 0.5;
        let (ao_, bo) = (
            self.joint_u(i, false, h, 0.0),
            self.joint_u(i, true, h, 0.0),
        );
        let (ai, bi) = (
            self.joint_u(i, false, -h, 0.0),
            self.joint_u(i, true, -h, 0.0),
        );
        // Each line keeps every break in its own range (the faces use the
        // same breaks), and the strip between them is zipped by U.
        let mut uo = vec![ao_];
        uo.extend(self.top_breaks(i, h, ao_, bo));
        uo.push(bo);
        let mut ui = vec![ai];
        ui.extend(self.top_breaks(i, -h, ai, bi));
        ui.push(bi);
        let pt = |u: f32, w: f32| {
            let q = s.a + s.d * u + s.n * w;
            to3(q, self.top_pt(q))
        };
        let (mut a, mut b) = (0, 0);
        while a + 1 < uo.len() || b + 1 < ui.len() {
            let outer = b + 1 == ui.len() || (a + 1 < uo.len() && uo[a + 1] <= ui[b + 1]);
            let tri = if outer {
                a += 1;
                [pt(uo[a - 1], h), pt(uo[a], h), pt(ui[b], -h)]
            } else {
                b += 1;
                [pt(uo[a], h), pt(ui[b], -h), pt(ui[b - 1], -h)]
            };
            mb.tri(slot::TOP, tri, [1.0; 3], Vec3::Y);
        }
    }

    fn end_cap(&self, mb: &mut ArchMesh, start: bool) {
        let h = self.p.t * 0.5;
        let u = if start { self.u_start } else { self.u_end };
        let mut levels = vec![0.0];
        levels.extend(&self.v_breaks);
        let s = if start {
            self.segs[0]
        } else {
            self.segs[self.segs.len() - 1]
        };
        let facing = to3(if start { -s.d } else { s.d }, 0.0);
        let (to, ti) = (self.top(u, h), self.top(u, -h));
        for k in 0..levels.len() {
            let v0 = levels[k];
            let (v1o, v1i) = if k + 1 < levels.len() {
                (levels[k + 1], levels[k + 1])
            } else {
                (to, ti)
            };
            let q = [
                self.map(u, v0, h, 0.0),
                self.map(u, v0, -h, 0.0),
                self.map(u, v1i, -h, 0.0),
                self.map(u, v1o, h, 0.0),
            ];
            let ao = [
                self.ao(u, v0, 0.0),
                self.ao(u, v0, 0.0),
                self.ao(u, v1i, 0.0),
                self.ao(u, v1o, 0.0),
            ];
            mb.quad(slot::WALL, q, ao, facing);
        }
    }

    /// U positions where reveal and panel geometry is split to match the
    /// faces.
    fn split_us(&self) -> Vec<f32> {
        let mut c: Vec<f32> = self.segs.iter().skip(1).map(|s| s.u0).collect();
        c.extend(&self.u_breaks);
        c.sort_by(f32::total_cmp);
        c
    }

    /// The panel plane's w.
    fn panel_w(&self, o: &Hole) -> f32 {
        let t = self.p.t;
        let setback = match o.kind {
            OpeningKind::Window => WINDOW_SETBACK,
            OpeningKind::Door => DOOR_SETBACK,
            OpeningKind::Passage => 0.5,
        };
        t * 0.5 - t * setback
    }

    fn reveals(&self, mb: &mut ArchMesh, o: &Hole) {
        let h = self.p.t * 0.5;
        let wp = self.panel_w(o);
        let mut contour = self.opening_poly(o, false);
        if o.at_foot {
            // No sill reveal: start at the bottom right, end at the bottom left.
            contour.rotate_left(1);
        } else {
            contour.push(contour[0]);
        }
        let cuts_u = self.split_us();
        let mut pts: Vec<Vec2> = Vec::new();
        for e in contour.windows(2) {
            let (a, b) = (e[0], e[1]);
            let mut ts: Vec<Vec2> = Vec::new();
            for &c in &cuts_u {
                if (a.x - c) * (b.x - c) < 0.0 {
                    ts.push(clip::cross_at(a, b, 0, c));
                }
            }
            for &c in &self.v_breaks {
                if (a.y - c) * (b.y - c) < 0.0 {
                    ts.push(clip::cross_at(a, b, 1, c));
                }
            }
            ts.sort_by(|p, q| (*p - a).length().total_cmp(&(*q - a).length()));
            pts.push(a);
            pts.extend(ts);
        }
        if let Some(last) = contour.last() {
            pts.push(*last);
        }
        let centre = Vec2::new(o.u, o.sill + o.height * 0.5);
        let c3 = self.map(centre.x, centre.y, wp, 0.0);
        let depth_ao = |w: f32| {
            if (w - wp).abs() < 1e-4 {
                REVEAL_AO
            } else {
                1.0
            }
        };
        for e in pts.windows(2) {
            let (a, b) = (e[0], e[1]);
            if (a - b).length_squared() < 1e-10 {
                continue;
            }
            for (w0, w1) in [(h, wp), (wp, -h)] {
                let q = [
                    self.map(a.x, a.y, w0, 0.0),
                    self.map(b.x, b.y, w0, 0.0),
                    self.map(b.x, b.y, w1, 0.0),
                    self.map(a.x, a.y, w1, 0.0),
                ];
                let mid = (q[0] + q[1] + q[2] + q[3]) * 0.25;
                let ao = [
                    depth_ao(w0) * self.ao(a.x, a.y, 0.0),
                    depth_ao(w0) * self.ao(b.x, b.y, 0.0),
                    depth_ao(w1) * self.ao(b.x, b.y, 0.0),
                    depth_ao(w1) * self.ao(a.x, a.y, 0.0),
                ];
                mb.quad(slot::REVEAL, q, ao, c3 - mid);
            }
        }
    }

    fn panel(&self, mb: &mut ArchMesh, o: &Hole) {
        let s_slot = match o.kind {
            OpeningKind::Window => slot::GLASS,
            OpeningKind::Door => slot::DOOR,
            OpeningKind::Passage => return,
        };
        let wp = self.panel_w(o);
        let pieces = clip::split_convex(self.opening_poly(o, false), 0, &self.split_us());
        let mut vb = Vec::new();
        for p in pieces {
            vb.extend(clip::split_convex(p, 1, &self.v_breaks));
        }
        for piece in vb {
            for side in [1.0f32, -1.0] {
                for k in 1..piece.len() - 1 {
                    let t = [piece[0], piece[k], piece[k + 1]];
                    let p3 = t.map(|q| self.map(q.x, q.y, wp, 0.0));
                    let n3 = t.map(|q| self.shade_n(q.x) * side);
                    let ao = t.map(|q| 0.9 * self.ao(q.x, q.y, 0.0).sqrt());
                    mb.tri_n(s_slot, p3, n3, ao, n3[0]);
                }
            }
        }
    }

    /// A projecting sill under windows, a threshold under doors, and glazing
    /// bars.
    fn opening_trim(&self, mb: &mut ArchMesh, o: &Hole) {
        let h = self.p.t * 0.5;
        let wp = self.panel_w(o);
        let (l, r) = (o.l(), o.r());
        match o.kind {
            OpeningKind::Passage => {}
            OpeningKind::Door => self.band(
                mb,
                &Band {
                    u0: l - THRESHOLD_OVERRUN,
                    u1: r + THRESHOLD_OVERRUN,
                    v0: o.sill,
                    v1: o.sill + THRESHOLD_HEIGHT,
                    proud: THRESHOLD_PROUD,
                    both_sides: false,
                    shift_at_butt: false,
                },
                -h + 0.02,
            ),
            OpeningKind::Window => {
                self.band(
                    mb,
                    &Band {
                        u0: l - SILL_OVERRUN,
                        u1: r + SILL_OVERRUN,
                        v0: o.sill - SILL_HEIGHT,
                        v1: o.sill,
                        proud: SILL_PROUD,
                        both_sides: false,
                        shift_at_butt: false,
                    },
                    h,
                );
                let top = o.spring();
                let a = self.map(o.u, o.sill, wp, 0.0);
                let b = self.map(o.u, o.head(), wp, 0.0);
                let n = self.shade_n(o.u);
                mb.beam(
                    slot::TRIM,
                    a - n * (GLAZING_BAR_DEPTH * 0.5),
                    b - n * (GLAZING_BAR_DEPTH * 0.5),
                    GLAZING_BAR_WIDTH,
                    GLAZING_BAR_DEPTH,
                    n,
                    0.85,
                );
                // The horizontal bar only on a straight stretch.
                let seg = self.seg_of(o.u);
                if self.seg_of(l + 0.01) == seg && self.seg_of(r - 0.01) == seg {
                    let mid_v = o.sill + (top - o.sill) * GLAZING_BAR_RISE;
                    let c = self.map(l, mid_v, wp, 0.0);
                    let d = self.map(r, mid_v, wp, 0.0);
                    mb.beam(
                        slot::TRIM,
                        c - Vec3::Y * (GLAZING_BAR_WIDTH * 0.5),
                        d - Vec3::Y * (GLAZING_BAR_WIDTH * 0.5),
                        GLAZING_BAR_DEPTH,
                        GLAZING_BAR_WIDTH,
                        Vec3::Y,
                        0.85,
                    );
                }
            }
        }
    }

    /// A box in (U, v, w) swept along the wall; `w_in` is its inner w when
    /// one-sided.
    fn band(&self, mb: &mut ArchMesh, b: &Band, w_in: f32) {
        let h = self.p.t * 0.5;
        let w_hi = h + b.proud;
        let w_lo = if b.both_sides { -h - b.proud } else { w_in };
        let shift = if b.shift_at_butt { b.proud } else { 0.0 };
        let (u0, u1) = (b.u0.max(self.u_start), b.u1.min(self.u_end));
        if u1 - u0 < 1e-4 || b.v1 - b.v0 < 1e-4 {
            return;
        }
        let mut cuts: Vec<f32> = self.segs.iter().map(|s| s.u0).collect();
        cuts.push(self.u_end);
        let mut us = vec![u0];
        us.extend(
            cuts.iter()
                .copied()
                .filter(|&c| c > u0 + 1e-5 && c < u1 - 1e-5),
        );
        us.push(u1);
        let foot = |v: f32| {
            if v < 0.01 && self.p.ao > 0.0 {
                0.6
            } else {
                1.0
            }
        };
        for k in 0..us.len() - 1 {
            let (ua, ub) = (us[k], us[k + 1]);
            let m = |u: f32, v: f32, w: f32| self.map(u, v, w, shift);
            for (w, sgn) in [(w_hi, 1.0f32), (w_lo, -1.0)] {
                if sgn < 0.0 && !b.both_sides {
                    continue;
                }
                let si = self.seg_of((ua + ub) * 0.5);
                let na = self.shade_n_in(si, ua) * sgn;
                let nb = self.shade_n_in(si, ub) * sgn;
                let p = [
                    m(ua, b.v0, w),
                    m(ub, b.v0, w),
                    m(ub, b.v1, w),
                    m(ua, b.v1, w),
                ];
                let ao = [foot(b.v0), foot(b.v0), 1.0, 1.0];
                mb.tri_n(
                    slot::TRIM,
                    [p[0], p[1], p[2]],
                    [na, nb, nb],
                    [ao[0], ao[1], ao[2]],
                    na,
                );
                mb.tri_n(
                    slot::TRIM,
                    [p[0], p[2], p[3]],
                    [na, nb, na],
                    [ao[0], ao[2], ao[3]],
                    na,
                );
            }
            let top = [
                m(ua, b.v1, w_lo),
                m(ub, b.v1, w_lo),
                m(ub, b.v1, w_hi),
                m(ua, b.v1, w_hi),
            ];
            mb.quad(slot::TRIM, top, [1.0; 4], Vec3::Y);
            if b.v0 > 0.001 {
                let bot = [
                    m(ua, b.v0, w_lo),
                    m(ub, b.v0, w_lo),
                    m(ub, b.v0, w_hi),
                    m(ua, b.v0, w_hi),
                ];
                let a = if self.p.ao > 0.0 { 0.7 } else { 1.0 };
                mb.quad(slot::TRIM, bot, [a; 4], -Vec3::Y);
            }
        }
        // Caps where the band stops along the wall (not at joints that
        // continue or butt without a cap).
        for (u, start) in [(u0, true), (u1, false)] {
            let at_end = (start && u < self.u_start + 1e-5) || (!start && u > self.u_end - 1e-5);
            if at_end
                && (self.closed || !self.joints[if start { 0 } else { self.joints.len() - 1 }].cap)
            {
                continue;
            }
            let s = self.segs[self.seg_of(u)];
            let f = to3(if start { -s.d } else { s.d }, 0.0);
            let m = |v: f32, w: f32| self.map(u, v, w, shift);
            let q = [m(b.v0, w_lo), m(b.v0, w_hi), m(b.v1, w_hi), m(b.v1, w_lo)];
            mb.quad(slot::TRIM, q, [foot(b.v0), foot(b.v0), 1.0, 1.0], f);
        }
    }

    fn plinth(&self, mb: &mut ArchMesh) {
        let mut runs = vec![(self.u_start, self.u_end)];
        for o in self.holes.iter().filter(|o| o.at_foot) {
            let (l, r) = (o.l() - PLINTH_DOOR_GAP, o.r() + PLINTH_DOOR_GAP);
            let mut next = Vec::new();
            for (a, b) in runs {
                if r <= a || l >= b {
                    next.push((a, b));
                } else {
                    if l > a {
                        next.push((a, l));
                    }
                    if r < b {
                        next.push((r, b));
                    }
                }
            }
            runs = next;
        }
        for (a, b) in runs {
            self.band(
                mb,
                &Band {
                    u0: a,
                    u1: b,
                    v0: 0.0,
                    v1: PLINTH_HEIGHT.min(self.top_min() - 0.01),
                    proud: PLINTH_PROUD,
                    both_sides: true,
                    shift_at_butt: true,
                },
                0.0,
            );
        }
    }

    /// The prism filling the outside notch of a split (acute) corner.
    fn bevel_fill(&self, mb: &mut ArchMesh, tri: [Vec2; 3]) {
        let top = tri.iter().map(|p| self.top_pt(*p)).fold(f32::MAX, f32::min);
        let area = cross2(tri[1] - tri[0], tri[2] - tri[0]);
        if area.abs() < 1e-8 {
            return;
        }
        let centre = (tri[0] + tri[1] + tri[2]) / 3.0;
        for k in 0..3 {
            let (a, b) = (tri[k], tri[(k + 1) % 3]);
            let mid = (a + b) * 0.5;
            let out = to3(mid - centre, 0.0);
            let q = [to3(a, 0.0), to3(b, 0.0), to3(b, top), to3(a, top)];
            let ao = [self.ao(0.0, 0.0, 0.0), self.ao(0.0, 0.0, 0.0), 1.0, 1.0];
            mb.quad(slot::WALL, q, ao, out);
        }
        mb.tri(
            slot::TOP,
            [to3(tri[0], top), to3(tri[1], top), to3(tri[2], top)],
            [1.0; 3],
            Vec3::Y,
        );
    }

    /// One oriented box per segment for the collider hint.
    fn collider_boxes(&self) -> Vec<HintBox> {
        let h = self.p.t * 0.5;
        self.segs
            .iter()
            .filter(|s| s.len > 1e-4)
            .map(|s| {
                let ends = [s.a, s.a + s.d * s.len];
                let top = ends
                    .iter()
                    .flat_map(|p| [self.top_pt(*p + s.n * h), self.top_pt(*p - s.n * h)])
                    .fold(0.0f32, f32::max);
                let mid = s.a + s.d * (s.len * 0.5);
                HintBox {
                    center: to3(mid, top * 0.5),
                    half_extents: Vec3::new(s.len * 0.5, top * 0.5, h),
                    rotation: Quat::from_rotation_y(-pmath::atan2(s.d.y, s.d.x)),
                }
            })
            .collect()
    }
}

struct Band {
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    proud: f32,
    both_sides: bool,
    /// Stop at the neighbour's matching band at butt ends (plinths).
    shift_at_butt: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate_wall(w: &Wall) -> (box_core::mesh::MeshData, Vec<String>) {
        let out = w.build(None);
        (out.mesh.unwrap_or_default(), out.warnings)
    }

    /// Regression (S6): at a hard joint each face shades with its own
    /// segment's normal.
    #[test]
    fn corner_faces_shade_with_their_own_normal() {
        let w = Wall::through(&[Vec2::new(-3.2, 0.0), Vec2::ZERO, Vec2::new(0.0, 3.2)]);
        let (m, warnings) = generate_wall(&w);
        assert!(warnings.is_empty(), "{warnings:?}");
        let h = w.thickness * 0.5;
        // Second arm runs along +z; its outer (right) face is at x = -h.
        let mut seen = 0;
        for (pos, n) in m.positions.iter().zip(&m.normals) {
            if (pos.x + h).abs() < 1e-4
                && pos.z > h + 1e-3
                && pos.z < 3.0
                && pos.y > 0.4
                && pos.y < 2.0
            {
                assert!(n.x < -0.99, "{pos:?} {n:?}");
                seen += 1;
            }
        }
        assert!(seen > 0);
    }

    #[test]
    fn arcs_bulge_to_the_right() {
        let w = Wall {
            path: WallPath::Points {
                points: vec![
                    WallPoint::new(-2.0, 0.0).with_bulge(1.0),
                    WallPoint::new(2.0, 0.0),
                ],
                closed: false,
            },
            ..Wall::default()
        };
        let line = w.centreline(None).unwrap();
        // Travel +x, right is +z.
        let max_z = line.points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        assert!((max_z - 2.0).abs() < 0.05, "{max_z}");
    }
}
