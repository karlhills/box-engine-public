//! Camera projection helpers: world points to viewport positions and
//! viewport positions to world rays, with the renderer's own projection
//! (right-handed, +Y up, looking down −Z, infinite reverse-Z perspective).
//!
//! Use these instead of re-deriving the projection: UI world anchors, picking,
//! gizmos and game code all go through [`Camera::world_to_viewport`] and
//! [`Camera::viewport_ray`] (or the [`ViewCamera`] forms).
//!
//! Viewport coordinates are pixels from the top-left corner, +Y down, in the
//! same units as the viewport size passed in (the engine's `WindowSize` and
//! `Input::cursor_position` are both physical pixels). Normalized device
//! coordinates (NDC) are −1..1 with +Y up; NDC depth is reverse-Z: 1 on the
//! near plane, falling toward 0 at infinity.

use box_core::math::Ray;
use box_core::transform::GlobalTransform;
use glam::camera::rh::proj::directx;
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::components::Camera;
use crate::viewport::ViewCamera;

/// The near plane the renderer actually uses (`Camera::near`, at least 0.1 mm).
fn near(camera: &Camera) -> f32 {
    camera.near.max(1e-4)
}

/// A camera's rotation and position, scale ignored (as the renderer does).
fn pose(transform: &GlobalTransform) -> (Quat, Vec3) {
    let (_, rotation, position) = transform.0.to_scale_rotation_translation();
    (rotation.normalize(), position)
}

fn aspect_of(viewport: Vec2) -> f32 {
    viewport.x.max(1.0) / viewport.y.max(1.0)
}

/// A world point seen through a camera: where it lands on the viewport and
/// how far in front of the camera it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportPoint {
    /// Position in viewport pixels from the top-left corner (+Y down).
    pub pixels: Vec2,
    /// Position as a fraction of the viewport: (0, 0) top-left, (1, 1)
    /// bottom-right.
    pub normalized: Vec2,
    /// Normalized device coordinates: x and y in −1..1 (+Y up) on screen; z
    /// is the reverse-Z depth-buffer value (1 on the near plane, toward 0 at
    /// infinity).
    pub ndc: Vec3,
    /// Distance in front of the camera along its view axis (−Z), in metres.
    pub depth: f32,
}

impl ViewportPoint {
    /// True when the point is inside the viewport rectangle (edges included).
    pub fn is_on_screen(&self) -> bool {
        (0.0..=1.0).contains(&self.normalized.x) && (0.0..=1.0).contains(&self.normalized.y)
    }
}

impl Camera {
    /// The projection matrix the renderer uses for a viewport of `aspect`
    /// (width / height): an infinite reverse-Z perspective (depth 1 on the
    /// near plane, 0 at infinity; clip `w` is the view depth).
    pub fn projection(&self, aspect: f32) -> Mat4 {
        directx::perspective_infinite_reverse(self.fov_y, aspect, near(self))
    }

    /// World to clip space for a camera at `transform` (scale ignored):
    /// `projection(aspect) * view`.
    pub fn view_projection(&self, transform: &GlobalTransform, aspect: f32) -> Mat4 {
        let (rotation, position) = pose(transform);
        self.projection(aspect) * Mat4::from_rotation_translation(rotation, position).inverse()
    }

    /// Where world point `point` appears in a viewport of `viewport` size
    /// (pixels) seen from a camera at `transform`, or `None` when it is not
    /// in front of the near plane (behind the camera). Points outside the
    /// view but in front of the camera are returned; check
    /// [`ViewportPoint::is_on_screen`].
    pub fn world_to_viewport(
        &self,
        transform: &GlobalTransform,
        viewport: Vec2,
        point: Vec3,
    ) -> Option<ViewportPoint> {
        let (rotation, position) = pose(transform);
        let v = rotation.inverse() * (point - position);
        let depth = -v.z;
        if !depth.is_finite() || depth <= near(self) {
            return None;
        }
        let f = 1.0 / (self.fov_y * 0.5).tan();
        let ndc = Vec3::new(
            f / aspect_of(viewport) * v.x / depth,
            f * v.y / depth,
            near(self) / depth,
        );
        let normalized = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        Some(ViewportPoint {
            pixels: normalized * viewport,
            normalized,
            ndc,
            depth,
        })
    }

    /// The world-space ray from a camera at `transform` through viewport
    /// pixel `pixel` of a viewport of `viewport` size. It starts at the
    /// camera position; the viewport centre gives the camera's forward (−Z).
    pub fn viewport_ray(&self, transform: &GlobalTransform, viewport: Vec2, pixel: Vec2) -> Ray {
        let size = viewport.max(Vec2::ONE);
        let ndc = Vec2::new(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0);
        self.ndc_ray(transform, aspect_of(viewport), ndc)
    }

    /// [`Self::viewport_ray`] through normalized device coordinates `ndc`
    /// (−1..1, +Y up) of a viewport with `aspect` = width / height.
    pub fn ndc_ray(&self, transform: &GlobalTransform, aspect: f32, ndc: Vec2) -> Ray {
        let (rotation, position) = pose(transform);
        let tan = (self.fov_y * 0.5).tan();
        let local = Vec3::new(ndc.x * tan * aspect, ndc.y * tan, -1.0);
        Ray::new(position, rotation * local)
    }
}

impl ViewCamera {
    /// [`Camera::view_projection`] for this camera.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.camera.view_projection(&self.transform, aspect)
    }

    /// [`Camera::world_to_viewport`] for this camera.
    pub fn world_to_viewport(&self, viewport: Vec2, point: Vec3) -> Option<ViewportPoint> {
        self.camera
            .world_to_viewport(&self.transform, viewport, point)
    }

    /// [`Camera::viewport_ray`] for this camera.
    pub fn viewport_ray(&self, viewport: Vec2, pixel: Vec2) -> Ray {
        self.camera.viewport_ray(&self.transform, viewport, pixel)
    }

    /// [`Camera::ndc_ray`] for this camera.
    pub fn ndc_ray(&self, aspect: f32, ndc: Vec2) -> Ray {
        self.camera.ndc_ray(&self.transform, aspect, ndc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::transform::Transform;

    const SIZE: Vec2 = Vec2::new(1280.0, 720.0);

    fn cam() -> (Camera, GlobalTransform) {
        let t = Transform::from_xyz(3.0, 2.0, 5.0).looking_at(Vec3::new(-1.0, 0.5, -2.0), Vec3::Y);
        (Camera::default(), GlobalTransform(t.to_affine()))
    }

    fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
        (a - b).length() < eps
    }

    #[test]
    fn project_then_unproject_round_trips() {
        let (c, g) = cam();
        for p in [
            Vec3::new(-1.0, 0.5, -2.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(-4.0, 3.0, -10.0),
            Vec3::new(2.0, 1.0, 0.0),
        ] {
            let vp = c.world_to_viewport(&g, SIZE, p).expect("in front");
            let ray = c.viewport_ray(&g, SIZE, vp.pixels);
            let along = (p - ray.origin).dot(ray.direction);
            assert!(close(ray.at(along), p, 1e-3), "{p}: {:?}", ray.at(along));
            // Depth is the distance along the view axis.
            assert!((along * ray.direction.dot(g.forward()) - vp.depth).abs() < 1e-3);
        }
    }

    #[test]
    fn matches_the_renderer_matrix() {
        let (c, g) = cam();
        let p = Vec3::new(-2.0, 1.5, -6.0);
        let vp = c.world_to_viewport(&g, SIZE, p).unwrap();
        let clip = c.view_projection(&g, SIZE.x / SIZE.y) * p.extend(1.0);
        assert!(close(clip.truncate() / clip.w, vp.ndc, 1e-5));
        assert!((clip.w - vp.depth).abs() < 1e-4);
        // Reverse-Z: 1 on the near plane, smaller further away.
        let near = c
            .world_to_viewport(&g, SIZE, g.translation() + g.forward() * (c.near + 1e-3))
            .unwrap();
        assert!(near.ndc.z > 0.99 && vp.ndc.z < near.ndc.z && vp.ndc.z > 0.0);
    }

    #[test]
    fn centre_of_the_screen_is_the_forward_ray() {
        let (c, g) = cam();
        let ray = c.viewport_ray(&g, SIZE, SIZE * 0.5);
        assert!(close(ray.origin, g.translation(), 1e-6));
        assert!(close(ray.direction, g.forward(), 1e-6));
        let ahead = c
            .world_to_viewport(&g, SIZE, g.translation() + g.forward() * 7.0)
            .unwrap();
        assert!((ahead.pixels - SIZE * 0.5).length() < 1e-3);
        assert!((ahead.depth - 7.0).abs() < 1e-4);
        assert!(ahead.is_on_screen());
        // +Y up on screen is −Y in pixels; +X right is +X.
        let up = c
            .world_to_viewport(&g, SIZE, g.translation() + g.forward() * 7.0 + g.up())
            .unwrap();
        assert!(up.pixels.y < SIZE.y * 0.5 && (up.pixels.x - SIZE.x * 0.5).abs() < 1e-2);
        let right = c
            .world_to_viewport(&g, SIZE, g.translation() + g.forward() * 7.0 + g.right())
            .unwrap();
        assert!(right.pixels.x > SIZE.x * 0.5);
    }

    #[test]
    fn corners_follow_the_field_of_view() {
        let (c, g) = cam();
        let ray = c.viewport_ray(&g, SIZE, Vec2::new(SIZE.x * 0.5, 0.0));
        let angle = ray.direction.angle_between(g.forward());
        assert!((angle - c.fov_y * 0.5).abs() < 1e-4);
        let vp = c.world_to_viewport(&g, SIZE, ray.at(10.0)).unwrap();
        assert!(vp.normalized.y.abs() < 1e-4, "{:?}", vp.normalized);
        assert!((vp.normalized.x - 0.5).abs() < 1e-4);
    }

    #[test]
    fn points_behind_or_at_the_camera_do_not_project() {
        let (c, g) = cam();
        let eye = g.translation();
        assert!(
            c.world_to_viewport(&g, SIZE, eye - g.forward() * 3.0)
                .is_none()
        );
        assert!(c.world_to_viewport(&g, SIZE, eye).is_none());
        assert!(
            c.world_to_viewport(&g, SIZE, eye + g.forward() * (c.near * 0.5))
                .is_none()
        );
        assert!(c.world_to_viewport(&g, SIZE, Vec3::NAN).is_none());
        // Beside the view but in front: projected, off screen.
        let side = c
            .world_to_viewport(&g, SIZE, eye + g.forward() + g.right() * 50.0)
            .unwrap();
        assert!(!side.is_on_screen() && side.pixels.x > SIZE.x);
    }

    #[test]
    fn view_camera_forms_agree_and_scale_is_ignored() {
        let (c, g) = cam();
        let scaled = GlobalTransform(g.0 * glam::Affine3A::from_scale(Vec3::splat(3.0)));
        let v = ViewCamera {
            transform: scaled,
            camera: c,
        };
        let p = Vec3::new(-1.0, 0.0, -3.0);
        assert_eq!(
            v.world_to_viewport(SIZE, p).map(|x| x.pixels.round()),
            c.world_to_viewport(&g, SIZE, p).map(|x| x.pixels.round())
        );
        let a = v.viewport_ray(SIZE, Vec2::new(100.0, 600.0));
        let b = c.viewport_ray(&g, SIZE, Vec2::new(100.0, 600.0));
        assert!(close(a.direction, b.direction, 1e-5) && close(a.origin, b.origin, 1e-5));
    }
}
