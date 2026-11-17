//! Shared per-frame GPU work: viewport pass + egui paint into a target view.

use crate::app::Shell;
use crate::viewport::Viewport;

/// GPU timestamps around the viewport pass (0,1) and the egui pass (2,3).
pub struct Timestamps {
    pub set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    pub readback: wgpu::Buffer,
    pub period_ns: f32,
}

impl Timestamps {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("ts"),
            ty: wgpu::QueryType::Timestamp,
            count: 4,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ts resolve"),
            size: 32,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ts readback"),
            size: 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            set,
            resolve,
            readback,
            period_ns: queue.get_timestamp_period(),
        }
    }

    /// Reads the last frame's (viewport_ms, egui_ms). Call after the frame's GPU work is done.
    pub fn read(&self, device: &wgpu::Device) -> Option<(f32, f32)> {
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        rx.recv().ok()?.ok()?;
        let v: [u64; 4] = {
            let data = slice.get_mapped_range().ok()?;
            let mut v = [0u64; 4];
            for (i, c) in data.chunks(8).take(4).enumerate() {
                v[i] = u64::from_le_bytes(c.try_into().ok()?);
            }
            v
        };
        self.readback.unmap();
        let ms = |a: u64, b: u64| b.saturating_sub(a) as f32 * self.period_ns / 1.0e6;
        Some((ms(v[0], v[1]), ms(v[2], v[3])))
    }
}

pub struct Gfx {
    pub timestamps: Option<Timestamps>,
    pub renderer: egui_wgpu::Renderer,
    pub viewport: Viewport,
    /// Windowed runs debounce viewport resizes; headless runs apply them immediately.
    pub debounce_frames: u32,
    /// Bench only: skip the viewport pass to isolate egui's own cost.
    pub skip_viewport: bool,
    pending: Option<([u32; 2], u32)>,
}

impl Gfx {
    pub fn new(
        device: &wgpu::Device,
        output_format: wgpu::TextureFormat,
        debounce_frames: u32,
    ) -> Self {
        Self {
            timestamps: None,
            renderer: egui_wgpu::Renderer::new(
                device,
                output_format,
                egui_wgpu::RendererOptions::default(),
            ),
            viewport: Viewport::new(device),
            debounce_frames,
            skip_viewport: false,
            pending: None,
        }
    }

    /// Picks up the viewport size the UI asked for this frame, with an optional debounce so a
    /// live window resize does not reallocate the target every frame.
    fn sync_viewport(&mut self, device: &wgpu::Device, shell: &mut Shell) {
        let req = shell.ed.viewport_request;
        if req[0] == 0 || req[1] == 0 {
            return;
        }
        let first = self.viewport.texture_id.is_none();
        if req != self.viewport.size {
            let stable = match self.pending {
                Some((s, n)) if s == req => n + 1,
                _ => 0,
            };
            self.pending = Some((req, stable));
            if first || stable >= self.debounce_frames {
                self.viewport.ensure_size(device, &mut self.renderer, req);
                self.pending = None;
            }
        }
        shell.ed.viewport_tex = self.viewport.texture_id;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ctx: &egui::Context,
        shell: &mut Shell,
        output: egui::FullOutput,
        target: &wgpu::TextureView,
        size_px: [u32; 2],
        time: f32,
    ) -> (wgpu::CommandEncoder, f32) {
        let t0 = std::time::Instant::now();
        let ppp = output.pixels_per_point;
        let prims = ctx.tessellate(output.shapes, ppp);
        let tess_ms = t0.elapsed().as_secs_f32() * 1000.0;

        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        self.sync_viewport(device, shell);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        if !self.skip_viewport {
            self.viewport.render(
                queue,
                &mut encoder,
                time,
                shell.ed.cam_yaw,
                shell.ed.cam_pitch,
                shell.ed.selected.is_some() && shell.ed.outline_selected,
                self.timestamps.as_ref().map(|t| &t.set),
            );
        }

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size_px,
            pixels_per_point: ppp,
        };
        let cbs = self
            .renderer
            .update_buffers(device, queue, &mut encoder, &prims, &screen);
        assert!(cbs.is_empty());
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: self.timestamps.as_ref().map(|t| {
                    wgpu::RenderPassTimestampWrites {
                        query_set: &t.set,
                        beginning_of_pass_write_index: Some(2),
                        end_of_pass_write_index: Some(3),
                    }
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &prims, &screen);
        }
        if let Some(t) = &self.timestamps {
            encoder.resolve_query_set(&t.set, 0..4, &t.resolve, 0);
            encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.readback, 0, 32);
        }
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        (encoder, tess_ms)
    }
}
