// Skinned StandardMaterial meshes (RS4): mesh.wgsl's passes with vertices
// posed by `skin_vertex` (skin.wgsl). Fragment entry points are mesh.wgsl's.

#include "mesh.wgsl"
#include "skin.wgsl"

@vertex
fn vs_main_skinned(in: SkinnedVertexInput) -> VertexOutput {
    let v = skin_vertex(in, in.instance);
    return main_vertex(v, object.model * vec4<f32>(v.position, 1.0));
}

@vertex
fn vs_prepass_skinned(in: SkinnedVertexInput) -> PrepassOutput {
    let v = skin_vertex(in, in.instance);
    return prepass_vertex(v, object.model * vec4<f32>(v.position, 1.0));
}

@vertex
fn vs_outline_skinned(in: SkinnedVertexInput) -> OutlineOutput {
    let v = skin_vertex(in, in.instance);
    var out: OutlineOutput;
    out.clip = globals.view_proj * object.model * vec4<f32>(v.position, 1.0);
    out.id = u32(object.params.z);
    return out;
}
