//! Focus navigation, pointer interaction and input capture.
//!
//! **Actions.** The UI reads named actions, so bindings are remapped like
//! any other ([`default_actions`] are merged into the game's `ActionMap` for
//! names it does not define):
//!
//! | Action | Default bindings | Effect |
//! |---|---|---|
//! | `ui_navigate` (axis2) | arrow keys, D-pad, left stick | move focus (spatially); left/right adjust a focused slider |
//! | `ui_confirm` | Enter, Space, gamepad South | click a button, switch a toggle |
//! | `ui_back` | Escape, Backspace, gamepad East | `Back` event for the active modal root |
//! | `ui_next` / `ui_prev` | Tab / gamepad right, left shoulder | focus the next / previous node in document order |
//!
//! A held direction repeats after `repeat_delay`, every `repeat_interval`
//! (real time, so it works while the game is paused).
//!
//! **Focus.** Navigation works inside the active modal root (the topmost
//! visible root with `modal: true`). When one opens, focus goes to the node
//! it last had, else its `autofocus` node, else its first focusable node.
//! Directional moves pick the nearest focusable node in that direction
//! (centre distance, perpendicular offset weighted double, overlapping
//! nodes preferred) and wrap to the far side when `wrap_navigation` is on.
//!
//! **Capture.** While a modal root is active, every game action except the
//! `ui_*` ones and `UiSettings::passthrough_actions` reads idle
//! (`Actions::capture`), and the press that closes the menu never reaches
//! gameplay. Outside modal roots, actions bound to mouse buttons read idle
//! while the pointer is over (or pressing) an interactive node.

use box_core::action::{ActionMap, Actions, Axis2Binding, Binding, GamepadStick};
use box_core::input::{GamepadButton, Input, KeyCode, MouseButton};
use epaint::{Pos2, Vec2, vec2};

use crate::node::NodeKind;
use crate::ui::{NodeId, Ui, UiEventKind};

pub const UI_NAVIGATE: &str = "ui_navigate";
pub const UI_CONFIRM: &str = "ui_confirm";
pub const UI_BACK: &str = "ui_back";
pub const UI_NEXT: &str = "ui_next";
pub const UI_PREV: &str = "ui_prev";

/// The UI's own actions; they stay readable while the UI captures input.
pub const UI_ACTIONS: [&str; 5] = [UI_NAVIGATE, UI_CONFIRM, UI_BACK, UI_NEXT, UI_PREV];

/// The default bindings of the UI actions.
pub fn default_actions() -> ActionMap {
    ActionMap::new()
        .axis2(
            UI_NAVIGATE,
            [
                Axis2Binding::arrows(),
                Axis2Binding::dpad(),
                Axis2Binding::Stick(GamepadStick::Left),
            ],
        )
        .button(
            UI_CONFIRM,
            [
                Binding::Key(KeyCode::Enter),
                Binding::Key(KeyCode::Space),
                Binding::Gamepad(GamepadButton::South),
            ],
        )
        .button(
            UI_BACK,
            [
                Binding::Key(KeyCode::Escape),
                Binding::Key(KeyCode::Backspace),
                Binding::Gamepad(GamepadButton::East),
            ],
        )
        .button(
            UI_NEXT,
            [
                Binding::Key(KeyCode::Tab),
                Binding::Gamepad(GamepadButton::RightShoulder),
            ],
        )
        .button(UI_PREV, [Binding::Gamepad(GamepadButton::LeftShoulder)])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    fn vector(self) -> Vec2 {
        match self {
            Dir::Up => vec2(0.0, -1.0),
            Dir::Down => vec2(0.0, 1.0),
            Dir::Left => vec2(-1.0, 0.0),
            Dir::Right => vec2(1.0, 0.0),
        }
    }

    fn forward(self) -> bool {
        matches!(self, Dir::Down | Dir::Right)
    }
}

/// Navigation bookkeeping between frames.
#[derive(Clone, Debug, Default)]
pub(crate) struct NavState {
    held: Option<Dir>,
    timer: f32,
    last_cursor: Option<Vec2>,
    /// This UI set `Actions::capture`.
    captured: bool,
    /// This UI set `Actions::set_mouse_captured`.
    mouse_captured: bool,
}

impl NavState {
    pub(crate) fn is_capturing(&self) -> bool {
        self.captured || self.mouse_captured
    }
}

impl Ui {
    /// The node and every ancestor are visible and enabled, and a world
    /// anchor is on screen.
    fn is_live(&self, mut id: NodeId) -> bool {
        loop {
            let Some(n) = self.node(id) else { return false };
            if n.hidden || n.disabled || n.world.as_ref().is_some_and(|w| w.screen.is_none()) {
                return false;
            }
            match n.parent {
                Some(p) => id = p,
                None => return true,
            }
        }
    }

    fn focusable(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.kind.is_focusable()) && self.is_live(id)
    }

    /// Focusable nodes under `root`, in document order.
    fn focusables(&self, root: NodeId) -> Vec<NodeId> {
        self.preorder(root)
            .into_iter()
            .filter(|n| self.focusable(*n))
            .collect()
    }

    /// The topmost visible modal root.
    fn find_modal(&self) -> Option<NodeId> {
        self.draw_order()
            .into_iter()
            .rev()
            .find(|r| self.node(*r).is_some_and(|n| n.modal) && self.is_live(*r))
    }

    /// Where focus goes when `root` opens.
    fn initial_focus(&self, root: NodeId) -> Option<NodeId> {
        if let Some(r) = self.remembered.get(&root)
            && self.focusable(*r)
            && self.root_of(*r) == Some(root)
        {
            return Some(*r);
        }
        let all = self.focusables(root);
        all.iter()
            .copied()
            .find(|n| self.node(*n).is_some_and(|n| n.autofocus))
            .or_else(|| all.first().copied())
    }

    fn center(&self, id: NodeId) -> Option<Pos2> {
        let n = self.node(id)?;
        n.rect.is_positive().then(|| n.rect.center())
    }

    /// The node focus moves to from `from` in `dir`.
    fn neighbour(&self, from: NodeId, dir: Dir, candidates: &[NodeId]) -> Option<NodeId> {
        let ordered = |forward: bool| {
            let i = candidates.iter().position(|c| *c == from)?;
            let n = candidates.len();
            let j = if forward { i + 1 } else { i + n - 1 };
            if !self.settings.wrap_navigation && (j >= n && forward || !forward && i == 0) {
                return None;
            }
            Some(candidates[j % n])
        };
        let Some(c0) = self.center(from) else {
            return ordered(dir.forward());
        };
        let from_rect = self.node(from)?.rect;
        let d = dir.vector();
        let perp = vec2(d.y.abs(), d.x.abs());
        let overlaps = |id: NodeId| {
            let r = self.node(id).map(|n| n.rect);
            r.is_some_and(|r| {
                if perp.x > 0.0 {
                    r.min.x < from_rect.max.x && r.max.x > from_rect.min.x
                } else {
                    r.min.y < from_rect.max.y && r.max.y > from_rect.min.y
                }
            })
        };
        let mut best: Option<(f32, NodeId)> = None;
        for &c in candidates {
            if c == from {
                continue;
            }
            let Some(p) = self.center(c) else { continue };
            let v = p - c0;
            let along = v.dot(d);
            if along <= 1.0 {
                continue;
            }
            let across = if overlaps(c) { 0.0 } else { v.dot(perp).abs() };
            let score = along + 2.0 * across;
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, c));
            }
        }
        if let Some((_, c)) = best {
            return Some(c);
        }
        if !self.settings.wrap_navigation {
            return None;
        }
        // Wrap: the farthest node the other way, preferring the same
        // column (or row).
        let mut wrap: Option<(bool, f32, NodeId)> = None;
        for &c in candidates {
            if c == from {
                continue;
            }
            let Some(p) = self.center(c) else { continue };
            let back = -(p - c0).dot(d);
            if back <= 1.0 {
                continue;
            }
            let key = (overlaps(c), back);
            if wrap.is_none_or(|(o, b, _)| (key.0, key.1) > (o, b)) {
                wrap = Some((key.0, key.1, c));
            }
        }
        wrap.map(|(_, _, c)| c)
    }

    fn activate(&mut self, id: NodeId) {
        let Some(n) = self.node_mut(id) else { return };
        match n.kind {
            NodeKind::Button => self.push_event(id, UiEventKind::Click),
            NodeKind::Toggle => {
                n.checked = !n.checked;
                let checked = n.checked;
                self.dirty = true;
                self.push_event(id, UiEventKind::Toggle(checked));
            }
            _ => {}
        }
    }

    fn change_value(&mut self, id: NodeId, value: f32) {
        let Some(n) = self.node_mut(id) else { return };
        let v = if n.max > n.min {
            value.clamp(n.min, n.max)
        } else {
            value
        };
        if (v - n.value).abs() > f32::EPSILON {
            n.value = v;
            self.dirty = true;
            self.push_event(id, UiEventKind::Change(v));
        }
    }

    /// The topmost interactive node under `p` (points), within `scope`.
    fn hit(&self, p: Pos2, scope: Option<NodeId>) -> Option<NodeId> {
        let roots = match scope {
            Some(s) => vec![s],
            None => self.draw_order(),
        };
        for root in roots.into_iter().rev() {
            for id in self.preorder(root).into_iter().rev() {
                let Some(n) = self.node(id) else { continue };
                if n.shown
                    && n.kind.is_focusable()
                    && n.rect.contains(p)
                    && n.clip.contains(p)
                    && self.is_live(id)
                {
                    return Some(id);
                }
            }
        }
        None
    }

    /// The topmost shown list under `p`.
    fn list_at(&self, p: Pos2, scope: Option<NodeId>) -> Option<NodeId> {
        let roots = match scope {
            Some(s) => vec![s],
            None => self.draw_order(),
        };
        roots.into_iter().rev().find_map(|root| {
            self.preorder(root).into_iter().rev().find(|id| {
                self.node(*id)
                    .is_some_and(|n| n.shown && n.kind == NodeKind::List && n.rect.contains(p))
            })
        })
    }

    /// Slider value under the pointer at `x` (points).
    fn slider_value_at(&self, id: NodeId, x: f32) -> Option<f32> {
        let n = self.node(id)?;
        let r = &n.resolved;
        let inner_max = n.rect.max.x - r.padding.right;
        let start = inner_max - r.control_width;
        let t = ((x - start) / r.control_width.max(1.0)).clamp(0.0, 1.0);
        let mut v = n.min + t * (n.max - n.min);
        if n.step > 0.0 {
            v = n.min + ((v - n.min) / n.step).round() * n.step;
        }
        Some(v)
    }

    /// One frame of input. `dt` is real seconds.
    pub(crate) fn process_input(&mut self, input: &Input, actions: &mut Actions, dt: f32) {
        profiling::scope!("ui input");
        let modal = self.sync_capture(actions);
        self.navigate(modal, input, actions, dt);
    }

    /// The active modal root, with game input captured while there is one.
    /// Runs at the start of `Update` (with the rest of the input) and again
    /// in `PostUpdate`, so a modal shown or hidden by game code during the
    /// frame captures or frees input from the next frame's first fixed step
    /// on, not one fixed step later.
    pub(crate) fn sync_capture(&mut self, actions: &mut Actions) -> Option<NodeId> {
        let modal = self.find_modal();
        if modal != self.active_modal {
            self.active_modal = modal;
            self.nav.held = None;
            self.dirty = true;
        }
        match modal {
            Some(_) if !actions.is_captured() || !self.nav.captured => {
                let allow: Vec<&str> = UI_ACTIONS
                    .iter()
                    .copied()
                    .chain(self.settings.passthrough_actions.iter().map(String::as_str))
                    .collect();
                actions.capture(allow);
                self.nav.captured = true;
            }
            None if self.nav.captured => {
                actions.release_capture();
                self.nav.captured = false;
            }
            _ => {}
        }
        modal
    }

    /// Focus, navigation, pointer and activation for one frame.
    fn navigate(&mut self, modal: Option<NodeId>, input: &Input, actions: &mut Actions, dt: f32) {
        // Focus must be live and, with a modal root, inside it.
        let focus_ok = self
            .focused
            .is_some_and(|f| self.focusable(f) && modal.is_none_or(|m| self.root_of(f) == Some(m)));
        if !focus_ok {
            let next = modal.and_then(|m| self.initial_focus(m));
            self.set_focus(next);
        } else if let Some(m) = modal
            && self.focused.is_none()
        {
            let next = self.initial_focus(m);
            self.set_focus(next);
        }

        // Pointer.
        let cursor = input.cursor_position.map(|p| vec2(p.x, p.y) / self.scale);
        let moved = cursor != self.nav.last_cursor;
        self.nav.last_cursor = cursor;
        let hit = cursor.and_then(|c| self.hit(Pos2::ZERO + c, modal));
        if hit != self.hovered {
            self.hovered = hit;
            self.dirty = true;
        }
        if moved
            && let Some(h) = hit
            && self.focused != Some(h)
        {
            self.set_focus(Some(h));
        }
        if input.button_pressed(MouseButton::Left) && self.pressed != hit {
            self.pressed = hit;
            self.dirty = true;
        }
        if let (Some(p), Some(c)) = (self.pressed, cursor)
            && self.kind(p) == Some(NodeKind::Slider)
            && input.button_down(MouseButton::Left)
            && let Some(v) = self.slider_value_at(p, c.x)
        {
            self.change_value(p, v);
        }
        if input.button_released(MouseButton::Left) {
            if let Some(p) = self.pressed.take() {
                if hit == Some(p) {
                    self.activate(p);
                }
                self.dirty = true;
            }
        } else if !input.button_down(MouseButton::Left) && self.pressed.is_some() {
            self.pressed = None;
            self.dirty = true;
        }
        let wheel = input.scroll_lines.y * 40.0 + input.scroll_pixels.y;
        if wheel != 0.0
            && let Some(c) = cursor
            && let Some(list) = self.list_at(Pos2::ZERO + c, modal)
            && let Some(n) = self.node_mut(list)
        {
            n.scroll -= wheel;
            self.dirty = true;
        }
        let mouse = modal.is_some() || self.hovered.is_some() || self.pressed.is_some();
        if mouse != self.nav.mouse_captured {
            actions.set_mouse_captured(mouse);
            self.nav.mouse_captured = mouse;
        }

        // Keyboard and gamepad.
        let v = actions.axis2(UI_NAVIGATE);
        let dir = (v.length() >= self.settings.stick_threshold).then(|| {
            if v.x.abs() > v.y.abs() {
                if v.x > 0.0 { Dir::Right } else { Dir::Left }
            } else if v.y > 0.0 {
                Dir::Up
            } else {
                Dir::Down
            }
        });
        let fire = match (dir, self.nav.held) {
            (Some(d), Some(h)) if d == h => {
                self.nav.timer -= dt;
                if self.nav.timer <= 0.0 {
                    self.nav.timer += self.settings.repeat_interval.max(0.01);
                    Some(d)
                } else {
                    None
                }
            }
            (Some(d), _) => {
                self.nav.held = Some(d);
                self.nav.timer = self.settings.repeat_delay;
                Some(d)
            }
            (None, _) => {
                self.nav.held = None;
                None
            }
        };
        let scope = modal.or_else(|| self.focused.and_then(|f| self.root_of(f)));
        let Some(scope) = scope else {
            return;
        };
        if let Some(d) = fire {
            let focused = self.focused;
            match focused {
                Some(f)
                    if self.kind(f) == Some(NodeKind::Slider)
                        && matches!(d, Dir::Left | Dir::Right) =>
                {
                    if let Some(n) = self.node(f) {
                        let step = if n.step > 0.0 {
                            n.step
                        } else {
                            (n.max - n.min) / 10.0
                        };
                        let v = n.value + if d == Dir::Right { step } else { -step };
                        self.change_value(f, v);
                    }
                }
                Some(f) => {
                    let candidates = self.focusables(scope);
                    if let Some(next) = self.neighbour(f, d, &candidates) {
                        self.set_focus(Some(next));
                    }
                }
                None => {
                    let next = self.initial_focus(scope);
                    self.set_focus(next);
                }
            }
        }
        for (name, forward) in [(UI_NEXT, true), (UI_PREV, false)] {
            if actions.pressed(name) {
                let candidates = self.focusables(scope);
                let next = match self.focused {
                    Some(f) => self.neighbour_in_order(f, forward, &candidates),
                    None => candidates.first().copied(),
                };
                if next.is_some() {
                    self.set_focus(next);
                }
            }
        }
        if actions.pressed(UI_CONFIRM)
            && let Some(f) = self.focused
        {
            self.activate(f);
        }
        if actions.pressed(UI_BACK)
            && let Some(m) = modal
        {
            self.push_event(m, UiEventKind::Back);
        }
    }

    fn neighbour_in_order(&self, from: NodeId, forward: bool, c: &[NodeId]) -> Option<NodeId> {
        let i = c.iter().position(|x| *x == from)?;
        let n = c.len();
        Some(
            c[if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            }],
        )
    }
}
