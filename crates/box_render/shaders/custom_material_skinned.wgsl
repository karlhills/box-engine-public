// Skinned entry points of a material shader (RS4), included after
// custom_material.wgsl: the same varyings with vertices posed by
// `skin_vertex` (skin.wgsl) before the material's `vertex` hook runs.
// Fragment entry points are custom_material.wgsl's.

@vertex
fn vs_main_skinned(in: SkinnedVertexInput) -> MaterialVaryings {
    return material_varyings(skin_vertex(in, in.instance), in.instance, globals.view_proj);
}

@vertex
fn vs_shadow_skinned(in: SkinnedVertexInput) -> MaterialVaryings {
    return material_varyings(skin_vertex(in, in.instance), in.instance, material_shadow_pass.light_view_proj);
}

@vertex
fn vs_shadow_pancake_skinned(in: SkinnedVertexInput) -> MaterialVaryings {
    var out = material_varyings(skin_vertex(in, in.instance), in.instance, material_shadow_pass.light_view_proj);
    out.clip.z = max(out.clip.z, 0.0);
    return out;
}

@vertex
fn vs_outline_skinned(in: SkinnedVertexInput) -> MaterialOutlineOutput {
    let v = material_varyings(skin_vertex(in, in.instance), in.instance, globals.view_proj);
    var out: MaterialOutlineOutput;
    out.clip = v.clip;
    out.id = u32(object.params.z);
    return out;
}
