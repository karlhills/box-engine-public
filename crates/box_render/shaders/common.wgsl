// Shared constants, the per-frame uniform block and the frame bind group.
// Must match `GlobalsUniform` in src/uniforms.rs field for field.

const PI: f32 = 3.14159265359;
const CASCADE_COUNT: u32 = 4u;

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // xyz position, w = near plane distance
    camera_pos: vec4<f32>,
    // xyz forward (unit)
    camera_forward: vec4<f32>,
    // xyz unit vector toward the light, w = 1 when a directional light exists
    sun_dir: vec4<f32>,
    // rgb = colour * intensity, w = sun disc angular radius (radians)
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    sky_ground: vec4<f32>,
    // x = ambient intensity
    ambient: vec4<f32>,
    // x = density, y = height falloff, z = base height
    fog: vec4<f32>,
    // x = exposure, y = tonemapper (0 AgX, 1 ACES fitted, 2 none, 3 PBR Neutral)
    post: vec4<f32>,
    cascade_view_proj: array<mat4x4<f32>, 4>,
    // far view depth of each cascade
    cascade_splits: vec4<f32>,
    // world-space size of one shadow texel per cascade
    cascade_texel: vec4<f32>,
    // depth bias per cascade, in light clip-space depth units
    cascade_bias: vec4<f32>,
    // x = shadows enabled, y = normal offset (texels), z = cascade blend band (fraction), w = show cascades
    shadow: vec4<f32>,
    // width, height, 1/width, 1/height
    viewport: vec4<f32>,
    // x = seconds since start, y = seconds since the previous frame
    time: vec4<f32>,
    // x = ambient occlusion on, y = show the occlusion term, z = fraction
    // applied to direct light
    ao: vec4<f32>,
    // xyz = camera right * tan(half horizontal fov), w = AO radius (m)
    ao_right: vec4<f32>,
    // xyz = camera up * tan(half vertical fov), w = AO intensity
    ao_up: vec4<f32>,
    // x = AO bias (m), y = AO power, z = half-resolution pixels per metre at
    // view depth 1
    ao_params: vec4<f32>,
    // x = bloom blend (0 off), y = threshold on scene luminance before
    // exposure (0 off), z = knee half-width (same units)
    bloom: vec4<f32>,
    // x = contrast, y = saturation, z = scene grading on, w = display grading on
    grade: vec4<f32>,
    // white balance matrix (linear RGB)
    white_balance: mat3x3<f32>,
    lift: vec4<f32>,
    inv_gamma: vec4<f32>,
    gain: vec4<f32>,
    // Aerial haze: x = density (1/m), y = max opacity (0 off), z = start
    // distance (m), w = sun scatter
    haze: vec4<f32>,
    // rgb = haze colour, w = sky height (sine of elevation)
    haze_color: vec4<f32>,
    // Height mist: x = top, y = bottom (m), z = max opacity (0 off),
    // w = near fade distance (m)
    mist: vec4<f32>,
    mist_color: vec4<f32>,
    // Distance quieting: x = start, y = end (m), z = desaturate, w = contrast
    quiet: vec4<f32>,
    // rgb = tone, w = 1 when quieting is on
    quiet_tone: vec4<f32>,
    // rgb = upper sky colour, w = 1 for the layered gradient
    sky_upper: vec4<f32>,
    // rgb = colour below the horizon (layered gradient)
    sky_below: vec4<f32>,
    // x = sun glow, y = sun disc, z = horizon glow, w = stars
    sky_sun: vec4<f32>,
    // rgb = shadows tint, w = luminance where it ends
    shadows_tint: vec4<f32>,
    // rgb = midtones tint, w = luminance where the highlights start
    midtones_tint: vec4<f32>,
    // rgb = highlights tint, w = 1 when the three tints apply
    highlights_tint: vec4<f32>,
    // x = intensity * 3 (0 off), y = smoothness * 5, z = horizontal scale
    vignette: vec4<f32>,
    // xy = wind direction (world x, z), z = strength, w = gust length (m)
    wind: vec4<f32>,
    // x = gust speed (m/s)
    wind_gust: vec4<f32>,
    // x = 1 when `opaque_depth` holds the prepass depth, y = 1 when
    // `opaque_color` holds the opaque scene's colour (transparent pass)
    scene_depth_on: vec4<f32>,
};

// One point or spot light. Must match `GpuLocalLight` in src/uniforms.rs.
struct LocalLight {
    position: vec3<f32>,
    range: f32,
    // linear colour * intensity
    color: vec3<f32>,
    // 1 for the frame's shadowed light (its views are `lights.shadow_view_proj`)
    shadow: f32,
    // unit spot axis; zero for point lights
    direction: vec3<f32>,
    // cone factor saturate(cos * spot_scale + spot_offset)^2; (0, 1) for point lights
    spot_scale: f32,
    spot_offset: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

// A box that replaces the sky ambient inside it. Must match
// `GpuAmbientVolume` in src/uniforms.rs.
struct AmbientVolume {
    // world to box space (rotation and translation)
    world_to_local: mat4x4<f32>,
    // xyz = half extents (m), w = blend distance (m)
    half_extents: vec4<f32>,
    // rgb = ambient colour * intensity, w = fraction of the sky ambient kept
    ambient: vec4<f32>,
};

// The counted light buffer: header, the shadowed local light's views, the
// ambient volumes, then `count.x` lights, nearest/brightest first.
struct Lights {
    // x = lights, y = ambient volumes
    count: vec4<u32>,
    // x = shadow kind (0 none, 1 spot, 2 point), y = normal bias (texels),
    // z = depth bias (texels), w = world texel size per metre from the light
    shadow: vec4<f32>,
    // spot: [0]; point: +X, -X, +Y, -Y, +Z, -Z
    shadow_view_proj: array<mat4x4<f32>, 6>,
    volumes: array<AmbientVolume, 8>,
    items: array<LocalLight>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d_array;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(0) @binding(3) var<storage, read> lights: Lights;
// Half-resolution ambient occlusion (r) and its linear view depth (r), from
// ao.wgsl; 1x1 placeholders while ambient occlusion is off.
@group(0) @binding(5) var ao_texture: texture_2d<f32>;
@group(0) @binding(6) var ao_depth_texture: texture_2d<f32>;
// Full-resolution opaque depth from the prepass (reverse-Z), for soft
// transparency; a 1x1 placeholder unless `globals.scene_depth_on.x` is 1.
@group(0) @binding(7) var opaque_depth: texture_depth_2d;
// The opaque scene's resolved HDR colour (opaque surfaces, sky, scene lines;
// fogged, before exposure), bound for the transparent draws of frames where
// a visible material shader reads it; a 1x1 placeholder elsewhere. Sampled
// with `scene_sampler` (linear, clamped).
@group(0) @binding(9) var opaque_color: texture_2d<f32>;
@group(0) @binding(10) var scene_sampler: sampler;
// The shadowed local light's depth (one layer per view; standard depth),
// compared with `shadow_sampler`.
@group(0) @binding(8) var local_shadow_map: texture_depth_2d_array;

fn has_sun() -> bool {
    return globals.sun_dir.w > 0.5;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The part of `c` above the bloom threshold (globals.bloom.y) in luminance,
// with a quadratic soft knee of half-width globals.bloom.z: 0 below
// threshold - knee, `c` scaled by (luma - threshold) / luma above
// threshold + knee. Used by the first bloom downsample and the tone-mapping
// pass when the threshold is on.
fn bloom_bright(c: vec3<f32>) -> vec3<f32> {
    let threshold = globals.bloom.y;
    let knee = globals.bloom.z;
    let l = luminance(c);
    let s = clamp(l - threshold + knee, 0.0, 2.0 * knee);
    let soft = s * s / (4.0 * knee + 1e-5);
    return c * (max(soft, l - threshold) / max(l, 1e-5));
}

// Fullscreen triangle covering the viewport; returns NDC xy for vertex 0..2.
fn fullscreen_ndc(vertex_index: u32) -> vec2<f32> {
    let uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    return uv * 2.0 - 1.0;
}
