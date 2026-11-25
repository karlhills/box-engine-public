// GPU skinning (RS4), shared by every pass of skinned meshes: the
// `*_skinned` vertex entry points of mesh_skinned.wgsl (main, prepass,
// outline), shadow_skinned.wgsl, error_skinned.wgsl and material shaders
// (custom_material_skinned.wgsl) all start with `skin_vertex`, so every
// pass places the vertices exactly alike.
//
// `joint_matrices` holds one palette per drawn skinned object, back to back
// (src/skinning.rs); the object's palette starts at `object.params.x`
// (bit-cast u32). A palette matrix takes a bind-space vertex to the posed
// position in the object's model space (mesh⁻¹ × joint × inverse bind), so
// the object's model matrix still places it in the world. Skinned objects
// with the same mesh, material and pipeline are one instanced draw: each
// instance reads its own palette.
// Requires common.wgsl and object.wgsl.

@group(1) @binding(2) var<storage, read> joint_matrices: array<mat4x4<f32>>;

struct SkinnedVertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) tangent: vec4<f32>,
    @location(5) uv1: vec2<f32>,
    @location(6) custom0: vec4<f32>,
    @location(7) custom1: vec4<f32>,
    // Second vertex buffer (src/skinning.rs `GpuSkinVertex`).
    @location(8) joints: vec4<u32>,
    @location(9) weights: vec4<f32>,
    @builtin(instance_index) instance: u32,
};

// The weighted palette matrix of a vertex of the current `object`.
fn skin_matrix(joints: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let base = bitcast<u32>(object.params.x);
    return joint_matrices[base + joints.x] * weights.x
        + joint_matrices[base + joints.y] * weights.y
        + joint_matrices[base + joints.z] * weights.z
        + joint_matrices[base + joints.w] * weights.w;
}

// Makes the drawn instance the current `object` and returns the vertex
// posed by its joints, in model space, as the plain `VertexInput` the
// passes' helpers take. Normals and tangents use the matrix's linear part
// (joints are rigid or uniformly scaled). `instance` is the entry point's
// `in.instance` (see object.wgsl): `skin_vertex(in, in.instance)`.
fn skin_vertex(in: SkinnedVertexInput, instance: u32) -> VertexInput {
    load_object(instance);
    let m = skin_matrix(in.joints, in.weights);
    var out: VertexInput;
    out.position = (m * vec4<f32>(in.position, 1.0)).xyz;
    out.normal = (m * vec4<f32>(in.normal, 0.0)).xyz;
    out.tangent = vec4<f32>((m * vec4<f32>(in.tangent.xyz, 0.0)).xyz, in.tangent.w);
    out.uv = in.uv;
    out.color = in.color;
    out.uv1 = in.uv1;
    out.custom0 = in.custom0;
    out.custom1 = in.custom1;
    out.instance = instance;
    return out;
}
