//! [`RoofUnderside`]: a roof's underside as plain data, so walls can take
//! their tops from it (spike S6, rule J8: "walls tuck into roofs").
//!
//! [`Roof::underside`](crate::Roof::underside) produces it in the roof's
//! local space; a game rule moves it into a wall's space with
//! [`RoofUnderside::transformed`] and writes it into
//! [`Wall::roof`](crate::Wall::roof). The wall's top is then
//! `min over surfaces + lift` at every plan point, so gables, hip walls and
//! eave walls fall out of one rule and the joint is hidden in the slab.

use bevy_reflect::Reflect;
use box_core::reflect::{ReflectDefault, Unit};
use glam::{Affine3A, Vec2, Vec3};

/// Plan points are (x, z) in the owner's local space.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub enum UndersideSurface {
    /// A sloped plane: height `height` along the line through `origin`,
    /// rising by `slope` (rise over run) per metre along `inward`.
    Plane {
        origin: Vec2,
        inward: Vec2,
        #[reflect(@Unit("m"))]
        height: f32,
        slope: f32,
    },
    /// A cone around `centre`: height `height` at distance `radius`, rising
    /// by `slope` per metre towards the centre.
    Cone {
        centre: Vec2,
        #[reflect(@Unit("m"))]
        radius: f32,
        #[reflect(@Unit("m"))]
        height: f32,
        slope: f32,
    },
    /// A dome around `centre`: `height + rise * sqrt(1 - (d / radius)²)`
    /// inside `radius`, `height` outside.
    Dome {
        centre: Vec2,
        #[reflect(@Unit("m"))]
        radius: f32,
        #[reflect(@Unit("m"))]
        height: f32,
        #[reflect(@Unit("m"))]
        rise: f32,
    },
}

impl Default for UndersideSurface {
    fn default() -> Self {
        Self::Plane {
            origin: Vec2::ZERO,
            inward: Vec2::Y,
            height: 3.0,
            slope: 0.5,
        }
    }
}

impl UndersideSurface {
    /// Height at plan point `p` (non-finite parameters give `f32::MAX`, so
    /// the surface is ignored by the minimum).
    pub fn at(&self, p: Vec2) -> f32 {
        let h = match self {
            Self::Plane {
                origin,
                inward,
                height,
                slope,
            } => height + slope * (p - *origin).dot(inward.normalize_or_zero()),
            Self::Cone {
                centre,
                radius,
                height,
                slope,
            } => height + slope * (radius - p.distance(*centre)),
            Self::Dome {
                centre,
                radius,
                height,
                rise,
            } => {
                let d = p.distance(*centre) / radius.max(1e-4);
                height + rise * (1.0 - (d * d).min(1.0)).sqrt()
            }
        };
        if h.is_finite() { h } else { f32::MAX }
    }

    /// For a plane: the height along a line `o + d * u` as `(a, b)` with
    /// height `a + b * u`. `None` for curved surfaces.
    pub(crate) fn linear_along(&self, o: Vec2, d: Vec2) -> Option<(f32, f32)> {
        match self {
            Self::Plane {
                origin,
                inward,
                height,
                slope,
            } => {
                let i = inward.normalize_or_zero();
                Some((height + slope * (o - *origin).dot(i), slope * d.dot(i)))
            }
            _ => None,
        }
    }

    fn transformed(&self, to: &Affine3A) -> Self {
        let pt = |p: Vec2, h: f32| {
            let q = to.transform_point3(Vec3::new(p.x, h, p.y));
            (Vec2::new(q.x, q.z), q.y)
        };
        let dir = |d: Vec2| {
            let q = to.transform_vector3(Vec3::new(d.x, 0.0, d.y));
            Vec2::new(q.x, q.z).normalize_or_zero()
        };
        match self {
            Self::Plane {
                origin,
                inward,
                height,
                slope,
            } => {
                let (origin, height) = pt(*origin, *height);
                Self::Plane {
                    origin,
                    inward: dir(*inward),
                    height,
                    slope: *slope,
                }
            }
            Self::Cone {
                centre,
                radius,
                height,
                slope,
            } => {
                let (centre, height) = pt(*centre, *height);
                Self::Cone {
                    centre,
                    radius: *radius,
                    height,
                    slope: *slope,
                }
            }
            Self::Dome {
                centre,
                radius,
                height,
                rise,
            } => {
                let (centre, height) = pt(*centre, *height);
                Self::Dome {
                    centre,
                    radius: *radius,
                    height,
                    rise: *rise,
                }
            }
        }
    }
}

/// A roof's underside: the lowest of its surfaces, plus `lift`.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_arch"]
pub struct RoofUnderside {
    pub surfaces: Vec<UndersideSurface>,
    /// Added to the surfaces: a roof gives half its slab thickness, so a
    /// wall rising to it ends hidden inside the slab.
    #[reflect(@Unit("m"))]
    pub lift: f32,
}

impl Default for RoofUnderside {
    fn default() -> Self {
        Self {
            surfaces: vec![UndersideSurface::default()],
            lift: 0.0,
        }
    }
}

impl RoofUnderside {
    /// Height at plan point `p` (x, z). `f32::MAX` without surfaces.
    pub fn at(&self, p: Vec2) -> f32 {
        let m = self
            .surfaces
            .iter()
            .map(|s| s.at(p))
            .fold(f32::MAX, f32::min);
        if m == f32::MAX { m } else { m + self.lift }
    }

    /// True when every surface is a plane (wall tops are then piecewise
    /// linear along a face, with kinks where planes cross).
    pub fn is_planar(&self) -> bool {
        self.surfaces
            .iter()
            .all(|s| matches!(s, UndersideSurface::Plane { .. }))
    }

    /// The same underside in another space: `to` maps the roof's space into
    /// the wall's (for example `inverse(wall world) × roof world`). Exact
    /// for translations and rotations about Y, which is what buildings use;
    /// tilts and non-uniform scales are approximated.
    pub fn transformed(&self, to: &Affine3A) -> Self {
        Self {
            surfaces: self.surfaces.iter().map(|s| s.transformed(to)).collect(),
            lift: self.lift,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_of_planes_makes_a_gable() {
        let u = RoofUnderside {
            surfaces: vec![
                UndersideSurface::Plane {
                    origin: Vec2::new(0.0, -2.0),
                    inward: Vec2::Y,
                    height: 3.0,
                    slope: 1.0,
                },
                UndersideSurface::Plane {
                    origin: Vec2::new(0.0, 2.0),
                    inward: -Vec2::Y,
                    height: 3.0,
                    slope: 1.0,
                },
            ],
            lift: 0.1,
        };
        assert!((u.at(Vec2::new(5.0, -2.0)) - 3.1).abs() < 1e-5);
        assert!((u.at(Vec2::new(5.0, 0.0)) - 5.1).abs() < 1e-5);
        let moved = u.transformed(&Affine3A::from_translation(Vec3::new(1.0, -1.0, 0.0)));
        assert!((moved.at(Vec2::new(6.0, 0.0)) - 4.1).abs() < 1e-5);
    }
}
