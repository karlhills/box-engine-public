// The interface of material shaders (`ShaderMaterial`). A material shader
// defines
//
//     fn surface(in: SurfaceInput, p: <Params>) -> Surface
//
// and optionally
//
//     fn vertex(position: vec3<f32>, in: VertexInput, p: <Params>) -> vec3<f32>
//
// which returns the displaced world-space position (normals are not
// changed). The same hook places vertices in the main pass, the shadow
// cascades, the depth prepass and the selection outline. For wind sway call
//
//     wind_offset(position, weight, strength, frequency) -> vec3<f32>
//
// (wind.wgsl: the global `Environment::wind`; weight 0 pinned .. 1 free,
// e.g. `in.color.a` or `wind_height_weight(in.position.y, 0.0, 1.0)`):
//
//     fn vertex(position: vec3f, in: VertexInput, p: P) -> vec3f {
//         return position + wind_offset(position, in.color.a, p.sway, 1.2);
//     }
//
// `<Params>` is the `@material` struct (`MaterialParams` when the file
// declares none); its values are also the global `material`. The engine
// library (globals, object, lighting, shadows, fog, wind) is always
// available; `#import box::<module>` lines are accepted for readability.
// The returned Surface is lit with `shade_standard`, shadowed and fogged
// (fog, haze, mist, quieting) by the engine.
//
// Scene depth: a transparent material can read the depth of the opaque
// scene behind it (soft intersections, shorelines, depth tint):
//
//     let gap = scene_depth(in.frag_coord) - fragment_view_depth(in.frag_coord);
//
// both in metres along the camera's view axis. Calling `scene_depth` makes
// the engine run its depth prepass while the material is visible
// (`ShaderMaterial::scene_depth` asks for it explicitly). Where the depth is
// not available (a frame without the prepass, the shadow passes, opaque
// materials drawing into the prepass) or nothing opaque is behind (the
// sky), `scene_depth` returns SCENE_DEPTH_FAR, so a gap reads as "deep";
// `scene_depth_available()` tells the cases apart. The prepass holds opaque
// and masked surfaces only, so an opaque material reads its own depth.
//
// Scene colour: a transparent material can also read the colour of the
// opaque scene behind it (refraction, heat haze, frosted glass):
//
//     let uv = scene_uv(in.frag_coord) + offset;
//     s.emissive = scene_color_at(uv) * (1.0 - tint);
//     s.alpha = 1.0;
//
// `scene_color(frag_coord)` is the colour straight behind the pixel and
// `scene_color_at(uv)` the filtered colour at a viewport position (0..1,
// from the top left; clamped at the edges). The colour is linear HDR, lit
// and fogged, before exposure and tone mapping, and holds opaque and masked
// surfaces, the sky and scene lines; never other transparent surfaces.
// Calling either makes the engine draw transparent surfaces in a second
// pass after copying the opaque scene, while the material is visible. An
// offset sample can land on something in front of the surface: compare
// `scene_depth_at(uv)` with `fragment_view_depth` and fall back to the
// straight sample. Where the colour is unavailable (opaque and masked
// materials, the prepass, the shadow passes) both return black;
// `scene_color_available()` tells.
// Requires common.wgsl and lighting.wgsl.

struct SurfaceInput {
    // world-space position
    position: vec3<f32>,
    // unit world-space normal, flipped on back faces of double-sided materials
    normal: vec3<f32>,
    // world-space tangent, w = bitangent sign; zero when the mesh has none
    tangent: vec4<f32>,
    // UV set 0
    uv: vec2<f32>,
    // UV set 1 (zero when the mesh has none): data channels such as
    // box_arch's weathering coordinates
    uv1: vec2<f32>,
    // the mesh's custom data sets 0 and 1 (`MeshData::custom`; zero when
    // the mesh has none), interpolated across the triangle. The `vertex`
    // hook reads the per-vertex values as `in.custom0` / `in.custom1`.
    custom0: vec4<f32>,
    custom1: vec4<f32>,
    // vertex colour (linear)
    color: vec4<f32>,
    // unit vector toward the camera
    view_dir: vec3<f32>,
    front_facing: bool,
    // the fragment's @builtin(position): pixel centre in xy, depth in z;
    // pass it to `scene_depth` and `fragment_view_depth`
    frag_coord: vec4<f32>,
};

// What `scene_depth` returns where it has no depth: far enough that any
// gap computed from it counts as deep.
const SCENE_DEPTH_FAR: f32 = 1.0e9;

// True when the scene depth was rendered this frame and is bound here.
fn scene_depth_available() -> bool {
    return globals.scene_depth_on.x > 0.5 && textureDimensions(opaque_depth).x > 1u;
}

// Linear view depth (metres along the camera's forward axis) of the nearest
// opaque surface at `frag_coord` (`SurfaceInput.frag_coord`).
// SCENE_DEPTH_FAR where nothing opaque is there or the depth is unavailable.
fn scene_depth(frag_coord: vec4<f32>) -> f32 {
    if globals.scene_depth_on.x < 0.5 {
        return SCENE_DEPTH_FAR;
    }
    let size = textureDimensions(opaque_depth);
    let p = vec2<i32>(frag_coord.xy);
    if size.x <= 1u || p.x < 0 || p.y < 0 || p.x >= i32(size.x) || p.y >= i32(size.y) {
        // The 1x1 placeholder (prepass and shadow passes) or off the target.
        return SCENE_DEPTH_FAR;
    }
    let d = textureLoad(opaque_depth, p, 0);
    if d <= 0.0 {
        // The sky (reverse-Z clears to 0).
        return SCENE_DEPTH_FAR;
    }
    // Infinite reverse-Z: NDC depth = near / view depth.
    return globals.camera_pos.w / d;
}

// Viewport position (0..1 from the top left) of `frag_coord`, for
// `scene_color_at` and `scene_depth_at`.
fn scene_uv(frag_coord: vec4<f32>) -> vec2<f32> {
    return frag_coord.xy / globals.viewport.xy;
}

// `scene_depth` at a viewport position (0..1), clamped to the view.
fn scene_depth_at(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(opaque_depth));
    let p = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * size;
    return scene_depth(vec4<f32>(min(p, size - vec2<f32>(0.5)), 0.0, 1.0));
}

// True when the opaque scene's colour is bound here: the transparent draws
// of a frame where a visible material shader reads it.
fn scene_color_available() -> bool {
    return globals.scene_depth_on.y > 0.5 && textureDimensions(opaque_color).x > 1u;
}

// Linear HDR colour of the opaque scene straight behind `frag_coord`
// (`SurfaceInput.frag_coord`); black where it is unavailable.
fn scene_color(frag_coord: vec4<f32>) -> vec3<f32> {
    if !scene_color_available() {
        return vec3<f32>(0.0);
    }
    let size = vec2<i32>(textureDimensions(opaque_color));
    let p = clamp(vec2<i32>(frag_coord.xy), vec2<i32>(0), size - vec2<i32>(1));
    return textureLoad(opaque_color, p, 0).rgb;
}

// Filtered linear HDR colour of the opaque scene at a viewport position
// (0..1 from the top left, clamped); black where it is unavailable.
fn scene_color_at(uv: vec2<f32>) -> vec3<f32> {
    let c = textureSampleLevel(opaque_color, scene_sampler, uv, 0.0).rgb;
    return select(vec3<f32>(0.0), c, scene_color_available());
}

// Linear view depth (metres) of the fragment at `frag_coord` itself.
fn fragment_view_depth(frag_coord: vec4<f32>) -> f32 {
    return globals.camera_pos.w / max(frag_coord.z, 1e-12);
}

// A plain surface: vertex colour, roughness 0.5, dielectric, no emission.
fn default_surface(in: SurfaceInput) -> Surface {
    var s: Surface;
    s.position = in.position;
    s.normal = in.normal;
    s.base_color = in.color.rgb;
    s.alpha = in.color.a;
    s.roughness = 0.5;
    s.metallic = 0.0;
    s.emissive = vec3<f32>(0.0);
    s.occlusion = 1.0;
    return s;
}
