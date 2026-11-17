// Tone mapping: resolved HDR scene -> sRGB output. In order: bloom (mixed
// in), vignette, exposure, scene grading (white balance, contrast,
// shadows/midtones/highlights, saturation), the tone mapper (AgX, ACES
// fitted, PBR Neutral or none), display grading (lift, gamma, gain on the
// sRGB-encoded value) and a tiny dither against banding. The output view is sRGB, so the
// shader returns linear values.

#include "common.wgsl"

@group(1) @binding(0) var hdr_texture: texture_2d<f32>;
// Mip 0 of the bloom chain (bloom.wgsl); a 1x1 placeholder without bloom.
@group(1) @binding(1) var bloom_texture: texture_2d<f32>;
@group(1) @binding(2) var bloom_sampler: sampler;

struct TonemapOutput {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> TonemapOutput {
    var out: TonemapOutput;
    out.clip = vec4<f32>(fullscreen_ndc(vertex_index), 0.0, 1.0);
    return out;
}

// Analytic AgX approximation (polynomial fit of the default AgX contrast curve,
// after Benjamin Wrensch's "minimal AgX").
fn agx_contrast_approx(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(color: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        0.842479062253094, 0.0423282422610123, 0.0423756549057051,
        0.0784335999999992, 0.878468636469772, 0.0784336,
        0.0792237451477643, 0.0791661274605434, 0.879142973793104,
    );
    let outset = mat3x3<f32>(
        1.19687900512017, -0.0528968517574562, -0.0529716355144438,
        -0.0980208811401368, 1.15190312990417, -0.0980434501171241,
        -0.0990297440797205, -0.0989611768448433, 1.15107367264116,
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * max(color, vec3<f32>(0.0));
    v = clamp(log2(max(v, vec3<f32>(1e-10))), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    v = agx_contrast_approx(v);
    v = outset * v;
    // Back to linear for the sRGB render target.
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// Stephen Hill's ACES fit (sRGB -> ACES RRT+ODT -> sRGB).
fn aces_fitted(color: vec3<f32>) -> vec3<f32> {
    let input_mat = mat3x3<f32>(
        0.59719, 0.07600, 0.02840,
        0.35458, 0.90834, 0.13383,
        0.04823, 0.01566, 0.83777,
    );
    let output_mat = mat3x3<f32>(
        1.60475, -0.10208, -0.00327,
        -0.53108, 1.10813, -0.07276,
        -0.07367, -0.00605, 1.07602,
    );
    let v = input_mat * color;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return saturate(output_mat * (a / b));
}

// Khronos PBR Neutral (KhronosGroup/ToneMapping, PBR_Neutral): a small toe
// offset, identity up to START_COMPRESSION, then a hyperbolic roll-off of the
// peak channel toward 1 with slight desaturation. Output is display-linear.
const NEUTRAL_START_COMPRESSION: f32 = 0.76;
const NEUTRAL_DESATURATION: f32 = 0.15;

fn pbr_neutral(color: vec3<f32>) -> vec3<f32> {
    let x = min(color.r, min(color.g, color.b));
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    var c = color - vec3<f32>(offset);
    let peak = max(c.r, max(c.g, c.b));
    if peak < NEUTRAL_START_COMPRESSION {
        return c;
    }
    let d = 1.0 - NEUTRAL_START_COMPRESSION;
    let new_peak = 1.0 - d * d / (peak + d - NEUTRAL_START_COMPRESSION);
    c *= new_peak / peak;
    let g = 1.0 - 1.0 / (NEUTRAL_DESATURATION * (peak - new_peak) + 1.0);
    return mix(c, vec3<f32>(new_peak), g);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Interleaved gradient noise in [-0.5, 0.5] LSB of an 8-bit target.
fn dither(pos: vec2<f32>) -> f32 {
    let ign = fract(52.9829189 * fract(dot(pos, vec2<f32>(0.06711056, 0.00583715))));
    return (ign - 0.5) / 255.0;
}

// Middle grey, the pivot of the contrast.
const MID_GREY: f32 = 0.18;

// White balance, contrast (in log space around middle grey), shadows /
// midtones / highlights tints (by luminance, as Unity's) and saturation on
// exposed scene-linear colour.
fn grade_scene(c: vec3<f32>) -> vec3<f32> {
    var x = max(globals.white_balance * c, vec3<f32>(0.0));
    x = MID_GREY * pow(x / MID_GREY, vec3<f32>(globals.grade.x));
    if globals.highlights_tint.w > 0.5 {
        let luma = luminance(x);
        let shadows = 1.0 - smoothstep(0.0, globals.shadows_tint.w, luma);
        let highlights = smoothstep(globals.midtones_tint.w, 1.0, luma);
        let midtones = 1.0 - shadows - highlights;
        x = x * (globals.shadows_tint.rgb * shadows + globals.midtones_tint.rgb * midtones + globals.highlights_tint.rgb * highlights);
    }
    let l = luminance(x);
    return max(vec3<f32>(l) + (x - vec3<f32>(l)) * globals.grade.y, vec3<f32>(0.0));
}

// Vignette (URP's formula): darkens toward the corners of the image.
fn vignette(pos: vec2<f32>) -> f32 {
    let uv = pos / globals.viewport.xy;
    var d = abs(uv - vec2<f32>(0.5)) * globals.vignette.x;
    d.x *= globals.vignette.z;
    return pow(saturate(1.0 - dot(d, d)), globals.vignette.y);
}

// Lift, gamma and gain on a display (sRGB-encoded) value in 0..1.
fn grade_display(v: vec3<f32>) -> vec3<f32> {
    let x = globals.gain.rgb * (v + globals.lift.rgb * (vec3<f32>(1.0) - v));
    return pow(saturate(x), globals.inv_gamma.rgb);
}

@fragment
fn fs_main(in: TonemapOutput) -> @location(0) vec4<f32> {
    var scene = textureLoad(hdr_texture, vec2<i32>(in.clip.xy), 0).rgb;
    if globals.ao.y > 0.5 {
        // The ambient occlusion view (lighting.wgsl): the output byte is
        // the occlusion term.
        return vec4<f32>(srgb_to_linear(saturate(scene)), 1.0);
    }
    if globals.bloom.x > 0.0 {
        let uv = in.clip.xy / globals.viewport.xy;
        let bloom = textureSampleLevel(bloom_texture, bloom_sampler, uv, 0.0).rgb;
        if globals.bloom.y > 0.0 {
            // Thresholded: only the bright part is swapped for its blur.
            scene += (bloom - bloom_bright(scene)) * globals.bloom.x;
        } else {
            scene = mix(scene, bloom, globals.bloom.x);
        }
    }
    if globals.vignette.x > 0.0 {
        scene *= vignette(in.clip.xy);
    }
    var hdr = scene * globals.post.x;
    if globals.grade.z > 0.5 {
        hdr = grade_scene(hdr);
    }
    var mapped: vec3<f32>;
    let mode = u32(globals.post.y + 0.5);
    if mode == 0u {
        mapped = agx(hdr);
    } else if mode == 1u {
        mapped = aces_fitted(hdr);
    } else if mode == 3u {
        mapped = pbr_neutral(hdr);
    } else {
        mapped = hdr;
    }
    var display = linear_to_srgb(saturate(mapped));
    if globals.grade.w > 0.5 {
        display = grade_display(display);
    }
    let srgb = display + vec3<f32>(dither(in.clip.xy));
    return vec4<f32>(srgb_to_linear(saturate(srgb)), 1.0);
}
