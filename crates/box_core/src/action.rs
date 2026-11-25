//! Named input actions: bindings from keys, mouse buttons and gamepads to
//! game verbs such as "jump" or "move".
//!
//! An [`ActionMap`] resource holds the bindings (serializable, so a project can
//! ship them as JSON). [`ActionsPlugin`] evaluates them from [`Input`] once per
//! frame into the [`Actions`] resource, which games query by name:
//!
//! ```
//! use box_core::prelude::*;
//!
//! let mut app = App::new();
//! app.add_plugin(&ActionsPlugin);
//! app.world.insert_resource(
//!     ActionMap::new()
//!         .button("jump", [Binding::Key(KeyCode::Space), Binding::Gamepad(GamepadButton::South)])
//!         .axis2("move", [Axis2Binding::wasd(), Axis2Binding::arrows(), Axis2Binding::Stick(GamepadStick::Left)]),
//! );
//! app.add_system(Stage::Update, |w| {
//!     let actions = w.resource::<Actions>();
//!     let _jump = actions.pressed("jump");
//!     let _move: Vec2 = actions.axis2("move"); // +X right, +Y forward/up
//! });
//! app.world.resource_mut::<Input>().press_key(KeyCode::W);
//! app.tick(1.0 / 60.0);
//! assert_eq!(app.world.resource::<Actions>().axis2("move"), Vec2::Y);
//! ```
//!
//! # Per-frame and fixed-step semantics
//!
//! Every action has a `held` state, `pressed`/`released` edges and a value.
//! What `pressed` and `released` mean depends on where they are read:
//!
//! - In `Update` and `PostUpdate`: true for exactly one frame, the frame whose
//!   input produced the edge.
//! - In `FixedUpdate`: true in exactly one fixed step, the first step that runs
//!   after the edge. A press in a frame that runs no fixed step is kept for
//!   the next step; a frame that runs two steps reports it only in the first.
//!   Several presses between two fixed steps count as one.
//!
//! `held` and axis values are the current values in every stage. A press and
//! release within one frame still reports `pressed` (and `released`) once.
//! The plugin's systems must run before game systems that read actions in
//! the same stage; `box_engine::app_with_world` adds it first.
//!
//! Unknown action names read as not held and zero; they are collected in
//! [`Actions::undefined`] so a typo shows up in the log once.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Mutex;

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::app::{App, Plugin, Stage};
use crate::input::{
    GamepadAxis, GamepadButton, Input, KeyCode, MouseButton, apply_dead_zone,
    apply_radial_dead_zone,
};
use crate::world::World;

/// Default dead zone of axis actions on gamepad axes and sticks.
pub const DEFAULT_ACTION_DEAD_ZONE: f32 = 0.15;

fn default_dead_zone() -> f32 {
    DEFAULT_ACTION_DEAD_ZONE
}

/// A digital input. Gamepad bindings match any connected gamepad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    Gamepad(GamepadButton),
}

impl From<KeyCode> for Binding {
    fn from(k: KeyCode) -> Self {
        Binding::Key(k)
    }
}

impl From<MouseButton> for Binding {
    fn from(b: MouseButton) -> Self {
        Binding::Mouse(b)
    }
}

impl From<GamepadButton> for Binding {
    fn from(b: GamepadButton) -> Self {
        Binding::Gamepad(b)
    }
}

impl Binding {
    fn down(self, input: &Input) -> bool {
        match self {
            Binding::Key(k) => input.key_down(k),
            Binding::Mouse(b) => input.button_down(b),
            Binding::Gamepad(b) => input.gamepads().any(|(_, g)| g.button_down(b)),
        }
    }

    fn pressed(self, input: &Input) -> bool {
        match self {
            Binding::Key(k) => input.key_pressed(k),
            Binding::Mouse(b) => input.button_pressed(b),
            Binding::Gamepad(b) => input.gamepads().any(|(_, g)| g.button_pressed(b)),
        }
    }

    fn value(self, input: &Input) -> f32 {
        if self.down(input) { 1.0 } else { 0.0 }
    }
}

/// The raw value of `axis` on the connected gamepad that deflects it most.
fn strongest_axis(input: &Input, axis: GamepadAxis) -> f32 {
    input
        .gamepads()
        .map(|(_, g)| g.axis_raw(axis))
        .fold(0.0, |best, v| if v.abs() > best.abs() { v } else { best })
}

/// A source for a one-dimensional axis action, in -1..=1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis1Binding {
    /// Two digital inputs: `positive` gives +1, `negative` gives -1.
    Buttons {
        negative: Binding,
        positive: Binding,
    },
    /// A gamepad stick axis (-1..=1) or trigger (0..=1), with the action's
    /// dead zone. `invert` flips the sign.
    Gamepad {
        axis: GamepadAxis,
        #[serde(default)]
        invert: bool,
    },
}

impl Axis1Binding {
    /// Two digital inputs, `negative` first.
    pub fn buttons(negative: impl Into<Binding>, positive: impl Into<Binding>) -> Self {
        Axis1Binding::Buttons {
            negative: negative.into(),
            positive: positive.into(),
        }
    }

    /// A gamepad axis, not inverted.
    pub fn gamepad(axis: GamepadAxis) -> Self {
        Axis1Binding::Gamepad {
            axis,
            invert: false,
        }
    }

    fn value(self, input: &Input, dead_zone: f32) -> f32 {
        match self {
            Axis1Binding::Buttons { negative, positive } => {
                positive.value(input) - negative.value(input)
            }
            Axis1Binding::Gamepad { axis, invert } => {
                let v = apply_dead_zone(strongest_axis(input, axis), dead_zone);
                if invert { -v } else { v }
            }
        }
    }
}

/// One of the two gamepad sticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GamepadStick {
    Left,
    Right,
}

impl GamepadStick {
    fn axes(self) -> (GamepadAxis, GamepadAxis) {
        match self {
            GamepadStick::Left => (GamepadAxis::LeftStickX, GamepadAxis::LeftStickY),
            GamepadStick::Right => (GamepadAxis::RightStickX, GamepadAxis::RightStickY),
        }
    }
}

/// A source for a two-dimensional axis action: +X right, +Y up (forward).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis2Binding {
    /// Four digital inputs. Diagonals are normalized to length 1, so moving
    /// diagonally is not faster.
    Buttons {
        up: Binding,
        down: Binding,
        left: Binding,
        right: Binding,
    },
    /// A gamepad stick with the action's radial dead zone, rescaled so the
    /// output still reaches length 1.
    Stick(GamepadStick),
}

impl Axis2Binding {
    /// W, S, A, D.
    pub fn wasd() -> Self {
        Self::keys(KeyCode::W, KeyCode::S, KeyCode::A, KeyCode::D)
    }

    /// The arrow keys.
    pub fn arrows() -> Self {
        Self::keys(KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right)
    }

    /// The gamepad d-pad.
    pub fn dpad() -> Self {
        Axis2Binding::Buttons {
            up: GamepadButton::DPadUp.into(),
            down: GamepadButton::DPadDown.into(),
            left: GamepadButton::DPadLeft.into(),
            right: GamepadButton::DPadRight.into(),
        }
    }

    /// Four keys: up, down, left, right.
    pub fn keys(up: KeyCode, down: KeyCode, left: KeyCode, right: KeyCode) -> Self {
        Axis2Binding::Buttons {
            up: up.into(),
            down: down.into(),
            left: left.into(),
            right: right.into(),
        }
    }

    fn value(self, input: &Input, dead_zone: f32) -> Vec2 {
        match self {
            Axis2Binding::Buttons {
                up,
                down,
                left,
                right,
            } => Vec2::new(
                right.value(input) - left.value(input),
                up.value(input) - down.value(input),
            )
            .normalize_or_zero(),
            Axis2Binding::Stick(stick) => {
                let (x, y) = stick.axes();
                // The pad whose stick is deflected furthest.
                let raw = input
                    .gamepads()
                    .map(|(_, g)| Vec2::new(g.axis_raw(x), g.axis_raw(y)))
                    .fold(Vec2::ZERO, |best, v| {
                        if v.length_squared() > best.length_squared() {
                            v
                        } else {
                            best
                        }
                    });
                apply_radial_dead_zone(raw, dead_zone)
            }
        }
    }
}

/// What an action is bound to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionKind {
    /// Held while any binding is down. Reads as 1.0 on an axis query while
    /// held.
    Button { bindings: Vec<Binding> },
    /// The sum of the bindings, clamped to -1..=1. Held while non-zero.
    Axis1 {
        bindings: Vec<Axis1Binding>,
        /// Applied to gamepad axes (0..1).
        #[serde(default = "default_dead_zone")]
        dead_zone: f32,
    },
    /// The sum of the bindings, clamped to length 1. Held while non-zero.
    Axis2 {
        bindings: Vec<Axis2Binding>,
        /// Applied radially to gamepad sticks (0..1).
        #[serde(default = "default_dead_zone")]
        dead_zone: f32,
    },
}

impl ActionKind {
    /// True if any binding is a mouse button.
    pub fn uses_mouse(&self) -> bool {
        let mouse = |b: &Binding| matches!(b, Binding::Mouse(_));
        match self {
            ActionKind::Button { bindings } => bindings.iter().any(mouse),
            ActionKind::Axis1 { bindings, .. } => bindings.iter().any(|b| match b {
                Axis1Binding::Buttons { negative, positive } => mouse(negative) || mouse(positive),
                Axis1Binding::Gamepad { .. } => false,
            }),
            ActionKind::Axis2 { bindings, .. } => bindings.iter().any(|b| match b {
                Axis2Binding::Buttons {
                    up,
                    down,
                    left,
                    right,
                } => [up, down, left, right].into_iter().any(mouse),
                Axis2Binding::Stick(_) => false,
            }),
        }
    }

    /// (held, pressed edge from a binding this frame, value).
    fn sample(&self, input: &Input) -> (bool, bool, Vec2) {
        match self {
            ActionKind::Button { bindings } => {
                let held = bindings.iter().any(|b| b.down(input));
                let tapped = bindings.iter().any(|b| b.pressed(input));
                (held, tapped, Vec2::new(if held { 1.0 } else { 0.0 }, 0.0))
            }
            ActionKind::Axis1 {
                bindings,
                dead_zone,
            } => {
                let v: f32 = bindings.iter().map(|b| b.value(input, *dead_zone)).sum();
                let v = v.clamp(-1.0, 1.0);
                (v != 0.0, false, Vec2::new(v, 0.0))
            }
            ActionKind::Axis2 {
                bindings,
                dead_zone,
            } => {
                let v: Vec2 = bindings.iter().map(|b| b.value(input, *dead_zone)).sum();
                let v = v.clamp_length_max(1.0);
                (v != Vec2::ZERO, false, v)
            }
        }
    }
}

/// Named actions and their bindings: a resource. Serializes as
/// `{"actions": {"jump": {"kind": "button", "bindings": [{"key": "Space"}]}}}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ActionMap {
    actions: BTreeMap<String, ActionKind>,
}

impl ActionMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a button action.
    pub fn button(mut self, name: &str, bindings: impl IntoIterator<Item = Binding>) -> Self {
        self.insert(
            name,
            ActionKind::Button {
                bindings: bindings.into_iter().collect(),
            },
        );
        self
    }

    /// Add (or replace) a 1D axis action with the default dead zone.
    pub fn axis1(mut self, name: &str, bindings: impl IntoIterator<Item = Axis1Binding>) -> Self {
        self.insert(
            name,
            ActionKind::Axis1 {
                bindings: bindings.into_iter().collect(),
                dead_zone: DEFAULT_ACTION_DEAD_ZONE,
            },
        );
        self
    }

    /// Add (or replace) a 2D axis action with the default dead zone.
    pub fn axis2(mut self, name: &str, bindings: impl IntoIterator<Item = Axis2Binding>) -> Self {
        self.insert(
            name,
            ActionKind::Axis2 {
                bindings: bindings.into_iter().collect(),
                dead_zone: DEFAULT_ACTION_DEAD_ZONE,
            },
        );
        self
    }

    /// Set the dead zone of an axis action (clamped to 0..=0.99). Button
    /// actions and unknown names are left alone.
    pub fn with_dead_zone(mut self, name: &str, dead_zone: f32) -> Self {
        let dz = dead_zone.clamp(0.0, 0.99);
        match self.actions.get_mut(name) {
            Some(ActionKind::Axis1 { dead_zone, .. } | ActionKind::Axis2 { dead_zone, .. }) => {
                *dead_zone = dz
            }
            Some(ActionKind::Button { .. }) | None => {}
        }
        self
    }

    pub fn insert(&mut self, name: &str, kind: ActionKind) {
        self.actions.insert(name.to_string(), kind);
    }

    pub fn remove(&mut self, name: &str) -> Option<ActionKind> {
        self.actions.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&ActionKind> {
        self.actions.get(name)
    }

    /// Action names in alphabetical order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.actions.keys().map(String::as_str)
    }

    /// Adds every action of `defaults` that this map does not define;
    /// actions already here keep their bindings. Games add their code
    /// defaults this way, so a project's `assets/settings/input.actions.json`
    /// (loaded by `box_engine::settings`) overrides bindings per action and
    /// actions it leaves out keep the game's defaults:
    ///
    /// ```
    /// use box_core::prelude::*;
    ///
    /// let mut map = ActionMap::from_json(
    ///     r#"{"actions": {"jump": {"kind": "button", "bindings": [{"key": "J"}]}}}"#,
    /// ).unwrap();
    /// map.merge_missing(
    ///     &ActionMap::new()
    ///         .button("jump", [Binding::Key(KeyCode::Space)])
    ///         .axis2("move", [Axis2Binding::wasd()]),
    /// );
    /// assert_eq!(map.get("jump"), Some(&ActionKind::Button { bindings: vec![Binding::Key(KeyCode::J)] }));
    /// assert!(map.get("move").is_some());
    /// ```
    pub fn merge_missing(&mut self, defaults: &ActionMap) {
        for (name, kind) in &defaults.actions {
            self.actions
                .entry(name.clone())
                .or_insert_with(|| kind.clone());
        }
    }

    /// Number of actions.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// True when no action is defined.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("an action map always serializes")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActionState {
    held: bool,
    pressed: bool,
    released: bool,
    /// Edges not yet seen by a fixed step.
    pending_pressed: bool,
    pending_released: bool,
    /// Edges for the current fixed step.
    fixed_pressed: bool,
    fixed_released: bool,
    value: Vec2,
}

#[derive(Debug, Default)]
struct UndefinedNames {
    all: BTreeSet<String>,
    unreported: Vec<String>,
}

/// Evaluated action state: a resource written by [`ActionsPlugin`]. See the
/// [module docs](self) for what `pressed` means in each stage.
#[derive(Debug, Default)]
pub struct Actions {
    states: HashMap<String, ActionState>,
    evaluated_generation: Option<u64>,
    in_fixed_step: bool,
    undefined: Mutex<UndefinedNames>,
    /// While set, every action outside the set reads idle (see [`capture`](Self::capture)).
    captured: Option<HashSet<String>>,
    /// While set, actions with a mouse button binding read idle.
    mouse_captured: bool,
    /// Actions bound to a mouse button, as of the last evaluation.
    mouse_actions: HashSet<String>,
    /// Input injected into actions ([`inject`](Self::inject)).
    injected: HashMap<String, InjectedInput>,
    /// Actions that read injected input at the last evaluation.
    injected_now: HashSet<String>,
}

/// Input injected into one action for a number of input frames, on top of
/// its bindings: how agents (`send_input`) and tests drive a game without
/// knowing its keys. A button is held; an axis reads `value` (x for a 1D
/// axis; for a 2D axis +Y is forward).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InjectedInput {
    /// Held for buttons; axes are held while `value` is not zero.
    pub held: bool,
    pub value: Vec2,
    /// Input frames (evaluations) left; at least 1.
    pub frames: u32,
}

impl InjectedInput {
    /// A button held for `frames` frames (1: a tap).
    pub fn press(frames: u32) -> Self {
        Self {
            held: true,
            value: Vec2::X,
            frames: frames.max(1),
        }
    }

    /// An axis at `value` for `frames` frames.
    pub fn axis(value: Vec2, frames: u32) -> Self {
        Self {
            held: value != Vec2::ZERO,
            value,
            frames: frames.max(1),
        }
    }
}

/// What a masked action reads as.
const IDLE: ActionState = ActionState {
    held: false,
    pressed: false,
    released: false,
    pending_pressed: false,
    pending_released: false,
    fixed_pressed: false,
    fixed_released: false,
    value: Vec2::ZERO,
};

impl Actions {
    fn state(&self, name: &str) -> Option<&ActionState> {
        let masked = self
            .captured
            .as_ref()
            .is_some_and(|allow| !allow.contains(name))
            || (self.mouse_captured
                && self.mouse_actions.contains(name)
                && !self.injected_now.contains(name));
        let s = self.states.get(name);
        if masked && s.is_some() {
            return Some(&IDLE);
        }
        if s.is_none() {
            let mut u = self.undefined.lock().unwrap_or_else(|e| e.into_inner());
            if u.all.insert(name.to_string()) {
                u.unreported.push(name.to_string());
            }
        }
        s
    }

    /// True on the edge where the action became held (see the module docs for
    /// fixed-step semantics).
    pub fn pressed(&self, name: &str) -> bool {
        let fixed = self.in_fixed_step;
        self.state(name)
            .is_some_and(|s| if fixed { s.fixed_pressed } else { s.pressed })
    }

    /// True on the edge where the action stopped being held.
    pub fn released(&self, name: &str) -> bool {
        let fixed = self.in_fixed_step;
        self.state(name)
            .is_some_and(|s| if fixed { s.fixed_released } else { s.released })
    }

    /// True while any binding is active (an axis action: while non-zero).
    pub fn held(&self, name: &str) -> bool {
        self.state(name).is_some_and(|s| s.held)
    }

    /// A 1D axis in -1..=1; a button action reads 1.0 while held.
    pub fn axis1(&self, name: &str) -> f32 {
        self.state(name).map_or(0.0, |s| s.value.x)
    }

    /// A 2D axis with length at most 1: +X right, +Y up (forward). A 1D axis
    /// or button action reads on X.
    pub fn axis2(&self, name: &str) -> Vec2 {
        self.state(name).map_or(Vec2::ZERO, |s| s.value)
    }

    /// True if the action map defines `name` (as of the last evaluation).
    pub fn is_defined(&self, name: &str) -> bool {
        self.states.contains_key(name)
    }

    /// Every name queried that the action map does not define, sorted.
    pub fn undefined(&self) -> Vec<String> {
        let u = self.undefined.lock().unwrap_or_else(|e| e.into_inner());
        u.all.iter().cloned().collect()
    }

    /// Undefined names not returned by an earlier call, for logging each once.
    pub fn take_unreported_undefined(&self) -> Vec<String> {
        let mut u = self.undefined.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut u.unreported)
    }

    /// Captures input for a modal owner (an in-game menu, `box_ui`): until
    /// [`release_capture`](Self::release_capture), every action except those
    /// in `allow` reads as idle (not held, no edges, zero), in every stage.
    /// Evaluation continues underneath, so held state stays correct.
    pub fn capture<'a>(&mut self, allow: impl IntoIterator<Item = &'a str>) {
        self.captured = Some(allow.into_iter().map(str::to_string).collect());
    }

    /// Ends a [`capture`](Self::capture). Edges that happened while captured
    /// are dropped, including those still waiting for a fixed step, so the
    /// press that closed a menu never reaches gameplay.
    pub fn release_capture(&mut self) {
        if self.captured.take().is_some() {
            self.drop_edges(|_| true);
        }
    }

    /// True while [`capture`](Self::capture) is in effect.
    pub fn is_captured(&self) -> bool {
        self.captured.is_some()
    }

    /// Masks actions bound to a mouse button (the pointer is over an
    /// interactive UI element). Turning it off drops their pending edges.
    pub fn set_mouse_captured(&mut self, captured: bool) {
        if self.mouse_captured && !captured {
            let mouse = std::mem::take(&mut self.mouse_actions);
            self.drop_edges(|n| mouse.contains(n));
            self.mouse_actions = mouse;
        }
        self.mouse_captured = captured;
    }

    /// True while mouse-bound actions are masked.
    pub fn is_mouse_captured(&self) -> bool {
        self.mouse_captured
    }

    fn drop_edges(&mut self, which: impl Fn(&str) -> bool) {
        for (name, s) in &mut self.states {
            if which(name) {
                s.pressed = false;
                s.released = false;
                s.pending_pressed = false;
                s.pending_released = false;
                s.fixed_pressed = false;
                s.fixed_released = false;
            }
        }
    }

    /// Inject input into action `name` for its next `input.frames` input
    /// frames, replacing what was injected into it before. The action must
    /// exist in the `ActionMap` when it is evaluated, or the injection is
    /// dropped. Injected input reaches the game like bound input, edges
    /// included: a UI [`capture`](Self::capture) masks it, the mouse-capture
    /// mask (clicks that belong to the editor) does not.
    pub fn inject(&mut self, name: &str, input: InjectedInput) {
        self.injected.insert(name.to_string(), input);
    }

    /// Drop injected input: every action reads only its bindings again
    /// from the next evaluation.
    pub fn clear_injected(&mut self) {
        self.injected.clear();
    }

    /// Input frames of injected input left per action.
    pub fn injected(&self) -> impl Iterator<Item = (&str, &InjectedInput)> {
        self.injected.iter().map(|(n, i)| (n.as_str(), i))
    }

    /// Sample every action from `input`, computing edges against the previous
    /// evaluation. [`ActionsPlugin`] calls this once per input frame.
    pub fn evaluate(&mut self, map: &ActionMap, input: &Input) {
        self.states.retain(|name, _| map.actions.contains_key(name));
        self.injected
            .retain(|name, _| map.actions.contains_key(name));
        self.injected_now.clear();
        self.mouse_actions
            .retain(|name| map.actions.contains_key(name));
        for (name, kind) in &map.actions {
            if kind.uses_mouse() && !self.mouse_actions.contains(name) {
                self.mouse_actions.insert(name.clone());
            }
            let (mut held, tapped, mut value) = kind.sample(input);
            if let Some(inj) = self.injected.get_mut(name) {
                self.injected_now.insert(name.clone());
                held |= inj.held;
                if inj.value != Vec2::ZERO {
                    value = match kind {
                        ActionKind::Button { .. } => Vec2::X,
                        _ => inj.value.clamp_length_max(1.0),
                    };
                }
                inj.frames -= 1;
                if inj.frames == 0 {
                    self.injected.remove(name);
                }
            }
            if !self.states.contains_key(name) {
                self.states.insert(name.clone(), ActionState::default());
            }
            let s = self.states.get_mut(name).expect("inserted above");
            let was = s.held;
            s.pressed = !was && (held || tapped);
            s.released = (was && !held) || (s.pressed && !held);
            s.held = held;
            s.value = value;
            s.pending_pressed |= s.pressed;
            s.pending_released |= s.released;
        }
        // Names defined now are no longer undefined.
        let mut u = self.undefined.lock().unwrap_or_else(|e| e.into_inner());
        if !u.all.is_empty() {
            u.all.retain(|n| !map.actions.contains_key(n));
            u.unreported.retain(|n| !map.actions.contains_key(n));
        }
        drop(u);
        self.evaluated_generation = Some(input.generation());
    }

    /// Start a fixed step: hand the edges gathered since the previous step to
    /// this one.
    pub fn begin_fixed_step(&mut self) {
        for s in self.states.values_mut() {
            s.fixed_pressed = std::mem::take(&mut s.pending_pressed);
            s.fixed_released = std::mem::take(&mut s.pending_released);
        }
        self.in_fixed_step = true;
    }

    /// Leave fixed-step mode: queries report per-frame edges again.
    pub fn end_fixed_steps(&mut self) {
        self.in_fixed_step = false;
    }
}

/// Evaluate actions unless this input frame was already evaluated.
fn evaluate_if_new(world: &mut World) {
    let (Some(input), Some(actions)) = (
        world.get_resource::<Input>(),
        world.get_resource::<Actions>(),
    ) else {
        return;
    };
    if actions.evaluated_generation == Some(input.generation()) {
        return;
    }
    let Some(mut actions) = world.remove_resource::<Actions>() else {
        return;
    };
    if let (Some(map), Some(input)) = (
        world.get_resource::<ActionMap>(),
        world.get_resource::<Input>(),
    ) {
        actions.evaluate(map, input);
    }
    world.insert_resource(actions);
}

/// First system of every fixed step.
pub fn actions_fixed_step_system(world: &mut World) {
    evaluate_if_new(world);
    if let Some(a) = world.get_resource_mut::<Actions>() {
        a.begin_fixed_step();
    }
}

/// First system of `Update`.
pub fn actions_update_system(world: &mut World) {
    evaluate_if_new(world);
    if let Some(a) = world.get_resource_mut::<Actions>() {
        a.end_fixed_steps();
    }
}

/// Adds [`ActionMap`] (if missing) and [`Actions`], and evaluates actions at
/// the start of each fixed step and of `Update`. Add it before plugins whose
/// systems read actions.
pub struct ActionsPlugin;

impl Plugin for ActionsPlugin {
    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<Input>();
        app.world.resource_or_default::<ActionMap>();
        app.world.resource_or_default::<Actions>();
        app.add_system(Stage::FixedUpdate, actions_fixed_step_system);
        app.add_system(Stage::Update, actions_update_system);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::GamepadId;

    fn map() -> ActionMap {
        ActionMap::new()
            .button(
                "jump",
                [
                    Binding::Key(KeyCode::Space),
                    Binding::Gamepad(GamepadButton::South),
                    Binding::Mouse(MouseButton::Left),
                ],
            )
            .axis1(
                "throttle",
                [
                    Axis1Binding::buttons(KeyCode::S, KeyCode::W),
                    Axis1Binding::gamepad(GamepadAxis::RightTrigger),
                ],
            )
            .axis2(
                "move",
                [
                    Axis2Binding::wasd(),
                    Axis2Binding::arrows(),
                    Axis2Binding::Stick(GamepadStick::Left),
                ],
            )
    }

    const PAD: GamepadId = GamepadId(3);

    #[test]
    fn injected_input_presses_holds_and_releases() {
        let (m, mut a, i) = (map(), Actions::default(), Input::default());
        a.inject("jump", InjectedInput::press(2));
        a.inject("move", InjectedInput::axis(Vec2::new(0.0, 2.0), 3));
        a.inject("nope", InjectedInput::press(1));
        a.evaluate(&m, &i);
        assert!(a.pressed("jump") && a.held("jump"));
        assert_eq!(a.axis2("move"), Vec2::Y, "clamped to length 1");
        assert_eq!(a.injected().count(), 2, "unknown names are dropped");
        a.evaluate(&m, &i);
        assert!(!a.pressed("jump") && a.held("jump"));
        a.evaluate(&m, &i);
        assert!(a.released("jump") && !a.held("jump"));
        assert_eq!(a.axis2("move"), Vec2::Y);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::ZERO);
        assert_eq!(a.injected().count(), 0);
        // The mouse mask leaves injected input alone; a UI capture does not.
        a.set_mouse_captured(true);
        a.inject("jump", InjectedInput::press(1));
        a.evaluate(&m, &i);
        assert!(a.pressed("jump"));
        a.capture(["ui_accept"]);
        a.inject("jump", InjectedInput::press(1));
        a.evaluate(&m, &i);
        assert!(!a.held("jump"));
    }

    #[test]
    fn wasd_composite_is_normalized() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.press_key(KeyCode::W);
        i.press_key(KeyCode::D);
        a.evaluate(&m, &i);
        let v = a.axis2("move");
        assert!((v.length() - 1.0).abs() < 1e-6, "{v}");
        assert!((v.x - v.y).abs() < 1e-6 && v.x > 0.0);
        // Opposite keys cancel.
        i.press_key(KeyCode::S);
        i.press_key(KeyCode::A);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::ZERO);
        assert!(!a.held("move"));
    }

    #[test]
    fn several_bindings_sum_and_clamp() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        // W and Up together still give length 1.
        i.press_key(KeyCode::W);
        i.press_key(KeyCode::Up);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::Y);
        // Keyboard plus a stick pushing the other way.
        i.release_key(KeyCode::Up);
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickY, -1.0);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::ZERO);
        // 1D: W (+1) plus a full trigger (+1) clamps to 1.
        i.set_gamepad_axis(PAD, GamepadAxis::RightTrigger, 1.0);
        a.evaluate(&m, &i);
        assert_eq!(a.axis1("throttle"), 1.0);
        i.release_key(KeyCode::W);
        i.press_key(KeyCode::S);
        a.evaluate(&m, &i);
        assert_eq!(a.axis1("throttle"), 0.0);
    }

    #[test]
    fn stick_dead_zone_is_radial_and_rescaled() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 0.1);
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 0.1);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::ZERO, "inside the 0.15 dead zone");
        assert!(!a.held("move"));
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 0.0);
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 1.0);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::Y);
        // A custom dead zone: half deflection past a 0.5 zone is zero.
        let m = map().with_dead_zone("move", 0.5);
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 0.5);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), Vec2::ZERO);
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickY, 0.75);
        a.evaluate(&m, &i);
        assert!((a.axis2("move").y - 0.5).abs() < 1e-6);
    }

    #[test]
    fn trigger_axis_has_a_dead_zone() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.set_gamepad_axis(PAD, GamepadAxis::RightTrigger, 0.1);
        a.evaluate(&m, &i);
        assert_eq!(a.axis1("throttle"), 0.0);
        i.set_gamepad_axis(PAD, GamepadAxis::RightTrigger, 1.0);
        a.evaluate(&m, &i);
        assert_eq!(a.axis1("throttle"), 1.0);
    }

    #[test]
    fn a_press_is_reported_once_across_bindings() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.press_key(KeyCode::Space);
        a.evaluate(&m, &i);
        assert!(a.pressed("jump") && a.held("jump") && !a.released("jump"));
        assert_eq!(a.axis1("jump"), 1.0);
        i.end_frame();
        a.evaluate(&m, &i);
        assert!(!a.pressed("jump") && a.held("jump"));
        // A second binding while the first is held: no new press.
        i.press_gamepad_button(PAD, GamepadButton::South);
        a.evaluate(&m, &i);
        assert!(!a.pressed("jump"));
        i.end_frame();
        // Releasing one of two held bindings: still held, no release.
        i.release_key(KeyCode::Space);
        a.evaluate(&m, &i);
        assert!(a.held("jump") && !a.released("jump"));
        i.end_frame();
        i.release_gamepad_button(PAD, GamepadButton::South);
        a.evaluate(&m, &i);
        assert!(!a.held("jump") && a.released("jump"));
        i.end_frame();
        a.evaluate(&m, &i);
        assert!(!a.released("jump"));
    }

    #[test]
    fn a_tap_within_one_frame_is_not_lost() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.press_button(MouseButton::Left);
        i.release_button(MouseButton::Left);
        a.evaluate(&m, &i);
        assert!(a.pressed("jump") && a.released("jump") && !a.held("jump"));
        i.end_frame();
        a.evaluate(&m, &i);
        assert!(!a.pressed("jump") && !a.released("jump"));
    }

    #[test]
    fn gamepad_disconnect_releases_its_actions() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.connect_gamepad(PAD, "pad");
        i.press_gamepad_button(PAD, GamepadButton::South);
        i.set_gamepad_axis(PAD, GamepadAxis::LeftStickX, 1.0);
        a.evaluate(&m, &i);
        assert!(a.held("jump") && a.axis2("move") == Vec2::X);
        i.end_frame();
        i.disconnect_gamepad(PAD);
        a.evaluate(&m, &i);
        assert!(a.released("jump") && !a.held("jump"));
        assert_eq!(a.axis2("move"), Vec2::ZERO);
        assert!(a.released("move"));
        // Reconnecting with the button still down presses again.
        i.end_frame();
        i.connect_gamepad(GamepadId(4), "pad 2");
        i.press_gamepad_button(GamepadId(4), GamepadButton::South);
        a.evaluate(&m, &i);
        assert!(a.pressed("jump"));
    }

    #[test]
    fn the_strongest_gamepad_wins() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.set_gamepad_axis(GamepadId(0), GamepadAxis::LeftStickX, 0.5);
        i.set_gamepad_axis(GamepadId(1), GamepadAxis::LeftStickX, -1.0);
        a.evaluate(&m, &i);
        assert_eq!(a.axis2("move"), -Vec2::X);
    }

    #[test]
    fn unknown_names_read_as_zero_and_are_listed_once() {
        let (m, mut a, mut i) = (map(), Actions::default(), Input::default());
        i.press_key(KeyCode::Space);
        a.evaluate(&m, &i);
        assert!(!a.pressed("jmup") && !a.held("jmup") && !a.released("jmup"));
        assert_eq!(a.axis1("nope"), 0.0);
        assert_eq!(a.axis2("nope"), Vec2::ZERO);
        assert!(a.is_defined("jump") && !a.is_defined("jmup"));
        assert_eq!(a.undefined(), vec!["jmup".to_string(), "nope".to_string()]);
        assert_eq!(a.take_unreported_undefined(), vec!["jmup", "nope"]);
        let _ = a.pressed("jmup");
        assert!(a.take_unreported_undefined().is_empty());
        // Defining the name later clears it.
        let m = m.button("jmup", [Binding::Key(KeyCode::J)]);
        a.evaluate(&m, &i);
        assert_eq!(a.undefined(), vec!["nope".to_string()]);
    }

    #[test]
    fn removed_actions_disappear() {
        let (mut m, mut a, i) = (map(), Actions::default(), Input::default());
        a.evaluate(&m, &i);
        assert!(a.is_defined("move"));
        m.remove("move");
        a.evaluate(&m, &i);
        assert!(!a.is_defined("move"));
        assert_eq!(m.names().collect::<Vec<_>>(), vec!["jump", "throttle"]);
    }

    #[test]
    fn action_maps_round_trip_through_json() {
        let m = map().with_dead_zone("throttle", 0.3);
        let json = m.to_json();
        assert_eq!(ActionMap::from_json(&json).unwrap(), m);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["actions"]["jump"]["kind"], "button");
        assert_eq!(v["actions"]["jump"]["bindings"][0]["key"], "Space");
        assert_eq!(v["actions"]["throttle"]["dead_zone"], 0.3);
        // Hand-written: dead_zone and invert default.
        let m = ActionMap::from_json(
            r#"{"actions": {
                "look_x": {"kind": "axis1", "bindings": [{"gamepad": {"axis": "RightStickX"}}]},
                "move": {"kind": "axis2", "bindings": [{"stick": "Left"},
                    {"buttons": {"up": {"key": "W"}, "down": {"key": "S"},
                                 "left": {"key": "A"}, "right": {"key": "D"}}}]}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            m.get("look_x"),
            Some(&ActionKind::Axis1 {
                bindings: vec![Axis1Binding::gamepad(GamepadAxis::RightStickX)],
                dead_zone: DEFAULT_ACTION_DEAD_ZONE,
            })
        );
        assert!(
            matches!(m.get("move"), Some(ActionKind::Axis2 { bindings, .. }) if bindings[1] == Axis2Binding::wasd())
        );
        assert!(ActionMap::from_json(r#"{"actions": {"x": {"kind": "wobble"}}}"#).is_err());
    }

    // ----- frame and fixed-step semantics, through an App -----

    #[derive(Default)]
    struct Seen {
        fixed: Vec<(bool, bool)>,
        update: Vec<bool>,
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugin(&ActionsPlugin);
        app.world.insert_resource(map());
        app.world.insert_resource(Seen::default());
        app.add_system(Stage::FixedUpdate, |w| {
            let a = w.resource::<Actions>();
            let seen = (a.pressed("jump"), a.released("jump"));
            w.resource_mut::<Seen>().fixed.push(seen);
        });
        app.add_system(Stage::Update, |w| {
            let p = w.resource::<Actions>().pressed("jump");
            w.resource_mut::<Seen>().update.push(p);
        });
        app
    }

    /// One runner frame: tick, then end the input frame.
    fn frame(app: &mut App, dt: f32) {
        app.tick(dt);
        app.world.resource_mut::<Input>().end_frame();
    }

    fn take(app: &mut App) -> Seen {
        std::mem::take(app.world.resource_mut::<Seen>())
    }

    #[test]
    fn a_capture_masks_actions_and_drops_the_closing_press() {
        let mut app = app();
        app.world.resource_mut::<ActionMap>().insert(
            "ui_confirm",
            ActionKind::Button {
                bindings: vec![Binding::Key(KeyCode::Space)],
            },
        );
        frame(&mut app, 1.0 / 60.0);
        take(&mut app);
        app.world.resource_mut::<Actions>().capture(["ui_confirm"]);
        // Space confirms in the menu; gameplay's "jump" (also Space) is masked.
        app.world.resource_mut::<Input>().press_key(KeyCode::Space);
        app.add_system(Stage::Update, |w| {
            let a = w.resource::<Actions>();
            assert!(!a.is_captured() || !a.held("jump"));
            if a.pressed("ui_confirm") {
                // The menu closes on this press.
                w.resource_mut::<Actions>().release_capture();
            }
        });
        frame(&mut app, 1.0 / 60.0);
        frame(&mut app, 1.0 / 60.0);
        frame(&mut app, 1.0 / 60.0);
        let seen = take(&mut app);
        assert!(seen.update.iter().all(|p| !p), "{:?}", seen.update);
        assert!(seen.fixed.iter().all(|(p, _)| !p), "{:?}", seen.fixed);
        // Still held after the capture ends, without a new edge.
        assert!(app.world.resource::<Actions>().held("jump"));
    }

    #[test]
    fn mouse_capture_masks_only_mouse_bound_actions() {
        let mut a = Actions::default();
        let map = ActionMap::new()
            .button("fire", [Binding::Mouse(MouseButton::Left)])
            .button("jump", [Binding::Key(KeyCode::Space)]);
        let mut input = Input::default();
        input.press_button(MouseButton::Left);
        input.press_key(KeyCode::Space);
        a.evaluate(&map, &input);
        a.set_mouse_captured(true);
        assert!(!a.pressed("fire") && a.pressed("jump"));
        a.set_mouse_captured(false);
        assert!(!a.pressed("fire"), "the edge was dropped");
        assert!(a.held("fire"));
    }

    #[test]
    fn two_fixed_steps_in_a_frame_see_a_press_once() {
        let mut app = app();
        app.world.resource_mut::<Input>().press_key(KeyCode::Space);
        frame(&mut app, 1.0 / 30.0); // two fixed steps
        let seen = take(&mut app);
        assert_eq!(seen.fixed, vec![(true, false), (false, false)]);
        assert_eq!(seen.update, vec![true]);
        frame(&mut app, 1.0 / 30.0);
        let seen = take(&mut app);
        assert_eq!(seen.fixed, vec![(false, false), (false, false)]);
        assert_eq!(seen.update, vec![false]);
    }

    #[test]
    fn a_press_in_a_frame_without_fixed_steps_reaches_the_next_step() {
        let mut app = app();
        app.world.resource_mut::<Input>().press_key(KeyCode::Space);
        frame(&mut app, 0.005); // no fixed step yet
        let seen = take(&mut app);
        assert!(seen.fixed.is_empty());
        assert_eq!(seen.update, vec![true]);
        frame(&mut app, 0.005);
        frame(&mut app, 0.005);
        let seen = take(&mut app);
        assert!(seen.fixed.is_empty());
        assert_eq!(seen.update, vec![false, false], "Update saw it once");
        frame(&mut app, 0.005); // 0.02 s: one step
        let seen = take(&mut app);
        assert_eq!(seen.fixed, vec![(true, false)]);
        assert_eq!(seen.update, vec![false]);
    }

    #[test]
    fn a_tap_between_fixed_steps_reaches_the_next_step() {
        let mut app = app();
        {
            let input = app.world.resource_mut::<Input>();
            input.press_key(KeyCode::Space);
            input.release_key(KeyCode::Space);
        }
        frame(&mut app, 0.01); // no step
        frame(&mut app, 0.01); // one step
        let seen = take(&mut app);
        assert_eq!(seen.fixed, vec![(true, true)]);
        assert_eq!(seen.update, vec![true, false]);
    }

    #[test]
    fn update_without_new_input_does_not_repeat_edges_in_fixed_steps() {
        // `App::update` without `end_frame` keeps the same input frame: the
        // per-frame edge is still visible in Update, but fixed steps that
        // already consumed it do not see it again.
        let mut app = app();
        app.world.resource_mut::<Input>().press_key(KeyCode::Space);
        app.tick(1.0 / 60.0);
        app.tick(1.0 / 60.0);
        let seen = take(&mut app);
        assert_eq!(seen.fixed, vec![(true, false), (false, false)]);
        assert_eq!(seen.update, vec![true, true]);
    }
}
