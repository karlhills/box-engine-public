// Entry points of a material shader (`ShaderMaterial`), included after the
// user's code, which defines `surface` (and `vertex`, or the engine adds a
// pass-through one). See material_api.wgsl.

// See forward.wgsl.
override ALPHA_MODE: u32 = 0u;
override DOUBLE_SIDED: bool = false;
// Mask mode: fragments with alpha below this are discarded.
override ALPHA_CUTOFF: f32 = 0.5;
// Transparent modes: `ShaderMaterial::depth_fade` (0 off).
override DEPTH_FADE: f32 = 0.0;

// Shadow cascade pass only (group 0, binding 4; see shadow_depth.wgsl).
struct MaterialShadowPass {
    light_view_proj: mat4x4<f32>,
};
@group(0) @binding(4) var<uniform> material_shadow_pass: MaterialShadowPass;

struct MaterialVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) tangent: vec4<f32>,
    @location(5) uv1: vec2<f32>,
};

// `instance` is the entry point's `in.instance` (see object.wgsl).
fn material_varyings(in: VertexInput, instance: u32, view_proj: mat4x4<f32>) -> MaterialVaryings {
    load_object(instance);
    let world = object.model * vec4<f32>(in.position, 1.0);
    let position = vertex(world.xyz, in, material);
    var out: MaterialVaryings;
    out.clip = view_proj * vec4<f32>(position, 1.0);
    out.world_pos = position;
    out.normal = (object.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz;
    out.color = in.color;
    out.uv = in.uv;
    out.tangent = world_tangent(in.tangent);
    out.uv1 = in.uv1;
    return out;
}

fn material_surface_input(in: MaterialVaryings, front_facing: bool) -> SurfaceInput {
    var si: SurfaceInput;
    si.position = in.world_pos;
    let len_sq = dot(in.normal, in.normal);
    var n = vec3<f32>(0.0, 1.0, 0.0);
    if len_sq > 1e-20 {
        n = in.normal * inverseSqrt(len_sq);
    }
    if DOUBLE_SIDED && !front_facing {
        n = -n;
    }
    si.normal = n;
    si.tangent = in.tangent;
    si.uv = in.uv;
    si.uv1 = in.uv1;
    si.color = in.color;
    si.view_dir = normalize(globals.camera_pos.xyz - in.world_pos);
    si.front_facing = front_facing;
    si.frag_coord = in.clip;
    return si;
}

@vertex
fn vs_main(in: VertexInput) -> MaterialVaryings {
    return material_varyings(in, in.instance, globals.view_proj);
}

@fragment
fn fs_main(in: MaterialVaryings, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let si = material_surface_input(in, front_facing);
    let s = surface(si, material);
    if ALPHA_MODE == ALPHA_MASK && s.alpha < ALPHA_CUTOFF {
        discard;
    }
    var color = vec3<f32>(0.0);
    if !is_multiply(ALPHA_MODE) {
        let view_depth = dot(in.world_pos - globals.camera_pos.xyz, globals.camera_forward.xyz);
        var shadow = 1.0;
        if has_sun() && globals.shadow.x > 0.5 {
            shadow = sample_shadow(in.world_pos, s.normal, view_depth);
        }
        color = shade_standard(s, si.view_dir, shadow);
        if globals.shadow.w > 0.5 {
            color *= cascade_debug_tint(view_depth);
        }
    }
    var fade = 1.0;
    if ALPHA_MODE >= ALPHA_BLEND {
        fade = depth_fade(in.clip, DEPTH_FADE);
    }
    return forward_output(ALPHA_MODE, color, s.base_color, s.alpha, in.world_pos, fade);
}

@vertex
fn vs_shadow(in: VertexInput) -> MaterialVaryings {
    return material_varyings(in, in.instance, material_shadow_pass.light_view_proj);
}

@vertex
fn vs_shadow_pancake(in: VertexInput) -> MaterialVaryings {
    var out = material_varyings(in, in.instance, material_shadow_pass.light_view_proj);
    out.clip.z = max(out.clip.z, 0.0);
    return out;
}

@fragment
fn fs_shadow_mask(in: MaterialVaryings, @builtin(front_facing) front_facing: bool) {
    let s = surface(material_surface_input(in, front_facing), material);
    if s.alpha < ALPHA_CUTOFF {
        discard;
    }
}

// Depth + normal prepass (prepass.wgsl): the geometric normal; masked
// materials run `surface` for their alpha.
@fragment
fn fs_prepass(in: MaterialVaryings, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    if ALPHA_MODE == ALPHA_MASK {
        let s = surface(material_surface_input(in, front_facing), material);
        if s.alpha < ALPHA_CUTOFF {
            discard;
        }
    }
    return prepass_normal(in.normal, DOUBLE_SIDED && !front_facing);
}

// Selection outline mask of shaders with a `vertex` hook (outline_mask.wgsl
// draws the rest): the outline id, with the vertices where the main pass
// puts them.
struct MaterialOutlineOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
};

@vertex
fn vs_outline(in: VertexInput) -> MaterialOutlineOutput {
    let v = material_varyings(in, in.instance, globals.view_proj);
    var out: MaterialOutlineOutput;
    out.clip = v.clip;
    out.id = u32(object.params.z);
    return out;
}

@fragment
fn fs_outline(in: MaterialOutlineOutput) -> @location(0) vec4<u32> {
    return vec4<u32>(in.id, 0u, 0u, 0u);
}
