// A material shader that reads the scene depth: water whose colour and
// opacity deepen with the depth of what is under it, with a line of foam
// where it meets geometry, and small waves from the `vertex` hook.
//
// Use it with a transparent alpha mode ("Blend"). Calling `scene_depth`
// makes the engine run its depth prepass while the material is visible.
// `scene_depth(in.frag_coord)` is the view depth (metres) of the opaque
// surface behind this pixel and `fragment_view_depth(in.frag_coord)` that of
// the water itself; where the scene depth is unavailable (or only sky is
// behind) `scene_depth` returns SCENE_DEPTH_FAR, which reads as deep water.
//
// The waves lift vertices up to `wave_height` off the mesh, outside its
// bounds: give the material a `bounds_padding` of at least that, so the
// water is not culled early at the edges of the view or of shadows.

@material struct Shoreline {
    /// Water colour in the deep (linear).
    @color deep: vec3f = vec3f(0.02, 0.1, 0.2),
    /// Water colour in the shallows (linear).
    @color shallow: vec3f = vec3f(0.12, 0.5, 0.55),
    /// Depth (m) at which the water reaches its deep colour and opacity.
    @range(0.05, 10.0) depth: f32 = 0.6,
    /// Width (m) of the foam line where the water meets geometry.
    @range(0.0, 1.0) foam: f32 = 0.06,
    /// Wave height (m).
    @range(0.0, 1.0) wave_height: f32 = 0.02,
}

fn surface(in: SurfaceInput, p: Shoreline) -> Surface {
    var s = default_surface(in);
    // Metres of water between this pixel and what is behind it, along the view.
    let gap = max(scene_depth(in.frag_coord) - fragment_view_depth(in.frag_coord), 0.0);
    let t = saturate(gap / p.depth);
    s.base_color = mix(p.shallow, p.deep, t);
    s.alpha = mix(0.3, 0.9, t);
    s.roughness = 0.1;
    let foam = 1.0 - smoothstep(0.0, max(p.foam, 1e-4), gap);
    s.base_color = mix(s.base_color, vec3f(0.9, 0.95, 0.95), foam);
    s.alpha = max(s.alpha, foam);
    return s;
}

fn vertex(position: vec3f, in: VertexInput, p: Shoreline) -> vec3f {
    let t = globals.time.x;
    let w = sin(position.x * 3.0 + t * 1.5) * cos(position.z * 2.5 + t * 1.1);
    return position + vec3f(0.0, w * p.wave_height, 0.0);
}
