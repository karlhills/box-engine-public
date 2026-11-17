// Wind sway, shared by every pass of standard materials (mesh.wgsl,
// shadow_depth.wgsl) and available to material shaders' `vertex` hooks, so
// shadows, the prepass and outlines move exactly with the surface.
// Driven by the global wind (`Environment::wind`) and `globals.time`.
// Requires common.wgsl.

// World-space offset of a vertex at world position `p` with sway weight
// `weight` (0 pinned, 1 free), for a material that sways `strength` metres
// per unit of wind and flutters at `frequency` Hz: a lean downwind that
// swells as gust fronts roll past, plus a flutter whose phase varies in
// space so neighbours do not move in lockstep. Tips dip slightly as they
// lean. Zero when calm.
fn wind_offset(p: vec3<f32>, weight: f32, strength: f32, frequency: f32) -> vec3<f32> {
    let amount = globals.wind.z * strength * weight;
    if amount == 0.0 {
        return vec3<f32>(0.0);
    }
    let dir = globals.wind.xy;
    let t = globals.time.x;
    let along = dot(p.xz, dir);
    let gust = 0.5 + 0.5 * sin((along - t * globals.wind_gust.x) * (2.0 * PI / max(globals.wind.w, 0.01)));
    let phase = t * (2.0 * PI) * frequency + dot(p, vec3<f32>(0.7, 0.3, 0.5));
    let flutter = sin(phase) + 0.5 * sin(phase * 2.3 + 1.7);
    let lean = amount * (0.35 + 0.65 * gust);
    let shake = amount * 0.2 * flutter;
    let across = vec2<f32>(-dir.y, dir.x) * (shake * 0.5);
    return vec3<f32>(dir.x * (lean + shake) + across.x, -0.2 * lean, dir.y * (lean + shake) + across.y);
}

// The height weight ramp: 0 at `bottom`, 1 at `top` (object-space y).
fn wind_height_weight(y: f32, bottom: f32, top: f32) -> f32 {
    if top <= bottom {
        return select(0.0, 1.0, y >= top);
    }
    return smoothstep(bottom, top, y);
}
