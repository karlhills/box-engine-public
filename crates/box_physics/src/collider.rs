//! The [`Collider`] component: a collision shape attached to an entity.

use std::collections::HashMap;

use box_core::assets::{AssetLookup, Assets};
use box_core::mesh::MeshData;
use box_core::mesh_asset::MeshAsset;
use box_core::primitive::Shape;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::world::World;
use glam::Vec3;
use parry3d::shape::{SharedShape, TriMeshFlags};

/// A collision shape on an entity, placed by its `GlobalTransform`. Saved in
/// scenes. Queries on [`Physics`](crate::Physics) find colliders; trigger
/// colliders also report overlaps as [`TriggerEvent`](crate::TriggerEvent)s.
///
/// Scale: the entity's world scale is applied per axis where the shape allows
/// it. A cuboid scales exactly on all three axes. Sphere radii use the largest
/// axis; capsule and cylinder radii use the larger of X and Z and their
/// heights use Y. Negative scale counts as positive; shear is ignored.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Collider {
    /// The collision shape, centred on the entity origin plus `offset`.
    pub shape: ColliderShape,
    /// Offset of the shape's centre from the entity origin, in the entity's
    /// local space (so it rotates and scales with the entity).
    #[reflect(@Unit("m"), @Step(0.01))]
    pub offset: Vec3,
    /// A trigger reports overlaps with other colliders as trigger events.
    /// Queries skip triggers unless their filter includes them.
    pub is_trigger: bool,
    /// Collision layers this collider is on, one bit per layer (bit 0 is the
    /// default layer). Queries test their filter mask against these bits.
    pub layers: u32,
    /// Layers this collider interacts with. A trigger and another collider
    /// overlap only if each one's mask contains a layer of the other (rigid
    /// bodies collide by the same rule).
    pub mask: u32,
    /// Friction coefficient for rigid bodies touching this collider (0: ice,
    /// 1: rubber). Two colliders combine theirs by averaging. Used by the
    /// `dynamics` feature; queries ignore it.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.01))]
    pub friction: f32,
    /// Bounciness (0: no bounce, 1: keeps all its speed). Two colliders
    /// combine theirs by averaging. Used by the `dynamics` feature; queries
    /// ignore it.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub restitution: f32,
}

/// The shape of a [`Collider`]. Round shapes are aligned with Y.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ColliderShape {
    /// The shape of the entity's `Primitive` component, kept in step with it
    /// (a plane becomes a flat box). No collider exists while the entity has
    /// no `Primitive`.
    #[default]
    Primitive,
    /// An axis-aligned box.
    Cuboid {
        /// Edge lengths along X, Y and Z.
        #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        size: Vec3,
    },
    /// A sphere.
    Sphere {
        /// Sphere radius.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
    },
    /// A capsule along Y: a cylinder with hemispherical ends.
    Capsule {
        /// Radius of the cylinder and the end caps.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Half the length of the cylindrical part.
        #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        half_height: f32,
    },
    /// A capped cylinder along Y.
    Cylinder {
        /// Cylinder radius.
        #[reflect(@Range { min: 0.001, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        radius: f32,
        /// Total height.
        #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"), @Step(0.01))]
        height: f32,
    },
    /// The triangles of the entity's `MeshAsset` mesh (imported level
    /// geometry), as a triangle mesh in the entity's pose and scale. Found
    /// through the world's `AssetLookup`; there is no collider until the
    /// mesh is loaded, and it is rebuilt when the mesh asset changes.
    /// Triangle meshes are surfaces with no inside (`contains_point` finds
    /// nothing) and are static: on a `RigidBody` other than `Fixed` the
    /// collider is left out of the simulation (warned once). `offset` is
    /// ignored.
    Mesh,
    /// The convex hull of the entity's `MeshAsset` mesh vertices, in the
    /// entity's pose and scale: a solid shape, for imported props that move
    /// (dynamic and kinematic bodies can use it). Found through
    /// `AssetLookup` like `Mesh`; rebuilt when the mesh asset changes. Holes
    /// and concave parts are filled in. `offset` is ignored.
    ConvexHull,
}

impl ColliderShape {
    /// True for the shapes built from the entity's `MeshAsset` (`Mesh`,
    /// `ConvexHull`).
    pub fn uses_mesh(&self) -> bool {
        matches!(self, Self::Mesh | Self::ConvexHull)
    }
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            shape: ColliderShape::default(),
            offset: Vec3::ZERO,
            is_trigger: false,
            layers: Collider::DEFAULT_LAYERS,
            mask: Collider::ALL_LAYERS,
            friction: Collider::DEFAULT_FRICTION,
            restitution: 0.0,
        }
    }
}

impl Collider {
    /// Layer bits of a new collider: the default layer (bit 0).
    pub const DEFAULT_LAYERS: u32 = 1;
    /// Every layer.
    pub const ALL_LAYERS: u32 = u32::MAX;
    /// Friction of a new collider.
    pub const DEFAULT_FRICTION: f32 = 0.5;

    pub fn new(shape: ColliderShape) -> Self {
        Self {
            shape,
            ..Self::default()
        }
    }

    /// A collider matching the entity's `Primitive`.
    pub fn from_primitive() -> Self {
        Self::new(ColliderShape::Primitive)
    }

    /// A box with edge lengths `size`.
    pub fn cuboid(size: Vec3) -> Self {
        Self::new(ColliderShape::Cuboid { size })
    }

    pub fn sphere(radius: f32) -> Self {
        Self::new(ColliderShape::Sphere { radius })
    }

    /// A capsule along Y; its total height is `2 * (half_height + radius)`.
    pub fn capsule(radius: f32, half_height: f32) -> Self {
        Self::new(ColliderShape::Capsule {
            radius,
            half_height,
        })
    }

    /// A cylinder along Y with total height `height`.
    pub fn cylinder(radius: f32, height: f32) -> Self {
        Self::new(ColliderShape::Cylinder { radius, height })
    }

    /// This collider as a trigger.
    pub fn trigger(mut self) -> Self {
        self.is_trigger = true;
        self
    }

    pub fn with_offset(mut self, offset: Vec3) -> Self {
        self.offset = offset;
        self
    }

    /// Set the layers this collider is on and the layers it interacts with.
    pub fn with_layers(mut self, layers: u32, mask: u32) -> Self {
        self.layers = layers;
        self.mask = mask;
        self
    }

    /// Set the friction and restitution rigid bodies use against it.
    pub fn with_material(mut self, friction: f32, restitution: f32) -> Self {
        self.friction = friction;
        self.restitution = restitution;
        self
    }
}

const MAX_LEN: f32 = 10_000.0;
const MIN_RADIUS: f32 = 0.001;

/// Finite, non-negative and bounded; bad input becomes 0.
fn extent(v: f32) -> f32 {
    if v.is_finite() {
        v.abs().min(MAX_LEN)
    } else {
        0.0
    }
}

/// Like `extent`, but never below `MIN_RADIUS`.
fn radius(v: f32) -> f32 {
    extent(v).max(MIN_RADIUS)
}

/// The scale-independent description a parry shape is built from: the
/// collider shape with `Primitive` resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Resolved {
    Cuboid { half: Vec3 },
    Sphere { radius: f32 },
    Capsule { radius: f32, half_height: f32 },
    Cylinder { radius: f32, half_height: f32 },
}

impl Resolved {
    /// `None` for `ColliderShape::Primitive` without a primitive, and for
    /// `ColliderShape::Mesh` and `ConvexHull` (built by [`MeshShapes`]).
    pub(crate) fn new(shape: &ColliderShape, primitive: Option<&Shape>) -> Option<Self> {
        Some(match *shape {
            ColliderShape::Mesh | ColliderShape::ConvexHull => return None,
            ColliderShape::Primitive => match *primitive? {
                Shape::Cuboid { size } => Self::Cuboid { half: size * 0.5 },
                Shape::Sphere { radius, .. } => Self::Sphere { radius },
                Shape::Cylinder { radius, height, .. } => Self::Cylinder {
                    radius,
                    half_height: height * 0.5,
                },
                Shape::Capsule {
                    radius,
                    half_height,
                    ..
                } => Self::Capsule {
                    radius,
                    half_height,
                },
                Shape::Plane { size, .. } => Self::Cuboid {
                    half: Vec3::new(size.x, 0.0, size.y) * 0.5,
                },
            },
            ColliderShape::Cuboid { size } => Self::Cuboid { half: size * 0.5 },
            ColliderShape::Sphere { radius } => Self::Sphere { radius },
            ColliderShape::Capsule {
                radius,
                half_height,
            } => Self::Capsule {
                radius,
                half_height,
            },
            ColliderShape::Cylinder { radius, height } => Self::Cylinder {
                radius,
                half_height: height * 0.5,
            },
        })
    }

    /// The parry shape at world scale `scale` (absolute values). Every
    /// parameter is made finite and non-negative first.
    pub(crate) fn build(&self, scale: Vec3) -> SharedShape {
        let s = scale.abs();
        let radial = s.x.max(s.z);
        match *self {
            Self::Cuboid { half } => SharedShape::cuboid(
                extent(half.x * s.x),
                extent(half.y * s.y),
                extent(half.z * s.z),
            ),
            Self::Sphere { radius: r } => SharedShape::ball(radius(r * s.max_element())),
            Self::Capsule {
                radius: r,
                half_height,
            } => SharedShape::capsule_y(extent(half_height * s.y), radius(r * radial)),
            Self::Cylinder {
                radius: r,
                half_height,
            } => SharedShape::cylinder(extent(half_height * s.y), radius(r * radial)),
        }
    }
}

/// The mesh asset version a `ColliderShape::Mesh` or `ConvexHull` collider
/// is built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MeshKey {
    pub(crate) id: u64,
    pub(crate) version: u64,
}

/// The loaded mesh `mesh` names, if any (through `AssetLookup`).
pub(crate) fn mesh_key(world: &World, mesh: Option<&MeshAsset>) -> Option<MeshKey> {
    let r = &mesh?.mesh;
    let handle = world.get_resource::<AssetLookup>()?.get::<MeshData>(r)?;
    let version = world.get_resource::<Assets<MeshData>>()?.version(handle)?;
    Some(MeshKey {
        id: handle.id(),
        version,
    })
}

/// Triangle-mesh and convex-hull shapes built from mesh assets, by asset
/// version, world scale and kind, shared by every collider using the same
/// mesh at the same scale. A new version of an asset drops the shapes of
/// older ones.
#[derive(Default)]
pub(crate) struct MeshShapes {
    shapes: HashMap<(MeshKey, [u32; 3], bool), Option<SharedShape>>,
}

impl MeshShapes {
    /// The shape of mesh `key` at world `scale` for a `Mesh` or `ConvexHull`
    /// collider `shape`; `None` when the asset is gone or has no valid
    /// triangle (or, for a hull, no volume).
    pub(crate) fn get(
        &mut self,
        world: &World,
        key: MeshKey,
        scale: Vec3,
        shape: &ColliderShape,
    ) -> Option<SharedShape> {
        let hull = *shape == ColliderShape::ConvexHull;
        let scale = scale.abs();
        let k = (key, scale.to_array().map(f32::to_bits), hull);
        if let Some(shape) = self.shapes.get(&k) {
            return shape.clone();
        }
        self.shapes
            .retain(|(old, _, _), _| old.id != key.id || old.version == key.version);
        let shape = world
            .get_resource::<Assets<MeshData>>()
            .and_then(|a| a.get_by_id(key.id))
            .and_then(|mesh| {
                if hull {
                    convex_hull(mesh, scale)
                } else {
                    trimesh(mesh, scale)
                }
            });
        self.shapes.insert(k, shape.clone());
        shape
    }
}

/// The convex hull of `mesh`'s finite vertices at `scale`; `None` (warned)
/// when they span no volume (a flat or empty mesh).
fn convex_hull(mesh: &MeshData, scale: Vec3) -> Option<SharedShape> {
    profiling::scope!("collider convex hull");
    let points: Vec<Vec3> = mesh
        .positions
        .iter()
        .map(|p| *p * scale)
        .filter(|p| p.is_finite())
        .collect();
    let bounds = points.iter().fold(None, |b: Option<(Vec3, Vec3)>, p| {
        Some(b.map_or((*p, *p), |(lo, hi)| (lo.min(*p), hi.max(*p))))
    });
    // parry needs a volume: refuse flat or empty point sets up front.
    let flat = bounds.is_none_or(|(lo, hi)| (hi - lo).min_element() < 1e-5);
    let hull = if flat {
        None
    } else {
        SharedShape::convex_hull(&points)
    };
    if hull.is_none() {
        tracing::warn!(
            target: "box_physics",
            "convex hull collider: the mesh has no volume ({} vertices); no collider",
            points.len()
        );
    }
    hull
}

/// A static triangle mesh of `mesh`'s valid triangles at `scale`, with
/// duplicate vertices merged and internal edges fixed, so that shapes slide
/// over triangle seams.
fn trimesh(mesh: &MeshData, scale: Vec3) -> Option<SharedShape> {
    profiling::scope!("collider trimesh");
    let n = mesh.positions.len();
    let vertices: Vec<Vec3> = mesh
        .positions
        .iter()
        .map(|p| {
            let p = *p * scale;
            if p.is_finite() { p } else { Vec3::ZERO }
        })
        .collect();
    let indices: Vec<[u32; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .copied()
        .filter(|t| {
            t.iter().all(|&v| (v as usize) < n) && {
                let [a, b, c] = t.map(|v| vertices[v as usize]);
                (b - a).cross(c - a).length_squared() > 1e-12
            }
        })
        .collect();
    if indices.is_empty() {
        return None;
    }
    match SharedShape::trimesh_with_flags(vertices, indices, TriMeshFlags::FIX_INTERNAL_EDGES) {
        Ok(shape) => Some(shape),
        Err(e) => {
            tracing::warn!(target: "box_physics", "mesh collider: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::shape::{Ball, Cuboid, Cylinder};

    #[test]
    fn primitive_shapes_resolve() {
        let cube = Shape::Cuboid {
            size: Vec3::new(2.0, 4.0, 6.0),
        };
        assert_eq!(
            Resolved::new(&ColliderShape::Primitive, Some(&cube)),
            Some(Resolved::Cuboid {
                half: Vec3::new(1.0, 2.0, 3.0)
            })
        );
        assert_eq!(Resolved::new(&ColliderShape::Primitive, None), None);
        let plane = Shape::Plane {
            size: glam::Vec2::new(4.0, 2.0),
            cells: glam::UVec2::ONE,
        };
        assert_eq!(
            Resolved::new(&ColliderShape::Primitive, Some(&plane)),
            Some(Resolved::Cuboid {
                half: Vec3::new(2.0, 0.0, 1.0)
            })
        );
    }

    #[test]
    fn scale_applies_per_shape() {
        let cube = Resolved::Cuboid { half: Vec3::ONE }.build(Vec3::new(2.0, -3.0, 4.0));
        let c = cube.as_shape::<Cuboid>().unwrap();
        assert_eq!(c.half_extents, Vec3::new(2.0, 3.0, 4.0));
        let ball = Resolved::Sphere { radius: 1.0 }.build(Vec3::new(1.0, 5.0, 2.0));
        assert_eq!(ball.as_shape::<Ball>().unwrap().radius, 5.0);
        let cyl = Resolved::Cylinder {
            radius: 1.0,
            half_height: 1.0,
        }
        .build(Vec3::new(2.0, 3.0, 1.0));
        let c = cyl.as_shape::<Cylinder>().unwrap();
        assert_eq!((c.radius, c.half_height), (2.0, 3.0));
    }

    #[test]
    fn bad_parameters_never_panic() {
        for r in [
            Resolved::Cuboid {
                half: Vec3::new(f32::NAN, -1.0, f32::INFINITY),
            },
            Resolved::Sphere { radius: -1.0 },
            Resolved::Capsule {
                radius: f32::NAN,
                half_height: -2.0,
            },
            Resolved::Cylinder {
                radius: 0.0,
                half_height: f32::NEG_INFINITY,
            },
        ] {
            let shape = r.build(Vec3::new(1.0, f32::NAN, -1.0));
            let aabb = shape.compute_aabb(&parry3d::math::Pose::IDENTITY);
            assert!(aabb.mins.is_finite() && aabb.maxs.is_finite(), "{r:?}");
        }
    }
}
