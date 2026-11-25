// A material shader that reads the scene colour: water that refracts what
// is under it. Ripples bend the view of the bottom, the water absorbs and
// tints the light that crosses it, a line of foam marks where it meets
// geometry, and the rippled normal gives the engine's lighting its sun
// glint and sky reflection.
//
// Use it with a transparent alpha mode ("Blend"). Calling `scene_color_at`
// makes the engine copy the opaque scene before it draws transparent
// surfaces, and `scene_depth` makes it run its depth prepass, while the
// material is visible. The shader returns alpha 1 and shows the scene
// itself: the refracted colour goes out through `emissive` (it is already
// lit), the water's own colour through `base_color` (lit by the engine).
//
// Where the scene colour is unavailable the shader falls back to plain
// alpha blending, like shoreline.wgsl.

@material struct Pond {
    /// Colour the water scatters back in the deep (linear).
    @color deep: vec3f = vec3f(0.01, 0.07, 0.11),
    /// Tint of the light that crosses the water (linear).
    @color tint: vec3f = vec3f(0.45, 0.85, 0.8),
    /// Depth (m) at which the bottom is no longer visible.
    @range(0.05, 10.0) depth: f32 = 1.2,
    /// Width (m) of the foam line where the water meets geometry.
    @range(0.0, 1.0) foam: f32 = 0.05,
    /// Steepness of the ripples (0 flat).
    @range(0.0, 1.0) ripple: f32 = 0.12,
    /// Ripples per metre.
    @range(0.1, 20.0) ripple_scale: f32 = 5.0,
    /// How far ripples bend the view: the offset, as a fraction of the
    /// view, of a full slope seen from 1 m.
    @range(0.0, 1.0) refraction: f32 = 0.25,
}

// Slope (d height / d x, d height / d z) of three crossing wave trains.
fn pond_slope(xz: vec2f, scale: f32, t: f32) -> vec2f {
    let a = vec2f(0.8, 0.6);
    let b = vec2f(-0.5, 0.86);
    let c = vec2f(0.2, -0.98);
    let p = xz * scale;
    return a * cos(dot(p, a) + t * 1.3)
        + b * 0.7 * cos(dot(p, b) * 1.7 + t * 1.9)
        + c * 0.5 * cos(dot(p, c) * 2.6 - t * 2.4);
}

fn surface(in: SurfaceInput, p: Pond) -> Surface {
    var s = default_surface(in);
    let slope = pond_slope(in.position.xz, p.ripple_scale, globals.time.x) * p.ripple;
    s.normal = normalize(vec3f(-slope.x, 1.0, -slope.y));
    s.roughness = 0.08;

    // Metres of water behind this pixel, along the view.
    let here = fragment_view_depth(in.frag_coord);
    let straight = max(scene_depth(in.frag_coord) - here, 0.0);
    // Bend the view by the ripple: less in the shallows (so the shoreline
    // stays put) and with distance.
    let bend = slope * p.refraction * saturate(straight / p.depth) / max(here, 1.0);
    var uv = scene_uv(in.frag_coord) + bend;
    var gap = scene_depth_at(uv) - here;
    if gap < 0.0 {
        // The bent view landed on something in front of the water.
        uv = scene_uv(in.frag_coord);
        gap = straight;
    }
    let t = saturate(gap / p.depth);
    let foam = 1.0 - smoothstep(0.0, max(p.foam, 1e-4), straight);
    let white = vec3f(0.9, 0.95, 0.95);

    if scene_color_available() {
        // Light from the bottom, tinted and absorbed by the water it crossed.
        let through = scene_color_at(uv) * mix(vec3f(1.0), p.tint, saturate(t * 3.0)) * (1.0 - t);
        s.emissive = through * (1.0 - foam);
        s.base_color = mix(p.deep * t, white, foam);
        s.alpha = 1.0;
    } else {
        s.base_color = mix(mix(p.tint * 0.5, p.deep, t), white, foam);
        s.alpha = max(mix(0.3, 0.9, t), foam);
    }
    return s;
}
