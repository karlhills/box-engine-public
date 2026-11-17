// Depth-only shadow cascade pass. Group 0 holds the globals (binding 0, for
// wind and material shaders' vertex hooks) and one light view-projection per
// cascade, selected with a dynamic offset (binding 4).
//
// Casters between the light and the cascade's box still shadow it. With depth
// clip control the pipeline disables depth clipping and their depth clamps to
// the near plane (`vs_main`); without it `vs_pancake` flattens them onto the
// near plane in the vertex shader. Alpha-masked materials add `fs_mask`
// (alpha-tested casters: foliage cards cast their cut-out); swaying materials
// use the `*_wind` vertex entry points, which place vertices exactly as the
// main pass does.

#include "common.wgsl"
#include "object.wgsl"
#include "wind.wgsl"
#include "material_standard.wgsl"

struct ShadowPass {
    light_view_proj: mat4x4<f32>,
};

@group(0) @binding(4) var<uniform> shadow_pass: ShadowPass;

struct ShadowOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

fn shadow_vertex(in: VertexInput, clip: vec4<f32>) -> ShadowOutput {
    var out: ShadowOutput;
    out.clip = clip;
    out.uv = in.uv;
    out.alpha = in.color.a;
    return out;
}

// Orthographic, so w is 1 and z is the depth.
fn pancake(out: ShadowOutput) -> ShadowOutput {
    var o = out;
    o.clip.z = max(o.clip.z, 0.0);
    return o;
}

// `instance` is the entry point's `in.instance` (see object.wgsl).
fn static_clip(in: VertexInput, instance: u32) -> vec4<f32> {
    load_object(instance);
    return shadow_pass.light_view_proj * object.model * vec4<f32>(in.position, 1.0);
}

fn wind_clip(in: VertexInput, instance: u32) -> vec4<f32> {
    load_object(instance);
    return shadow_pass.light_view_proj * wind_world_position(in);
}

@vertex
fn vs_main(in: VertexInput) -> ShadowOutput {
    return shadow_vertex(in, static_clip(in, in.instance));
}

@vertex
fn vs_pancake(in: VertexInput) -> ShadowOutput {
    return pancake(shadow_vertex(in, static_clip(in, in.instance)));
}

@vertex
fn vs_wind(in: VertexInput) -> ShadowOutput {
    return shadow_vertex(in, wind_clip(in, in.instance));
}

@vertex
fn vs_wind_pancake(in: VertexInput) -> ShadowOutput {
    return pancake(shadow_vertex(in, wind_clip(in, in.instance)));
}

@fragment
fn fs_mask(in: ShadowOutput) {
    let alpha = textureSample(base_color_texture, material_sampler, material_uv(in.uv)).a * material.base_color.a * vertex_opacity(in.alpha);
    if alpha < material.params.z {
        discard;
    }
}
