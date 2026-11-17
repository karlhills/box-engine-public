// The error material on skinned meshes (RS4): error.wgsl with vertices
// posed by `skin_vertex` (skin.wgsl).

#include "error.wgsl"
#include "skin.wgsl"

@vertex
fn vs_main_skinned(in: SkinnedVertexInput) -> ErrorOutput {
    let v = skin_vertex(in, in.instance);
    let world = object.model * vec4<f32>(v.position, 1.0);
    var out: ErrorOutput;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    return out;
}
