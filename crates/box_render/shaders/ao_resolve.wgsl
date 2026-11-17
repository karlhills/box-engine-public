// Ambient occlusion, step 1: the single-sample depth + normal prepass
// downsampled to half resolution. Each half-resolution texel takes the
// top-left pixel of its 2x2 block, so depth and normal always come from the
// same surface. Outputs the linear view depth (sky: AO_SKY_DEPTH) and the
// encoded normal.

#include "common.wgsl"
#include "ao_common.wgsl"

@group(1) @binding(0) var prepass_depth: texture_depth_2d;
@group(1) @binding(1) var prepass_normal_texture: texture_2d<f32>;

struct ResolveOutput {
    @location(0) depth: f32,
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_resolve(in: FullscreenOutput) -> ResolveOutput {
    let p = vec2<i32>(in.clip.xy) * 2;
    var out: ResolveOutput;
    out.depth = linear_depth(textureLoad(prepass_depth, p, 0));
    out.normal = textureLoad(prepass_normal_texture, p, 0);
    return out;
}
