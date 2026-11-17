//! Bind group layouts, the fixed render pipelines, and the material pipeline
//! cache keyed by [`PipelineKey`].

use std::collections::HashMap;
use std::num::NonZeroU64;

use box_core::material::{AlphaMode, DepthWrite};

use crate::shaders::ProcessedShaders;
use crate::uniforms::{
    GlobalsUniform, GpuVertex, LINE_ATTRIBUTES, LineVertex, MESH_ATTRIBUTES, ObjectUniform,
    StandardMaterialUniform,
};

pub(crate) const MSAA_SAMPLES: u32 = 4;
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub(crate) const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Selection outline mask: the per-object outline id of the nearest
/// selected surface ((object slot + 1) * 2 + primary bit).
pub(crate) const OUTLINE_MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// Size of the outline composite uniform (`Outline` in outline.wgsl).
pub(crate) const OUTLINE_UNIFORM_SIZE: u64 = 48;
/// Prepass normals (prepass.wgsl), full and half resolution.
pub(crate) const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgb10a2Unorm;
/// Half-resolution linear view depth for ambient occlusion.
pub(crate) const AO_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Ambient occlusion terms.
pub(crate) const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// Bind group and pipeline layouts; created once, shared by all pipeline
/// generations.
pub(crate) struct Layouts {
    /// Group 0 of scene passes: globals, shadow map, comparison sampler,
    /// point lights.
    pub frame: wgpu::BindGroupLayout,
    /// Group 1: instance data, the draw slot list (instances.rs) and the
    /// joint palettes (skinning.rs).
    pub object: wgpu::BindGroupLayout,
    /// Group 0 of the shadow pass: globals and the per-cascade light matrix
    /// (dynamic offset).
    pub shadow_pass: wgpu::BindGroupLayout,
    /// Group 2 of `StandardMaterial` draws: factors, four textures, sampler.
    pub standard_material: wgpu::BindGroupLayout,
    /// Group 2 of `ShaderMaterial` draws: the parameter uniform.
    pub custom_material: wgpu::BindGroupLayout,
    /// Group 1 of the tone-mapping pass: the resolved HDR texture, bloom
    /// mip 0 and a linear sampler.
    pub tonemap: wgpu::BindGroupLayout,
    /// Group 1 of the ambient occlusion resolve: the prepass depth and
    /// normals.
    pub ao_resolve: wgpu::BindGroupLayout,
    /// Group 1 of the occlusion pass: half-resolution depth and normals.
    pub ao_compute: wgpu::BindGroupLayout,
    /// Group 1 of the occlusion blur: half-resolution depth and raw AO.
    pub ao_blur: wgpu::BindGroupLayout,
    /// Group 1 of the bloom passes: the source mip and a linear sampler.
    pub bloom: wgpu::BindGroupLayout,
    /// Group 1 of display-referred lines: the multisampled scene depth.
    pub display_lines: wgpu::BindGroupLayout,
    /// Group 1 of the outline composite: mask, mask depth, scene depth and
    /// outline parameters.
    pub outline: wgpu::BindGroupLayout,
    /// Group 1 of particle draws: the particle texture and its sampler.
    pub particle: wgpu::BindGroupLayout,
    pub standard_main: wgpu::PipelineLayout,
    pub standard_shadow: wgpu::PipelineLayout,
    /// Shadows of alpha-masked or swaying standard materials.
    pub standard_shadow_mask: wgpu::PipelineLayout,
    /// Prepass and outline mask of alpha-masked or swaying standard
    /// materials (frame, object, material).
    pub standard_prepass_mask: wgpu::PipelineLayout,
    pub custom_main: wgpu::PipelineLayout,
    pub custom_shadow: wgpu::PipelineLayout,
    /// Frame and object groups only (error material, outline mask).
    pub frame_object: wgpu::PipelineLayout,
}

impl Layouts {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let uniform =
            |binding, visibility, dynamic: bool, size: usize| wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: dynamic,
                    min_binding_size: NonZeroU64::new(size as u64),
                },
                count: None,
            };
        let vf = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let globals_size = std::mem::size_of::<GlobalsUniform>();
        let frame = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame layout"),
            entries: &[
                uniform(0, vf, false, globals_size),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                // Vertex too: lit particles are lit per vertex.
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: vf,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        // The header plus one light (the smallest runtime-sized array).
                        min_binding_size: NonZeroU64::new(
                            crate::uniforms::LIGHTS_HEADER
                                + std::mem::size_of::<crate::uniforms::GpuLocalLight>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The prepass depth (soft transparency).
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // The shadowed local light's views.
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let storage = |binding, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size),
            },
            count: None,
        };
        let object = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("instance layout"),
            entries: &[
                storage(0, std::mem::size_of::<ObjectUniform>() as u64),
                storage(1, 4),
                // Joint palettes of skinned objects (skinning.rs).
                storage(2, 64),
            ],
        });
        // Binding 7 holds a placeholder scene depth: material shaders that
        // call `scene_depth` reference it in their masked shadow fragment.
        let shadow_pass = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow pass layout"),
            entries: &[
                uniform(0, vf, false, globals_size),
                uniform(4, wgpu::ShaderStages::VERTEX, true, 64),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let texture = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled,
            },
            count: None,
        };
        let filterable = wgpu::TextureSampleType::Float { filterable: true };
        let standard_material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("standard material layout"),
            entries: &[
                // Vertex too: wind sway.
                uniform(0, vf, false, std::mem::size_of::<StandardMaterialUniform>()),
                texture(1, filterable, false),
                texture(2, filterable, false),
                texture(3, filterable, false),
                texture(4, filterable, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // The parameter struct's size differs per shader; wgpu checks the
        // bound size against the shader at draw time.
        let custom_material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader material layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: vf,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let unfilterable = wgpu::TextureSampleType::Float { filterable: false };
        let linear_sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let tonemap = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap layout"),
            entries: &[
                texture(0, unfilterable, false),
                texture(1, filterable, false),
                linear_sampler(2),
            ],
        });
        let ao_resolve = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ao resolve layout"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Depth, false),
                texture(1, unfilterable, false),
            ],
        });
        let ao_compute = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ao layout"),
            entries: &[
                texture(0, unfilterable, false),
                texture(1, unfilterable, false),
            ],
        });
        let ao_blur = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ao blur layout"),
            entries: &[
                texture(0, unfilterable, false),
                texture(2, unfilterable, false),
            ],
        });
        let bloom = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom layout"),
            entries: &[texture(0, filterable, false), linear_sampler(1)],
        });
        let display_lines = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display lines layout"),
            entries: &[texture(0, wgpu::TextureSampleType::Depth, true)],
        });
        let outline = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline layout"),
            entries: &[
                texture(0, wgpu::TextureSampleType::Uint, false),
                texture(1, wgpu::TextureSampleType::Depth, false),
                texture(2, wgpu::TextureSampleType::Depth, true),
                uniform(
                    3,
                    wgpu::ShaderStages::FRAGMENT,
                    false,
                    OUTLINE_UNIFORM_SIZE as usize,
                ),
            ],
        });
        let particle = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle layout"),
            entries: &[texture(0, filterable, false), linear_sampler(1)],
        });
        let layout = |label: &str, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let standard_main = layout(
            "standard material pipeline layout",
            &[Some(&frame), Some(&object), Some(&standard_material)],
        );
        let standard_shadow = layout(
            "shadow pipeline layout",
            &[Some(&shadow_pass), Some(&object)],
        );
        let standard_shadow_mask = layout(
            "masked shadow pipeline layout",
            &[Some(&shadow_pass), Some(&object), Some(&standard_material)],
        );
        let standard_prepass_mask = layout(
            "masked prepass pipeline layout",
            &[Some(&frame), Some(&object), Some(&standard_material)],
        );
        let custom_main = layout(
            "shader material pipeline layout",
            &[Some(&frame), Some(&object), Some(&custom_material)],
        );
        let custom_shadow = layout(
            "shader material shadow pipeline layout",
            &[Some(&shadow_pass), Some(&object), Some(&custom_material)],
        );
        let frame_object = layout(
            "frame + object pipeline layout",
            &[Some(&frame), Some(&object)],
        );
        Self {
            frame,
            object,
            shadow_pass,
            standard_material,
            custom_material,
            tonemap,
            ao_resolve,
            ao_compute,
            ao_blur,
            bloom,
            display_lines,
            outline,
            particle,
            standard_main,
            standard_shadow,
            standard_shadow_mask,
            standard_prepass_mask,
            custom_main,
            custom_shadow,
            frame_object,
        }
    }
}

/// The outline mask pipelines: counter-clockwise front faces, clockwise
/// (mirrored transforms), and no culling (double-sided materials).
pub(crate) type OutlineMaskPipelines = [wgpu::RenderPipeline; 3];

/// One generation of the fixed pipelines and engine material modules,
/// rebuilt as a unit on shader hot reload.
pub(crate) struct Pipelines {
    pub sky: wgpu::RenderPipeline,
    pub debug_depth: wgpu::RenderPipeline,
    pub debug_overlay: wgpu::RenderPipeline,
    pub tonemap: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    /// Display-referred lines after tone mapping, per output format.
    pub display_depth: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    pub display_overlay: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    /// Selected meshes into the outline mask.
    pub outline_mask: OutlineMaskPipelines,
    /// Outline composite after tone mapping, per output format.
    pub outline: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    /// Ambient occlusion: prepass downsample to half resolution, occlusion,
    /// blur.
    pub ao_resolve: wgpu::RenderPipeline,
    pub ao: wgpu::RenderPipeline,
    pub ao_blur: wgpu::RenderPipeline,
    /// Bloom: first downsample (from the scene), downsample, upsample
    /// (blended with the blend constant).
    pub bloom_first: wgpu::RenderPipeline,
    pub bloom_down: wgpu::RenderPipeline,
    pub bloom_up: wgpu::RenderPipeline,
    /// Billboard particles: alpha (premultiplied) and additive blending.
    pub particle_alpha: wgpu::RenderPipeline,
    pub particle_additive: wgpu::RenderPipeline,
    /// Modules the [`PipelineCache`] builds material pipelines from.
    pub modules: EngineModules,
}

/// Engine shader modules behind material pipelines.
pub(crate) struct EngineModules {
    /// `StandardMaterial` forward pass (mesh.wgsl).
    pub standard: wgpu::ShaderModule,
    /// Shadow casters (shadow_depth.wgsl).
    pub shadow: wgpu::ShaderModule,
    /// The error material (error.wgsl).
    pub error: wgpu::ShaderModule,
    /// Skinned variants (RS4): mesh_skinned.wgsl, shadow_skinned.wgsl,
    /// error_skinned.wgsl.
    pub standard_skinned: wgpu::ShaderModule,
    pub shadow_skinned: wgpu::ShaderModule,
    pub error_skinned: wgpu::ShaderModule,
}

fn mesh_buffer() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GpuVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &MESH_ATTRIBUTES,
    }
}

fn mesh_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(mesh_buffer())]
}

/// The vertex buffers of `layout`: the mesh vertices, plus joints and
/// weights for skinned meshes.
fn vertex_buffers(layout: VertexLayout) -> Vec<Option<wgpu::VertexBufferLayout<'static>>> {
    match layout {
        VertexLayout::Mesh => vec![Some(mesh_buffer())],
        VertexLayout::Skinned => vec![Some(mesh_buffer()), Some(crate::skinning::skin_buffer())],
    }
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

impl Pipelines {
    /// Builds every fixed pipeline and the material modules. `output_formats`
    /// are the tone-mapping targets.
    #[profiling::function]
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        shaders: &ProcessedShaders,
        hdr_format: wgpu::TextureFormat,
        output_formats: &[wgpu::TextureFormat],
    ) -> Self {
        let module = |name: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(shaders.get(name).into()),
            })
        };
        let layout = |label: &str, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };

        let modules = EngineModules {
            standard: module("mesh.wgsl"),
            shadow: module("shadow_depth.wgsl"),
            error: module("error.wgsl"),
            standard_skinned: module("mesh_skinned.wgsl"),
            shadow_skinned: module("shadow_skinned.wgsl"),
            error_skinned: module("error_skinned.wgsl"),
        };
        let sky_module = module("sky.wgsl");
        let debug_module = module("debug_lines.wgsl");
        let tonemap_module = module("tonemap.wgsl");
        let display_module = module("display_lines.wgsl");
        let outline_mask_module = module("outline_mask.wgsl");
        let outline_module = module("outline.wgsl");
        let ao_resolve_module = module("ao_resolve.wgsl");
        let ao_module = module("ao.wgsl");
        let bloom_module = module("bloom.wgsl");
        let particle_module = module("particles.wgsl");

        let frame_only_layout = layout("frame-only pipeline layout", &[Some(&layouts.frame)]);
        let tonemap_layout = layout(
            "tonemap pipeline layout",
            &[Some(&layouts.frame), Some(&layouts.tonemap)],
        );

        let msaa = wgpu::MultisampleState {
            count: MSAA_SAMPLES,
            ..Default::default()
        };
        let hdr_target = [Some(wgpu::ColorTargetState {
            format: hdr_format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky pipeline"),
            layout: Some(&frame_only_layout),
            vertex: wgpu::VertexState {
                module: &sky_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            // Only where the depth buffer still holds the clear value (0).
            depth_stencil: Some(depth_state(false, wgpu::CompareFunction::GreaterEqual)),
            multisample: msaa,
            fragment: Some(wgpu::FragmentState {
                module: &sky_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &hdr_target,
            }),
            multiview_mask: None,
            cache: None,
        });

        let line_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &LINE_ATTRIBUTES,
        })];
        let debug_pipeline = |label: &str, compare| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&frame_only_layout),
                vertex: wgpu::VertexState {
                    module: &debug_module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &line_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    ..Default::default()
                },
                depth_stencil: Some(depth_state(false, compare)),
                multisample: msaa,
                fragment: Some(wgpu::FragmentState {
                    module: &debug_module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &hdr_target,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let debug_depth = debug_pipeline(
            "debug lines (depth tested)",
            wgpu::CompareFunction::GreaterEqual,
        );
        let debug_overlay = debug_pipeline("debug lines (overlay)", wgpu::CompareFunction::Always);

        let tonemap = output_formats
            .iter()
            .map(|&format| {
                let targets = [Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })];
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&format!("tonemap pipeline ({format:?})")),
                    layout: Some(&tonemap_layout),
                    vertex: wgpu::VertexState {
                        module: &tonemap_module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &tonemap_module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                (format, pipeline)
            })
            .collect();

        let display_layout = layout(
            "display lines pipeline layout",
            &[Some(&layouts.frame), Some(&layouts.display_lines)],
        );
        let display_pipelines = |entry: &str| -> HashMap<_, _> {
            output_formats
                .iter()
                .map(|&format| {
                    let targets = [Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })];
                    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some(&format!("display lines {entry} ({format:?})")),
                        layout: Some(&display_layout),
                        vertex: wgpu::VertexState {
                            module: &display_module,
                            entry_point: Some("vs_main"),
                            compilation_options: Default::default(),
                            buffers: &line_buffers,
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::LineList,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &display_module,
                            entry_point: Some(entry),
                            compilation_options: Default::default(),
                            targets: &targets,
                        }),
                        multiview_mask: None,
                        cache: None,
                    });
                    (format, pipeline)
                })
                .collect()
        };
        let display_depth = display_pipelines("fs_depth_tested");
        let display_overlay = display_pipelines("fs_on_top");

        let buffers = mesh_buffers();
        let outline_mask = [
            (wgpu::FrontFace::Ccw, Some(wgpu::Face::Back)),
            (wgpu::FrontFace::Cw, Some(wgpu::Face::Back)),
            (wgpu::FrontFace::Ccw, None),
        ]
        .map(|(front_face, cull_mode)| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("outline mask pipeline"),
                layout: Some(&layouts.frame_object),
                vertex: wgpu::VertexState {
                    module: &outline_mask_module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    front_face,
                    cull_mode,
                    ..Default::default()
                },
                // Reverse-Z: the nearest selected surface wins.
                depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Greater)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &outline_mask_module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OUTLINE_MASK_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let outline_layout = layout(
            "outline pipeline layout",
            &[Some(&layouts.frame), Some(&layouts.outline)],
        );
        let outline = output_formats
            .iter()
            .map(|&format| {
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&format!("outline composite ({format:?})")),
                    layout: Some(&outline_layout),
                    vertex: wgpu::VertexState {
                        module: &outline_module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &outline_module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                (format, pipeline)
            })
            .collect();

        // Fullscreen passes: a triangle, no depth, one or two targets.
        let fullscreen = |label: &str,
                          group1: &wgpu::BindGroupLayout,
                          module: &wgpu::ShaderModule,
                          vertex: &str,
                          fragment: &str,
                          targets: &[Option<wgpu::ColorTargetState>]| {
            let layout = layout(label, &[Some(&layouts.frame), Some(group1)]);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let target = |format| Some(wgpu::ColorTargetState::from(format));
        let resolve_targets = [target(AO_DEPTH_FORMAT), target(NORMAL_FORMAT)];
        let ao_resolve = fullscreen(
            "ao resolve",
            &layouts.ao_resolve,
            &ao_resolve_module,
            "vs_fullscreen",
            "fs_resolve",
            &resolve_targets,
        );
        let ao = fullscreen(
            "ao",
            &layouts.ao_compute,
            &ao_module,
            "vs_fullscreen",
            "fs_ssao",
            &[target(AO_FORMAT)],
        );
        let ao_blur = fullscreen(
            "ao blur",
            &layouts.ao_blur,
            &ao_module,
            "vs_fullscreen",
            "fs_blur",
            &[target(AO_FORMAT)],
        );
        let bloom_first = fullscreen(
            "bloom first downsample",
            &layouts.bloom,
            &bloom_module,
            "vs_main",
            "fs_downsample_first",
            &[target(hdr_format)],
        );
        let bloom_down = fullscreen(
            "bloom downsample",
            &layouts.bloom,
            &bloom_module,
            "vs_main",
            "fs_downsample",
            &[target(hdr_format)],
        );
        // mip = mix(mip, upsampled, scatter): the scatter is the blend constant.
        let bloom_up = fullscreen(
            "bloom upsample",
            &layouts.bloom,
            &bloom_module,
            "vs_main",
            "fs_upsample",
            &[Some(wgpu::ColorTargetState {
                format: hdr_format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Constant,
                        dst_factor: wgpu::BlendFactor::OneMinusConstant,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::REPLACE,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        );

        let particle_layout = layout(
            "particle pipeline layout",
            &[Some(&layouts.frame), Some(&layouts.particle)],
        );
        let particle_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::particle_render::GpuParticle>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &crate::particle_render::PARTICLE_ATTRIBUTES,
        })];
        let particle_pipeline = |additive: bool| {
            let constants = [("ADDITIVE", if additive { 1.0 } else { 0.0 })];
            let options = wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            };
            let blend = if additive {
                wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: keep_destination_alpha(),
                }
            } else {
                wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if additive {
                    "particles (additive)"
                } else {
                    "particles (alpha)"
                }),
                layout: Some(&particle_layout),
                vertex: wgpu::VertexState {
                    module: &particle_module,
                    entry_point: Some("vs_main"),
                    compilation_options: options.clone(),
                    buffers: &particle_buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                // Tested against opaque depth, not written (reverse-Z).
                depth_stencil: Some(depth_state(false, wgpu::CompareFunction::Greater)),
                multisample: msaa,
                fragment: Some(wgpu::FragmentState {
                    module: &particle_module,
                    entry_point: Some("fs_main"),
                    compilation_options: options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: hdr_format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let particle_alpha = particle_pipeline(false);
        let particle_additive = particle_pipeline(true);

        Self {
            particle_alpha,
            particle_additive,
            ao_resolve,
            ao,
            ao_blur,
            bloom_first,
            bloom_down,
            bloom_up,
            sky,
            debug_depth,
            debug_overlay,
            tonemap,
            display_depth,
            display_overlay,
            outline_mask,
            outline,
            modules,
        }
    }
}

/// Which shader a material pipeline runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ShaderKey {
    Standard,
    /// The magenta error material (main pass only).
    Error,
    /// A material shader, by its `ShaderSource` asset id.
    Custom(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Pass {
    /// The HDR forward pass (opaque, then sorted transparent).
    Main,
    /// A shadow cascade (depth only).
    Shadow,
    /// The depth + normal prepass (opaque and alpha-masked materials).
    Prepass,
    /// The selection outline mask, for materials that move vertices (wind
    /// sway, a material shader's `vertex` hook); others use the fixed
    /// outline mask pipelines.
    Outline,
    /// Depth only, in the HDR pass just before the colour draw of a
    /// transparent material with `prewrite_depth` (no colour writes).
    Prewrite,
}

/// The `ALPHA_MODE` pipeline constant (forward.wgsl).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum AlphaKey {
    Opaque = 0,
    Mask = 1,
    Blend = 2,
    Additive = 3,
    Multiply = 4,
    Multiply2x = 5,
}

impl From<AlphaMode> for AlphaKey {
    fn from(mode: AlphaMode) -> Self {
        match mode {
            AlphaMode::Opaque => Self::Opaque,
            AlphaMode::Mask { .. } => Self::Mask,
            AlphaMode::Blend => Self::Blend,
            AlphaMode::Additive => Self::Additive,
            AlphaMode::Multiply => Self::Multiply,
            AlphaMode::Multiply2x => Self::Multiply2x,
        }
    }
}

impl AlphaKey {
    /// Drawn in the sorted transparent pass, without depth writes and
    /// without casting shadows.
    pub(crate) fn is_transparent(self) -> bool {
        matches!(
            self,
            Self::Blend | Self::Additive | Self::Multiply | Self::Multiply2x
        )
    }

    /// The colour blend of the main pass (`None`: replace).
    pub(crate) fn blend(self) -> Option<wgpu::BlendState> {
        let multiply = |src_factor| wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: keep_destination_alpha(),
        };
        match self {
            Self::Opaque | Self::Mask => None,
            Self::Blend => Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            Self::Additive => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: keep_destination_alpha(),
            }),
            // dst * src.
            Self::Multiply => Some(multiply(wgpu::BlendFactor::Zero)),
            // src * dst + dst * src = 2 * src * dst.
            Self::Multiply2x => Some(multiply(wgpu::BlendFactor::Dst)),
        }
    }
}

/// The depth comparison of a main-pass pipeline (reverse-Z: nearer is
/// greater).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DepthCompare {
    /// Nearer than what is there (the default).
    Greater,
    /// Nearer or at the same depth: the colour draw after a depth pre-write.
    GreaterEqual,
    /// No depth test.
    Always,
}

impl DepthCompare {
    pub(crate) fn wgpu(self) -> wgpu::CompareFunction {
        match self {
            Self::Greater => wgpu::CompareFunction::Greater,
            Self::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
            Self::Always => wgpu::CompareFunction::Always,
        }
    }
}

/// The depth state of a main-pass (or pre-write) pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DepthKey {
    pub write: bool,
    pub compare: DepthCompare,
}

impl DepthKey {
    /// Writes and tests depth: opaque surfaces and depth pre-writes.
    pub(crate) const OPAQUE: Self = Self {
        write: true,
        compare: DepthCompare::Greater,
    };
    /// Tests but does not write: transparent surfaces.
    pub(crate) const TRANSPARENT: Self = Self {
        write: false,
        compare: DepthCompare::Greater,
    };

    /// The default for `alpha`.
    pub(crate) fn for_alpha(alpha: AlphaKey) -> Self {
        if alpha.is_transparent() {
            Self::TRANSPARENT
        } else {
            Self::OPAQUE
        }
    }
}

/// A material's pipeline state in the HDR pass (R6), from its alpha mode
/// and its `depth_write`, `depth_test` and `prewrite_depth` fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MaterialDepth {
    /// The colour draw.
    pub main: DepthKey,
    /// A depth-only draw ([`Pass::Prewrite`], [`DepthKey::OPAQUE`]) right
    /// before the colour draw.
    pub prewrite: bool,
}

impl MaterialDepth {
    /// `prewrite` applies to transparent modes with the depth test on (a
    /// pre-write without a test would keep the last surface drawn, not the
    /// nearest); the colour draw then passes where its depth equals the
    /// written one.
    pub(crate) fn resolve(alpha: AlphaKey, write: DepthWrite, test: bool, prewrite: bool) -> Self {
        let prewrite = prewrite && test && alpha.is_transparent();
        let write = match write {
            DepthWrite::Auto => !alpha.is_transparent(),
            DepthWrite::On => true,
            DepthWrite::Off => false,
        };
        let compare = match (test, prewrite) {
            (false, _) => DepthCompare::Always,
            (true, true) => DepthCompare::GreaterEqual,
            (true, false) => DepthCompare::Greater,
        };
        Self {
            main: DepthKey { write, compare },
            prewrite,
        }
    }

    /// The default state for `alpha` (what every material had before R6).
    pub(crate) fn default_for(alpha: AlphaKey) -> Self {
        Self {
            main: DepthKey::for_alpha(alpha),
            prewrite: false,
        }
    }

    /// The colour draw writes and tests depth like an ordinary opaque
    /// surface, so it belongs in the depth prepass (ambient occlusion, scene
    /// depth). Opaque materials that do not write or test depth are left
    /// out of it.
    pub(crate) fn in_prepass(&self) -> bool {
        self.main == DepthKey::OPAQUE
    }
}

/// The vertex buffer layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum VertexLayout {
    /// [`GpuVertex`]: position, normal, uv, colour, tangent, uv1.
    Mesh,
    /// [`GpuVertex`] plus a second buffer of joints and weights
    /// (`GpuSkinVertex`): skinned meshes, drawn by the `*_skinned` vertex
    /// entry points (skin.wgsl).
    Skinned,
}

/// Everything a material pipeline depends on: shader, pass and flags,
/// vertex layout, sample count and target format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineKey {
    pub shader: ShaderKey,
    pub pass: Pass,
    pub alpha: AlphaKey,
    pub double_sided: bool,
    /// Clockwise front faces (a mirroring transform).
    pub mirrored: bool,
    /// Bits of the mask cutoff of a material shader (its `ALPHA_CUTOFF`
    /// constant); 0 otherwise. `StandardMaterial` reads its cutoff from the
    /// material uniform.
    pub alpha_cutoff_bits: u32,
    /// Bits of the depth fade distance of a material shader (its
    /// `DEPTH_FADE` constant); 0 otherwise. `StandardMaterial` reads it from
    /// the material uniform.
    pub depth_fade_bits: u32,
    /// A `StandardMaterial` that sways in the wind (the `*_wind` vertex entry
    /// points, which read the material).
    pub wind: bool,
    /// Depth write and compare of [`Pass::Main`] and [`Pass::Prewrite`]
    /// keys; [`DepthKey::OPAQUE`] in other passes (their state is fixed).
    pub depth: DepthKey,
    pub vertex_layout: VertexLayout,
    pub samples: u32,
    pub format: wgpu::TextureFormat,
}

impl PipelineKey {
    /// A main-pass key drawing into the HDR target.
    pub(crate) fn main(
        shader: ShaderKey,
        alpha: AlphaKey,
        double_sided: bool,
        mirrored: bool,
        hdr_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            shader,
            pass: Pass::Main,
            alpha,
            double_sided,
            mirrored,
            alpha_cutoff_bits: 0,
            depth_fade_bits: 0,
            wind: false,
            depth: DepthKey::for_alpha(alpha),
            vertex_layout: VertexLayout::Mesh,
            samples: MSAA_SAMPLES,
            format: hdr_format,
        }
    }

    /// The depth-only draw before the colour draw of a transparent material
    /// with `prewrite_depth`, in the HDR pass.
    pub(crate) fn prewrite(
        shader: ShaderKey,
        alpha: AlphaKey,
        double_sided: bool,
        mirrored: bool,
        hdr_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            pass: Pass::Prewrite,
            depth: DepthKey::OPAQUE,
            ..Self::main(shader, alpha, double_sided, mirrored, hdr_format)
        }
    }

    /// A shadow-pass key (opaque or mask).
    pub(crate) fn shadow(
        shader: ShaderKey,
        mask: bool,
        double_sided: bool,
        mirrored: bool,
    ) -> Self {
        Self {
            shader,
            pass: Pass::Shadow,
            alpha: if mask {
                AlphaKey::Mask
            } else {
                AlphaKey::Opaque
            },
            double_sided,
            mirrored,
            alpha_cutoff_bits: 0,
            depth_fade_bits: 0,
            wind: false,
            depth: DepthKey::OPAQUE,
            vertex_layout: VertexLayout::Mesh,
            samples: 1,
            format: SHADOW_FORMAT,
        }
    }

    /// A depth + normal prepass key (single-sample: spike S5).
    pub(crate) fn prepass(
        shader: ShaderKey,
        mask: bool,
        double_sided: bool,
        mirrored: bool,
    ) -> Self {
        Self {
            shader,
            pass: Pass::Prepass,
            alpha: if mask {
                AlphaKey::Mask
            } else {
                AlphaKey::Opaque
            },
            double_sided,
            mirrored,
            alpha_cutoff_bits: 0,
            depth_fade_bits: 0,
            wind: false,
            depth: DepthKey::OPAQUE,
            vertex_layout: VertexLayout::Mesh,
            samples: 1,
            format: NORMAL_FORMAT,
        }
    }

    /// A selection outline mask key (materials that move vertices).
    pub(crate) fn outline(shader: ShaderKey, double_sided: bool, mirrored: bool) -> Self {
        Self {
            shader,
            pass: Pass::Outline,
            alpha: AlphaKey::Opaque,
            double_sided,
            mirrored,
            alpha_cutoff_bits: 0,
            depth_fade_bits: 0,
            wind: false,
            depth: DepthKey::OPAQUE,
            vertex_layout: VertexLayout::Mesh,
            samples: 1,
            format: OUTLINE_MASK_FORMAT,
        }
    }

    pub(crate) fn with_cutoff(mut self, cutoff: f32) -> Self {
        self.alpha_cutoff_bits = cutoff.to_bits();
        self
    }

    pub(crate) fn with_depth_fade(mut self, distance: f32) -> Self {
        self.depth_fade_bits = distance.to_bits();
        self
    }

    pub(crate) fn with_wind(mut self, wind: bool) -> Self {
        self.wind = wind;
        self
    }

    /// A main-pass key with another depth state (R6).
    pub(crate) fn with_depth(mut self, depth: DepthKey) -> Self {
        self.depth = depth;
        self
    }

    /// The skinned variant (RS4).
    pub(crate) fn skinned(mut self) -> Self {
        self.vertex_layout = VertexLayout::Skinned;
        self
    }
}

/// The engine module behind a non-custom material pipeline key.
pub(crate) fn engine_module<'a>(
    modules: &'a EngineModules,
    key: &PipelineKey,
) -> &'a wgpu::ShaderModule {
    let skinned = key.vertex_layout == VertexLayout::Skinned;
    match (key.shader, key.pass, skinned) {
        // The error material's prepass is the standard opaque one.
        (ShaderKey::Error, Pass::Prepass, false) => &modules.standard,
        (ShaderKey::Error, Pass::Prepass, true) => &modules.standard_skinned,
        (ShaderKey::Error, _, false) => &modules.error,
        (ShaderKey::Error, _, true) => &modules.error_skinned,
        (_, Pass::Shadow, false) => &modules.shadow,
        (_, Pass::Shadow, true) => &modules.shadow_skinned,
        (_, _, false) => &modules.standard,
        (_, _, true) => &modules.standard_skinned,
    }
}

/// Creates the pipeline for `key` from `module` (the module of
/// `key.shader` for `key.pass`). Errors go to the device's error scopes.
pub(crate) fn create_material_pipeline(
    device: &wgpu::Device,
    layouts: &Layouts,
    key: &PipelineKey,
    module: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let depth_clamp = device
        .features()
        .contains(wgpu::Features::DEPTH_CLIP_CONTROL);
    let custom = matches!(key.shader, ShaderKey::Custom(_));
    let mut constants: Vec<(&str, f64)> = Vec::new();
    if key.shader != ShaderKey::Error && (key.pass != Pass::Shadow || custom) {
        constants.push(("ALPHA_MODE", key.alpha as u32 as f64));
        constants.push(("DOUBLE_SIDED", if key.double_sided { 1.0 } else { 0.0 }));
    }
    if custom {
        constants.push(("ALPHA_CUTOFF", f32::from_bits(key.alpha_cutoff_bits) as f64));
        constants.push(("DEPTH_FADE", f32::from_bits(key.depth_fade_bits) as f64));
    }
    // Swaying standard materials use the `*_wind` vertex entry points.
    let wind = key.wind && key.shader == ShaderKey::Standard;
    let options = wgpu::PipelineCompilationOptions {
        constants: &constants,
        ..Default::default()
    };
    let primitive = wgpu::PrimitiveState {
        front_face: if key.mirrored {
            wgpu::FrontFace::Cw
        } else {
            wgpu::FrontFace::Ccw
        },
        cull_mode: (!key.double_sided).then_some(wgpu::Face::Back),
        unclipped_depth: key.pass == Pass::Shadow && depth_clamp,
        ..Default::default()
    };
    // Skinned meshes use the `*_skinned` vertex entry points (skin.wgsl).
    // Swaying materials keep their layouts (they bind the material) but do
    // not sway when skinned.
    let skinned = key.vertex_layout == VertexLayout::Skinned;
    let buffers = vertex_buffers(key.vertex_layout);
    let label = format!(
        "{:?} {:?} {:?} {:?} pipeline",
        key.shader, key.pass, key.alpha, key.vertex_layout
    );
    match key.pass {
        Pass::Main | Pass::Prewrite => {
            let layout = match key.shader {
                ShaderKey::Standard => &layouts.standard_main,
                ShaderKey::Error => &layouts.frame_object,
                ShaderKey::Custom(_) => &layouts.custom_main,
            };
            // A depth pre-write runs the material's fragment stage (its
            // discards apply) but writes no colour.
            let (blend, write_mask) = match key.pass {
                Pass::Prewrite => (None, wgpu::ColorWrites::empty()),
                _ => (key.alpha.blend(), wgpu::ColorWrites::ALL),
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(match (skinned, wind) {
                        (true, _) => "vs_main_skinned",
                        (false, true) => "vs_main_wind",
                        (false, false) => "vs_main",
                    }),
                    compilation_options: options.clone(),
                    buffers: &buffers,
                },
                primitive,
                // Reverse-Z: nearer is greater. By default transparent
                // surfaces test against opaque depth but do not write it.
                depth_stencil: Some(depth_state(key.depth.write, key.depth.compare.wgpu())),
                multisample: wgpu::MultisampleState {
                    count: key.samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.format,
                        blend,
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }
        Pass::Prepass => {
            let mask = key.alpha == AlphaKey::Mask;
            let (layout, fragment) = match key.shader {
                ShaderKey::Custom(_) => (&layouts.custom_main, "fs_prepass"),
                _ if mask => (&layouts.standard_prepass_mask, "fs_prepass_mask"),
                _ if wind => (&layouts.standard_prepass_mask, "fs_prepass"),
                _ => (&layouts.frame_object, "fs_prepass"),
            };
            let vertex = match (custom, skinned, wind) {
                (true, true, _) => "vs_main_skinned",
                (true, false, _) => "vs_main",
                (false, true, _) => "vs_prepass_skinned",
                (false, false, true) => "vs_prepass_wind",
                (false, false, false) => "vs_prepass",
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: options.clone(),
                    buffers: &buffers,
                },
                primitive,
                depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Greater)),
                multisample: wgpu::MultisampleState {
                    count: key.samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options: options,
                    targets: &[Some(wgpu::ColorTargetState::from(key.format))],
                }),
                multiview_mask: None,
                cache: None,
            })
        }
        Pass::Shadow => {
            let mask = key.alpha == AlphaKey::Mask;
            let (layout, vertex, fragment) = match (custom, depth_clamp) {
                (false, clamp) => (
                    if mask || wind {
                        &layouts.standard_shadow_mask
                    } else {
                        &layouts.standard_shadow
                    },
                    match (skinned, wind, clamp) {
                        (true, _, true) => "vs_skinned",
                        (true, _, false) => "vs_skinned_pancake",
                        (false, false, true) => "vs_main",
                        (false, false, false) => "vs_pancake",
                        (false, true, true) => "vs_wind",
                        (false, true, false) => "vs_wind_pancake",
                    },
                    "fs_mask",
                ),
                (true, clamp) => (
                    &layouts.custom_shadow,
                    match (skinned, clamp) {
                        (true, true) => "vs_shadow_skinned",
                        (true, false) => "vs_shadow_pancake_skinned",
                        (false, true) => "vs_shadow",
                        (false, false) => "vs_shadow_pancake",
                    },
                    "fs_shadow_mask",
                ),
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: options.clone(),
                    buffers: &buffers,
                },
                primitive,
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: key.format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 0,
                        slope_scale: 1.5,
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                fragment: mask.then_some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fragment),
                    compilation_options: options,
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            })
        }
        Pass::Outline => {
            let layout = if custom {
                &layouts.custom_main
            } else {
                &layouts.standard_prepass_mask
            };
            let vertex = match (skinned, custom) {
                (true, _) => "vs_outline_skinned",
                (false, true) => "vs_outline",
                (false, false) => "vs_outline_wind",
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vertex),
                    compilation_options: options.clone(),
                    buffers: &buffers,
                },
                primitive,
                // As the fixed outline mask: the nearest selected surface wins.
                depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Greater)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_outline"),
                    compilation_options: options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }
    }
}

fn keep_destination_alpha() -> wgpu::BlendComponent {
    wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    }
}

/// Material pipelines by key. Ids are stable: a rebuilt pipeline (hot
/// reload) replaces the old one in its slot, so material entries holding ids
/// stay valid.
#[derive(Default)]
pub(crate) struct PipelineCache {
    slots: Vec<Option<(PipelineKey, wgpu::RenderPipeline)>>,
    index: HashMap<PipelineKey, u32>,
}

impl PipelineCache {
    pub(crate) fn get(&self, id: u32) -> Option<&wgpu::RenderPipeline> {
        self.slots.get(id as usize)?.as_ref().map(|(_, p)| p)
    }

    pub(crate) fn id(&self, key: &PipelineKey) -> Option<u32> {
        self.index.get(key).copied()
    }

    pub(crate) fn insert(&mut self, key: PipelineKey, pipeline: wgpu::RenderPipeline) -> u32 {
        if let Some(&id) = self.index.get(&key) {
            self.slots[id as usize] = Some((key, pipeline));
            return id;
        }
        let id = self.slots.len() as u32;
        self.slots.push(Some((key, pipeline)));
        self.index.insert(key, id);
        id
    }

    /// Keys (with ids) of every pipeline for which `filter` is true.
    pub(crate) fn keys_where(
        &self,
        filter: impl Fn(&PipelineKey) -> bool,
    ) -> Vec<(u32, PipelineKey)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|(k, _)| (i as u32, *k)))
            .filter(|(_, k)| filter(k))
            .collect()
    }

    /// Replaces the pipelines in the given slots (all of a rebuild at once).
    pub(crate) fn replace(&mut self, rebuilt: Vec<(u32, wgpu::RenderPipeline)>) {
        for (id, pipeline) in rebuilt {
            if let Some(Some((_, p))) = self.slots.get_mut(id as usize) {
                *p = pipeline;
            }
        }
    }

    /// Drops every pipeline of `shader` (its source asset was removed).
    pub(crate) fn remove_shader(&mut self, shader: ShaderKey) {
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|(k, _)| k.shader == shader) {
                *slot = None;
            }
        }
        self.index.retain(|k, _| k.shader != shader);
    }

    /// Live pipelines.
    pub(crate) fn len(&self) -> usize {
        self.index.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rg11b10Ufloat;

    #[test]
    fn pipeline_keys_separate_every_input() {
        let base = PipelineKey::main(ShaderKey::Standard, AlphaKey::Opaque, false, false, HDR);
        let variants =
            [
                base,
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Mask, false, false, HDR),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Additive, false, false, HDR),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Multiply, false, false, HDR),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Opaque, true, false, HDR),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Opaque, false, true, HDR),
                PipelineKey::main(ShaderKey::Error, AlphaKey::Opaque, false, false, HDR),
                PipelineKey::main(ShaderKey::Custom(7), AlphaKey::Opaque, false, false, HDR),
                PipelineKey::main(ShaderKey::Custom(8), AlphaKey::Opaque, false, false, HDR),
                PipelineKey::main(ShaderKey::Custom(7), AlphaKey::Mask, false, false, HDR)
                    .with_cutoff(0.5),
                PipelineKey::main(ShaderKey::Custom(7), AlphaKey::Mask, false, false, HDR)
                    .with_cutoff(0.25),
                PipelineKey::main(
                    ShaderKey::Standard,
                    AlphaKey::Opaque,
                    false,
                    false,
                    wgpu::TextureFormat::Rgba16Float,
                ),
                PipelineKey { samples: 1, ..base },
                PipelineKey::shadow(ShaderKey::Standard, false, false, false),
                PipelineKey::shadow(ShaderKey::Standard, true, false, false),
                PipelineKey::shadow(ShaderKey::Standard, false, true, false),
                PipelineKey::shadow(ShaderKey::Standard, false, false, true),
                PipelineKey::prepass(ShaderKey::Standard, false, false, false),
                PipelineKey::prepass(ShaderKey::Standard, true, false, false),
                PipelineKey::prepass(ShaderKey::Standard, false, true, false),
                PipelineKey::prepass(ShaderKey::Standard, false, false, false).with_wind(true),
                PipelineKey::shadow(ShaderKey::Standard, false, false, false).with_wind(true),
                base.with_wind(true),
                PipelineKey::main(ShaderKey::Custom(7), AlphaKey::Blend, false, false, HDR)
                    .with_depth_fade(2.0),
                PipelineKey::outline(ShaderKey::Standard, false, false).with_wind(true),
                PipelineKey::outline(ShaderKey::Custom(7), false, false),
                PipelineKey::outline(ShaderKey::Custom(7), true, false),
                base.skinned(),
                PipelineKey::shadow(ShaderKey::Standard, false, false, false).skinned(),
                PipelineKey::outline(ShaderKey::Standard, false, false).skinned(),
                // R6: Multiply2x, depth state, the depth pre-write.
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Multiply2x, false, false, HDR),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR)
                    .with_depth(DepthKey::OPAQUE),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR)
                    .with_depth(DepthKey {
                        write: false,
                        compare: DepthCompare::Always,
                    }),
                PipelineKey::main(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR)
                    .with_depth(DepthKey {
                        write: false,
                        compare: DepthCompare::GreaterEqual,
                    }),
                PipelineKey::prewrite(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR),
                PipelineKey::prewrite(ShaderKey::Custom(7), AlphaKey::Blend, false, false, HDR),
            ];
        let set: HashSet<_> = variants.iter().collect();
        assert_eq!(set.len(), variants.len(), "every input changes the key");
        // Equal inputs give equal keys.
        assert_eq!(
            base,
            PipelineKey::main(ShaderKey::Standard, AlphaKey::Opaque, false, false, HDR)
        );
        assert_eq!(
            PipelineKey::shadow(ShaderKey::Standard, false, false, false).samples,
            1
        );
        assert!(AlphaKey::from(AlphaMode::Blend).is_transparent());
        assert!(!AlphaKey::from(AlphaMode::Mask { cutoff: 0.5 }).is_transparent());
        assert_eq!(
            AlphaKey::Multiply as u32,
            4,
            "matches ALPHA_MULTIPLY in forward.wgsl"
        );
        assert_eq!(
            AlphaKey::Multiply2x as u32,
            5,
            "matches ALPHA_MULTIPLY2X in forward.wgsl"
        );
        assert!(AlphaKey::from(AlphaMode::Multiply2x).is_transparent());
        // The main key's default depth state follows the alpha mode.
        assert_eq!(base.depth, DepthKey::OPAQUE);
        let blend = PipelineKey::main(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR);
        assert_eq!(blend.depth, DepthKey::TRANSPARENT);
        let pre = PipelineKey::prewrite(ShaderKey::Standard, AlphaKey::Blend, false, false, HDR);
        assert_eq!((pre.pass, pre.depth), (Pass::Prewrite, DepthKey::OPAQUE));
        assert_eq!(
            (pre.samples, pre.format),
            (MSAA_SAMPLES, HDR),
            "in the HDR pass"
        );
    }

    #[test]
    fn material_depth_maps_the_material_fields() {
        use DepthCompare::*;
        let key = |write, compare| DepthKey { write, compare };
        let r = MaterialDepth::resolve;
        // Defaults: what every material had before R6.
        for alpha in [
            AlphaKey::Opaque,
            AlphaKey::Mask,
            AlphaKey::Blend,
            AlphaKey::Additive,
            AlphaKey::Multiply,
            AlphaKey::Multiply2x,
        ] {
            assert_eq!(
                r(alpha, DepthWrite::Auto, true, false),
                MaterialDepth::default_for(alpha),
                "{alpha:?}"
            );
        }
        let opaque = r(AlphaKey::Opaque, DepthWrite::Auto, true, false);
        assert_eq!(opaque.main, key(true, Greater));
        assert!(opaque.in_prepass());
        let blend = r(AlphaKey::Blend, DepthWrite::Auto, true, false);
        assert_eq!((blend.main, blend.prewrite), (key(false, Greater), false));
        // Overrides.
        assert_eq!(
            r(AlphaKey::Blend, DepthWrite::On, true, false).main,
            key(true, Greater)
        );
        let no_write = r(AlphaKey::Opaque, DepthWrite::Off, true, false);
        assert_eq!(no_write.main, key(false, Greater));
        assert!(!no_write.in_prepass(), "not depth for AO");
        let no_test = r(AlphaKey::Opaque, DepthWrite::Auto, false, false);
        assert_eq!(no_test.main, key(true, Always));
        assert!(!no_test.in_prepass());
        // The ghost: depth first, then colour where equal or nearer.
        let ghost = r(AlphaKey::Blend, DepthWrite::Auto, true, true);
        assert_eq!(
            (ghost.main, ghost.prewrite),
            (key(false, GreaterEqual), true)
        );
        // A pre-write needs a transparent mode and the depth test.
        assert_eq!(r(AlphaKey::Opaque, DepthWrite::Auto, true, true), opaque);
        let untested = r(AlphaKey::Blend, DepthWrite::Auto, false, true);
        assert_eq!(
            (untested.main, untested.prewrite),
            (key(false, Always), false)
        );
        assert_eq!(GreaterEqual.wgpu(), wgpu::CompareFunction::GreaterEqual);
    }

    #[test]
    fn multiply_blends_scale_the_destination() {
        use wgpu::BlendFactor::*;
        let color = |a: AlphaKey| a.blend().map(|b| (b.color.src_factor, b.color.dst_factor));
        assert_eq!(color(AlphaKey::Opaque), None);
        assert_eq!(color(AlphaKey::Mask), None);
        // dst * src.
        assert_eq!(color(AlphaKey::Multiply), Some((Zero, Src)));
        // src * dst + dst * src = 2 src dst: 0.5 is neutral.
        assert_eq!(color(AlphaKey::Multiply2x), Some((Dst, Src)));
        assert_eq!(color(AlphaKey::Additive), Some((One, One)));
        // Multiply modes keep the destination alpha.
        for a in [AlphaKey::Multiply, AlphaKey::Multiply2x, AlphaKey::Additive] {
            assert_eq!(a.blend().unwrap().alpha, keep_destination_alpha());
        }
    }
}
