//! A reusable orbit camera controller, and the active camera's projection
//! for games: [`world_to_screen`], [`screen_ray`] and [`cursor_ray`].

use box_core::input::Input;
use box_core::prelude::*;
use box_render::{Camera, ViewportPoint};

use crate::runner::WindowSize;

/// The world's first active [`Camera`] and its `GlobalTransform`: the
/// camera the renderer draws the game with.
pub fn active_camera(world: &World) -> Option<(Camera, GlobalTransform)> {
    world
        .query::<(&Camera, &GlobalTransform)>()
        .iter()
        .find(|(c, _)| c.active)
        .map(|(c, g)| (*c, *g))
}

/// The render target size in physical pixels ([`WindowSize`]), if known.
fn screen_size(world: &World) -> Option<Vec2> {
    let size = world.get_resource::<WindowSize>()?;
    (size.width > 0 && size.height > 0).then(|| Vec2::new(size.width as f32, size.height as f32))
}

/// Where world point `point` appears on screen through the
/// [active camera](active_camera), in physical pixels of [`WindowSize`]
/// (the coordinates of `Input::cursor_position`). `None` without an active
/// camera or a window size, or when the point is behind the camera; a point
/// in front but outside the view is returned
/// ([`ViewportPoint::is_on_screen`] is false).
///
/// The camera's `GlobalTransform` is as of the last transform propagation
/// (the start of `PostUpdate`): in `Update` a camera moved this frame is
/// seen where it was drawn last.
pub fn world_to_screen(world: &World, point: Vec3) -> Option<ViewportPoint> {
    let (camera, transform) = active_camera(world)?;
    camera.world_to_viewport(&transform, screen_size(world)?, point)
}

/// The world-space ray from the [active camera](active_camera) through
/// screen pixel `pixel` (physical pixels from the top-left, as
/// `Input::cursor_position`). `None` without an active camera or a window
/// size. Cast it with `Physics::raycast` to pick what is under a point.
pub fn screen_ray(world: &World, pixel: Vec2) -> Option<Ray> {
    let (camera, transform) = active_camera(world)?;
    Some(camera.viewport_ray(&transform, screen_size(world)?, pixel))
}

/// The ray under the mouse cursor: [`screen_ray`] at
/// `Input::cursor_position`. `None` when the cursor is outside the window
/// (or locked away), or without an active camera.
///
/// ```ignore
/// if let Some(ray) = cursor_ray(world) {
///     let hit = world.resource::<Physics>().raycast(&ray, 100.0, &QueryFilter::default());
/// }
/// ```
pub fn cursor_ray(world: &World) -> Option<Ray> {
    let cursor = world.get_resource::<Input>()?.cursor_position?;
    screen_ray(world, cursor)
}

/// Smallest distance the controller uses, so the eye never sits on the target.
const DISTANCE_FLOOR: f32 = 1.0e-3;
/// Largest distance the controller uses, so an "unlimited" `max_distance`
/// cannot put the eye at infinity.
const DISTANCE_CEILING: f32 = 1.0e9;
/// Largest |pitch| the controller uses, just short of straight up or down.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 1.0e-3;

/// `a` and `b` as a (low, high) pair inside `floor..=ceiling`. A NaN limit
/// counts as unset: it falls back to the other one, or to the full range.
fn ordered_limits(a: f32, b: f32, floor: f32, ceiling: f32) -> (f32, f32) {
    // `f32::min`/`max` return the other operand when one is NaN.
    let (low, high) = (a.min(b), a.max(b));
    if low.is_nan() {
        return (floor, ceiling);
    }
    (low.clamp(floor, ceiling), high.clamp(floor, ceiling))
}

/// Orbit/pan/zoom around a target point. Attach next to a `Camera` and a
/// `Transform`; [`orbit_camera_system`] writes the transform each frame.
///
/// Mouse: left or middle drag orbits, right drag (or Shift + left/middle) pans,
/// the wheel zooms toward the cursor. Trackpad: two-finger scroll orbits
/// (Shift pans), pinch zooms.
#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    /// Radians around +Y; 0 looks toward −Z from +Z.
    pub yaw: f32,
    /// Radians above the horizon (positive looks down on the target).
    pub pitch: f32,
    /// The limits below are sanitised when input is applied: swapped pairs
    /// are reordered, the distance stays positive and finite, and the pitch
    /// stops just short of ±90°.
    pub min_distance: f32,
    pub max_distance: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    /// Radians per pointer unit.
    pub orbit_speed: f32,
    /// Fraction of distance per pointer unit.
    pub pan_speed: f32,
    /// Fractional distance change per wheel line.
    pub zoom_speed: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 20.0,
            yaw: 0.6,
            pitch: 0.45,
            min_distance: 1.0,
            max_distance: 400.0,
            min_pitch: -1.2,
            max_pitch: 1.55,
            orbit_speed: 0.005,
            pan_speed: 0.0015,
            zoom_speed: 0.12,
        }
    }
}

impl OrbitCamera {
    /// Unit vector from the target toward the camera.
    pub fn offset_direction(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        )
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.offset_direction() * self.distance
    }

    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.eye()).looking_at(self.target, Vec3::Y)
    }

    /// The distance limits as used: ordered, never NaN, and within
    /// [`DISTANCE_FLOOR`]..=[`DISTANCE_CEILING`] whatever the public fields hold.
    fn distance_limits(&self) -> (f32, f32) {
        ordered_limits(
            self.min_distance,
            self.max_distance,
            DISTANCE_FLOOR,
            DISTANCE_CEILING,
        )
    }

    /// The pitch limits as used: ordered, never NaN, and short of the poles,
    /// where the look-at has no defined up direction.
    fn pitch_limits(&self) -> (f32, f32) {
        ordered_limits(self.min_pitch, self.max_pitch, -PITCH_LIMIT, PITCH_LIMIT)
    }

    /// Bring pitch and distance inside the limits. Cannot panic and leaves
    /// neither NaN: a NaN value lands on the lower limit.
    fn clamp(&mut self) {
        let (low, high) = self.pitch_limits();
        self.pitch = self.pitch.max(low).min(high);
        let (low, high) = self.distance_limits();
        self.distance = self.distance.max(low).min(high);
    }

    /// Apply one frame of input. `cursor_ndc` is the cursor in normalized
    /// device coordinates (−1..1, +Y up) and `tan_half_fov`/`aspect` describe
    /// the camera, for zooming toward the cursor.
    pub fn apply_input(
        &mut self,
        input: &Input,
        cursor_ndc: Option<Vec2>,
        tan_half_fov: f32,
        aspect: f32,
    ) {
        // The fields are public: start from a valid state (distance > 0).
        self.clamp();
        let pan_mode = input.modifiers.shift;
        let dragging_orbit =
            input.button_down(MouseButton::Left) || input.button_down(MouseButton::Middle);
        let dragging_pan = input.button_down(MouseButton::Right);

        let mut orbit = Vec2::ZERO;
        let mut pan = Vec2::ZERO;
        if (dragging_orbit && pan_mode) || dragging_pan {
            pan += input.mouse_delta;
        } else if dragging_orbit {
            orbit += input.mouse_delta;
        }
        if pan_mode {
            pan += input.scroll_pixels;
        } else {
            orbit -= input.scroll_pixels;
        }

        self.yaw -= orbit.x * self.orbit_speed;
        self.pitch += orbit.y * self.orbit_speed;

        if pan != Vec2::ZERO {
            let t = self.transform();
            let scale = self.distance * self.pan_speed;
            self.target += (-t.right() * pan.x + t.up() * pan.y) * scale;
        }

        let zoom_steps = input.scroll_lines.y + input.pinch * 8.0;
        if zoom_steps != 0.0 {
            let old = self.distance;
            let (low, high) = self.distance_limits();
            let new = (old * (1.0 - self.zoom_speed).powf(zoom_steps))
                .max(low)
                .min(high);
            if let Some(ndc) = cursor_ndc {
                // Move the target toward the point under the cursor on the plane
                // through the target facing the camera, so that point stays put.
                let t = self.transform();
                let half_h = tan_half_fov * old;
                let half_w = half_h * aspect;
                let under_cursor =
                    self.target + t.right() * (ndc.x * half_w) + t.up() * (ndc.y * half_h);
                self.target += (under_cursor - self.target) * (1.0 - new / old);
            }
            self.distance = new;
        }
        self.clamp();
    }
}

/// Updates every `(OrbitCamera, Transform)` from the `Input` resource.
pub fn orbit_camera_system(world: &mut World) {
    let Some(input) = world.get_resource::<Input>().cloned() else {
        return;
    };
    let size = world
        .get_resource::<WindowSize>()
        .copied()
        .unwrap_or_default();
    let cursor_ndc = input.cursor_position.and_then(|p| {
        (size.width > 0 && size.height > 0).then(|| {
            Vec2::new(
                p.x / size.width as f32 * 2.0 - 1.0,
                1.0 - p.y / size.height as f32 * 2.0,
            )
        })
    });
    let aspect = if size.height > 0 {
        size.width as f32 / size.height as f32
    } else {
        16.0 / 9.0
    };
    for (orbit, transform, camera) in world
        .query::<(&mut OrbitCamera, &mut Transform, Option<&Camera>)>()
        .iter()
    {
        let fov = camera.map(|c| c.fov_y).unwrap_or(50f32.to_radians());
        orbit.apply_input(&input, cursor_ndc, (fov * 0.5).tan(), aspect);
        *transform = orbit.transform();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    #[test]
    fn transform_looks_at_target_from_distance() {
        let o = OrbitCamera {
            target: Vec3::new(1.0, 2.0, 3.0),
            distance: 10.0,
            ..Default::default()
        };
        let t = o.transform();
        assert!(((t.translation - o.target).length() - 10.0).abs() < 1e-4);
        assert!(approx(t.forward(), (o.target - t.translation).normalize()));
    }

    #[test]
    fn drag_orbits_wheel_zooms_right_drag_pans() {
        let mut o = OrbitCamera::default();
        let mut input = Input::default();
        input.press_button(MouseButton::Left);
        input.mouse_delta = Vec2::new(100.0, -50.0);
        let (yaw, pitch, target) = (o.yaw, o.pitch, o.target);
        o.apply_input(&input, None, 0.5, 1.5);
        assert!((o.yaw - (yaw - 0.5)).abs() < 1e-5);
        assert!((o.pitch - (pitch - 0.25)).abs() < 1e-5);
        assert_eq!(o.target, target);

        let mut input = Input::default();
        input.scroll_lines = Vec2::new(0.0, 2.0);
        let d = o.distance;
        o.apply_input(&input, Some(Vec2::ZERO), 0.5, 1.5);
        assert!((o.distance - d * 0.88f32.powi(2)).abs() < 1e-3);
        assert!(
            approx(o.target, target),
            "zoom at screen centre keeps the target"
        );

        let mut input = Input::default();
        input.press_button(MouseButton::Right);
        input.mouse_delta = Vec2::new(10.0, 0.0);
        o.apply_input(&input, None, 0.5, 1.5);
        assert!(o.target != target);
        assert!(
            (o.target - target).dot(o.transform().right()) < 0.0,
            "drag right moves scene right"
        );
    }

    #[test]
    fn zoom_toward_cursor_moves_target_toward_cursor() {
        let mut o = OrbitCamera::default();
        let mut input = Input::default();
        input.scroll_lines = Vec2::new(0.0, 1.0);
        let right = o.transform().right();
        o.apply_input(&input, Some(Vec2::new(1.0, 0.0)), 0.5, 1.0);
        assert!(o.target.dot(right) > 0.0);
    }

    #[test]
    fn pitch_and_distance_are_clamped() {
        let mut o = OrbitCamera::default();
        let mut input = Input::default();
        input.press_button(MouseButton::Left);
        input.mouse_delta = Vec2::new(0.0, 1.0e6);
        o.apply_input(&input, None, 0.5, 1.0);
        assert_eq!(o.pitch, o.max_pitch);
        let mut input = Input::default();
        input.scroll_lines = Vec2::new(0.0, 1000.0);
        o.apply_input(&input, None, 0.5, 1.0);
        assert_eq!(o.distance, o.min_distance);
    }

    fn run_system_with_input(orbit: OrbitCamera) -> (Transform, OrbitCamera) {
        let mut w = World::new();
        let mut input = Input::default();
        input.cursor_position = Some(Vec2::new(10.0, 20.0));
        input.scroll_lines = Vec2::new(0.0, 3.0);
        input.press_button(MouseButton::Right);
        input.mouse_delta = Vec2::new(5.0, 5.0);
        w.insert_resource(input);
        w.insert_resource(WindowSize {
            width: 800,
            height: 600,
            scale_factor_milli: 1000,
        });
        let e = w.spawn((orbit, Transform::default()));
        orbit_camera_system(&mut w);
        orbit_camera_system(&mut w);
        (
            *w.get::<Transform>(e).unwrap(),
            *w.get::<OrbitCamera>(e).unwrap(),
        )
    }

    fn assert_sane(case: &str, orbit: OrbitCamera) {
        let (t, o) = run_system_with_input(orbit);
        assert!(
            t.translation.is_finite() && t.rotation.is_finite() && o.target.is_finite(),
            "{case}: {t:?} {o:?}"
        );
        assert!(o.distance > 0.0 && o.distance.is_finite(), "{case}: {o:?}");
        assert!(o.pitch.abs() < std::f32::consts::FRAC_PI_2, "{case}: {o:?}");
    }

    #[test]
    fn swapped_limits_do_not_panic() {
        assert_sane(
            "min > max",
            OrbitCamera {
                min_distance: 10.0,
                max_distance: 1.0,
                min_pitch: 1.0,
                max_pitch: -1.0,
                ..Default::default()
            },
        );
    }

    #[test]
    fn nan_limits_do_not_panic() {
        assert_sane(
            "NaN limits",
            OrbitCamera {
                min_distance: f32::NAN,
                max_distance: f32::NAN,
                min_pitch: f32::NAN,
                max_pitch: f32::NAN,
                ..Default::default()
            },
        );
    }

    #[test]
    fn zero_distance_does_not_produce_nan() {
        assert_sane(
            "zero distance",
            OrbitCamera {
                distance: 0.0,
                min_distance: 0.0,
                ..Default::default()
            },
        );
    }

    #[test]
    fn pitch_stays_off_the_poles() {
        assert_sane(
            "straight down",
            OrbitCamera {
                pitch: 10.0,
                max_pitch: 10.0,
                ..Default::default()
            },
        );
        assert_sane(
            "straight up, unlimited distance",
            OrbitCamera {
                pitch: -10.0,
                min_pitch: -10.0,
                distance: f32::INFINITY,
                max_distance: f32::INFINITY,
                ..Default::default()
            },
        );
    }

    #[test]
    fn system_writes_transform() {
        let mut w = World::new();
        w.insert_resource(Input::default());
        let e = w.spawn((OrbitCamera::default(), Transform::default()));
        orbit_camera_system(&mut w);
        let expected = OrbitCamera::default().transform();
        assert!(approx(
            w.get::<Transform>(e).unwrap().translation,
            expected.translation
        ));
    }

    #[test]
    fn screen_helpers_use_the_active_camera_window_and_cursor() {
        let mut w = World::new();
        let eye = Transform::from_xyz(0.0, 2.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y);
        // An inactive camera elsewhere is ignored.
        w.spawn((
            Camera {
                active: false,
                ..Default::default()
            },
            GlobalTransform(Transform::from_xyz(50.0, 0.0, 0.0).to_affine()),
        ));
        w.spawn((Camera::default(), GlobalTransform(eye.to_affine())));
        assert!(
            world_to_screen(&w, Vec3::ZERO).is_none(),
            "no window size yet"
        );
        w.insert_resource(WindowSize {
            width: 800,
            height: 600,
            scale_factor_milli: 2000,
        });
        w.insert_resource(Input::default());
        let centre = world_to_screen(&w, Vec3::ZERO).unwrap();
        assert!((centre.pixels - Vec2::new(400.0, 300.0)).length() < 1e-3);
        assert!(cursor_ray(&w).is_none(), "cursor outside the window");
        w.resource_mut::<Input>().cursor_position = Some(Vec2::new(400.0, 300.0));
        let ray = cursor_ray(&w).unwrap();
        assert!(approx(ray.origin, eye.translation));
        assert!(approx(ray.direction, eye.forward()));
        // A point off centre round-trips through the screen.
        let p = Vec3::new(1.5, 0.5, -1.0);
        let s = world_to_screen(&w, p).unwrap();
        let r = screen_ray(&w, s.pixels).unwrap();
        assert!(approx(r.at((p - r.origin).length()), p));
        assert!(
            world_to_screen(&w, Vec3::new(0.0, 2.0, 20.0)).is_none(),
            "behind"
        );
    }
}
