//! Gamepads through `gilrs`, fed into [`box_core::input::Input`].

use box_core::input::{GamepadAxis, GamepadButton, GamepadId, Input};
use gilrs::{Axis, Button, EventType, Gilrs};

/// Drains gamepad events into an [`Input`] once per frame. The runner owns
/// one; the editor can own another for Play sessions.
///
/// If the platform backend cannot start, the poller logs a warning once and
/// reports no gamepads.
pub struct GamepadPoller {
    gilrs: Option<Gilrs>,
    focused: bool,
    /// Copy the full device state into `Input` on the next update: at start
    /// (gamepads already plugged in) and when focus returns.
    needs_sync: bool,
}

impl Default for GamepadPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl GamepadPoller {
    /// Start the platform backend.
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(gilrs::Error::NotImplemented(_)) => {
                tracing::warn!("gamepads are not supported on this platform");
                None
            }
            Err(e) => {
                tracing::warn!("gamepads unavailable: {e}");
                None
            }
        };
        Self {
            gilrs,
            focused: true,
            needs_sync: true,
        }
    }

    /// A poller without a backend: `update` does nothing. For headless runs.
    pub fn disabled() -> Self {
        Self {
            gilrs: None,
            focused: true,
            needs_sync: false,
        }
    }

    /// Whether a gamepad backend is running.
    pub fn is_available(&self) -> bool {
        self.gilrs.is_some()
    }

    /// While unfocused, button and axis events are dropped (connections are
    /// still tracked); when focus returns, the next update copies the current
    /// device state, so a stick held through the switch reads correctly.
    pub fn set_focused(&mut self, focused: bool) {
        if focused && !self.focused {
            self.needs_sync = true;
        }
        self.focused = focused;
    }

    /// Apply every pending gamepad event to `input`.
    pub fn update(&mut self, input: &mut Input) {
        let Some(gilrs) = &mut self.gilrs else {
            return;
        };
        while let Some(ev) = gilrs.next_event() {
            let id = pad_id(ev.id);
            match translate(&ev.event) {
                Some(PadEvent::Connected) => {
                    let name = gilrs.gamepad(ev.id).name().to_string();
                    tracing::info!("gamepad {} connected: {name}", id.0);
                    input.connect_gamepad(id, name);
                }
                Some(PadEvent::Disconnected) => {
                    tracing::info!("gamepad {} disconnected", id.0);
                    input.disconnect_gamepad(id);
                }
                Some(e) if self.focused => apply(input, id, e),
                _ => {}
            }
        }
        if self.needs_sync && self.focused {
            sync(gilrs, input);
            self.needs_sync = false;
        }
        gilrs.inc();
    }
}

fn pad_id(id: gilrs::GamepadId) -> GamepadId {
    GamepadId(usize::from(id) as u32)
}

/// A gamepad event in engine terms.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PadEvent {
    Connected,
    Disconnected,
    Pressed(GamepadButton),
    Released(GamepadButton),
    Axis(GamepadAxis, f32),
}

fn translate(ev: &EventType) -> Option<PadEvent> {
    match *ev {
        EventType::Connected => Some(PadEvent::Connected),
        EventType::Disconnected => Some(PadEvent::Disconnected),
        EventType::ButtonPressed(b, _) => button(b).map(PadEvent::Pressed),
        EventType::ButtonReleased(b, _) => button(b).map(PadEvent::Released),
        EventType::ButtonChanged(b, v, _) => trigger_axis(b).map(|a| PadEvent::Axis(a, v)),
        EventType::AxisChanged(a, v, _) => axis(a).map(|a| PadEvent::Axis(a, v)),
        _ => None,
    }
}

fn apply(input: &mut Input, id: GamepadId, ev: PadEvent) {
    match ev {
        PadEvent::Connected => input.connect_gamepad(id, ""),
        PadEvent::Disconnected => input.disconnect_gamepad(id),
        PadEvent::Pressed(b) => input.press_gamepad_button(id, b),
        PadEvent::Released(b) => input.release_gamepad_button(id, b),
        PadEvent::Axis(a, v) => input.set_gamepad_axis(id, a, v),
    }
}

/// Make `input`'s gamepads match the devices' current state.
fn sync(gilrs: &Gilrs, input: &mut Input) {
    let connected: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| pad_id(id)).collect();
    let stale: Vec<GamepadId> = input
        .gamepads()
        .map(|(id, _)| id)
        .filter(|id| !connected.contains(id))
        .collect();
    for id in stale {
        input.disconnect_gamepad(id);
    }
    for (gid, pad) in gilrs.gamepads() {
        let id = pad_id(gid);
        input.connect_gamepad(id, pad.name());
        for b in BUTTONS {
            let Some(mapped) = button(b) else { continue };
            if pad.is_pressed(b) {
                input.press_gamepad_button(id, mapped);
            } else {
                input.release_gamepad_button(id, mapped);
            }
        }
        for a in AXES {
            if let Some(mapped) = axis(a) {
                input.set_gamepad_axis(id, mapped, pad.value(a));
            }
        }
        for b in [Button::LeftTrigger2, Button::RightTrigger2] {
            if let (Some(mapped), Some(data)) = (trigger_axis(b), pad.button_data(b)) {
                input.set_gamepad_axis(id, mapped, data.value());
            }
        }
    }
}

const BUTTONS: [Button; 17] = [
    Button::South,
    Button::East,
    Button::West,
    Button::North,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftTrigger2,
    Button::RightTrigger2,
    Button::LeftThumb,
    Button::RightThumb,
    Button::DPadUp,
    Button::DPadDown,
    Button::DPadLeft,
    Button::DPadRight,
    Button::Start,
    Button::Select,
    Button::Mode,
];

const AXES: [Axis; 4] = [
    Axis::LeftStickX,
    Axis::LeftStickY,
    Axis::RightStickX,
    Axis::RightStickY,
];

/// gilrs calls the bumpers `LeftTrigger`/`RightTrigger` and the triggers
/// `LeftTrigger2`/`RightTrigger2`.
fn button(b: Button) -> Option<GamepadButton> {
    use GamepadButton as G;
    Some(match b {
        Button::South => G::South,
        Button::East => G::East,
        Button::West => G::West,
        Button::North => G::North,
        Button::LeftTrigger => G::LeftShoulder,
        Button::RightTrigger => G::RightShoulder,
        Button::LeftTrigger2 => G::LeftTrigger,
        Button::RightTrigger2 => G::RightTrigger,
        Button::LeftThumb => G::LeftStick,
        Button::RightThumb => G::RightStick,
        Button::DPadUp => G::DPadUp,
        Button::DPadDown => G::DPadDown,
        Button::DPadLeft => G::DPadLeft,
        Button::DPadRight => G::DPadRight,
        Button::Start => G::Start,
        Button::Select => G::Select,
        Button::Mode => G::Mode,
        Button::C | Button::Z | Button::Unknown => return None,
    })
}

/// The analog value of a trigger button.
fn trigger_axis(b: Button) -> Option<GamepadAxis> {
    match b {
        Button::LeftTrigger2 => Some(GamepadAxis::LeftTrigger),
        Button::RightTrigger2 => Some(GamepadAxis::RightTrigger),
        _ => None,
    }
}

/// Stick axes (gilrs reports +Y up, as the engine does). D-pad axes become
/// d-pad buttons through gilrs' default filters; Z axes are unmapped.
fn axis(a: Axis) -> Option<GamepadAxis> {
    match a {
        Axis::LeftStickX => Some(GamepadAxis::LeftStickX),
        Axis::LeftStickY => Some(GamepadAxis::LeftStickY),
        Axis::RightStickX => Some(GamepadAxis::RightStickX),
        Axis::RightStickY => Some(GamepadAxis::RightStickY),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_button_maps_and_bumpers_are_shoulders() {
        for b in BUTTONS {
            assert!(button(b).is_some(), "{b:?}");
        }
        assert_eq!(
            button(Button::LeftTrigger),
            Some(GamepadButton::LeftShoulder)
        );
        assert_eq!(
            button(Button::RightTrigger2),
            Some(GamepadButton::RightTrigger)
        );
        assert_eq!(button(Button::Unknown), None);
        assert_eq!(
            trigger_axis(Button::LeftTrigger2),
            Some(GamepadAxis::LeftTrigger)
        );
        assert_eq!(trigger_axis(Button::South), None);
        for a in AXES {
            assert!(axis(a).is_some(), "{a:?}");
        }
        assert_eq!(axis(Axis::LeftZ), None);
    }

    #[test]
    fn connection_events_translate_without_codes() {
        assert_eq!(translate(&EventType::Connected), Some(PadEvent::Connected));
        assert_eq!(
            translate(&EventType::Disconnected),
            Some(PadEvent::Disconnected)
        );
        assert_eq!(translate(&EventType::Dropped), None);
    }

    #[test]
    fn events_apply_to_input() {
        let id = GamepadId(2);
        let mut input = Input::default();
        for ev in [
            PadEvent::Connected,
            PadEvent::Pressed(GamepadButton::South),
            PadEvent::Axis(GamepadAxis::LeftStickX, -0.8),
            PadEvent::Axis(GamepadAxis::RightTrigger, 0.6),
        ] {
            apply(&mut input, id, ev);
        }
        assert!(input.gamepad_button_pressed(id, GamepadButton::South));
        assert_eq!(input.gamepad_axis_raw(id, GamepadAxis::LeftStickX), -0.8);
        assert_eq!(input.gamepad_axis_raw(id, GamepadAxis::RightTrigger), 0.6);
        apply(&mut input, id, PadEvent::Released(GamepadButton::South));
        assert!(input.gamepad_button_released(id, GamepadButton::South));
        apply(&mut input, id, PadEvent::Disconnected);
        assert!(input.gamepad(id).is_none());
    }

    #[test]
    fn a_disabled_poller_does_nothing() {
        let mut poller = GamepadPoller::disabled();
        assert!(!poller.is_available());
        let mut input = Input::default();
        poller.set_focused(false);
        poller.set_focused(true);
        poller.update(&mut input);
        assert_eq!(input.gamepads().count(), 0);
    }
}
