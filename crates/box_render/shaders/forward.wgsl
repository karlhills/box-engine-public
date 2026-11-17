// Shared by forward material shaders: alpha modes, normal mapping and the
// final output for each alpha mode. Requires common.wgsl and fog.wgsl.

// Values of the `ALPHA_MODE` pipeline constant (`AlphaKey` in src/pipelines.rs).
const ALPHA_OPAQUE: u32 = 0u;
const ALPHA_MASK: u32 = 1u;
const ALPHA_BLEND: u32 = 2u;
const ALPHA_ADDITIVE: u32 = 3u;
const ALPHA_MULTIPLY: u32 = 4u;

// Applies tangent-space normal `tn` (components in -1..1) with the
// world-space tangent frame of `n` and `tangent` (w = bitangent sign, so the
// bitangent is cross(n, t) * w). Without a usable tangent it returns `n`.
fn perturb_normal(n: vec3<f32>, tangent: vec4<f32>, tn: vec3<f32>) -> vec3<f32> {
    let t = tangent.xyz - n * dot(n, tangent.xyz);
    let t_len_sq = dot(t, t);
    if t_len_sq < 1e-12 {
        return n;
    }
    let tu = t * inverseSqrt(t_len_sq);
    let b = cross(n, tu) * select(1.0, -1.0, tangent.w < 0.0);
    let m = tu * tn.x + b * tn.y + n * tn.z;
    let m_len_sq = dot(m, m);
    if m_len_sq < 1e-12 {
        return n;
    }
    return m * inverseSqrt(m_len_sq);
}

// Soft transparency: 0 where a transparent fragment at `frag` (its
// @builtin(position)) touches the opaque surface behind it, rising to 1
// `distance` metres in front of it. 1 when `distance` is 0 or the scene
// depth is not bound (it is whenever a visible material uses depth fade).
fn depth_fade(frag: vec4<f32>, distance: f32) -> f32 {
    if distance <= 0.0 || globals.scene_depth_on.x < 0.5 {
        return 1.0;
    }
    let scene = textureLoad(opaque_depth, vec2<i32>(frag.xy), 0);
    if scene <= 0.0 {
        // Nothing opaque behind (the sky).
        return 1.0;
    }
    // Infinite reverse-Z: NDC depth = near / view depth.
    let near = globals.camera_pos.w;
    let gap = near / scene - near / max(frag.z, 1e-12);
    return saturate(gap / distance);
}

// The fragment's output for `alpha_mode`, from the lit colour `color`, the
// unlit albedo `albedo` (multiply mode) and `alpha`. Blend output is
// premultiplied (blended with One, OneMinusSrcAlpha); additive output is
// added; multiply output multiplies what is behind. Fog and atmosphere:
// opaque and blend surfaces fade toward the fog colour, additive ones fade
// out, multiply ones fade toward no tint. `fade` (`depth_fade`) scales the
// coverage of transparent modes.
fn forward_output(alpha_mode: u32, color: vec3<f32>, albedo: vec3<f32>, alpha: f32, world_pos: vec3<f32>, fade: f32) -> vec4<f32> {
    let eye = globals.camera_pos.xyz;
    if globals.ao.y > 0.5 {
        // The ambient occlusion view (shade_standard): no fog.
        return vec4<f32>(color, 1.0);
    }
    switch alpha_mode {
        case 2u: {
            let a = saturate(alpha) * fade;
            return vec4<f32>(apply_fog(color, eye, world_pos) * a, a);
        }
        case 3u: {
            let tr = fog_transmittance(eye, world_pos) * atmosphere_transmittance(world_pos);
            return vec4<f32>(color * (max(alpha, 0.0) * tr * fade), 0.0);
        }
        case 4u: {
            let tr = fog_transmittance(eye, world_pos) * atmosphere_transmittance(world_pos);
            let tint = mix(vec3<f32>(1.0), albedo, saturate(alpha) * tr * fade);
            return vec4<f32>(tint, 1.0);
        }
        default: {
            return vec4<f32>(apply_fog(color, eye, world_pos), 1.0);
        }
    }
}
