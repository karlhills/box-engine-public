//! Game input during Play: while the game view has focus, this frame's egui
//! keyboard and pointer input is copied into the play world's
//! [`box_core::input::Input`], and keyboard events are then removed so editor
//! shortcuts and widgets never see game keys.
//!
//! Mouse motion is the pointer delta inside the window (physical pixels),
//! except while the game has grabbed the cursor: the windowed runner then
//! adds raw device motion instead. Gamepads are fed by the editor's
//! `GamepadPoller` (windowed runs); named actions are evaluated by the play
//! world from this `Input`.

use box_core::input::{Input, KeyCode, Modifiers, MouseButton};
use egui::{Event, Key, MouseWheelUnit, PointerButton, Rect};
use glam::Vec2;

/// The engine key for an egui key (physical position when known).
pub fn key_code(key: Key) -> Option<KeyCode> {
    use KeyCode as K;
    Some(match key {
        Key::A => K::A,
        Key::B => K::B,
        Key::C => K::C,
        Key::D => K::D,
        Key::E => K::E,
        Key::F => K::F,
        Key::G => K::G,
        Key::H => K::H,
        Key::I => K::I,
        Key::J => K::J,
        Key::K => K::K,
        Key::L => K::L,
        Key::M => K::M,
        Key::N => K::N,
        Key::O => K::O,
        Key::P => K::P,
        Key::Q => K::Q,
        Key::R => K::R,
        Key::S => K::S,
        Key::T => K::T,
        Key::U => K::U,
        Key::V => K::V,
        Key::W => K::W,
        Key::X => K::X,
        Key::Y => K::Y,
        Key::Z => K::Z,
        Key::Num0 => K::Digit0,
        Key::Num1 => K::Digit1,
        Key::Num2 => K::Digit2,
        Key::Num3 => K::Digit3,
        Key::Num4 => K::Digit4,
        Key::Num5 => K::Digit5,
        Key::Num6 => K::Digit6,
        Key::Num7 => K::Digit7,
        Key::Num8 => K::Digit8,
        Key::Num9 => K::Digit9,
        Key::Space => K::Space,
        Key::Enter => K::Enter,
        Key::Escape => K::Escape,
        Key::Tab => K::Tab,
        Key::Backspace => K::Backspace,
        Key::Delete => K::Delete,
        Key::ArrowLeft => K::Left,
        Key::ArrowRight => K::Right,
        Key::ArrowUp => K::Up,
        Key::ArrowDown => K::Down,
        Key::F1 => K::F1,
        Key::F2 => K::F2,
        Key::F3 => K::F3,
        Key::F4 => K::F4,
        Key::F5 => K::F5,
        Key::F6 => K::F6,
        Key::F7 => K::F7,
        Key::F8 => K::F8,
        Key::F9 => K::F9,
        Key::F10 => K::F10,
        Key::F11 => K::F11,
        Key::F12 => K::F12,
        _ => return None,
    })
}

pub fn mouse_button(button: PointerButton) -> MouseButton {
    match button {
        PointerButton::Primary => MouseButton::Left,
        PointerButton::Secondary => MouseButton::Right,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Extra1 => MouseButton::Other(3),
        PointerButton::Extra2 => MouseButton::Other(4),
    }
}

/// Copies this frame's input into `input` (the game has focus; `viewport` is
/// the game view in points), then strips keyboard and text events from egui.
/// Button presses count only inside the viewport; releases always do.
/// `pointer_motion` adds the pointer delta to `mouse_delta`; pass false when
/// raw device motion is forwarded instead (a grabbed cursor).
pub fn forward(ctx: &egui::Context, input: &mut Input, viewport: Rect, pointer_motion: bool) {
    let ppp = ctx.pixels_per_point();
    ctx.input(|i| {
        // egui reports modifiers as state, not key events.
        let m = i.modifiers;
        for (down, key) in [
            (m.shift, KeyCode::ShiftLeft),
            (m.ctrl, KeyCode::ControlLeft),
            (m.alt, KeyCode::AltLeft),
            (m.mac_cmd, KeyCode::SuperLeft),
        ] {
            if down {
                input.press_key(key);
            } else {
                input.release_key(key);
            }
        }
        input.modifiers = Modifiers {
            shift: m.shift,
            ctrl: m.ctrl,
            alt: m.alt,
            meta: m.mac_cmd,
        };
        input.cursor_position = i
            .pointer
            .latest_pos()
            .filter(|p| viewport.contains(*p))
            .map(|p| Vec2::new(p.x - viewport.left(), p.y - viewport.top()) * ppp);
        if pointer_motion {
            let d = i.pointer.delta();
            input.mouse_delta += Vec2::new(d.x, d.y) * ppp;
        }
        for e in &i.events {
            match e {
                Event::Key {
                    key,
                    physical_key,
                    pressed,
                    ..
                } => {
                    if let Some(code) = key_code(physical_key.unwrap_or(*key)) {
                        if *pressed {
                            input.press_key(code);
                        } else {
                            input.release_key(code);
                        }
                    }
                }
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    let b = mouse_button(*button);
                    if !*pressed {
                        input.release_button(b);
                    } else if viewport.contains(*pos) {
                        input.press_button(b);
                    }
                }
                Event::MouseWheel { unit, delta, .. } => {
                    let d = Vec2::new(delta.x, delta.y);
                    match unit {
                        MouseWheelUnit::Line => input.scroll_lines += d,
                        MouseWheelUnit::Page => input.scroll_lines += d * 10.0,
                        MouseWheelUnit::Point => input.scroll_pixels += d,
                    }
                }
                Event::Zoom(f) if *f > 0.0 => input.pinch += f.ln(),
                _ => {}
            }
        }
    });
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            !matches!(
                e,
                Event::Key { .. }
                    | Event::Text(_)
                    | Event::Copy
                    | Event::Cut
                    | Event::Paste(_)
                    | Event::Ime(_)
            )
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Modifiers as EguiModifiers, RawInput, pos2, vec2};

    fn key(key: Key, pressed: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: EguiModifiers::NONE,
        }
    }

    #[test]
    fn keys_and_buttons_reach_the_game_and_keys_leave_egui() {
        let ctx = egui::Context::default();
        let viewport = Rect::from_min_size(pos2(100.0, 50.0), vec2(400.0, 300.0));
        let mut input = Input::default();
        let raw = RawInput {
            events: vec![
                key(Key::W, true),
                key(Key::Space, true),
                Event::PointerButton {
                    pos: pos2(200.0, 100.0),
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: EguiModifiers::NONE,
                },
                // Outside the viewport: ignored.
                Event::PointerButton {
                    pos: pos2(10.0, 10.0),
                    button: PointerButton::Secondary,
                    pressed: true,
                    modifiers: EguiModifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let mut seen_after = usize::MAX;
        let _ = ctx.run_ui(raw, |ui| {
            forward(ui.ctx(), &mut input, viewport, true);
            seen_after = ui.input(|i| {
                i.events
                    .iter()
                    .filter(|e| matches!(e, Event::Key { .. }))
                    .count()
            });
        });
        assert_eq!(seen_after, 0, "game keys must not reach editor shortcuts");
        assert!(input.key_pressed(KeyCode::W) && input.key_down(KeyCode::Space));
        assert!(input.button_pressed(MouseButton::Left));
        assert!(!input.button_down(MouseButton::Right));
        assert_eq!(key_code(Key::Num7), Some(KeyCode::Digit7));
        assert_eq!(key_code(Key::Comma), None);
    }
}
