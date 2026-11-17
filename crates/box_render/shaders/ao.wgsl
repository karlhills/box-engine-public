// Ambient occlusion, steps 2 and 3, at half resolution: scalable ambient
// obscurance (McGuire et al. 2012, falloff variant B) over a spiral of
// samples rotated per pixel (`fs_ssao`), then a 4x4 depth-aware blur that
// removes the 4x4 rotation pattern (`fs_blur`). Positions are rebuilt in
// world space from the linear view depth; normals come from the prepass.

#include "common.wgsl"
#include "ao_common.wgsl"

@group(1) @binding(0) var ao_view_depth: texture_2d<f32>;
@group(1) @binding(1) var ao_normal: texture_2d<f32>;
@group(1) @binding(2) var ao_raw: texture_2d<f32>;

const SAMPLES: i32 = 12;
// Turns of the sample spiral.
const SPIRAL_TURNS: f32 = 7.0;
// Largest sampling radius in half-resolution pixels (texture cache).
const MAX_RADIUS_PX: f32 = 80.0;

// World position of half-resolution pixel `p` (centre) at view depth `d`.
fn ao_position(p: vec2<f32>, d: f32, size: vec2<f32>) -> vec3<f32> {
    let ndc = (p + vec2<f32>(0.5)) / size * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0);
    let ray = globals.camera_forward.xyz + globals.ao_right.xyz * ndc.x + globals.ao_up.xyz * ndc.y;
    return globals.camera_pos.xyz + ray * d;
}

@fragment
fn fs_ssao(in: FullscreenOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let isize = vec2<i32>(textureDimensions(ao_view_depth));
    let size = vec2<f32>(isize);
    let d = textureLoad(ao_view_depth, p, 0).r;
    if d >= AO_SKY_DEPTH {
        return vec4<f32>(1.0);
    }
    let n = normalize(textureLoad(ao_normal, p, 0).xyz * 2.0 - vec3<f32>(1.0));
    let position = ao_position(vec2<f32>(p), d, size);
    let radius = globals.ao_right.w;
    let radius_px = min(radius * globals.ao_params.z / d, MAX_RADIUS_PX);
    if radius_px < 1.0 {
        return vec4<f32>(1.0);
    }
    let r2 = radius * radius;
    let bias = globals.ao_params.x;
    // One of 16 spiral rotations per pixel, in a 4x4 ordered-dither tile;
    // the 4x4 blur averages all 16.
    let bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let angle0 = bayer[(p.y & 3) * 4 + (p.x & 3)] * (6.28318530718 / 16.0);
    // And one of 16 offsets along the spiral (the transposed tile), so the
    // blur also averages the sample distances.
    let jitter = bayer[(p.x & 3) * 4 + (p.y & 3)] / 16.0;
    var sum = 0.0;
    for (var i = 0; i < SAMPLES; i++) {
        let alpha = (f32(i) + jitter) / f32(SAMPLES);
        let angle = alpha * SPIRAL_TURNS * 6.28318530718 + angle0;
        let offset = vec2<f32>(cos(angle), sin(angle)) * (alpha * radius_px);
        let q = clamp(vec2<i32>(vec2<f32>(p) + offset + vec2<f32>(0.5)), vec2<i32>(0), isize - vec2<i32>(1));
        let qd = textureLoad(ao_view_depth, q, 0).r;
        if qd >= AO_SKY_DEPTH {
            continue;
        }
        let v = ao_position(vec2<f32>(q), qd, size) - position;
        let vv = dot(v, v);
        let vn = dot(v, n);
        let f = max(r2 - vv, 0.0);
        sum += f * f * f * max((vn - bias) / (vv + 0.01 * r2), 0.0);
    }
    let intensity = globals.ao_up.w;
    // Times the radius, so the result does not depend on the scale.
    let a = max(0.0, 1.0 - sum * intensity * 5.0 * radius / (r2 * r2 * r2 * f32(SAMPLES)));
    return vec4<f32>(pow(a, globals.ao_params.y));
}

// Relative depth difference over which the blur stops mixing texels.
const BLUR_DEPTH_TOLERANCE: f32 = 0.08;

@fragment
fn fs_blur(in: FullscreenOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let isize = vec2<i32>(textureDimensions(ao_view_depth));
    let d = textureLoad(ao_view_depth, p, 0).r;
    if d >= AO_SKY_DEPTH {
        return vec4<f32>(1.0);
    }
    var sum = 0.0;
    var weight = 0.0;
    for (var y = -2; y <= 1; y++) {
        for (var x = -2; x <= 1; x++) {
            let q = clamp(p + vec2<i32>(x, y), vec2<i32>(0), isize - vec2<i32>(1));
            let qd = textureLoad(ao_view_depth, q, 0).r;
            let w = saturate(1.0 - abs(qd - d) / (d * BLUR_DEPTH_TOLERANCE));
            sum += textureLoad(ao_raw, q, 0).r * w;
            weight += w;
        }
    }
    return vec4<f32>(sum / max(weight, 1e-4));
}
