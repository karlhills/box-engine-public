//! Per-material GPU state: the uniform buffer, bind group (group 2) and
//! pipeline ids of every `StandardMaterial` and `ShaderMaterial` asset,
//! rebuilt only when the material, the textures it resolves to, or its
//! shader change.

use std::collections::HashMap;

use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets};
use box_core::image::ImageData;
use box_core::material::{AlphaMode, StandardMaterial, WindWeight};
use box_core::shader_material::{ShaderMaterial, ShaderSource};

use crate::gpu_mesh::UploadStats;
use crate::material_shader::{CustomShaders, pack_params};
use crate::pipelines::{
    AlphaKey, EngineModules, Layouts, MaterialDepth, PipelineCache, PipelineKey, ShaderKey,
    create_material_pipeline, engine_module,
};
use crate::renderer::catch_errors;
use crate::textures::{DefaultTexture, TextureCache};
use crate::uniforms::{
    MATERIAL_FLAG_NORMAL_MAP, MATERIAL_FLAG_WIND_VERTEX_ALPHA, StandardMaterialUniform,
};

/// Material id of the renderer's default material (asset ids start at 1).
/// Used for empty material slots and handles to missing materials.
pub(crate) const DEFAULT_MATERIAL: u64 = 0;

/// The largest sway (`wind_offset` in wind.wgsl) per unit of wind strength
/// and material sway strength: lean 1 + flutter 0.3 + across 0.15, with a
/// margin.
pub(crate) const WIND_REACH: f32 = 1.5;

/// What the renderer needs to draw with one material.
pub(crate) struct MaterialEntry {
    /// Group 2; `None` for the error material.
    pub bind_group: Option<wgpu::BindGroup>,
    /// Main-pass pipeline per winding (index: mirrored). `None` when no
    /// pipeline could be created (the error is logged).
    pub main: Option<[u32; 2]>,
    /// Shadow pipeline per winding; `None` when the material casts no
    /// shadow (transparent modes).
    pub shadow: Option<[u32; 2]>,
    /// The shadow pipeline reads group 2 (alpha mask, wind sway, material
    /// shaders).
    pub shadow_binds_material: bool,
    /// Depth + normal prepass pipeline per winding; `None` for transparent
    /// modes and for materials that do not write and test depth. It reads
    /// group 2 when the shadow pipeline does.
    pub prepass: Option<[u32; 2]>,
    /// Depth-only pipeline per winding drawn just before the colour draw
    /// (`prewrite_depth` of a transparent material); `None` otherwise.
    pub prewrite: Option<[u32; 2]>,
    /// Selection outline mask pipeline per winding for materials that move
    /// vertices (wind sway, a material shader's `vertex` hook); `None` uses
    /// the fixed outline mask pipelines. It reads group 2.
    pub outline: Option<[u32; 2]>,
    pub transparent: bool,
    pub double_sided: bool,
    /// Transparent draw order bias (`sort_bias`): higher draws later.
    pub sort_bias: i32,
    /// Needs the prepass depth while visible: a transparent material with a
    /// depth fade, or a material shader that reads the scene depth.
    pub scene_depth: bool,
    /// Culling bounds margin in metres (`ShaderMaterial::bounds_padding`).
    pub bounds_padding: f32,
    /// `WindSway::strength` of a standard material (0 without wind): the
    /// sway is at most `WIND_REACH * strength * Wind::strength`, by which
    /// object bounds are widened for culling and the outline scissor.
    pub wind_reach: f32,
    /// Pipelines for skinned meshes (RS4), built the first time a skinned
    /// mesh draws with this material ([`MaterialCache::ensure_skinned`]).
    pub skinned: Option<SkinnedPipelines>,
    /// What the pipelines were built from, for the skinned variants.
    recipe: Option<Recipe>,
    stamp: Stamp,
    buffer: Option<wgpu::Buffer>,
}

/// A material's skinned pipelines per winding; `None` where the material
/// has none (as for the static ones) or creation failed (logged).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SkinnedPipelines {
    pub main: Option<[u32; 2]>,
    pub shadow: Option<[u32; 2]>,
    pub prepass: Option<[u32; 2]>,
    pub prewrite: Option<[u32; 2]>,
    /// Skinned vertices move: always a material outline pipeline (except
    /// the error material, which outlines in bind pose).
    pub outline: Option<[u32; 2]>,
}

/// The inputs of [`MaterialContext::pipelines_for`].
#[derive(Clone, Copy, Debug)]
struct Recipe {
    shader: ShaderKey,
    alpha: AlphaMode,
    double_sided: bool,
    moves: Moves,
    depth: MaterialDepth,
}

/// Inputs an entry was built from. A field that does not apply is 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stamp {
    version: u64,
    /// `AssetLookup::generation`, for materials with references.
    lookup: u64,
    /// `TextureCache::generation`, for materials with textures.
    textures: u64,
    /// The resolved shader asset and its compile state.
    shader: Option<(u64, u64)>,
}

/// Borrowed renderer state a material build needs.
pub(crate) struct MaterialContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub layouts: &'a Layouts,
    pub textures: &'a TextureCache,
    pub shaders: &'a CustomShaders,
    pub modules: &'a EngineModules,
    pub pipelines: &'a mut PipelineCache,
    pub hdr_format: wgpu::TextureFormat,
    pub upload: &'a mut UploadStats,
}

/// World data materials resolve against.
#[derive(Clone, Copy)]
pub(crate) struct MaterialInputs<'a> {
    pub standard: Option<&'a Assets<StandardMaterial>>,
    pub shader: Option<&'a Assets<ShaderMaterial>>,
    pub lookup: Option<&'a AssetLookup>,
}

impl MaterialInputs<'_> {
    fn lookup_generation(&self) -> u64 {
        self.lookup.map_or(0, AssetLookup::generation)
    }

    fn resolve_image(&self, r: &AssetRef) -> Option<u64> {
        self.lookup?.get::<ImageData>(r).map(|h| h.id())
    }

    fn resolve_shader(&self, r: &AssetRef) -> Option<u64> {
        self.lookup?.get::<ShaderSource>(r).map(|h| h.id())
    }
}

impl MaterialContext<'_> {
    /// The pipeline for `key`, created now if new. Creation errors are
    /// returned (and caught, never reaching the uncaptured-error handler).
    pub(crate) fn pipeline(&mut self, key: PipelineKey) -> Result<u32, String> {
        if let Some(id) = self.pipelines.id(&key) {
            return Ok(id);
        }
        let module = match key.shader {
            ShaderKey::Custom(id) => {
                &self
                    .shaders
                    .get(id)
                    .and_then(|s| s.compiled.as_ref())
                    .ok_or("material shader is not compiled")?
                    .module
            }
            _ => engine_module(self.modules, &key),
        };
        let (device, layouts) = (self.device, self.layouts);
        let pipeline = catch_errors(device, || {
            create_material_pipeline(device, layouts, &key, module)
        })?;
        Ok(self.pipelines.insert(key, pipeline))
    }

    /// Main, shadow, prepass, depth pre-write and (for materials that move
    /// vertices, and skinned variants) outline pipelines for both windings.
    fn pipelines_for(&mut self, r: &Recipe, skinned: bool) -> Result<MaterialPipelines, String> {
        let Recipe {
            shader,
            alpha,
            double_sided,
            moves,
            depth,
        } = *r;
        let layout = |k: PipelineKey| if skinned { k.skinned() } else { k };
        let key = AlphaKey::from(alpha);
        let cutoff = match (shader, alpha) {
            (ShaderKey::Custom(_), AlphaMode::Mask { cutoff }) => cutoff,
            _ => 0.0,
        };
        let wind = moves.wind;
        let fade = match shader {
            ShaderKey::Custom(_) if key.is_transparent() => moves.depth_fade,
            _ => 0.0,
        };
        let main = |mirrored| {
            layout(
                PipelineKey::main(shader, key, double_sided, mirrored, self.hdr_format)
                    .with_cutoff(cutoff)
                    .with_depth_fade(fade)
                    .with_wind(wind)
                    .with_depth(depth.main),
            )
        };
        let (m0, m1) = (main(false), main(true));
        let main = [self.pipeline(m0)?, self.pipeline(m1)?];
        let prewrite = if depth.prewrite {
            let prewrite = |mirrored| {
                layout(
                    PipelineKey::prewrite(shader, key, double_sided, mirrored, self.hdr_format)
                        .with_cutoff(cutoff)
                        .with_depth_fade(fade)
                        .with_wind(wind),
                )
            };
            let (p0, p1) = (prewrite(false), prewrite(true));
            Some([self.pipeline(p0)?, self.pipeline(p1)?])
        } else {
            None
        };
        let shadow = if key.is_transparent() {
            None
        } else {
            let mask = key == AlphaKey::Mask;
            let shadow = |mirrored| {
                layout(
                    PipelineKey::shadow(shader, mask, double_sided, mirrored)
                        .with_cutoff(cutoff)
                        .with_wind(wind),
                )
            };
            Some([self.pipeline(shadow(false))?, self.pipeline(shadow(true))?])
        };
        let prepass = if key.is_transparent() || !depth.in_prepass() {
            None
        } else {
            let mask = key == AlphaKey::Mask;
            let prepass = |mirrored| {
                layout(
                    PipelineKey::prepass(shader, mask, double_sided, mirrored)
                        .with_cutoff(cutoff)
                        .with_wind(wind),
                )
            };
            Some([
                self.pipeline(prepass(false))?,
                self.pipeline(prepass(true))?,
            ])
        };
        let outline = if (moves.vertices() || skinned) && shader != ShaderKey::Error {
            let outline = |mirrored| {
                layout(
                    PipelineKey::outline(shader, double_sided, mirrored)
                        .with_cutoff(cutoff)
                        .with_wind(wind),
                )
            };
            Some([
                self.pipeline(outline(false))?,
                self.pipeline(outline(true))?,
            ])
        } else {
            None
        };
        Ok(MaterialPipelines {
            main,
            shadow,
            prepass,
            prewrite,
            outline,
        })
    }
}

/// What a material does beyond its alpha and face modes that its pipelines
/// depend on.
#[derive(Clone, Copy, Debug, Default)]
struct Moves {
    /// A standard material swaying in the wind.
    wind: bool,
    /// A material shader with a `vertex` hook.
    vertex_hook: bool,
    /// Depth fade distance (a pipeline constant of material shaders).
    depth_fade: f32,
}

impl Moves {
    fn vertices(&self) -> bool {
        self.wind || self.vertex_hook
    }
}

/// A material's pipelines per winding (index: mirrored).
struct MaterialPipelines {
    main: [u32; 2],
    shadow: Option<[u32; 2]>,
    prepass: Option<[u32; 2]>,
    prewrite: Option<[u32; 2]>,
    outline: Option<[u32; 2]>,
}

#[derive(Default)]
pub(crate) struct MaterialCache {
    entries: HashMap<u64, MaterialEntry>,
    scratch: Vec<u8>,
    /// Some entry sways in the wind (`wind_reach` > 0).
    any_wind: bool,
    /// Some entry pads its culling bounds (`bounds_padding` > 0).
    any_padding: bool,
}

impl MaterialCache {
    pub(crate) fn get(&self, id: u64) -> Option<&MaterialEntry> {
        self.entries.get(&id)
    }

    /// True when some material sways in the wind, so object bounds need
    /// widening by their sway.
    pub(crate) fn any_wind(&self) -> bool {
        self.any_wind
    }

    /// True when some material pads its culling bounds.
    pub(crate) fn any_padding(&self) -> bool {
        self.any_padding
    }

    /// Builds the default material (and its pipelines) if missing.
    pub(crate) fn ensure_default(&mut self, cx: &mut MaterialContext<'_>) {
        if !self.entries.contains_key(&DEFAULT_MATERIAL) {
            let inputs = MaterialInputs {
                standard: None,
                shader: None,
                lookup: None,
            };
            let entry = self.build_standard(cx, &StandardMaterial::default(), &inputs, None);
            self.entries.insert(DEFAULT_MATERIAL, entry);
        }
    }

    /// Brings every material entry up to date and frees entries of removed
    /// materials. An unchanged material costs a map lookup and a few integer
    /// compares.
    #[profiling::function]
    pub(crate) fn sync(&mut self, cx: &mut MaterialContext<'_>, inputs: MaterialInputs<'_>) {
        let lookup_gen = inputs.lookup_generation();
        let texture_gen = cx.textures.generation();
        let before = self.entries.len();
        let mut rebuilt = false;
        let mut live = 1; // the default material
        if let Some(standard) = inputs.standard {
            live += standard.len();
            for (id, version, material) in standard.iter() {
                let has_refs = material.has_textures();
                let stamp = Stamp {
                    version,
                    lookup: if has_refs { lookup_gen } else { 0 },
                    textures: if has_refs { texture_gen } else { 0 },
                    shader: None,
                };
                if self.entries.get(&id).is_some_and(|e| e.stamp == stamp) {
                    continue;
                }
                let old = self.entries.remove(&id);
                let mut entry = self.build_standard(cx, material, &inputs, old);
                entry.stamp = stamp;
                self.entries.insert(id, entry);
                rebuilt = true;
            }
        }
        if let Some(shader) = inputs.shader {
            live += shader.len();
            for (id, version, material) in shader.iter() {
                let current = self.entries.get(&id);
                // The resolved shader is re-resolved only when the material
                // or the lookup changes; its compile state is checked always.
                let resolved = match current {
                    Some(e) if e.stamp.version == version && e.stamp.lookup == lookup_gen => {
                        e.stamp.shader.map(|(sid, _)| sid)
                    }
                    _ => inputs.resolve_shader(&material.shader),
                };
                let stamp = Stamp {
                    version,
                    lookup: lookup_gen,
                    textures: 0,
                    shader: resolved.map(|sid| (sid, cx.shaders.get(sid).map_or(0, |s| s.state))),
                };
                if current.is_some_and(|e| e.stamp == stamp) {
                    continue;
                }
                let old = self.entries.remove(&id);
                let mut entry = self.build_shader(cx, material, resolved, old);
                entry.stamp = stamp;
                self.entries.insert(id, entry);
                rebuilt = true;
            }
        }
        if self.entries.len() > live {
            self.entries.retain(|&id, _| {
                id == DEFAULT_MATERIAL
                    || inputs.standard.is_some_and(|a| a.get_by_id(id).is_some())
                    || inputs.shader.is_some_and(|a| a.get_by_id(id).is_some())
            });
        }
        if rebuilt || self.entries.len() != before {
            self.any_wind = self.entries.values().any(|e| e.wind_reach > 0.0);
            self.any_padding = self.entries.values().any(|e| e.bounds_padding > 0.0);
        }
    }

    fn build_standard(
        &mut self,
        cx: &mut MaterialContext<'_>,
        m: &StandardMaterial,
        inputs: &MaterialInputs<'_>,
        old: Option<MaterialEntry>,
    ) -> MaterialEntry {
        let textures = cx.textures;
        let view = |r: &AssetRef, fallback| {
            inputs
                .resolve_image(r)
                .and_then(|id| textures.view(id))
                .unwrap_or_else(|| textures.default_view(fallback))
        };
        let has_normal_map = inputs
            .resolve_image(&m.normal_texture)
            .is_some_and(|id| textures.view(id).is_some());
        let alpha = AlphaKey::from(m.alpha_mode);
        let depth_fade = if alpha.is_transparent() && m.depth_fade > 0.0 {
            m.depth_fade
        } else {
            0.0
        };
        let wind = m.wind.is_on();
        let (vertex_alpha_weight, bottom, top) = match m.wind.weight {
            WindWeight::HeightRamp { bottom, top } => (false, bottom, top),
            WindWeight::VertexAlpha => (true, 0.0, 0.0),
        };
        let mut flags = 0;
        if has_normal_map {
            flags |= MATERIAL_FLAG_NORMAL_MAP;
        }
        if wind && vertex_alpha_weight {
            flags |= MATERIAL_FLAG_WIND_VERTEX_ALPHA;
        }
        let uniform = StandardMaterialUniform {
            base_color: m.base_color.to_array(),
            emissive: m.emissive.to_array(),
            params: [
                m.roughness,
                m.metallic,
                match m.alpha_mode {
                    AlphaMode::Mask { cutoff } => cutoff,
                    _ => 0.0,
                },
                depth_fade,
            ],
            flags: [flags, 0, 0, 0],
            wind: if wind {
                [m.wind.strength, m.wind.frequency.max(0.0), bottom, top]
            } else {
                [0.0; 4]
            },
            uv: {
                let s = m.uv_scale;
                let ok = |v: f32| if v.is_finite() { v } else { 1.0 };
                [ok(s.x), ok(s.y), 0.0, 0.0]
            },
        };
        let buffer = uniform_buffer(cx, old.and_then(|e| e.buffer), bytemuck::bytes_of(&uniform));
        let bind_group = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("standard material bind group"),
            layout: &cx.layouts.standard_material,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view(
                        &m.base_color_texture,
                        DefaultTexture::White,
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(view(
                        &m.normal_texture,
                        DefaultTexture::Normal,
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(view(
                        &m.orm_texture,
                        DefaultTexture::Orm,
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(view(
                        &m.emissive_texture,
                        DefaultTexture::White,
                    )),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&textures.sampler),
                },
            ],
        });
        let moves = Moves {
            wind,
            ..Moves::default()
        };
        let recipe = Recipe {
            shader: ShaderKey::Standard,
            alpha: m.alpha_mode,
            double_sided: m.double_sided,
            moves,
            depth: MaterialDepth::resolve(alpha, m.depth_write, m.depth_test, m.prewrite_depth),
        };
        let pipelines = cx.pipelines_for(&recipe, false);
        if let Err(e) = &pipelines {
            tracing::error!("standard material pipeline creation failed: {e}");
        }
        let (main, shadow, prepass, prewrite, outline) = pipelines
            .map_or((None, None, None, None, None), |p| {
                (Some(p.main), p.shadow, p.prepass, p.prewrite, p.outline)
            });
        MaterialEntry {
            bind_group: Some(bind_group),
            main,
            shadow,
            shadow_binds_material: alpha == AlphaKey::Mask || wind,
            prepass,
            prewrite,
            outline,
            transparent: alpha.is_transparent(),
            double_sided: m.double_sided,
            sort_bias: m.sort_bias,
            scene_depth: depth_fade > 0.0,
            bounds_padding: 0.0,
            wind_reach: if wind { m.wind.strength } else { 0.0 },
            skinned: None,
            recipe: Some(recipe),
            stamp: Stamp::default(),
            buffer: Some(buffer),
        }
    }

    /// A `ShaderMaterial` whose shader resolved to asset `shader`: drawn by
    /// it when compiled, with the error material when it failed, and as the
    /// default material while it is not loaded.
    fn build_shader(
        &mut self,
        cx: &mut MaterialContext<'_>,
        m: &ShaderMaterial,
        shader: Option<u64>,
        old: Option<MaterialEntry>,
    ) -> MaterialEntry {
        let state = shader.and_then(|id| cx.shaders.get(id).map(|s| (id, s)));
        let Some((id, custom)) = state else {
            let inputs = MaterialInputs {
                standard: None,
                shader: None,
                lookup: None,
            };
            return self.build_standard(cx, &StandardMaterial::default(), &inputs, old);
        };
        let Some(compiled) = &custom.compiled else {
            return self.error_entry(cx, m.double_sided);
        };
        pack_params(
            &compiled.params,
            compiled.uniform_size,
            m,
            &mut self.scratch,
        );
        let buffer = uniform_buffer(cx, old.and_then(|e| e.buffer), &self.scratch);
        let bind_group = cx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader material bind group"),
            layout: &cx.layouts.custom_material,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let alpha = AlphaKey::from(m.alpha_mode);
        let depth_fade = if alpha.is_transparent() && m.depth_fade > 0.0 {
            m.depth_fade
        } else {
            0.0
        };
        let moves = Moves {
            wind: false,
            vertex_hook: compiled.has_vertex,
            depth_fade,
        };
        let recipe = Recipe {
            shader: ShaderKey::Custom(id),
            alpha: m.alpha_mode,
            double_sided: m.double_sided,
            moves,
            depth: MaterialDepth::resolve(alpha, m.depth_write, m.depth_test, m.prewrite_depth),
        };
        let padding = m.bounds_padding;
        match cx.pipelines_for(&recipe, false) {
            Ok(p) => MaterialEntry {
                bind_group: Some(bind_group),
                main: Some(p.main),
                shadow: p.shadow,
                shadow_binds_material: true,
                prepass: p.prepass,
                prewrite: p.prewrite,
                outline: p.outline,
                transparent: alpha.is_transparent(),
                double_sided: m.double_sided,
                sort_bias: m.sort_bias,
                scene_depth: depth_fade > 0.0 || m.scene_depth || compiled.reads_scene_depth,
                bounds_padding: if padding.is_finite() {
                    padding.max(0.0)
                } else {
                    0.0
                },
                wind_reach: 0.0,
                skinned: None,
                recipe: Some(recipe),
                stamp: Stamp::default(),
                buffer: Some(buffer),
            },
            Err(e) => {
                tracing::error!(
                    "material shader pipeline creation failed; drawing the error material\n{e}"
                );
                self.error_entry(cx, m.double_sided)
            }
        }
    }

    /// The magenta error material; it casts opaque shadows.
    fn error_entry(&mut self, cx: &mut MaterialContext<'_>, double_sided: bool) -> MaterialEntry {
        let recipe = Recipe {
            shader: ShaderKey::Error,
            alpha: AlphaMode::Opaque,
            double_sided,
            moves: Moves::default(),
            depth: MaterialDepth::default_for(AlphaKey::Opaque),
        };
        let pipelines = cx.pipelines_for(&recipe, false);
        let shadow = [false, true].map(|mirrored| {
            cx.pipeline(PipelineKey::shadow(
                ShaderKey::Standard,
                false,
                double_sided,
                mirrored,
            ))
        });
        let shadow = match shadow {
            [Ok(a), Ok(b)] => Some([a, b]),
            _ => None,
        };
        let pipelines = pipelines.ok();
        MaterialEntry {
            bind_group: None,
            main: pipelines.as_ref().map(|p| p.main),
            shadow,
            shadow_binds_material: false,
            prepass: pipelines.and_then(|p| p.prepass),
            prewrite: None,
            outline: None,
            transparent: false,
            double_sided,
            sort_bias: 0,
            scene_depth: false,
            bounds_padding: 0.0,
            wind_reach: 0.0,
            skinned: None,
            recipe: Some(recipe),
            stamp: Stamp::default(),
            buffer: None,
        }
    }

    /// Builds the skinned pipelines of material `id` if it has none yet.
    /// Cheap when they exist (a map lookup).
    pub(crate) fn ensure_skinned(&mut self, cx: &mut MaterialContext<'_>, id: u64) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if entry.skinned.is_some() {
            return;
        }
        let Some(r) = entry.recipe else {
            entry.skinned = Some(SkinnedPipelines::default());
            return;
        };
        let built = cx.pipelines_for(&r, true);
        let mut skinned = match built {
            Ok(p) => SkinnedPipelines {
                main: Some(p.main),
                shadow: p.shadow,
                prepass: p.prepass,
                prewrite: p.prewrite,
                outline: p.outline,
            },
            Err(e) => {
                tracing::error!("skinned material pipeline creation failed: {e}");
                SkinnedPipelines::default()
            }
        };
        // The error material casts standard opaque shadows.
        if r.shader == ShaderKey::Error {
            let shadow = [false, true].map(|mirrored| {
                cx.pipeline(
                    PipelineKey::shadow(ShaderKey::Standard, false, r.double_sided, mirrored)
                        .skinned(),
                )
            });
            skinned.shadow = match shadow {
                [Ok(a), Ok(b)] => Some([a, b]),
                _ => None,
            };
        }
        entry.skinned = Some(skinned);
    }

    /// Material entries (including the default material).
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// `old` rewritten with `bytes` when it has the same size, else a new
/// uniform buffer holding them.
fn uniform_buffer(
    cx: &mut MaterialContext<'_>,
    old: Option<wgpu::Buffer>,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buffer = match old {
        Some(b) if b.size() == bytes.len() as u64 => b,
        _ => cx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("material uniform"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
    };
    cx.queue.write_buffer(&buffer, 0, bytes);
    cx.upload.bytes += bytes.len() as u64;
    buffer
}
