//! Splines and paths.
//!
//! - [`Spline`] is parameter data: control points with optional tangents,
//!   roll and scale; a [`SplineMode`] (centripetal Catmull-Rom, cubic
//!   Bézier/Hermite, or polyline); open or closed.
//! - [`SplineCurve`] is its evaluator (built once with [`Spline::curve`]):
//!   position, derivative, roll and scale at a parameter, an
//!   [`ArcLengthTable`], sampling (uniform by length or adaptive flatness)
//!   and closest point.
//! - [`Path`] is what sweeps and placement consume: a list of [`Station`]s,
//!   each with an incoming and an outgoing [`Frame`]. Smooth stations (from
//!   a sampled curve) have equal frames; **corner** stations (polyline
//!   vertices) have different ones, and sweeps join them with mitre planes.
//!
//! The global curve parameter `t` runs from 0 to `segment_count()`; segment
//! `i` covers `[i, i + 1]` and goes from control point `i` to `i + 1`
//! (wrapping for closed splines). Frames use the engine's conventions (+Y
//! up, right-handed): `side = forward × up` points to the right of the
//! direction of travel.
//!
//! Robustness: non-finite control points and consecutive duplicates are
//! dropped when the curve is built; a spline with one point is a point and
//! one with none is empty. Nothing panics.
//!
//! **As a component.** [`Spline`] is also a saved, reflected component
//! (registered by [`crate::register_proc_types`]): its control points are in
//! the local space of its entity's `Transform`. Generators read it as a
//! declared input (`Inputs::component::<Spline>(uid)`, see
//! [`crate::sweep_gen::Sweep`]), so editing it regenerates every object that
//! depends on it; the editor's Edit Spline tool moves, inserts and deletes
//! its points.

use bevy_reflect::Reflect;
use box_core::pmath;
use box_core::reflect::{ReflectComponent, ReflectDefault, Step, Unit};
use glam::{Quat, Vec2, Vec3};

/// How control points are interpolated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum SplineMode {
    /// Centripetal Catmull-Rom (alpha 0.5): passes through every point, no
    /// cusps or self-intersections within a segment. Tangents are ignored.
    #[default]
    CatmullRom,
    /// Cubic Bézier given as Hermite tangents: segment `i` uses
    /// `points[i].out_tangent` and `points[i + 1].in_tangent` (handles at
    /// `position ± tangent / 3`). A missing tangent mirrors the other one
    /// of the same point, or falls back to the Catmull-Rom tangent.
    Bezier,
    /// Straight segments with sharp corners (walls, fences).
    Polyline,
}

/// One control point.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ControlPoint {
    /// Position in the spline entity's local space.
    #[reflect(@Unit("m"), @Step(0.01))]
    pub position: Vec3,
    /// Hermite derivative arriving at the point (Bézier mode; the handle
    /// sits at `position - in_tangent / 3`). None: mirrors `out_tangent`,
    /// else the Catmull-Rom tangent.
    pub in_tangent: Option<Vec3>,
    /// Hermite derivative leaving the point (Bézier mode; the handle sits
    /// at `position + out_tangent / 3`). None: mirrors `in_tangent`, else
    /// the Catmull-Rom tangent.
    pub out_tangent: Option<Vec3>,
    /// Rotation of the profile about the path direction, radians. Positive
    /// turns `up` toward `side` (clockwise, looking along the path).
    #[reflect(@Unit("rad"), @Step(0.01))]
    pub roll: f32,
    /// Profile scale: x across (width), y up (height).
    #[reflect(@Step(0.01))]
    pub scale: Vec2,
}

impl Default for ControlPoint {
    fn default() -> Self {
        Self::new(Vec3::ZERO)
    }
}

impl ControlPoint {
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            in_tangent: None,
            out_tangent: None,
            roll: 0.0,
            scale: Vec2::ONE,
        }
    }

    /// Same tangent in and out (a smooth Bézier point).
    pub fn with_tangent(mut self, tangent: Vec3) -> Self {
        self.in_tangent = Some(tangent);
        self.out_tangent = Some(tangent);
        self
    }

    /// Separate (broken) tangents.
    pub fn with_tangents(mut self, in_tangent: Vec3, out_tangent: Vec3) -> Self {
        self.in_tangent = Some(in_tangent);
        self.out_tangent = Some(out_tangent);
        self
    }

    pub fn with_roll(mut self, roll: f32) -> Self {
        self.roll = roll;
        self
    }

    pub fn with_scale(mut self, scale: Vec2) -> Self {
        self.scale = scale;
        self
    }
}

/// A 3D spline: control points, interpolation mode, open or closed. Plain
/// parameter data for geometry functions, and a saved component whose
/// points are in its entity's local space (see the module docs).
///
/// `Spline::default()` is the spline a new object starts with: four points
/// of a gentle S curve 4 m long, 0.15 m above the entity's origin. An empty
/// spline is [`Spline::new`].
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Spline {
    /// Control points, in order along the curve.
    pub points: Vec<ControlPoint>,
    /// Catmull-Rom (smooth through the points), Bézier (smooth, with
    /// tangent handles) or Polyline (straight segments, sharp corners).
    pub mode: SplineMode,
    /// The last point connects back to the first.
    pub closed: bool,
}

impl Default for Spline {
    fn default() -> Self {
        Self::from_positions(
            &[
                Vec3::new(-2.0, 0.15, 0.5),
                Vec3::new(-0.7, 0.15, -0.6),
                Vec3::new(0.7, 0.15, 0.6),
                Vec3::new(2.0, 0.15, -0.5),
            ],
            SplineMode::CatmullRom,
            false,
        )
    }
}

impl Spline {
    pub fn new(mode: SplineMode, closed: bool) -> Self {
        Self {
            points: Vec::new(),
            mode,
            closed,
        }
    }

    /// A spline through plain positions.
    pub fn from_positions(positions: &[Vec3], mode: SplineMode, closed: bool) -> Self {
        Self {
            points: positions.iter().map(|p| ControlPoint::new(*p)).collect(),
            mode,
            closed,
        }
    }

    /// The evaluator. Build once and reuse for repeated queries.
    pub fn curve(&self) -> SplineCurve {
        SplineCurve::new(self)
    }
}

/// One cubic Bézier segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicSegment {
    pub p: [Vec3; 4],
}

impl CubicSegment {
    pub fn position(&self, t: f32) -> Vec3 {
        let u = 1.0 - t;
        let [a, b, c, d] = self.p;
        a * (u * u * u) + b * (3.0 * u * u * t) + c * (3.0 * u * t * t) + d * (t * t * t)
    }

    pub fn derivative(&self, t: f32) -> Vec3 {
        let u = 1.0 - t;
        let [a, b, c, d] = self.p;
        (b - a) * (3.0 * u * u) + (c - b) * (6.0 * u * t) + (d - c) * (3.0 * t * t)
    }
}

/// Evaluator for a [`Spline`]. See the module docs for the parameter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SplineCurve {
    segments: Vec<CubicSegment>,
    /// Per segment (start, end) roll and scale.
    rolls: Vec<(f32, f32)>,
    scales: Vec<(Vec2, Vec2)>,
    mode: SplineMode,
    closed: bool,
    /// The only point of a one-point spline.
    single: Option<ControlPoint>,
}

const MIN_KNOT: f32 = 1e-4;

fn centripetal_tangents(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3) -> (Vec3, Vec3) {
    let d0 = (p1 - p0).length().sqrt().max(MIN_KNOT);
    let d1 = (p2 - p1).length().sqrt().max(MIN_KNOT);
    let d2 = (p3 - p2).length().sqrt().max(MIN_KNOT);
    let m1 = ((p1 - p0) / d0 - (p2 - p0) / (d0 + d1) + (p2 - p1) / d1) * d1;
    let m2 = ((p2 - p1) / d1 - (p3 - p1) / (d1 + d2) + (p3 - p2) / d2) * d1;
    (m1, m2)
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

impl SplineCurve {
    pub fn new(spline: &Spline) -> Self {
        let mut pts: Vec<ControlPoint> = Vec::with_capacity(spline.points.len());
        for p in &spline.points {
            if !p.position.is_finite() {
                continue;
            }
            if pts.last().is_some_and(|q| q.position == p.position) {
                continue;
            }
            let mut c = *p;
            c.roll = finite_or(c.roll, 0.0);
            c.scale = if c.scale.is_finite() {
                c.scale
            } else {
                Vec2::ONE
            };
            c.in_tangent = c.in_tangent.filter(|t| t.is_finite());
            c.out_tangent = c.out_tangent.filter(|t| t.is_finite());
            pts.push(c);
        }
        let mut closed = spline.closed;
        if closed && pts.len() > 2 && pts[0].position == pts[pts.len() - 1].position {
            pts.pop();
        }
        if pts.len() < 3 {
            closed = false;
        }
        let mut curve = SplineCurve {
            mode: spline.mode,
            closed,
            ..Default::default()
        };
        let n = pts.len();
        if n == 1 {
            curve.single = Some(pts[0]);
            return curve;
        }
        if n < 2 {
            return curve;
        }
        let segs = if closed { n } else { n - 1 };
        let at = |i: isize| -> Vec3 {
            if closed {
                pts[i.rem_euclid(n as isize) as usize].position
            } else if i < 0 {
                pts[0].position * 2.0 - pts[1].position
            } else if i as usize >= n {
                pts[n - 1].position * 2.0 - pts[n - 2].position
            } else {
                pts[i as usize].position
            }
        };
        for i in 0..segs {
            let (a, b) = (&pts[i], &pts[(i + 1) % n]);
            let (p0, p1, p2, p3) = (
                at(i as isize - 1),
                a.position,
                b.position,
                at(i as isize + 2),
            );
            let seg = match spline.mode {
                SplineMode::Polyline => {
                    let d = p2 - p1;
                    [p1, p1 + d / 3.0, p1 + d * (2.0 / 3.0), p2]
                }
                SplineMode::CatmullRom => {
                    let (m1, m2) = centripetal_tangents(p0, p1, p2, p3);
                    [p1, p1 + m1 / 3.0, p2 - m2 / 3.0, p2]
                }
                SplineMode::Bezier => {
                    let (c1, c2) = centripetal_tangents(p0, p1, p2, p3);
                    let m1 = a.out_tangent.or(a.in_tangent).unwrap_or(c1);
                    let m2 = b.in_tangent.or(b.out_tangent).unwrap_or(c2);
                    [p1, p1 + m1 / 3.0, p2 - m2 / 3.0, p2]
                }
            };
            curve.segments.push(CubicSegment { p: seg });
            curve.rolls.push((a.roll, b.roll));
            curve.scales.push((a.scale, b.scale));
        }
        curve
    }

    pub fn mode(&self) -> SplineMode {
        self.mode
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn segments(&self) -> &[CubicSegment] {
        &self.segments
    }

    /// Splits a global parameter into (segment, local t in 0..=1). Clamped
    /// for open curves, wrapped for closed ones.
    pub fn locate(&self, t: f32) -> (usize, f32) {
        let n = self.segments.len();
        if n == 0 {
            return (0, 0.0);
        }
        let t = finite_or(t, 0.0);
        let t = if self.closed {
            t.rem_euclid(n as f32)
        } else {
            t.clamp(0.0, n as f32)
        };
        let i = (t.floor() as usize).min(n - 1);
        (i, (t - i as f32).clamp(0.0, 1.0))
    }

    pub fn position(&self, t: f32) -> Vec3 {
        if self.segments.is_empty() {
            return self.single.map_or(Vec3::ZERO, |p| p.position);
        }
        let (i, u) = self.locate(t);
        self.segments[i].position(u)
    }

    /// dC/dt (per unit of the global parameter).
    pub fn derivative(&self, t: f32) -> Vec3 {
        if self.segments.is_empty() {
            return Vec3::ZERO;
        }
        let (i, u) = self.locate(t);
        self.segments[i].derivative(u)
    }

    /// Unit tangent; falls back to the chord direction where the derivative
    /// vanishes, and to −Z (forward) for a degenerate curve.
    pub fn tangent(&self, t: f32) -> Vec3 {
        if let Some(d) = self.derivative(t).try_normalize() {
            return d;
        }
        if self.segments.is_empty() {
            return Vec3::NEG_Z;
        }
        let (i, _) = self.locate(t);
        let s = &self.segments[i];
        (s.p[3] - s.p[0]).try_normalize().unwrap_or(Vec3::NEG_Z)
    }

    pub fn roll(&self, t: f32) -> f32 {
        if self.segments.is_empty() {
            return self.single.map_or(0.0, |p| p.roll);
        }
        let (i, u) = self.locate(t);
        let (a, b) = self.rolls[i];
        a + (b - a) * u
    }

    pub fn scale(&self, t: f32) -> Vec2 {
        if self.segments.is_empty() {
            return self.single.map_or(Vec2::ONE, |p| p.scale);
        }
        let (i, u) = self.locate(t);
        let (a, b) = self.scales[i];
        a.lerp(b, u)
    }

    /// Arc-length table with `per_segment` intervals per segment (clamped to
    /// 1..=1024), each integrated with 5-point Gauss-Legendre.
    pub fn arc_length_table(&self, per_segment: u32) -> ArcLengthTable {
        let k = per_segment.clamp(1, 1024) as usize;
        let mut t = vec![0.0f32];
        let mut s = vec![0.0f32];
        let mut acc = 0.0f64;
        for i in 0..self.segments.len() {
            for j in 0..k {
                let (a, b) = (
                    i as f32 + j as f32 / k as f32,
                    i as f32 + (j + 1) as f32 / k as f32,
                );
                acc += gauss_length(&self.segments[i], a - i as f32, b - i as f32);
                t.push(b);
                s.push(acc as f32);
            }
        }
        ArcLengthTable { t, s }
    }

    /// Total length (default table resolution).
    pub fn length(&self) -> f32 {
        self.arc_length_table(DEFAULT_TABLE).length()
    }

    /// Samples along the curve. Open curves include both ends; closed
    /// curves do not repeat the start. See [`Sampling`].
    pub fn sample(&self, sampling: Sampling) -> Vec<SplineSample> {
        let n = self.segments.len();
        if n == 0 {
            return self
                .single
                .map(|p| {
                    vec![SplineSample {
                        t: 0.0,
                        distance: 0.0,
                        position: p.position,
                        tangent: Vec3::NEG_Z,
                    }]
                })
                .unwrap_or_default();
        }
        let table = self.arc_length_table(DEFAULT_TABLE);
        let total = table.length();
        let mut ts: Vec<f32> = Vec::new();
        match sampling {
            Sampling::PerSegment(k) => {
                let k = k.clamp(1, 4096) as usize;
                for i in 0..n {
                    for j in 0..k {
                        ts.push(i as f32 + j as f32 / k as f32);
                    }
                }
            }
            Sampling::Uniform { step } => {
                let step = if step.is_finite() && step > 0.0 {
                    step
                } else {
                    total.max(1e-6)
                };
                if self.mode == SplineMode::Polyline {
                    // Corners are kept: each straight segment is divided evenly.
                    for i in 0..n {
                        let len = table.distance_at(i as f32 + 1.0) - table.distance_at(i as f32);
                        let k = ((len / step).ceil() as usize).clamp(1, MAX_SAMPLES / n.max(1));
                        for j in 0..k {
                            ts.push(i as f32 + j as f32 / k as f32);
                        }
                    }
                } else {
                    let k = ((total / step).ceil() as usize).clamp(1, MAX_SAMPLES);
                    for j in 0..k {
                        ts.push(table.param_at(total * j as f32 / k as f32, self));
                    }
                }
            }
            Sampling::Adaptive {
                tolerance,
                max_step,
            } => {
                let tol = if tolerance.is_finite() && tolerance > 0.0 {
                    tolerance
                } else {
                    0.01
                };
                let max_step = if max_step.is_finite() && max_step > 0.0 {
                    max_step
                } else {
                    f32::MAX
                };
                for i in 0..n {
                    ts.push(i as f32);
                    if self.mode != SplineMode::Polyline {
                        self.subdivide(i, 0.0, 1.0, tol, max_step, 0, &mut ts);
                    }
                    if ts.len() > MAX_SAMPLES {
                        break;
                    }
                }
            }
        }
        if !self.closed {
            ts.push(n as f32);
        }
        ts.into_iter()
            .map(|t| SplineSample {
                t,
                distance: table.distance_at_on(t, self),
                position: self.position(t),
                tangent: self.tangent(t),
            })
            .collect()
    }

    /// Pushes interior split parameters of segment `i` between `a` and `b`
    /// (exclusive) where the curve deviates more than `tol` from its chord
    /// or the chord is longer than `max_step`.
    fn subdivide(
        &self,
        i: usize,
        a: f32,
        b: f32,
        tol: f32,
        max_step: f32,
        depth: u32,
        out: &mut Vec<f32>,
    ) {
        let s = &self.segments[i];
        let (pa, pb) = (s.position(a), s.position(b));
        let chord = pb - pa;
        let len2 = chord.length_squared();
        let dev = |t: f32| {
            let p = s.position(t) - pa;
            if len2 > 0.0 {
                (p - chord * (p.dot(chord) / len2)).length()
            } else {
                p.length()
            }
        };
        let m = (a + b) * 0.5;
        let worst = dev(a + (b - a) * 0.25)
            .max(dev(m))
            .max(dev(a + (b - a) * 0.75));
        if depth < 16 && (worst > tol || len2.sqrt() > max_step) && out.len() < MAX_SAMPLES {
            self.subdivide(i, a, m, tol, max_step, depth + 1, out);
            out.push(i as f32 + m);
            self.subdivide(i, m, b, tol, max_step, depth + 1, out);
        }
    }

    /// Frames at the given samples (see [`Orientation`]), with roll and
    /// scale from the control points.
    pub fn frames(&self, samples: &[SplineSample], orientation: Orientation) -> Vec<Frame> {
        let pos: Vec<Vec3> = samples.iter().map(|s| s.position).collect();
        let tan: Vec<Vec3> = samples.iter().map(|s| s.tangent).collect();
        let ups = orient(&pos, &tan, self.closed, orientation);
        samples
            .iter()
            .zip(ups)
            .map(|(s, up)| {
                Frame::new(s.position, s.tangent, up, s.distance)
                    .rolled(self.roll(s.t))
                    .with_scale(self.scale(s.t))
            })
            .collect()
    }

    /// The closest point on the curve to `p`.
    pub fn closest_point(&self, p: Vec3) -> ClosestPoint {
        let n = self.segments.len();
        if n == 0 || !p.is_finite() {
            let q = self.position(0.0);
            return ClosestPoint {
                t: 0.0,
                position: q,
                distance: if p.is_finite() {
                    q.distance(p)
                } else {
                    f32::INFINITY
                },
            };
        }
        const COARSE: usize = 16;
        let mut best = (0.0f32, f32::MAX);
        for i in 0..n {
            for j in 0..=COARSE {
                let t = i as f32 + j as f32 / COARSE as f32;
                let d = self.position(t).distance_squared(p);
                if d < best.1 {
                    best = (t, d);
                }
            }
        }
        // Golden-section refinement in the bracket around the best sample.
        let h = 1.0 / COARSE as f32;
        let (mut a, mut b) = (best.0 - h, best.0 + h);
        if !self.closed {
            a = a.max(0.0);
            b = b.min(n as f32);
        }
        let f = |t: f32| self.position(t).distance_squared(p);
        let g = 0.618_034f32;
        let (mut c, mut d) = (b - g * (b - a), a + g * (b - a));
        let (mut fc, mut fd) = (f(c), f(d));
        for _ in 0..40 {
            if fc < fd {
                b = d;
                d = c;
                fd = fc;
                c = b - g * (b - a);
                fc = f(c);
            } else {
                a = c;
                c = d;
                fc = fd;
                d = a + g * (b - a);
                fd = f(d);
            }
        }
        let mut t = (a + b) * 0.5;
        if f(t) > best.1 {
            t = best.0;
        }
        let (i, u) = self.locate(t);
        let t = i as f32 + u;
        let q = self.position(t);
        ClosestPoint {
            t,
            position: q,
            distance: q.distance(p),
        }
    }
}

const DEFAULT_TABLE: u32 = 32;
/// Upper bound on samples produced by one sampling call.
pub const MAX_SAMPLES: usize = 100_000;

fn gauss_length(s: &CubicSegment, a: f32, b: f32) -> f64 {
    const X: [f64; 5] = [
        0.0,
        -0.538_469_310_105_683,
        0.538_469_310_105_683,
        -0.906_179_845_938_664,
        0.906_179_845_938_664,
    ];
    const W: [f64; 5] = [
        0.568_888_888_888_889,
        0.478_628_670_499_366,
        0.478_628_670_499_366,
        0.236_926_885_056_189,
        0.236_926_885_056_189,
    ];
    let (a, b) = (a as f64, b as f64);
    let (h, m) = ((b - a) * 0.5, (b + a) * 0.5);
    X.iter()
        .zip(W)
        .map(|(x, w)| w * s.derivative((m + h * x) as f32).length() as f64)
        .sum::<f64>()
        * h
}

/// Cumulative arc length at increasing parameters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArcLengthTable {
    pub t: Vec<f32>,
    pub s: Vec<f32>,
}

impl ArcLengthTable {
    pub fn length(&self) -> f32 {
        self.s.last().copied().unwrap_or(0.0)
    }

    /// Arc length at parameter `t` (linear between table entries; see
    /// [`distance_at_on`](Self::distance_at_on) for the exact value).
    pub fn distance_at(&self, t: f32) -> f32 {
        if self.t.len() < 2 || !t.is_finite() {
            return 0.0;
        }
        let k = self
            .t
            .partition_point(|&x| x <= t)
            .clamp(1, self.t.len() - 1);
        let (t0, t1) = (self.t[k - 1], self.t[k]);
        let u = if t1 > t0 {
            ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.s[k - 1] + (self.s[k] - self.s[k - 1]) * u
    }

    /// Exact arc length at `t` (Gauss-Legendre over the partial interval).
    pub fn distance_at_on(&self, t: f32, curve: &SplineCurve) -> f32 {
        if self.t.len() < 2 || curve.segments.is_empty() || !t.is_finite() {
            return 0.0;
        }
        let end = *self.t.last().unwrap_or(&0.0);
        if t >= end {
            return self.length();
        }
        let k = self
            .t
            .partition_point(|&x| x <= t)
            .clamp(1, self.t.len() - 1);
        let t0 = self.t[k - 1];
        let (i, _) = curve.locate(t0);
        let base = i as f32;
        self.s[k - 1] + gauss_length(&curve.segments[i], t0 - base, (t - base).min(1.0)) as f32
    }

    /// Parameter at arc length `distance` (clamped to the curve), refined
    /// with Newton steps on the curve.
    pub fn param_at(&self, distance: f32, curve: &SplineCurve) -> f32 {
        if self.t.len() < 2 || curve.segments.is_empty() {
            return 0.0;
        }
        let d = finite_or(distance, 0.0).clamp(0.0, self.length());
        let k = self
            .s
            .partition_point(|&x| x <= d)
            .clamp(1, self.s.len() - 1);
        let (t0, t1, s0, s1) = (self.t[k - 1], self.t[k], self.s[k - 1], self.s[k]);
        let mut t = if s1 > s0 {
            t0 + (t1 - t0) * (d - s0) / (s1 - s0)
        } else {
            t0
        };
        for _ in 0..4 {
            let err = self.distance_at_on(t, curve) - d;
            let speed = curve.derivative(t).length();
            if speed < 1e-9 || err.abs() < 1e-6 {
                break;
            }
            t = (t - err / speed).clamp(t0, t1);
        }
        t
    }
}

/// How [`SplineCurve::sample`] places samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampling {
    /// `n` equal parameter steps per segment (control points included).
    PerSegment(u32),
    /// Equal arc-length steps of at most `step` metres over the whole
    /// curve. In polyline mode each straight segment is divided evenly so
    /// corners are kept.
    Uniform { step: f32 },
    /// Control points plus recursive subdivision wherever the curve deviates
    /// more than `tolerance` metres from its chord or a chord is longer than
    /// `max_step` metres (fewer samples on straight parts).
    Adaptive { tolerance: f32, max_step: f32 },
}

/// One sample of a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplineSample {
    pub t: f32,
    /// Arc length from the start, metres.
    pub distance: f32,
    pub position: Vec3,
    /// Unit tangent.
    pub tangent: Vec3,
}

/// Result of [`SplineCurve::closest_point`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClosestPoint {
    pub t: f32,
    pub position: Vec3,
    pub distance: f32,
}

// ------------------------------------------------------------------ frames

/// How profile frames are oriented along a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Orientation {
    /// Keep `side` perpendicular to the given up direction (horizontal for
    /// +Y): faces containing the up axis stay vertical, the profile tilts
    /// only along slopes. Walls, fences, roads.
    Up(Vec3),
    /// Rotation-minimizing frames (double reflection), starting with `up`
    /// projected onto the first section plane: no twist beyond what the
    /// path forces. Tubes, cables, rails in 3D. Closed paths distribute the
    /// leftover twist evenly along the length so the seam matches.
    RotationMinimizing(Vec3),
}

impl Default for Orientation {
    fn default() -> Self {
        Orientation::Up(Vec3::Y)
    }
}

/// A position with an orthonormal basis: `forward` (direction of travel),
/// `up`, and `side = forward × up` (to the right). Profiles map as
/// `position + side * x * scale.x + up * y * scale.y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub position: Vec3,
    pub forward: Vec3,
    pub side: Vec3,
    pub up: Vec3,
    pub scale: Vec2,
    /// Arc length from the start of the path, metres.
    pub distance: f32,
    /// Total roll applied by [`Frame::rolled`], radians.
    pub roll: f32,
}

impl Default for Frame {
    fn default() -> Self {
        Self::new(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y, 0.0)
    }
}

impl Frame {
    /// Builds an orthonormal frame from a forward direction and an up hint
    /// (the hint is made perpendicular; degenerate hints pick any
    /// perpendicular).
    pub fn new(position: Vec3, forward: Vec3, up_hint: Vec3, distance: f32) -> Self {
        let f = forward.try_normalize().unwrap_or(Vec3::NEG_Z);
        let up = (up_hint - f * f.dot(up_hint))
            .try_normalize()
            .unwrap_or_else(|| f.any_orthonormal_vector());
        Self {
            position,
            forward: f,
            side: f.cross(up),
            up,
            scale: Vec2::ONE,
            distance,
            roll: 0.0,
        }
    }

    /// Rotated about `forward` by `roll` radians (positive turns `up`
    /// toward `side`).
    pub fn rolled(mut self, roll: f32) -> Self {
        if roll != 0.0 && roll.is_finite() {
            let q = Quat::from_axis_angle(self.forward, roll);
            self.up = (q * self.up).normalize();
            self.side = self.forward.cross(self.up);
            self.roll += roll;
        }
        self
    }

    pub fn with_scale(mut self, scale: Vec2) -> Self {
        self.scale = if scale.is_finite() { scale } else { Vec2::ONE };
        self
    }

    /// Maps a profile point (x across, y up) into the world.
    pub fn point(&self, p: Vec2) -> Vec3 {
        self.position + self.side * (p.x * self.scale.x) + self.up * (p.y * self.scale.y)
    }

    /// Maps a 2D profile normal into the world (inverse-transpose of the scale).
    pub fn normal(&self, n: Vec2) -> Vec3 {
        let s = Vec2::new(
            if self.scale.x != 0.0 {
                n.x / self.scale.x
            } else {
                0.0
            },
            if self.scale.y != 0.0 {
                n.y / self.scale.y
            } else {
                0.0
            },
        );
        (self.side * s.x + self.up * s.y).normalize_or_zero()
    }

    /// Linear blend (positions, scale, distance) with re-orthonormalized axes.
    pub fn lerp(&self, other: &Frame, u: f32) -> Frame {
        let mut f = Frame::new(
            self.position.lerp(other.position, u),
            self.forward.lerp(other.forward, u),
            self.up.lerp(other.up, u),
            self.distance + (other.distance - self.distance) * u,
        );
        f.scale = self.scale.lerp(other.scale, u);
        f.roll = self.roll + (other.roll - self.roll) * u;
        f
    }
}

/// `Orientation::Up` for one tangent: (side, up hint). `last_side` is the
/// fallback when the tangent is parallel to `up`.
fn up_for(t: Vec3, up: Vec3, last_side: Option<Vec3>) -> (Vec3, Vec3) {
    let side = t
        .cross(up)
        .try_normalize()
        .or(last_side)
        .unwrap_or_else(|| t.any_orthonormal_vector());
    (side, side.cross(t).normalize_or(up))
}

/// Up vectors for points with tangents.
fn orient(pos: &[Vec3], tan: &[Vec3], closed: bool, o: Orientation) -> Vec<Vec3> {
    let n = pos.len();
    let mut ups = Vec::with_capacity(n);
    match o {
        Orientation::Up(up) => {
            let up = up.try_normalize().unwrap_or(Vec3::Y);
            let mut last_side: Option<Vec3> = None;
            for &t in tan {
                let (side, u) = up_for(t, up, last_side);
                last_side = Some(side);
                ups.push(u);
            }
        }
        Orientation::RotationMinimizing(up) => {
            if n == 0 {
                return ups;
            }
            let up = up.try_normalize().unwrap_or(Vec3::Y);
            let f0 = Frame::new(pos[0], tan[0], up, 0.0);
            ups.push(f0.up);
            for i in 0..n - 1 {
                ups.push(reflect_twice(
                    pos[i],
                    pos[i + 1],
                    tan[i],
                    tan[i + 1],
                    ups[i],
                ));
            }
            if closed && n > 2 {
                // Transport once more across the closing segment and spread the twist.
                let back = reflect_twice(pos[n - 1], pos[0], tan[n - 1], tan[0], ups[n - 1]);
                let t0 = tan[0];
                let a = ups[0];
                let angle = signed_angle(a, back, t0);
                let mut len = vec![0.0f32; n + 1];
                for i in 0..n {
                    len[i + 1] = len[i] + pos[i].distance(pos[(i + 1) % n]);
                }
                let total = len[n].max(1e-9);
                for i in 1..n {
                    let q = Quat::from_axis_angle(tan[i], -angle * len[i] / total);
                    ups[i] = (q * ups[i]).normalize_or(up);
                }
            }
        }
    }
    ups
}

/// Double-reflection transport of `r` from (x0, t0) to (x1, t1)
/// (Wang, Jüttler, Zheng, Liu 2008).
fn reflect_twice(x0: Vec3, x1: Vec3, t0: Vec3, t1: Vec3, r: Vec3) -> Vec3 {
    let v1 = x1 - x0;
    let c1 = v1.dot(v1);
    let (rl, tl) = if c1 > 1e-20 {
        (
            r - v1 * (2.0 / c1 * v1.dot(r)),
            t0 - v1 * (2.0 / c1 * v1.dot(t0)),
        )
    } else {
        (r, t0)
    };
    let v2 = t1 - tl;
    let c2 = v2.dot(v2);
    let out = if c2 > 1e-20 {
        rl - v2 * (2.0 / c2 * v2.dot(rl))
    } else {
        rl
    };
    // Re-orthogonalize against the tangent to stop drift.
    (out - t1 * t1.dot(out)).try_normalize().unwrap_or(r)
}

/// Angle from `a` to `b` about `axis` (right-handed), radians.
fn signed_angle(a: Vec3, b: Vec3, axis: Vec3) -> f32 {
    let c = a.cross(b);
    pmath::atan2(c.dot(axis), a.dot(b))
}

// ------------------------------------------------------------------- paths

/// A point on a [`Path`]. Smooth stations have `incoming == outgoing`;
/// corners differ in direction (and so in frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub incoming: Frame,
    pub outgoing: Frame,
}

impl Station {
    pub fn smooth(frame: Frame) -> Self {
        Self {
            incoming: frame,
            outgoing: frame,
        }
    }

    pub fn position(&self) -> Vec3 {
        self.outgoing.position
    }

    pub fn distance(&self) -> f32 {
        self.outgoing.distance
    }

    /// True when the direction changes here (a sharp corner).
    pub fn is_corner(&self) -> bool {
        self.incoming.forward.dot(self.outgoing.forward) < 1.0 - 1e-6
    }
}

/// Stations along a path, the input of sweeps and placement. Closed paths
/// do not repeat the first station; the last station connects back to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub stations: Vec<Station>,
    pub closed: bool,
    /// Total length including the closing segment of a closed path.
    pub length: f32,
    /// How the frames were oriented (used to reconstruct a neighbouring
    /// path's frame for mitre ends).
    pub orientation: Orientation,
}

impl Path {
    /// A polyline with sharp corners, frames per segment. Non-finite and
    /// duplicate points are dropped; fewer than two points give an empty
    /// path.
    pub fn from_polyline(points: &[Vec3], closed: bool, orientation: Orientation) -> Self {
        let cps: Vec<ControlPoint> = points.iter().map(|p| ControlPoint::new(*p)).collect();
        Self::from_control_points(&cps, closed, orientation)
    }

    /// A polyline through control points (their roll and scale are used;
    /// tangents are ignored).
    pub fn from_control_points(
        points: &[ControlPoint],
        closed: bool,
        orientation: Orientation,
    ) -> Self {
        let mut pts: Vec<ControlPoint> = Vec::with_capacity(points.len());
        for p in points {
            if p.position.is_finite() && !pts.last().is_some_and(|q| q.position == p.position) {
                pts.push(*p);
            }
        }
        let mut closed = closed;
        if closed && pts.len() > 2 && pts[0].position == pts[pts.len() - 1].position {
            pts.pop();
        }
        if pts.len() < 3 {
            closed = false;
        }
        let n = pts.len();
        if n < 2 {
            return Path::default();
        }
        let segs = if closed { n } else { n - 1 };
        let dirs: Vec<Vec3> = (0..segs)
            .map(|k| (pts[(k + 1) % n].position - pts[k].position).normalize_or(Vec3::NEG_Z))
            .collect();
        let mut dist = vec![0.0f32; n + 1];
        for k in 0..segs {
            dist[k + 1] = dist[k] + pts[(k + 1) % n].position.distance(pts[k].position);
        }
        // One up vector per segment.
        let mids: Vec<Vec3> = (0..segs).map(|k| pts[k].position).collect();
        let ups: Vec<Vec3> = match orientation {
            Orientation::Up(_) => orient(&mids, &dirs, false, orientation),
            Orientation::RotationMinimizing(up) => {
                let mut ups = vec![
                    Frame::new(mids[0], dirs[0], up.try_normalize().unwrap_or(Vec3::Y), 0.0).up,
                ];
                for k in 1..segs {
                    let q = Quat::from_rotation_arc(dirs[k - 1], dirs[k]);
                    let u = (q * ups[k - 1]).normalize_or(Vec3::Y);
                    ups.push(u);
                }
                if closed {
                    let q = Quat::from_rotation_arc(dirs[segs - 1], dirs[0]);
                    let back = q * ups[segs - 1];
                    let angle = signed_angle(ups[0], back, dirs[0]);
                    let total = dist[segs].max(1e-9);
                    for (k, u) in ups.iter_mut().enumerate().skip(1) {
                        let q = Quat::from_axis_angle(dirs[k], -angle * dist[k] / total);
                        *u = (q * *u).normalize_or(Vec3::Y);
                    }
                }
                ups
            }
        };
        let frame = |seg: usize, point: usize, d: f32| -> Frame {
            let p = &pts[point % n];
            Frame::new(p.position, dirs[seg], ups[seg], d)
                .rolled(p.roll)
                .with_scale(p.scale)
        };
        let mut stations = Vec::with_capacity(n);
        for (k, &d) in dist.iter().enumerate().take(n) {
            let out_seg = if k < segs { k } else { segs - 1 };
            let in_seg = if k > 0 {
                k - 1
            } else if closed {
                segs - 1
            } else {
                0
            };
            stations.push(Station {
                incoming: frame(in_seg, k, d),
                outgoing: frame(out_seg, k, d),
            });
        }
        Path {
            stations,
            closed,
            length: dist[segs],
            orientation,
        }
    }

    /// Smooth stations from frames (as produced by [`SplineCurve::frames`]).
    /// `orientation` records how they were made (see [`Path::orientation`]).
    pub fn from_frames(frames: Vec<Frame>, closed: bool, orientation: Orientation) -> Self {
        let frames: Vec<Frame> = frames
            .into_iter()
            .filter(|f| f.position.is_finite() && f.forward.is_finite() && f.up.is_finite())
            .collect();
        let closed = closed && frames.len() > 2;
        let length = match (frames.first(), frames.last()) {
            (Some(a), Some(b)) => {
                b.distance
                    + if closed {
                        b.position.distance(a.position)
                    } else {
                        0.0
                    }
            }
            _ => 0.0,
        };
        Path {
            stations: frames.into_iter().map(Station::smooth).collect(),
            closed,
            length,
            orientation,
        }
    }

    /// A path along a spline: polyline mode gives corner stations at the
    /// control points; other modes give smooth stations at the samples.
    pub fn from_spline(spline: &Spline, sampling: Sampling, orientation: Orientation) -> Self {
        if spline.mode == SplineMode::Polyline {
            return Self::from_control_points(&spline.points, spline.closed, orientation);
        }
        let curve = spline.curve();
        let samples = curve.sample(sampling);
        let frames = curve.frames(&samples, orientation);
        Self::from_frames(frames, curve.is_closed(), orientation)
    }

    /// The frame a neighbouring path, built with this path's orientation and
    /// the same roll and scale, has at `own`'s position when it travels
    /// along `segment` (its raw segment vector: next point minus previous
    /// point, exactly as its own path computes its direction). With
    /// `Orientation::Up` the result is bit-identical to the neighbour's own
    /// frame, which is what makes mitre ends of two separate sweeps share
    /// vertices exactly. `None` for a zero or non-finite vector.
    pub fn neighbour_frame(&self, own: &Frame, segment: Vec3) -> Option<Frame> {
        if !segment.is_finite() || segment.length_squared() == 0.0 {
            return None;
        }
        let d = segment.normalize_or(Vec3::NEG_Z);
        let f = match self.orientation {
            Orientation::Up(up) => {
                let up = up.try_normalize().unwrap_or(Vec3::Y);
                let (_, hint) = up_for(d, up, None);
                Frame::new(own.position, d, hint, own.distance).rolled(own.roll)
            }
            Orientation::RotationMinimizing(_) => {
                let q = Quat::from_rotation_arc(own.forward, d.try_normalize()?);
                let mut f = Frame::new(own.position, d, q * own.up, own.distance);
                f.roll = own.roll;
                f
            }
        };
        Some(f.with_scale(own.scale))
    }

    pub fn is_empty(&self) -> bool {
        self.stations.len() < 2
    }

    /// Number of segments between stations (including the closing one).
    pub fn segment_count(&self) -> usize {
        match self.stations.len() {
            0 | 1 => 0,
            n if self.closed => n,
            n => n - 1,
        }
    }

    /// The frame at arc length `distance` (clamped, or wrapped when
    /// closed). Inside a segment the start station's outgoing frame is
    /// blended toward the end station's incoming frame; at a corner the
    /// outgoing frame wins.
    pub fn frame_at(&self, distance: f32) -> Frame {
        let n = self.stations.len();
        if n == 0 {
            return Frame::default();
        }
        if n == 1 {
            return self.stations[0].outgoing;
        }
        let d = finite_or(distance, 0.0);
        let d = if self.closed && self.length > 0.0 {
            d.rem_euclid(self.length)
        } else {
            d.clamp(0.0, self.length)
        };
        let segs = self.segment_count();
        let k = self
            .stations
            .partition_point(|s| s.distance() <= d)
            .saturating_sub(1)
            .min(segs - 1);
        let a = &self.stations[k];
        let b = &self.stations[(k + 1) % n];
        let (d0, d1) = (
            a.distance(),
            if k + 1 == n {
                self.length
            } else {
                b.distance()
            },
        );
        let u = if d1 > d0 {
            ((d - d0) / (d1 - d0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut end = b.incoming;
        end.distance = d1;
        let mut f = a.outgoing.lerp(&end, u);
        f.distance = d;
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_spline(mode: SplineMode) -> Spline {
        Spline::from_positions(
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, -4.0),
                Vec3::new(0.0, 0.0, -4.0),
            ],
            mode,
            true,
        )
    }

    #[test]
    fn catmull_rom_interpolates_control_points() {
        let s = square_spline(SplineMode::CatmullRom);
        let c = s.curve();
        assert_eq!(c.segment_count(), 4);
        for (i, p) in s.points.iter().enumerate() {
            assert!(c.position(i as f32).distance(p.position) < 1e-5);
        }
        // Closed: t wraps.
        assert!(c.position(4.0).distance(s.points[0].position) < 1e-5);
    }

    #[test]
    fn polyline_length_and_param_at() {
        let s = square_spline(SplineMode::Polyline);
        let c = s.curve();
        let table = c.arc_length_table(8);
        assert!((table.length() - 16.0).abs() < 1e-4);
        let t = table.param_at(6.0, &c);
        assert!((t - 1.5).abs() < 1e-4);
        assert!(c.position(t).distance(Vec3::new(4.0, 0.0, -2.0)) < 1e-4);
    }

    #[test]
    fn circle_length_from_bezier_tangents() {
        // Four Hermite points approximating a unit circle: length ≈ 2π.
        let k = 4.0 * (std::f32::consts::SQRT_2 - 1.0) / 3.0 * 3.0; // handle length × 3
        let pts = [(1.0, 0.0), (0.0, -1.0), (-1.0, 0.0), (0.0, 1.0)];
        let spline = Spline {
            points: pts
                .iter()
                .map(|&(x, z)| {
                    let p = Vec3::new(x, 0.0, z);
                    let t = Vec3::new(z, 0.0, -x) * k;
                    ControlPoint::new(p).with_tangent(t)
                })
                .collect(),
            mode: SplineMode::Bezier,
            closed: true,
        };
        let c = spline.curve();
        let l = c.length();
        assert!((l - std::f32::consts::TAU).abs() < 0.01, "{l}");
        let cp = c.closest_point(Vec3::new(3.0, 0.0, 0.0));
        assert!(cp.position.distance(Vec3::X) < 1e-3);
        assert!((cp.distance - 2.0).abs() < 1e-3);
    }

    #[test]
    fn uniform_sampling_is_even_by_length() {
        let s = Spline::from_positions(
            &[
                Vec3::ZERO,
                Vec3::new(1.0, 0.0, -3.0),
                Vec3::new(5.0, 0.0, -2.0),
                Vec3::new(6.0, 1.0, 2.0),
            ],
            SplineMode::CatmullRom,
            false,
        );
        let c = s.curve();
        let samples = c.sample(Sampling::Uniform { step: 0.25 });
        let len = c.length();
        let gaps: Vec<f32> = samples
            .windows(2)
            .map(|w| w[1].distance - w[0].distance)
            .collect();
        let want = len / gaps.len() as f32;
        assert!(gaps.iter().all(|g| (g - want).abs() < 1e-3), "{gaps:?}");
        assert!((samples.last().unwrap().distance - len).abs() < 1e-4);
    }

    #[test]
    fn adaptive_sampling_respects_tolerance_and_straights() {
        let s = Spline::from_positions(
            &[
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -2.0),
                Vec3::new(3.0, 0.0, -4.0),
                Vec3::new(5.0, 0.0, 0.0),
            ],
            SplineMode::CatmullRom,
            false,
        );
        let c = s.curve();
        let coarse = c.sample(Sampling::Adaptive {
            tolerance: 0.05,
            max_step: 100.0,
        });
        let fine = c.sample(Sampling::Adaptive {
            tolerance: 0.001,
            max_step: 100.0,
        });
        assert!(fine.len() > coarse.len());
        // Every chord stays within tolerance of the curve at its midpoint parameter.
        for w in fine.windows(2) {
            let mid = c.position((w[0].t + w[1].t) * 0.5);
            let chord_mid = (w[0].position + w[1].position) * 0.5;
            assert!(mid.distance(chord_mid) < 0.002);
        }
        let straight =
            Spline::from_positions(&[Vec3::ZERO, Vec3::X * 10.0], SplineMode::CatmullRom, false);
        assert_eq!(
            straight
                .curve()
                .sample(Sampling::Adaptive {
                    tolerance: 0.01,
                    max_step: 100.0
                })
                .len(),
            2
        );
    }

    #[test]
    fn rmf_frames_stay_orthonormal_and_untwisted() {
        // A helix: RMF frames are orthonormal, and Up frames keep side horizontal.
        let pts: Vec<Vec3> = (0..12)
            .map(|i| {
                let a = i as f32 * 0.6;
                Vec3::new(pmath::cos(a) * 3.0, i as f32 * 0.4, -pmath::sin(a) * 3.0)
            })
            .collect();
        let c = Spline::from_positions(&pts, SplineMode::CatmullRom, false).curve();
        let samples = c.sample(Sampling::PerSegment(8));
        for o in [
            Orientation::RotationMinimizing(Vec3::Y),
            Orientation::Up(Vec3::Y),
        ] {
            let frames = c.frames(&samples, o);
            for f in &frames {
                assert!((f.forward.length() - 1.0).abs() < 1e-4);
                assert!((f.up.length() - 1.0).abs() < 1e-4);
                assert!(f.forward.dot(f.up).abs() < 1e-4);
                assert!((f.side - f.forward.cross(f.up)).length() < 1e-5);
            }
            if let Orientation::Up(_) = o {
                assert!(frames.iter().all(|f| f.side.y.abs() < 1e-4));
            } else {
                // Consecutive RMF frames rotate little about the tangent.
                for w in frames.windows(2) {
                    let up_t = (w[1].up - w[1].forward * w[1].forward.dot(w[0].up)).normalize();
                    assert!(up_t.dot(w[0].up) > 0.95);
                }
            }
        }
    }

    #[test]
    fn closed_rmf_seam_matches() {
        // A tilted closed loop: after twist distribution the last frame's up
        // transports onto the first.
        let pts: Vec<Vec3> = (0..8)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                Vec3::new(
                    pmath::cos(a) * 3.0,
                    pmath::sin(2.0 * a),
                    -pmath::sin(a) * 3.0,
                )
            })
            .collect();
        let spline = Spline::from_positions(&pts, SplineMode::CatmullRom, true);
        let path = Path::from_spline(
            &spline,
            Sampling::PerSegment(12),
            Orientation::RotationMinimizing(Vec3::Y),
        );
        let (first, last) = (
            path.stations[0].outgoing,
            path.stations[path.stations.len() - 1].outgoing,
        );
        let back = reflect_twice(
            last.position,
            first.position,
            last.forward,
            first.forward,
            last.up,
        );
        assert!(back.dot(first.up) > 0.99, "{back} {}", first.up);
    }

    #[test]
    fn polyline_path_corners_and_frame_at() {
        let p = Path::from_polyline(
            &[
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -4.0),
                Vec3::new(3.0, 0.0, -4.0),
            ],
            false,
            Orientation::Up(Vec3::Y),
        );
        assert_eq!(p.stations.len(), 3);
        assert!(!p.stations[0].is_corner() && p.stations[1].is_corner());
        assert!((p.length - 7.0).abs() < 1e-6);
        // Walking −Z, the right side is +X.
        assert!((p.stations[0].outgoing.side - Vec3::X).length() < 1e-6);
        let f = p.frame_at(5.5);
        assert!(f.position.distance(Vec3::new(1.5, 0.0, -4.0)) < 1e-5);
        assert!((f.forward - Vec3::X).length() < 1e-5);
        assert!((p.frame_at(100.0).position - Vec3::new(3.0, 0.0, -4.0)).length() < 1e-5);
    }

    #[test]
    fn degenerate_splines_do_not_panic() {
        let nan = Vec3::splat(f32::NAN);
        for pts in [
            vec![],
            vec![nan],
            vec![Vec3::ONE],
            vec![Vec3::ONE, Vec3::ONE, nan],
        ] {
            for mode in [
                SplineMode::CatmullRom,
                SplineMode::Bezier,
                SplineMode::Polyline,
            ] {
                for closed in [false, true] {
                    let s = Spline::from_positions(&pts, mode, closed);
                    let c = s.curve();
                    assert!(c.position(0.5).is_finite());
                    assert!(c.length() == 0.0);
                    let _ = c.sample(Sampling::Uniform { step: 0.0 });
                    let _ = c.closest_point(Vec3::ZERO);
                    let p = Path::from_spline(
                        &s,
                        Sampling::Adaptive {
                            tolerance: f32::NAN,
                            max_step: -1.0,
                        },
                        Orientation::default(),
                    );
                    assert!(p.frame_at(1.0).position.is_finite());
                }
            }
        }
    }
}
