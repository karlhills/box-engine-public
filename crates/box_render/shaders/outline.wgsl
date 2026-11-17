// Selection outline, step 2: composited after tone mapping (alpha blended into
// the sRGB output, so colours are display-referred like display lines).
//
// The mask holds a per-object id (see outline_mask.wgsl), so each selected
// object has its own silhouette: a pixel is outlined when, within the outline
// width, there is a pixel of another selected object that is in front of it
// (or the pixel belongs to no selected object). Objects standing on a
// selected ground therefore keep their outlines, drawn on the ground around
// them. The band fades over its last pixel. Where the silhouette pixel is
// behind other scene geometry the outline is drawn with `occluded_alpha`.
// Drawn inside a scissor rectangle around the selected objects only.

#include "common.wgsl"

struct Outline {
    primary: vec4<f32>,
    secondary: vec4<f32>,
    // x = width in pixels, y = occluded alpha
    params: vec4<f32>,
};

// Outline id per pixel: 0 none, else (object slot + 1) * 2 + primary bit.
@group(1) @binding(0) var mask: texture_2d<u32>;
@group(1) @binding(1) var mask_depth: texture_depth_2d;
// The scene's multisampled depth (reverse-Z: nearer is greater).
@group(1) @binding(2) var scene_depth: texture_depth_multisampled_2d;
@group(1) @binding(3) var<uniform> outline: Outline;

// Largest supported radius in pixels (the CPU clamps the width to 4).
const MAX_RADIUS: i32 = 4;
// A selected surface behind the scene depth by less than this fraction of
// its depth still counts as visible.
const DEPTH_TOLERANCE: f32 = 0.0005;

struct OutlineOutput {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> OutlineOutput {
    var out: OutlineOutput;
    out.clip = vec4<f32>(fullscreen_ndc(vertex_index), 0.0, 1.0);
    return out;
}

// The farthest scene depth among the pixel's samples. A planar surface's
// depth at the pixel centre is never farther than this (the centre lies
// inside the sample pattern), so a visible selected surface always passes.
fn scene_far_depth(pixel: vec2<i32>) -> f32 {
    var d = 1.0;
    let n = textureNumSamples(scene_depth);
    for (var i = 0u; i < n; i++) {
        d = min(d, textureLoad(scene_depth, pixel, i32(i)));
    }
    return d;
}

@fragment
fn fs_main(in: OutlineOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip.xy);
    let center = textureLoad(mask, p, 0).r;
    // Reverse-Z; an empty pixel is behind everything.
    var center_depth = 0.0;
    if center != 0u {
        center_depth = textureLoad(mask_depth, p, 0);
    }
    let size = vec2<i32>(textureDimensions(mask));
    let width = outline.params.x;
    let r = min(i32(ceil(width)), MAX_RADIUS);
    let reach = width + 0.5;
    var best = reach;
    var best_id = 0u;
    var best_p = p;
    for (var dy = -r; dy <= r; dy++) {
        for (var dx = -r; dx <= r; dx++) {
            let q = p + vec2<i32>(dx, dy);
            if any(q < vec2<i32>(0)) || any(q >= size) {
                continue;
            }
            let d = length(vec2<f32>(f32(dx), f32(dy)));
            if d >= best {
                continue;
            }
            let id = textureLoad(mask, q, 0).r;
            if id == 0u || id == center {
                continue;
            }
            // Another object: only an edge if it is in front of this pixel.
            if center != 0u && textureLoad(mask_depth, q, 0) <= center_depth {
                continue;
            }
            best = d;
            best_id = id;
            best_p = q;
        }
    }
    if best_id == 0u {
        discard;
    }
    let coverage = saturate(reach - best);
    let visible = textureLoad(mask_depth, best_p, 0)
        >= scene_far_depth(best_p) * (1.0 - DEPTH_TOLERANCE);
    var color = outline.secondary;
    if (best_id & 1u) == 1u {
        color = outline.primary;
    }
    var alpha = color.a * coverage;
    if !visible {
        alpha *= outline.params.y;
    }
    return vec4<f32>(color.rgb, alpha);
}
