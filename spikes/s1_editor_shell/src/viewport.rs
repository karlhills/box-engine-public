//! Offscreen viewport: our own wgpu pass renders into an `Rgba8Unorm` texture that is
//! registered with `egui_wgpu::Renderer` as a native texture.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    res: [f32; 2],
    time: f32,
    pixel_angle: f32,
    cam_pos: [f32; 4],
    cam_fwd: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    accent: [f32; 4],
}

pub struct Viewport {
    pipeline: wgpu::RenderPipeline,
    ubuf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub size: [u32; 2],
    pub texture_id: Option<egui::TextureId>,
}

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Viewport {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: ubuf.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            ubuf,
            bind_group,
            texture: None,
            size: [0, 0],
            texture_id: None,
        }
    }

    /// (Re)creates the target when the requested pixel size changes and re-points the egui
    /// texture id at it. Returns true if a reallocation happened.
    pub fn ensure_size(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        size: [u32; 2],
    ) -> bool {
        let size = [size[0].clamp(16, 8192), size[1].clamp(16, 8192)];
        if self.texture.is_some() && size == self.size {
            return false;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport target"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        match self.texture_id {
            Some(id) => renderer.update_egui_texture_from_wgpu_texture(
                device,
                &view,
                wgpu::FilterMode::Linear,
                id,
            ),
            None => {
                self.texture_id =
                    Some(renderer.register_native_texture(device, &view, wgpu::FilterMode::Linear))
            }
        }
        self.texture = Some((texture, view));
        self.size = size;
        true
    }

    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        time: f32,
        yaw: f32,
        pitch: f32,
        outline: bool,
        timestamps: Option<&wgpu::QuerySet>,
    ) {
        let Some((_, view)) = &self.texture else {
            return;
        };
        let target = [0.3f32, 1.2, 0.6];
        let dist = 15.0;
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let pos = [
            target[0] + dist * cp * sy,
            target[1] + dist * sp,
            target[2] + dist * cp * cy,
        ];
        let fwd = norm([target[0] - pos[0], target[1] - pos[1], target[2] - pos[2]]);
        let right = norm(cross(fwd, [0.0, 1.0, 0.0]));
        let up = cross(right, fwd);
        let fov_y = 38f32.to_radians();
        let pixel_angle = 2.0 * (fov_y * 0.5).tan() / self.size[1] as f32;
        let a = crate::theme::tokens().accent;
        let u = Uniforms {
            res: [self.size[0] as f32, self.size[1] as f32],
            time,
            pixel_angle,
            cam_pos: [pos[0], pos[1], pos[2], 0.0],
            cam_fwd: [fwd[0], fwd[1], fwd[2], 0.0],
            cam_right: [right[0], right[1], right[2], 0.0],
            cam_up: [up[0], up[1], up[2], 0.0],
            accent: [
                a.r() as f32 / 255.0,
                a.g() as f32 / 255.0,
                a.b() as f32 / 255.0,
                if outline { 1.0 } else { 0.0 },
            ],
        };
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps.map(|query_set| wgpu::RenderPassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            }),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}
