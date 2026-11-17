// Skinned shadow casters (RS4): shadow_depth.wgsl with vertices posed by
// `skin_vertex` (skin.wgsl). `fs_mask` is shadow_depth.wgsl's.

#include "shadow_depth.wgsl"
#include "skin.wgsl"

fn skinned_shadow(in: SkinnedVertexInput, instance: u32) -> ShadowOutput {
    let v = skin_vertex(in, instance);
    return shadow_vertex(v, shadow_pass.light_view_proj * object.model * vec4<f32>(v.position, 1.0));
}

@vertex
fn vs_skinned(in: SkinnedVertexInput) -> ShadowOutput {
    return skinned_shadow(in, in.instance);
}

@vertex
fn vs_skinned_pancake(in: SkinnedVertexInput) -> ShadowOutput {
    return pancake(skinned_shadow(in, in.instance));
}
