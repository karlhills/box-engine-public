// CPU-simulated billboard particles: one instance per particle (a vertex
// buffer stepped per instance), drawn as a camera-facing quad (a triangle
// strip of four vertices) in the transparent pass. Alpha-blended output is
// premultiplied; additive output is added. Soft particles fade against the
// prepass depth (`depth_fade`), and every particle gets fog like other
// transparent surfaces. Lit particles take the ambient light (ambient
// volumes included), half the sun and half the local lights at their
// centre, per vertex.

#include "common.wgsl"
#include "lighting.wgsl"
#include "fog.wgsl"
#include "forward.wgsl"

// Additive blending (ParticleBlend::Additive); otherwise alpha.
override ADDITIVE: bool = false;

@group(1) @binding(0) var particle_texture: texture_2d<f32>;
@group(1) @binding(1) var particle_sampler: sampler;

// Must match `GpuParticle` in src/particle_render.rs.
struct ParticleInput {
    // xyz = world position, w = quad width (m)
    @location(0) position_size: vec4<f32>,
    // linear colour * intensity, alpha
    @location(1) color: vec4<f32>,
    // atlas frame: uv offset xy, uv scale zw
    @location(2) uv_rect: vec4<f32>,
    // x = rotation (rad), y = soft distance (m), z = 1 when lit
    @location(3) params: vec4<f32>,
    @builtin(vertex_index) vertex: u32,
};

struct ParticleVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) soft: f32,
};

// World-space camera right and up, from the inverse view-projection (so
// they follow any camera roll).
fn camera_axis(ndc: vec2<f32>) -> vec3<f32> {
    let o = globals.inv_view_proj * vec4<f32>(0.0, 0.0, 0.5, 1.0);
    let p = globals.inv_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    return normalize(p.xyz / p.w - o.xyz / o.w);
}

@vertex
fn vs_main(in: ParticleInput) -> ParticleVaryings {
    // Strip corners (-1, -1), (1, -1), (-1, 1), (1, 1).
    let corner = vec2<f32>(f32(in.vertex & 1u), f32(in.vertex >> 1u)) * 2.0 - 1.0;
    let c = cos(in.params.x);
    let s = sin(in.params.x);
    let r = vec2<f32>(corner.x * c - corner.y * s, corner.x * s + corner.y * c);
    let half = in.position_size.w * 0.5;
    let world = in.position_size.xyz
        + (camera_axis(vec2<f32>(1.0, 0.0)) * r.x + camera_axis(vec2<f32>(0.0, 1.0)) * r.y) * half;
    var out: ParticleVaryings;
    out.clip = globals.view_proj * vec4<f32>(world, 1.0);
    out.world_pos = world;
    out.uv = in.uv_rect.xy + (corner * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5)) * in.uv_rect.zw;
    var color = in.color;
    if in.params.z > 0.5 {
        color = vec4<f32>(color.rgb * unoriented_irradiance(in.position_size.xyz), color.a);
    }
    out.color = color;
    out.soft = in.params.y;
    return out;
}

@fragment
fn fs_main(in: ParticleVaryings) -> @location(0) vec4<f32> {
    let texel = textureSample(particle_texture, particle_sampler, in.uv);
    let c = in.color * texel;
    let fade = depth_fade(in.clip, in.soft);
    return forward_output(select(ALPHA_BLEND, ALPHA_ADDITIVE, ADDITIVE), c.rgb, c.rgb, c.a, in.world_pos, fade);
}
