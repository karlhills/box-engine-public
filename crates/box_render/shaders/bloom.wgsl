// Bloom (Jimenez, "Next Generation Post Processing in Call of Duty: Advanced
// Warfare", 2014): the resolved HDR scene is downsampled into a mip chain
// with a 13-tap filter (`fs_downsample_first` adds a Karis average against
// fireflies), then each mip is upsampled with a 3x3 tent filter and blended
// into the next larger one with the blend constant `scatter`:
// mip[i] = mix(mip[i], up(mip[i + 1]), scatter). Mip 0 is a weighted
// average of every blur level, so mixing it into the scene in the
// tone-mapping pass redistributes light without adding any.

#include "common.wgsl"

@group(1) @binding(0) var source: texture_2d<f32>;
@group(1) @binding(1) var source_sampler: sampler;

struct BloomOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> BloomOutput {
    let ndc = fullscreen_ndc(vertex_index);
    var out: BloomOutput;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = ndc * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return out;
}

fn tap(uv: vec2<f32>, texel: vec2<f32>, x: f32, y: f32) -> vec3<f32> {
    return textureSampleLevel(source, source_sampler, uv + vec2<f32>(x, y) * texel, 0.0).rgb;
}

// Weight of a 2x2 group against fireflies: 1 / (1 + luma).
fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luminance(c));
}

struct Taps {
    a: vec3<f32>, b: vec3<f32>, c: vec3<f32>,
    d: vec3<f32>, e: vec3<f32>, f: vec3<f32>,
    g: vec3<f32>, h: vec3<f32>, i: vec3<f32>,
    j: vec3<f32>, k: vec3<f32>, l: vec3<f32>, m: vec3<f32>,
};

// The 13 taps around `uv` in source texels: a 4x4 box of corners (a..i) and
// an inner diamond (j..m).
fn taps13(uv: vec2<f32>) -> Taps {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    var t: Taps;
    t.a = tap(uv, texel, -2.0, -2.0);
    t.b = tap(uv, texel, 0.0, -2.0);
    t.c = tap(uv, texel, 2.0, -2.0);
    t.d = tap(uv, texel, -2.0, 0.0);
    t.e = tap(uv, texel, 0.0, 0.0);
    t.f = tap(uv, texel, 2.0, 0.0);
    t.g = tap(uv, texel, -2.0, 2.0);
    t.h = tap(uv, texel, 0.0, 2.0);
    t.i = tap(uv, texel, 2.0, 2.0);
    t.j = tap(uv, texel, -1.0, -1.0);
    t.k = tap(uv, texel, 1.0, -1.0);
    t.l = tap(uv, texel, -1.0, 1.0);
    t.m = tap(uv, texel, 1.0, 1.0);
    return t;
}

@fragment
fn fs_downsample(in: BloomOutput) -> @location(0) vec4<f32> {
    let t = taps13(in.uv);
    var c = t.e * 0.125;
    c += (t.a + t.c + t.g + t.i) * 0.03125;
    c += (t.b + t.d + t.f + t.h) * 0.0625;
    c += (t.j + t.k + t.l + t.m) * 0.125;
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_downsample_first(in: BloomOutput) -> @location(0) vec4<f32> {
    let t = taps13(in.uv);
    // Five overlapping 2x2 groups, each weighted by its Karis average.
    let g0 = (t.j + t.k + t.l + t.m) * 0.25;
    let g1 = (t.a + t.b + t.d + t.e) * 0.25;
    let g2 = (t.b + t.c + t.e + t.f) * 0.25;
    let g3 = (t.d + t.e + t.g + t.h) * 0.25;
    let g4 = (t.e + t.f + t.h + t.i) * 0.25;
    let w0 = karis(g0) * 0.5;
    let w1 = karis(g1) * 0.125;
    let w2 = karis(g2) * 0.125;
    let w3 = karis(g3) * 0.125;
    let w4 = karis(g4) * 0.125;
    let c = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / max(w0 + w1 + w2 + w3 + w4, 1e-6);
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_upsample(in: BloomOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    var c = tap(in.uv, texel, 0.0, 0.0) * 4.0;
    c += (tap(in.uv, texel, -1.0, 0.0) + tap(in.uv, texel, 1.0, 0.0) + tap(in.uv, texel, 0.0, -1.0) + tap(in.uv, texel, 0.0, 1.0)) * 2.0;
    c += tap(in.uv, texel, -1.0, -1.0) + tap(in.uv, texel, 1.0, -1.0) + tap(in.uv, texel, -1.0, 1.0) + tap(in.uv, texel, 1.0, 1.0);
    return vec4<f32>(c / 16.0, 1.0);
}
