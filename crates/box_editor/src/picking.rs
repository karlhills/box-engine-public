//! Viewport picking: a ray from the editor camera through the cursor, tested
//! against every rendered mesh with `box_physics::MeshRayCache`.
//!
//! Each mesh asset gets a parry triangle mesh (with its own triangle BVH),
//! built on the first pick that reaches it and rebuilt only when the asset's
//! version changes. Entities are pre-filtered by their cached mesh bounds and
//! tested nearest-bounds-first, stopping once the remaining bounds are behind
//! the best hit. Picking runs on clicks, so this linear entity pass is cheaper
//! than keeping an entity BVH in sync every frame.

use std::cell::RefCell;

use box_core::math::Ray;
use box_core::mesh::MeshData;
use box_core::prelude::{Assets, Entity, GlobalTransform, World};
use box_physics::MeshRayCache;
use box_render::MeshRenderer;
use glam::{Vec2, Vec3};

use crate::camera::EditorCamera;

thread_local! {
    /// Asset ids are unique per process, so one cache serves every world
    /// picked on this thread.
    static MESH_CACHE: RefCell<MeshRayCache> = RefCell::new(MeshRayCache::new());
}

/// The world-space ray through `ndc` (−1..1, +Y up) for a viewport with
/// `aspect` = width / height.
pub fn camera_ray(camera: &EditorCamera, ndc: Vec2, aspect: f32) -> Ray {
    let t = camera.orbit.transform();
    let tan = (camera.camera.fov_y * 0.5).tan();
    let local = Vec3::new(ndc.x * tan * aspect, ndc.y * tan, -1.0);
    Ray::new(t.translation, (t.rotation * local).normalize())
}

/// A pick result: the selectable entity (the nearest ancestor-or-self with a
/// `Uid` that is not generated), the mesh entity hit, the distance along the
/// ray, the surface normal there (world space, facing the ray) and the
/// derived part hit, if the mesh belongs to one (what Cmd+Alt+click
/// inspects).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    pub entity: Entity,
    pub mesh_entity: Entity,
    pub distance: f32,
    pub normal: Vec3,
    pub part: Option<Entity>,
}

/// The nearest mesh under `ray`, mapped to a selectable (saved, not
/// instanced) entity. Meshes the renderer does not draw (the editor's
/// hidden entities, [`box_render::Hidden`], and `Visible(false)` subtrees)
/// are not hit.
pub fn pick(world: &World, ray: &Ray) -> Option<Pick> {
    pick_where(world, ray, |_| false)
}

/// [`pick`], letting the ray pass through meshes for which `skip` is true
/// (the editor's locked entities, for selection clicks).
pub fn pick_where(world: &World, ray: &Ray, skip: impl Fn(Entity) -> bool) -> Option<Pick> {
    let meshes = world.get_resource::<Assets<MeshData>>()?;
    let (distance, normal, mesh_entity) = MESH_CACHE.with_borrow_mut(|cache| {
        cache.prune(meshes);
        let mut candidates = Vec::new();
        type Drawn<'a> = box_core::world::Without<
            (Entity, &'a MeshRenderer, &'a GlobalTransform),
            &'a box_render::Hidden,
        >;
        let game_hidden = box_core::visibility::hidden_entities(world);
        for (e, mr, gt) in world.query::<Drawn>().iter() {
            if skip(e) || game_hidden.contains(&e) {
                continue;
            }
            let Some(bounds) = cache.local_bounds(meshes, mr.mesh) else {
                continue;
            };
            if let Some(t) = bounds.transformed(&gt.0).intersect_ray(ray) {
                candidates.push((t, e, mr.mesh, gt.0));
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut best: Option<(f32, Vec3, Entity)> = None;
        for (t_box, e, mesh, transform) in candidates {
            if best.is_some_and(|(bt, ..)| t_box > bt) {
                break;
            }
            let limit = best.map_or(f32::MAX, |(bt, ..)| bt);
            if let Some(hit) = cache.cast_ray(meshes, mesh, &transform, ray, limit)
                && best.is_none_or(|(bt, ..)| hit.toi < bt)
            {
                best = Some((hit.toi, hit.normal, e));
            }
        }
        best
    })?;
    let part = derived_part(world, mesh_entity);
    let mut cur = Some(mesh_entity);
    while let Some(e) = cur {
        // Generated entities (prefab contents, derived parts) select their
        // instance root or owner.
        if world.uid(e).is_some() && !box_core::prefab::is_generated(world, e) {
            return Some(Pick {
                entity: e,
                mesh_entity,
                distance,
                normal,
                part,
            });
        }
        cur = world.parent(e);
    }
    None
}

/// The nearest ancestor-or-self of `e` that is a derived part
/// (`box_core::derived::Derived`).
pub fn derived_part(world: &World, e: Entity) -> Option<Entity> {
    let mut cur = Some(e);
    while let Some(c) = cur {
        if world.has::<box_core::derived::Derived>(c) {
            return Some(c);
        }
        cur = world.parent(c);
    }
    None
}
