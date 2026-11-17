//! The editor camera: box_engine's [`OrbitCamera`] driven from egui input over
//! the viewport. It is not a scene entity and is never saved with the scene.
//!
//! Besides orbit, pan and zoom it flies (hold the right button: the mouse
//! looks around the eye, W A S D / Q E move, Shift is faster, the wheel sets
//! the speed) and turns to look along a world axis with a short animation
//! (the axis gizmo). Flying keeps the orbit model: the target stays at the
//! orbit distance in front of the eye, so releasing the button orbits
//! around a point in front of where the flight ended.

use box_core::input::{Input, Modifiers, MouseButton};
use box_core::math::Aabb;
use box_core::prelude::GlobalTransform;
use box_engine::OrbitCamera;
use box_render::{Camera, ViewCamera};
use glam::{Vec2, Vec3};

use crate::state::{EditorState, scene_bounds};

/// Mouse, trackpad and fly navigation as [`EditorCamera::apply`] implements
/// it, plus the viewport's modifier clicks, for Help ▸ Keyboard Shortcuts
/// (keep them together). Alt + left is orbit, so inspecting a generated part
/// uses Cmd + Alt (a click without movement never orbits).
pub const NAVIGATION: &[(&str, &str)] = &[
    ("Orbit", "Alt + left drag, or two-finger scroll"),
    ("Pan", "Middle drag, or Shift + two-finger scroll"),
    (
        "Zoom",
        "Mouse wheel (toward the cursor), Alt + right drag, or pinch",
    ),
    (
        "Fly",
        "Hold the right button: move the mouse to look, W A S D to move, Q / E down and up, \
         Shift faster, wheel sets the speed",
    ),
    (
        "View along an axis",
        "Click an axis of the axis gizmo (top right); click it again for the opposite side",
    ),
    (
        "Inspect a generated part",
        "Cmd + Alt + click it (Ctrl + Alt elsewhere): shown read-only in the inspector; \
         a plain click selects the object that generates it",
    ),
];

/// The short form shown in the viewport's navigation hint.
pub const NAVIGATION_HINT: &str = "Alt+drag orbit  ·  Middle-drag pan  ·  Scroll zoom  ·  \
                                   Right-drag + WASD fly  ·  F frame";

/// Radians of mouse look per pointer point while flying.
const LOOK_SPEED: f32 = 0.004;
/// Fly speed range (m/s) and the factor per wheel step.
pub const FLY_SPEED_MIN: f32 = 0.1;
pub const FLY_SPEED_MAX: f32 = 500.0;
const FLY_SPEED_STEP: f32 = 1.2;
/// Shift while flying multiplies the speed by this.
const FLY_FAST: f32 = 3.0;
/// Wheel lines per point of trackpad scroll while flying.
const POINTS_PER_LINE: f32 = 40.0;
/// Wheel lines of zoom per point of Alt + right drag.
const DRAG_ZOOM: f32 = 0.02;
/// Seconds an axis-view turn takes.
const TURN_SECONDS: f32 = 0.22;
/// Pitch limit of the editor camera: straight up and down are allowed (the
/// orbit controller stops just short of them).
const PITCH: f32 = std::f32::consts::FRAC_PI_2;

/// The keys that move the camera while flying (held this frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlyKeys {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub fast: bool,
}

/// A turn of the view to new angles (axis views).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    from: (f32, f32),
    to: (f32, f32),
    /// 0..1.
    t: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct EditorCamera {
    pub orbit: OrbitCamera,
    pub camera: Camera,
    /// Fly speed in metres per second (the wheel changes it while flying).
    pub fly_speed: f32,
    /// The right button is held over the viewport: mouse look and WASD.
    pub flying: bool,
    turn: Option<Turn>,
}

impl Default for EditorCamera {
    fn default() -> Self {
        Self {
            orbit: OrbitCamera {
                target: Vec3::new(0.0, 2.0, 0.0),
                distance: 30.0,
                yaw: 0.75,
                pitch: 0.42,
                min_distance: 0.2,
                max_distance: 2000.0,
                min_pitch: -PITCH,
                max_pitch: PITCH,
                ..Default::default()
            },
            camera: Camera::default(),
            fly_speed: 5.0,
            flying: false,
            turn: None,
        }
    }
}

/// Pointer state over the viewport for one frame, in egui points.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewportPointer {
    /// A drag that started on the viewport is still held.
    pub dragging: bool,
    pub primary: bool,
    pub middle: bool,
    pub secondary: bool,
    pub delta: Vec2,
    /// Cursor in normalized device coordinates (−1..1, +Y up), when over the
    /// viewport.
    pub cursor_ndc: Option<Vec2>,
    /// Wheel and trackpad input is only taken while hovered.
    pub hovered: bool,
    pub scroll_lines: Vec2,
    pub scroll_points: Vec2,
    /// Natural log of the pinch zoom factor (positive zooms in).
    pub pinch: f32,
    pub modifiers: Modifiers,
    /// Fly keys held this frame (used only while flying).
    pub keys: FlyKeys,
    /// Seconds since the last frame (fly movement).
    pub dt: f32,
}

/// What [`EditorCamera::apply`] did that the viewport shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Applied {
    /// The wheel changed the fly speed.
    pub fly_speed_changed: bool,
}

/// Orbit angles (yaw, pitch) of an eye at `dir` (unit) from the target.
fn angles_of(dir: Vec3) -> (f32, f32) {
    let dir = dir.normalize_or(Vec3::Z);
    (dir.x.atan2(dir.z), dir.y.clamp(-1.0, 1.0).asin())
}

/// Orbit angles that look from `side` (unit) of the target. Straight up or
/// down keeps −Z up on the screen (yaw 0).
fn side_angles(side: Vec3) -> (f32, f32) {
    let limit = PITCH - 1.0e-3;
    if side.y.abs() > 0.999 {
        (0.0, limit * side.y.signum())
    } else {
        angles_of(side)
    }
}

/// `a` minus `b` wrapped into −π..π.
fn wrap(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a - b + PI).rem_euclid(TAU) - PI
}

impl EditorCamera {
    pub fn view(&self) -> ViewCamera {
        ViewCamera::new(&self.orbit.transform(), self.camera)
    }

    /// Navigation (EDITOR.md section 5): Alt + left drag orbits, middle drag
    /// (or Shift + Alt + left) pans, the wheel zooms toward the cursor,
    /// Alt + right drag zooms, two-finger trackpad scroll orbits (Shift pans),
    /// pinch zooms, and holding the right button flies. A plain left drag is
    /// left for selection and gizmos.
    pub fn apply(&mut self, p: &ViewportPointer, aspect: f32) -> Applied {
        let fly = p.dragging && p.secondary && !p.modifiers.alt;
        if fly {
            self.turn = None;
            return self.fly(p);
        }
        self.flying = false;
        let mut input = Input::default();
        input.modifiers = p.modifiers;
        let mut cursor = p.cursor_ndc;
        if p.dragging {
            let orbit = p.primary && p.modifiers.alt;
            let pan = p.middle;
            if orbit {
                // OrbitCamera: middle orbits (Shift + middle pans).
                input.press_button(MouseButton::Middle);
            }
            if pan {
                input.press_button(MouseButton::Right);
            }
            if orbit || pan {
                input.mouse_delta = p.delta;
            }
            if p.secondary && p.modifiers.alt {
                // Drag right or up zooms in, toward the view centre.
                input.scroll_lines.y += (p.delta.x - p.delta.y) * DRAG_ZOOM;
                cursor = None;
            }
        }
        if p.hovered {
            input.scroll_lines += p.scroll_lines;
            input.scroll_pixels = p.scroll_points;
            input.pinch = p.pinch;
        }
        let moved = input.mouse_delta != Vec2::ZERO
            || input.scroll_lines != Vec2::ZERO
            || input.scroll_pixels != Vec2::ZERO
            || input.pinch != 0.0;
        if moved {
            self.turn = None;
        }
        let tan_half_fov = (self.camera.fov_y * 0.5).tan();
        self.orbit.apply_input(&input, cursor, tan_half_fov, aspect);
        Applied::default()
    }

    /// One frame of flight: mouse look turns the view about the eye, the
    /// keys move the eye (and the target with it), the wheel sets the speed.
    fn fly(&mut self, p: &ViewportPointer) -> Applied {
        self.flying = true;
        let mut applied = Applied::default();
        let eye = self.orbit.eye();
        let limit = PITCH - 1.0e-3;
        self.orbit.yaw -= p.delta.x * LOOK_SPEED;
        self.orbit.pitch = (self.orbit.pitch + p.delta.y * LOOK_SPEED).clamp(-limit, limit);
        if p.hovered {
            let steps = p.scroll_lines.y + p.scroll_points.y / POINTS_PER_LINE;
            if steps != 0.0 {
                self.fly_speed = (self.fly_speed * FLY_SPEED_STEP.powf(steps))
                    .clamp(FLY_SPEED_MIN, FLY_SPEED_MAX);
                applied.fly_speed_changed = true;
            }
        }
        self.orbit.target = eye - self.orbit.offset_direction() * self.orbit.distance;
        let t = self.orbit.transform();
        let k = p.keys;
        let axis = |pos: bool, neg: bool| pos as i32 as f32 - neg as i32 as f32;
        let dir = t.forward() * axis(k.forward, k.back)
            + t.right() * axis(k.right, k.left)
            + Vec3::Y * axis(k.up, k.down);
        if dir != Vec3::ZERO {
            let speed = self.fly_speed * if k.fast { FLY_FAST } else { 1.0 };
            self.orbit.target += dir.normalize() * speed * p.dt.clamp(0.0, 0.25);
        }
        applied
    }

    /// Turns the view (with a short animation, see [`Self::tick`]) to look
    /// at the target from the `side` of it (a unit axis such as `Vec3::X`:
    /// the eye on +X, looking along −X). When the view already looks from
    /// that side, it turns to the opposite side (the axis gizmo).
    pub fn look_from(&mut self, side: Vec3) {
        let current = self
            .turn
            .map_or((self.orbit.yaw, self.orbit.pitch), |t| t.to);
        let to = side_angles(side);
        let same = (current.1 - to.1).abs() < 1e-3
            && (side.y.abs() > 0.999 || wrap(current.0, to.0).abs() < 1e-3);
        self.turn_to(if same { -side } else { side });
    }

    /// Like [`Self::look_from`], always from `side` (View menu).
    pub fn turn_to(&mut self, side: Vec3) {
        self.turn = Some(Turn {
            from: (self.orbit.yaw, self.orbit.pitch),
            to: side_angles(side),
            t: 0.0,
        });
    }

    /// Advances an axis-view turn by `dt` seconds. Returns true while one
    /// is in progress (the viewport keeps repainting).
    pub fn tick(&mut self, dt: f32) -> bool {
        let Some(turn) = &mut self.turn else {
            return false;
        };
        turn.t = (turn.t + dt.max(0.0) / TURN_SECONDS).min(1.0);
        // Ease out.
        let s = 1.0 - (1.0 - turn.t).powi(3);
        let (fy, fp) = turn.from;
        let (ty, tp) = turn.to;
        self.orbit.yaw = fy + wrap(ty, fy) * s;
        self.orbit.pitch = fp + (tp - fp) * s;
        if turn.t >= 1.0 {
            self.orbit.yaw = ty;
            self.turn = None;
            return false;
        }
        true
    }

    /// An axis-view turn is in progress.
    pub fn turning(&self) -> bool {
        self.turn.is_some()
    }

    /// Distance at which the bounding sphere of `bounds` fits the vertical
    /// field of view.
    fn fit_distance(&self, bounds: &Aabb) -> f32 {
        let radius = bounds.half_extents().length().max(0.05);
        radius / (self.camera.fov_y * 0.5).sin() * 1.05
    }

    /// Moves the orbit target to the centre of `bounds` and backs off until
    /// its bounding sphere fits the vertical field of view.
    pub fn frame(&mut self, bounds: &Aabb) {
        if bounds.is_empty() {
            return;
        }
        self.turn = None;
        self.orbit.target = bounds.center();
        self.orbit.distance = self
            .fit_distance(bounds)
            .clamp(self.orbit.min_distance, self.orbit.max_distance);
    }

    /// The view a scene opens with, from the default angle: centred on
    /// `bounds` (the non-flat objects) and the scene camera's eye `game`
    /// (position, forward) so both are in view, and never closer to the
    /// objects than the scene camera is. With a camera and no objects, the
    /// scene camera's view.
    pub fn open_on(&mut self, bounds: Option<Aabb>, game: Option<(Vec3, Vec3)>) {
        let speed = self.fly_speed;
        *self = Self {
            fly_speed: speed,
            ..Self::default()
        };
        const MIN_HALF: f32 = 1.0;
        match (bounds, game) {
            (Some(b), game) => {
                let all = game.map_or(b, |(eye, _)| b.including(eye));
                let h = all.half_extents().max(Vec3::splat(MIN_HALF));
                let padded = Aabb::new(all.center() - h, all.center() + h);
                let mut distance = self.fit_distance(&padded);
                if let Some((eye, forward)) = game {
                    distance = distance.max((b.center() - eye).dot(forward));
                }
                self.orbit.target = all.center();
                self.orbit.distance =
                    distance.clamp(self.orbit.min_distance, self.orbit.max_distance);
            }
            (None, Some((eye, forward))) => {
                (self.orbit.yaw, self.orbit.pitch) = angles_of(-forward);
                self.orbit.distance = 10.0;
                self.orbit.target = eye + forward * self.orbit.distance;
            }
            (None, None) => {}
        }
    }
}

impl EditorState {
    /// Frames a scene that was just opened (project, scene file, new scene
    /// or the scratch scene; never undo or Play): see
    /// [`EditorCamera::open_on`]. The scene camera is the first active
    /// `Camera`.
    pub fn frame_on_open(&mut self) {
        let game = self
            .world
            .query::<(&Camera, &GlobalTransform)>()
            .iter()
            .find(|(c, _)| c.active)
            .map(|(_, gt)| (gt.translation(), gt.forward()));
        let bounds = scene_bounds(&self.world);
        self.camera.open_on(bounds, game);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_left_drag_does_not_move_the_camera() {
        let mut cam = EditorCamera::default();
        let before = cam.orbit.transform();
        let p = ViewportPointer {
            dragging: true,
            primary: true,
            delta: Vec2::new(40.0, 10.0),
            ..Default::default()
        };
        cam.apply(&p, 1.5);
        assert_eq!(cam.orbit.transform(), before);

        let alt = ViewportPointer {
            modifiers: Modifiers {
                alt: true,
                ..Default::default()
            },
            ..p
        };
        cam.apply(&alt, 1.5);
        assert_ne!(cam.orbit.yaw, EditorCamera::default().orbit.yaw);
    }

    #[test]
    fn middle_drag_pans_and_alt_right_drag_zooms() {
        let mut cam = EditorCamera::default();
        let start = cam;
        cam.apply(
            &ViewportPointer {
                dragging: true,
                middle: true,
                delta: Vec2::new(30.0, 0.0),
                ..Default::default()
            },
            1.5,
        );
        assert_eq!(cam.orbit.yaw, start.orbit.yaw, "no orbit");
        assert_ne!(cam.orbit.target, start.orbit.target, "panned");
        let d = cam.orbit.distance;
        cam.apply(
            &ViewportPointer {
                dragging: true,
                secondary: true,
                delta: Vec2::new(40.0, 0.0),
                modifiers: Modifiers {
                    alt: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            1.5,
        );
        assert!(cam.orbit.distance < d, "zoomed in");
        assert!(!cam.flying);
    }

    #[test]
    fn wheel_is_ignored_unless_hovered() {
        let mut cam = EditorCamera::default();
        let d = cam.orbit.distance;
        let p = ViewportPointer {
            scroll_lines: Vec2::new(0.0, 2.0),
            ..Default::default()
        };
        cam.apply(&p, 1.5);
        assert_eq!(cam.orbit.distance, d);
        cam.apply(&ViewportPointer { hovered: true, ..p }, 1.5);
        assert!(cam.orbit.distance < d);
    }

    #[test]
    fn flying_looks_about_the_eye_and_moves_with_the_keys() {
        let mut cam = EditorCamera::default();
        let eye = cam.orbit.eye();
        let rmb = ViewportPointer {
            dragging: true,
            secondary: true,
            hovered: true,
            dt: 0.2,
            ..Default::default()
        };
        cam.apply(
            &ViewportPointer {
                delta: Vec2::new(50.0, -20.0),
                ..rmb
            },
            1.5,
        );
        assert!(cam.flying);
        assert!(cam.orbit.eye().distance(eye) < 1e-3, "the eye stays put");
        assert_ne!(cam.orbit.yaw, EditorCamera::default().orbit.yaw);
        let forward = cam.orbit.transform().forward();
        cam.apply(
            &ViewportPointer {
                keys: FlyKeys {
                    forward: true,
                    ..Default::default()
                },
                ..rmb
            },
            1.5,
        );
        let moved = cam.orbit.eye() - eye;
        assert!((moved - forward).length() < 1e-3, "{moved}");
        // The wheel sets the speed instead of zooming.
        let d = cam.orbit.distance;
        let a = cam.apply(
            &ViewportPointer {
                scroll_lines: Vec2::new(0.0, 1.0),
                ..rmb
            },
            1.5,
        );
        assert!(a.fly_speed_changed);
        assert!((cam.fly_speed - 6.0).abs() < 1e-4);
        assert_eq!(cam.orbit.distance, d);
        // Releasing orbits around a point in front, at the same distance.
        cam.apply(&ViewportPointer::default(), 1.5);
        assert!(!cam.flying);
        assert!(((cam.orbit.eye() - cam.orbit.target).length() - d).abs() < 1e-3);
    }

    #[test]
    fn look_from_turns_to_the_axis_and_back() {
        let mut cam = EditorCamera::default();
        cam.look_from(Vec3::X);
        while cam.tick(1.0 / 60.0) {}
        let f = cam.orbit.transform().forward();
        assert!((f - Vec3::NEG_X).length() < 1e-3, "{f}");
        cam.look_from(Vec3::X);
        while cam.tick(1.0 / 60.0) {}
        let f = cam.orbit.transform().forward();
        assert!((f - Vec3::X).length() < 1e-3, "opposite side: {f}");
        cam.look_from(Vec3::Y);
        while cam.tick(1.0 / 60.0) {}
        let f = cam.orbit.transform().forward();
        assert!((f - Vec3::NEG_Y).length() < 2e-3, "top: {f}");
    }

    #[test]
    fn frame_fits_the_bounds() {
        let mut cam = EditorCamera::default();
        cam.frame(&Aabb::new(
            Vec3::new(9.0, 0.0, -1.0),
            Vec3::new(11.0, 2.0, 1.0),
        ));
        assert_eq!(cam.orbit.target, Vec3::new(10.0, 1.0, 0.0));
        let radius = 3f32.sqrt();
        let expected = radius / (cam.camera.fov_y * 0.5).sin() * 1.05;
        assert!((cam.orbit.distance - expected).abs() < 1e-4);
    }

    #[test]
    fn opening_shows_the_objects_and_the_scene_camera() {
        let cube = Aabb::new(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        let eye = Vec3::new(2.6, 1.5, 3.6);
        let forward = (Vec3::new(0.0, 0.5, 0.0) - eye).normalize();
        let mut cam = EditorCamera::default();
        cam.open_on(Some(cube), Some((eye, forward)));
        assert_eq!(cam.orbit.yaw, EditorCamera::default().orbit.yaw);
        assert!(cam.orbit.distance >= (cube.center() - eye).dot(forward) - 1e-3);
        let view = cam.orbit.transform();
        let half = (cam.camera.fov_y * 0.5).tan();
        for p in [cube.min, cube.max, eye] {
            let local = view.to_affine().inverse().transform_point3(p);
            assert!(local.z < 0.0, "{p} in front");
            assert!((local.y / -local.z).abs() < half, "{p} in view");
        }
        // Without a camera the objects alone are fitted.
        let mut cam = EditorCamera::default();
        cam.open_on(Some(cube), None);
        assert_eq!(cam.orbit.target, cube.center());
    }
}
