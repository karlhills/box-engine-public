// Cascaded shadow map sampling: cascade selection, normal-offset + depth bias,
// 3x3 PCF with a linear comparison sampler, blending near cascade boundaries.
// Requires common.wgsl.

fn shadow_cascade_index(view_depth: f32) -> u32 {
    for (var i = 0u; i < CASCADE_COUNT; i++) {
        if view_depth < globals.cascade_splits[i] {
            return i;
        }
    }
    return CASCADE_COUNT;
}

// 0 = fully shadowed, 1 = lit, -1 = outside the cascade's shadow map.
fn sample_cascade(cascade: u32, world_pos: vec3<f32>, normal: vec3<f32>) -> f32 {
    let n_dot_l = dot(normal, globals.sun_dir.xyz);
    // Push the lookup off the surface along the normal, more at grazing angles.
    let offset_scale = globals.shadow.y * globals.cascade_texel[cascade] * (1.0 - 0.5 * saturate(n_dot_l));
    let p = world_pos + normal * offset_scale;
    let clip = globals.cascade_view_proj[cascade] * vec4<f32>(p, 1.0);
    let ndc = clip.xyz / clip.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z > 1.0 {
        return -1.0;
    }
    let depth = ndc.z - globals.cascade_bias[cascade];
    let texel = 1.0 / vec2<f32>(textureDimensions(shadow_map));
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * texel;
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + o, i32(cascade), depth);
        }
    }
    return sum / 9.0;
}

// 0 = fully shadowed, 1 = lit.
fn sample_shadow(world_pos: vec3<f32>, normal: vec3<f32>, view_depth: f32) -> f32 {
    let cascade = shadow_cascade_index(view_depth);
    if cascade >= CASCADE_COUNT {
        return 1.0;
    }
    var s = sample_cascade(cascade, world_pos, normal);
    if s < 0.0 {
        s = 1.0;
    }
    let far = globals.cascade_splits[cascade];
    var near = globals.camera_pos.w;
    if cascade > 0u {
        near = globals.cascade_splits[cascade - 1u];
    }
    let band = (far - near) * globals.shadow.z;
    let t = smoothstep(far - band, far, view_depth);
    if t > 0.0 {
        if cascade + 1u < CASCADE_COUNT {
            // The next cascade is fitted to its own slice and may not reach
            // back into this band (narrow fields of view): keep this
            // cascade's value there instead of blending toward lit.
            let next = sample_cascade(cascade + 1u, world_pos, normal);
            if next >= 0.0 {
                s = mix(s, next, t);
            }
        } else {
            s = mix(s, 1.0, t);
        }
    }
    return s;
}

fn cascade_debug_tint(view_depth: f32) -> vec3<f32> {
    switch shadow_cascade_index(view_depth) {
        case 0u: { return vec3<f32>(1.0, 0.35, 0.35); }
        case 1u: { return vec3<f32>(0.35, 1.0, 0.35); }
        case 2u: { return vec3<f32>(0.35, 0.5, 1.0); }
        case 3u: { return vec3<f32>(1.0, 1.0, 0.35); }
        default: { return vec3<f32>(1.0); }
    }
}
