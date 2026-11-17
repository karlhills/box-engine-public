//! Windowed runner: winit 0.30 + egui-winit + egui-wgpu on box_render's
//! device and surface (no eframe). During Play it also applies the game's
//! `CursorOptions` to the window (grab and hide while the game has focus)
//! and forwards raw mouse motion for a grabbed cursor.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use box_core::input::Input;
use box_engine::CursorController;
use box_render::Renderer;
use glam::Vec2;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::editor::Editor;
use crate::gfx::EditorGfx;

/// Frames a new viewport size must hold before the target is reallocated.
const RESIZE_DEBOUNCE_FRAMES: u32 = 6;
/// How often agent control requests are answered while the window is
/// occluded and no frames are drawn.
const OCCLUDED_POLL: Duration = Duration::from_millis(16);

struct Gpu {
    window: Arc<Window>,
    renderer: Renderer,
    egui: egui_winit::State,
    gfx: EditorGfx,
}

struct App {
    editor: Editor,
    title: String,
    size: [u32; 2],
    /// Quit after this many presented frames (`--frames`).
    max_frames: Option<u64>,
    presented: u64,
    /// `on_quit` ran; events still queued before the loop exits are ignored.
    quitting: bool,
    gpu: Option<Gpu>,
    /// The OS reports the window as fully hidden; redraws pause until shown.
    occluded: bool,
    last_frame: Option<Instant>,
    error: Option<anyhow::Error>,
    /// Applies the play world's cursor grab to the window.
    cursor: CursorController,
    /// Whether `cursor` was last told the game has focus.
    cursor_focus: bool,
    /// When the system clipboard was last read for the inspector's paste.
    clipboard_read: Option<Instant>,
}

/// How often the system clipboard is read while the inspector shows paste
/// targets.
const CLIPBOARD_REFRESH: Duration = Duration::from_millis(500);

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        tracing::error!("{error:#}");
        // A lost GPU device ends the editor: keep the unsaved changes (the
        // recovery file is offered at the next start) and write a report.
        if box_engine::crash::report_device_lost(&error).is_some() && !self.quitting {
            self.quitting = true;
            self.editor.on_quit();
        }
        self.error = Some(error);
        event_loop.exit();
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        if !self.quitting {
            self.quitting = true;
            self.editor.on_quit();
        }
        event_loop.exit();
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.quitting {
            return;
        }
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let frame = match gpu.renderer.acquire_surface_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                // Occluded: wait for `Occluded(false)`. Otherwise (timeout,
                // outdated) back off briefly instead of spinning.
                if !self.occluded {
                    std::thread::sleep(Duration::from_millis(8));
                    gpu.window.request_redraw();
                }
                return;
            }
            Err(e) => return self.fail(event_loop, e.into()),
        };

        let now = Instant::now();
        if let Some(last) = self.last_frame {
            let dt = (now - last).as_secs_f32().max(1e-4);
            let fps = &mut self.editor.state.status.fps;
            *fps = if *fps == 0.0 {
                1.0 / dt
            } else {
                *fps * 0.9 + 0.1 / dt
            };
        }
        self.last_frame = Some(now);

        self.editor.poll_control(Some(&mut gpu.renderer));
        // The inspector showed paste targets: read the system clipboard (at
        // most twice a second) so values copied in another editor paste.
        if self.editor.state.clipboard_mut().take_wanted()
            && self
                .clipboard_read
                .is_none_or(|t| now - t >= CLIPBOARD_REFRESH)
        {
            self.clipboard_read = Some(now);
            let text = gpu.egui.clipboard_text();
            self.editor.state.clipboard_mut().set_system_text(text);
        }
        let raw = gpu.egui.take_egui_input(&gpu.window);
        let ctx = gpu.egui.egui_ctx().clone();
        let editor = &mut self.editor;
        let output = ctx.run_ui(raw, |ui| editor.ui(ui));
        // The cursor follows game focus (Escape, Stop, Pause and blur release
        // it); while focused, the play world's CursorOptions decide.
        let focus = self.editor.state.game_focus();
        if focus != self.cursor_focus {
            self.cursor.set_focused(&gpu.window, focus);
            self.cursor_focus = focus;
        }
        if self.editor.state.is_playing() {
            self.cursor
                .update(&gpu.window, &mut self.editor.state.world);
        }
        let platform = output.platform_output.clone();
        let size = gpu.window.inner_size();
        let encoder = gpu.gfx.frame(
            &mut gpu.renderer,
            &ctx,
            &mut self.editor,
            output,
            &frame.view,
            [size.width.max(1), size.height.max(1)],
        );
        gpu.renderer.queue().submit([encoder.finish()]);
        gpu.window.pre_present_notify();
        gpu.renderer.present_surface_frame(frame);
        gpu.egui.handle_platform_output(&gpu.window, platform);
        let title = self.editor.title();
        if title != self.title {
            gpu.window.set_title(&title);
            self.title = title;
        }

        let errors = gpu.renderer.validation_errors();
        if errors > 0 {
            return self.fail(
                event_loop,
                anyhow::anyhow!("{errors} GPU validation error(s); see the log"),
            );
        }
        self.presented += 1;
        if self.editor.quit_requested() || self.max_frames.is_some_and(|n| self.presented >= n) {
            return self.quit(event_loop);
        }
        // Vsync paces continuous redraws.
        gpu.window.request_redraw();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(self.size[0], self.size[1]));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("could not create window"),
                );
            }
        };
        let size = window.inner_size();
        let renderer = match Renderer::new_windowed(window.clone(), size.width, size.height) {
            Ok(r) => r,
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("could not create renderer"),
                );
            }
        };
        box_engine::crash::set_gpu(renderer.adapter_info());
        let format = renderer
            .surface_format()
            .expect("a windowed renderer has a surface");
        tracing::info!(
            "adapter {} ({:?}), surface {format:?}, scale {}",
            renderer.adapter_info().name,
            renderer.adapter_info().backend,
            window.scale_factor()
        );
        let ctx = egui::Context::default();
        self.editor.apply_theme(&ctx);
        let egui = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(renderer.device().limits().max_texture_dimension_2d as usize),
        );
        let gfx = EditorGfx::new(&renderer, format, RESIZE_DEBOUNCE_FRAMES);
        window.request_redraw();
        self.gpu = Some(Gpu {
            window,
            renderer,
            egui,
            gfx,
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let response = gpu.egui.on_window_event(&gpu.window, &event);
        match event {
            // With unsaved changes the editor shows its prompt first; the
            // answer sets `quit_requested`, checked after each frame.
            WindowEvent::CloseRequested => {
                if self.editor.request_quit() {
                    self.quit(event_loop);
                } else {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                gpu.renderer.resize(size.width, size.height);
                gpu.window.request_redraw();
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                self.last_frame = None;
                if !occluded {
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                // Release game input and the cursor at once, not next frame.
                self.editor.state.set_game_focus(false);
                if self.cursor_focus {
                    self.cursor.set_focused(&gpu.window, false);
                    self.cursor_focus = false;
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {
                if response.repaint {
                    gpu.window.request_redraw();
                }
            }
        }
    }

    /// Raw mouse motion goes to the game while it has grabbed the cursor
    /// (the pointer delta stands still then).
    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.editor.state.game_cursor_grabbed()
            && let Some(input) = self.editor.state.world.get_resource_mut::<Input>()
        {
            input.add_mouse_motion(Vec2::new(delta.0 as f32, delta.1 as f32));
        }
    }

    /// No frames are drawn while the window is occluded; agent control
    /// requests are still answered (screenshots render offscreen).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let idle = self.occluded && !self.quitting && self.editor.state.agent_control().is_some();
        match &mut self.gpu {
            Some(gpu) if idle => {
                self.editor.poll_control(Some(&mut gpu.renderer));
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + OCCLUDED_POLL));
            }
            _ => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    /// The loop is ending without `quit` (e.g. the macOS app menu's Quit,
    /// which terminates the application directly): no prompt is possible, so
    /// at least save the layout and keep unsaved changes in the recovery file.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if !self.quitting {
            self.quitting = true;
            self.editor.on_quit();
        }
    }
}

/// Opens the editor window and runs until it is closed, or until
/// `max_frames` frames have been presented.
pub fn run(editor: Editor, size: [u32; 2], max_frames: Option<u64>) -> Result<()> {
    let title = editor.title();
    // Play sessions in the window play sound (the edit world never does).
    box_engine::box_audio::enable_device_output();
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    let mut app = App {
        editor,
        title,
        size,
        max_frames,
        presented: 0,
        quitting: false,
        gpu: None,
        occluded: false,
        last_frame: None,
        error: None,
        cursor: CursorController::new(),
        // The controller starts focused: the first frame tells it otherwise.
        cursor_focus: true,
        clipboard_read: None,
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
