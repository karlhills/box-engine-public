// Display-referred debug lines, drawn after tone mapping straight into the
// output target. Colours are linear and alpha blended; the sRGB output view
// encodes them, so they appear exactly as given regardless of exposure and
// the tone mapper.

#include "common.wgsl"

// The scene's multisampled depth (reverse-Z: nearer is greater).
@group(1) @binding(0) var scene_depth: texture_depth_multisampled_2d;

struct LineInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

// A line behind a surface by less than this fraction of its depth still
// shows, so lines lying on surfaces (a grid on the ground) do not flicker.
const DEPTH_TOLERANCE: f32 = 0.001;

@vertex
fn vs_main(in: LineInput) -> LineOutput {
    var out: LineOutput;
    out.clip = globals.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

// The farthest scene depth among the pixel's samples, so a line is hidden
// only where the whole pixel is covered by something in front of it.
fn scene_far_depth(pixel: vec2<i32>) -> f32 {
    var d = 1.0;
    let n = textureNumSamples(scene_depth);
    for (var i = 0u; i < n; i++) {
        d = min(d, textureLoad(scene_depth, pixel, i32(i)));
    }
    return d;
}

@fragment
fn fs_depth_tested(in: LineOutput) -> @location(0) vec4<f32> {
    let scene = scene_far_depth(vec2<i32>(in.clip.xy));
    if in.clip.z < scene * (1.0 - DEPTH_TOLERANCE) {
        discard;
    }
    return in.color;
}

@fragment
fn fs_on_top(in: LineOutput) -> @location(0) vec4<f32> {
    return in.color;
}
