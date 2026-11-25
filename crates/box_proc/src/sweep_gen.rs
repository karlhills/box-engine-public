//! The [`Sweep`] generator: a profile swept along a [`Spline`] component.
//!
//! The spline is a declared input (`Inputs::component::<Spline>(uid)`), so
//! editing it (in the inspector, with the editor's Edit Spline tool, from
//! the CLI or a script) regenerates every sweep that reads it, and only
//! those. By default a sweep reads the `Spline` on its own entity; with
//! [`Sweep::spline`] set it reads another entity's, and then also watches
//! both entities' world transforms so the tube stays on the spline when
//! either moves.

use std::borrow::Cow;

use bevy_reflect::Reflect;
use box_core::identity::Uid;
use box_core::reflect::{
    Range, ReflectComponent, ReflectDefault, ReflectVariantDefaults, Unit, VariantDefaults,
};
use glam::{Affine3A, Vec2, Vec3};

use crate::generator::{GenContext, GenOutput, Generator};
use crate::ground::GroundView;
use crate::inputs::Inputs;
use crate::spline::{ControlPoint, Orientation, Path, Sampling, Spline, SplineMode};
use crate::sweep::{JointEnd, Profile, SweepParams, sweep};

/// The cross-section a [`Sweep`] carries along its spline (x across the
/// path, y up).
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq, VariantDefaults)]
#[type_path = "box_engine"]
pub enum SweepProfile {
    /// A rectangle standing on the path, centred across it: a wall, a kerb,
    /// a path or a rail.
    Rect {
        /// Across the path.
        #[reflect(@Range { min: 0.001, max: 100.0 }, @Unit("m"))]
        width: f32,
        /// Up from the path.
        #[reflect(@Range { min: 0.001, max: 100.0 }, @Unit("m"))]
        height: f32,
    },
    /// A circle centred on the path: a tube, a cable or a pipe.
    Circle {
        #[reflect(@Range { min: 0.001, max: 100.0 }, @Unit("m"))]
        radius: f32,
        /// Points around the circle.
        #[reflect(@Range { min: 3.0, max: 128.0 })]
        segments: u32,
    },
    /// Your own outline, in metres (x across, y up). Closed outlines make
    /// solids and can be capped; open ones make surfaces.
    Custom { points: Vec<Vec2>, closed: bool },
}

impl Default for SweepProfile {
    fn default() -> Self {
        Self::Circle {
            radius: 0.15,
            segments: 16,
        }
    }
}

/// A 0.3 × 0.15 m kerb, the default 0.15 m circle.
impl VariantDefaults for SweepProfile {
    fn variant_default(variant: &str) -> Option<Self> {
        Some(match variant {
            "Rect" => Self::Rect {
                width: 0.3,
                height: 0.15,
            },
            "Circle" => Self::default(),
            _ => return None,
        })
    }
}

impl SweepProfile {
    fn profile(&self) -> Profile {
        match self {
            Self::Rect { width, height } => Profile::rect(*width, *height),
            Self::Circle { radius, segments } => Profile::circle(*radius, *segments),
            Self::Custom { points, closed } => Profile::new(points.clone(), *closed),
        }
    }
}

/// How the profile turns along the spline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum SweepOrientation {
    /// The profile stays upright (its sides vertical, along the entity's
    /// +Y): walls, fences, roads.
    Up,
    /// No twist beyond what the curve forces: tubes and cables in 3D.
    #[default]
    RotationMinimizing,
}

/// A profile swept along a spline: tubes, rails, kerbs, walls, paths.
/// Reads the `Spline` on this entity, or on the entity named by `spline`;
/// editing that spline regenerates the sweep.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Sweep {
    /// The entity whose `Spline` is swept. None: this entity's own spline.
    pub spline: Option<Uid>,
    /// The cross-section: a rectangle, a circle or your own outline.
    pub profile: SweepProfile,
    /// How far the sampled path may stray from the curve: smaller is
    /// smoother and heavier.
    #[reflect(@Range { min: 0.0005, max: 1.0 }, @Unit("m"))]
    pub tolerance: f32,
    /// Upright profile (walls, roads) or no twist (tubes).
    pub orientation: SweepOrientation,
    /// Close the ends of an open spline (closed profiles only).
    pub caps: bool,
    /// Lay the path on the ground (a `Terrain`): each point's height is
    /// the ground under it plus `ground_offset`, sampled at least once per
    /// terrain cell, and the sweep follows when the terrain changes. Parts
    /// of the path off every terrain keep the spline's height.
    pub follow_ground: bool,
    /// Height of the path above the ground with `follow_ground`.
    #[reflect(@Unit("m"))]
    pub ground_offset: f32,
}

impl Default for Sweep {
    fn default() -> Self {
        Self {
            spline: None,
            profile: SweepProfile::default(),
            tolerance: 0.01,
            orientation: SweepOrientation::default(),
            caps: true,
            follow_ground: false,
            ground_offset: 0.0,
        }
    }
}

impl Sweep {
    /// The entity whose spline this sweep reads, for an owner `owner`.
    pub fn spline_uid(&self, owner: Option<Uid>) -> Option<Uid> {
        self.spline.or(owner)
    }

    /// Sweeps along `spline` given in this entity's local space (what
    /// [`Generator::generate`] does once it has the spline).
    pub fn sweep_local(&self, spline: &Spline) -> GenOutput {
        self.sweep_path(self.path(spline))
    }

    fn tolerance(&self) -> f32 {
        if self.tolerance.is_finite() && self.tolerance > 0.0 {
            self.tolerance.max(1e-4)
        } else {
            0.01
        }
    }

    fn orientation(&self) -> Orientation {
        match self.orientation {
            SweepOrientation::Up => Orientation::Up(Vec3::Y),
            SweepOrientation::RotationMinimizing => Orientation::RotationMinimizing(Vec3::Y),
        }
    }

    fn path(&self, spline: &Spline) -> Path {
        Path::from_spline(
            spline,
            Sampling::Adaptive {
                tolerance: self.tolerance(),
                max_step: f32::MAX,
            },
            self.orientation(),
        )
    }

    /// The path along `spline` (this entity's local space, `to_world` its
    /// world transform) laid on `ground` plus `ground_offset`: sampled at
    /// least every ground cell, each sample's height from the ground under
    /// it.
    fn ground_path(&self, spline: &Spline, to_world: &Affine3A, ground: &GroundView) -> Path {
        let step = ground.cell_size().max(0.1);
        let to_local = to_world.inverse();
        let offset = if self.ground_offset.is_finite() {
            self.ground_offset
        } else {
            0.0
        };
        let conform = |p: Vec3| {
            let mut w = to_world.transform_point3(p);
            if let Some(h) = ground.height(w.x, w.z) {
                w.y = h + offset;
            }
            to_local.transform_point3(w)
        };
        if spline.mode == SplineMode::Polyline {
            // Corners stay at the control points; long sides get points.
            let pts = &spline.points;
            let n = pts.len();
            let sides = if spline.closed && n > 2 {
                n
            } else {
                n.saturating_sub(1)
            };
            let mut out: Vec<ControlPoint> = Vec::new();
            for i in 0..sides {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                let pieces = (a.position.distance(b.position) / step)
                    .ceil()
                    .clamp(1.0, 4096.0) as usize;
                for k in 0..pieces {
                    let t = k as f32 / pieces as f32;
                    let mut c = ControlPoint::new(conform(a.position.lerp(b.position, t)));
                    c.roll = a.roll + (b.roll - a.roll) * t;
                    c.scale = a.scale.lerp(b.scale, t);
                    out.push(c);
                }
            }
            if !(spline.closed && n > 2)
                && let Some(last) = pts.last()
            {
                let mut c = *last;
                c.position = conform(c.position);
                c.in_tangent = None;
                c.out_tangent = None;
                out.push(c);
            }
            return Path::from_control_points(&out, spline.closed, self.orientation());
        }
        let curve = spline.curve();
        let mut samples = curve.sample(Sampling::Adaptive {
            tolerance: self.tolerance(),
            max_step: step,
        });
        for s in &mut samples {
            s.position = conform(s.position);
        }
        // Tangents and distances of the laid path.
        let n = samples.len();
        let closed = curve.is_closed();
        let at = |i: isize| -> Vec3 {
            if closed {
                samples[i.rem_euclid(n as isize) as usize].position
            } else {
                samples[i.clamp(0, n as isize - 1) as usize].position
            }
        };
        let tangents: Vec<Vec3> = (0..n as isize)
            .map(|i| (at(i + 1) - at(i - 1)).normalize_or(samples[i as usize].tangent))
            .collect();
        let mut distance = 0.0;
        for i in 0..n {
            if i > 0 {
                distance += samples[i].position.distance(samples[i - 1].position);
            }
            samples[i].tangent = tangents[i];
            samples[i].distance = distance;
        }
        let frames = curve.frames(&samples, self.orientation());
        Path::from_frames(frames, closed, self.orientation())
    }

    fn sweep_path(&self, path: Path) -> GenOutput {
        let end = if self.caps {
            JointEnd::Cap
        } else {
            JointEnd::Open
        };
        let params = SweepParams {
            profile: self.profile.profile(),
            start: end,
            end,
            cap_slot: 1,
            ..SweepParams::default()
        };
        let built = sweep(&path, &params);
        GenOutput {
            mesh: (built.mesh.triangle_count() > 0).then_some(built.mesh),
            warnings: built.warnings,
            ..GenOutput::default()
        }
    }
}

/// `spline` with its points mapped by `to_local` (positions as points,
/// tangents as vectors; roll and scale unchanged).
fn transformed(spline: &Spline, to_local: &Affine3A) -> Spline {
    let mut s = spline.clone();
    for p in &mut s.points {
        p.position = to_local.transform_point3(p.position);
        p.in_tangent = p.in_tangent.map(|t| to_local.transform_vector3(t));
        p.out_tangent = p.out_tangent.map(|t| to_local.transform_vector3(t));
    }
    s
}

impl Generator for Sweep {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Surface", "Caps"];
    const CATEGORY: &'static str = "Shapes";

    fn inputs(&self, inputs: &mut Inputs) {
        let owner = inputs.owner();
        let Some(uid) = self.spline_uid(owner) else {
            return;
        };
        inputs.component::<Spline>(uid);
        if self.follow_ground {
            inputs.ground();
            if let Some(own) = owner {
                inputs.world_transform(own);
            }
        }
        if Some(uid) != owner {
            inputs.world_transform(uid);
            if let Some(own) = owner {
                inputs.world_transform(own);
            }
        }
    }

    fn generate(&self, ctx: &GenContext) -> GenOutput {
        let owner = ctx.owner();
        let Some(uid) = self.spline_uid(owner) else {
            return GenOutput::empty().warn(
                "no spline: add a Spline to this entity or set `spline` to an entity with one",
            );
        };
        // A missing spline is reported by the runtime ("input ... is missing").
        let Some(spline) = ctx.component::<Spline>(uid) else {
            return GenOutput::empty();
        };
        let spline: Cow<'_, Spline> = if Some(uid) == owner {
            Cow::Borrowed(spline)
        } else {
            // Into this entity's space: inverse(own world) × spline's world.
            let own = owner.and_then(|o| ctx.world_transform(o));
            let theirs = ctx.world_transform(uid);
            match (own, theirs) {
                (Some(own), Some(theirs)) => {
                    Cow::Owned(transformed(spline, &(own.inverse() * theirs)))
                }
                (None, Some(theirs)) => Cow::Owned(transformed(spline, &theirs)),
                _ => Cow::Borrowed(spline),
            }
        };
        if !self.follow_ground {
            return self.sweep_local(&spline);
        }
        let to_world = owner
            .and_then(|o| ctx.world_transform(o))
            .unwrap_or(Affine3A::IDENTITY);
        match ctx.ground().filter(|g| !g.is_empty()) {
            Some(g) => self.sweep_path(self.ground_path(&spline, &to_world, g.view())),
            None => self
                .sweep_local(&spline)
                .warn("follow_ground is on but there is no Terrain to follow"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sweep_of_the_default_spline_is_a_capped_tube() {
        let out = Sweep::default().sweep_local(&Spline::default());
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let m = out.mesh.unwrap();
        assert_eq!(m.groups.len(), 2, "surface and caps");
        let b = m.bounds();
        assert!(b.min.y.abs() < 1e-3, "rests on the entity's origin: {b:?}");
        assert!(b.min.x < -2.0 && b.min.x > -2.2, "{b:?}");
        assert!(b.max.x > 2.0 && b.max.x < 2.2, "{b:?}");
    }

    #[test]
    fn a_spline_with_one_point_warns_and_draws_nothing() {
        let s = Spline::from_positions(&[Vec3::ZERO], Default::default(), false);
        let out = Sweep::default().sweep_local(&s);
        assert!(out.mesh.is_none());
        assert!(!out.warnings.is_empty());
    }
}
