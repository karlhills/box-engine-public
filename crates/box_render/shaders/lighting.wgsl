// Standard material lighting: Lambert diffuse + GGX specular for the primary
// directional light and the local (point and spot) lights, one of which may
// be shadowed, hemisphere ambient (replaced inside ambient volumes) with
// screen-space ambient occlusion, emissive.
// Requires common.wgsl.

// What a material shader produces and `shade_standard` lights.
struct Surface {
    // world-space position
    position: vec3<f32>,
    // unit world-space normal
    normal: vec3<f32>,
    // linear albedo
    base_color: vec3<f32>,
    // used by the alpha mode (mask, blend, additive, multiply)
    alpha: f32,
    roughness: f32,
    metallic: f32,
    // linear radiance added after lighting
    emissive: vec3<f32>,
    // ambient occlusion: scales ambient (indirect) light only
    occlusion: f32,
};

// Sky radiance seen along `dir`: ground -> horizon -> zenith by dir.y.
// Used for ambient specular.
fn hemisphere(dir: vec3<f32>) -> vec3<f32> {
    let t = dir.y * 0.5 + 0.5;
    if t < 0.5 {
        return mix(globals.sky_ground.rgb, globals.sky_horizon.rgb, t * 2.0);
    }
    return mix(globals.sky_horizon.rgb, globals.sky_zenith.rgb, (t - 0.5) * 2.0);
}

// Approximate irradiance for a surface facing `n`: each half of the sphere is
// averaged (zenith+horizon above, horizon+ground below) and blended by n.y,
// so a vertical wall sees a mix of sky, horizon and ground rather than only
// the (usually brightest) horizon colour.
fn hemisphere_irradiance(n: vec3<f32>) -> vec3<f32> {
    let sky = 0.5 * (globals.sky_zenith.rgb + globals.sky_horizon.rgb);
    let ground = 0.5 * (globals.sky_ground.rgb + globals.sky_horizon.rgb);
    return mix(ground, sky, n.y * 0.5 + 0.5);
}

fn d_ggx(n_dot_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn v_smith_ggx_correlated(n_dot_v: f32, n_dot_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn f_schlick(f0: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - v_dot_h, 5.0);
}

// Karis' analytic approximation of the split-sum environment BRDF.
fn env_brdf_approx(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// Radiance reflected toward `v` by light of colour `light_color` arriving
// from direction `l`, scaled by `attenuation` (shadowing, distance falloff).
fn direct_light(
    n: vec3<f32>,
    v: vec3<f32>,
    l: vec3<f32>,
    light_color: vec3<f32>,
    attenuation: f32,
    n_dot_v: f32,
    f0: vec3<f32>,
    diffuse_color: vec3<f32>,
    a: f32,
) -> vec3<f32> {
    let n_dot_l = saturate(dot(n, l));
    if n_dot_l <= 0.0 {
        return vec3<f32>(0.0);
    }
    let h = normalize(l + v);
    let n_dot_h = saturate(dot(n, h));
    let v_dot_h = saturate(dot(v, h));
    let f = f_schlick(f0, v_dot_h);
    let spec = d_ggx(n_dot_h, a) * v_smith_ggx_correlated(n_dot_v, n_dot_l, a) * f;
    let diffuse = diffuse_color * (vec3<f32>(1.0) - f);
    return (diffuse + spec * PI) * light_color * (n_dot_l * attenuation);
}

// Inverse-square falloff with a smooth window that reaches 0 at `range`
// (Karis, "Real Shading in Unreal Engine 4"). Distances below 1 cm are
// clamped so a surface touching the light stays finite.
fn point_light_attenuation(distance_sq: f32, range: f32) -> f32 {
    let x = distance_sq / max(range * range, 1e-8);
    let window = saturate(1.0 - x * x);
    return window * window / max(distance_sq, 1e-4);
}

// Cone factor of a spot light for the direction `l` (unit, surface to
// light): 1 inside the inner angle, 0 beyond the outer, a smooth square in
// between (`SpotLight::cone_attenuation`). 1 for point lights.
fn spot_attenuation(light: LocalLight, l: vec3<f32>) -> f32 {
    let t = saturate(dot(-l, light.direction) * light.spot_scale + light.spot_offset);
    return t * t;
}

// The cube face (0..5: +X, -X, +Y, -Y, +Z, -Z) a point light sees `d`
// (light to surface) through.
fn cube_face(d: vec3<f32>) -> u32 {
    let a = abs(d);
    if a.x >= a.y && a.x >= a.z {
        return select(1u, 0u, d.x > 0.0);
    }
    if a.y >= a.z {
        return select(3u, 2u, d.y > 0.0);
    }
    return select(5u, 4u, d.z > 0.0);
}

// Shadow of the frame's shadowed local light at `position` (0 shadowed ..
// 1 lit): normal-offset and depth bias in shadow texels at this distance,
// then 3x3 bilinear PCF. `l` points to the light, `distance` is to it.
fn local_shadow(light: LocalLight, position: vec3<f32>, n: vec3<f32>, l: vec3<f32>, distance: f32) -> f32 {
    let kind = u32(lights.shadow.x + 0.5);
    if kind == 0u {
        return 1.0;
    }
    var face = 0u;
    if kind == 2u {
        face = cube_face(position - light.position);
    }
    let texel = distance * lights.shadow.w;
    let n_dot_l = dot(n, l);
    let p = position + n * (lights.shadow.y * texel * (1.0 - 0.5 * saturate(n_dot_l))) + l * (lights.shadow.z * texel);
    let clip = lights.shadow_view_proj[face] * vec4<f32>(p, 1.0);
    if clip.w <= 1e-6 {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z > 1.0 {
        return 1.0;
    }
    let step = 1.0 / vec2<f32>(textureDimensions(local_shadow_map));
    var sum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let o = vec2<f32>(f32(x), f32(y)) * step;
            sum += textureSampleCompareLevel(local_shadow_map, shadow_sampler, uv + o, i32(face), ndc.z);
        }
    }
    return sum / 9.0;
}

// Light arriving from local light `light` at `position` (falloff, cone and
// shadow), before the cosine term. `l` is set to the unit direction to the
// light. 0 beyond the light's range.
fn local_light_attenuation(light: LocalLight, position: vec3<f32>, n: vec3<f32>, l: ptr<function, vec3<f32>>) -> f32 {
    let to_light = light.position - position;
    let distance_sq = dot(to_light, to_light);
    if distance_sq >= light.range * light.range {
        return 0.0;
    }
    let inv = inverseSqrt(max(distance_sq, 1e-12));
    *l = to_light * inv;
    var att = point_light_attenuation(distance_sq, light.range) * spot_attenuation(light, *l);
    if light.shadow > 0.5 && att > 0.0 && dot(n, *l) > 0.0 {
        att *= local_shadow(light, position, n, *l, distance_sq * inv);
    }
    return att;
}

// Ambient volumes at `position`: rgb = ambient light they add, a = the
// fraction of the sky ambient that remains. (0, 0, 0, 1) outside every
// volume. Each blends in over its blend distance outside its box.
fn ambient_override(position: vec3<f32>) -> vec4<f32> {
    var sky = 1.0;
    var extra = vec3<f32>(0.0);
    let count = min(lights.count.y, 8u);
    for (var i = 0u; i < count; i++) {
        let v = lights.volumes[i];
        let q = (v.world_to_local * vec4<f32>(position, 1.0)).xyz;
        let d = abs(q) - v.half_extents.xyz;
        let outside = length(max(d, vec3<f32>(0.0))) + min(max(d.x, max(d.y, d.z)), 0.0);
        let w = 1.0 - smoothstep(0.0, max(v.half_extents.w, 1e-4), outside);
        sky = mix(sky, v.ambient.w, w);
        extra = mix(extra, v.ambient.rgb, w);
    }
    return vec4<f32>(extra, sky);
}

// Irradiance at `position` for a surface without a normal (particles):
// ambient (with volumes) plus half of the sun and the local lights, as if
// lit from all sides. Unshadowed.
fn unoriented_irradiance(position: vec3<f32>) -> vec3<f32> {
    let amb = ambient_override(position);
    var e = hemisphere_irradiance(vec3<f32>(0.0)) * (globals.ambient.x * amb.w) + amb.rgb;
    if has_sun() {
        e += globals.sun_color.rgb * 0.5;
    }
    let count = min(lights.count.x, arrayLength(&lights.items));
    for (var i = 0u; i < count; i++) {
        let light = lights.items[i];
        let to_light = light.position - position;
        let distance_sq = dot(to_light, to_light);
        if distance_sq >= light.range * light.range {
            continue;
        }
        let l = to_light * inverseSqrt(max(distance_sq, 1e-12));
        e += light.color * (0.5 * point_light_attenuation(distance_sq, light.range) * spot_attenuation(light, l));
    }
    return e;
}

// Ambient occlusion at world position `position` (1 = unoccluded): the
// half-resolution occlusion upsampled for this fragment. Of the four
// nearest half-resolution texels, those whose depth matches the fragment's
// are used (bilinear when all match, else the closest in depth), so
// foreground and background fragments of the same pixel each get their own
// surface's occlusion at silhouettes, with MSAA too. A fragment no texel
// matches (transparent surfaces, sub-texel geometry) is unoccluded.
fn screen_ao(position: vec3<f32>) -> f32 {
    if globals.ao.x < 0.5 {
        return 1.0;
    }
    let clip = globals.view_proj * vec4<f32>(position, 1.0);
    if clip.w <= 1e-6 {
        return 1.0;
    }
    let uv = clip.xy / clip.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let size = vec2<i32>(textureDimensions(ao_texture));
    let depth = dot(position - globals.camera_pos.xyz, globals.camera_forward.xyz);
    let st = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(st));
    let f = st - floor(st);
    var bilinear = 0.0;
    var nearest = 1.0;
    var nearest_diff = 1e30;
    var all_match = true;
    for (var i = 0; i < 4; i++) {
        let o = vec2<i32>(i & 1, i >> 1u);
        let q = clamp(base + o, vec2<i32>(0), size - vec2<i32>(1));
        let d = textureLoad(ao_depth_texture, q, 0).r;
        let a = textureLoad(ao_texture, q, 0).r;
        let w = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        bilinear += a * w;
        let diff = abs(d - depth) / max(depth, 1e-4);
        if diff < nearest_diff {
            nearest_diff = diff;
            nearest = a;
        }
        all_match = all_match && diff < AO_MATCH;
    }
    if all_match {
        return bilinear;
    }
    return select(1.0, nearest, nearest_diff < AO_REJECT);
}

// Relative depth difference below which a half-resolution AO texel belongs
// to the fragment's surface, and above which no texel does.
const AO_MATCH: f32 = 0.03;
const AO_REJECT: f32 = 0.15;

// Outgoing radiance toward the viewer. `shadow` is 0 (fully shadowed) .. 1 (lit)
// and applies to the directional light only (the shadowed local light, if
// any, samples its own map).
// Light intensity is scaled so a white Lambert surface facing a light of
// intensity 1 reflects radiance 1; a point light of intensity I gives that
// irradiance at I / distance^2.
fn shade_standard(s: Surface, v: vec3<f32>, shadow: f32) -> vec3<f32> {
    let n = s.normal;
    let roughness = clamp(s.roughness, 0.045, 1.0);
    let a = roughness * roughness;
    let f0 = mix(vec3<f32>(0.04), s.base_color, s.metallic);
    let diffuse_color = s.base_color * (1.0 - s.metallic);
    let n_dot_v = max(dot(n, v), 1e-4);
    let ao = screen_ao(s.position);
    if globals.ao.y > 0.5 {
        return vec3<f32>(ao);
    }
    let direct_ao = mix(1.0, ao, globals.ao.z);

    var color = vec3<f32>(0.0);
    if has_sun() {
        color += direct_light(n, v, globals.sun_dir.xyz, globals.sun_color.rgb, shadow * direct_ao, n_dot_v, f0, diffuse_color, a);
    }
    let count = min(lights.count.x, arrayLength(&lights.items));
    for (var i = 0u; i < count; i++) {
        let light = lights.items[i];
        var l = vec3<f32>(0.0);
        let att = local_light_attenuation(light, s.position, n, &l);
        if att <= 0.0 {
            continue;
        }
        color += direct_light(n, v, l, light.color, att * direct_ao, n_dot_v, f0, diffuse_color, a);
    }

    // Ambient volumes replace (part of) the sky ambient; occlusion darkens
    // both alike.
    let amb = ambient_override(s.position);
    let ambient = globals.ambient.x * amb.w * s.occlusion * ao;
    let occluded = s.occlusion * ao;
    let env = env_brdf_approx(f0, roughness, n_dot_v);
    color += diffuse_color * hemisphere_irradiance(n) * ambient + diffuse_color * amb.rgb * occluded;
    let r = reflect(-v, n);
    color += hemisphere(r) * env * ambient + amb.rgb * env * occluded;
    color += s.emissive;
    return color;
}
