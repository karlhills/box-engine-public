//! The forward renderer: device setup, per-frame extraction and pass encoding.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::image::ImageData;
use box_core::math::Aabb;
use box_core::mesh::MeshData;
use box_core::time::Time;
use box_core::transform::GlobalTransform;
use box_core::world::{Entity, World};
use glam::camera::rh::proj::directx;
use glam::{Mat4, Quat, Vec3, Vec4};

use crate::cascades::{self, CASCADE_COUNT, CameraView, CasterCuller, SHADOW_MAP_SIZE};
use crate::components::{
    AmbientVolume, Camera, DirectionalLight, Environment, MeshRenderer, PointLight, RenderSettings,
    SelectionOutline, ShaderMaterial, ShaderSource, SpotLight, StandardMaterial, Tonemapper,
};
use crate::culling::Frustum;
use crate::debug_draw::{DebugDraw, LineLayer};
use crate::gpu_mesh::{MeshCache, UploadStats};
use crate::instances::{Batch, InstanceStore, batch};
use crate::lights::{self, LIGHT_BUFFER_SIZE, LightCandidate, MAX_LOCAL_LIGHTS, ShadowHeader};
use crate::material_shader::CustomShaders;
use crate::materials::{DEFAULT_MATERIAL, MaterialCache, MaterialContext, MaterialInputs};
use crate::overlay::OverlayPass;
use crate::particle_render::ParticleRenderer;
use crate::pipelines::{
    AO_DEPTH_FORMAT, AO_FORMAT, DEPTH_FORMAT, Layouts, MSAA_SAMPLES, OUTLINE_MASK_FORMAT,
    OUTLINE_UNIFORM_SIZE, Pass, PipelineCache, Pipelines, SHADOW_FORMAT, ShaderKey,
    create_material_pipeline, engine_module,
};
use crate::post::{self, AoTargets, BloomTargets};
use crate::shaders::{self, ShaderLibrary, ShaderSources};
use crate::skinning::JointPalettes;
use crate::textures::TextureCache;
use crate::uniforms::{GlobalsUniform, LineVertex, UNIFORM_SLOT};
use crate::uniforms::{GpuAmbientVolume, LOCAL_SHADOW_VIEWS, MAX_AMBIENT_VOLUMES};
use crate::viewport::{VIEWPORT_FORMAT, ViewCamera, ViewportTarget};

/// Format of offscreen capture targets.
const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// The capture target's plain view, where the UI overlay draws.
const CAPTURE_UI_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Normal-offset shadow bias in shadow texels.
const SHADOW_NORMAL_OFFSET: f32 = 1.5;
/// Constant shadow depth bias in shadow texels (converted per cascade).
const SHADOW_DEPTH_BIAS: f32 = 1.0;
/// Fraction of each cascade over which it blends into the next.
const CASCADE_BLEND_BAND: f32 = 0.15;
/// Device features the renderer uses when the adapter has them: depth clamp
/// for shadow casters between the light and a cascade's box.
const OPTIONAL_FEATURES: wgpu::Features =
    wgpu::Features::DEPTH_CLIP_CONTROL.union(wgpu::Features::TIMESTAMP_QUERY);
/// Passes [`Renderer::set_gpu_timing`] can time per frame.
const MAX_TIMED_PASSES: u32 = 48;

/// Why the renderer could not be created or could not render.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("{}", no_adapter_text())]
    NoAdapter,
    #[error("failed to create the GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("surface error: {0}")]
    Surface(String),
    #[error("shader error: {0}")]
    Shader(String),
    #[error("capture failed: {0}")]
    Capture(String),
    /// The GPU device stopped working (driver reset, GPU removed or hung).
    /// Nothing more can be drawn with this renderer.
    #[error(
        "the GPU device was lost ({0}). The graphics driver reset or the GPU stopped responding; restart the app, and update the graphics driver if it happens again"
    )]
    DeviceLost(String),
}

/// The message of [`RenderError::NoAdapter`]: which backends were tried and
/// what to do.
fn no_adapter_text() -> String {
    let forced = std::env::var("WGPU_BACKEND").ok().filter(|v| !v.is_empty());
    let needs = if cfg!(target_os = "macos") {
        "Metal (macOS 11 or later)"
    } else if cfg!(windows) {
        "Direct3D 12 or Vulkan"
    } else {
        "Vulkan"
    };
    match forced {
        Some(b) => format!(
            "no GPU adapter found for WGPU_BACKEND={b}; box-engine needs {needs}. Unset WGPU_BACKEND to use the default backend"
        ),
        None => format!(
            "no GPU adapter found: box-engine needs a GPU with {needs}. Update the graphics driver; in a virtual machine or remote session, enable GPU acceleration"
        ),
    }
}

/// Per-frame statistics (except `validation_errors`, which is cumulative).
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    /// Entities with a `MeshRenderer` whose mesh is uploaded.
    pub objects: u32,
    /// Objects that passed frustum culling.
    pub visible: u32,
    /// Draw calls submitted (meshes in all passes, sky, lines, tone mapping,
    /// selection outline).
    pub draws: u32,
    /// Mesh draws into the shadow cascades (included in `draws`).
    pub shadow_draws: u32,
    /// Objects drawn into at least one shadow cascade or local shadow view.
    /// Casters are culled per cascade (light-space box) and per view.
    pub shadow_casters: u32,
    /// Objects drawn skinned (RS4): a `SkinnedMesh` with a loaded skin on
    /// a mesh with joints.
    pub skinned: u32,
    /// Joint palette matrices uploaded for skinned objects this frame.
    pub joint_matrices: u32,
    /// Triangles submitted by mesh draws in all passes.
    pub triangles: u64,
    /// Instances drawn by mesh draws in all passes (an instanced draw of
    /// `n` objects counts `n`).
    pub instances: u32,
    /// Draws in the sorted transparent pass (included in `draws`).
    pub transparent_draws: u32,
    /// Point lights in the light buffer (visible; point and spot lights
    /// together at most [`MAX_LOCAL_LIGHTS`](crate::MAX_LOCAL_LIGHTS)).
    pub point_lights: u32,
    /// Spot lights in the light buffer.
    pub spot_lights: u32,
    /// Views rendered for the shadowed local light: 0 (none), 1 (spot) or
    /// 6 (point).
    pub local_shadow_views: u32,
    /// Mesh draws into the local light's shadow views (included in `draws`).
    pub local_shadow_draws: u32,
    /// Ambient volumes counted this frame (at most 8).
    pub ambient_volumes: u32,
    /// Particles drawn (instances of the particle draws).
    pub particles: u32,
    /// Particle emitters drawn (one instanced draw each, included in
    /// `draws` and `transparent_draws`).
    pub particle_emitters: u32,
    /// Material pipelines in the cache (all keys created so far).
    pub material_pipelines: u32,
    /// Bytes written with `Queue::write_buffer` this frame.
    pub upload_bytes: u64,
    /// GPU buffers re-created because they had to grow this frame.
    pub buffer_reallocations: u32,
    /// wgpu validation errors since the renderer was created.
    pub validation_errors: u32,
    pub cpu_render_ms: f32,
    /// CPU time from the start of the frame until its commands were
    /// submitted: extraction, uploads, encoding and submission, without
    /// waiting for the GPU or presenting.
    pub cpu_submit_ms: f32,
    /// GPU time from the start to the end of each render pass, `(label,
    /// milliseconds)` in submission order (one entry per pass; the shadow
    /// cascades are four passes). Tile-based GPUs (Apple) overlap
    /// independent passes, so these intervals can overlap and do not add
    /// up. Empty unless [`Renderer::set_gpu_timing`] is on and the adapter
    /// supports timestamp queries.
    pub gpu_passes: Vec<(&'static str, f32)>,
    /// GPU time of the frame: from the start of its first pass to the end
    /// of its last.
    pub gpu_ms: f32,
    /// False when the last `render` call skipped the frame (for example
    /// because the window is occluded) and nothing was drawn or presented.
    pub presented: bool,
}

struct SurfaceState {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// The sRGB view format the tone-mapping pass writes through.
    view_format: wgpu::TextureFormat,
}

/// A window surface texture acquired with
/// [`Renderer::acquire_surface_frame`]. Present it with
/// [`Renderer::present_surface_frame`].
pub struct SurfaceFrame {
    texture: wgpu::SurfaceTexture,
    /// A view in [`Renderer::surface_format`].
    pub view: wgpu::TextureView,
    suboptimal: bool,
}

enum Acquired {
    Frame(wgpu::SurfaceTexture, bool),
    /// Occluded, timed out, or outdated (and reconfigured): skip this frame.
    Skip,
}

/// Where [`Renderer::draw_frame`] puts the image.
enum FrameTarget<'a> {
    /// The renderer's own size-dependent buffers, tone mapped into `output`.
    Main {
        output: &'a wgpu::TextureView,
        format: wgpu::TextureFormat,
        /// A non-sRGB view of the same texture where available, for the UI
        /// overlay (blended in sRGB space, as UI is designed).
        ui: (&'a wgpu::TextureView, wgpu::TextureFormat),
    },
    Viewport(&'a ViewportTarget),
}

/// Size-dependent render targets.
pub(crate) struct Targets {
    _msaa: wgpu::Texture,
    msaa_view: wgpu::TextureView,
    _hdr: wgpu::Texture,
    hdr_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    tonemap_bind_group: wgpu::BindGroup,
    /// Scene depth for depth-tested display lines.
    display_bind_group: wgpu::BindGroup,
    size: (u32, u32),
    /// Selection outline mask, created the first time an outline is drawn.
    outline: OnceLock<OutlineTargets>,
    /// Prepass and ambient occlusion targets, created the first time
    /// ambient occlusion is drawn.
    ao: OnceLock<AoTargets>,
    /// Bloom mip chain, created the first time bloom is drawn.
    bloom: OnceLock<BloomTargets>,
}

/// The selection outline's mask and the composite's bind group.
struct OutlineTargets {
    _mask: wgpu::Texture,
    mask_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

struct CaptureTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Plain (non-sRGB) view for the UI overlay.
    ui_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_row: u32,
    size: (u32, u32),
}

struct FrameResources {
    globals_buffer: wgpu::Buffer,
    /// Group 0 of scene passes, with 1x1 placeholder AO textures (the main
    /// pass binds `AoTargets::frame_bind_group` while AO is on).
    frame_bind_group: wgpu::BindGroup,
    shadow_array_view: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    /// Linear clamp sampler (bloom).
    linear_sampler: wgpu::Sampler,
    /// 1x1 placeholders bound while ambient occlusion or bloom are off.
    _placeholders: Vec<wgpu::Texture>,
    placeholder_bloom: wgpu::TextureView,
    _shadow_texture: wgpu::Texture,
    shadow_layer_views: Vec<wgpu::TextureView>,
    /// Light matrices of the shadow passes, one 256-byte slot each: the
    /// cascades, then the local light's views.
    shadow_pass_buffer: wgpu::Buffer,
    shadow_pass_bind_group: wgpu::BindGroup,
    /// The counted local light buffer (`Lights` in common.wgsl).
    light_buffer: wgpu::Buffer,
    /// The shadowed local light's map, allocated on first use and when its
    /// resolution or view count grows.
    local_shadow: Option<LocalShadowMap>,
    /// 1x1 single-layer stand-in bound until then.
    placeholder_local_shadow: wgpu::TextureView,
    placeholder_ao: wgpu::TextureView,
    placeholder_ao_depth: wgpu::TextureView,
    placeholder_depth: wgpu::TextureView,
    /// Bumped whenever a binding shared by every frame bind group changes
    /// (the local shadow map), so cached frame bind groups are remade.
    frame_generation: u64,
    debug_buffer: Option<wgpu::Buffer>,
    /// `Outline` in outline.wgsl; written only on frames with an outline.
    outline_buffer: wgpu::Buffer,
}

/// No pipeline: the draw is skipped in that pass.
const NO_PIPELINE: u32 = u32::MAX;

/// One material group of one object.
#[derive(Clone, Copy, Debug)]
struct DrawItem {
    /// The object's instance slot.
    slot: u32,
    mesh: u64,
    first_index: u32,
    index_count: u32,
    /// Material cache id ([`DEFAULT_MATERIAL`] for empty slots).
    material: u64,
    /// `material` when the shadow pipeline reads the material (alpha mask,
    /// material shaders), else 0, so shadow draws of different materials
    /// batch together.
    shadow_material: u64,
    /// Main-pass pipeline, by the object's winding: a transform that mirrors
    /// the mesh (negative determinant) reverses its triangles, so it uses the
    /// clockwise-front pipelines.
    main_pipeline: u32,
    /// Shadow pipeline when the group casts into any cascade.
    shadow_pipeline: u32,
    /// Prepass pipeline when the group is visible and opaque or
    /// alpha-masked (used while the prepass runs: ambient occlusion, or a
    /// visible depth-fading material). It binds `shadow_material`.
    prepass_pipeline: u32,
    /// Outline mask pipeline of a material that moves vertices, when the
    /// object is outlined; [`NO_PIPELINE`] uses the fixed `outline` variant.
    /// It binds `material`.
    outline_pipeline: u32,
    transparent: bool,
    /// Bit `i`: drawn into shadow cascade `i`.
    cascades: u8,
    /// Bit `i`: drawn into view `i` of the shadowed local light.
    local_views: u8,
    /// View depth of the object's bounds centre (transparent sort key).
    depth: f32,
    /// Outline mask pipeline (0 front ccw, 1 front cw, 2 no culling) when
    /// the object is selected and visible.
    outline: Option<u8>,
    /// Drawn skinned: the pipelines are skinned variants and the mesh's
    /// joint buffer is bound as vertex buffer 1.
    skinned: bool,
}

/// Back to front by view depth; ties keep their order (stable sort).
fn sort_transparent(order: &mut [u32], draws: &[DrawItem]) {
    order.sort_by(|&a, &b| draws[b as usize].depth.total_cmp(&draws[a as usize].depth));
}

/// The batching key of a draw: draws with equal keys are one instanced
/// draw. Pipeline first, then material, then mesh, so state changes are rare.
type DrawKey = (u32, u64, u64, u32, u32);

/// By [`DrawKey`] (`(pipeline, material)` from `state`), then submission order.
fn sort_by_state(order: &mut [u32], draws: &[DrawItem], state: impl Fn(&DrawItem) -> (u32, u64)) {
    order.sort_unstable_by_key(|&i| (draw_key(&draws[i as usize], &state), i));
}

fn draw_key(d: &DrawItem, state: impl Fn(&DrawItem) -> (u32, u64)) -> DrawKey {
    let (pipeline, material) = state(d);
    (pipeline, material, d.mesh, d.first_index, d.index_count)
}

fn main_state(d: &DrawItem) -> (u32, u64) {
    (d.main_pipeline, d.material)
}

fn shadow_state(d: &DrawItem) -> (u32, u64) {
    (d.shadow_pipeline, d.shadow_material)
}

fn prepass_state(d: &DrawItem) -> (u32, u64) {
    (d.prepass_pipeline, d.shadow_material)
}

fn outline_state(d: &DrawItem) -> (u32, u64) {
    if d.outline_pipeline == NO_PIPELINE {
        (NO_PIPELINE, 0)
    } else {
        (d.outline_pipeline, d.material)
    }
}

/// What `prepare` decided for this frame.
struct FramePlan {
    shadows: bool,
    /// Views of the shadowed local light to render (0 none).
    local_views: usize,
    /// Debug line vertices per [`LineLayer`], stored back to back in the
    /// debug buffer in declaration order.
    lines: [u32; 4],
    /// Scissor rectangle (x, y, width, height) of the selection outline
    /// composite; `None` when no selected object is visible.
    outline: Option<[u32; 4]>,
    /// Run the depth + normal prepass (ambient occlusion, or a visible
    /// depth-fading material).
    prepass: bool,
    /// Run ambient occlusion (implies `prepass`).
    ao: bool,
    /// Bloom scatter (the upsample blend), when bloom is on.
    bloom: Option<f32>,
}

impl FramePlan {
    fn line_range(&self, layer: LineLayer) -> std::ops::Range<u32> {
        let i = layer as usize;
        let start: u32 = self.lines[..i].iter().sum();
        start..start + self.lines[i]
    }
}

/// The forward renderer. Owns the GPU device and every GPU resource.
pub struct Renderer {
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    adapter_info: wgpu::AdapterInfo,
    /// Set by the device-lost callback; see [`Renderer::device_lost`].
    lost: Arc<std::sync::Mutex<Option<String>>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: Option<SurfaceState>,
    size: (u32, u32),
    hdr_format: wgpu::TextureFormat,
    output_formats: Vec<wgpu::TextureFormat>,
    layouts: Layouts,
    pipelines: Pipelines,
    pipeline_generation: u64,
    shaders: ShaderLibrary,
    /// Buffers for [`render`](Self::render) and [`capture`](Self::capture) at
    /// `size`. Created on first use, so a renderer that only draws into
    /// [`ViewportTarget`]s never allocates them.
    targets: Option<Targets>,
    capture_target: Option<CaptureTarget>,
    frame: FrameResources,
    meshes: MeshCache,
    textures: TextureCache,
    /// Game UI drawn over the tone-mapped image (overlay.rs).
    overlay: OverlayPass,
    materials: MaterialCache,
    pipeline_cache: PipelineCache,
    custom_shaders: CustomShaders,
    /// Engine library sources material shaders are composed with (from the
    /// last engine shader build that succeeded).
    library: ShaderSources,
    stats: FrameStats,
    error_count: Arc<AtomicU32>,
    draw_list: Vec<DrawItem>,
    /// Draw indices of the opaque and alpha-tested main pass, sorted by state.
    main_order: Vec<u32>,
    /// Draw indices of the transparent pass, back to front.
    transparent_order: Vec<u32>,
    /// Draw indices of shadow casters, sorted by state.
    shadow_order: Vec<u32>,
    /// Draw indices of visible selected objects, drawn into the outline mask.
    outline_order: Vec<u32>,
    /// Outline id per selected entity (1 primary, 0.5 secondary).
    outline_ids: HashMap<Entity, f32>,
    /// Persistent per-object instance data and the frame's draw slot list.
    instances: InstanceStore,
    /// The frame's joint palettes of skinned objects (RS4).
    skins: JointPalettes,
    /// Instanced draws of the opaque main pass.
    main_batches: Vec<Batch>,
    /// Draw indices of the depth + normal prepass, sorted by state.
    prepass_order: Vec<u32>,
    /// Instanced draws of the prepass.
    prepass_batches: Vec<Batch>,
    /// Transparent draws, one instance each, back to front.
    transparent_batches: Vec<Batch>,
    /// Instanced draws per shadow cascade.
    shadow_batches: [Vec<Batch>; CASCADE_COUNT],
    /// Instanced draws per view of the shadowed local light.
    local_shadow_batches: [Vec<Batch>; LOCAL_SHADOW_VIEWS],
    /// Instanced draws into the selection outline mask.
    outline_batches: Vec<Batch>,
    lights: Vec<LightCandidate>,
    light_data: Vec<u8>,
    /// Visible ambient volumes this frame (with their distance to the eye).
    volumes: Vec<(f32, GpuAmbientVolume)>,
    /// Particle instances, draws and textures.
    particles: ParticleRenderer,
    /// Per-pass GPU timestamps ([`set_gpu_timing`](Self::set_gpu_timing)).
    gpu_timer: Option<GpuTimer>,
    /// When the current frame's `draw_frame` started.
    frame_start: Instant,
    /// Milliseconds from `frame_start` to the frame's submission (f32 bits;
    /// `submit` takes `&self`).
    submit_ms: AtomicU32,
}

/// Timestamp queries at the start and end of each render pass.
struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Labels of this frame's timed passes; pass `i` writes queries 2i and
    /// 2i + 1. Filled while encoding (`encode` takes `&self`).
    labels: std::sync::Mutex<Vec<&'static str>>,
    /// Nanoseconds per timestamp tick.
    period_ns: f32,
}

impl Renderer {
    /// Creates a renderer presenting to a window surface. `width`/`height` are
    /// physical pixels.
    pub fn new_windowed(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let instance = create_instance();
        let surface = instance
            .create_surface(target)
            .map_err(|e| RenderError::Surface(e.to_string()))?;
        let adapter = pick_adapter(&instance, Some(&surface))?;
        let caps = surface.get_capabilities(&adapter);
        // The swapchain stores a non-sRGB format that has an sRGB twin, and the
        // tone-mapping pass writes through the sRGB view, so the encoded bytes
        // are the same as with an sRGB swapchain. UI layers such as egui-wgpu
        // prefer the plain (gamma) view of the same texture; see
        // `surface_format`. Platforms without such a pair fall back to an sRGB
        // format.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb() && f.add_srgb_suffix() != *f)
            .or_else(|| caps.formats.iter().copied().find(|f| f.is_srgb()))
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| RenderError::Surface("surface reports no formats".into()))?;
        let view_format = format.add_srgb_suffix();
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or_else(|| RenderError::Surface("surface not supported by adapter".into()))?;
        config.format = format;
        config.present_mode = wgpu::PresentMode::Fifo;
        if view_format != format {
            config.view_formats = vec![view_format];
        }
        let surface = SurfaceState {
            surface,
            config,
            view_format,
        };
        Self::new_inner(
            instance,
            adapter,
            Some(surface),
            width,
            height,
            OPTIONAL_FEATURES,
        )
    }

    /// Creates a renderer without a window; frames go to an offscreen target.
    /// Returns [`RenderError::NoAdapter`] when no GPU is available.
    pub fn new_headless(width: u32, height: u32) -> Result<Self, RenderError> {
        let instance = create_instance();
        let adapter = pick_adapter(&instance, None)?;
        Self::new_inner(instance, adapter, None, width, height, OPTIONAL_FEATURES)
    }

    /// `optional_features` are enabled when the adapter supports them.
    fn new_inner(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        mut surface: Option<SurfaceState>,
        width: u32,
        height: u32,
        optional_features: wgpu::Features,
    ) -> Result<Self, RenderError> {
        let adapter_info = adapter.get_info();
        let hdr_format = choose_hdr_format(&adapter);
        let mut required_features = optional_features & adapter.features();
        if hdr_format == wgpu::TextureFormat::Rg11b10Ufloat {
            required_features |= wgpu::Features::RG11B10UFLOAT_RENDERABLE;
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("box_render device"),
                required_features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            }))?;
        // Details for bug reports at debug level, so every run does not
        // print the feature set.
        tracing::debug!(
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            device_type = ?adapter_info.device_type,
            driver = %adapter_info.driver,
            features = ?required_features,
            hdr_format = ?hdr_format,
            "renderer initialised"
        );

        let error_count = Arc::new(AtomicU32::new(0));
        let counter = error_count.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            counter.fetch_add(1, Ordering::Relaxed);
            if matches!(e, wgpu::Error::OutOfMemory { .. }) {
                tracing::error!(
                    "wgpu error: {e}. The GPU ran out of memory: use a smaller window or render size, fewer or smaller textures and shadow maps, or close other apps that use the GPU"
                );
            } else {
                tracing::error!("wgpu error: {e}");
            }
        }));
        // Only recorded here: the callback also runs when the device is
        // dropped. Frames check it (`check_device`) and return an error.
        let lost = Arc::new(std::sync::Mutex::new(None));
        let lost_slot = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            let text = match reason {
                wgpu::DeviceLostReason::Destroyed => format!("destroyed: {message}"),
                wgpu::DeviceLostReason::Unknown => message,
            };
            if let Ok(mut slot) = lost_slot.lock() {
                slot.get_or_insert(text);
            }
        });

        let size = clamp_size(&device, width, height);
        let mut output_formats = vec![CAPTURE_FORMAT];
        if let Some(s) = &mut surface {
            (s.config.width, s.config.height) = size;
            s.surface.configure(&device, &s.config);
            if s.view_format != CAPTURE_FORMAT {
                output_formats.push(s.view_format);
            }
        }

        let layouts = Layouts::new(&device);
        let (shaders, processed) = ShaderLibrary::new(shaders::hot_reload_dir());
        let pipelines = catch_errors(&device, || {
            Pipelines::new(&device, &layouts, &processed, hdr_format, &output_formats)
        })
        .map_err(RenderError::Shader)?;
        let frame = FrameResources::new(&device, &layouts, hdr_format);
        let skins = JointPalettes::new(&device);
        let instances = InstanceStore::new(&device, &layouts, skins.buffer());
        let textures = TextureCache::new(&device, &queue);
        let overlay = OverlayPass::new(
            &device,
            &[
                CAPTURE_FORMAT,
                CAPTURE_UI_FORMAT,
                VIEWPORT_FORMAT,
                surface
                    .as_ref()
                    .map_or(CAPTURE_UI_FORMAT, |s| s.config.format),
            ],
        );
        let custom_shaders = CustomShaders::default();
        let mut pipeline_cache = PipelineCache::default();
        let mut materials = MaterialCache::default();
        materials.ensure_default(&mut MaterialContext {
            device: &device,
            queue: &queue,
            layouts: &layouts,
            textures: &textures,
            shaders: &custom_shaders,
            modules: &pipelines.modules,
            pipelines: &mut pipeline_cache,
            hdr_format,
            upload: &mut UploadStats::default(),
        });
        let particles = ParticleRenderer::new(&device, &queue);

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            adapter_info,
            lost,
            device,
            queue,
            surface,
            size,
            hdr_format,
            output_formats,
            layouts,
            pipelines,
            pipeline_generation: 0,
            shaders,
            targets: None,
            capture_target: None,
            frame,
            meshes: MeshCache::default(),
            textures,
            materials,
            pipeline_cache,
            custom_shaders,
            overlay,
            library: processed.sources,
            stats: FrameStats::default(),
            error_count,
            draw_list: Vec::new(),
            main_order: Vec::new(),
            transparent_order: Vec::new(),
            shadow_order: Vec::new(),
            outline_order: Vec::new(),
            outline_ids: HashMap::new(),
            main_batches: Vec::new(),
            prepass_order: Vec::new(),
            prepass_batches: Vec::new(),
            transparent_batches: Vec::new(),
            shadow_batches: Default::default(),
            local_shadow_batches: Default::default(),
            outline_batches: Vec::new(),
            lights: Vec::new(),
            light_data: Vec::new(),
            volumes: Vec::new(),
            particles,
            gpu_timer: None,
            instances,
            skins,
            frame_start: Instant::now(),
            submit_ms: AtomicU32::new(0),
        })
    }

    /// Times every render pass on the GPU with timestamp queries; results
    /// are in [`FrameStats::gpu_passes`]. A measurement mode: each frame
    /// then waits for the GPU after submitting. Returns false (and stays
    /// off) when the adapter has no timestamp queries.
    pub fn set_gpu_timing(&mut self, on: bool) -> bool {
        if !on {
            self.gpu_timer = None;
            return false;
        }
        if !self
            .device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            return false;
        }
        if self.gpu_timer.is_none() {
            let count = MAX_TIMED_PASSES * 2;
            let size = count as u64 * 8;
            self.gpu_timer = Some(GpuTimer {
                query_set: self.device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("pass timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count,
                }),
                resolve: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp resolve"),
                    size,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("timestamp readback"),
                    size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                labels: Default::default(),
                period_ns: self.queue.get_timestamp_period(),
            });
        }
        true
    }

    /// Timestamp writes for the next timed pass, `None` when timing is off
    /// (or the frame's query budget is used up).
    fn timestamps(&self, label: &'static str) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let t = self.gpu_timer.as_ref()?;
        let mut labels = t.labels.lock().ok()?;
        let i = labels.len() as u32;
        if i >= MAX_TIMED_PASSES {
            return None;
        }
        labels.push(label);
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &t.query_set,
            beginning_of_pass_write_index: Some(2 * i),
            end_of_pass_write_index: Some(2 * i + 1),
        })
    }

    /// Resolves this frame's timestamps into the readback buffer.
    fn resolve_timestamps(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(t) = &self.gpu_timer else { return };
        let n = t.labels.lock().map_or(0, |l| l.len() as u32) * 2;
        if n == 0 {
            return;
        }
        encoder.resolve_query_set(&t.query_set, 0..n, &t.resolve, 0);
        encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.readback, 0, n as u64 * 8);
    }

    /// Waits for the GPU and reads the frame's pass timings into the stats.
    fn collect_timestamps(&mut self) {
        let Some(t) = &self.gpu_timer else { return };
        let labels = t.labels.lock().map(|l| l.clone()).unwrap_or_default();
        if labels.is_empty() {
            return;
        }
        let slice = t.readback.slice(..labels.len() as u64 * 16);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        if self
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
            || !matches!(rx.recv(), Ok(Ok(())))
        {
            tracing::warn!(target: "box_render", "GPU timestamps could not be read");
            return;
        }
        let mut passes = Vec::with_capacity(labels.len());
        let mut span = (u64::MAX, 0);
        if let Ok(data) = slice.get_mapped_range() {
            let ticks: &[u64] = bytemuck::cast_slice(&data);
            for (i, label) in labels.iter().enumerate() {
                let (begin, end) = (ticks[2 * i], ticks[2 * i + 1]);
                span = (span.0.min(begin), span.1.max(end));
                let ns = end.saturating_sub(begin) as f32 * t.period_ns;
                passes.push((*label, ns / 1.0e6));
            }
        }
        t.readback.unmap();
        self.stats.gpu_ms = span.1.saturating_sub(span.0) as f32 * t.period_ns / 1.0e6;
        self.stats.gpu_passes = passes;
    }

    /// Resizes the render targets (physical pixels). Zero sizes are ignored;
    /// sizes beyond the device's maximum texture size are clamped to it, so
    /// [`size`](Self::size) may be smaller than requested.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let (width, height) = clamp_size(&self.device, width, height);
        if (width, height) == self.size {
            return;
        }
        self.size = (width, height);
        if let Some(s) = &mut self.surface {
            s.config.width = width;
            s.config.height = height;
            s.surface.configure(&self.device, &s.config);
        }
        if self.targets.is_some() {
            self.targets = None;
            self.ensure_targets();
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn stats(&self) -> &FrameStats {
        &self.stats
    }

    /// Meshes with GPU buffers in the renderer's cache.
    pub fn cached_meshes(&self) -> usize {
        self.meshes.len()
    }

    /// Images uploaded as textures (default textures not included).
    pub fn cached_textures(&self) -> usize {
        self.textures.len()
    }

    /// Materials with GPU state, including the default material.
    pub fn cached_materials(&self) -> usize {
        self.materials.len()
    }

    /// Why the current version of a material shader failed to compile, if
    /// it did (it then draws with its previous version, or with the error
    /// material if no version compiled). Also logged once per version.
    pub fn shader_error(&self, shader: Handle<ShaderSource>) -> Option<&str> {
        self.custom_shaders.get(shader.id())?.error.as_deref()
    }

    /// Why the GPU device was lost, once it has been. Every frame after
    /// that fails with [`RenderError::DeviceLost`].
    pub fn device_lost(&self) -> Option<String> {
        self.lost.lock().ok().and_then(|l| l.clone())
    }

    fn check_device(&self) -> Result<(), RenderError> {
        match self.device_lost() {
            Some(reason) => Err(RenderError::DeviceLost(reason)),
            None => Ok(()),
        }
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// The GPU device. Tools that draw on top of the scene (for example an
    /// egui renderer) share it; errors they cause count in
    /// [`validation_errors`](Self::validation_errors).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// wgpu validation errors on this device since creation, including those
    /// caused by other users of [`device`](Self::device).
    pub fn validation_errors(&self) -> u32 {
        self.error_count.load(Ordering::Relaxed)
    }

    /// Format of [`SurfaceFrame::view`]: non-sRGB (for example `Bgra8Unorm`)
    /// when the platform offers one with an sRGB twin, which is what
    /// egui-wgpu prefers. The renderer's own [`render`](Self::render) writes
    /// through the sRGB twin of the same texture. `None` when headless.
    pub fn surface_format(&self) -> Option<wgpu::TextureFormat> {
        self.surface.as_ref().map(|s| s.config.format)
    }

    /// Processes shader hot reload, draws one frame and presents it (windowed)
    /// or renders it offscreen (headless). Clears [`DebugDraw`] afterwards.
    #[profiling::function]
    pub fn render(&mut self, world: &mut World) -> Result<(), RenderError> {
        self.check_device()?;
        let start = Instant::now();
        self.process_hot_reload();

        let Some(surface) = &self.surface else {
            self.ensure_capture_target();
            let (view, ui_view) = self.capture_views();
            let encoder = self.draw_frame(
                world,
                None,
                FrameTarget::Main {
                    output: &view,
                    format: CAPTURE_FORMAT,
                    ui: (&ui_view, CAPTURE_UI_FORMAT),
                },
            );
            // Headless frames have no swapchain to pace them. Wait for the GPU
            // so work does not pile up and frame timings include GPU time.
            let index = self.submit(encoder);
            let _ = self.device.poll(wgpu::PollType::Wait {
                submission_index: Some(index),
                timeout: None,
            });
            self.collect_timestamps();
            self.finish_stats(start);
            return Ok(());
        };
        let view_format = surface.view_format;
        let base_format = surface.config.format;

        let (frame, suboptimal) = match self.acquire() {
            Ok(Acquired::Frame(t, suboptimal)) => (t, suboptimal),
            Ok(Acquired::Skip) => {
                self.skip_frame(world);
                return Ok(());
            }
            Err(e) => {
                world.resource_or_default::<DebugDraw>().clear();
                return Err(e);
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("surface view"),
            format: Some(view_format),
            ..Default::default()
        });
        let ui_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("surface view (UI)"),
            format: Some(base_format),
            ..Default::default()
        });
        let encoder = self.draw_frame(
            world,
            None,
            FrameTarget::Main {
                output: &view,
                format: view_format,
                ui: (&ui_view, base_format),
            },
        );
        self.submit(encoder);
        self.present_surface_frame(SurfaceFrame {
            texture: frame,
            view,
            suboptimal,
        });
        self.collect_timestamps();
        self.finish_stats(start);
        Ok(())
    }

    /// Renders one frame offscreen at [`size`](Self::size) and reads it back.
    #[profiling::function]
    pub fn capture(&mut self, world: &mut World) -> Result<image::RgbaImage, RenderError> {
        self.check_device()?;
        let start = Instant::now();
        self.process_hot_reload();
        self.ensure_capture_target();
        let (view, ui_view) = self.capture_views();
        let encoder = self.draw_frame(
            world,
            None,
            FrameTarget::Main {
                output: &view,
                format: CAPTURE_FORMAT,
                ui: (&ui_view, CAPTURE_UI_FORMAT),
            },
        );
        let target = self.capture_target.as_ref().expect("capture target");
        let image = self.read_back(
            encoder,
            &target.texture,
            &target.readback,
            target.padded_row,
            target.size,
        );
        self.collect_timestamps();
        self.finish_stats(start);
        image
    }

    /// Creates an offscreen target of `width` x `height` physical pixels
    /// (clamped to 1..=the device's maximum texture size).
    pub fn create_viewport_target(&self, width: u32, height: u32) -> ViewportTarget {
        let size = self.clamp_target_size(width, height);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport colour"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VIEWPORT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[CAPTURE_FORMAT],
        });
        let srgb_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("viewport colour (sRGB view)"),
            format: Some(CAPTURE_FORMAT),
            ..Default::default()
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("viewport colour (unorm view)"),
            format: Some(VIEWPORT_FORMAT),
            ..Default::default()
        });
        ViewportTarget {
            size,
            targets: Targets::new(
                &self.device,
                &self.layouts,
                &self.frame,
                self.hdr_format,
                size,
            ),
            texture,
            srgb_view,
            view,
        }
    }

    /// Recreates `target` at a new size. Returns `false` (and keeps the old
    /// textures) if the clamped size is unchanged. After a resize the
    /// target's [`view`](ViewportTarget::view) is a new view.
    pub fn resize_viewport_target(
        &self,
        target: &mut ViewportTarget,
        width: u32,
        height: u32,
    ) -> bool {
        if self.clamp_target_size(width, height) == target.size {
            return false;
        }
        *target = self.create_viewport_target(width, height);
        true
    }

    /// Draws the world into `target`, from `camera` if given, otherwise from
    /// the first active [`Camera`] entity. Processes shader hot reload,
    /// submits the work (without waiting for it) and clears [`DebugDraw`].
    #[profiling::function]
    pub fn render_to(
        &mut self,
        world: &mut World,
        target: &ViewportTarget,
        camera: Option<&ViewCamera>,
    ) {
        let start = Instant::now();
        self.process_hot_reload();
        let encoder = self.draw_frame(world, camera, FrameTarget::Viewport(target));
        self.submit(encoder);
        self.collect_timestamps();
        self.finish_stats(start);
    }

    /// Reads a viewport target back (waits for the GPU). The bytes are the
    /// sRGB-encoded colour, as in [`capture`](Self::capture).
    pub fn read_viewport_target(
        &self,
        target: &ViewportTarget,
    ) -> Result<image::RgbaImage, RenderError> {
        let (w, h) = target.size;
        let padded_row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport readback"),
            size: padded_row as u64 * h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("viewport readback"),
            });
        self.read_back(encoder, &target.texture, &readback, padded_row, target.size)
    }

    /// Acquires the window's next surface texture for a caller that draws
    /// into it itself (for example an editor UI). Returns `Ok(None)` when the
    /// frame should be skipped: the window is occluded, acquisition timed out,
    /// or the surface was outdated and has been reconfigured. Errors when
    /// headless.
    pub fn acquire_surface_frame(&mut self) -> Result<Option<SurfaceFrame>, RenderError> {
        self.check_device()?;
        let format = self
            .surface_format()
            .ok_or_else(|| RenderError::Surface("the renderer has no window surface".into()))?;
        match self.acquire()? {
            Acquired::Skip => Ok(None),
            Acquired::Frame(texture, suboptimal) => {
                let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("surface view"),
                    format: Some(format),
                    ..Default::default()
                });
                Ok(Some(SurfaceFrame {
                    texture,
                    view,
                    suboptimal,
                }))
            }
        }
    }

    /// Presents a frame from [`acquire_surface_frame`](Self::acquire_surface_frame)
    /// after its work has been submitted.
    pub fn present_surface_frame(&mut self, frame: SurfaceFrame) {
        {
            profiling::scope!("present");
            self.queue.present(frame.texture);
        }
        if frame.suboptimal
            && let Some(s) = &self.surface
        {
            s.surface.configure(&self.device, &s.config);
        }
    }

    // ----- internals -----

    fn acquire(&mut self) -> Result<Acquired, RenderError> {
        let Some(surface) = &self.surface else {
            return Err(RenderError::Surface(
                "the renderer has no window surface".into(),
            ));
        };
        Ok(match surface.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => Acquired::Frame(t, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => Acquired::Frame(t, true),
            status @ (wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded) => {
                tracing::debug!(
                    "surface texture unavailable ({}); frame skipped",
                    if matches!(status, wgpu::CurrentSurfaceTexture::Timeout) {
                        "timeout"
                    } else {
                        "occluded"
                    }
                );
                Acquired::Skip
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                tracing::debug!("surface outdated or lost; reconfigured, frame skipped");
                surface.surface.configure(&self.device, &surface.config);
                Acquired::Skip
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(RenderError::Surface(
                    "validation error while acquiring the surface texture".into(),
                ));
            }
        })
    }

    fn clamp_target_size(&self, width: u32, height: u32) -> (u32, u32) {
        clamp_size(&self.device, width, height)
    }

    fn ensure_targets(&mut self) {
        if self.targets.is_none() {
            self.targets = Some(Targets::new(
                &self.device,
                &self.layouts,
                &self.frame,
                self.hdr_format,
                self.size,
            ));
        }
    }

    /// Copies `texture` (RGBA8) into `readback` at the end of `encoder`,
    /// submits, waits and returns the pixels.
    fn read_back(
        &self,
        mut encoder: wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        readback: &wgpu::Buffer,
        padded_row: u32,
        (w, h): (u32, u32),
    ) -> Result<image::RgbaImage, RenderError> {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.submit(encoder);

        profiling::scope!("readback");
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RenderError::Capture(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Capture(e.to_string()))?
            .map_err(|e| RenderError::Capture(e.to_string()))?;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        {
            let data = slice
                .get_mapped_range()
                .map_err(|e| RenderError::Capture(e.to_string()))?;
            for row in data.chunks(padded_row as usize).take(h as usize) {
                pixels.extend_from_slice(&row[..(w * 4) as usize]);
            }
        }
        readback.unmap();
        image::RgbaImage::from_raw(w, h, pixels)
            .ok_or_else(|| RenderError::Capture("readback size mismatch".into()))
    }

    fn submit(&self, encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        profiling::scope!("submit");
        let index = self.queue.submit([encoder.finish()]);
        let ms = self.frame_start.elapsed().as_secs_f32() * 1000.0;
        self.submit_ms.store(ms.to_bits(), Ordering::Relaxed);
        index
    }

    fn finish_stats(&mut self, start: Instant) {
        self.stats.validation_errors = self.error_count.load(Ordering::Relaxed);
        self.stats.cpu_render_ms = start.elapsed().as_secs_f32() * 1000.0;
        self.stats.cpu_submit_ms = f32::from_bits(self.submit_ms.load(Ordering::Relaxed));
        self.stats.presented = true;
    }

    fn skip_frame(&mut self, world: &mut World) {
        self.stats.presented = false;
        self.stats.validation_errors = self.error_count.load(Ordering::Relaxed);
        world.resource_or_default::<DebugDraw>().clear();
    }

    /// The capture target's sRGB view and its plain (UI) view.
    fn capture_views(&self) -> (wgpu::TextureView, wgpu::TextureView) {
        let t = self.capture_target.as_ref().expect("capture target");
        (t.view.clone(), t.ui_view.clone())
    }

    fn ensure_capture_target(&mut self) {
        if self
            .capture_target
            .as_ref()
            .is_some_and(|t| t.size == self.size)
        {
            return;
        }
        let (w, h) = self.size;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CAPTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[CAPTURE_UI_FORMAT],
        });
        let view = texture.create_view(&Default::default());
        let ui_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("capture target (UI)"),
            format: Some(CAPTURE_UI_FORMAT),
            ..Default::default()
        });
        let padded_row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture readback"),
            size: padded_row as u64 * h as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.capture_target = Some(CaptureTarget {
            texture,
            view,
            ui_view,
            readback,
            padded_row,
            size: (w, h),
        });
    }

    fn process_hot_reload(&mut self) {
        if !self.shaders.poll_changed() {
            return;
        }
        match self.reload_shaders() {
            Ok(()) => tracing::info!("shaders reloaded"),
            Err(e) => tracing::error!("shader reload failed; keeping previous pipelines\n{e}"),
        }
    }

    /// Re-reads and validates every shader, then rebuilds all pipelines inside
    /// error scopes. On any error the previous pipelines stay.
    fn reload_shaders(&mut self) -> Result<(), String> {
        let processed = self.shaders.reload()?;
        let (device, layouts) = (&self.device, &self.layouts);
        let pipelines = catch_errors(device, || {
            Pipelines::new(
                device,
                layouts,
                &processed,
                self.hdr_format,
                &self.output_formats,
            )
        })
        .map_err(|e| format!("pipeline creation failed: {e}"))?;
        // Engine material pipelines are rebuilt in place, all or nothing.
        let keys = self
            .pipeline_cache
            .keys_where(|k| !matches!(k.shader, ShaderKey::Custom(_)));
        let rebuilt = catch_errors(device, || {
            keys.iter()
                .map(|(id, key)| {
                    let module = engine_module(&pipelines.modules, key);
                    (*id, create_material_pipeline(device, layouts, key, module))
                })
                .collect::<Vec<_>>()
        })
        .map_err(|e| format!("pipeline creation failed: {e}"))?;
        self.pipeline_cache.replace(rebuilt);
        self.pipelines = pipelines;
        self.library = processed.sources;
        // Material shaders include the library: recompile them next frame.
        self.custom_shaders.library_generation += 1;
        self.pipeline_generation += 1;
        Ok(())
    }

    fn draw_frame(
        &mut self,
        world: &mut World,
        camera: Option<&ViewCamera>,
        target: FrameTarget<'_>,
    ) -> wgpu::CommandEncoder {
        self.frame_start = Instant::now();
        let validation_errors = self.stats.validation_errors;
        self.stats = FrameStats {
            validation_errors,
            ..Default::default()
        };
        if let Some(t) = &self.gpu_timer
            && let Ok(mut labels) = t.labels.lock()
        {
            labels.clear();
        }
        let size = match &target {
            FrameTarget::Main { .. } => self.size,
            FrameTarget::Viewport(v) => v.size,
        };
        let plan = self.prepare(world, size, camera);
        self.overlay
            .prepare(&self.device, &self.queue, world, &self.textures, size);
        let (overlay_view, overlay_format) = match &target {
            FrameTarget::Main { ui, .. } => *ui,
            FrameTarget::Viewport(v) => (&v.view, VIEWPORT_FORMAT),
        };
        let (mut encoder, counts) = match target {
            FrameTarget::Main { output, format, .. } => {
                self.ensure_targets();
                let targets = self.targets.as_ref().expect("main targets");
                self.encode(&plan, targets, output, format)
            }
            FrameTarget::Viewport(v) => {
                self.encode(&plan, &v.targets, &v.srgb_view, CAPTURE_FORMAT)
            }
        };
        let overlay_timing = if self.overlay.has_draws() {
            self.timestamps("ui")
        } else {
            None
        };
        let overlay_draws =
            self.overlay
                .encode(&mut encoder, overlay_view, overlay_format, overlay_timing);
        self.stats.draws = counts.draws + overlay_draws;
        self.stats.shadow_draws = counts.shadow_draws;
        self.stats.local_shadow_draws = counts.local_shadow_draws;
        self.stats.transparent_draws = counts.transparent_draws;
        self.stats.instances = counts.instances;
        self.stats.triangles = counts.triangles;
        self.resolve_timestamps(&mut encoder);
        encoder
    }

    /// Extracts the frame from the world and writes every GPU buffer.
    /// `size` is the target size in pixels; `camera` overrides the world's
    /// active camera.
    #[profiling::function]
    fn prepare(
        &mut self,
        world: &mut World,
        (w, h): (u32, u32),
        camera_override: Option<&ViewCamera>,
    ) -> FramePlan {
        world.resource_or_default::<Assets<MeshData>>();
        world.resource_or_default::<Assets<StandardMaterial>>();
        world.resource_or_default::<DebugDraw>();
        let env = *world.resource_or_default::<Environment>();
        let settings = *world.resource_or_default::<RenderSettings>();

        // Mesh uploads (change-driven).
        let removed = world.resource_mut::<Assets<MeshData>>().drain_removed();
        self.meshes.remove(&removed);
        let mut upload = UploadStats::default();
        self.meshes.sync(
            &self.device,
            &self.queue,
            world.resource::<Assets<MeshData>>(),
            &mut upload,
        );

        // Textures, material shaders, then materials (change-driven).
        self.textures.sync(
            &self.device,
            &self.queue,
            world.get_resource::<Assets<ImageData>>(),
            &mut upload,
        );
        self.custom_shaders.sync(
            &self.device,
            &self.layouts,
            &self.library,
            &mut self.pipeline_cache,
            world.get_resource::<Assets<ShaderSource>>(),
        );
        self.materials.sync(
            &mut MaterialContext {
                device: &self.device,
                queue: &self.queue,
                layouts: &self.layouts,
                textures: &self.textures,
                shaders: &self.custom_shaders,
                modules: &self.pipelines.modules,
                pipelines: &mut self.pipeline_cache,
                hdr_format: self.hdr_format,
                upload: &mut upload,
            },
            MaterialInputs {
                standard: world.get_resource::<Assets<StandardMaterial>>(),
                shader: world.get_resource::<Assets<ShaderMaterial>>(),
                lookup: world.get_resource::<AssetLookup>(),
            },
        );

        // Skinned objects: joint palettes, posed bounds and their materials'
        // skinned pipelines (built on first use).
        self.skins.prepare(world, &self.meshes);
        if !self.skins.draws.is_empty() {
            let mut cx = MaterialContext {
                device: &self.device,
                queue: &self.queue,
                layouts: &self.layouts,
                textures: &self.textures,
                shaders: &self.custom_shaders,
                modules: &self.pipelines.modules,
                pipelines: &mut self.pipeline_cache,
                hdr_format: self.hdr_format,
                upload: &mut upload,
            };
            self.materials.ensure_skinned(&mut cx, DEFAULT_MATERIAL);
            for &e in self.skins.draws.keys() {
                if let Some(mr) = world.get::<MeshRenderer>(e) {
                    for m in &mr.materials {
                        self.materials.ensure_skinned(&mut cx, m.id());
                    }
                }
            }
        }
        self.stats.skinned = self.skins.draws.len() as u32;
        self.stats.joint_matrices = self.skins.len() as u32;

        // Camera.
        let aspect = w as f32 / h as f32;
        let camera = match camera_override {
            Some(v) => Some((v.camera, v.transform)),
            None => world
                .query::<(&Camera, &GlobalTransform)>()
                .iter()
                .find(|(c, _)| c.active)
                .map(|(c, g)| (*c, *g)),
        };
        let cam_view = match &camera {
            Some((c, g)) => {
                let (_, rotation, position) = g.0.to_scale_rotation_translation();
                CameraView {
                    position,
                    rotation: rotation.normalize(),
                    fov_y: c.fov_y,
                    aspect,
                    near: c.near.max(1e-4),
                }
            }
            None => CameraView {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                fov_y: Camera::default().fov_y,
                aspect,
                near: Camera::default().near,
            },
        };
        let view = Mat4::from_rotation_translation(cam_view.rotation, cam_view.position).inverse();
        let proj = directx::perspective_infinite_reverse(cam_view.fov_y, aspect, cam_view.near);
        let view_proj = proj * view;
        let frustum = Frustum::from_view_proj(&view_proj);
        let forward = cam_view.rotation * Vec3::NEG_Z;
        let ao = &env.ambient_occlusion;
        let ao_on = camera.is_some() && ao.enabled && ao.intensity > 0.0 && ao.radius > 0.0;
        let bloom = &env.bloom;
        let bloom_on = camera.is_some() && bloom.enabled && bloom.intensity > 0.0;

        // Light.
        let light = world
            .query::<(&DirectionalLight, &GlobalTransform)>()
            .iter()
            .next()
            .map(|(l, g)| (*l, g.forward()));
        let shadows = camera.is_some()
            && light.is_some_and(|(l, _)| l.shadows && l.shadow_distance > cam_view.near);

        // Shadow cascades, fitted before the objects so casters can be culled
        // per cascade.
        let mut globals = GlobalsUniform::zeroed_default();
        let mut culler = None;
        if let (true, Some((l, dir))) = (shadows, light) {
            let cascades = cascades::compute_cascades(&cam_view, dir, l.shadow_distance);
            let mut pass_data = vec![0u8; CASCADE_COUNT * UNIFORM_SLOT as usize];
            for (i, c) in cascades.iter().enumerate() {
                let m = c.view_proj.to_cols_array();
                let off = i * UNIFORM_SLOT as usize;
                pass_data[off..off + 64].copy_from_slice(bytemuck::cast_slice(&m));
                globals.cascade_view_proj[i] = c.view_proj.to_cols_array_2d();
                globals.cascade_splits[i] = c.split_far;
                globals.cascade_texel[i] = c.texel_world;
                globals.cascade_bias[i] = SHADOW_DEPTH_BIAS * c.texel_world / c.depth_range;
            }
            self.queue
                .write_buffer(&self.frame.shadow_pass_buffer, 0, &pass_data);
            upload.bytes += pass_data.len() as u64;
            culler = Some(CasterCuller::new(&cascades));
        }

        // Selection outline ids (empty, and free, without a selection).
        self.outline_ids.clear();
        let outline = world
            .get_resource::<SelectionOutline>()
            .filter(|o| !o.is_empty() && camera.is_some());
        if let Some(o) = &outline {
            self.outline_ids
                .extend(o.secondary.iter().map(|&e| (e, 0.5)));
            self.outline_ids.extend(o.primary.iter().map(|&e| (e, 1.0)));
        }
        let mut outline_rect: Option<[f32; 4]> = None;

        // Entities hidden by `Visible(false)` (theirs or an ancestor's):
        // their meshes and local lights are not drawn.
        let game_hidden = box_core::visibility::hidden_entities(world);

        // Point and spot lights: the visible ones, the most important when
        // over the cap; then the one shadowed light and the ambient volumes.
        self.lights.clear();
        self.volumes.clear();
        if camera.is_some() {
            // Written to reject NaN as well.
            let usable = |range: f32, intensity: f32, radiance: Vec3, position: Vec3| {
                range > 0.0 && intensity > 0.0 && radiance.is_finite() && position.is_finite()
            };
            let reaches = |position: Vec3, range: f32| {
                let reach = Vec3::splat(range);
                frustum.intersects_aabb(&Aabb::new(position - reach, position + reach))
            };
            for (e, l, g) in world
                .query::<(Entity, &PointLight, &GlobalTransform)>()
                .iter()
            {
                if game_hidden.contains(&e) {
                    continue;
                }
                let position = g.translation();
                let radiance = l.color.to_vec3() * l.intensity;
                if usable(l.range, l.intensity, radiance, position) && reaches(position, l.range) {
                    let mut c = LightCandidate::point(position, l.range, radiance);
                    c.shadow = l.shadows.then_some(l.shadow);
                    self.lights.push(c);
                }
            }
            for (e, l, g) in world
                .query::<(Entity, &SpotLight, &GlobalTransform)>()
                .iter()
            {
                if game_hidden.contains(&e) {
                    continue;
                }
                let position = g.translation();
                let radiance = l.color.to_vec3() * l.intensity;
                let direction = g.forward().normalize_or_zero();
                if usable(l.range, l.intensity, radiance, position)
                    && direction != Vec3::ZERO
                    && reaches(position, l.range)
                {
                    let mut c = LightCandidate::point(position, l.range, radiance);
                    c.direction = direction;
                    c.spot = crate::components::spot_scale_offset(l.inner_angle, l.outer_angle);
                    c.outer_angle = l.outer_angle.clamp(0.0, std::f32::consts::FRAC_PI_2);
                    c.shadow = l.shadows.then_some(l.shadow);
                    self.lights.push(c);
                }
            }
            lights::select_lights(&mut self.lights, cam_view.position, MAX_LOCAL_LIGHTS);
            for (v, g) in world.query::<(&AmbientVolume, &GlobalTransform)>().iter() {
                let (scale, rotation, translation) = g.0.to_scale_rotation_translation();
                let half = (v.half_extents * scale).abs();
                let blend = v.blend_distance.max(0.0);
                let reach = half.length() + blend;
                if !half.is_finite() || !translation.is_finite() || !reaches(translation, reach) {
                    continue;
                }
                let gap = ((translation - cam_view.position).length() - reach).max(0.0);
                let world_to_local =
                    Mat4::from_rotation_translation(rotation.normalize(), translation).inverse();
                self.volumes.push((
                    gap,
                    GpuAmbientVolume {
                        world_to_local: world_to_local.to_cols_array_2d(),
                        half_extents: half.extend(blend).to_array(),
                        ambient: (v.color.to_vec3() * v.intensity.max(0.0))
                            .extend(v.sky_contribution.clamp(0.0, 1.0))
                            .to_array(),
                    },
                ));
            }
            if self.volumes.len() > MAX_AMBIENT_VOLUMES {
                // The nearest, applied in scene order.
                let mut by_gap: Vec<f32> = self.volumes.iter().map(|(g, _)| *g).collect();
                by_gap.sort_by(f32::total_cmp);
                let cut = by_gap[MAX_AMBIENT_VOLUMES - 1];
                let mut kept = 0;
                self.volumes.retain(|(g, _)| {
                    let keep = *g <= cut && kept < MAX_AMBIENT_VOLUMES;
                    kept += keep as usize;
                    keep
                });
            }
        }
        let shadowed = lights::shadowed_light(&self.lights, cam_view.position);
        let mut shadow_header = ShadowHeader::default();
        let mut local_frusta: Vec<Frustum> = Vec::new();
        if let Some(i) = shadowed {
            let light = self.lights[i];
            let settings = light.shadow.unwrap_or_default();
            let max = self.device.limits().max_texture_dimension_2d.min(4096);
            let resolution = settings.resolution.clamp(128, max);
            let views = lights::shadow_views(&light, resolution);
            self.frame
                .ensure_local_shadow(&self.device, &self.layouts, resolution, views.count);
            let mut pass_data = vec![0u8; views.count * UNIFORM_SLOT as usize];
            for (v, m) in views.view_proj[..views.count].iter().enumerate() {
                let off = v * UNIFORM_SLOT as usize;
                pass_data[off..off + 64].copy_from_slice(bytemuck::cast_slice(&m.to_cols_array()));
                local_frusta.push(Frustum::from_view_proj(m));
            }
            self.queue.write_buffer(
                &self.frame.shadow_pass_buffer,
                CASCADE_COUNT as u64 * UNIFORM_SLOT,
                &pass_data,
            );
            upload.bytes += pass_data.len() as u64;
            shadow_header = ShadowHeader {
                kind: if light.is_spot() { 1 } else { 2 },
                normal_bias: settings.normal_bias.max(0.0),
                depth_bias: settings.depth_bias.max(0.0),
                texel_scale: views.texel_scale,
                view_proj: views.view_proj,
            };
        }
        let volumes: Vec<GpuAmbientVolume> = self.volumes.iter().map(|(_, v)| *v).collect();
        lights::pack_lights(
            &self.lights,
            shadowed,
            &shadow_header,
            &volumes,
            &mut self.light_data,
        );
        self.queue
            .write_buffer(&self.frame.light_buffer, 0, &self.light_data);
        upload.bytes += self.light_data.len() as u64;
        let spots = self.lights.iter().filter(|l| l.is_spot()).count() as u32;
        self.stats.spot_lights = spots;
        self.stats.point_lights = self.lights.len() as u32 - spots;
        self.stats.local_shadow_views = local_frusta.len() as u32;
        self.stats.ambient_volumes = volumes.len() as u32;

        // Objects: a persistent instance slot per object with a mesh, one
        // draw item per material group of each object drawn in any pass.
        self.draw_list.clear();
        self.instances.begin_frame();
        let mut depth_fade_visible = false;
        let wind_scale = if self.materials.any_wind() && env.wind.direction != glam::Vec2::ZERO {
            env.wind.strength.max(0.0) * crate::materials::WIND_REACH
        } else {
            0.0
        };
        if camera.is_some() {
            type Drawn<'a> = box_core::world::Without<
                (Entity, &'a GlobalTransform, &'a MeshRenderer),
                &'a crate::components::Hidden,
            >;
            let selection = !self.outline_ids.is_empty();
            for (entity, gt, mr) in world.query::<Drawn>().iter() {
                if game_hidden.contains(&entity) {
                    continue;
                }
                let Some(gpu) = self.meshes.get(mr.mesh.id()) else {
                    continue;
                };
                self.stats.objects += 1;
                let outline_id = if selection {
                    self.outline_ids.get(&entity).copied().unwrap_or(0.0)
                } else {
                    0.0
                };
                let skinned = if self.skins.draws.is_empty() {
                    None
                } else {
                    self.skins.draws.get(&entity).copied()
                };
                let slot =
                    self.instances
                        .slot(entity, &gt.0, outline_id, skinned.map_or(0, |s| s.base));
                let local_bounds = skinned.map_or(gpu.bounds, |s| s.bounds);
                let mut world_bounds = local_bounds.transformed(&gt.0);
                if wind_scale > 0.0 {
                    // Swaying vertices leave the mesh bounds by up to this.
                    let reach = gpu
                        .groups
                        .iter()
                        .filter_map(|g| mr.materials.get(g.material_slot as usize))
                        .filter_map(|m| self.materials.get(m.id()))
                        .fold(0.0f32, |r, e| r.max(e.wind_reach))
                        * wind_scale;
                    if reach > 0.0 {
                        let pad = Vec3::splat(reach);
                        world_bounds = Aabb::new(world_bounds.min - pad, world_bounds.max + pad);
                    }
                }
                let visible = frustum.intersects_aabb(&world_bounds);
                let cascades = match &culler {
                    Some(c) if mr.cast_shadows => c.cascade_mask(&world_bounds),
                    _ => 0,
                };
                let local_views = if mr.cast_shadows {
                    local_frusta
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| f.intersects_aabb(&world_bounds))
                        .fold(0u8, |m, (i, _)| m | (1 << i))
                } else {
                    0
                };
                if visible {
                    self.stats.visible += 1;
                }
                if !visible && cascades == 0 && local_views == 0 {
                    continue;
                }
                let outlined = visible && outline_id > 0.0;
                let mirrored = gt.0.matrix3.determinant() < 0.0;
                let depth = (world_bounds.center() - cam_view.position).dot(forward);
                let mut casts = false;
                for group in &gpu.groups {
                    let requested = mr
                        .materials
                        .get(group.material_slot as usize)
                        .map(|m| m.id())
                        .and_then(|id| self.materials.get(id).map(|e| (id, e)));
                    let Some((material, entry)) = requested.or_else(|| {
                        self.materials
                            .get(DEFAULT_MATERIAL)
                            .map(|e| (DEFAULT_MATERIAL, e))
                    }) else {
                        continue;
                    };
                    let (main, shadow, prepass, outline) = match skinned {
                        Some(_) => {
                            let s = entry.skinned.unwrap_or_default();
                            (s.main, s.shadow, s.prepass, s.outline)
                        }
                        None => (entry.main, entry.shadow, entry.prepass, entry.outline),
                    };
                    let main_pipeline = match (visible, main) {
                        (true, Some(p)) => p[mirrored as usize],
                        _ => NO_PIPELINE,
                    };
                    let shadow_pipeline = match (cascades | local_views != 0, shadow) {
                        (true, Some(p)) => p[mirrored as usize],
                        _ => NO_PIPELINE,
                    };
                    if main_pipeline == NO_PIPELINE && shadow_pipeline == NO_PIPELINE {
                        continue;
                    }
                    let prepass_pipeline = match (visible, prepass) {
                        (true, Some(p)) => p[mirrored as usize],
                        _ => NO_PIPELINE,
                    };
                    let outline_pipeline = match (outlined, outline) {
                        (true, Some(p)) => p[mirrored as usize],
                        _ => NO_PIPELINE,
                    };
                    depth_fade_visible |= visible && entry.depth_fade;
                    casts |= shadow_pipeline != NO_PIPELINE;
                    self.draw_list.push(DrawItem {
                        slot,
                        mesh: mr.mesh.id(),
                        first_index: group.first_index,
                        index_count: group.index_count,
                        material,
                        shadow_material: if entry.shadow_binds_material {
                            material
                        } else {
                            0
                        },
                        main_pipeline,
                        shadow_pipeline,
                        prepass_pipeline,
                        outline_pipeline,
                        transparent: entry.transparent,
                        cascades,
                        local_views,
                        depth,
                        outline: (outlined && main_pipeline != NO_PIPELINE).then_some(
                            if entry.double_sided {
                                2
                            } else {
                                mirrored as u8
                            },
                        ),
                        skinned: skinned.is_some(),
                    });
                }
                if casts {
                    self.stats.shadow_casters += 1;
                }
                if outlined {
                    let r = screen_rect(&view_proj, &world_bounds, (w, h));
                    outline_rect = Some(match outline_rect {
                        None => r,
                        Some(o) => [
                            o[0].min(r[0]),
                            o[1].min(r[1]),
                            o[2].max(r[2]),
                            o[3].max(r[3]),
                        ],
                    });
                }
            }
        }
        self.instances.end_frame();
        // Particles: instances of every visible emitter.
        let particle_frame = if camera.is_some() {
            self.particles.prepare(
                world,
                cam_view.position,
                forward,
                &frustum,
                &self.device,
                &self.queue,
                &self.layouts,
                &self.textures,
                &mut upload,
            )
        } else {
            self.particles.draws.clear();
            Default::default()
        };
        self.stats.particles = particle_frame.particles;
        self.stats.particle_emitters = particle_frame.emitters;
        // The prepass runs for ambient occlusion and for soft transparency.
        let prepass_on = ao_on || depth_fade_visible || particle_frame.soft;
        // Pass orders: opaque and shadows by state, transparent back to front.
        self.main_order.clear();
        self.transparent_order.clear();
        self.shadow_order.clear();
        self.prepass_order.clear();
        self.outline_order.clear();
        let list = &self.draw_list;
        for (i, d) in list.iter().enumerate() {
            let i = i as u32;
            if d.main_pipeline != NO_PIPELINE {
                if d.transparent {
                    self.transparent_order.push(i);
                } else {
                    self.main_order.push(i);
                }
            }
            if d.shadow_pipeline != NO_PIPELINE {
                self.shadow_order.push(i);
            }
            if prepass_on && d.prepass_pipeline != NO_PIPELINE {
                self.prepass_order.push(i);
            }
            if d.outline.is_some() {
                self.outline_order.push(i);
            }
        }
        sort_by_state(&mut self.main_order, list, main_state);
        sort_by_state(&mut self.shadow_order, list, shadow_state);
        sort_by_state(&mut self.prepass_order, list, prepass_state);
        sort_transparent(&mut self.transparent_order, list);
        self.outline_order.sort_unstable_by_key(|&i| {
            let d = &list[i as usize];
            (d.outline, draw_key(d, outline_state), i)
        });

        // Instanced draws: runs of equal draw keys, their slots back to back
        // in the draw slot list.
        let slots = &mut self.instances.list;
        let item = |i: u32| &list[i as usize];
        batch(
            &self.main_order,
            |i| Some((draw_key(item(i), main_state), item(i).slot)),
            true,
            slots,
            &mut self.main_batches,
        );
        batch(
            &self.prepass_order,
            |i| Some((draw_key(item(i), prepass_state), item(i).slot)),
            true,
            slots,
            &mut self.prepass_batches,
        );
        // Back-to-front runs of equal keys (puffs of one cloud material)
        // are one instanced draw: instances rasterise in slot order, so the
        // blend order is unchanged.
        batch(
            &self.transparent_order,
            |i| Some((draw_key(item(i), main_state), item(i).slot)),
            true,
            slots,
            &mut self.transparent_batches,
        );
        for (cascade, batches) in self.shadow_batches.iter_mut().enumerate() {
            let bit = 1u8 << cascade;
            batch(
                &self.shadow_order,
                |i| {
                    let d = item(i);
                    (d.cascades & bit != 0).then(|| (draw_key(d, shadow_state), d.slot))
                },
                true,
                slots,
                batches,
            );
        }
        for (view, batches) in self.local_shadow_batches.iter_mut().enumerate() {
            let bit = 1u8 << view;
            batch(
                &self.shadow_order,
                |i| {
                    let d = item(i);
                    (d.local_views & bit != 0).then(|| (draw_key(d, shadow_state), d.slot))
                },
                true,
                slots,
                batches,
            );
        }
        batch(
            &self.outline_order,
            |i| {
                let d = item(i);
                Some(((d.outline, draw_key(d, outline_state)), d.slot))
            },
            true,
            slots,
            &mut self.outline_batches,
        );
        let (joint_bytes, joints_replaced) = self.skins.upload(&self.device, &self.queue);
        upload.bytes += joint_bytes;
        self.stats.buffer_reallocations += u32::from(joints_replaced);
        let instance_upload = self.instances.upload(
            &self.device,
            &self.queue,
            &self.layouts,
            self.skins.buffer(),
            joints_replaced,
        );
        upload.bytes += instance_upload.bytes;
        self.stats.buffer_reallocations += instance_upload.reallocations;

        // Outline parameters and scissor (only with a visible selection).
        let outline_scissor = match (&outline, outline_rect) {
            (Some(o), Some(rect)) => {
                let width = o.width_px.clamp(0.5, 4.0);
                let params: [[f32; 4]; 3] = [
                    o.color.to_array(),
                    o.secondary_color.to_array(),
                    [width, o.occluded_alpha.clamp(0.0, 1.0), 0.0, 0.0],
                ];
                self.queue.write_buffer(
                    &self.frame.outline_buffer,
                    0,
                    bytemuck::cast_slice(&params),
                );
                upload.bytes += OUTLINE_UNIFORM_SIZE;
                scissor(rect, width.ceil() + 1.0, (w, h))
            }
            _ => None,
        };

        // Globals.
        globals.view_proj = view_proj.to_cols_array_2d();
        globals.inv_view_proj = view_proj.inverse().to_cols_array_2d();
        globals.camera_pos = cam_view.position.extend(cam_view.near).to_array();
        globals.camera_forward = forward.extend(0.0).to_array();
        if let Some((l, dir)) = light {
            globals.sun_dir = (-dir).extend(1.0).to_array();
            globals.sun_color = (l.color.to_vec3() * l.intensity)
                .extend(env.sun_disc_size)
                .to_array();
        }
        globals.sky_zenith = env.sky_zenith.to_array();
        globals.sky_horizon = env.sky_horizon.to_array();
        globals.sky_ground = env.ground.to_array();
        globals.ambient = [env.ambient_intensity, 0.0, 0.0, 0.0];
        globals.fog = [
            env.fog_density.max(0.0),
            env.fog_height_falloff,
            env.fog_base_height,
            0.0,
        ];
        let tonemapper = match env.tonemapper {
            Tonemapper::AgX => 0.0,
            Tonemapper::AcesFitted => 1.0,
            Tonemapper::None => 2.0,
        };
        globals.post = [env.exposure, tonemapper, 0.0, 0.0];
        globals.shadow = [
            if shadows { 1.0 } else { 0.0 },
            SHADOW_NORMAL_OFFSET,
            CASCADE_BLEND_BAND,
            if settings.show_cascades && shadows {
                1.0
            } else {
                0.0
            },
        ];
        globals.viewport = [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32];
        if ao_on {
            let tan_y = (cam_view.fov_y * 0.5).tan();
            let half_height = post::half_size((w, h)).1 as f32;
            let right = cam_view.rotation * Vec3::X * (tan_y * aspect);
            let up = cam_view.rotation * Vec3::Y * tan_y;
            globals.ao = [
                1.0,
                if settings.show_ambient_occlusion {
                    1.0
                } else {
                    0.0
                },
                ao.direct_lighting.clamp(0.0, 1.0),
                0.0,
            ];
            globals.ao_right = right.extend(ao.radius).to_array();
            globals.ao_up = up.extend(ao.intensity.max(0.0)).to_array();
            globals.ao_params = [
                ao.bias.max(0.0),
                ao.power.max(0.01),
                half_height / (2.0 * tan_y),
                0.0,
            ];
        }
        if bloom_on {
            globals.bloom = [bloom.intensity.clamp(0.0, 1.0), 0.0, 0.0, 0.0];
        }
        post::write_grading(&env.color_grading, &mut globals);
        post::write_look(&env, aspect, &mut globals);
        globals.scene_depth = [if prepass_on { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
        if let Some(t) = world.get_resource::<Time>() {
            globals.time = [t.elapsed, t.delta, 0.0, 0.0];
        }
        self.queue
            .write_buffer(&self.frame.globals_buffer, 0, bytemuck::bytes_of(&globals));
        upload.bytes += std::mem::size_of::<GlobalsUniform>() as u64;

        // Debug lines.
        let debug = world.resource_mut::<DebugDraw>();
        let lines = debug.layers.each_ref().map(|l| l.len() as u32);
        let total: u32 = lines.iter().sum();
        if total > 0 {
            let bytes = (total as usize * std::mem::size_of::<LineVertex>()) as u64;
            if self.frame.ensure_debug_capacity(&self.device, bytes) {
                self.stats.buffer_reallocations += 1;
            }
            let buffer = self.frame.debug_buffer.as_ref().expect("debug buffer");
            let mut offset = 0u64;
            for layer in &debug.layers {
                self.queue
                    .write_buffer(buffer, offset, bytemuck::cast_slice(layer));
                offset += (layer.len() * std::mem::size_of::<LineVertex>()) as u64;
            }
            upload.bytes += bytes;
        }
        debug.clear();

        self.stats.upload_bytes = upload.bytes;
        self.stats.buffer_reallocations += upload.reallocations;
        self.stats.material_pipelines = self.pipeline_cache.len() as u32;
        FramePlan {
            shadows,
            local_views: local_frusta.len(),
            lines,
            outline: outline_scissor,
            prepass: prepass_on,
            ao: ao_on,
            bloom: bloom_on.then_some(bloom.scatter.clamp(0.0, 1.0)),
        }
    }

    /// Records the shadow, HDR scene (opaque, sky, transparent), outline mask
    /// and tone-mapping passes. Returns the encoder and the draw counts.
    #[profiling::function]
    fn encode(
        &self,
        plan: &FramePlan,
        targets: &Targets,
        output: &wgpu::TextureView,
        output_format: wgpu::TextureFormat,
    ) -> (wgpu::CommandEncoder, FrameCounts) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
        let mut draws = 0u32;
        let mut shadow_draws = 0u32;
        let mut transparent_draws = 0u32;
        let mut instances = 0u32;
        let mut triangles = 0u64;

        if plan.shadows {
            profiling::scope!("shadow passes");
            for (cascade, layer) in self.frame.shadow_layer_views.iter().enumerate() {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow cascade pass"),
                    timestamp_writes: self.timestamps("shadows"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: layer,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_bind_group(
                    0,
                    &self.frame.shadow_pass_bind_group,
                    &[(cascade as u64 * UNIFORM_SLOT) as u32],
                );
                pass.set_bind_group(1, &self.instances.bind_group, &[]);
                let mut state = DrawState::default();
                for b in &self.shadow_batches[cascade] {
                    if let Some(tris) = self.draw_batch(&mut pass, b, Pass::Shadow, &mut state) {
                        shadow_draws += 1;
                        instances += b.count;
                        triangles += tris;
                    }
                }
            }
            draws += shadow_draws;
        }

        let mut local_shadow_draws = 0u32;
        if let Some(map) = self
            .frame
            .local_shadow
            .as_ref()
            .filter(|_| plan.local_views > 0)
        {
            profiling::scope!("local shadow passes");
            for view in 0..plan.local_views {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("local shadow pass"),
                    timestamp_writes: self.timestamps("local shadow"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &map.layer_views[view],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_bind_group(
                    0,
                    &self.frame.shadow_pass_bind_group,
                    &[((CASCADE_COUNT + view) as u64 * UNIFORM_SLOT) as u32],
                );
                pass.set_bind_group(1, &self.instances.bind_group, &[]);
                let mut state = DrawState::default();
                for b in &self.local_shadow_batches[view] {
                    if let Some(tris) = self.draw_batch(&mut pass, b, Pass::Shadow, &mut state) {
                        local_shadow_draws += 1;
                        instances += b.count;
                        triangles += tris;
                    }
                }
            }
            draws += local_shadow_draws;
        }

        let ao = plan.prepass.then(|| {
            let ao = targets
                .ao
                .get_or_init(|| AoTargets::new(&self.device, &self.layouts, targets.size));
            let frame = &self.frame;
            let group = ao.frame_bind_group(frame.frame_generation, |ao, depth, scene_depth| {
                frame_bind_group(
                    &self.device,
                    &self.layouts,
                    [&frame.globals_buffer, &frame.light_buffer],
                    &frame.shadow_array_view,
                    &frame.shadow_sampler,
                    [ao, depth, scene_depth],
                    frame.local_shadow_view(),
                )
            });
            (ao, group)
        });
        let ao_group = ao.as_ref().map(|(_, g)| g.clone());
        let ao = ao.map(|(a, _)| a);
        if let Some(ao) = ao {
            profiling::scope!("prepass and ao passes");
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("depth normal prepass"),
                    timestamp_writes: self.timestamps("prepass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &ao.prepass_normal,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &ao.prepass_depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.frame.frame_bind_group, &[]);
                pass.set_bind_group(1, &self.instances.bind_group, &[]);
                let mut state = DrawState::default();
                for b in &self.prepass_batches {
                    if let Some(tris) = self.draw_batch(&mut pass, b, Pass::Prepass, &mut state) {
                        draws += 1;
                        instances += b.count;
                        triangles += tris;
                    }
                }
            }
            let fullscreen = |encoder: &mut wgpu::CommandEncoder,
                              label: &'static str,
                              targets: &[Option<wgpu::RenderPassColorAttachment<'_>>],
                              pipeline: &wgpu::RenderPipeline,
                              group: &wgpu::BindGroup| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    timestamp_writes: self.timestamps(label),
                    color_attachments: targets,
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.frame.frame_bind_group, &[]);
                pass.set_bind_group(1, group, &[]);
                pass.draw(0..3, 0..1);
            };
            let attach = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            if plan.ao {
                fullscreen(
                    &mut encoder,
                    "ao resolve",
                    &[attach(&ao.half_depth), attach(&ao.half_normal)],
                    &self.pipelines.ao_resolve,
                    &ao.resolve_bind_group,
                );
                fullscreen(
                    &mut encoder,
                    "ssao",
                    &[attach(&ao.raw)],
                    &self.pipelines.ao,
                    &ao.compute_bind_group,
                );
                fullscreen(
                    &mut encoder,
                    "ao blur",
                    &[attach(&ao.ao)],
                    &self.pipelines.ao_blur,
                    &ao.blur_bind_group,
                );
                draws += 3;
            }
        }

        {
            profiling::scope!("scene pass");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hdr scene pass"),
                timestamp_writes: self.timestamps("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&targets.hdr_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        // Kept only when display lines or the selection
                        // outline test against it after tone mapping.
                        store: if plan.lines[LineLayer::Display as usize] > 0
                            || plan.outline.is_some()
                        {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(
                0,
                ao_group.as_ref().unwrap_or(&self.frame.frame_bind_group),
                &[],
            );
            pass.set_bind_group(1, &self.instances.bind_group, &[]);
            let mut state = DrawState::default();
            for b in &self.main_batches {
                if let Some(tris) = self.draw_batch(&mut pass, b, Pass::Main, &mut state) {
                    draws += 1;
                    instances += b.count;
                    triangles += tris;
                }
            }

            pass.set_pipeline(&self.pipelines.sky);
            pass.draw(0..3, 0..1);
            draws += 1;

            // Depth-tested scene lines before transparent surfaces, so glass
            // in front of them tints them; overlay lines after.
            let lines = |pass: &mut wgpu::RenderPass<'_>, layer, pipeline| {
                let range = plan.line_range(layer);
                match &self.frame.debug_buffer {
                    Some(buffer) if !range.is_empty() => {
                        pass.set_vertex_buffer(0, buffer.slice(..));
                        pass.set_pipeline(pipeline);
                        pass.draw(range, 0..1);
                        1
                    }
                    _ => 0,
                }
            };
            draws += lines(&mut pass, LineLayer::Scene, &self.pipelines.debug_depth);

            // Transparent surfaces and particle emitters, back to front,
            // after the sky: depth tested against opaque geometry, not
            // written. Both lists are sorted; merge them by depth (meshes
            // first on ties).
            let mut state = DrawState::default();
            let particle_draws = &self.particles.draws;
            let (mut m, mut p) = (0, 0);
            let mut particle_group = false;
            while m < self.transparent_batches.len() || p < particle_draws.len() {
                let mesh_depth = self
                    .transparent_batches
                    .get(m)
                    .map(|b| self.draw_list[b.item as usize].depth);
                let particle_depth = particle_draws.get(p).map(|d| d.depth);
                let mesh_next = match (mesh_depth, particle_depth) {
                    (Some(a), Some(b)) => a >= b,
                    (Some(_), None) => true,
                    _ => false,
                };
                if mesh_next {
                    let b = &self.transparent_batches[m];
                    m += 1;
                    if particle_group {
                        // Group 1 back to the instance data.
                        pass.set_bind_group(1, &self.instances.bind_group, &[]);
                        particle_group = false;
                    }
                    if let Some(tris) = self.draw_batch(&mut pass, b, Pass::Main, &mut state) {
                        draws += 1;
                        transparent_draws += 1;
                        instances += b.count;
                        triangles += tris;
                    }
                    continue;
                }
                let d = particle_draws[p];
                p += 1;
                let (Some(buffer), Some(group)) = (
                    self.particles.buffer(),
                    self.particles.bind_group(d.texture),
                ) else {
                    continue;
                };
                // The next mesh draw rebinds its pipeline, material and mesh.
                state = DrawState::default();
                pass.set_pipeline(if d.additive {
                    &self.pipelines.particle_additive
                } else {
                    &self.pipelines.particle_alpha
                });
                pass.set_bind_group(1, group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..4, d.first..d.first + d.count);
                particle_group = true;
                draws += 1;
                transparent_draws += 1;
                instances += d.count;
                triangles += 2 * d.count as u64;
            }

            draws += lines(
                &mut pass,
                LineLayer::SceneOverlay,
                &self.pipelines.debug_overlay,
            );
        }

        let outline = plan.outline.map(|rect| {
            let ot = targets.outline.get_or_init(|| {
                OutlineTargets::new(
                    &self.device,
                    &self.layouts,
                    targets,
                    &self.frame.outline_buffer,
                )
            });
            (rect, ot)
        });
        if let Some((_, ot)) = outline {
            profiling::scope!("outline mask pass");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline mask pass"),
                timestamp_writes: self.timestamps("outline mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &ot.mask_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &ot.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame.frame_bind_group, &[]);
            pass.set_bind_group(1, &self.instances.bind_group, &[]);
            let mut pipeline = None;
            let mut state = DrawState::default();
            for b in &self.outline_batches {
                let item = &self.draw_list[b.item as usize];
                if item.outline_pipeline != NO_PIPELINE {
                    // A material that moves vertices: its own vertex stage.
                    pipeline = None;
                    if let Some(tris) = self.draw_batch(&mut pass, b, Pass::Outline, &mut state) {
                        draws += 1;
                        instances += b.count;
                        triangles += tris;
                    }
                    continue;
                }
                let (Some(mesh), Some(variant)) = (self.meshes.get(item.mesh), item.outline) else {
                    continue;
                };
                if pipeline != Some(variant) {
                    pipeline = Some(variant);
                    pass.set_pipeline(&self.pipelines.outline_mask[variant as usize]);
                }
                // The next material outline draw rebinds everything.
                state = DrawState::default();
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    item.first_index..item.first_index + item.index_count,
                    0,
                    b.first..b.first + b.count,
                );
                draws += 1;
                instances += b.count;
                triangles += item.index_count as u64 / 3 * b.count as u64;
            }
        }

        let bloom = plan.bloom.map(|scatter| {
            let chain = targets.bloom.get_or_init(|| {
                BloomTargets::new(
                    &self.device,
                    &self.layouts,
                    &targets.hdr_view,
                    self.hdr_format,
                    &self.frame.linear_sampler,
                    targets.size,
                )
            });
            (scatter, chain)
        });
        if let Some((scatter, chain)) = bloom {
            profiling::scope!("bloom passes");
            let mut bloom_pass = |label: &'static str,
                                  view: &wgpu::TextureView,
                                  load: wgpu::LoadOp<wgpu::Color>,
                                  pipeline: &wgpu::RenderPipeline,
                                  group: &wgpu::BindGroup| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    timestamp_writes: self.timestamps("bloom"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.frame.frame_bind_group, &[]);
                pass.set_bind_group(1, group, &[]);
                pass.set_blend_constant(wgpu::Color {
                    r: scatter as f64,
                    g: scatter as f64,
                    b: scatter as f64,
                    a: scatter as f64,
                });
                pass.draw(0..3, 0..1);
            };
            for (mip, view) in chain.mips.iter().enumerate() {
                let pipeline = if mip == 0 {
                    &self.pipelines.bloom_first
                } else {
                    &self.pipelines.bloom_down
                };
                let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
                bloom_pass("bloom downsample", view, clear, pipeline, &chain.down[mip]);
                draws += 1;
            }
            for mip in (1..chain.mips.len()).rev() {
                bloom_pass(
                    "bloom upsample",
                    &chain.mips[mip - 1],
                    wgpu::LoadOp::Load,
                    &self.pipelines.bloom_up,
                    &chain.up[mip],
                );
                draws += 1;
            }
        }

        {
            profiling::scope!("tonemap pass");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap pass"),
                timestamp_writes: self.timestamps("tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let pipeline = self
                .pipelines
                .tonemap
                .get(&output_format)
                .expect("tonemap pipeline for output format");
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.frame.frame_bind_group, &[]);
            pass.set_bind_group(
                1,
                bloom.map_or(&targets.tonemap_bind_group, |(_, b)| &b.tonemap_bind_group),
                &[],
            );
            pass.draw(0..3, 0..1);
            draws += 1;

            // Selection outline, under the display lines (gizmos stay on top).
            if let Some(([x, y, w, h], ot)) = outline {
                pass.set_scissor_rect(x, y, w, h);
                pass.set_pipeline(
                    self.pipelines
                        .outline
                        .get(&output_format)
                        .expect("outline pipeline for output format"),
                );
                pass.set_bind_group(1, &ot.bind_group, &[]);
                pass.draw(0..3, 0..1);
                draws += 1;
                pass.set_scissor_rect(0, 0, targets.size.0, targets.size.1);
            }

            // Display-referred lines on top of the tone-mapped image.
            if let Some(buffer) = &self.frame.debug_buffer
                && plan.lines[2] + plan.lines[3] > 0
            {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.set_bind_group(1, &targets.display_bind_group, &[]);
                for (layer, pipelines) in [
                    (LineLayer::Display, &self.pipelines.display_depth),
                    (LineLayer::DisplayOverlay, &self.pipelines.display_overlay),
                ] {
                    let range = plan.line_range(layer);
                    if !range.is_empty() {
                        pass.set_pipeline(
                            pipelines
                                .get(&output_format)
                                .expect("display line pipeline for output format"),
                        );
                        pass.draw(range, 0..1);
                        draws += 1;
                    }
                }
            }
        }

        (
            encoder,
            FrameCounts {
                draws,
                shadow_draws,
                local_shadow_draws,
                transparent_draws,
                instances,
                triangles,
            },
        )
    }

    /// Records one instanced draw in `pass`, changing pipeline, material
    /// bind group and buffers only when they differ from `state`. Returns
    /// the triangle count, or `None` when the draw was skipped.
    fn draw_batch(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        b: &Batch,
        which: Pass,
        state: &mut DrawState,
    ) -> Option<u64> {
        let item = &self.draw_list[b.item as usize];
        let (pipeline_id, material) = match which {
            Pass::Main => main_state(item),
            Pass::Shadow => shadow_state(item),
            Pass::Prepass => prepass_state(item),
            Pass::Outline => outline_state(item),
        };
        let mesh = self.meshes.get(item.mesh)?;
        let pipeline = self.pipeline_cache.get(pipeline_id)?;
        let entry = self.materials.get(item.material)?;
        if state.pipeline != pipeline_id {
            pass.set_pipeline(pipeline);
            state.pipeline = pipeline_id;
            state.material = None;
        }
        let binds_material =
            matches!(which, Pass::Main | Pass::Outline) || entry.shadow_binds_material;
        if binds_material && state.material != Some(material) {
            if let Some(bind_group) = &entry.bind_group {
                pass.set_bind_group(2, bind_group, &[]);
            }
            state.material = Some(material);
        }
        if state.mesh != Some((item.mesh, item.skinned)) {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            if item.skinned {
                pass.set_vertex_buffer(1, mesh.skin.as_ref()?.buffer.slice(..));
            }
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            state.mesh = Some((item.mesh, item.skinned));
        }
        pass.draw_indexed(
            item.first_index..item.first_index + item.index_count,
            0,
            b.first..b.first + b.count,
        );
        Some(item.index_count as u64 / 3 * b.count as u64)
    }
}

/// Bound state while recording draws, to skip redundant calls.
struct DrawState {
    pipeline: u32,
    material: Option<u64>,
    /// Bound mesh, and whether its joint buffer is bound too.
    mesh: Option<(u64, bool)>,
}

impl Default for DrawState {
    fn default() -> Self {
        Self {
            pipeline: NO_PIPELINE,
            material: None,
            mesh: None,
        }
    }
}

/// Counters of one encoded frame.
struct FrameCounts {
    draws: u32,
    shadow_draws: u32,
    local_shadow_draws: u32,
    transparent_draws: u32,
    instances: u32,
    triangles: u64,
}

impl GlobalsUniform {
    fn zeroed_default() -> Self {
        let mut g: Self = bytemuck::Zeroable::zeroed();
        g.sun_dir = [0.0, 1.0, 0.0, 0.0];
        g.post = [1.0, 0.0, 0.0, 0.0];
        g
    }
}

/// Pixel rectangle `[x0, y0, x1, y1]` covering `aabb` on a `w` x `h` target
/// (y down). The whole target when the box reaches behind the camera.
fn screen_rect(view_proj: &Mat4, aabb: &Aabb, (w, h): (u32, u32)) -> [f32; 4] {
    let full = [0.0, 0.0, w as f32, h as f32];
    let mut rect = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
            if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
            if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
        );
        let clip = *view_proj * Vec4::from((corner, 1.0));
        if clip.w <= 1e-5 {
            return full;
        }
        let x = (clip.x / clip.w * 0.5 + 0.5) * w as f32;
        let y = (0.5 - clip.y / clip.w * 0.5) * h as f32;
        rect = [
            rect[0].min(x),
            rect[1].min(y),
            rect[2].max(x),
            rect[3].max(y),
        ];
    }
    rect
}

/// `rect` grown by `margin` pixels and clipped to the target, as a scissor
/// rectangle `[x, y, width, height]`; `None` when nothing is left.
fn scissor(rect: [f32; 4], margin: f32, (w, h): (u32, u32)) -> Option<[u32; 4]> {
    let x0 = (rect[0] - margin).floor().clamp(0.0, w as f32) as u32;
    let y0 = (rect[1] - margin).floor().clamp(0.0, h as f32) as u32;
    let x1 = (rect[2] + margin).ceil().clamp(0.0, w as f32) as u32;
    let y1 = (rect[3] + margin).ceil().clamp(0.0, h as f32) as u32;
    (x1 > x0 && y1 > y0).then(|| [x0, y0, x1 - x0, y1 - y0])
}

/// `width` x `height` clamped to 1..=the device's maximum texture size.
fn clamp_size(device: &wgpu::Device, width: u32, height: u32) -> (u32, u32) {
    let max = device.limits().max_texture_dimension_2d;
    (width.clamp(1, max), height.clamp(1, max))
}

/// Runs `create` inside validation, internal and out-of-memory error scopes
/// and returns the first error one of them caught. A validation scope alone
/// is not enough for pipelines: a shader naga accepts but the backend's
/// compiler (MSL, HLSL) rejects is reported as an internal error.
pub(crate) fn catch_errors<T>(
    device: &wgpu::Device,
    create: impl FnOnce() -> T,
) -> Result<T, String> {
    let scopes = [
        wgpu::ErrorFilter::OutOfMemory,
        wgpu::ErrorFilter::Internal,
        wgpu::ErrorFilter::Validation,
    ]
    .map(|filter| device.push_error_scope(filter));
    let value = create();
    // Scopes pop in reverse order of pushing.
    let mut error = None;
    for scope in scopes.into_iter().rev() {
        error = error.or(pollster::block_on(scope.pop()));
    }
    match error {
        Some(e) => Err(e.to_string()),
        None => Ok(value),
    }
}

/// GPU backend validation for renderers created from now on: 0 = build
/// default, 1 = off, 2 = on. See [`set_gpu_validation`].
static GPU_VALIDATION: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Turns wgpu's backend validation and debug labels (`InstanceFlags::VALIDATION`
/// and `DEBUG`) on or off for renderers created after the call; `None`
/// restores the build default (on in debug builds of wgpu, off in release).
/// `WGPU_VALIDATION`/`WGPU_DEBUG` still override. wgpu's own API validation
/// (counted in `FrameStats::validation_errors`) is always on. Exported
/// players turn it on for the development profile.
pub fn set_gpu_validation(on: Option<bool>) {
    let v = match on {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    GPU_VALIDATION.store(v, std::sync::atomic::Ordering::Relaxed);
}

fn create_instance() -> wgpu::Instance {
    let default_backends = if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        wgpu::Backends::METAL
    } else if cfg!(windows) {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::PRIMARY
    };
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = default_backends;
    desc.flags = wgpu::InstanceFlags::from_build_config();
    let debug = wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG;
    match GPU_VALIDATION.load(std::sync::atomic::Ordering::Relaxed) {
        1 => desc.flags.remove(debug),
        2 => desc.flags.insert(debug),
        _ => {}
    }
    // WGPU_BACKEND, WGPU_VALIDATION, WGPU_DEBUG, ... override the defaults.
    wgpu::Instance::new(desc.with_env())
}

/// Picks the best adapter: DX12/Metal before Vulkan before GL, discrete before
/// integrated before software (integrated first with `WGPU_POWER_PREF=low`).
fn pick_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, RenderError> {
    let low_power = wgpu::PowerPreference::from_env() == Some(wgpu::PowerPreference::LowPower);
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    adapters
        .into_iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .min_by_key(|a| {
            let info = a.get_info();
            let backend = match info.backend {
                wgpu::Backend::Dx12 | wgpu::Backend::Metal => 0,
                wgpu::Backend::Vulkan => 1,
                wgpu::Backend::Gl => 2,
                _ => 3,
            };
            let device = match info.device_type {
                wgpu::DeviceType::DiscreteGpu => {
                    if low_power {
                        1
                    } else {
                        0
                    }
                }
                wgpu::DeviceType::IntegratedGpu => {
                    if low_power {
                        0
                    } else {
                        1
                    }
                }
                wgpu::DeviceType::VirtualGpu => 2,
                wgpu::DeviceType::Other => 3,
                wgpu::DeviceType::Cpu => 4,
            };
            (backend, device)
        })
        .ok_or(RenderError::NoAdapter)
}

/// `Rg11b10Ufloat` when the adapter can render, multisample (4x) and resolve
/// it; otherwise `Rgba16Float`.
fn choose_hdr_format(adapter: &wgpu::Adapter) -> wgpu::TextureFormat {
    let format = wgpu::TextureFormat::Rg11b10Ufloat;
    let features = adapter.get_texture_format_features(format);
    let usable = adapter
        .features()
        .contains(wgpu::Features::RG11B10UFLOAT_RENDERABLE)
        && features.allowed_usages.contains(
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        && features.flags.contains(
            wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4
                | wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE,
        );
    if usable {
        format
    } else {
        wgpu::TextureFormat::Rgba16Float
    }
}

impl Targets {
    fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        frame: &FrameResources,
        hdr_format: wgpu::TextureFormat,
        (w, h): (u32, u32),
    ) -> Self {
        let texture = |label: &str, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = texture(
            "hdr msaa color",
            hdr_format,
            MSAA_SAMPLES,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let hdr = texture(
            "hdr resolved color",
            hdr_format,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture(
            "scene depth",
            DEPTH_FORMAT,
            MSAA_SAMPLES,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let msaa_view = msaa.create_view(&Default::default());
        let hdr_view = hdr.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let tonemap_bind_group = post::tonemap_bind_group(
            device,
            layouts,
            &hdr_view,
            &frame.placeholder_bloom,
            &wgpu::BindingResource::Sampler(&frame.linear_sampler),
        );
        let display_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display lines bind group"),
            layout: &layouts.display_lines,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&depth_view),
            }],
        });
        Self {
            _msaa: msaa,
            msaa_view,
            _hdr: hdr,
            hdr_view,
            _depth: depth,
            depth_view,
            tonemap_bind_group,
            display_bind_group,
            size: (w, h),
            outline: OnceLock::new(),
            ao: OnceLock::new(),
            bloom: OnceLock::new(),
        }
    }
}

impl OutlineTargets {
    fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        targets: &Targets,
        params: &wgpu::Buffer,
    ) -> Self {
        let (w, h) = targets.size;
        let texture = |label: &str, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let mask = texture("outline mask", OUTLINE_MASK_FORMAT);
        let depth = texture("outline mask depth", DEPTH_FORMAT);
        let mask_view = mask.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline bind group"),
            layout: &layouts.outline,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mask_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&targets.depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        Self {
            _mask: mask,
            mask_view,
            _depth: depth,
            depth_view,
            bind_group,
        }
    }
}

/// Group 0 of scene passes: globals, shadow map and sampler, local lights,
/// AO, AO depth, the prepass (scene) depth and the local shadow map.
fn frame_bind_group(
    device: &wgpu::Device,
    layouts: &Layouts,
    [globals, lights]: [&wgpu::Buffer; 2],
    shadow_map: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
    [ao, ao_depth, scene_depth]: [&wgpu::TextureView; 3],
    local_shadow: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame bind group"),
        layout: &layouts.frame,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(shadow_map),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: lights.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(ao),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(ao_depth),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(scene_depth),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(local_shadow),
            },
        ],
    })
}

/// The shadowed local light's depth map: one layer per view.
struct LocalShadowMap {
    _texture: wgpu::Texture,
    array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
    resolution: u32,
    layers: u32,
}

impl LocalShadowMap {
    fn new(device: &wgpu::Device, resolution: u32, layers: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("local shadow map"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("local shadow array view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_views = (0..layers)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("local shadow layer view"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        Self {
            _texture: texture,
            array_view,
            layer_views,
            resolution,
            layers,
        }
    }
}

impl FrameResources {
    fn new(device: &wgpu::Device, layouts: &Layouts, hdr_format: wgpu::TextureFormat) -> Self {
        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals uniform"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow cascades"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth_or_array_layers: CASCADE_COUNT as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_array_view = shadow_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shadow cascades array view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadow_layer_views = (0..CASCADE_COUNT as u32)
            .map(|layer| {
                shadow_texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("shadow cascade layer view"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow comparison sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        // Sized for the cap, so it never grows (8 KiB).
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point lights"),
            size: LIGHT_BUFFER_SIZE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let placeholder = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let placeholders = vec![
            placeholder("ao placeholder", AO_FORMAT),
            placeholder("ao depth placeholder", AO_DEPTH_FORMAT),
            placeholder("bloom placeholder", hdr_format),
            placeholder("scene depth placeholder", DEPTH_FORMAT),
            placeholder("local shadow placeholder", SHADOW_FORMAT),
        ];
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let (placeholder_ao, placeholder_ao_depth, placeholder_bloom, placeholder_depth) = (
            view(&placeholders[0]),
            view(&placeholders[1]),
            view(&placeholders[2]),
            view(&placeholders[3]),
        );
        let placeholder_local_shadow = placeholders[4].create_view(&wgpu::TextureViewDescriptor {
            label: Some("local shadow placeholder view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let frame_bind_group = frame_bind_group(
            device,
            layouts,
            [&globals_buffer, &light_buffer],
            &shadow_array_view,
            &shadow_sampler,
            [&placeholder_ao, &placeholder_ao_depth, &placeholder_depth],
            &placeholder_local_shadow,
        );
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shadow_pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow pass uniforms"),
            size: (CASCADE_COUNT + LOCAL_SHADOW_VIEWS) as u64 * UNIFORM_SLOT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_pass_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow pass bind group"),
            layout: &layouts.shadow_pass,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &shadow_pass_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(64),
                    }),
                },
            ],
        });
        let outline_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("outline uniform"),
            size: OUTLINE_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            globals_buffer,
            frame_bind_group,
            shadow_array_view,
            shadow_sampler,
            linear_sampler,
            _placeholders: placeholders,
            placeholder_bloom,
            _shadow_texture: shadow_texture,
            shadow_layer_views,
            shadow_pass_buffer,
            shadow_pass_bind_group,
            light_buffer,
            local_shadow: None,
            placeholder_local_shadow,
            placeholder_ao,
            placeholder_ao_depth,
            placeholder_depth,
            frame_generation: 0,
            debug_buffer: None,
            outline_buffer,
        }
    }

    /// The local shadow map view frame bind groups bind.
    fn local_shadow_view(&self) -> &wgpu::TextureView {
        self.local_shadow
            .as_ref()
            .map_or(&self.placeholder_local_shadow, |m| &m.array_view)
    }

    /// Makes sure the local shadow map is `resolution`² with at least
    /// `views` layers, reallocating it (and remaking the frame bind groups)
    /// only when that changes. A map with more layers than needed is kept.
    fn ensure_local_shadow(
        &mut self,
        device: &wgpu::Device,
        layouts: &Layouts,
        resolution: u32,
        views: usize,
    ) {
        let fits = self
            .local_shadow
            .as_ref()
            .is_some_and(|m| m.resolution == resolution && m.layers as usize >= views);
        if fits {
            return;
        }
        self.local_shadow = Some(LocalShadowMap::new(device, resolution, views as u32));
        self.frame_generation += 1;
        self.frame_bind_group = frame_bind_group(
            device,
            layouts,
            [&self.globals_buffer, &self.light_buffer],
            &self.shadow_array_view,
            &self.shadow_sampler,
            [
                &self.placeholder_ao,
                &self.placeholder_ao_depth,
                &self.placeholder_depth,
            ],
            self.local_shadow_view(),
        );
    }

    /// Ensures the debug line buffer holds `bytes`; returns true if it was
    /// (re)allocated because it had to grow.
    fn ensure_debug_capacity(&mut self, device: &wgpu::Device, bytes: u64) -> bool {
        let current = self.debug_buffer.as_ref().map_or(0, |b| b.size());
        if bytes <= current {
            return false;
        }
        let size = (current * 2)
            .max(bytes)
            .max(64 * 1024)
            .next_multiple_of(256);
        self.debug_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug lines"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        current > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::color::Color;
    use box_core::transform::{Transform, propagate_transforms};

    fn headless() -> Option<Renderer> {
        headless_with(OPTIONAL_FEATURES)
    }

    fn headless_with(optional_features: wgpu::Features) -> Option<Renderer> {
        if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
            return None;
        }
        let instance = create_instance();
        let created = pick_adapter(&instance, None).and_then(|adapter| {
            Renderer::new_inner(instance, adapter, None, 64, 64, optional_features)
        });
        match created {
            Ok(r) => Some(r),
            Err(RenderError::NoAdapter) => None,
            Err(e) => panic!("{e}"),
        }
    }

    /// A pillar far toward a low sun shadows the ground in front of the
    /// camera, with depth clamp and, without the feature, with pancaking.
    #[test]
    fn casters_before_the_cascade_box_cast_with_and_without_depth_clamp() {
        let (Some(mut clamp), Some(mut pancake)) =
            (headless(), headless_with(wgpu::Features::empty()))
        else {
            eprintln!("skipping: no GPU");
            return;
        };
        assert!(
            !pancake
                .device
                .features()
                .contains(wgpu::Features::DEPTH_CLIP_CONTROL)
        );
        let mut world = World::new();
        world.resource_or_default::<Environment>().fog_density = 0.0;
        let material = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let meshes = world.resource_or_default::<Assets<MeshData>>();
        let ground = meshes.add(MeshData::grid(400.0, 400.0, 4, 4));
        let cube = meshes.add(MeshData::cuboid(Vec3::ONE));
        world.spawn((Transform::default(), MeshRenderer::new(ground, material)));
        world.spawn((
            Transform::from_xyz(-60.0, 30.0, 0.0).with_scale(Vec3::new(2.0, 60.0, 2.0)),
            MeshRenderer::new(cube, material),
        ));
        world.spawn((
            Transform::from_xyz(0.0, 3.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
            Camera::default(),
        ));
        world.spawn((
            Transform::default().looking_to(Vec3::new(1.0, -0.25, 0.0), Vec3::Y),
            DirectionalLight {
                shadow_distance: 12.0,
                ..Default::default()
            },
        ));
        propagate_transforms(&mut world);

        let a = clamp.capture(&mut world).unwrap();
        assert_eq!(clamp.stats().shadow_casters, 2);
        let b = pancake.capture(&mut world).unwrap();
        let sum = |p: &image::Rgba<u8>| p.0[..3].iter().map(|&c| c as u32).sum::<u32>();
        for img in [&a, &b] {
            // The shadow runs through the centre; the bottom edge is lit.
            let (dark, lit) = (sum(img.get_pixel(32, 32)), sum(img.get_pixel(32, 60)));
            assert!(dark * 10 < lit * 9, "shadowed {dark}, lit {lit}");
        }
        let diff = crate::golden::compare(&a, &b, &crate::golden::DEFAULT_TOLERANCE);
        assert!(diff.is_ok(), "pancaking matches depth clamp: {diff:?}");
        assert_eq!(clamp.validation_errors() + pancake.validation_errors(), 0);
    }

    /// A shader naga accepts but the backend compiler may reject (Metal
    /// refuses the 64 MiB local array) is an internal error, not a validation
    /// error. It must be returned, never reach the uncaptured-error handler.
    /// Metal only: DX12's compiler accepts the array but takes 30-40 minutes.
    #[test]
    fn catch_errors_catches_backend_shader_failures() {
        let Some(r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        if r.adapter_info.backend != wgpu::Backend::Metal {
            eprintln!("skipping: only Metal rejects this shader");
            return;
        }
        let source = "
            @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                var big: array<f32, 16777216>;
                big[i] = 1.0;
                return vec4<f32>(big[i + 1u], 0.0, 0.0, 1.0);
            }
            @fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }";
        let result = catch_errors(&r.device, || {
            let module = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("backend reject"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            r.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("backend reject"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(CAPTURE_FORMAT.into())],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
        });
        eprintln!(
            "--- reported error ---\n{:?}\n----------------------",
            result.as_ref().err()
        );
        assert_eq!(r.validation_errors(), 0, "the error escaped the scopes");
        let err = result.expect_err("Metal rejects the shader");
        assert!(err.contains("Internal"), "{err}");
    }

    #[test]
    fn catch_errors_reports_and_swallows_validation_errors() {
        let Some(r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        assert_eq!(catch_errors(&r.device, || 7), Ok(7));
        let err = catch_errors(&r.device, || {
            r.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("too large"),
                size: wgpu::Extent3d {
                    width: u32::MAX,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CAPTURE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        })
        .unwrap_err();
        assert!(err.contains("too large"), "{err}");
        // Caught errors do not reach the uncaptured-error counter, and the
        // scopes are balanced (a later error is still counted).
        assert_eq!(r.validation_errors(), 0);
    }

    /// A lost device (simulated with `destroy`) makes frames fail with a
    /// clear error instead of hanging or drawing nothing.
    #[test]
    fn a_lost_device_fails_frames_with_device_lost() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let mut world = World::new();
        r.render(&mut world).unwrap();
        assert_eq!(r.device_lost(), None);
        r.device.destroy();
        let _ = r.device.poll(wgpu::PollType::wait_indefinitely());
        assert!(r.device_lost().is_some(), "the callback ran");
        let e = r.render(&mut world).unwrap_err();
        assert!(matches!(e, RenderError::DeviceLost(_)), "{e:?}");
        let text = e.to_string();
        assert!(text.contains("GPU device was lost"), "{text}");
        assert!(text.contains("update the graphics driver"), "{text}");
        assert!(matches!(
            r.capture(&mut world),
            Err(RenderError::DeviceLost(_))
        ));
    }

    #[test]
    fn no_adapter_says_what_is_needed() {
        let text = RenderError::NoAdapter.to_string();
        assert!(text.starts_with("no GPU adapter found"), "{text}");
        assert!(text.contains("box-engine needs"), "{text}");
    }

    #[test]
    fn hot_reload_keeps_previous_pipelines_on_error() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let dir = std::env::temp_dir().join(format!("box_render_hot_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");
        for entry in std::fs::read_dir(&src).unwrap() {
            let p = entry.unwrap().path();
            std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
        r.shaders.set_dir(dir.clone());

        // A good reload bumps the generation.
        r.reload_shaders().unwrap();
        assert_eq!(r.pipeline_generation, 1);

        // A syntax error is rejected by naga before touching the GPU.
        let sky = std::fs::read_to_string(dir.join("sky.wgsl")).unwrap();
        std::fs::write(dir.join("sky.wgsl"), sky.replace("1.0);\n}", "1.0)\n}")).unwrap();
        let err = r.reload_shaders().unwrap_err();
        eprintln!("--- reported error ---\n{err}\n----------------------");
        assert!(err.contains("(at sky.wgsl:"), "{err}");
        assert_eq!(r.pipeline_generation, 1, "previous pipelines must be kept");

        // A binding mismatch passes naga but fails pipeline creation; it is
        // caught by the error scope and does not count as an uncaptured error.
        std::fs::write(dir.join("sky.wgsl"), &sky).unwrap();
        let object = std::fs::read_to_string(dir.join("object.wgsl")).unwrap();
        std::fs::write(
            dir.join("object.wgsl"),
            object.replace("@group(1) @binding(0)", "@group(1) @binding(3)"),
        )
        .unwrap();
        let err = r.reload_shaders().unwrap_err();
        eprintln!("--- reported error ---\n{err}\n----------------------");
        assert!(err.contains("pipeline creation failed"), "{err}");
        assert_eq!(r.pipeline_generation, 1);
        std::fs::write(dir.join("object.wgsl"), &object).unwrap();

        // The old pipelines still render.
        let mut world = World::new();
        world.spawn((Transform::default(), Camera::default()));
        propagate_transforms(&mut world);
        let img = r.capture(&mut world).unwrap();
        assert_eq!(img.dimensions(), (64, 64));
        assert_eq!(r.stats().validation_errors, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mesh_buffers_are_reused_and_grow_by_doubling() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let mut world = World::new();
        without_post_effects(&mut world);
        let mesh = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::cuboid(Vec3::ONE));
        let material = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        world.spawn((
            Transform::from_xyz(0.0, 0.0, -5.0),
            MeshRenderer::new(mesh, material),
        ));
        world.spawn((Transform::default(), Camera::default()));
        propagate_transforms(&mut world);

        r.render(&mut world).unwrap();
        let first_upload = r.stats().upload_bytes;
        assert_eq!(r.stats().objects, 1);
        assert_eq!(r.stats().visible, 1);
        assert_eq!(r.stats().buffer_reallocations, 0);

        // Unchanged mesh: no mesh upload.
        r.render(&mut world).unwrap();
        assert!(r.stats().upload_bytes < first_upload);

        // Same-size edit: rewrite in place, no reallocation.
        world
            .resource_mut::<Assets<MeshData>>()
            .get_mut(mesh)
            .unwrap()
            .positions[0] += Vec3::X;
        r.render(&mut world).unwrap();
        // The first frame also wrote the material uniform, the instance data
        // and the draw slot list (one slot), once each.
        let once = std::mem::size_of::<crate::uniforms::StandardMaterialUniform>()
            + std::mem::size_of::<crate::uniforms::ObjectUniform>()
            + 4;
        assert_eq!(r.stats().upload_bytes, first_upload - once as u64);
        assert_eq!(r.stats().buffer_reallocations, 0);

        // Growth past the headroom: both buffers reallocate once.
        *world
            .resource_mut::<Assets<MeshData>>()
            .get_mut(mesh)
            .unwrap() = MeshData::uv_sphere(1.0, 32, 16);
        r.render(&mut world).unwrap();
        assert_eq!(r.stats().buffer_reallocations, 2);

        // Removal frees the GPU mesh.
        world.resource_mut::<Assets<MeshData>>().remove(mesh);
        r.render(&mut world).unwrap();
        assert_eq!(r.meshes.len(), 0);
        assert_eq!(r.stats().objects, 0);
        assert_eq!(r.stats().validation_errors, 0);
    }

    /// Objects sharing a mesh and material are one instanced draw per pass;
    /// instance data is written only when a transform changes; despawned
    /// and hidden objects give their slots back.
    #[test]
    fn identical_objects_draw_instanced_and_upload_only_changes() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let mut world = lit_cube_world();
        without_post_effects(&mut world);
        world.spawn((
            Transform::from_xyz(0.0, 3.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
            Camera::default(),
        ));
        let (mesh, material) = {
            let (_, mr) = world
                .query::<(Entity, &MeshRenderer)>()
                .iter()
                .next()
                .map(|(e, m)| (e, m.clone()))
                .unwrap();
            (mr.mesh, mr.materials[0])
        };
        let cubes: Vec<Entity> = (0..200)
            .map(|i| {
                world.spawn((
                    Transform::from_xyz(
                        (i % 20) as f32 * 1.5 - 15.0,
                        0.0,
                        -((i / 20) as f32) * 1.5,
                    ),
                    MeshRenderer::new(mesh, material),
                ))
            })
            .collect();
        propagate_transforms(&mut world);
        r.render(&mut world).unwrap();
        let s = r.stats().clone();
        assert_eq!(s.objects, 201);
        // One main draw for all visible cubes, plus sky and tone mapping.
        assert_eq!(s.draws - s.shadow_draws, 3, "{s:?}");
        assert!(s.shadow_draws <= 4 && s.instances > 200, "{s:?}");
        assert_eq!(r.instances.written, 201);

        // Idle: no instance data and no draw list written.
        r.render(&mut world).unwrap();
        assert_eq!(r.instances.written, 0);
        let idle_upload = r.stats().upload_bytes;
        // One moved cube: one chunk of slots rewritten.
        world
            .component_mut::<Transform>(cubes[7])
            .unwrap()
            .translation
            .y += 0.5;
        propagate_transforms(&mut world);
        r.render(&mut world).unwrap();
        assert!(
            r.instances.written as usize <= 64,
            "{}",
            r.instances.written
        );
        assert!(r.stats().upload_bytes > idle_upload);

        // Hidden and despawned cubes are not drawn and free their slots.
        world
            .insert_one(cubes[0], crate::components::Hidden)
            .unwrap();
        assert!(world.despawn(cubes[1]));
        r.render(&mut world).unwrap();
        assert_eq!(r.stats().objects, 199);
        assert_eq!(r.instances.len(), 199);
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn meshes_of_a_replaced_world_are_freed() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let world = || {
            let mut world = lit_cube_world();
            world.spawn((Transform::from_xyz(0.0, 0.0, 4.0), Camera::default()));
            propagate_transforms(&mut world);
            world
        };
        let mut first = world();
        r.render(&mut first).unwrap();
        assert_eq!((r.meshes.len(), r.stats().objects), (1, 1));
        // A new world has its own asset store; the old one never reports its
        // meshes as removed.
        drop(first);
        let mut second = world();
        r.render(&mut second).unwrap();
        assert_eq!((r.meshes.len(), r.stats().objects), (1, 1));
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn rejected_meshes_are_checked_once_per_version() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let mut world = lit_cube_world();
        let mut invalid = MeshData::cuboid(Vec3::ONE);
        invalid.indices[0] = 9999;
        assert!(invalid.validate().is_err());
        let mut empty = MeshData::cuboid(Vec3::ONE);
        empty.indices.clear();
        let assets = world.resource_mut::<Assets<MeshData>>();
        let invalid = assets.add(invalid);
        assets.add(empty);
        for _ in 0..3 {
            r.render(&mut world).unwrap();
        }
        // The cube, the invalid mesh and the empty mesh: once each.
        assert_eq!(r.meshes.upload_attempts, 3);
        assert_eq!(r.meshes.len(), 1);

        // A new version is checked again, and drawn once it is valid.
        world
            .resource_mut::<Assets<MeshData>>()
            .get_mut(invalid)
            .unwrap()
            .indices[0] = 0;
        r.render(&mut world).unwrap();
        r.render(&mut world).unwrap();
        assert_eq!(r.meshes.upload_attempts, 4);
        assert_eq!(r.meshes.len(), 2);

        // Removing a rejected mesh forgets it.
        let assets = world.resource_mut::<Assets<MeshData>>();
        assets.get_mut(invalid).unwrap().indices[0] = 9999;
        r.render(&mut world).unwrap();
        world.resource_mut::<Assets<MeshData>>().remove(invalid);
        r.render(&mut world).unwrap();
        assert_eq!(r.meshes.tracked(), 2);
        assert_eq!(r.validation_errors(), 0);
    }

    /// Turns ambient occlusion and bloom off (tests of draw counts, uploads
    /// and exact pixels that are not about those effects).
    fn without_post_effects(world: &mut World) {
        let env = world.resource_or_default::<Environment>();
        env.ambient_occlusion.enabled = false;
        env.bloom.enabled = false;
    }

    fn lit_cube_world() -> World {
        let mut world = World::new();
        let mesh = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::cuboid(Vec3::ONE));
        let material = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        world.spawn((Transform::default(), MeshRenderer::new(mesh, material)));
        world.spawn((
            Transform::default().looking_to(Vec3::new(0.3, -1.0, -0.4), Vec3::Y),
            DirectionalLight::default(),
        ));
        propagate_transforms(&mut world);
        world
    }

    #[test]
    fn display_lines_ignore_exposure_and_respect_scene_depth() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let magenta = [255u8, 0, 255, 255];
        let eye = Transform::from_xyz(0.0, 0.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y);
        let camera = ViewCamera::new(&eye, Camera::default());
        let target = r.create_viewport_target(64, 64);
        // A line behind the cube, across the whole view.
        let draw = |layer: LineLayer| {
            let mut world = lit_cube_world();
            world.resource_or_default::<Environment>().exposure = 8.0;
            world.resource_or_default::<DebugDraw>().line_on(
                layer,
                Vec3::new(-20.0, 0.0, -2.0),
                Vec3::new(20.0, 0.0, -2.0),
                Color::srgb(1.0, 0.0, 1.0),
            );
            world
        };
        let row_of_line = |img: &image::RgbaImage| {
            (0..64)
                .find(|&y| img.get_pixel(1, y).0 == magenta)
                .expect("line visible over the sky with its exact colour")
        };

        let mut world = draw(LineLayer::Display);
        r.render_to(&mut world, &target, Some(&camera));
        let img = r.read_viewport_target(&target).unwrap();
        let y = row_of_line(&img);
        assert_ne!(img.get_pixel(32, y).0, magenta, "hidden behind the cube");

        let mut world = draw(LineLayer::DisplayOverlay);
        r.render_to(&mut world, &target, Some(&camera));
        let img = r.read_viewport_target(&target).unwrap();
        let y = row_of_line(&img);
        assert_eq!(img.get_pixel(32, y).0, magenta, "drawn on top of the cube");
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn viewport_target_matches_capture_with_explicit_camera() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let eye = Transform::from_xyz(2.0, 1.5, 3.0).looking_at(Vec3::ZERO, Vec3::Y);

        // Reference: the camera is an entity, rendered by `capture`.
        let mut world = lit_cube_world();
        world.spawn((eye, Camera::default()));
        propagate_transforms(&mut world);
        r.resize(96, 64);
        let expected = r.capture(&mut world).unwrap();

        // Same view through an explicit camera into a viewport target; the
        // world has no camera entity at all.
        let mut world = lit_cube_world();
        let mut target = r.create_viewport_target(32, 32);
        assert!(r.resize_viewport_target(&mut target, 96, 64));
        assert!(!r.resize_viewport_target(&mut target, 96, 64));
        assert_eq!(target.size(), (96, 64));
        r.render_to(
            &mut world,
            &target,
            Some(&ViewCamera::new(&eye, Camera::default())),
        );
        assert_eq!(r.stats().visible, 1);
        let actual = r.read_viewport_target(&target).unwrap();
        let diff = crate::golden::compare(&actual, &expected, &crate::golden::EXACT);
        assert!(
            diff.is_ok(),
            "viewport bytes must equal the capture: {diff:?}"
        );
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn screen_rect_and_scissor() {
        let eye =
            glam::camera::rh::view::look_at_mat4(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y);
        let proj = directx::perspective_infinite_reverse(50f32.to_radians(), 1.0, 0.1);
        let vp = proj * eye;
        let unit = Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5));
        let r = screen_rect(&vp, &unit, (64, 64));
        // Centred and symmetric, inside the target.
        assert!((r[0] + r[2] - 64.0).abs() < 1e-3 && (r[1] + r[3] - 64.0).abs() < 1e-3);
        assert!(r[0] > 16.0 && r[2] < 48.0, "{r:?}");
        let s = scissor(r, 3.0, (64, 64)).unwrap();
        assert!(s[0] as f32 <= r[0] - 3.0 && (s[0] + s[2]) as f32 >= r[2] + 3.0);
        // Reaching behind the camera: the whole target.
        let around = Aabb::new(Vec3::splat(-10.0), Vec3::splat(10.0));
        assert_eq!(screen_rect(&vp, &around, (64, 64)), [0.0, 0.0, 64.0, 64.0]);
        // Entirely off the target: nothing to draw.
        assert_eq!(scissor([100.0, 10.0, 120.0, 20.0], 2.0, (64, 64)), None);
    }

    /// A cube at the origin seen from +Z, optionally a second one behind it,
    /// and the sun. Returns the world and the (front, back) cube entities.
    fn outline_world(back: bool) -> (World, Entity, Option<Entity>) {
        let mut world = World::new();
        let mesh = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::cuboid(Vec3::ONE));
        let material = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let front = world.spawn((Transform::default(), MeshRenderer::new(mesh, material)));
        let back = back.then(|| {
            world.spawn((
                Transform::from_xyz(0.0, 0.0, -3.0),
                MeshRenderer::new(mesh, material),
            ))
        });
        world.spawn((
            Transform::default().looking_to(Vec3::new(0.3, -1.0, -0.4), Vec3::Y),
            DirectionalLight::default(),
        ));
        propagate_transforms(&mut world);
        (world, front, back)
    }

    #[test]
    fn selection_outline_surrounds_the_silhouette_and_dims_when_hidden() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let magenta = [255u8, 0, 255, 255];
        let eye = Transform::from_xyz(0.0, 0.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y);
        let camera = ViewCamera::new(&eye, Camera::default());
        let target = r.create_viewport_target(64, 64);
        let outline = |entities: Vec<Entity>| SelectionOutline {
            primary: entities,
            color: Color::srgb(1.0, 0.0, 1.0),
            width_px: 2.0,
            occluded_alpha: 0.3,
            ..Default::default()
        };
        let mut render = |world: &mut World| {
            r.render_to(world, &target, Some(&camera));
            let draws = r.stats().draws;
            (r.read_viewport_target(&target).unwrap(), draws)
        };

        let (mut world, front, _) = outline_world(false);
        let (plain, plain_draws) = render(&mut world);

        // An empty outline changes nothing and costs nothing.
        world.insert_resource(SelectionOutline::default());
        let (empty, empty_draws) = render(&mut world);
        assert_eq!(empty_draws, plain_draws);
        assert!(crate::golden::compare(&empty, &plain, &crate::golden::EXACT).is_ok());

        // Selected: a solid band just outside the silhouette, the interior
        // untouched. One mask draw plus one composite.
        world.insert_resource(outline(vec![front]));
        let (img, draws) = render(&mut world);
        assert_eq!(draws, plain_draws + 2);
        let first = (0..64)
            .find(|&x| img.get_pixel(x, 32).0 == magenta)
            .expect("outline left of the cube");
        assert!(first > 8 && first < 32, "{first}");
        assert_ne!(plain.get_pixel(first, 32), plain.get_pixel(first + 4, 32));
        assert_eq!(img.get_pixel(32, 32), plain.get_pixel(32, 32), "interior");
        assert_eq!(img.get_pixel(1, 1), plain.get_pixel(1, 1), "far from it");

        // A cube hidden behind the front one: a faint band on the occluder,
        // never full strength.
        let (mut world, _, back) = outline_world(true);
        let (plain, _) = render(&mut world);
        world.insert_resource(outline(vec![back.unwrap()]));
        let (img, _) = render(&mut world);
        assert!(img.pixels().all(|p| p.0 != magenta));
        let changed = (0..64)
            .filter(|&x| img.get_pixel(x, 32) != plain.get_pixel(x, 32))
            .count();
        assert!(changed >= 2, "dimmed outline drawn ({changed} px)");
        assert_eq!(img.get_pixel(32, 32), plain.get_pixel(32, 32));
        assert_eq!(r.validation_errors(), 0);
    }

    fn draw(depth: f32, pipeline: u32, material: u64, mesh: u64) -> DrawItem {
        DrawItem {
            slot: 0,
            mesh,
            first_index: 0,
            index_count: 3,
            material,
            shadow_material: 0,
            main_pipeline: pipeline,
            shadow_pipeline: NO_PIPELINE,
            prepass_pipeline: NO_PIPELINE,
            outline_pipeline: NO_PIPELINE,
            transparent: true,
            cascades: 0,
            local_views: 0,
            depth,
            outline: None,
            skinned: false,
        }
    }

    #[test]
    fn transparent_draws_sort_back_to_front_and_opaque_by_state() {
        let draws = [
            draw(5.0, 0, 1, 1),
            draw(20.0, 0, 1, 1),
            draw(-1.0, 0, 1, 1),
            draw(5.0, 0, 2, 2),
            draw(12.0, 0, 1, 1),
        ];
        let mut order: Vec<u32> = (0..5).collect();
        sort_transparent(&mut order, &draws);
        // Farthest first; equal depths keep submission order.
        assert_eq!(order, [1, 4, 0, 3, 2]);

        let draws = [
            draw(0.0, 2, 1, 1),
            draw(0.0, 1, 9, 1),
            draw(0.0, 1, 3, 7),
            draw(0.0, 1, 3, 2),
        ];
        let mut order: Vec<u32> = (0..4).collect();
        sort_by_state(&mut order, &draws, main_state);
        assert_eq!(order, [3, 2, 1, 0], "pipeline, then material, then mesh");
    }

    /// A camera looking straight down at a 4 m plane at the origin.
    fn top_down_world() -> World {
        let mut world = World::new();
        without_post_effects(&mut world);
        let env = world.resource_or_default::<Environment>();
        env.fog_density = 0.0;
        world.spawn((
            Transform::from_xyz(0.0, 4.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
            Camera::default(),
        ));
        world
    }

    fn luma(img: &image::RgbaImage, x: u32, y: u32) -> u32 {
        img.get_pixel(x, y).0[..3].iter().map(|&c| c as u32).sum()
    }

    /// A wall on a ground plane with a thin pole in front of it, seen from
    /// above the pole's middle, in the ambient occlusion view.
    fn ao_world() -> World {
        let mut world = World::new();
        let env = world.resource_or_default::<Environment>();
        env.fog_density = 0.0;
        env.bloom.enabled = false;
        env.ambient_occlusion.radius = 0.3;
        world
            .resource_or_default::<RenderSettings>()
            .show_ambient_occlusion = true;
        let material = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let meshes = world.resource_or_default::<Assets<MeshData>>();
        let ground = meshes.add(MeshData::plane(40.0, 40.0));
        let wall = meshes.add(MeshData::cuboid(Vec3::new(8.0, 3.0, 0.3)));
        let pole = meshes.add(MeshData::cylinder(0.05, 3.0, 12));
        world.spawn((Transform::default(), MeshRenderer::new(ground, material)));
        world.spawn((
            Transform::from_xyz(0.0, 1.5, -1.0),
            MeshRenderer::new(wall, material),
        ));
        world.spawn((
            Transform::from_xyz(0.0, 1.5, 0.5),
            MeshRenderer::new(pole, material),
        ));
        world.spawn((
            Transform::from_xyz(0.0, 2.5, 5.0).looking_at(Vec3::new(0.0, 0.6, 0.0), Vec3::Y),
            Camera::default(),
        ));
        propagate_transforms(&mut world);
        world
    }

    /// Creases darken, open ground does not, and a thin pole crossing the
    /// crease keeps its own (unoccluded) value: the per-fragment depth-aware
    /// upsample does not bleed the half-resolution crease into it.
    #[test]
    fn ambient_occlusion_darkens_creases_and_keeps_silhouettes_clean() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        r.resize(256, 160);
        let mut world = ao_world();
        let img = r.capture(&mut world).unwrap();
        let out =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
        std::fs::create_dir_all(&out).unwrap();
        img.save(out.join("ao_crease_pole.png")).unwrap();
        let v = |x, y| img.get_pixel(x, y).0[0];
        // The crease: the darkest pixel of a column away from the pole.
        let (crease_y, crease) = (40..150)
            .map(|y| (y, v(60, y)))
            .min_by_key(|p| p.1)
            .unwrap();
        assert!(crease < 225, "crease {crease} at row {crease_y}");
        assert!(v(60, 155) > 245, "open ground {}", v(60, 155));
        assert!(v(60, crease_y - 25) > 235, "wall above the crease");
        // The pole's pixels on the crease row (it is about 3 px wide at x = 128).
        let pole = (124..133).map(|x| v(x, crease_y)).max().unwrap();
        assert!(pole > 235, "pole on the crease row {pole}");
        // Ambient occlusion off: no prepass and no AO passes.
        world
            .resource_mut::<Environment>()
            .ambient_occlusion
            .enabled = false;
        let draws = r.stats().draws;
        r.capture(&mut world).unwrap();
        // The three prepass draws and the three AO passes.
        assert_eq!(r.stats().draws + 6, draws, "prepass and AO passes skipped");
        assert_eq!(r.validation_errors(), 0);
    }

    /// Bloom spreads a bright emissive sphere's light around it; without
    /// bloom the dark surroundings stay dark.
    #[test]
    fn bloom_glows_around_bright_objects() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let mut world = World::new();
        let env = world.resource_or_default::<Environment>();
        *env = Environment {
            sky_zenith: Color::BLACK,
            sky_horizon: Color::BLACK,
            ground: Color::BLACK,
            fog_density: 0.0,
            ..Environment::default()
        };
        env.ambient_occlusion.enabled = false;
        env.bloom.intensity = 0.2;
        let material =
            world
                .resource_or_default::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::BLACK,
                    emissive: Color::linear(40.0, 30.0, 20.0),
                    ..Default::default()
                });
        let sphere = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::uv_sphere(0.3, 24, 12));
        world.spawn((Transform::default(), MeshRenderer::new(sphere, material)));
        world.spawn((Transform::from_xyz(0.0, 0.0, 4.0), Camera::default()));
        propagate_transforms(&mut world);
        let glow = r.capture(&mut world).unwrap();
        world.resource_mut::<Environment>().bloom.enabled = false;
        let plain = r.capture(&mut world).unwrap();
        // Beside the sphere (it spans about 10 px around the centre).
        assert!(luma(&glow, 32, 18) > luma(&plain, 32, 18) + 20);
        assert!(luma(&plain, 32, 18) < 6, "dark without bloom");
        assert!(luma(&glow, 32, 32) > 600, "the sphere stays bright");
        assert_eq!(r.validation_errors(), 0);
    }

    /// Default grading changes nothing; saturation 0 is grey; a positive
    /// temperature warms; lift/gamma/gain act on the display image.
    #[test]
    fn color_grading_adjusts_the_tone_mapped_image() {
        use crate::components::ColorGrading;
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let render = |r: &mut Renderer, grading: Option<ColorGrading>| {
            let mut world = lit_cube_world();
            without_post_effects(&mut world);
            if let Some(g) = grading {
                world.resource_mut::<Environment>().color_grading = g;
            }
            world.spawn((
                Transform::from_xyz(2.0, 1.5, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
                Camera::default(),
            ));
            propagate_transforms(&mut world);
            r.capture(&mut world).unwrap()
        };
        let reference = render(&mut r, None);
        let base = render(&mut r, Some(ColorGrading::default()));
        assert!(crate::golden::compare(&base, &reference, &crate::golden::EXACT).is_ok());
        let grey = render(
            &mut r,
            Some(ColorGrading {
                saturation: 0.0,
                ..Default::default()
            }),
        );
        for (x, y) in [(4, 4), (32, 32), (50, 50)] {
            let [cr, cg, cb, _] = grey.get_pixel(x, y).0;
            assert!(
                cr.abs_diff(cg) <= 2 && cg.abs_diff(cb) <= 2,
                "{cr} {cg} {cb}"
            );
        }
        let warm = render(
            &mut r,
            Some(ColorGrading {
                temperature: 0.6,
                ..Default::default()
            }),
        );
        let ratio = |img: &image::RgbaImage| {
            let [cr, _, cb, _] = img.get_pixel(32, 32).0;
            cr as f32 / cb.max(1) as f32
        };
        assert!(ratio(&warm) > ratio(&base) * 1.05, "warmer");
        let graded = render(
            &mut r,
            Some(ColorGrading {
                lift: Vec3::splat(0.2),
                gain: Vec3::new(1.0, 1.0, 0.5),
                ..Default::default()
            }),
        );
        let [_, _, b0, _] = base.get_pixel(32, 32).0;
        let [_, _, b1, _] = graded.get_pixel(32, 32).0;
        assert!(b1 < b0, "gain 0.5 on blue: {b0} -> {b1}");
        assert!(luma(&graded, 4, 4) != luma(&base, 4, 4));
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn texture_uploads_follow_asset_versions() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        use box_core::asset_ref::{AssetId, AssetRef};
        use box_core::image::{ColorSpace, ImageData};
        let mut world = top_down_world();
        let red = || ImageData::solid([255, 0, 0, 255], ColorSpace::Srgb);
        let image = world.resource_or_default::<Assets<ImageData>>().add(red());
        world.resource_or_default::<AssetLookup>().insert(
            AssetId::from_u128(1),
            None,
            Some("red.png"),
            image,
        );
        let material =
            world
                .resource_or_default::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::WHITE,
                    base_color_texture: AssetRef::path("red.png"),
                    ..Default::default()
                });
        let plane = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::plane(4.0, 4.0));
        world.spawn((Transform::default(), MeshRenderer::new(plane, material)));
        propagate_transforms(&mut world);

        let img = r.capture(&mut world).unwrap();
        let [cr, cg, _, _] = img.get_pixel(32, 32).0;
        assert!(
            cr > 3 * cg.max(1),
            "red texture drawn: {:?}",
            img.get_pixel(32, 32)
        );
        assert_eq!((r.textures.uploads, r.cached_textures()), (1, 1));
        let generation = r.textures.generation();
        // Unchanged: nothing uploaded, nothing rebuilt.
        for _ in 0..3 {
            r.render(&mut world).unwrap();
        }
        assert_eq!(r.textures.uploads, 1);
        // A version bump of the same size rewrites in place (same view).
        *world
            .resource_mut::<Assets<ImageData>>()
            .get_mut(image)
            .unwrap() = ImageData::solid([0, 0, 255, 255], ColorSpace::Srgb);
        let img = r.capture(&mut world).unwrap();
        let [cr, _, cb, _] = img.get_pixel(32, 32).0;
        assert!(cb > 3 * cr.max(1), "blue after the edit");
        assert_eq!(r.textures.uploads, 2);
        assert_eq!(r.textures.generation(), generation, "rewritten in place");
        // A new size re-creates the texture; materials pick up the new view.
        let mut big = ImageData::new(4, 4, ColorSpace::Srgb, [0, 255, 0, 255].repeat(16)).unwrap();
        big.generate_mips();
        *world
            .resource_mut::<Assets<ImageData>>()
            .get_mut(image)
            .unwrap() = big;
        let img = r.capture(&mut world).unwrap();
        let [cr, cg, _, _] = img.get_pixel(32, 32).0;
        assert!(cg > 3 * cr.max(1), "green after the resize");
        assert_eq!(r.textures.uploads, 3);
        assert_ne!(r.textures.generation(), generation);
        // Removal frees it and the material falls back to white.
        world.resource_mut::<Assets<ImageData>>().remove(image);
        let fallback = r.capture(&mut world).unwrap();
        assert_eq!(r.cached_textures(), 0);
        world
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(material)
            .unwrap()
            .base_color_texture = AssetRef::NONE;
        let untextured = r.capture(&mut world).unwrap();
        assert_eq!(
            fallback.get_pixel(32, 32),
            untextured.get_pixel(32, 32),
            "the white default texture passes the factor through"
        );
        // An invalid image is rejected once per version and never bound.
        let bad = world.resource_mut::<Assets<ImageData>>().add(ImageData {
            width: 2,
            height: 2,
            color_space: ColorSpace::Srgb,
            mips: vec![vec![0; 3]],
        });
        for _ in 0..3 {
            r.render(&mut world).unwrap();
        }
        assert_eq!(r.textures.uploads, 4);
        world.resource_mut::<Assets<ImageData>>().remove(bad);
        r.render(&mut world).unwrap();
        assert_eq!(r.validation_errors(), 0);
    }

    /// The tangent frame convention: a normal map tilting normals toward +U
    /// (tangent) brightens a plane lit from +U's side, and toward +V
    /// (bitangent = dP/dV) one lit from +V's side; no NaNs without tangents.
    #[test]
    fn normal_maps_tilt_toward_u_and_v() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        use box_core::asset_ref::{AssetId, AssetRef};
        use box_core::image::{ColorSpace, ImageData};
        // The plane's U runs along +X and V along +Z.
        let render = |r: &mut Renderer, tilt: [u8; 4], light: Vec3, tangents: bool| {
            let mut world = top_down_world();
            world.resource_or_default::<Environment>().ambient_intensity = 0.0;
            let image = world
                .resource_or_default::<Assets<ImageData>>()
                .add(ImageData::solid(tilt, ColorSpace::Linear));
            world.resource_or_default::<AssetLookup>().insert(
                AssetId::from_u128(2),
                None,
                Some("n.png"),
                image,
            );
            let material =
                world
                    .resource_or_default::<Assets<StandardMaterial>>()
                    .add(StandardMaterial {
                        normal_texture: AssetRef::path("n.png"),
                        roughness: 1.0,
                        ..Default::default()
                    });
            let mut mesh = MeshData::plane(4.0, 4.0);
            if tangents {
                mesh.generate_tangents();
            }
            let plane = world.resource_or_default::<Assets<MeshData>>().add(mesh);
            world.spawn((Transform::default(), MeshRenderer::new(plane, material)));
            world.spawn((
                Transform::default().looking_to(-light, Vec3::Y),
                DirectionalLight {
                    shadows: false,
                    ..Default::default()
                },
            ));
            propagate_transforms(&mut world);
            let img = r.capture(&mut world).unwrap();
            luma(&img, 32, 32)
        };
        let (plus_u, minus_u) = ([218, 128, 218, 255], [37, 128, 218, 255]);
        let (plus_v, minus_v) = ([128, 218, 218, 255], [128, 37, 218, 255]);
        let from_x = Vec3::new(1.0, 0.6, 0.0);
        let from_z = Vec3::new(0.0, 0.6, 1.0);
        let (a, b) = (
            render(&mut r, plus_u, from_x, true),
            render(&mut r, minus_u, from_x, true),
        );
        assert!(a > b + 30, "+U tilt faces a light from +X: {a} vs {b}");
        let (a, b) = (
            render(&mut r, plus_v, from_z, true),
            render(&mut r, minus_v, from_z, true),
        );
        assert!(a > b + 30, "+V tilt faces a light from +Z: {a} vs {b}");
        // Without tangents the geometric normal is used: both tilts equal.
        let (a, b) = (
            render(&mut r, plus_u, from_x, false),
            render(&mut r, minus_u, from_x, false),
        );
        assert_eq!(a, b);
        assert!(a > 0, "lit, not NaN black");
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn point_lights_fade_to_nothing_at_their_range() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        r.resize(128, 128);
        let build = |light: Option<PointLight>| {
            let mut world = top_down_world();
            let env = world.resource_or_default::<Environment>();
            env.ambient_intensity = 0.05;
            let material = world
                .resource_or_default::<Assets<StandardMaterial>>()
                .add(StandardMaterial::default());
            let plane = world
                .resource_or_default::<Assets<MeshData>>()
                .add(MeshData::plane(8.0, 8.0));
            world.spawn((Transform::default(), MeshRenderer::new(plane, material)));
            if let Some(l) = light {
                world.spawn((Transform::from_xyz(-1.0, 0.5, 0.0), l));
            }
            propagate_transforms(&mut world);
            world
        };
        let light = PointLight {
            color: Color::linear(1.0, 0.5, 0.2),
            intensity: 2.0,
            range: 1.5,
            ..Default::default()
        };
        let lit = r.capture(&mut build(Some(light))).unwrap();
        assert_eq!(r.stats().point_lights, 1);
        let dark = r.capture(&mut build(None)).unwrap();
        // Under the light (x = -1 m) it is brighter and warm; beyond the
        // range (x = +1 m) it is exactly the unlit image.
        let under = (128.0 * (0.5 - 1.0 / (2.0 * 4.0 * (25f32.to_radians()).tan()))) as u32;
        assert!(luma(&lit, under, 64) > luma(&dark, under, 64) + 60);
        let [cr, _, cb, _] = lit.get_pixel(under, 64).0;
        assert!(cr > cb, "warm light");
        assert_eq!(
            lit.get_pixel(128 - under, 64),
            dark.get_pixel(128 - under, 64)
        );
        // Outside the view frustum: culled from the buffer.
        let mut far = build(None);
        far.spawn((Transform::from_xyz(0.0, 50.0, 0.0), light));
        propagate_transforms(&mut far);
        r.render(&mut far).unwrap();
        assert_eq!(r.stats().point_lights, 0);
        assert_eq!(r.validation_errors(), 0);
    }

    #[test]
    fn shader_materials_bind_params_keep_good_versions_and_show_errors() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        use box_core::asset_ref::{AssetId, AssetRef};
        use box_core::shader_material::{ShaderParamValue, ShaderSource};
        let tinted = |color: &str| {
            format!(
                "@material struct P {{ @color tint: vec3f = vec3f({color}), }}\n\
                 fn surface(in: SurfaceInput, p: P) -> Surface {{\n    \
                 var s = default_surface(in);\n    s.base_color = vec3f(0.0);\n    \
                 s.emissive = p.tint;\n    return s;\n}}\n"
            )
        };
        let mut world = top_down_world();
        world.resource_or_default::<Environment>().ambient_intensity = 0.0;
        let source = world
            .resource_or_default::<Assets<ShaderSource>>()
            .add(ShaderSource::new("tint.wgsl", tinted("1.0, 0.0, 0.0")));
        world.resource_or_default::<AssetLookup>().insert(
            AssetId::from_u128(3),
            None,
            Some("tint.wgsl"),
            source,
        );
        let material = world
            .resource_or_default::<Assets<ShaderMaterial>>()
            .add(ShaderMaterial::new(AssetRef::path("tint.wgsl")));
        let plane = world
            .resource_or_default::<Assets<MeshData>>()
            .add(MeshData::plane(4.0, 4.0));
        world.spawn((Transform::default(), MeshRenderer::new(plane, material)));
        propagate_transforms(&mut world);
        let px =
            |r: &mut Renderer, world: &mut World| r.capture(world).unwrap().get_pixel(32, 32).0;

        // The header default (red).
        let [cr, cg, cb, _] = px(&mut r, &mut world);
        assert!(cr > 3 * cg && cr > 3 * cb, "red default: {cr} {cg} {cb}");
        let pipelines = r.pipeline_cache.len();
        // A parameter value (green): no recompilation, no new pipelines.
        world
            .resource_mut::<Assets<ShaderMaterial>>()
            .get_mut(material)
            .unwrap()
            .params = ShaderMaterial::default()
            .with_param(
                "tint",
                ShaderParamValue::Color(Color::linear(0.0, 1.0, 0.0)),
            )
            .params;
        let [cr, cg, _, _] = px(&mut r, &mut world);
        assert!(cg > 3 * cr, "green param");
        assert_eq!(r.pipeline_cache.len(), pipelines);

        // A broken edit keeps the previous version and reports the error.
        world
            .resource_mut::<Assets<ShaderSource>>()
            .get_mut(source)
            .unwrap()
            .text =
            "fn surface(in: SurfaceInput, p: MaterialParams) -> Surface { return 1.0; }".into();
        let [cr, cg, _, _] = px(&mut r, &mut world);
        assert!(cg > 3 * cr, "previous version still draws");
        let err = r.shader_error(source).expect("error reported");
        assert!(err.contains("tint.wgsl"), "{err}");

        // A fixed edit recompiles: blue default, the green value still bound.
        world
            .resource_mut::<Assets<ShaderSource>>()
            .get_mut(source)
            .unwrap()
            .text = tinted("0.0, 0.0, 1.0");
        let [cr, cg, _, _] = px(&mut r, &mut world);
        assert!(cg > 3 * cr, "the material's own value wins");
        assert!(r.shader_error(source).is_none());

        // A shader that never compiled draws the error material (magenta).
        let broken = world
            .resource_mut::<Assets<ShaderSource>>()
            .add(ShaderSource::new("broken.wgsl", "this is not wgsl"));
        world.resource_mut::<AssetLookup>().insert(
            AssetId::from_u128(4),
            None,
            Some("broken.wgsl"),
            broken,
        );
        world
            .resource_mut::<Assets<ShaderMaterial>>()
            .get_mut(material)
            .unwrap()
            .shader = AssetRef::path("broken.wgsl");
        let img = r.capture(&mut world).unwrap();
        let magenta = img
            .pixels()
            .filter(|p| p.0[0] > p.0[1] + 60 && p.0[2] > p.0[1] + 60)
            .count();
        assert!(
            magenta > 500,
            "error material: {magenta} px, centre {:?}",
            img.get_pixel(32, 32)
        );
        assert!(r.shader_error(broken).is_some());

        // The asset server's placeholder for a file that failed to import
        // (an empty source) draws the error material too.
        let failed = world
            .resource_mut::<Assets<ShaderSource>>()
            .add(ShaderSource::default());
        world.resource_mut::<AssetLookup>().insert(
            AssetId::from_u128(5),
            None,
            Some("failed.wgsl"),
            failed,
        );
        world
            .resource_mut::<Assets<ShaderMaterial>>()
            .get_mut(material)
            .unwrap()
            .shader = AssetRef::path("failed.wgsl");
        let img = r.capture(&mut world).unwrap();
        let magenta = img
            .pixels()
            .filter(|p| p.0[0] > p.0[1] + 60 && p.0[2] > p.0[1] + 60)
            .count();
        assert!(magenta > 500, "failed import: {magenta} px");
        assert!(
            r.shader_error(failed)
                .is_some_and(|e| e.contains("failed to import"))
        );

        // An unresolved shader (not loaded yet) draws the default material.
        world
            .resource_mut::<Assets<ShaderMaterial>>()
            .get_mut(material)
            .unwrap()
            .shader = AssetRef::path("missing.wgsl");
        r.render(&mut world).unwrap();
        assert_eq!(r.stats().draws, 3, "plane, sky, tone map");
        assert_eq!(r.validation_errors(), 0);
    }

    /// Selecting a ground plane together with objects standing on it keeps
    /// every object's outline (each selected object is its own silhouette,
    /// not part of one union), and the primary keeps its accent colour.
    #[test]
    fn outlines_of_objects_on_a_selected_ground_survive() {
        let Some(mut r) = headless() else {
            eprintln!("skipping: no GPU");
            return;
        };
        let (primary, secondary) = ([255u8, 0, 255, 255], [0u8, 255, 0, 255]);
        let mut world = World::new();
        world.resource_or_default::<Environment>().fog_density = 0.0;
        let material = world
            .resource_or_default::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let meshes = world.resource_or_default::<Assets<MeshData>>();
        let plane = meshes.add(MeshData::plane(8.0, 8.0));
        let cube = meshes.add(MeshData::cuboid(Vec3::ONE));
        let ground = world.spawn((Transform::default(), MeshRenderer::new(plane, material)));
        let left = world.spawn((
            Transform::from_xyz(-1.2, 0.5, 0.0),
            MeshRenderer::new(cube, material),
        ));
        let right = world.spawn((
            Transform::from_xyz(1.2, 0.5, 0.0),
            MeshRenderer::new(cube, material),
        ));
        world.spawn((
            Transform::default().looking_to(Vec3::new(0.3, -1.0, -0.4), Vec3::Y),
            DirectionalLight::default(),
        ));
        propagate_transforms(&mut world);
        world.insert_resource(SelectionOutline {
            primary: vec![left],
            secondary: vec![ground, right],
            color: Color::srgb(1.0, 0.0, 1.0),
            secondary_color: Color::srgb(0.0, 1.0, 0.0),
            width_px: 2.0,
            occluded_alpha: 0.3,
        });
        let eye = Transform::from_xyz(0.0, 2.5, 5.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y);
        let target = r.create_viewport_target(256, 160);
        r.render_to(
            &mut world,
            &target,
            Some(&ViewCamera::new(&eye, Camera::default())),
        );
        let img = r.read_viewport_target(&target).unwrap();
        let out =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
        std::fs::create_dir_all(&out).unwrap();
        img.save(out.join("outline_selected_ground.png")).unwrap();
        // Along the row through the cubes' middles: the primary (left) and
        // the secondary (right) outline both appear.
        let count = |rgba: [u8; 4]| img.pixels().filter(|p| p.0 == rgba).count();
        assert!(
            count(primary) > 50,
            "primary outline: {} px",
            count(primary)
        );
        // The right cube's own (secondary) outline, inside the ground's
        // silhouette: well below the ground's far edge, around x = 140..195.
        let right_cube_band = (140..195)
            .flat_map(|x| (75..110).map(move |y| (x, y)))
            .filter(|&(x, y)| img.get_pixel(x, y).0 == secondary)
            .count();
        assert!(
            right_cube_band > 20,
            "right cube outline: {right_cube_band} px"
        );
        assert_eq!(r.validation_errors(), 0);
    }
}
