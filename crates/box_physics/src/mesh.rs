//! Ray casts against render meshes: [`MeshRayCache`] builds a parry triangle
//! mesh (with its own triangle BVH) per `MeshData` asset and reuses it until
//! the asset changes. The editor picks with it.

use std::collections::HashMap;

use box_core::assets::{Assets, Handle};
use box_core::math::{Aabb, Ray};
use box_core::mesh::MeshData;
use glam::{Affine3A, Vec3};
use parry3d::math::Pose;
use parry3d::query::{Ray as ParryRay, RayCast};
use parry3d::shape::TriMesh;

/// A ray hit on a mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshHit {
    /// Distance from the ray origin, in world units.
    pub toi: f32,
    /// World-space hit point.
    pub point: Vec3,
    /// World-space geometric normal of the hit triangle, facing the ray origin.
    pub normal: Vec3,
    /// Index of the triangle in the mesh's index buffer (`indices[3 * i..]`).
    pub triangle: u32,
}

struct Entry {
    version: u64,
    bounds: Aabb,
    /// Built on the first ray cast; `None` inside when the mesh has no valid
    /// triangle.
    trimesh: Option<Option<TriMesh>>,
    /// Maps parry triangle ids back to the asset's triangle indices (bad
    /// triangles are dropped before building).
    triangles: Vec<u32>,
}

/// Triangle meshes built from `MeshData` assets, keyed by asset id and rebuilt
/// when the asset's version changes. Asset ids are unique per process, so one
/// cache can serve several `Assets` stores.
///
/// Both triangle faces count, and any affine transform works (non-uniform
/// scale and shear included): rays are moved into the mesh's local space.
#[derive(Default)]
pub struct MeshRayCache {
    entries: HashMap<u64, Entry>,
}

impl MeshRayCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of meshes cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop entries whose asset no longer exists in `meshes`. Cheap when the
    /// cache holds no more entries than `meshes` holds assets.
    pub fn prune(&mut self, meshes: &Assets<MeshData>) {
        if self.entries.len() > meshes.len() {
            self.entries.retain(|id, _| meshes.get_by_id(*id).is_some());
        }
    }

    fn entry(&mut self, meshes: &Assets<MeshData>, handle: Handle<MeshData>) -> Option<&mut Entry> {
        let mesh = meshes.get(handle)?;
        let version = meshes.version(handle)?;
        let entry = self.entries.entry(handle.id()).or_insert_with(|| Entry {
            version,
            bounds: mesh.bounds(),
            trimesh: None,
            triangles: Vec::new(),
        });
        if entry.version != version {
            *entry = Entry {
                version,
                bounds: mesh.bounds(),
                trimesh: None,
                triangles: Vec::new(),
            };
        }
        Some(entry)
    }

    /// Local-space bounds of a mesh asset (cached; no vertex scan after the
    /// first call per version). `None` if the asset does not exist.
    pub fn local_bounds(
        &mut self,
        meshes: &Assets<MeshData>,
        handle: Handle<MeshData>,
    ) -> Option<Aabb> {
        self.entry(meshes, handle).map(|e| e.bounds)
    }

    /// The nearest hit of a world-space `ray` on mesh `handle` placed by
    /// `transform` (its world matrix), no further than `max_toi`. `None` for
    /// a miss, a missing asset, a mesh without valid triangles or a
    /// degenerate transform.
    pub fn cast_ray(
        &mut self,
        meshes: &Assets<MeshData>,
        handle: Handle<MeshData>,
        transform: &Affine3A,
        ray: &Ray,
        max_toi: f32,
    ) -> Option<MeshHit> {
        if transform.matrix3.determinant().abs() < 1e-12 || max_toi.is_nan() || max_toi < 0.0 {
            return None;
        }
        let entry = self.entry(meshes, handle)?;
        if entry.trimesh.is_none() {
            let mesh = meshes.get(handle)?;
            let (trimesh, triangles) = build_trimesh(mesh);
            entry.trimesh = Some(trimesh);
            entry.triangles = triangles;
        }
        let trimesh = entry.trimesh.as_ref()?.as_ref()?;
        let inv = transform.inverse();
        // Unnormalized local direction: a local hit at `t` is the world
        // point `ray.at(t)`, so `t` stays a world distance.
        let local = ParryRay::new(
            inv.transform_point3(ray.origin),
            inv.transform_vector3(ray.direction),
        );
        if !local.origin.is_finite() || !local.dir.is_finite() {
            return None;
        }
        let hit = trimesh.cast_ray_and_get_normal(&Pose::IDENTITY, &local, max_toi, false)?;
        let mut normal = (inv.matrix3.transpose() * hit.normal).normalize_or_zero();
        if normal.dot(ray.direction) > 0.0 {
            normal = -normal;
        }
        Some(MeshHit {
            toi: hit.time_of_impact,
            point: ray.at(hit.time_of_impact),
            normal,
            triangle: entry
                .triangles
                .get(hit.subshape as usize)
                .copied()
                .unwrap_or(0),
        })
    }
}

/// A parry triangle mesh of `mesh`'s valid triangles (indices in range,
/// non-degenerate), plus each kept triangle's original index.
fn build_trimesh(mesh: &MeshData) -> (Option<TriMesh>, Vec<u32>) {
    profiling::scope!("build_trimesh");
    let n = mesh.positions.len();
    let mut indices = Vec::with_capacity(mesh.indices.len() / 3);
    let mut triangles = Vec::with_capacity(mesh.indices.len() / 3);
    for (i, tri) in mesh.indices.as_chunks::<3>().0.iter().enumerate() {
        if tri.iter().any(|&v| v as usize >= n) {
            continue;
        }
        let [a, b, c] = tri.map(|v| mesh.positions[v as usize]);
        if !(a.is_finite() && b.is_finite() && c.is_finite())
            || (b - a).cross(c - a).length_squared() == 0.0
        {
            continue;
        }
        indices.push(*tri);
        triangles.push(i as u32);
    }
    if indices.is_empty() {
        return (None, Vec::new());
    }
    match TriMesh::new(mesh.positions.clone(), indices) {
        Ok(t) => (Some(t), triangles),
        Err(e) => {
            tracing::warn!("mesh picking shape: {e}");
            (None, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    fn store() -> (Assets<MeshData>, Handle<MeshData>) {
        let mut meshes = Assets::new();
        let cube = meshes.add(MeshData::cuboid(Vec3::ONE));
        (meshes, cube)
    }

    #[test]
    fn matches_box_core_ray_mesh() {
        let (meshes, cube) = store();
        let mut cache = MeshRayCache::new();
        let mesh = meshes.get(cube).unwrap();
        let transforms = [
            Affine3A::from_translation(Vec3::new(0.0, 0.0, -10.0)),
            Affine3A::from_scale_rotation_translation(
                Vec3::splat(2.0),
                Quat::from_rotation_y(0.3),
                Vec3::new(0.0, 0.0, -10.0),
            ),
            Affine3A::from_scale_rotation_translation(
                Vec3::new(1.0, 1.0, 6.0),
                Quat::IDENTITY,
                Vec3::new(0.0, 0.0, -10.0),
            ),
        ];
        let ray = Ray::new(Vec3::new(0.1, 0.2, 0.0), Vec3::NEG_Z);
        for m in transforms {
            let ours = cache.cast_ray(&meshes, cube, &m, &ray, f32::MAX).unwrap();
            let theirs = box_core::math::ray_mesh(&ray, mesh, &m).unwrap();
            assert!(
                (ours.toi - theirs.distance).abs() < 1e-4,
                "{ours:?} {theirs:?}"
            );
            assert!((ours.normal - theirs.normal).length() < 1e-4);
            assert!((ours.point - theirs.position).length() < 1e-4);
            assert_eq!(ours.triangle as usize, theirs.triangle);
        }
        // From inside: the far wall, normal facing the origin.
        let inside = Ray::new(Vec3::new(0.0, 0.0, -10.0), Vec3::X);
        let hit = cache
            .cast_ray(&meshes, cube, &transforms[0], &inside, f32::MAX)
            .unwrap();
        assert!((hit.toi - 0.5).abs() < 1e-5 && (hit.normal - Vec3::NEG_X).length() < 1e-5);
        // Misses, a degenerate transform and a distance limit.
        let miss = Ray::new(Vec3::new(5.0, 0.0, 0.0), Vec3::NEG_Z);
        assert!(
            cache
                .cast_ray(&meshes, cube, &transforms[0], &miss, f32::MAX)
                .is_none()
        );
        let flat = Affine3A::from_scale(Vec3::new(1.0, 0.0, 1.0));
        assert!(
            cache
                .cast_ray(&meshes, cube, &flat, &ray, f32::MAX)
                .is_none()
        );
        assert!(
            cache
                .cast_ray(&meshes, cube, &transforms[0], &ray, 5.0)
                .is_none()
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn rebuilds_when_the_asset_changes_and_prunes_removed_assets() {
        let (mut meshes, cube) = store();
        let mut cache = MeshRayCache::new();
        let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z);
        let id = Affine3A::IDENTITY;
        let hit = cache.cast_ray(&meshes, cube, &id, &ray, f32::MAX).unwrap();
        assert!((hit.toi - 4.5).abs() < 1e-5);
        *meshes.get_mut(cube).unwrap() = MeshData::cuboid(Vec3::splat(4.0));
        let hit = cache.cast_ray(&meshes, cube, &id, &ray, f32::MAX).unwrap();
        assert!((hit.toi - 3.0).abs() < 1e-5, "rebuilt after the edit");
        assert_eq!(
            cache.local_bounds(&meshes, cube).unwrap().max,
            Vec3::splat(2.0)
        );
        meshes.remove(cube);
        assert!(cache.cast_ray(&meshes, cube, &id, &ray, f32::MAX).is_none());
        cache.prune(&meshes);
        assert!(cache.is_empty());
    }

    #[test]
    fn bad_indices_and_empty_meshes_are_handled() {
        let mut meshes = Assets::new();
        let mut broken = MeshData::cuboid(Vec3::ONE);
        broken.indices.splice(0..0, [0, 1, 999]);
        let broken = meshes.add(broken);
        let empty = meshes.add(MeshData::default());
        let mut cache = MeshRayCache::new();
        let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z);
        let id = Affine3A::IDENTITY;
        let hit = cache
            .cast_ray(&meshes, broken, &id, &ray, f32::MAX)
            .unwrap();
        let reference = box_core::math::ray_mesh(&ray, meshes.get(broken).unwrap(), &id).unwrap();
        assert_eq!(
            hit.triangle as usize, reference.triangle,
            "original index kept"
        );
        assert!(
            cache
                .cast_ray(&meshes, empty, &id, &ray, f32::MAX)
                .is_none()
        );
    }
}
