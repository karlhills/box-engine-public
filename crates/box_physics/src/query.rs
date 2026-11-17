//! Query inputs and results: [`QueryFilter`], [`QueryShape`], [`Pose`],
//! [`RayHit`] and [`ShapeHit`].

use box_core::world::Entity;
use glam::{Quat, Vec3};
use parry3d::shape::{Ball, Capsule, Cuboid, Cylinder, Shape};

use crate::collider::Collider;

/// Which colliders a query considers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryFilter {
    /// Only colliders on at least one of these layers
    /// (`collider.layers & mask != 0`).
    pub mask: u32,
    /// Skip this entity's collider (usually the querying entity itself).
    pub exclude: Option<Entity>,
    /// Include trigger colliders. Off by default: triggers are for overlap
    /// events, not for blocking rays and movement.
    pub include_triggers: bool,
}

impl Default for QueryFilter {
    fn default() -> Self {
        Self {
            mask: Collider::ALL_LAYERS,
            exclude: None,
            include_triggers: false,
        }
    }
}

impl QueryFilter {
    /// Every non-trigger collider.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_mask(mut self, mask: u32) -> Self {
        self.mask = mask;
        self
    }

    pub fn excluding(mut self, entity: Entity) -> Self {
        self.exclude = Some(entity);
        self
    }

    pub fn with_triggers(mut self) -> Self {
        self.include_triggers = true;
        self
    }

    pub(crate) fn accepts(&self, entity: Entity, layers: u32, is_trigger: bool) -> bool {
        layers & self.mask != 0
            && self.exclude != Some(entity)
            && (self.include_triggers || !is_trigger)
    }
}

/// A position and orientation (no scale) for query shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Pose {
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    pub fn new(translation: Vec3, rotation: Quat) -> Self {
        Self {
            translation,
            rotation,
        }
    }

    pub fn from_translation(translation: Vec3) -> Self {
        Self::new(translation, Quat::IDENTITY)
    }

    /// The parry pose, or `None` if any component is not finite.
    pub(crate) fn to_parry(self) -> Option<parry3d::math::Pose> {
        let rotation = self.rotation.normalize();
        (self.translation.is_finite() && rotation.is_finite())
            .then(|| parry3d::math::Pose::from_parts(self.translation, rotation))
    }
}

impl From<Vec3> for Pose {
    fn from(translation: Vec3) -> Self {
        Self::from_translation(translation)
    }
}

/// A shape to cast or test for overlaps. Same conventions as
/// [`ColliderShape`](crate::ColliderShape): round shapes are aligned with Y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QueryShape {
    Sphere {
        radius: f32,
    },
    /// A box with edge lengths `size`.
    Cuboid {
        size: Vec3,
    },
    /// A capsule along Y; total height `2 * (half_height + radius)`.
    Capsule {
        radius: f32,
        half_height: f32,
    },
    /// A cylinder along Y with total height `height`.
    Cylinder {
        radius: f32,
        height: f32,
    },
}

impl QueryShape {
    pub fn sphere(radius: f32) -> Self {
        Self::Sphere { radius }
    }

    pub fn cuboid(size: Vec3) -> Self {
        Self::Cuboid { size }
    }

    pub fn capsule(radius: f32, half_height: f32) -> Self {
        Self::Capsule {
            radius,
            half_height,
        }
    }

    pub fn cylinder(radius: f32, height: f32) -> Self {
        Self::Cylinder { radius, height }
    }

    /// The parry shape, built on the stack. `None` for non-finite or negative
    /// parameters.
    pub(crate) fn to_parry(self) -> Option<LocalShape> {
        let ok = |v: f32| v.is_finite() && v >= 0.0;
        Some(match self {
            Self::Sphere { radius } if ok(radius) => LocalShape::Ball(Ball::new(radius)),
            Self::Cuboid { size } if ok(size.x) && ok(size.y) && ok(size.z) => {
                LocalShape::Cuboid(Cuboid::new(size * 0.5))
            }
            Self::Capsule {
                radius,
                half_height,
            } if ok(radius) && ok(half_height) => {
                LocalShape::Capsule(Capsule::new_y(half_height, radius))
            }
            Self::Cylinder { radius, height } if ok(radius) && ok(height) => {
                LocalShape::Cylinder(Cylinder::new(height * 0.5, radius))
            }
            _ => return None,
        })
    }
}

/// A query shape as a concrete parry shape, without allocating.
pub(crate) enum LocalShape {
    Ball(Ball),
    Cuboid(Cuboid),
    Capsule(Capsule),
    Cylinder(Cylinder),
}

impl LocalShape {
    pub(crate) fn as_dyn(&self) -> &dyn Shape {
        match self {
            Self::Ball(s) => s,
            Self::Cuboid(s) => s,
            Self::Capsule(s) => s,
            Self::Cylinder(s) => s,
        }
    }
}

/// A ray hit on a collider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub entity: Entity,
    /// Distance along the ray, in metres (ray directions are normalized).
    /// Zero when the ray starts inside the collider.
    pub toi: f32,
    /// World-space hit point.
    pub point: Vec3,
    /// World-space surface normal at the hit, facing the ray origin; zero
    /// when the ray starts inside the collider.
    pub normal: Vec3,
}

/// A shape cast hit on a collider.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeHit {
    pub entity: Entity,
    /// Distance travelled along the (normalized) cast direction before
    /// contact, in metres.
    pub toi: f32,
    /// World-space contact point on the hit collider.
    pub point: Vec3,
    /// World-space outward normal of the hit collider at the contact point:
    /// slide along the plane it defines.
    pub normal: Vec3,
    /// The cast shape already overlapped this collider at its start pose
    /// (`toi` is 0; the normal is the direction to push out along).
    pub penetrating: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_rules() {
        let mut w = box_core::world::World::new();
        let a = w.spawn(());
        let b = w.spawn(());
        let f = QueryFilter::new();
        assert!(f.accepts(a, 1, false));
        assert!(!f.accepts(a, 1, true), "triggers skipped by default");
        assert!(f.with_triggers().accepts(a, 1, true));
        assert!(!f.with_mask(2).accepts(a, 1, false));
        assert!(f.with_mask(3).accepts(a, 2, false));
        assert!(!f.excluding(a).accepts(a, 1, false));
        assert!(f.excluding(a).accepts(b, 1, false));
    }

    #[test]
    fn bad_query_shapes_and_poses_are_rejected() {
        assert!(QueryShape::sphere(f32::NAN).to_parry().is_none());
        assert!(
            QueryShape::cuboid(Vec3::new(1.0, -1.0, 1.0))
                .to_parry()
                .is_none()
        );
        assert!(QueryShape::capsule(0.5, 1.0).to_parry().is_some());
        assert!(Pose::from_translation(Vec3::NAN).to_parry().is_none());
        assert!(
            Pose::new(Vec3::ZERO, Quat::from_xyzw(0.0, 0.0, 0.0, 2.0))
                .to_parry()
                .is_some()
        );
    }
}
