//! CPU frustum culling.

use box_core::math::Aabb;
use glam::{Mat4, Vec3, Vec4};

/// Frustum planes extracted from a view-projection matrix (wgpu clip space,
/// `0 <= z <= w`). Degenerate planes, such as the far plane of an infinite
/// projection, are dropped.
#[derive(Clone, Debug)]
pub(crate) struct Frustum {
    planes: Vec<Vec4>,
}

impl Frustum {
    pub(crate) fn from_view_proj(m: &Mat4) -> Self {
        Self::from_planes(m, true)
    }

    /// The frustum without its near plane: everything in front of the far
    /// plane and inside the four sides, however near.
    pub(crate) fn from_view_proj_open_near(m: &Mat4) -> Self {
        Self::from_planes(m, false)
    }

    fn from_planes(m: &Mat4, near: bool) -> Self {
        let r0 = m.row(0);
        let r1 = m.row(1);
        let r2 = m.row(2);
        let r3 = m.row(3);
        let near = if near { r2 } else { Vec4::ZERO };
        let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, near, r3 - r2]
            .into_iter()
            .filter_map(|p| {
                let len = p.truncate().length();
                (len > 1e-6).then(|| p / len)
            })
            .collect();
        Self { planes }
    }

    /// True if any part of the box may be inside the frustum.
    pub(crate) fn intersects_aabb(&self, aabb: &Aabb) -> bool {
        if aabb.is_empty() {
            return false;
        }
        let c = aabb.center();
        let h = aabb.half_extents();
        self.planes.iter().all(|p| {
            let n = p.truncate();
            let r = h.dot(n.abs());
            n.dot(c) + p.w >= -r
        })
    }

    #[cfg(test)]
    pub(crate) fn plane_count(&self) -> usize {
        self.planes.len()
    }
}

/// `bounds` grown by `reach` metres on every side: the culling bounds of a
/// mesh whose vertices a material moves (wind sway, `bounds_padding`).
pub(crate) fn padded(bounds: Aabb, reach: f32) -> Aabb {
    if reach > 0.0 && !bounds.is_empty() {
        let pad = Vec3::splat(reach);
        Aabb::new(bounds.min - pad, bounds.max + pad)
    } else {
        bounds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::camera::rh::{proj::directx, view};

    fn camera() -> Mat4 {
        let proj = directx::perspective_infinite_reverse(60f32.to_radians(), 16.0 / 9.0, 0.1);
        let view = view::look_at_mat4(Vec3::new(0.0, 2.0, 10.0), Vec3::ZERO, Vec3::Y);
        proj * view
    }

    fn unit_box(at: Vec3) -> Aabb {
        Aabb::new(at - Vec3::splat(0.5), at + Vec3::splat(0.5))
    }

    #[test]
    fn infinite_reverse_z_has_five_planes() {
        assert_eq!(Frustum::from_view_proj(&camera()).plane_count(), 5);
    }

    #[test]
    fn culls_objects_outside_the_frustum() {
        let f = Frustum::from_view_proj(&camera());
        let boxes = [
            unit_box(Vec3::ZERO),                                 // in front: visible
            unit_box(Vec3::new(0.0, 2.0, 20.0)),                  // behind the camera
            unit_box(Vec3::new(200.0, 0.0, 0.0)),                 // far to the right
            unit_box(Vec3::new(0.0, 2.0, -5000.0)), // very far ahead: infinite far plane
            unit_box(Vec3::new(0.0, -100.0, 0.0)),  // far below
            unit_box(Vec3::new(0.0, 2.0, 9.95)),    // straddles the near plane
            Aabb::new(Vec3::splat(-1000.0), Vec3::splat(1000.0)), // contains the camera
            Aabb::EMPTY,
        ];
        let visible: Vec<usize> = (0..boxes.len())
            .filter(|&i| f.intersects_aabb(&boxes[i]))
            .collect();
        assert_eq!(visible, vec![0, 3, 5, 6]);
    }

    #[test]
    fn padding_keeps_displaced_meshes_just_outside_the_frustum() {
        let f = Frustum::from_view_proj(&camera());
        // The right edge of the view near the origin is about 11 m to the
        // side (60° vertical, 16:9, 10.2 m from the camera).
        let outside = unit_box(Vec3::new(12.5, 0.0, 0.0));
        assert!(!f.intersects_aabb(&outside), "culled without padding");
        assert!(
            f.intersects_aabb(&padded(outside, 1.5)),
            "drawn with padding"
        );
        assert!(!f.intersects_aabb(&padded(outside, 0.2)), "too little");
        assert_eq!(padded(outside, 0.0), outside);
        assert!(padded(Aabb::EMPTY, 2.0).is_empty());
    }
}
