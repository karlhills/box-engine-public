//! The [`Primitive`] component: a built-in shape with editable parameters.
//!
//! No GPU code. `Primitive` is a generator like any other (`box_proc`
//! implements its `Generator` trait): the engine's generator runtime turns
//! `Primitive::mesh()` into a mesh asset when the component changes.

use glam::{UVec2, Vec2, Vec3};

use crate::mesh::MeshData;
use crate::reflect::{
    Range, Reflect, ReflectComponent, ReflectDefault, ReflectVariantDefaults, Step, Unit,
    VariantDefaults,
};

/// A built-in shape with parameters. Saved in scenes; the mesh is derived.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Primitive {
    /// The shape and its parameters.
    pub shape: Shape,
    /// Whether the shape casts shadows.
    pub cast_shadows: bool,
}

impl Default for Primitive {
    fn default() -> Self {
        Self {
            shape: Shape::default(),
            cast_shadows: true,
        }
    }
}

/// Shape of a [`Primitive`]. All shapes are centred on the origin; round
/// shapes are aligned with Y. A variant given with only some fields takes the
/// rest from [`VariantDefaults`] (the editor's Add menu shapes).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq, VariantDefaults)]
#[type_path = "box_engine"]
pub enum Shape {
    /// An axis-aligned box.
    Cuboid {
        /// Edge lengths along X, Y and Z.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        size: Vec3,
    },
    /// A UV sphere.
    Sphere {
        /// Sphere radius.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Segments around the Y axis.
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        sectors: u32,
        /// Segments from pole to pole.
        #[reflect(@Range { min: 2.0, max: 128.0 })]
        stacks: u32,
    },
    /// A capped cylinder along Y.
    Cylinder {
        /// Cylinder radius.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Total height.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        height: f32,
        /// Segments around the Y axis.
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
    },
    /// A capsule along Y: a cylinder with hemispherical ends.
    Capsule {
        /// Radius of the cylinder and the end caps.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Half the length of the cylindrical part.
        #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        half_height: f32,
        /// Segments around the Y axis.
        #[reflect(@Range { min: 3.0, max: 256.0 })]
        segments: u32,
        /// Rings per hemisphere.
        #[reflect(@Range { min: 1.0, max: 64.0 })]
        rings: u32,
    },
    /// A flat plane on XZ facing +Y.
    Plane {
        /// Extent along X and Z.
        #[reflect(@Range { min: 0.001, max: 10_000.0 }, @Unit("m"), @Step(0.1))]
        size: Vec2,
        /// Grid cells along X and Z.
        #[reflect(@Range { min: 1.0, max: 1024.0 })]
        cells: UVec2,
    },
}

impl Default for Shape {
    fn default() -> Self {
        Self::Cuboid { size: Vec3::ONE }
    }
}

/// The shapes the editor's Add menu creates, with the segment counts of the
/// [`Primitive`] constructors: a 1 m cube, a 0.5 m sphere, a 0.5 × 1 m
/// cylinder, a 0.4 m capsule with a 0.4 m half height and a 10 m plane.
impl VariantDefaults for Shape {
    fn variant_default(variant: &str) -> Option<Self> {
        Some(match variant {
            "Cuboid" => Primitive::cuboid(Vec3::ONE).shape,
            "Sphere" => Primitive::sphere(0.5).shape,
            "Cylinder" => Primitive::cylinder(0.5, 1.0).shape,
            "Capsule" => Primitive::capsule(0.4, 0.4).shape,
            "Plane" => Primitive::plane(Vec2::splat(10.0)).shape,
            _ => return None,
        })
    }
}

const MIN_LEN: f32 = 0.001;
const MAX_LEN: f32 = 10_000.0;

/// Finite, positive and bounded; bad input falls back to 1 m.
fn length(v: f32) -> f32 {
    if v.is_finite() {
        v.abs().clamp(MIN_LEN, MAX_LEN)
    } else {
        1.0
    }
}

/// Like `length`, but zero is allowed (capsule straight part).
fn length_or_zero(v: f32) -> f32 {
    if v.is_finite() {
        v.abs().min(MAX_LEN)
    } else {
        0.0
    }
}

impl Primitive {
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            ..Self::default()
        }
    }

    /// A box with edge lengths `size`.
    pub fn cuboid(size: Vec3) -> Self {
        Self::new(Shape::Cuboid { size })
    }

    /// A sphere (32 sectors, 16 stacks: the editor's default).
    pub fn sphere(radius: f32) -> Self {
        Self::new(Shape::Sphere {
            radius,
            sectors: 32,
            stacks: 16,
        })
    }

    /// A capsule along Y with hemispherical ends; `half_height` is half the
    /// length of the straight part (24 segments, 8 rings).
    pub fn capsule(radius: f32, half_height: f32) -> Self {
        Self::new(Shape::Capsule {
            radius,
            half_height,
            segments: 24,
            rings: 8,
        })
    }

    /// A capped cylinder along Y (32 segments).
    pub fn cylinder(radius: f32, height: f32) -> Self {
        Self::new(Shape::Cylinder {
            radius,
            height,
            segments: 32,
        })
    }

    /// A flat plane on XZ facing +Y, one grid cell.
    pub fn plane(size: Vec2) -> Self {
        Self::new(Shape::Plane {
            size,
            cells: UVec2::ONE,
        })
    }

    /// The same shape, casting shadows or not.
    pub fn with_shadows(mut self, cast_shadows: bool) -> Self {
        self.cast_shadows = cast_shadows;
        self
    }

    /// Build the mesh, with tangents (so normal-mapped materials work on
    /// primitives). Parameters out of range (negative, non-finite, huge
    /// segment counts) are clamped to the ranges declared on the fields, so
    /// any value an inspector or a file provides yields a valid mesh.
    pub fn mesh(&self) -> MeshData {
        self.shape.mesh()
    }
}

impl Shape {
    /// The same shape with every parameter clamped to the range declared on
    /// its field (non-finite lengths become 1 m, a capsule's non-finite
    /// straight part 0). [`mesh`](Self::mesh) builds this.
    pub fn clamped(&self) -> Shape {
        match *self {
            Shape::Cuboid { size } => Shape::Cuboid {
                size: Vec3::new(length(size.x), length(size.y), length(size.z)),
            },
            Shape::Sphere {
                radius,
                sectors,
                stacks,
            } => Shape::Sphere {
                radius: length(radius),
                sectors: sectors.clamp(3, 256),
                stacks: stacks.clamp(2, 128),
            },
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => Shape::Cylinder {
                radius: length(radius),
                height: length(height),
                segments: segments.clamp(3, 256),
            },
            Shape::Capsule {
                radius,
                half_height,
                segments,
                rings,
            } => Shape::Capsule {
                radius: length(radius),
                half_height: length_or_zero(half_height),
                segments: segments.clamp(3, 256),
                rings: rings.clamp(1, 64),
            },
            Shape::Plane { size, cells } => Shape::Plane {
                size: Vec2::new(length(size.x), length(size.y)),
                cells: UVec2::new(cells.x.clamp(1, 1024), cells.y.clamp(1, 1024)),
            },
        }
    }

    /// See [`Primitive::mesh`].
    pub fn mesh(&self) -> MeshData {
        let mesh = match self.clamped() {
            Shape::Cuboid { size } => MeshData::cuboid(size),
            Shape::Sphere {
                radius,
                sectors,
                stacks,
            } => MeshData::uv_sphere(radius, sectors, stacks),
            Shape::Cylinder {
                radius,
                height,
                segments,
            } => MeshData::cylinder(radius, height, segments),
            Shape::Capsule {
                radius,
                half_height,
                segments,
                rings,
            } => MeshData::capsule(radius, half_height, segments, rings),
            Shape::Plane { size, cells } => MeshData::grid(size.x, size.y, cells.x, cells.y),
        };
        mesh.with_tangents()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Aabb;

    fn shapes() -> Vec<Shape> {
        vec![
            Shape::Cuboid {
                size: Vec3::new(1.0, 2.0, 3.0),
            },
            Shape::Sphere {
                radius: 0.5,
                sectors: 16,
                stacks: 8,
            },
            Shape::Cylinder {
                radius: 0.5,
                height: 2.0,
                segments: 12,
            },
            Shape::Capsule {
                radius: 0.5,
                half_height: 1.0,
                segments: 12,
                rings: 4,
            },
            Shape::Plane {
                size: Vec2::new(4.0, 2.0),
                cells: UVec2::new(4, 2),
            },
        ]
    }

    #[test]
    fn every_shape_builds_a_valid_mesh() {
        for s in shapes() {
            let m = Primitive::new(s.clone()).mesh();
            m.validate().unwrap();
            assert!(m.triangle_count() > 0, "{s:?}");
            assert!(
                m.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-4),
                "{s:?}"
            );
        }
    }

    #[test]
    fn mesh_matches_parameters() {
        let m = Shape::Cuboid {
            size: Vec3::new(1.0, 2.0, 3.0),
        }
        .mesh();
        assert_eq!(
            m.bounds(),
            Aabb::new(Vec3::new(-0.5, -1.0, -1.5), Vec3::new(0.5, 1.0, 1.5))
        );
        let m = Shape::Capsule {
            radius: 0.5,
            half_height: 1.0,
            segments: 12,
            rings: 4,
        }
        .mesh();
        assert!((m.bounds().max.y - 1.5).abs() < 1e-5);
        let m = Shape::Plane {
            size: Vec2::new(4.0, 2.0),
            cells: UVec2::new(4, 2),
        }
        .mesh();
        assert_eq!(m.triangle_count(), 16);
    }

    #[test]
    fn bad_parameters_are_clamped() {
        let bad = [
            Shape::Cuboid {
                size: Vec3::new(f32::NAN, -2.0, 0.0),
            },
            Shape::Sphere {
                radius: f32::INFINITY,
                sectors: 0,
                stacks: u32::MAX,
            },
            Shape::Cylinder {
                radius: -1.0,
                height: 0.0,
                segments: 1,
            },
            Shape::Capsule {
                radius: 0.0,
                half_height: f32::NAN,
                segments: u32::MAX,
                rings: 0,
            },
            Shape::Plane {
                size: Vec2::new(-3.0, f32::NEG_INFINITY),
                cells: UVec2::new(0, u32::MAX),
            },
        ];
        for s in bad {
            let c = s.clamped();
            assert_ne!(c, s, "{s:?} needed clamping");
            assert_eq!(c.clamped(), c, "clamping is idempotent");
            let m = s.mesh();
            m.validate().unwrap();
            assert!(m.positions.iter().all(|p| p.is_finite()), "{s:?}");
            assert!(m.vertex_count() < 300_000, "{s:?}");
        }
    }
}
