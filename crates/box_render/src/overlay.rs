//! Screen-space overlay: 2D triangles drawn over the tone-mapped image,
//! after the display lines, in every frame target (window, capture,
//! [`ViewportTarget`](crate::ViewportTarget)). Game UI (`box_ui`) writes the
//! [`Overlay`] resource; the renderer re-uploads geometry only when its
//! [revision](Overlay::revision) changes and applies texture updates once.
//!
//! Positions are physical pixels of the target (the world's `WindowSize`),
//! colours sRGB-encoded and premultiplied. The pass draws into a plain
//! (non-sRGB) view of the output where there is one, so blending happens
//! in sRGB space as UI colours are designed (translucent panels and text
//! anti-aliasing look as in CSS or egui); on an sRGB-only surface it
//! converts to linear and blends there. Two texture kinds:
//! [`OverlayTexture::Managed`] (textures the overlay's owner uploads through
//! [`Overlay::texture_updates`], such as a font atlas; premultiplied sRGB
//! values) and [`OverlayTexture::Image`] (an `Assets<ImageData>` entry the
//! renderer already uploaded; straight alpha).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use box_core::world::World;
use bytemuck::{Pod, Zeroable};

use crate::textures::TextureCache;

/// One overlay vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct OverlayVertex {
    /// Physical pixels from the target's top-left corner.
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    /// sRGB-encoded, premultiplied RGBA.
    pub color: [u8; 4],
}

/// The texture a batch samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayTexture {
    /// A texture uploaded through [`Overlay::texture_updates`] under this id
    /// (allocate ids with [`Overlay::new_texture_id`]).
    Managed(u64),
    /// An `Assets<ImageData>` entry, by handle id. Skipped until uploaded.
    Image(u64),
}

/// A run of indices drawn with one texture and scissor rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayBatch {
    pub texture: OverlayTexture,
    /// Scissor rectangle in physical pixels: x, y, width, height (clamped
    /// to the target).
    pub clip: [u32; 4],
    pub first_index: u32,
    pub index_count: u32,
}

/// A change to a managed texture: RGBA8 premultiplied sRGB-encoded pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayTextureUpdate {
    pub id: u64,
    /// `None` replaces the whole texture (it is created or resized to
    /// `size`); `Some` writes a patch at this offset of the existing one.
    pub pos: Option<[u32; 2]>,
    pub size: [u32; 2],
    pub rgba: Vec<u8>,
    /// Bilinear filtering (fonts); otherwise nearest.
    pub linear: bool,
}

/// Resource: the overlay to draw this frame. Retained: it is drawn every
/// frame until replaced.
#[derive(Debug, Default)]
pub struct Overlay {
    vertices: Vec<OverlayVertex>,
    indices: Vec<u32>,
    batches: Vec<OverlayBatch>,
    /// Pending managed texture changes, applied in order by the next frame
    /// the renderer draws (then cleared).
    pub texture_updates: Vec<OverlayTextureUpdate>,
    /// The managed textures the owner still uses (a font atlas also while
    /// no text is shown). The renderer releases every other managed texture
    /// it holds when it draws this overlay, so atlases of earlier UIs (a
    /// previous Play session, a rebuilt font set) do not pile up.
    pub live_textures: Vec<u64>,
    revision: u64,
}

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
static NEXT_TEXTURE: AtomicU64 = AtomicU64::new(1);

impl Overlay {
    /// A managed texture id unique in this process.
    pub fn new_texture_id() -> u64 {
        NEXT_TEXTURE.fetch_add(1, Ordering::Relaxed)
    }

    /// Replaces the geometry. Indices of a batch are relative to the whole
    /// vertex list.
    pub fn set_geometry(
        &mut self,
        vertices: Vec<OverlayVertex>,
        indices: Vec<u32>,
        batches: Vec<OverlayBatch>,
    ) {
        self.vertices = vertices;
        self.indices = indices;
        self.batches = batches;
        // Unique across worlds, so a renderer drawing several worlds never
        // mistakes one world's geometry for another's.
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    }

    /// Removes the geometry (texture updates stay pending).
    pub fn clear(&mut self) {
        if !self.batches.is_empty() {
            self.set_geometry(Vec::new(), Vec::new(), Vec::new());
        }
    }

    pub fn vertices(&self) -> &[OverlayVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn batches(&self) -> &[OverlayBatch] {
        &self.batches
    }

    /// Changes whenever the geometry is replaced; unique per process.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ScreenUniform {
    size: [f32; 2],
    _pad: [f32; 2],
}

struct ManagedTexture {
    texture: wgpu::Texture,
    size: [u32; 2],
    bind_group: wgpu::BindGroup,
}

/// One recorded draw.
#[derive(Clone, Copy, Debug)]
struct Draw {
    texture: OverlayTexture,
    clip: [u32; 4],
    first_index: u32,
    index_count: u32,
}

/// The overlay's GPU side, owned by the renderer.
pub(crate) struct OverlayPass {
    layout: wgpu::BindGroupLayout,
    managed_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    image_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    screen_buffer: wgpu::Buffer,
    screen_group: wgpu::BindGroup,
    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    managed: HashMap<u64, ManagedTexture>,
    /// Image bind groups, rebuilt when the texture cache changes.
    images: HashMap<u64, wgpu::BindGroup>,
    images_generation: u64,
    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    draws: Vec<Draw>,
    revision: u64,
    target: (u32, u32),
}

impl OverlayPass {
    pub(crate) fn new(device: &wgpu::Device, output_formats: &[wgpu::TextureFormat]) -> Self {
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay screen layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay screen"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay screen"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/overlay.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay pipeline layout"),
            bind_group_layouts: &[Some(&screen_layout), Some(&layout)],
            immediate_size: 0,
        });
        let vertex_attributes = wgpu::vertex_attr_array![
            0 => Float32x2,
            1 => Float32x2,
            2 => Unorm8x4,
        ];
        let mut formats = output_formats.to_vec();
        formats.sort_by_key(|f| format!("{f:?}"));
        formats.dedup();
        // sRGB views get colours converted to linear (blending then happens
        // in linear space); plain views get the sRGB values as they are, so
        // blending happens in sRGB space as UI is designed (CSS, egui).
        let pipelines = |kind: &str| -> HashMap<_, _> {
            formats
                .iter()
                .map(|&format| {
                    let space = if format.is_srgb() { "linear" } else { "gamma" };
                    let entry = format!("fs_{kind}_{space}");
                    let entry = entry.as_str();
                    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&format!("overlay {entry} ({format:?})")),
                        layout: Some(&pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: &module,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &[Some(wgpu::VertexBufferLayout {
                                array_stride: std::mem::size_of::<OverlayVertex>() as u64,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &vertex_attributes,
                            })],
                        },
                        primitive: wgpu::PrimitiveState::default(),
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &module,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        multiview_mask: None,
                        cache: None,
                    });
                    (format, pipeline)
                })
                .collect()
        };
        let sampler = |filter: wgpu::FilterMode, label: &str| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        };
        Self {
            managed_pipelines: pipelines("managed"),
            image_pipelines: pipelines("image"),
            layout,
            screen_buffer,
            screen_group,
            linear_sampler: sampler(wgpu::FilterMode::Linear, "overlay linear"),
            nearest_sampler: sampler(wgpu::FilterMode::Nearest, "overlay nearest"),
            managed: HashMap::new(),
            images: HashMap::new(),
            images_generation: u64::MAX,
            vertex_buffer: None,
            index_buffer: None,
            draws: Vec::new(),
            revision: 0,
            target: (0, 0),
        }
    }

    /// Applies texture updates and, when the overlay changed, uploads its
    /// geometry. Idle cost without an overlay: one resource lookup.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world: &mut World,
        textures: &TextureCache,
        size: (u32, u32),
    ) {
        let Some(overlay) = world.get_resource_mut::<Overlay>() else {
            self.draws.clear();
            self.revision = 0;
            return;
        };
        for update in overlay.texture_updates.drain(..) {
            self.apply_texture_update(device, queue, update);
        }
        if self.managed.len() > overlay.live_textures.len() {
            let live = &overlay.live_textures;
            self.managed.retain(|id, _| live.contains(id));
        }
        if textures.generation() != self.images_generation {
            self.images.clear();
            self.images_generation = textures.generation();
        }
        if size != self.target {
            self.target = size;
            queue.write_buffer(
                &self.screen_buffer,
                0,
                bytemuck::bytes_of(&ScreenUniform {
                    size: [size.0 as f32, size.1 as f32],
                    _pad: [0.0; 2],
                }),
            );
        }
        // Image bind groups for this frame's batches (views appear once
        // the texture cache uploaded the image).
        for b in overlay.batches() {
            if let OverlayTexture::Image(id) = b.texture
                && !self.images.contains_key(&id)
                && let Some(view) = textures.view(id)
            {
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("overlay image"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                    ],
                });
                self.images.insert(id, group);
            }
        }
        if overlay.revision() == self.revision {
            return;
        }
        self.revision = overlay.revision();
        self.draws.clear();
        if overlay.is_empty() {
            return;
        }
        let vertices = bytemuck::cast_slice(overlay.vertices());
        let indices = bytemuck::cast_slice(overlay.indices());
        write_growing(
            device,
            queue,
            &mut self.vertex_buffer,
            vertices,
            wgpu::BufferUsages::VERTEX,
            "overlay vertices",
        );
        write_growing(
            device,
            queue,
            &mut self.index_buffer,
            indices,
            wgpu::BufferUsages::INDEX,
            "overlay indices",
        );
        self.draws.extend(overlay.batches().iter().map(|b| Draw {
            texture: b.texture,
            clip: b.clip,
            first_index: b.first_index,
            index_count: b.index_count,
        }));
    }

    fn apply_texture_update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        u: OverlayTextureUpdate,
    ) {
        let [w, h] = u.size;
        if w == 0 || h == 0 || u.rgba.len() != (w * h * 4) as usize {
            tracing::warn!(target: "box_render", "overlay texture {}: bad update ({w}x{h}, {} bytes)", u.id, u.rgba.len());
            return;
        }
        let origin = match u.pos {
            None => {
                let recreate = self.managed.get(&u.id).is_none_or(|t| t.size != u.size);
                if recreate {
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("overlay texture"),
                        size: wgpu::Extent3d {
                            width: w,
                            height: h,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&Default::default());
                    let sampler = if u.linear {
                        &self.linear_sampler
                    } else {
                        &self.nearest_sampler
                    };
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("overlay texture"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(sampler),
                            },
                        ],
                    });
                    self.managed.insert(
                        u.id,
                        ManagedTexture {
                            texture,
                            size: u.size,
                            bind_group,
                        },
                    );
                }
                [0, 0]
            }
            Some(pos) => pos,
        };
        let Some(t) = self.managed.get(&u.id) else {
            tracing::warn!(target: "box_render", "overlay texture {}: patch before the texture was created", u.id);
            return;
        };
        if origin[0] + w > t.size[0] || origin[1] + h > t.size[1] {
            tracing::warn!(target: "box_render", "overlay texture {}: patch outside the texture", u.id);
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin[0],
                    y: origin[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &u.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// True when the next [`encode`](Self::encode) draws something.
    pub(crate) fn has_draws(&self) -> bool {
        !self.draws.is_empty()
    }

    /// Draws the overlay into `output` (loaded, not cleared). Returns the
    /// number of draw calls.
    pub(crate) fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) -> u32 {
        let (Some(vb), Some(ib)) = (&self.vertex_buffer, &self.index_buffer) else {
            return 0;
        };
        if self.draws.is_empty() {
            return 0;
        }
        let (Some(managed_pipeline), Some(image_pipeline)) = (
            self.managed_pipelines.get(&format),
            self.image_pipelines.get(&format),
        ) else {
            return 0;
        };
        profiling::scope!("overlay pass");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay pass"),
            timestamp_writes,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_bind_group(0, &self.screen_group, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
        let (tw, th) = self.target;
        let mut draws = 0;
        let mut current: Option<bool> = None;
        for d in &self.draws {
            let (group, is_image) = match d.texture {
                OverlayTexture::Managed(id) => match self.managed.get(&id) {
                    Some(t) => (&t.bind_group, false),
                    None => continue,
                },
                OverlayTexture::Image(id) => match self.images.get(&id) {
                    Some(g) => (g, true),
                    None => continue,
                },
            };
            let [x, y, w, h] = d.clip;
            let x0 = x.min(tw);
            let y0 = y.min(th);
            let x1 = x.saturating_add(w).min(tw);
            let y1 = y.saturating_add(h).min(th);
            if x1 <= x0 || y1 <= y0 || d.index_count == 0 {
                continue;
            }
            if current != Some(is_image) {
                pass.set_pipeline(if is_image {
                    image_pipeline
                } else {
                    managed_pipeline
                });
                current = Some(is_image);
            }
            pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
            pass.set_bind_group(1, group, &[]);
            pass.draw_indexed(d.first_index..d.first_index + d.index_count, 0, 0..1);
            draws += 1;
        }
        draws
    }
}

/// Writes `bytes` into `buffer`, recreating it (doubled) when too small.
fn write_growing(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut Option<wgpu::Buffer>,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
    label: &str,
) {
    let needed = (bytes.len() as u64)
        .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT)
        .max(256);
    if buffer.as_ref().is_none_or(|b| b.size() < needed) {
        let size = needed.next_power_of_two();
        *buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }
    if let Some(b) = buffer {
        // write_buffer needs a multiple of 4 bytes.
        if bytes.len().is_multiple_of(4) {
            queue.write_buffer(b, 0, bytes);
        } else {
            let mut padded = bytes.to_vec();
            padded.resize(bytes.len().next_multiple_of(4), 0);
            queue.write_buffer(b, 0, &padded);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Renderer;

    fn atlas(id: u64) -> OverlayTextureUpdate {
        OverlayTextureUpdate {
            id,
            pos: None,
            size: [4, 4],
            rgba: vec![255; 64],
            linear: true,
        }
    }

    #[test]
    fn textures_the_overlay_no_longer_uses_are_released() {
        if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
            return;
        }
        let Ok(r) = Renderer::new_headless(8, 8) else {
            return;
        };
        let (device, queue) = (r.device(), r.queue());
        let textures = TextureCache::new(device, queue);
        let mut pass = OverlayPass::new(device, &[wgpu::TextureFormat::Rgba8Unorm]);
        let mut world = World::default();
        // A first UI's atlas, then a second UI (a new Play session, a new
        // font set) with its own.
        let (a, b) = (Overlay::new_texture_id(), Overlay::new_texture_id());
        let mut o = Overlay::default();
        o.texture_updates.push(atlas(a));
        o.live_textures = vec![a];
        world.insert_resource(o);
        pass.prepare(device, queue, &mut world, &textures, (8, 8));
        assert_eq!(pass.managed.len(), 1);
        let o = world.resource_mut::<Overlay>();
        o.texture_updates.push(atlas(b));
        o.live_textures = vec![b];
        pass.prepare(device, queue, &mut world, &textures, (8, 8));
        assert_eq!(pass.managed.keys().copied().collect::<Vec<_>>(), [b]);
        // A patch outside the texture is refused, not written.
        world
            .resource_mut::<Overlay>()
            .texture_updates
            .push(OverlayTextureUpdate {
                pos: Some([2, 2]),
                ..atlas(b)
            });
        pass.prepare(device, queue, &mut world, &textures, (8, 8));
        assert_eq!(r.validation_errors(), 0);
    }
}
