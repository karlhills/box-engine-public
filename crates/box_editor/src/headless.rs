//! Headless capture of the whole editor UI: a fixed-size `RawInput`, the
//! shared paint path into an offscreen `Rgba8Unorm` texture, and readback.
//! The basis for editor golden tests and `box-editor --capture`.

use anyhow::{Context as _, Result, bail};
use box_core::prelude::{Entity, Primitive};
use box_render::{Renderer, wgpu};
use egui::{Pos2, RawInput, Rect, ViewportId, ViewportInfo, vec2};

use crate::editor::Editor;
use crate::gfx::EditorGfx;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const FIXED_DT: f64 = 1.0 / 60.0;

#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions {
    /// Output size in physical pixels.
    pub size: [u32; 2],
    pub pixels_per_point: f32,
    /// Frames to run before the captured one (layout settles in 2-4).
    pub frames: u32,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            size: [2560, 1440],
            pixels_per_point: 2.0,
            frames: 4,
        }
    }
}

/// Selects a mesh entity that has a parent (the first in scene order), so a
/// capture shows tree depth, the inspector, the selection bounds and the gizmo.
pub fn select_sample_entity(editor: &mut Editor) {
    let world = &editor.state.world;
    let mut stack: Vec<Entity> = world.roots().iter().rev().copied().collect();
    let mut pick = None;
    while let Some(e) = stack.pop() {
        if world.parent(e).is_some() && world.has::<Primitive>(e) {
            pick = world.uid(e);
            break;
        }
        stack.extend(world.children(e).into_iter().rev());
    }
    editor.state.select(pick);
}

fn raw_input(options: &CaptureOptions, frame: u32) -> RawInput {
    let ppp = options.pixels_per_point;
    let points = vec2(options.size[0] as f32 / ppp, options.size[1] as f32 / ppp);
    let screen = Rect::from_min_size(Pos2::ZERO, points);
    let mut raw = RawInput {
        screen_rect: Some(screen),
        time: Some(frame as f64 * FIXED_DT),
        predicted_dt: FIXED_DT as f32,
        focused: true,
        ..Default::default()
    };
    raw.viewports.insert(
        ViewportId::ROOT,
        ViewportInfo {
            native_pixels_per_point: Some(ppp),
            focused: Some(true),
            inner_rect: Some(screen),
            ..Default::default()
        },
    );
    raw
}

/// CPU time of one headless frame, in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    /// `Context::run_ui`: every pane's UI code (hierarchy, inspector, ...).
    pub ui_ms: f32,
    /// Tessellation, scene extraction and encoding, egui encoding.
    pub render_ms: f32,
    /// Everything including waiting for the GPU.
    pub total_ms: f32,
    /// The scene was rendered into the viewport target this frame (it is
    /// not while the viewport pane is hidden).
    pub scene_rendered: bool,
}

/// A headless editor: an offscreen target on the renderer's device, driven
/// one frame at a time with a fixed 1/60 s clock.
pub struct Headless {
    renderer: Renderer,
    ctx: egui::Context,
    gfx: EditorGfx,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    options: CaptureOptions,
    frame: u32,
    /// The last frame's accessibility tree, after [`Self::enable_accesskit`].
    accesskit: Option<egui::accesskit::TreeUpdate>,
}

impl Headless {
    pub fn new(editor: &mut Editor, options: &CaptureOptions) -> Result<Self> {
        let [w, h] = options.size;
        anyhow::ensure!(w > 0 && h > 0, "capture size must be non-zero");
        let renderer =
            Renderer::new_headless(w, h).context("could not create a headless renderer")?;
        box_engine::crash::set_gpu(renderer.adapter_info());
        let texture = renderer.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("editor capture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        // Thumbnails are textures of the previous context.
        editor.state.assets.thumbs.clear();
        let gfx = EditorGfx::new(&renderer, FORMAT, 0);
        editor.state.status.fps = (1.0 / FIXED_DT) as f32;
        Ok(Self {
            renderer,
            ctx,
            gfx,
            texture,
            view,
            options: *options,
            frame: 0,
            accesskit: None,
        })
    }

    /// Records each frame's accessibility tree (widget labels, values and
    /// rectangles in points), so tests can find widgets by their text.
    pub fn enable_accesskit(&self) {
        self.ctx.enable_accesskit();
    }

    /// The last frame's accessibility tree (see [`Self::enable_accesskit`]).
    pub fn accesskit_update(&self) -> Option<&egui::accesskit::TreeUpdate> {
        self.accesskit.as_ref()
    }

    /// One frame: agent control requests, UI, scene render, egui paint,
    /// submit and wait for the GPU.
    pub fn frame(&mut self, editor: &mut Editor, events: Vec<egui::Event>) -> FrameTiming {
        let start = std::time::Instant::now();
        editor.poll_control(Some(&mut self.renderer));
        editor
            .state
            .console
            .log
            .set_manual_time(self.frame as f64 * FIXED_DT);
        let mut raw = raw_input(&self.options, self.frame);
        raw.events = events;
        let mut output = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.accesskit = output.platform_output.accesskit_update.take();
        let ui_ms = start.elapsed().as_secs_f32() * 1000.0;
        let r0 = std::time::Instant::now();
        let encoder = self.gfx.frame(
            &mut self.renderer,
            &self.ctx,
            editor,
            output,
            &self.view,
            self.options.size,
        );
        let index = self.renderer.queue().submit([encoder.finish()]);
        let render_ms = r0.elapsed().as_secs_f32() * 1000.0;
        let _ = self.renderer.device().poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        });
        self.frame += 1;
        FrameTiming {
            ui_ms,
            render_ms,
            total_ms: start.elapsed().as_secs_f32() * 1000.0,
            scene_rendered: self.gfx.scene_rendered,
        }
    }

    /// The renderer (for statistics and GPU cache checks).
    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    /// The last frame's pixels. Fails if wgpu reported validation errors.
    pub fn image(&self) -> Result<image::RgbaImage> {
        let errors = self.renderer.validation_errors();
        if errors > 0 {
            bail!("{errors} GPU validation error(s) in the headless editor");
        }
        read_texture(&self.renderer, &self.texture, self.options.size)
    }
}

/// Runs the editor for `options.frames` frames at a fixed 1/60 s step and
/// returns the last frame. Log records are stamped with the frame time.
/// Fails if wgpu reported validation errors.
///
/// With a project open, every import the frames started (the asset browser
/// imports the files it shows) is finished before the next frame, so
/// thumbnails and states are deterministic.
pub fn capture(editor: &mut Editor, options: &CaptureOptions) -> Result<image::RgbaImage> {
    editor.state.console.log.set_manual_time(0.0);
    let mut h = Headless::new(editor, options)?;
    editor.state.wait_assets_idle(ASSET_IDLE_TIMEOUT);
    for _ in 0..options.frames.max(1) {
        h.frame(editor, Vec::new());
        editor.state.wait_assets_idle(ASSET_IDLE_TIMEOUT);
    }
    h.image()
}

/// How long a capture waits for imports between frames.
const ASSET_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn read_texture(
    renderer: &Renderer,
    texture: &wgpu::Texture,
    [w, h]: [u32; 2],
) -> Result<image::RgbaImage> {
    let device = renderer.device();
    let padded = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("editor capture readback"),
        size: padded as u64 * h as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
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
    renderer.queue().submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device
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
    readback.unmap();
    image::RgbaImage::from_raw(w, h, pixels).context("readback size mismatch")
}
