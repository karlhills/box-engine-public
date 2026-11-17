// Per-object instance data and the mesh vertex layout.
// Must match `ObjectUniform` and `GpuVertex` in src/uniforms.rs.
//
// Objects are drawn instanced: every drawn entity has a persistent slot in
// `objects`, and a draw of `count` instances starting at `first` draws the
// slots `draw_slots[first .. first + count]`. Vertex entry points call
// `load_object(in.instance)` first; `object` is then the drawn object for
// the rest of the vertex stage (it is not available in fragment shaders).
//
// Read `in.instance` only in the `@vertex` function itself and pass it on
// as a `u32` argument: naga's HLSL backend (DX12) adds the draw's first
// instance to `instance_index` only where an entry point reads its own
// input, so a helper reading `in.instance` gets the raw SV_InstanceID and
// loads another object's data.

struct Object {
    model: mat4x4<f32>,
    normal_matrix: mat4x4<f32>,
    // z = selection outline id, w = 1 or -1 (mirroring transform)
    params: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> objects: array<Object>;
@group(1) @binding(1) var<storage, read> draw_slots: array<u32>;

var<private> object: Object;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    // xyz tangent, w bitangent sign; zero when the mesh has no tangents
    @location(4) tangent: vec4<f32>,
    // UV set 1 (zero when the mesh has none)
    @location(5) uv1: vec2<f32>,
    // index into `draw_slots` (includes the draw's first instance)
    @builtin(instance_index) instance: u32,
};

// Makes the drawn instance the current `object`.
fn load_object(instance: u32) {
    object = objects[draw_slots[instance]];
}

// World-space tangent with the bitangent sign corrected for mirroring.
fn world_tangent(t: vec4<f32>) -> vec4<f32> {
    return vec4<f32>((object.model * vec4<f32>(t.xyz, 0.0)).xyz, t.w * object.params.w);
}
