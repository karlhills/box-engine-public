//! Input state, independent of the windowing library.
//!
//! The runner translates platform events (winit, gilrs) into calls on
//! [`Input`] and calls [`Input::end_frame`] after each frame. The editor feeds
//! a Play world the same way. Games usually read named actions
//! ([`crate::action`]) rather than raw devices.

use std::collections::{BTreeMap, HashSet};

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Keys the engine knows about. Unlisted keys are ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Command on macOS, the Windows key elsewhere.
    pub meta: bool,
}

/// A connected gamepad, as numbered by the platform backend. Ids are not
/// reused within a process while the backend runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GamepadId(pub u32);

/// Gamepad buttons by position (Xbox layout names in comments).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadButton {
    /// Bottom face button (A on Xbox, Cross on PlayStation).
    South,
    /// Right face button (B, Circle).
    East,
    /// Left face button (X, Square).
    West,
    /// Top face button (Y, Triangle).
    North,
    /// Left bumper (LB, L1).
    LeftShoulder,
    /// Right bumper (RB, R1).
    RightShoulder,
    /// Left trigger pressed past the backend's threshold (LT, L2). The analog
    /// value is [`GamepadAxis::LeftTrigger`].
    LeftTrigger,
    /// Right trigger pressed past the backend's threshold (RT, R2).
    RightTrigger,
    /// Left stick click (LS, L3).
    LeftStick,
    /// Right stick click (RS, R3).
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    /// Start / Menu / Options.
    Start,
    /// Select / View / Share.
    Select,
    /// The logo button, where the platform delivers it.
    Mode,
}

/// Analog gamepad axes. Sticks are in -1..=1 with +X right and +Y up;
/// triggers are in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
}

impl GamepadAxis {
    fn index(self) -> usize {
        self as usize
    }

    /// Triggers are 0..=1; sticks are -1..=1.
    pub fn is_trigger(self) -> bool {
        matches!(self, GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger)
    }
}

/// Default for [`Input::gamepad_dead_zone`].
pub const DEFAULT_GAMEPAD_DEAD_ZONE: f32 = 0.1;

/// Button and axis state of one connected gamepad.
#[derive(Clone, Debug, Default)]
pub struct GamepadState {
    name: String,
    down: HashSet<GamepadButton>,
    pressed: HashSet<GamepadButton>,
    released: HashSet<GamepadButton>,
    axes: [f32; 6],
}

impl GamepadState {
    /// The name the platform reports, or empty when unknown.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn button_down(&self, button: GamepadButton) -> bool {
        self.down.contains(&button)
    }

    pub fn button_pressed(&self, button: GamepadButton) -> bool {
        self.pressed.contains(&button)
    }

    pub fn button_released(&self, button: GamepadButton) -> bool {
        self.released.contains(&button)
    }

    /// The axis value as the device reports it, without a dead zone.
    pub fn axis_raw(&self, axis: GamepadAxis) -> f32 {
        self.axes[axis.index()]
    }

    fn press(&mut self, button: GamepadButton) {
        if self.down.insert(button) {
            self.pressed.insert(button);
        }
    }

    fn release(&mut self, button: GamepadButton) {
        if self.down.remove(&button) {
            self.released.insert(button);
        }
    }

    fn release_all(&mut self) {
        let down: Vec<_> = self.down.iter().copied().collect();
        for b in down {
            self.release(b);
        }
        self.axes = [0.0; 6];
    }
}

/// Apply an axial dead zone and rescale so the output starts at 0 at the edge
/// of the dead zone and still reaches 1 at full deflection.
pub fn apply_dead_zone(value: f32, dead_zone: f32) -> f32 {
    let dz = dead_zone.clamp(0.0, 0.99);
    let magnitude = value.abs();
    if magnitude <= dz {
        0.0
    } else {
        value.signum() * ((magnitude - dz) / (1.0 - dz)).min(1.0)
    }
}

/// Apply a radial dead zone to a stick: below `dead_zone` the stick is at
/// rest; above it the length is rescaled to 0..=1, keeping the direction.
pub fn apply_radial_dead_zone(value: Vec2, dead_zone: f32) -> Vec2 {
    let len = value.length();
    let scaled = apply_dead_zone(len, dead_zone);
    if scaled == 0.0 {
        Vec2::ZERO
    } else {
        value * (scaled / len)
    }
}

/// Keyboard, mouse and gamepad state for the current frame.
#[derive(Clone, Debug)]
pub struct Input {
    keys_down: HashSet<KeyCode>,
    keys_pressed: HashSet<KeyCode>,
    keys_released: HashSet<KeyCode>,
    buttons_down: HashSet<MouseButton>,
    buttons_pressed: HashSet<MouseButton>,
    buttons_released: HashSet<MouseButton>,
    gamepads: BTreeMap<GamepadId, GamepadState>,
    generation: u64,
    /// Cursor position in physical pixels, if the cursor is over the window.
    pub cursor_position: Option<Vec2>,
    /// Raw pointer motion this frame, in device units, accumulated from every
    /// motion event since the last [`end_frame`](Self::end_frame). Keeps
    /// reporting motion while the cursor is locked.
    pub mouse_delta: Vec2,
    /// Mouse-wheel motion this frame, in lines.
    pub scroll_lines: Vec2,
    /// Trackpad scroll this frame, in logical pixels.
    pub scroll_pixels: Vec2,
    /// Trackpad pinch this frame (positive zooms in).
    pub pinch: f32,
    pub modifiers: Modifiers,
    /// Axial dead zone applied by [`gamepad_axis`](Self::gamepad_axis).
    /// Actions apply their own dead zone to the raw values.
    pub gamepad_dead_zone: f32,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            keys_down: HashSet::new(),
            keys_pressed: HashSet::new(),
            keys_released: HashSet::new(),
            buttons_down: HashSet::new(),
            buttons_pressed: HashSet::new(),
            buttons_released: HashSet::new(),
            gamepads: BTreeMap::new(),
            generation: 0,
            cursor_position: None,
            mouse_delta: Vec2::ZERO,
            scroll_lines: Vec2::ZERO,
            scroll_pixels: Vec2::ZERO,
            pinch: 0.0,
            modifiers: Modifiers::default(),
            gamepad_dead_zone: DEFAULT_GAMEPAD_DEAD_ZONE,
        }
    }
}

impl Input {
    pub fn key_down(&self, key: KeyCode) -> bool {
        self.keys_down.contains(&key)
    }

    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.keys_pressed.contains(&key)
    }

    pub fn key_released(&self, key: KeyCode) -> bool {
        self.keys_released.contains(&key)
    }

    pub fn button_down(&self, button: MouseButton) -> bool {
        self.buttons_down.contains(&button)
    }

    pub fn button_pressed(&self, button: MouseButton) -> bool {
        self.buttons_pressed.contains(&button)
    }

    pub fn button_released(&self, button: MouseButton) -> bool {
        self.buttons_released.contains(&button)
    }

    /// Connected gamepads, in id order.
    pub fn gamepads(&self) -> impl Iterator<Item = (GamepadId, &GamepadState)> {
        self.gamepads.iter().map(|(id, g)| (*id, g))
    }

    pub fn gamepad(&self, id: GamepadId) -> Option<&GamepadState> {
        self.gamepads.get(&id)
    }

    pub fn gamepad_button_down(&self, id: GamepadId, button: GamepadButton) -> bool {
        self.gamepad(id).is_some_and(|g| g.button_down(button))
    }

    pub fn gamepad_button_pressed(&self, id: GamepadId, button: GamepadButton) -> bool {
        self.gamepad(id).is_some_and(|g| g.button_pressed(button))
    }

    pub fn gamepad_button_released(&self, id: GamepadId, button: GamepadButton) -> bool {
        self.gamepad(id).is_some_and(|g| g.button_released(button))
    }

    /// The axis value with [`gamepad_dead_zone`](Self::gamepad_dead_zone)
    /// applied (axially, then rescaled); 0 for a gamepad that is not connected.
    pub fn gamepad_axis(&self, id: GamepadId, axis: GamepadAxis) -> f32 {
        apply_dead_zone(self.gamepad_axis_raw(id, axis), self.gamepad_dead_zone)
    }

    /// The axis value as the device reports it; 0 for a gamepad that is not
    /// connected.
    pub fn gamepad_axis_raw(&self, id: GamepadId, axis: GamepadAxis) -> f32 {
        self.gamepad(id).map_or(0.0, |g| g.axis_raw(axis))
    }

    /// Counts calls to [`end_frame`](Self::end_frame). Two reads with the same
    /// value see the same per-frame state.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    // ----- called by the runner, or by the editor feeding a Play world -----

    pub fn press_key(&mut self, key: KeyCode) {
        if self.keys_down.insert(key) {
            self.keys_pressed.insert(key);
        }
    }

    pub fn release_key(&mut self, key: KeyCode) {
        if self.keys_down.remove(&key) {
            self.keys_released.insert(key);
        }
    }

    pub fn press_button(&mut self, button: MouseButton) {
        if self.buttons_down.insert(button) {
            self.buttons_pressed.insert(button);
        }
    }

    pub fn release_button(&mut self, button: MouseButton) {
        if self.buttons_down.remove(&button) {
            self.buttons_released.insert(button);
        }
    }

    /// Add raw pointer motion (device units) to this frame's
    /// [`mouse_delta`](Self::mouse_delta).
    pub fn add_mouse_motion(&mut self, delta: Vec2) {
        self.mouse_delta += delta;
    }

    /// Mark a gamepad connected (or update its name).
    pub fn connect_gamepad(&mut self, id: GamepadId, name: impl Into<String>) {
        self.gamepads.entry(id).or_default().name = name.into();
    }

    /// Remove a gamepad and its state. Actions held only through it read as
    /// released on their next evaluation.
    pub fn disconnect_gamepad(&mut self, id: GamepadId) {
        self.gamepads.remove(&id);
    }

    /// Press a gamepad button. An unknown id is connected first, so a feeder
    /// that skips connection events still works.
    pub fn press_gamepad_button(&mut self, id: GamepadId, button: GamepadButton) {
        self.gamepads.entry(id).or_default().press(button);
    }

    pub fn release_gamepad_button(&mut self, id: GamepadId, button: GamepadButton) {
        if let Some(g) = self.gamepads.get_mut(&id) {
            g.release(button);
        }
    }

    /// Set an axis, clamped to its range (-1..=1 for sticks, 0..=1 for
    /// triggers). An unknown id is connected first. NaN reads as 0.
    pub fn set_gamepad_axis(&mut self, id: GamepadId, axis: GamepadAxis, value: f32) {
        let value = if value.is_nan() { 0.0 } else { value };
        let min = if axis.is_trigger() { 0.0 } else { -1.0 };
        self.gamepads.entry(id).or_default().axes[axis.index()] = value.clamp(min, 1.0);
    }

    /// Release every key, mouse button and gamepad button and centre the
    /// gamepad axes, for example when the window loses focus. Gamepads stay
    /// connected.
    pub fn release_all(&mut self) {
        let keys: Vec<_> = self.keys_down.iter().copied().collect();
        for k in keys {
            self.release_key(k);
        }
        let buttons: Vec<_> = self.buttons_down.iter().copied().collect();
        for b in buttons {
            self.release_button(b);
        }
        for g in self.gamepads.values_mut() {
            g.release_all();
        }
    }

    /// Clear per-frame state (edges, mouse motion, scroll, pinch) and advance
    /// [`generation`](Self::generation). Called by the runner after each frame.
    pub fn end_frame(&mut self) {
        self.keys_pressed.clear();
        self.keys_released.clear();
        self.buttons_pressed.clear();
        self.buttons_released.clear();
        for g in self.gamepads.values_mut() {
            g.pressed.clear();
            g.released.clear();
        }
        self.mouse_delta = Vec2::ZERO;
        self.scroll_lines = Vec2::ZERO;
        self.scroll_pixels = Vec2::ZERO;
        self.pinch = 0.0;
        self.generation += 1;
    }
}

/// How the OS cursor is held. See [`CursorOptions`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorGrab {
    /// The cursor moves freely.
    #[default]
    None,
    /// The cursor cannot leave the window.
    Confined,
    /// The cursor stays in place; read motion from [`Input::mouse_delta`].
    /// Where the platform cannot lock (Windows), the runner confines the
    /// cursor and recentres it each frame instead.
    Locked,
}

/// The cursor a game asks for: a resource the runner reads every frame and
/// applies to the window. The runner releases the grab while the window is
/// unfocused and applies it again when focus returns. Headless runs ignore it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorOptions {
    pub grab: CursorGrab,
    pub visible: bool,
    /// What the runner actually applied: `None` while the window is unfocused,
    /// headless, or when the platform refused. Written by the runner.
    pub active_grab: CursorGrab,
}

impl Default for CursorOptions {
    fn default() -> Self {
        Self::free()
    }
}

impl CursorOptions {
    /// A visible cursor that moves freely.
    pub fn free() -> Self {
        Self {
            grab: CursorGrab::None,
            visible: true,
            active_grab: CursorGrab::None,
        }
    }

    /// A hidden, locked cursor for mouse look.
    pub fn locked() -> Self {
        Self {
            grab: CursorGrab::Locked,
            visible: false,
            active_grab: CursorGrab::None,
        }
    }

    /// Ask for a hidden, locked cursor (mouse look), keeping the
    /// runner-written [`active_grab`](Self::active_grab):
    /// `world.resource_mut::<CursorOptions>().lock()`.
    pub fn lock(&mut self) {
        self.grab = CursorGrab::Locked;
        self.visible = false;
    }

    /// Ask for a visible, free cursor, keeping
    /// [`active_grab`](Self::active_grab).
    pub fn release(&mut self) {
        self.grab = CursorGrab::None;
        self.visible = true;
    }

    /// True if the game asked for a locked cursor. Whether the runner managed
    /// to apply it (the window may be unfocused) is
    /// `active_grab == CursorGrab::Locked`.
    pub fn is_locked(&self) -> bool {
        self.grab == CursorGrab::Locked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressed_is_edge_triggered() {
        let mut i = Input::default();
        i.press_key(KeyCode::A);
        assert!(i.key_down(KeyCode::A) && i.key_pressed(KeyCode::A));
        i.end_frame();
        i.press_key(KeyCode::A); // key repeat
        assert!(i.key_down(KeyCode::A) && !i.key_pressed(KeyCode::A));
        i.release_key(KeyCode::A);
        assert!(!i.key_down(KeyCode::A) && i.key_released(KeyCode::A));
        i.end_frame();
        assert!(!i.key_released(KeyCode::A));
    }

    #[test]
    fn mouse_motion_accumulates_and_clears_at_end_of_frame() {
        let mut i = Input::default();
        i.add_mouse_motion(Vec2::new(3.0, -1.0));
        i.add_mouse_motion(Vec2::new(2.0, 4.0));
        assert_eq!(i.mouse_delta, Vec2::new(5.0, 3.0));
        i.scroll_lines = Vec2::new(0.0, 1.0);
        i.scroll_pixels = Vec2::new(2.0, 0.0);
        i.pinch = 0.5;
        let g = i.generation();
        i.end_frame();
        assert_eq!(i.mouse_delta, Vec2::ZERO);
        assert_eq!(i.scroll_lines, Vec2::ZERO);
        assert_eq!(i.scroll_pixels, Vec2::ZERO);
        assert_eq!(i.pinch, 0.0);
        assert_eq!(i.generation(), g + 1);
    }

    #[test]
    fn end_frame_resets_every_edge_but_keeps_held_state() {
        let pad = GamepadId(0);
        let mut i = Input::default();
        i.press_key(KeyCode::W);
        i.press_button(MouseButton::Left);
        i.press_gamepad_button(pad, GamepadButton::South);
        i.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.5);
        i.end_frame();
        assert!(!i.key_pressed(KeyCode::W) && i.key_down(KeyCode::W));
        assert!(!i.button_pressed(MouseButton::Left) && i.button_down(MouseButton::Left));
        assert!(!i.gamepad_button_pressed(pad, GamepadButton::South));
        assert!(i.gamepad_button_down(pad, GamepadButton::South));
        assert_eq!(i.gamepad_axis_raw(pad, GamepadAxis::LeftStickX), 0.5);
        i.release_gamepad_button(pad, GamepadButton::South);
        assert!(i.gamepad_button_released(pad, GamepadButton::South));
        i.end_frame();
        assert!(!i.gamepad_button_released(pad, GamepadButton::South));
    }

    #[test]
    fn gamepads_connect_disconnect_and_clamp() {
        let (a, b) = (GamepadId(1), GamepadId(7));
        let mut i = Input::default();
        i.connect_gamepad(a, "Pad A");
        i.press_gamepad_button(b, GamepadButton::Start); // connects b
        let ids: Vec<_> = i.gamepads().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![a, b]);
        assert_eq!(i.gamepad(a).unwrap().name(), "Pad A");
        i.set_gamepad_axis(a, GamepadAxis::LeftTrigger, -0.5);
        assert_eq!(i.gamepad_axis_raw(a, GamepadAxis::LeftTrigger), 0.0);
        i.set_gamepad_axis(a, GamepadAxis::RightStickY, 3.0);
        assert_eq!(i.gamepad_axis_raw(a, GamepadAxis::RightStickY), 1.0);
        i.set_gamepad_axis(a, GamepadAxis::RightStickX, f32::NAN);
        assert_eq!(i.gamepad_axis_raw(a, GamepadAxis::RightStickX), 0.0);
        i.disconnect_gamepad(b);
        assert!(i.gamepad(b).is_none());
        assert!(!i.gamepad_button_down(b, GamepadButton::Start));
        assert_eq!(i.gamepad_axis(b, GamepadAxis::LeftStickX), 0.0);
    }

    #[test]
    fn gamepad_axis_applies_the_dead_zone() {
        let pad = GamepadId(0);
        let mut i = Input::default();
        i.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.05);
        assert_eq!(i.gamepad_axis(pad, GamepadAxis::LeftStickX), 0.0);
        assert_eq!(i.gamepad_axis_raw(pad, GamepadAxis::LeftStickX), 0.05);
        i.set_gamepad_axis(pad, GamepadAxis::LeftStickX, -1.0);
        assert_eq!(i.gamepad_axis(pad, GamepadAxis::LeftStickX), -1.0);
        i.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.55);
        assert!((i.gamepad_axis(pad, GamepadAxis::LeftStickX) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn radial_dead_zone_keeps_direction() {
        assert_eq!(apply_radial_dead_zone(Vec2::new(0.1, 0.1), 0.2), Vec2::ZERO);
        let v = apply_radial_dead_zone(Vec2::new(0.0, 0.6), 0.2);
        assert!((v - Vec2::new(0.0, 0.5)).length() < 1e-6, "{v}");
        let v = apply_radial_dead_zone(Vec2::new(1.0, 1.0), 0.2);
        assert!((v.length() - 1.0).abs() < 1e-6 && (v.x - v.y).abs() < 1e-6);
    }

    #[test]
    fn release_all_releases_gamepads_too() {
        let pad = GamepadId(0);
        let mut i = Input::default();
        i.press_key(KeyCode::Space);
        i.press_gamepad_button(pad, GamepadButton::East);
        i.set_gamepad_axis(pad, GamepadAxis::LeftStickY, 1.0);
        i.end_frame();
        i.release_all();
        assert!(i.key_released(KeyCode::Space));
        assert!(i.gamepad_button_released(pad, GamepadButton::East));
        assert_eq!(i.gamepad_axis_raw(pad, GamepadAxis::LeftStickY), 0.0);
        assert!(i.gamepad(pad).is_some(), "still connected");
    }
}
