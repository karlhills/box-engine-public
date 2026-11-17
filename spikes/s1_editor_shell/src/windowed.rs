//! Windowed runner: winit 0.30 + egui-winit + egui-wgpu, no eframe.

use crate::app::{self, Shell};
use crate::gfx::Gfx;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui: egui_winit::State,
    gfx: Gfx,
}

struct WinApp {
    shell: Shell,
    state: Option<State>,
    start: Instant,
    last: Instant,
    smoothed_ms: f32,
    layout_path: std::path::PathBuf,
    frame_times: Vec<f32>,
    /// Auto-quit after this many seconds (for scripted runs).
    seconds: Option<f32>,
}

impl ApplicationHandler for WinApp {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("box-engine — S1 editor shell")
                    .with_inner_size(LogicalSize::new(1440.0, 900.0)),
            )
            .expect("window"),
        );
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(desc.with_env());
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .expect("adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("s1 windowed"),
            ..Default::default()
        }))
        .expect("device");
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // egui-wgpu prefers a non-sRGB (gamma) framebuffer.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface config");
        config.format = format;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&device, &config);

        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let egui = egui_winit::State::new(
            ctx,
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let gfx = Gfx::new(&device, format, 6);
        println!(
            "adapter: {} ({:?}), surface {:?}, scale {}",
            adapter.get_info().name,
            adapter.get_info().backend,
            format,
            window.scale_factor()
        );
        window.request_redraw();
        self.state = Some(State {
            window,
            surface,
            config,
            device,
            queue,
            egui,
            gfx,
        });
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some(s) = self.seconds {
            if self.start.elapsed().as_secs_f32() > s {
                let _ = app::save_layout(&self.layout_path, &self.shell.tree);
                report(&self.frame_times);
                println!("auto-quit after {s} s");
                el.exit();
            } else {
                el.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(
                    Instant::now() + std::time::Duration::from_millis(250),
                ));
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(st) = &mut self.state else {
            return;
        };
        let _ = st.egui.on_window_event(&st.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                let _ = app::save_layout(&self.layout_path, &self.shell.tree);
                report(&self.frame_times);
                el.exit();
            }
            WindowEvent::Occluded(false) => st.window.request_redraw(),
            WindowEvent::Resized(size) => {
                st.config.width = size.width.max(1);
                st.config.height = size.height.max(1);
                st.surface.configure(&st.device, &st.config);
                st.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last).as_secs_f32() * 1000.0;
                self.last = now;
                self.frame_times.push(dt);
                self.smoothed_ms = self.smoothed_ms * 0.9 + dt * 0.1;
                self.shell.ed.frame_ms = self.smoothed_ms;

                let acquired = st.surface.get_current_texture();
                if self.frame_times.len() <= 3 {
                    println!(
                        "redraw {}: surface {:?}",
                        self.frame_times.len(),
                        match &acquired {
                            wgpu::CurrentSurfaceTexture::Success(_) => "success",
                            wgpu::CurrentSurfaceTexture::Suboptimal(_) => "suboptimal",
                            wgpu::CurrentSurfaceTexture::Timeout => "timeout",
                            wgpu::CurrentSurfaceTexture::Occluded => "occluded",
                            wgpu::CurrentSurfaceTexture::Outdated => "outdated",
                            wgpu::CurrentSurfaceTexture::Lost => "lost",
                            _ => "other",
                        }
                    );
                }
                let frame = match acquired {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        st.surface.configure(&st.device, &st.config);
                        st.window.request_redraw();
                        return;
                    }
                    wgpu::CurrentSurfaceTexture::Occluded => {
                        // Wait for WindowEvent::Occluded(false) instead of spinning.
                        return;
                    }
                    _ => {
                        st.window.request_redraw();
                        return;
                    }
                };
                let view = frame.texture.create_view(&Default::default());
                let raw = st.egui.take_egui_input(&st.window);
                let ctx = st.egui.egui_ctx().clone();
                let shell = &mut self.shell;
                let out = ctx.run_ui(raw, |ui| shell.ui(ui));
                if self.shell.ed.quit {
                    let _ = app::save_layout(&self.layout_path, &self.shell.tree);
                    report(&self.frame_times);
                    el.exit();
                    return;
                }
                let platform = out.platform_output.clone();
                let (encoder, _) = st.gfx.paint(
                    &st.device,
                    &st.queue,
                    &ctx,
                    &mut self.shell,
                    out,
                    &view,
                    [st.config.width, st.config.height],
                    self.start.elapsed().as_secs_f32(),
                );
                st.queue.submit([encoder.finish()]);
                st.window.pre_present_notify();
                st.queue.present(frame);
                st.egui.handle_platform_output(&st.window, platform);
                if self.frame_times.len() == 1 || self.frame_times.len().is_multiple_of(120) {
                    println!("presented frame {}", self.frame_times.len());
                }
                // The viewport animates, so redraw continuously (vsync paces us).
                st.window.request_redraw();
            }
            _ => {}
        }
    }
}

fn report(frame_times: &[f32]) {
    if frame_times.len() < 30 {
        return;
    }
    let mut v: Vec<f32> = frame_times[10..].to_vec();
    v.sort_by(f32::total_cmp);
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let p95 = v[(v.len() as f32 * 0.95) as usize];
    let p99 = v[(v.len() as f32 * 0.99) as usize];
    let over = |budget: f32| v.iter().filter(|&&x| x > budget * 1.5).count();
    println!(
        "frames over 1.5x of 8.33 ms: {}, over 1.5x of 16.67 ms: {}",
        over(8.333),
        over(16.667)
    );
    println!(
        "frames: {}  interval mean {:.2} ms  p95 {:.2}  p99 {:.2}  max {:.2}",
        v.len(),
        mean,
        p95,
        p99,
        v[v.len() - 1]
    );
}

pub fn run(seconds: Option<f32>) -> anyhow::Result<()> {
    let layout_path = std::path::PathBuf::from(".box/layout.json");
    let tree = match app::load_layout(&layout_path) {
        Ok(t) => t,
        Err(e) => {
            println!("layout: using default ({e})");
            app::default_layout()
        }
    };
    let mut shell = Shell::new(tree);
    if let Some(id) = shell.ed.find("Townhouse_013") {
        shell.ed.reveal_and_select(id);
    }
    let el = EventLoop::new()?;
    let mut app = WinApp {
        shell,
        state: None,
        start: Instant::now(),
        last: Instant::now(),
        smoothed_ms: 16.0,
        layout_path,
        frame_times: Vec::new(),
        seconds,
    };
    el.run_app(&mut app)?;
    Ok(())
}
