//! Engine input glue: [`InputPlugin`] (actions plus cursor options),
//! [`CursorController`], which applies [`CursorOptions`] to a winit window,
//! and [`apply_window_event`], the winit-to-[`Input`] mapping the runner
//! uses (for games that run their own event loop, for example to draw an
//! egui interface over the scene).

use box_core::action::{Actions, ActionsPlugin};
use box_core::input::{CursorGrab, CursorOptions};
use box_core::prelude::*;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Window};

/// Named actions ([`ActionsPlugin`]), the [`CursorOptions`] resource and a
/// warning, once per name, when a system reads an action the map does not
/// define. [`crate::app_with_world`] adds it before any other system.
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(&ActionsPlugin);
        app.world.resource_or_default::<CursorOptions>();
        app.add_system(Stage::PostUpdate, warn_undefined_actions);
        // Playtests: input goes in before actions are evaluated, recording
        // reads them after (crate::playtest).
        app.system(Stage::FixedUpdate, crate::playtest::playtest_frame_system)
            .before("actions_fixed_step_system");
        app.system(Stage::Update, crate::playtest::playtest_frame_system)
            .before("actions_update_system");
        app.system(Stage::Update, crate::playtest::playtest_record_system)
            .after("actions_update_system");
    }
}

fn warn_undefined_actions(world: &mut World) {
    if let Some(actions) = world.get_resource::<Actions>() {
        for name in actions.take_unreported_undefined() {
            tracing::warn!(
                "action {name:?} is not in the ActionMap; it reads as released and zero"
            );
        }
    }
}

/// Applies the world's [`CursorOptions`] to a window: grabs and hides the
/// cursor when asked, releases it while the window is unfocused and grabs it
/// again when focus returns. Writes what it applied to
/// [`CursorOptions::active_grab`].
///
/// `Locked` falls back to `Confined` where the platform cannot lock
/// (Windows), recentring the hidden cursor each frame; `Confined` falls back
/// to `Locked` where it cannot confine (macOS).
pub struct CursorController {
    focused: bool,
    /// The (grab, visible) last sent to the window.
    applied: Option<(CursorGrab, bool)>,
    active: CursorGrab,
    warned: bool,
}

impl Default for CursorController {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorController {
    pub fn new() -> Self {
        Self {
            focused: true,
            applied: None,
            active: CursorGrab::None,
            warned: false,
        }
    }

    /// Call on every focus change. Losing focus releases the cursor at once.
    pub fn set_focused(&mut self, window: &Window, focused: bool) {
        self.focused = focused;
        if !focused {
            self.apply(window, (CursorGrab::None, true));
        }
    }

    /// Apply the requested cursor state if it changed. Call once per frame
    /// after the app's update.
    pub fn update(&mut self, window: &Window, world: &mut World) {
        let Some(options) = world.get_resource_mut::<CursorOptions>() else {
            return;
        };
        let want = if self.focused {
            (options.grab, options.visible)
        } else {
            (CursorGrab::None, true)
        };
        if self.applied != Some(want) {
            self.apply(window, want);
        }
        options.active_grab = self.active;
        if want.0 == CursorGrab::Locked && self.active == CursorGrab::Confined {
            let size = window.inner_size();
            let _ = window.set_cursor_position(PhysicalPosition::new(
                size.width as f64 / 2.0,
                size.height as f64 / 2.0,
            ));
        }
    }

    fn apply(&mut self, window: &Window, (grab, visible): (CursorGrab, bool)) {
        let attempts: &[(CursorGrabMode, CursorGrab)] = match grab {
            CursorGrab::None => &[(CursorGrabMode::None, CursorGrab::None)],
            CursorGrab::Locked => &[
                (CursorGrabMode::Locked, CursorGrab::Locked),
                (CursorGrabMode::Confined, CursorGrab::Confined),
            ],
            CursorGrab::Confined => &[
                (CursorGrabMode::Confined, CursorGrab::Confined),
                (CursorGrabMode::Locked, CursorGrab::Locked),
            ],
        };
        let mut result = Err(None);
        for &(mode, got) in attempts {
            match window.set_cursor_grab(mode) {
                Ok(()) => {
                    result = Ok(got);
                    break;
                }
                Err(e) => result = Err(Some(e)),
            }
        }
        self.active = match result {
            Ok(got) => got,
            Err(e) => {
                if !self.warned {
                    let reason = e.map(|e| e.to_string()).unwrap_or_default();
                    tracing::warn!("could not grab the cursor ({grab:?}): {reason}");
                    self.warned = true;
                }
                CursorGrab::None
            }
        };
        window.set_cursor_visible(visible);
        self.applied = Some((grab, visible));
    }
}

/// Feeds one winit window event into `input`: keys, modifiers, mouse
/// buttons, cursor position (physical pixels), wheel and trackpad scroll
/// (`scale_factor` turns pixel deltas into logical pixels), pinch, and a
/// focus loss (which releases everything held). Returns whether the event
/// was one of these. The runner calls it for every window event; a game
/// with its own event loop calls it the same way, after asking its UI
/// whether it wants the event.
pub fn apply_window_event(input: &mut Input, event: &WindowEvent, scale_factor: f64) -> bool {
    match event {
        WindowEvent::Focused(false) => input.release_all(),
        WindowEvent::KeyboardInput { event, .. } => {
            if let PhysicalKey::Code(code) = event.physical_key
                && let Some(k) = crate::input_map::key(code)
            {
                match event.state {
                    ElementState::Pressed => input.press_key(k),
                    ElementState::Released => input.release_key(k),
                }
            }
        }
        WindowEvent::ModifiersChanged(m) => {
            let s = m.state();
            input.modifiers = Modifiers {
                shift: s.shift_key(),
                ctrl: s.control_key(),
                alt: s.alt_key(),
                meta: s.super_key(),
            };
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let b = crate::input_map::button(*button);
            match state {
                ElementState::Pressed => input.press_button(b),
                ElementState::Released => input.release_button(b),
            }
        }
        WindowEvent::CursorMoved { position, .. } => {
            input.cursor_position = Some(Vec2::new(position.x as f32, position.y as f32));
        }
        WindowEvent::CursorLeft { .. } => input.cursor_position = None,
        WindowEvent::MouseWheel { delta, .. } => match delta {
            MouseScrollDelta::LineDelta(x, y) => input.scroll_lines += Vec2::new(*x, *y),
            MouseScrollDelta::PixelDelta(p) => {
                input.scroll_pixels +=
                    Vec2::new((p.x / scale_factor) as f32, (p.y / scale_factor) as f32)
            }
        },
        WindowEvent::PinchGesture { delta, .. } => input.pinch += *delta as f32,
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Jumps(u32);

    /// The engine app runs fixed steps and evaluates actions before game
    /// systems in both FixedUpdate and Update.
    #[test]
    fn engine_apps_evaluate_actions_in_fixed_and_frame_stages() {
        let mut app = crate::default_app();
        app.world.insert_resource(
            ActionMap::new().button("jump", [Binding::Gamepad(GamepadButton::South)]),
        );
        app.world.insert_resource(Jumps::default());
        app.add_system(Stage::FixedUpdate, |w| {
            if w.resource::<Actions>().pressed("jump") {
                w.resource_mut::<Jumps>().0 += 1;
            }
        });
        app.add_system(Stage::Update, |w| {
            let _ = w.resource::<Actions>().pressed("jupm");
        });
        assert_eq!(
            *app.world.resource::<CursorOptions>(),
            CursorOptions::free()
        );
        app.world
            .resource_mut::<Input>()
            .press_gamepad_button(GamepadId(0), GamepadButton::South);
        for _ in 0..3 {
            app.tick(1.0 / 30.0);
            app.world.resource_mut::<Input>().end_frame();
        }
        assert_eq!(app.world.resource::<FixedTime>().tick, 6);
        assert_eq!(app.world.resource::<Jumps>().0, 1);
        let actions = app.world.resource::<Actions>();
        assert_eq!(actions.undefined(), vec!["jupm".to_string()]);
        assert!(
            actions.take_unreported_undefined().is_empty(),
            "already logged by the engine"
        );
    }
}
