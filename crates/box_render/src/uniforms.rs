//! GPU-side data layouts. Field order must match the WGSL structs in
//! `shaders/common.wgsl`, `shaders/object.wgsl` and
//! `shaders/material_standard.wgsl`.

use bytemuck::{Pod, Zeroable};

pub(crate) type Mat4Cols = [[f32; 4]; 4];

/// Per-frame uniform block (`Globals` in common.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GlobalsUniform {
    pub view_proj: Mat4Cols,
    pub inv_view_proj: Mat4Cols,
    pub camera_pos: [f32; 4],
    pub camera_forward: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_color: [f32; 4],
    pub sky_zenith: [f32; 4],
    pub sky_horizon: [f32; 4],
    pub sky_ground: [f32; 4],
    pub ambient: [f32; 4],
    pub fog: [f32; 4],
    pub post: [f32; 4],
    pub cascade_view_proj: [Mat4Cols; 4],
    pub cascade_splits: [f32; 4],
    pub cascade_texel: [f32; 4],
    pub cascade_bias: [f32; 4],
    pub shadow: [f32; 4],
    pub viewport: [f32; 4],
    /// x = seconds since start, y = seconds since the previous frame.
    pub time: [f32; 4],
    /// x = ambient occlusion on, y = show the occlusion term,
    /// z = fraction applied to direct light.
    pub ao: [f32; 4],
    /// xyz = camera right * tan(half horizontal fov), w = AO radius (m).
    pub ao_right: [f32; 4],
    /// xyz = camera up * tan(half vertical fov), w = AO intensity.
    pub ao_up: [f32; 4],
    /// x = AO bias (m), y = AO power, z = half-resolution pixels per metre
    /// at view depth 1.
    pub ao_params: [f32; 4],
    /// x = bloom blend (0 off), y = threshold on scene luminance before
    /// exposure (0 off), z = knee half-width (same units).
    pub bloom: [f32; 4],
    /// x = contrast, y = saturation, z = 1 when white balance, contrast or
    /// saturation apply, w = 1 when lift, gamma or gain apply.
    pub grade: [f32; 4],
    /// White balance as a linear-RGB matrix (columns, w unused).
    pub white_balance: [[f32; 4]; 3],
    /// xyz lift, gamma (as 1 / gamma), gain.
    pub lift: [f32; 4],
    pub inv_gamma: [f32; 4],
    pub gain: [f32; 4],
    /// Aerial haze: x = density (1/m), y = max opacity (0 off), z = start
    /// distance (m), w = sun scatter.
    pub haze: [f32; 4],
    /// rgb = haze colour, w = sky height (sine of elevation).
    pub haze_color: [f32; 4],
    /// Height mist: x = top, y = bottom (m), z = max opacity (0 off),
    /// w = near fade distance (m).
    pub mist: [f32; 4],
    pub mist_color: [f32; 4],
    /// Distance quieting: x = start, y = end (m), z = desaturate,
    /// w = contrast.
    pub quiet: [f32; 4],
    /// rgb = tone, w = 1 when quieting is on.
    pub quiet_tone: [f32; 4],
    /// rgb = upper sky colour, w = 1 for the layered gradient.
    pub sky_upper: [f32; 4],
    /// rgb = colour below the horizon (layered gradient).
    pub sky_below: [f32; 4],
    /// x = sun glow, y = sun disc, z = horizon glow, w = stars.
    pub sky_sun: [f32; 4],
    /// rgb = shadows tint, w = luminance where it ends.
    pub shadows_tint: [f32; 4],
    /// rgb = midtones tint, w = luminance where the highlights start.
    pub midtones_tint: [f32; 4],
    /// rgb = highlights tint, w = 1 when the three tints apply.
    pub highlights_tint: [f32; 4],
    /// x = intensity * 3 (0 off), y = smoothness * 5, z = horizontal scale
    /// (the aspect ratio when rounded, else 1).
    pub vignette: [f32; 4],
    /// xy = wind direction (x, z), z = strength, w = gust length (m).
    pub wind: [f32; 4],
    /// x = gust speed (m/s).
    pub wind_gust: [f32; 4],
    /// x = 1 when the scene depth (prepass depth, binding 7) is bound,
    /// y = 1 when the frame copies the opaque scene's colour (binding 9).
    pub scene_depth: [f32; 4],
}

/// Per-object uniform (`Object` in object.wgsl), one 256-byte slot each.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct ObjectUniform {
    pub model: Mat4Cols,
    pub normal_matrix: Mat4Cols,
    /// z = selection outline id, w = 1, or -1 for a mirroring transform
    /// (flips the bitangent of normal maps).
    pub params: [f32; 4],
}

/// `StandardMaterial` in material_standard.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct StandardMaterialUniform {
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    /// x = roughness, y = metallic, z = alpha cutoff (mask mode), w = depth
    /// fade distance (transparent modes; 0 off).
    pub params: [f32; 4],
    /// x = flags ([`MATERIAL_FLAG_NORMAL_MAP`], [`MATERIAL_FLAG_WIND_VERTEX_ALPHA`]).
    pub flags: [u32; 4],
    /// Wind sway: x = strength, y = frequency (Hz), z/w = bottom/top of the
    /// height weight ramp (object space).
    pub wind: [f32; 4],
    /// xy = texture repeats per UV unit.
    pub uv: [f32; 4],
}

/// The material has a normal map (and the mesh may have tangents).
pub(crate) const MATERIAL_FLAG_NORMAL_MAP: u32 = 1;
/// The wind weight is the vertex alpha (which then does not affect alpha).
pub(crate) const MATERIAL_FLAG_WIND_VERTEX_ALPHA: u32 = 2;

/// Dynamic-offset stride for per-object and per-cascade uniforms.
pub(crate) const UNIFORM_SLOT: u64 = 256;

/// Interleaved mesh vertex (`VertexInput` in object.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    /// xyz tangent, w bitangent sign; all zero when the mesh has none.
    pub tangent: [f32; 4],
    /// UV set 1 (zero when the mesh has none): data for material shaders,
    /// such as box_arch's weathering coordinates.
    pub uv1: [f32; 2],
    /// `MeshData::custom` sets 0 and 1 (zero when the mesh has none): free
    /// data for material shaders.
    pub custom: [[f32; 4]; 2],
}

pub(crate) const MESH_ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x3,
    2 => Float32x2,
    3 => Float32x4,
    4 => Float32x4,
    5 => Float32x2,
    6 => Float32x4,
    7 => Float32x4,
];

/// One point or spot light in the light storage buffer (`LocalLight` in
/// common.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuLocalLight {
    pub position: [f32; 3],
    pub range: f32,
    /// Linear colour times intensity.
    pub color: [f32; 3],
    /// 1 for the frame's shadowed light, else 0.
    pub shadow: f32,
    /// Unit spot axis (where it points); zero for point lights.
    pub direction: [f32; 3],
    /// Cone factor `saturate(cos * spot_scale + spot_offset)^2`; (0, 1)
    /// for point lights.
    pub spot_scale: f32,
    pub spot_offset: f32,
    pub _pad: [f32; 3],
}

/// One ambient override box (`AmbientVolume` in common.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuAmbientVolume {
    /// World to box space (rotation and translation; no scale).
    pub world_to_local: Mat4Cols,
    /// xyz = half extents (m, scale applied), w = blend distance (m).
    pub half_extents: [f32; 4],
    /// rgb = colour * intensity, w = sky contribution.
    pub ambient: [f32; 4],
}

/// Most ambient volumes counted per frame.
pub(crate) const MAX_AMBIENT_VOLUMES: usize = 8;
/// Views of the shadowed local light (a cube for a point light).
pub(crate) const LOCAL_SHADOW_VIEWS: usize = 6;

/// Bytes before the first light: `count: vec4<u32>` (x = lights, y =
/// ambient volumes), `shadow: vec4<f32>`, the shadow views' matrices and
/// the ambient volumes.
pub(crate) const LIGHTS_HEADER: u64 =
    (32 + LOCAL_SHADOW_VIEWS * 64 + MAX_AMBIENT_VOLUMES * std::mem::size_of::<GpuAmbientVolume>())
        as u64;

/// Debug line vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct LineVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

pub(crate) const LINE_ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x4,
];

const _: () = assert!(std::mem::size_of::<ObjectUniform>() as u64 <= UNIFORM_SLOT);
const _: () = assert!(std::mem::size_of::<GlobalsUniform>() == 640 + 28 * 16);
const _: () = assert!(std::mem::size_of::<GpuVertex>() == 104);
const _: () = assert!(std::mem::size_of::<GpuLocalLight>() == 64);
const _: () = assert!(std::mem::size_of::<GpuAmbientVolume>() == 96);
const _: () = assert!(LIGHTS_HEADER.is_multiple_of(16));
const _: () = assert!(std::mem::size_of::<StandardMaterialUniform>() == 96);
