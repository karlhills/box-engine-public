//! The per-frame GPU path shared by the window and headless capture:
//! scene into the viewport target (box_render), then egui into the output.

use box_render::wgpu;
use box_render::{Renderer, ViewportTarget};

use crate::editor::Editor;

pub(crate) struct EditorGfx {
    egui: egui_wgpu::Renderer,
    target: Option<ViewportTarget>,
    /// Windowed runs wait until the requested size has been stable for this
    /// many frames before reallocating; headless runs use 0.
    debounce_frames: u32,
    pending: Option<([u32; 2], u32)>,
    clear: wgpu::Color,
    /// The last frame rendered the scene (the viewport pane was shown).
    pub scene_rendered: bool,
}

impl EditorGfx {
    pub fn new(
        renderer: &Renderer,
        output_format: wgpu::TextureFormat,
        debounce_frames: u32,
    ) -> Self {
        let bg = crate::theme::DARK.divider;
        Self {
            egui: egui_wgpu::Renderer::new(
                renderer.device(),
                output_format,
                egui_wgpu::RendererOptions::default(),
            ),
            target: None,
            scene_rendered: false,
            debounce_frames,
            pending: None,
            // The UI covers the whole target; the clear only shows for a
            // frame while a window is resized. Values are as stored in the
            // (non-sRGB) target.
            clear: wgpu::Color {
                r: bg.r() as f64 / 255.0,
                g: bg.g() as f64 / 255.0,
                b: bg.b() as f64 / 255.0,
                a: 1.0,
            },
        }
    }

    /// Creates or resizes the viewport target to the size the viewport pane
    /// asked for and points the egui texture at it.
    fn sync_target(&mut self, renderer: &Renderer, editor: &mut Editor) {
        let req = editor.state.viewport.request_px;
        if req[0] == 0 || req[1] == 0 {
            return;
        }
        let device = renderer.device();
        match &mut self.target {
            None => {
                let target = renderer.create_viewport_target(req[0], req[1]);
                let id = self.egui.register_native_texture(
                    device,
                    target.view(),
                    wgpu::FilterMode::Linear,
                );
                editor.state.viewport.texture = Some(id);
                self.target = Some(target);
                self.pending = None;
            }
            Some(target) => {
                let current = target.size();
                if [current.0, current.1] == req {
                    self.pending = None;
                } else {
                    let stable = match self.pending {
                        Some((size, n)) if size == req => n + 1,
                        _ => 0,
                    };
                    self.pending = Some((req, stable));
                    if stable >= self.debounce_frames
                        && renderer.resize_viewport_target(target, req[0], req[1])
                        && let Some(id) = editor.state.viewport.texture
                    {
                        self.egui.update_egui_texture_from_wgpu_texture(
                            device,
                            target.view(),
                            wgpu::FilterMode::Linear,
                            id,
                        );
                        self.pending = None;
                    }
                }
            }
        }
        if let Some(t) = &self.target {
            editor.state.viewport.target_px = [t.size().0, t.size().1];
        }
    }

    /// Renders the scene into the viewport target (submitted immediately)
    /// when the viewport pane was shown this frame (it is not while its tab
    /// is hidden), then records the egui pass into `output`. The caller submits the
    /// returned encoder and presents.
    pub fn frame(
        &mut self,
        renderer: &mut Renderer,
        ctx: &egui::Context,
        editor: &mut Editor,
        mut output: egui::FullOutput,
        target_view: &wgpu::TextureView,
        size_px: [u32; 2],
    ) -> wgpu::CommandEncoder {
        let ppp = output.pixels_per_point;
        let prims = ctx.tessellate(output.shapes, ppp);
        // Drained: egui asserts that every delta is applied.
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in &deltas {
                self.egui
                    .update_texture(renderer.device(), renderer.queue(), id, delta);
            }
        }

        // Nothing shows the scene while the viewport tab is hidden.
        let shown = std::mem::take(&mut editor.state.viewport.shown);
        if shown {
            self.sync_target(renderer, editor);
        } else {
            editor.state.status.draws = 0;
            editor.state.status.triangles = 0;
        }
        self.scene_rendered = false;
        if shown && let Some(target) = &self.target {
            self.scene_rendered = true;
            editor.prepare_render();
            // The editor camera, or during Play the game's own camera.
            let camera = editor.state.viewport_camera();
            renderer.render_to(&mut editor.state.world, target, camera.as_ref());
            let stats = renderer.stats();
            editor.state.status.draws = stats.draws;
            editor.state.status.triangles = stats.triangles;
        }
        // Compile errors of the shaders the inspector showed this frame.
        let queries = std::mem::take(&mut editor.state.assets.shader_queries);
        editor.state.assets.shader_errors = queries
            .iter()
            .filter_map(|h| renderer.shader_error(*h).map(|e| (h.id(), e.to_string())))
            .collect();

        let device = renderer.device();
        let queue = renderer.queue();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("editor ui"),
        });
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size_px,
            pixels_per_point: ppp,
        };
        let callbacks = self
            .egui
            .update_buffers(device, queue, &mut encoder, &prims, &screen);
        if !callbacks.is_empty() {
            queue.submit(callbacks);
        }
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.egui.render(&mut pass, &prims, &screen);
        }
        for id in output.textures_delta.free.drain() {
            self.egui.free_texture(&id);
        }
        encoder
    }
}
