//! Headless rendering: fixed-size RawInput at pixels_per_point 2.0, offscreen target,
//! PNG readback and frame timing.

use crate::app::Shell;
use crate::gfx::Gfx;
use egui::{Event, Pos2, RawInput, Rect, ViewportId, ViewportInfo, vec2};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct Headless {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub ctx: egui::Context,
    pub shell: Shell,
    pub gfx: Gfx,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    pub size_px: [u32; 2],
    pub ppp: f32,
    pub frame: u64,
    pub errors: Arc<AtomicU32>,
    pub adapter_name: String,
}

#[derive(Clone, Copy, Default)]
pub struct FrameTiming {
    /// `Context::run_ui` (layout + widgets), CPU.
    pub ui_ms: f32,
    /// `Context::tessellate`, CPU.
    pub tess_ms: f32,
    /// Texture/buffer upload and command encoding, CPU.
    pub prep_ms: f32,
    /// Submit + wait for the GPU (wall clock; includes queue latency).
    pub gpu_ms: f32,
    /// GPU execution of the viewport pass (timestamp queries).
    pub gpu_viewport_ms: f32,
    /// GPU execution of the egui pass (timestamp queries).
    pub gpu_egui_ms: f32,
    pub total_ms: f32,
}

impl Headless {
    pub fn new(shell: Shell, size_px: [u32; 2], ppp: f32) -> anyhow::Result<Self> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::PRIMARY;
        let instance = wgpu::Instance::new(desc.with_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))?;
        let adapter_name = format!(
            "{} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        let ts_ok = adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("s1 headless"),
                required_features: if ts_ok {
                    wgpu::Features::TIMESTAMP_QUERY
                } else {
                    wgpu::Features::empty()
                },
                ..Default::default()
            }))?;
        let errors = Arc::new(AtomicU32::new(0));
        let ec = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            ec.fetch_add(1, Ordering::Relaxed);
            eprintln!("wgpu error: {e}");
        }));
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless target"),
            size: wgpu::Extent3d {
                width: size_px[0],
                height: size_px[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let padded = (size_px[0] * 4).div_ceil(256) * 256;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded * size_px[1]) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut gfx = Gfx::new(&device, FORMAT, 0);
        if ts_ok {
            gfx.timestamps = Some(crate::gfx::Timestamps::new(&device, &queue));
        }
        Ok(Self {
            device,
            queue,
            ctx,
            shell,
            gfx,
            target,
            view,
            readback,
            size_px,
            ppp,
            frame: 0,
            errors,
            adapter_name,
        })
    }

    fn input(&self, events: Vec<Event>) -> RawInput {
        let mut vp = ViewportInfo {
            native_pixels_per_point: Some(self.ppp),
            focused: Some(true),
            ..Default::default()
        };
        let pts = vec2(
            self.size_px[0] as f32 / self.ppp,
            self.size_px[1] as f32 / self.ppp,
        );
        vp.inner_rect = Some(Rect::from_min_size(Pos2::ZERO, pts));
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, pts)),
            time: Some(self.frame as f64 / 60.0),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        raw.viewports.insert(ViewportId::ROOT, vp);
        raw
    }

    /// One full frame: egui run, tessellate, viewport pass, egui paint, submit, wait for GPU.
    pub fn frame(&mut self, events: Vec<Event>) -> FrameTiming {
        let start = Instant::now();
        let raw = self.input(events);
        let shell = &mut self.shell;
        let out = self.ctx.run_ui(raw, |ui| shell.ui(ui));
        let ui_ms = start.elapsed().as_secs_f32() * 1000.0;
        let time = self.frame as f32 / 60.0;
        let p0 = Instant::now();
        let (encoder, tess_ms) = self.gfx.paint(
            &self.device,
            &self.queue,
            &self.ctx,
            &mut self.shell,
            out,
            &self.view,
            self.size_px,
            time,
        );
        let prep_ms = p0.elapsed().as_secs_f32() * 1000.0 - tess_ms;
        let g0 = Instant::now();
        self.queue.submit([encoder.finish()]);
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let gpu_ms = g0.elapsed().as_secs_f32() * 1000.0;
        self.frame += 1;
        let total_ms = start.elapsed().as_secs_f32() * 1000.0;
        let (gpu_viewport_ms, gpu_egui_ms) = self
            .gfx
            .timestamps
            .as_ref()
            .and_then(|t| t.read(&self.device))
            .map(|(vp, eg)| (if self.gfx.skip_viewport { 0.0 } else { vp }, eg))
            .unwrap_or((0.0, 0.0));
        self.shell.ed.frame_ms = total_ms;
        FrameTiming {
            ui_ms,
            tess_ms,
            prep_ms,
            gpu_ms,
            gpu_viewport_ms,
            gpu_egui_ms,
            total_ms,
        }
    }

    pub fn capture(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        // Render a fresh frame, then read it back.
        self.frame(vec![]);
        let [w, h] = self.size_px;
        let padded = (w * 4).div_ceil(256) * 256;
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        rx.recv()??;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        {
            let data = slice
                .get_mapped_range()
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            for row in data.chunks(padded as usize).take(h as usize) {
                pixels.extend_from_slice(&row[..(w * 4) as usize]);
            }
        }
        self.readback.unmap();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::create(path)?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&pixels)?;
        Ok(())
    }
}
