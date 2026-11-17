//! Cascaded shadow map math: practical split scheme, bounding-sphere fit and
//! texel snapping.

use box_core::math::Aabb;
use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Quat, Vec3};

use crate::culling::Frustum;

pub(crate) const CASCADE_COUNT: usize = 4;
pub(crate) const SHADOW_MAP_SIZE: u32 = 2048;
/// Blend between log and uniform splits (1 = fully logarithmic).
const SPLIT_LAMBDA: f32 = 0.75;

/// Far view depth of each cascade using the practical split scheme
/// (a blend of logarithmic and uniform splits).
pub(crate) fn cascade_splits(near: f32, far: f32, lambda: f32) -> [f32; CASCADE_COUNT] {
    let far = far.max(near * 1.01);
    let mut out = [0.0; CASCADE_COUNT];
    for (i, o) in out.iter_mut().enumerate() {
        let p = (i + 1) as f32 / CASCADE_COUNT as f32;
        let log = near * (far / near).powf(p);
        let uni = near + (far - near) * p;
        *o = lambda * log + (1.0 - lambda) * uni;
    }
    out[CASCADE_COUNT - 1] = far;
    out
}

/// The camera parameters cascades are fitted to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CameraView {
    pub position: Vec3,
    pub rotation: Quat,
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cascade {
    pub view_proj: Mat4,
    /// Far view depth covered by this cascade.
    pub split_far: f32,
    /// World-space size of one shadow texel.
    pub texel_world: f32,
    /// Light clip-space depth range in world units.
    pub depth_range: f32,
}

/// Bounding sphere (centre distance along the view axis, radius) of the view
/// frustum slice `[n, f]`. Depends only on the projection, so it does not
/// change size as the camera rotates.
pub(crate) fn slice_sphere(n: f32, f: f32, fov_y: f32, aspect: f32) -> (f32, f32) {
    let tan_v = (fov_y * 0.5).tan();
    let tan_h = tan_v * aspect;
    let k2 = tan_h * tan_h + tan_v * tan_v;
    // Equidistant from a near and a far corner, clamped to the far plane.
    let z = ((f + n) * (1.0 + k2) * 0.5).min(f);
    let r_near = ((z - n).powi(2) + n * n * k2).sqrt();
    let r_far = ((f - z).powi(2) + f * f * k2).sqrt();
    (z, r_near.max(r_far))
}

/// A stable orthonormal "up" for looking along `dir`.
fn light_up(dir: Vec3) -> Vec3 {
    if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y }
}

/// Fits one orthographic light projection per cascade. `light_dir` is the
/// direction the light travels (unit).
pub(crate) fn compute_cascades(
    cam: &CameraView,
    light_dir: Vec3,
    shadow_distance: f32,
) -> [Cascade; CASCADE_COUNT] {
    let light_dir = light_dir.normalize_or(Vec3::NEG_Y);
    let splits = cascade_splits(cam.near, shadow_distance, SPLIT_LAMBDA);
    let forward = cam.rotation * Vec3::NEG_Z;
    let up = light_up(light_dir);
    let light_rot = view::look_to_mat4(Vec3::ZERO, light_dir, up);
    let light_rot_inv = light_rot.inverse();
    // Depth range in front of the cascade sphere that keeps full depth
    // precision. Casters further toward the light still cast: their depth is
    // clamped to the near plane (see shadow_depth.wgsl).
    let backoff = shadow_distance.max(10.0);

    std::array::from_fn(|i| {
        let n = if i == 0 { cam.near } else { splits[i - 1] };
        let f = splits[i];
        let (z, radius) = slice_sphere(n, f, cam.fov_y, cam.aspect);
        // Quantize the radius so float noise never changes the texel size.
        let radius = (radius * 16.0).ceil() / 16.0;
        let texel = 2.0 * radius / SHADOW_MAP_SIZE as f32;

        // Snap the centre to whole texels in light space.
        let center = cam.position + forward * z;
        let mut ls = light_rot.transform_point3(center);
        ls.x = (ls.x / texel).floor() * texel;
        ls.y = (ls.y / texel).floor() * texel;
        let center = light_rot_inv.transform_point3(ls);

        let eye = center - light_dir * (radius + backoff);
        let view = view::look_to_mat4(eye, light_dir, up);
        let depth_range = 2.0 * radius + backoff;
        let proj = directx::orthographic(-radius, radius, -radius, radius, 0.0, depth_range);
        Cascade {
            view_proj: proj * view,
            split_far: f,
            texel_world: texel,
            depth_range,
        }
    })
}

/// Per-cascade caster culling volumes: each cascade's orthographic light
/// box, extended without limit toward the light. A caster anywhere up-light
/// of the box shadows it (the shadow pass clamps its depth to the near plane
/// instead of clipping it), so tall or distant casters outside the view are
/// kept. A caster beside or behind the box cannot touch the shadow map, so
/// culling it never changes the result.
pub(crate) struct CasterCuller {
    frusta: [Frustum; CASCADE_COUNT],
}

impl CasterCuller {
    pub(crate) fn new(cascades: &[Cascade; CASCADE_COUNT]) -> Self {
        Self {
            frusta: std::array::from_fn(|i| {
                Frustum::from_view_proj_open_near(&cascades[i].view_proj)
            }),
        }
    }

    /// Bit `i` is set when a caster with world bounds `aabb` may touch
    /// cascade `i`.
    pub(crate) fn cascade_mask(&self, aabb: &Aabb) -> u8 {
        self.frusta
            .iter()
            .enumerate()
            .filter(|(_, f)| f.intersects_aabb(aabb))
            .fold(0, |m, (i, _)| m | (1 << i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(position: Vec3, rotation: Quat) -> CameraView {
        CameraView {
            position,
            rotation,
            fov_y: 50f32.to_radians(),
            aspect: 16.0 / 9.0,
            near: 0.1,
        }
    }

    #[test]
    fn splits_are_increasing_and_end_at_far() {
        let s = cascade_splits(0.1, 60.0, SPLIT_LAMBDA);
        assert!(s[0] > 0.1);
        for w in s.windows(2) {
            assert!(w[0] < w[1], "{s:?}");
        }
        assert_eq!(s[3], 60.0);
        // Pure uniform and pure log splits.
        let u = cascade_splits(0.0001, 100.0, 0.0);
        assert!((u[0] - 25.0).abs() < 0.01 && (u[1] - 50.0).abs() < 0.01);
        let l = cascade_splits(1.0, 10000.0, 1.0);
        assert!((l[0] - 10.0).abs() < 0.01 && (l[1] - 100.0).abs() < 0.1);
    }

    #[test]
    fn slice_sphere_contains_slice_corners() {
        for (n, f) in [(0.1, 5.0), (5.0, 20.0), (20.0, 60.0)] {
            let (z, r) = slice_sphere(n, f, 50f32.to_radians(), 16.0 / 9.0);
            let tv = (25f32.to_radians()).tan();
            let th = tv * 16.0 / 9.0;
            for d in [n, f] {
                let corner = Vec3::new(th * d, tv * d, d);
                let dist = (corner - Vec3::new(0.0, 0.0, z)).length();
                assert!(
                    dist <= r + 1e-4,
                    "corner at {d} outside sphere ({dist} > {r})"
                );
            }
        }
    }

    #[test]
    fn cascades_cover_the_view_frustum() {
        let rot = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.3);
        let c = cam(Vec3::new(3.0, 5.0, 8.0), rot);
        let light = Vec3::new(-0.4, -1.0, -0.3).normalize();
        let cascades = compute_cascades(&c, light, 60.0);
        let splits = cascade_splits(c.near, 60.0, SPLIT_LAMBDA);
        let tv = (c.fov_y * 0.5).tan();
        let th = tv * c.aspect;
        for (i, cascade) in cascades.iter().enumerate() {
            let n = if i == 0 { c.near } else { splits[i - 1] };
            for d in [n, splits[i]] {
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let local = Vec3::new(sx * th * d, sy * tv * d, -d);
                    let world = c.position + c.rotation * local;
                    let p = cascade.view_proj.project_point3(world);
                    assert!(p.x.abs() <= 1.0 && p.y.abs() <= 1.0, "cascade {i}: {p}");
                    assert!((0.0..=1.0).contains(&p.z), "cascade {i}: depth {}", p.z);
                }
            }
        }
    }

    #[test]
    fn texel_snapping_keeps_the_grid_fixed() {
        let light = Vec3::new(0.3, -1.0, 0.2).normalize();
        let a = compute_cascades(&cam(Vec3::new(0.0, 4.0, 0.0), Quat::IDENTITY), light, 60.0);
        let b = compute_cascades(
            &cam(Vec3::new(0.013, 4.0, -0.021), Quat::IDENTITY),
            light,
            60.0,
        );
        // A fixed world point must land on the same sub-texel position in both
        // frames (the projection moved by whole texels only).
        let p = Vec3::new(1.234, 0.0, -3.21);
        for i in 0..CASCADE_COUNT {
            let to_texels = |c: &Cascade| {
                let q = c.view_proj.project_point3(p);
                (q.truncate() * 0.5 * SHADOW_MAP_SIZE as f32).fract()
            };
            let (fa, fb) = (to_texels(&a[i]), to_texels(&b[i]));
            let d = (fa - fb).abs();
            let d = d.min(glam::Vec2::ONE - d);
            assert!(d.max_element() < 1e-2, "cascade {i}: {fa} vs {fb}");
            assert_eq!(a[i].texel_world, b[i].texel_world);
        }
    }

    #[test]
    fn casters_are_culled_per_cascade_but_kept_toward_the_light() {
        let rot = Quat::from_rotation_x(-0.35);
        let c = cam(Vec3::new(0.0, 2.0, 10.0), rot);
        let light = Vec3::new(0.3, -1.0, -0.2).normalize();
        let cascades = compute_cascades(&c, light, 60.0);
        let culler = CasterCuller::new(&cascades);
        let unit = |p: Vec3| Aabb::new(p - Vec3::splat(0.5), p + Vec3::splat(0.5));
        let view = Frustum::from_view_proj(
            &(directx::perspective_infinite_reverse(c.fov_y, c.aspect, c.near)
                * Mat4::from_rotation_translation(c.rotation, c.position).inverse()),
        );

        // Visible ground a few metres ahead lies in the first cascade.
        let ground = c.position + (c.rotation * Vec3::NEG_Z) * 4.0;
        assert!(view.intersects_aabb(&unit(ground)));
        assert_ne!(culler.cascade_mask(&unit(ground)) & 1, 0);

        // Far to the side: no cascade.
        assert_eq!(culler.cascade_mask(&unit(Vec3::new(500.0, 0.0, 0.0))), 0);

        // 20 m up toward the light: off screen, but it shadows the visible
        // ground, so it still casts into the first cascade.
        let above = ground - light * 20.0;
        assert!(!view.intersects_aabb(&unit(above)));
        assert_ne!(culler.cascade_mask(&unit(above)) & 1, 0);

        // However far toward the light, it still casts (depth clamp); deep
        // behind the receivers along the light it cannot, so it is culled.
        assert_ne!(culler.cascade_mask(&unit(ground - light * 500.0)) & 1, 0);
        assert_eq!(culler.cascade_mask(&unit(ground + light * 500.0)), 0);

        // The mask agrees with where the cascade projection puts a point.
        for p in [
            ground,
            above,
            Vec3::new(30.0, 0.0, -25.0),
            Vec3::new(-8.0, 1.0, 3.0),
        ] {
            let tiny = Aabb::new(p, p);
            let mask = culler.cascade_mask(&tiny);
            for (i, cascade) in cascades.iter().enumerate() {
                let q = cascade.view_proj.project_point3(p);
                let inside = q.x.abs() <= 1.0 && q.y.abs() <= 1.0 && q.z <= 1.0;
                assert_eq!(mask & (1 << i) != 0, inside, "cascade {i}, point {p}");
            }
        }
    }

    #[test]
    fn rotation_does_not_change_cascade_size() {
        let light = Vec3::new(0.3, -1.0, 0.2).normalize();
        let a = compute_cascades(&cam(Vec3::ZERO, Quat::IDENTITY), light, 60.0);
        let b = compute_cascades(&cam(Vec3::ZERO, Quat::from_rotation_y(1.1)), light, 60.0);
        for i in 0..CASCADE_COUNT {
            assert_eq!(a[i].texel_world, b[i].texel_world);
        }
    }
}
