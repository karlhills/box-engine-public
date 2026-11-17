//! Drawing particles: each visible emitter's live particles become
//! instances (position, size, colour, atlas frame, rotation) in one
//! per-frame vertex buffer, sorted back to front within alpha-blended
//! emitters, and are drawn as instanced camera-facing quads
//! (particles.wgsl) in the transparent pass, merged with transparent meshes
//! by depth.

use std::collections::HashMap;

use box_core::assets::AssetLookup;
use box_core::image::{ColorSpace, ImageData};
use box_core::world::World;
use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Vec4};

use crate::culling::Frustum;
use crate::gpu_mesh::UploadStats;
use crate::particles::{ParticleBlend, ParticleEmitter, ParticleSpace, ParticleState};
use crate::pipelines::Layouts;
use crate::textures::TextureCache;

/// One particle instance (`ParticleInput` in particles.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuParticle {
    pub position: [f32; 3],
    /// Quad width (m).
    pub size: f32,
    /// Linear colour times intensity, alpha.
    pub color: [f32; 4],
    /// Atlas frame: uv offset xy, uv scale zw.
    pub uv: [f32; 4],
    /// x = rotation (rad), y = soft distance (m), z = 1 when lit.
    pub params: [f32; 4],
}

pub(crate) const PARTICLE_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x4,
    1 => Float32x4,
    2 => Float32x4,
    3 => Float32x4,
];

const _: () = assert!(std::mem::size_of::<GpuParticle>() == 64);

/// Entries of the size and colour tables built per emitter per frame.
const LUT: usize = 64;

/// One emitter's instanced draw this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ParticleDraw {
    pub first: u32,
    pub count: u32,
    pub additive: bool,
    /// Texture bind group key (image asset id; 0 the built-in dot).
    pub texture: u64,
    /// View depth of the emitter's bounds centre (merged with transparent
    /// meshes back to front).
    pub depth: f32,
}

/// What [`ParticleRenderer::prepare`] found.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ParticleFrame {
    /// A visible emitter fades softly (needs the prepass depth).
    pub soft: bool,
    pub particles: u32,
    pub emitters: u32,
}

pub(crate) struct ParticleRenderer {
    buffer: Option<wgpu::Buffer>,
    instances: Vec<GpuParticle>,
    pub draws: Vec<ParticleDraw>,
    /// Sort scratch: (view depth, particle index).
    order: Vec<(f32, u32)>,
    _dot: wgpu::Texture,
    dot_view: wgpu::TextureView,
    /// Texture bind groups by image id (0 the dot) with the texture cache
    /// generation they were made at.
    bind_groups: HashMap<u64, (u64, wgpu::BindGroup)>,
}

/// A soft round dot: white, alpha falling smoothly from the centre.
fn dot_image() -> ImageData {
    const N: u32 = 64;
    let mut pixels = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let p = (glam::Vec2::new(x as f32, y as f32) + 0.5) / N as f32 * 2.0 - 1.0;
            let a = (1.0 - p.length_squared()).clamp(0.0, 1.0);
            pixels.extend_from_slice(&[255, 255, 255, (a * a * 255.0).round() as u8]);
        }
    }
    let mut image =
        ImageData::new(N, N, ColorSpace::Srgb, pixels).expect("valid built-in particle image");
    image.generate_mips();
    image
}

impl ParticleRenderer {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let image = dot_image();
        let dot = crate::textures::create_texture(device, "particle dot", &image);
        crate::textures::write_levels(queue, &dot, &image);
        let dot_view = dot.create_view(&Default::default());
        Self {
            buffer: None,
            instances: Vec::new(),
            draws: Vec::new(),
            order: Vec::new(),
            _dot: dot,
            dot_view,
            bind_groups: HashMap::new(),
        }
    }

    pub(crate) fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref()
    }

    pub(crate) fn bind_group(&self, texture: u64) -> Option<&wgpu::BindGroup> {
        self.bind_groups.get(&texture).map(|(_, g)| g)
    }

    /// Builds this frame's instances and draws from every visible emitter
    /// with live particles, uploads them, and makes missing texture bind
    /// groups.
    #[allow(clippy::too_many_arguments)]
    #[profiling::function]
    pub(crate) fn prepare(
        &mut self,
        world: &World,
        eye: Vec3,
        forward: Vec3,
        frustum: &Frustum,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &Layouts,
        textures: &TextureCache,
        upload: &mut UploadStats,
    ) -> ParticleFrame {
        self.instances.clear();
        self.draws.clear();
        let mut frame = ParticleFrame::default();
        let lookup = world.get_resource::<AssetLookup>();
        let generation = textures.generation();
        for (e, s) in world.query::<(&ParticleEmitter, &ParticleState)>().iter() {
            let bounds = s.bounds();
            if s.is_empty() || !frustum.intersects_aabb(&bounds) {
                continue;
            }
            let texture = e
                .texture
                .is_some()
                .then(|| lookup?.get::<ImageData>(&e.texture))
                .flatten()
                .map(|h| h.id())
                .filter(|id| textures.view(*id).is_some())
                .unwrap_or(0);
            let first = self.instances.len() as u32;
            self.write_emitter(e, s, eye, forward);
            let count = self.instances.len() as u32 - first;
            if count == 0 {
                continue;
            }
            frame.soft |= e.soft_distance > 0.0;
            frame.particles += count;
            frame.emitters += 1;
            self.draws.push(ParticleDraw {
                first,
                count,
                additive: e.blend == ParticleBlend::Additive,
                texture,
                depth: (bounds.center() - eye).dot(forward),
            });
            let stale = self
                .bind_groups
                .get(&texture)
                .is_none_or(|(g, _)| *g != generation);
            if stale {
                let view = if texture == 0 {
                    &self.dot_view
                } else {
                    textures.view(texture).expect("checked above")
                };
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("particle texture bind group"),
                    layout: &layouts.particle,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&textures.sampler),
                        },
                    ],
                });
                self.bind_groups.insert(texture, (generation, group));
            }
        }
        // Back to front with the transparent meshes (stable: equal depths
        // keep their order).
        self.draws.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        if self.bind_groups.len() > 2 * self.draws.len() + 16 {
            let live: std::collections::HashSet<u64> =
                self.draws.iter().map(|d| d.texture).collect();
            self.bind_groups.retain(|k, _| *k == 0 || live.contains(k));
        }
        if !self.instances.is_empty() {
            let bytes = bytemuck::cast_slice::<GpuParticle, u8>(&self.instances);
            let current = self.buffer.as_ref().map_or(0, wgpu::Buffer::size);
            if bytes.len() as u64 > current {
                let size = (current * 2)
                    .max(bytes.len() as u64)
                    .max(64 * 1024)
                    .next_multiple_of(256);
                self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("particle instances"),
                    size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                if current > 0 {
                    upload.reallocations += 1;
                }
            }
            let buffer = self.buffer.as_ref().expect("particle buffer");
            queue.write_buffer(buffer, 0, bytes);
            upload.bytes += bytes.len() as u64;
        }
        frame
    }

    /// Appends one emitter's particles (back to front when alpha blended).
    fn write_emitter(&mut self, e: &ParticleEmitter, s: &ParticleState, eye: Vec3, forward: Vec3) {
        let sizes: [f32; LUT] = std::array::from_fn(|i| e.size_at(i as f32 / (LUT - 1) as f32));
        let intensity = e.intensity.max(0.0);
        let colors: [Vec4; LUT] = std::array::from_fn(|i| {
            let c = e.color_at(i as f32 / (LUT - 1) as f32);
            (c.truncate() * intensity).extend(c.w)
        });
        let columns = e.atlas.x.max(1);
        let rows = e.atlas.y.max(1);
        let frames = columns * rows;
        let scale = [1.0 / columns as f32, 1.0 / rows as f32];
        let params = [
            0.0,
            e.soft_distance.max(0.0),
            if e.lit { 1.0 } else { 0.0 },
            0.0,
        ];
        let local = e.space == ParticleSpace::Local;
        let transform = *s.transform();
        let particles = s.particles();
        let world_pos = |i: usize| {
            let p = particles[i].position;
            if local {
                transform.transform_point3(p)
            } else {
                p
            }
        };
        self.order.clear();
        if e.blend == ParticleBlend::Alpha {
            self.order.extend(
                (0..particles.len()).map(|i| ((world_pos(i) - eye).dot(forward), i as u32)),
            );
            self.order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        } else {
            self.order
                .extend((0..particles.len()).map(|i| (0.0, i as u32)));
        }
        for &(_, i) in &self.order {
            let p = &particles[i as usize];
            let t = (p.age / p.lifetime).clamp(0.0, 1.0);
            let k = (t * (LUT - 1) as f32).round() as usize;
            let size = sizes[k] * p.size_scale;
            let color = colors[k];
            if size <= 0.0 || color.w <= 0.0 {
                continue;
            }
            let frame = ((t * frames as f32) as u32).min(frames - 1);
            let uv = [
                (frame % columns) as f32 * scale[0],
                (frame / columns) as f32 * scale[1],
                scale[0],
                scale[1],
            ];
            let mut params = params;
            params[0] = p.rotation;
            self.instances.push(GpuParticle {
                position: world_pos(i as usize).to_array(),
                size,
                color: color.to_array(),
                uv,
                params,
            });
        }
    }
}
