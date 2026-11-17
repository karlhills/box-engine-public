// StandardMaterial forward pass (HDR, MSAA). One module serves every alpha
// mode and face mode through pipeline-overridable constants. It also holds
// the prepass and outline-mask entry points of standard materials. The
// `*_wind` vertex entry points of swaying materials (`WindSway`) all place
// vertices with `wind_world_position`, so every pass moves alike.

#include "common.wgsl"
#include "object.wgsl"
#include "lighting.wgsl"
#include "shadows.wgsl"
#include "fog.wgsl"
#include "forward.wgsl"
#include "prepass.wgsl"
#include "wind.wgsl"
#include "material_standard.wgsl"

// See forward.wgsl.
override ALPHA_MODE: u32 = 0u;
// Both faces are drawn; back faces flip the normal.
override DOUBLE_SIDED: bool = false;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) tangent: vec4<f32>,
};

fn main_vertex(in: VertexInput, world: vec4<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = (object.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz;
    out.color = in.color;
    out.uv = in.uv;
    out.tangent = world_tangent(in.tangent);
    return out;
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    load_object(in.instance);
    return main_vertex(in, object.model * vec4<f32>(in.position, 1.0));
}

@vertex
fn vs_main_wind(in: VertexInput) -> VertexOutput {
    load_object(in.instance);
    return main_vertex(in, wind_world_position(in));
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    // Sample everything before any discard (uniform control flow).
    let uv = material_uv(in.uv);
    let base_sample = textureSample(base_color_texture, material_sampler, uv);
    let normal_sample = textureSample(normal_texture, material_sampler, uv).xyz;
    let orm = textureSample(orm_texture, material_sampler, uv).xyz;
    let emissive_sample = textureSample(emissive_texture, material_sampler, uv).rgb;

    let vertex_color = vec4<f32>(in.color.rgb, vertex_opacity(in.color.a));
    let base = material.base_color * vertex_color * base_sample;
    if ALPHA_MODE == ALPHA_MASK && base.a < material.params.z {
        discard;
    }

    var n = normalize(in.normal);
    if (material.flags.x & 1u) != 0u {
        n = perturb_normal(n, in.tangent, normal_sample * 2.0 - 1.0);
    }
    if DOUBLE_SIDED && !front_facing {
        n = -n;
    }
    let to_camera = globals.camera_pos.xyz - in.world_pos;
    let v = normalize(to_camera);
    let view_depth = dot(-to_camera, globals.camera_forward.xyz);

    var s: Surface;
    s.position = in.world_pos;
    s.normal = n;
    s.base_color = base.rgb;
    s.alpha = base.a;
    s.roughness = material.params.x * orm.g;
    s.metallic = material.params.y * orm.b;
    s.emissive = material.emissive.rgb * emissive_sample;
    s.occlusion = orm.r;

    var color = vec3<f32>(0.0);
    if ALPHA_MODE != ALPHA_MULTIPLY {
        var shadow = 1.0;
        if has_sun() && globals.shadow.x > 0.5 {
            shadow = sample_shadow(in.world_pos, n, view_depth);
        }
        color = shade_standard(s, v, shadow);
        if globals.shadow.w > 0.5 {
            color *= cascade_debug_tint(view_depth);
        }
    }
    var fade = 1.0;
    if ALPHA_MODE >= ALPHA_BLEND {
        fade = depth_fade(in.clip, material.params.w);
    }
    return forward_output(ALPHA_MODE, color, s.base_color, s.alpha, in.world_pos, fade);
}

// Depth + normal prepass (prepass.wgsl). `fs_prepass` reads no material, so
// opaque materials and the error material share its pipeline layout;
// alpha-masked and swaying materials bind the material (`fs_prepass_mask`
// for masked ones).
struct PrepassOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
};

fn prepass_vertex(in: VertexInput, world: vec4<f32>) -> PrepassOutput {
    var out: PrepassOutput;
    out.clip = globals.view_proj * world;
    out.normal = (object.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz;
    out.uv = in.uv;
    out.alpha = in.color.a;
    return out;
}

@vertex
fn vs_prepass(in: VertexInput) -> PrepassOutput {
    load_object(in.instance);
    return prepass_vertex(in, object.model * vec4<f32>(in.position, 1.0));
}

@vertex
fn vs_prepass_wind(in: VertexInput) -> PrepassOutput {
    load_object(in.instance);
    return prepass_vertex(in, wind_world_position(in));
}

@fragment
fn fs_prepass(in: PrepassOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    return prepass_normal(in.normal, DOUBLE_SIDED && !front_facing);
}

@fragment
fn fs_prepass_mask(in: PrepassOutput, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let alpha = textureSample(base_color_texture, material_sampler, material_uv(in.uv)).a * material.base_color.a * vertex_opacity(in.alpha);
    if alpha < material.params.z {
        discard;
    }
    return prepass_normal(in.normal, DOUBLE_SIDED && !front_facing);
}

// Selection outline mask of swaying materials (outline_mask.wgsl draws the
// rest): the object's outline id, with the vertices where the main pass
// puts them.
struct OutlineOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
};

@vertex
fn vs_outline_wind(in: VertexInput) -> OutlineOutput {
    load_object(in.instance);
    var out: OutlineOutput;
    out.clip = globals.view_proj * wind_world_position(in);
    out.id = u32(object.params.z);
    return out;
}

@fragment
fn fs_outline(in: OutlineOutput) -> @location(0) vec4<u32> {
    return vec4<u32>(in.id, 0u, 0u, 0u);
}
