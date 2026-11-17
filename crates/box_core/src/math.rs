//! Bounds, rays and picking.

use glam::{Affine3A, Vec3};

use crate::mesh::MeshData;

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Self {
        points.into_iter().fold(Self::EMPTY, |b, p| b.including(p))
    }

    pub fn including(self, p: Vec3) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.cmpgt(self.max).any()
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(&self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// Bounds of this box after an affine transform (conservative).
    pub fn transformed(&self, m: &Affine3A) -> Self {
        if self.is_empty() {
            return *self;
        }
        let c = m.transform_point3(self.center());
        let h = self.half_extents();
        let abs = glam::Mat3::from_cols(
            Vec3::from(m.matrix3.x_axis).abs(),
            Vec3::from(m.matrix3.y_axis).abs(),
            Vec3::from(m.matrix3.z_axis).abs(),
        );
        let e = abs * h;
        Self::new(c - e, c + e)
    }

    /// Ray–box intersection; returns the entry distance along the ray if hit.
    /// The box faces count as inside, so a ray running along a face hits.
    pub fn intersect_ray(&self, ray: &Ray) -> Option<f32> {
        let (mut tmin, mut tmax) = (0.0f32, f32::INFINITY);
        for axis in 0..3 {
            let (o, d) = (ray.origin[axis], ray.direction[axis]);
            let (lo, hi) = (self.min[axis], self.max[axis]);
            if d == 0.0 {
                // Parallel to this slab: no constraint on `t`, and no
                // `0 * inf = NaN` when the origin is exactly on a face.
                if o < lo || o > hi {
                    return None;
                }
            } else {
                let (a, b) = ((lo - o) / d, (hi - o) / d);
                tmin = tmin.max(a.min(b));
                tmax = tmax.min(a.max(b));
            }
        }
        (tmax >= tmin).then_some(tmin)
    }
}

/// A ray with a normalized direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self {
            origin,
            direction: direction.normalize_or(Vec3::NEG_Z),
        }
    }

    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

/// Ray–triangle intersection (Möller–Trumbore, both faces). Returns the
/// distance along the ray (in units of `ray.direction`) of a hit in front of
/// the origin. The parallel/degenerate test is relative to the sizes of the
/// triangle and of `ray.direction`, so it does not depend on the units used.
pub fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = ray.direction.cross(e2);
    let det = e1.dot(p);
    // `det / (|e1| |e2| |direction|)` is the sine of the angle between the ray
    // and the triangle's plane times the sine of the triangle's corner angle.
    if det.abs() <= RELATIVE_EPS * e1.length() * e2.length() * ray.direction.length() {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.direction.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0 && t.is_finite()).then_some(t)
}

/// Below this (as a ratio to the magnitude of the inputs) a determinant is
/// treated as zero: a few `f32` rounding steps.
const RELATIVE_EPS: f32 = 1e-6;

/// The nearest intersection of a ray with a mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    /// Distance from the ray origin, in world units.
    pub distance: f32,
    /// World-space hit point.
    pub position: Vec3,
    /// World-space geometric normal of the hit triangle, facing the ray origin.
    pub normal: Vec3,
    /// Index of the triangle (`indices[3 * triangle..]`).
    pub triangle: usize,
}

/// Intersect a world-space ray with `mesh` placed by `transform` (its world
/// matrix, e.g. `GlobalTransform.0`). Both triangle faces count. Returns the
/// nearest hit, or `None` for a miss or a degenerate transform. Out-of-range
/// indices are skipped rather than trusted.
pub fn ray_mesh(ray: &Ray, mesh: &MeshData, transform: &Affine3A) -> Option<RayHit> {
    // Degenerate (flattened) relative to the axis lengths, so a small uniform
    // scale is not mistaken for a singular matrix.
    let m = transform.matrix3;
    let axes = m.x_axis.length() * m.y_axis.length() * m.z_axis.length();
    let det = m.determinant().abs();
    if det <= RELATIVE_EPS * axes || !det.is_finite() {
        return None;
    }
    let inv = transform.inverse();
    // Unnormalized local direction: a local hit at `t` is the world point
    // `ray.at(t)`, so `t` stays a world distance.
    let local = Ray {
        origin: inv.transform_point3(ray.origin),
        direction: inv.transform_vector3(ray.direction),
    };
    mesh.bounds().intersect_ray(&local)?;
    let mut best: Option<(f32, usize, Vec3)> = None;
    for (i, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        let corner = |k: usize| mesh.positions.get(tri[k] as usize).copied();
        let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
            continue;
        };
        if let Some(t) = ray_triangle(&local, a, b, c)
            && best.is_none_or(|(bt, _, _)| t < bt)
        {
            best = Some((t, i, (b - a).cross(c - a)));
        }
    }
    let (t, triangle, local_normal) = best?;
    let mut normal = (inv.matrix3.transpose() * local_normal).normalize_or_zero();
    if normal.dot(ray.direction) > 0.0 {
        normal = -normal;
    }
    Some(RayHit {
        distance: t,
        position: ray.at(t),
        normal,
        triangle,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transformed_bounds_contain_rotated_box() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let m = Affine3A::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let t = b.transformed(&m);
        let r = 2f32.sqrt();
        assert!((t.max.x - r).abs() < 1e-5 && (t.max.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn ray_hits_box() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        let hit = b.intersect_ray(&Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z));
        assert_eq!(hit, Some(4.0));
        assert!(
            b.intersect_ray(&Ray::new(Vec3::new(3.0, 0.0, 5.0), Vec3::NEG_Z))
                .is_none()
        );
    }

    #[test]
    fn ray_triangle_hits_inside_and_misses_outside() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        let down = |x, y| Ray::new(Vec3::new(x, y, 2.0), Vec3::NEG_Z);
        assert_eq!(ray_triangle(&down(0.25, 0.25), a, b, c), Some(2.0));
        assert_eq!(ray_triangle(&down(0.75, 0.75), a, b, c), None);
        // Back face counts; behind the origin does not.
        let up = Ray::new(Vec3::new(0.2, 0.2, -1.0), Vec3::Z);
        assert_eq!(ray_triangle(&up, a, b, c), Some(1.0));
        let away = Ray::new(Vec3::new(0.2, 0.2, 1.0), Vec3::Z);
        assert_eq!(ray_triangle(&away, a, b, c), None);
        // Parallel ray.
        let flat = Ray::new(Vec3::new(-1.0, 0.2, 0.0), Vec3::X);
        assert_eq!(ray_triangle(&flat, a, b, c), None);
    }

    #[test]
    fn ray_mesh_nearest_hit_with_transform() {
        let cube = MeshData::cuboid(Vec3::ONE);
        // Cube scaled x2 and moved to z = -10: front face at z = -9.
        let m = Affine3A::from_scale_rotation_translation(
            Vec3::splat(2.0),
            glam::Quat::from_rotation_y(0.3),
            Vec3::new(0.0, 0.0, -10.0),
        );
        let m_axis = Affine3A::from_scale_rotation_translation(
            Vec3::splat(2.0),
            glam::Quat::IDENTITY,
            Vec3::new(0.0, 0.0, -10.0),
        );
        let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
        let hit = ray_mesh(&ray, &cube, &m_axis).unwrap();
        assert!((hit.distance - 9.0).abs() < 1e-5, "{hit:?}");
        assert!((hit.position - Vec3::new(0.0, 0.0, -9.0)).length() < 1e-5);
        assert!((hit.normal - Vec3::Z).length() < 1e-5, "{hit:?}");
        // Rotated: still hit, nearer than the centre, normal faces the ray.
        let hit = ray_mesh(&ray, &cube, &m).unwrap();
        assert!(hit.distance < 10.0 && hit.distance > 8.0, "{hit:?}");
        assert!(hit.normal.dot(ray.direction) < 0.0);
        assert!((hit.normal.length() - 1.0).abs() < 1e-5);
        // Non-uniform scale: the hit distance is in world units.
        let stretched = Affine3A::from_scale_rotation_translation(
            Vec3::new(1.0, 1.0, 6.0),
            glam::Quat::IDENTITY,
            Vec3::new(0.0, 0.0, -10.0),
        );
        let hit = ray_mesh(&ray, &cube, &stretched).unwrap();
        assert!((hit.distance - 7.0).abs() < 1e-4, "{hit:?}");
        // Miss and degenerate transform.
        let miss = Ray::new(Vec3::new(5.0, 0.0, 0.0), Vec3::NEG_Z);
        assert!(ray_mesh(&miss, &cube, &m_axis).is_none());
        let flat = Affine3A::from_scale(Vec3::new(1.0, 0.0, 1.0));
        assert!(ray_mesh(&ray, &cube, &flat).is_none());
        // Bad indices are skipped, not trusted.
        let mut broken = cube.clone();
        broken.indices.extend_from_slice(&[0, 1, 999]);
        assert!(ray_mesh(&ray, &broken, &m_axis).is_some());
    }

    #[test]
    fn ray_mesh_does_not_depend_on_authoring_scale() {
        let ray = Ray::new(Vec3::new(0.1, 0.2, 5.0), Vec3::NEG_Z);
        // A millimetre-sized mesh scaled up to a metre.
        let tiny = MeshData::cuboid(Vec3::splat(1e-3));
        let hit = ray_mesh(&ray, &tiny, &Affine3A::from_scale(Vec3::splat(1e3))).unwrap();
        assert!((hit.distance - 4.5).abs() < 1e-3, "{hit:?}");
        // A kilometre-sized mesh scaled down to a metre.
        let huge = MeshData::cuboid(Vec3::splat(1e5));
        let hit = ray_mesh(&ray, &huge, &Affine3A::from_scale(Vec3::splat(1e-5))).unwrap();
        assert!((hit.distance - 4.5).abs() < 1e-3, "{hit:?}");
        // A tiny object seen from close by.
        let cube = MeshData::cuboid(Vec3::ONE);
        let near = Ray::new(Vec3::new(0.0, 0.0, 1e-4), Vec3::NEG_Z);
        let hit = ray_mesh(&near, &cube, &Affine3A::from_scale(Vec3::splat(1e-5))).unwrap();
        assert!((hit.distance - 9.5e-5).abs() < 1e-7, "{hit:?}");
        // Small and large triangles on their own.
        for s in [1e-6f32, 1e-3, 1e4] {
            let down = Ray::new(Vec3::new(0.25, 0.25, 2.0) * s, Vec3::NEG_Z);
            let t = ray_triangle(&down, Vec3::ZERO, Vec3::X * s, Vec3::Y * s).unwrap();
            assert!((t / s - 2.0).abs() < 1e-4, "scale {s}: {t}");
            let flat = Ray::new(Vec3::new(-1.0, 0.2, 0.0) * s, Vec3::X);
            assert_eq!(
                ray_triangle(&flat, Vec3::ZERO, Vec3::X * s, Vec3::Y * s),
                None
            );
        }
    }

    #[test]
    fn ray_along_a_box_face_is_a_hit() {
        let b = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
        // Origin exactly on the x = 1 and y = -1 slabs, direction zero on both axes.
        let ray = Ray::new(Vec3::new(1.0, -1.0, 5.0), Vec3::NEG_Z);
        assert_eq!(b.intersect_ray(&ray), Some(4.0));
        let outside = Ray::new(Vec3::new(1.5, -1.0, 5.0), Vec3::NEG_Z);
        assert_eq!(b.intersect_ray(&outside), None);
        let inside = Ray::new(Vec3::new(1.0, 0.0, 0.0), Vec3::Y);
        assert_eq!(b.intersect_ray(&inside), Some(0.0));
        assert_eq!(Aabb::EMPTY.intersect_ray(&ray), None);
    }

    #[test]
    fn ray_mesh_from_inside_hits_the_far_wall() {
        let cube = MeshData::cuboid(Vec3::splat(2.0));
        let ray = Ray::new(Vec3::ZERO, Vec3::X);
        let hit = ray_mesh(&ray, &cube, &Affine3A::IDENTITY).unwrap();
        assert!((hit.distance - 1.0).abs() < 1e-5);
        assert!((hit.normal - Vec3::NEG_X).length() < 1e-5, "{hit:?}");
    }
}
