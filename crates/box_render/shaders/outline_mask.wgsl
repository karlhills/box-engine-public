// Selection outline, step 1: selected meshes into a single-sample mask.
// R holds the object's outline id from `object.params.z`: (object slot + 1)
// * 2, plus 1 for the primary selection, so every selected object is its own
// silhouette. The depth attachment keeps the nearest selected surface, which
// the composite uses to tell which of two touching objects is in front, and
// compares with the scene depth to find hidden silhouettes.

#include "common.wgsl"
#include "object.wgsl"

struct MaskOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
};

@vertex
fn vs_main(in: VertexInput) -> MaskOutput {
    load_object(in.instance);
    // Same expression as mesh.wgsl, so the depths match the scene pass.
    let world = object.model * vec4<f32>(in.position, 1.0);
    var out: MaskOutput;
    out.clip = globals.view_proj * world;
    out.id = u32(object.params.z);
    return out;
}

@fragment
fn fs_main(in: MaskOutput) -> @location(0) vec4<u32> {
    return vec4<u32>(in.id, 0u, 0u, 0u);
}
