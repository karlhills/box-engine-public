// StandardMaterial bind group (group 2). Must match
// `StandardMaterialUniform` in src/uniforms.rs.
// Requires common.wgsl, object.wgsl and wind.wgsl.

struct StandardMaterial {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    // x = roughness, y = metallic, z = alpha cutoff (mask mode), w = depth
    // fade distance (transparent modes)
    params: vec4<f32>,
    // x bit 0: has a normal map; bit 1: the wind weight is the vertex alpha
    flags: vec4<u32>,
    // wind sway: x = strength, y = frequency (Hz), z/w = bottom/top of the
    // height weight ramp (object space)
    wind: vec4<f32>,
    // xy = texture repeats per UV unit (`uv_scale`)
    uv: vec4<f32>,
};

@group(2) @binding(0) var<uniform> material: StandardMaterial;
// Missing textures are 1x1 defaults: white base colour, ORM and emissive
// (factors pass through), a flat normal.
@group(2) @binding(1) var base_color_texture: texture_2d<f32>;
@group(2) @binding(2) var normal_texture: texture_2d<f32>;
// occlusion (r), roughness (g), metallic (b)
@group(2) @binding(3) var orm_texture: texture_2d<f32>;
@group(2) @binding(4) var emissive_texture: texture_2d<f32>;
@group(2) @binding(5) var material_sampler: sampler;

// Texture coordinates of UV set 0 scaled by the material's `uv_scale`.
fn material_uv(uv: vec2<f32>) -> vec2<f32> {
    return uv * material.uv.xy;
}

// The vertex alpha's share of the material alpha: none when it is the wind
// weight instead.
fn vertex_opacity(alpha: f32) -> f32 {
    if (material.flags.x & 2u) != 0u {
        return 1.0;
    }
    return alpha;
}

// World position (w = 1) of a vertex of the current `object`, swayed by the
// material's wind. The `*_wind` vertex entry points of every pass of
// swaying standard materials use it (the others do not read the material).
fn wind_world_position(in: VertexInput) -> vec4<f32> {
    let world = object.model * vec4<f32>(in.position, 1.0);
    var weight = in.color.a;
    if (material.flags.x & 2u) == 0u {
        weight = wind_height_weight(in.position.y, material.wind.z, material.wind.w);
    }
    return vec4<f32>(world.xyz + wind_offset(world.xyz, weight, material.wind.x, material.wind.y), 1.0);
}
