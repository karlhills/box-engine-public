// A material shader (ShaderMaterial): diagonal stripes in a tint colour
// with glowing grooves, and an optional sway of the top of the mesh.
//
// The header declares the parameters an inspector shows; the engine turns
// it into a plain WGSL struct bound as `material`.

import box::lighting;

@material struct Stripes {
    /// Stripe colour (linear).
    @color tint: vec3f = vec3f(0.85, 0.25, 0.12),
    /// Stripes per UV unit.
    @range(1.0, 40.0) frequency: f32 = 8.0,
    /// Fraction of each stripe in the tint colour.
    @range(0.0, 1.0) width: f32 = 0.5,
    /// Emission of the grooves between stripes.
    @range(0.0, 10.0) glow: f32 = 0.0,
    /// Sideways sway of the top, in metres (0 for none).
    @range(0.0, 1.0) sway: f32 = 0.0,
}

fn surface(in: SurfaceInput, p: Stripes) -> Surface {
    var s = default_surface(in);
    let t = fract((in.uv.x + in.uv.y) * p.frequency);
    let stripe = step(t, p.width);
    s.base_color = mix(vec3f(0.92, 0.9, 0.85), p.tint, stripe);
    s.roughness = mix(0.8, 0.35, stripe);
    // Narrow grooves at the stripe edges.
    let edge = min(t, 1.0 - t);
    s.emissive = p.tint * p.glow * (1.0 - smoothstep(0.0, 0.04, edge));
    return s;
}

fn vertex(position: vec3f, in: VertexInput, p: Stripes) -> vec3f {
    let height = max(in.position.y, 0.0);
    return position + vec3f(sin(globals.time.x * 2.0) * p.sway * height, 0.0, 0.0);
}
