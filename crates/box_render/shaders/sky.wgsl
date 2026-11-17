// Gradient sky with sun disc, glow and stars, under the atmosphere (haze,
// mist, quieting near the horizon), drawn as a fullscreen triangle at the
// far plane (reverse-Z depth 0) where no geometry was drawn.

#include "common.wgsl"
#include "fog.wgsl"

struct SkyOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> SkyOutput {
    let ndc = fullscreen_ndc(vertex_index);
    var out: SkyOutput;
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_main(in: SkyOutput) -> @location(0) vec4<f32> {
    // Reconstruct the view ray through a point on the near plane (depth 1 in reverse-Z).
    let p = globals.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let rd = normalize(p.xyz / p.w - globals.camera_pos.xyz);
    return vec4<f32>(sky_atmosphere(sky_radiance(rd), rd), 1.0);
}
