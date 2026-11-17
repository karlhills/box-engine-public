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
    // vertex colour (linear)
    color: vec4<f32>,
    // unit vector toward the camera
    view_dir: vec3<f32>,
    front_facing: bool,
};

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
