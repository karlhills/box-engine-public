// Sky gradient, sun glow, stars, analytic exponential height fog and the
// atmosphere (aerial haze, height mist, distance quieting). The sky and the
// fog share `fog_color` so distant geometry fades into the horizon
// seamlessly; the sky counts as distant scenery up to the haze's sky height.
// Requires common.wgsl.

// Where the sky is treated as being for the height mist: far enough that a
// ray dipping below the horizon reaches a band below the camera.
const SKY_DISTANCE: f32 = 10000.0;

// Hue of the sun at roughly unit brightness.
fn sun_tint() -> vec3<f32> {
    let c = globals.sun_color.rgb;
    return c / max(max(c.r, max(c.g, c.b)), 1e-4);
}

// 0..1 amount of sun-coloured scattering when looking toward the sun.
fn sun_glow(rd: vec3<f32>) -> f32 {
    if !has_sun() {
        return 0.0;
    }
    let c = saturate(dot(rd, globals.sun_dir.xyz));
    // Fade the glow as the sun drops below the horizon.
    let above = smoothstep(-0.25, 0.05, globals.sun_dir.y);
    return (0.45 * pow(c, 6.0) + 0.35 * pow(c, 48.0)) * above * globals.sky_sun.x;
}

fn tint_toward_sun(base: vec3<f32>, amount: f32) -> vec3<f32> {
    let target_color = sun_tint() * max(luminance(base), 1e-4) * 1.8;
    return mix(base, target_color, saturate(amount));
}

fn fog_color(rd: vec3<f32>) -> vec3<f32> {
    return tint_toward_sun(globals.sky_horizon.rgb, sun_glow(rd));
}

// Ground -> horizon -> zenith (simple), or below-horizon haze -> horizon ->
// upper sky -> zenith (layered).
fn sky_gradient(rd: vec3<f32>) -> vec3<f32> {
    let y = rd.y;
    if globals.sky_upper.w > 0.5 {
        var c = mix(globals.sky_horizon.rgb, globals.sky_upper.rgb, smoothstep(0.0, 0.28, y));
        c = mix(c, globals.sky_zenith.rgb, smoothstep(0.22, 1.0, y));
        return mix(c, globals.sky_below.rgb, smoothstep(0.0, -0.12, y));
    }
    if y >= 0.0 {
        return mix(globals.sky_horizon.rgb, globals.sky_zenith.rgb, pow(y, 0.45));
    }
    // Below the horizon, fade slowly so it matches fogged distant ground.
    return mix(globals.sky_horizon.rgb, globals.sky_ground.rgb, smoothstep(0.0, 0.4, -y));
}

fn star_hash(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3<f32>(443.897, 441.423, 437.195));
    q += dot(q, q.yzx + 19.19);
    return fract((q.x + q.y) * q.z);
}

// A fixed star field: a few of the cells of a grid around the camera hold
// a star wholly inside them, twinkling slowly. Scaled by `globals.sky_sun.w`.
fn stars(rd: vec3<f32>) -> vec3<f32> {
    let strength = globals.sky_sun.w;
    if strength <= 0.0 || rd.y <= 0.0 {
        return vec3<f32>(0.0);
    }
    let scale = 150.0;
    let q = rd * scale;
    let cell = floor(q);
    let r = star_hash(cell);
    if r <= 0.993 {
        return vec3<f32>(0.0);
    }
    let jitter = vec3<f32>(star_hash(cell + 1.3), star_hash(cell + 2.7), star_hash(cell + 4.1)) - 0.5;
    let on_sky = normalize(cell + 0.5 + jitter * 0.3) * scale;
    // Only stars whose disc lies inside this cell, so none is clipped.
    let f = on_sky - cell;
    if any(f < vec3<f32>(0.33)) || any(f > vec3<f32>(0.67)) {
        return vec3<f32>(0.0);
    }
    let d = length(q - on_sky);
    let twinkle = 0.7 + 0.3 * sin(globals.time.x * (1.0 + r * 3.0) + r * 60.0);
    let b = saturate(1.0 - d * 3.4) * twinkle * (0.5 + 3.0 * (r - 0.993) / 0.007);
    return vec3<f32>(0.9, 0.93, 1.0) * (b * strength * smoothstep(0.02, 0.25, rd.y));
}

fn sky_radiance(rd: vec3<f32>) -> vec3<f32> {
    var color = tint_toward_sun(sky_gradient(rd), sun_glow(rd));
    if has_sun() {
        let l = globals.sun_dir.xyz;
        let c = dot(rd, l);
        let r = max(globals.sun_color.w, 1e-4);
        let disc = smoothstep(cos(r * 1.3), cos(r * 0.8), c) * smoothstep(-0.02, 0.01, rd.y);
        let halo = pow(saturate(c), 600.0) * 0.6;
        color += globals.sun_color.rgb * (disc * 5.0 * globals.sky_sun.y + halo * globals.sky_sun.x);
        if globals.sky_sun.z > 0.0 {
            // Warm band along the horizon, strongest below the sun.
            let flat_rd = normalize(vec3<f32>(rd.x, 0.0, rd.z) + vec3<f32>(1e-4, 0.0, 0.0));
            let flat_sun = normalize(vec3<f32>(l.x, 0.0, l.z) + vec3<f32>(1e-4, 0.0, 0.0));
            let az = saturate(dot(flat_rd, flat_sun) * 0.5 + 0.5);
            let band = exp(-abs(rd.y) * 9.0);
            let lum = max(luminance(globals.sky_horizon.rgb), 1e-4);
            color += sun_tint() * lum * globals.sky_sun.z * band * (0.33 + 0.67 * az * az);
        }
    }
    return color + stars(rd);
}

// Integrated exponential height fog along the segment ro -> p (Quilez).
fn fog_transmittance(ro: vec3<f32>, p: vec3<f32>) -> f32 {
    let a = globals.fog.x;
    if a <= 0.0 {
        return 1.0;
    }
    let b = max(globals.fog.y, 1e-5);
    let d = p - ro;
    let t = length(d);
    let rd = d / max(t, 1e-5);
    let e0 = min(-(ro.y - globals.fog.z) * b, 80.0);
    let start = a * exp(e0);
    let k = rd.y * b * t;
    var amount: f32;
    if abs(k) < 1e-3 {
        // Limit for a horizontal ray: constant density along the segment.
        amount = start * t * (1.0 - 0.5 * k);
    } else if k < -80.0 {
        // A long downward ray: exp(-k) overflows while `start` underflows.
        // Here 1 - exp(-k) is -exp(-k), so the exponents can be added first.
        amount = -a * exp(min(e0 - k, 80.0)) / (rd.y * b);
    } else {
        amount = start * (1.0 - exp(-k)) / (rd.y * b);
    }
    return exp(-max(amount, 0.0));
}

// Aerial haze opacity at `dist` metres from the camera.
fn haze_amount(dist: f32) -> f32 {
    let d = max(dist - globals.haze.z, 0.0);
    return (1.0 - exp(-globals.haze.x * d)) * globals.haze.y;
}

// The haze colour seen along `rd`, brighter toward the sun.
fn haze_color(rd: vec3<f32>) -> vec3<f32> {
    var c = globals.haze_color.rgb;
    if has_sun() {
        c += sun_tint() * (pow(saturate(dot(rd, globals.sun_dir.xyz)), 6.0) * globals.haze.w);
    }
    return c;
}

// Height mist opacity of a surface at `p`, `dist` metres from the camera.
fn mist_amount(p: vec3<f32>, dist: f32) -> f32 {
    let m = globals.mist;
    let h = saturate((m.x - p.y) / max(m.x - m.y, 1e-3));
    return h * h * (3.0 - 2.0 * h) * m.z * saturate(dist / max(m.w, 1e-3));
}

// Distance quieting of `color`: weight `q` (0..1) of the full effect.
fn quiet(color: vec3<f32>, q: f32) -> vec3<f32> {
    let grey = vec3<f32>(luminance(color));
    var c = mix(color, grey, globals.quiet.z * q);
    return mix(c, globals.quiet_tone.rgb, globals.quiet.w * q);
}

// Quieting, height mist and aerial haze of a surface at `p` (in that
// order: the haze covers the mist, which covers the surface).
fn atmosphere(color: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let to_p = p - globals.camera_pos.xyz;
    let dist = length(to_p);
    var c = color;
    if globals.quiet_tone.w > 0.5 {
        c = quiet(c, smoothstep(globals.quiet.x, globals.quiet.y, dist));
    }
    if globals.mist.z > 0.0 {
        c = mix(c, globals.mist_color.rgb, mist_amount(p, dist));
    }
    if globals.haze.y > 0.0 {
        let rd = to_p / max(dist, 1e-3);
        c = mix(c, haze_color(rd), haze_amount(dist));
    }
    return c;
}

// How much of a surface at `p` the mist and haze let through (additive and
// multiply surfaces fade out by it).
fn atmosphere_transmittance(p: vec3<f32>) -> f32 {
    let to_p = p - globals.camera_pos.xyz;
    let dist = length(to_p);
    var t = 1.0;
    if globals.mist.z > 0.0 {
        t *= 1.0 - mist_amount(p, dist);
    }
    if globals.haze.y > 0.0 {
        t *= 1.0 - haze_amount(dist);
    }
    return t;
}

// The atmosphere over the sky along `rd`: below `sky height` the sky counts
// as infinitely far scenery (full haze and quieting); the mist is evaluated
// SKY_DISTANCE away, so it covers the sky below the horizon when its band
// lies below the camera.
fn sky_atmosphere(color: vec3<f32>, rd: vec3<f32>) -> vec3<f32> {
    var far = 0.0;
    if globals.haze_color.w > 0.0 {
        far = 1.0 - smoothstep(0.0, globals.haze_color.w, rd.y);
    }
    var c = color;
    if globals.quiet_tone.w > 0.5 {
        c = quiet(c, far);
    }
    if globals.mist.z > 0.0 {
        let p = globals.camera_pos.xyz + rd * SKY_DISTANCE;
        c = mix(c, globals.mist_color.rgb, mist_amount(p, SKY_DISTANCE));
    }
    if globals.haze.y > 0.0 {
        c = mix(c, haze_color(rd), globals.haze.y * far);
    }
    return c;
}

// Fog and atmosphere of a surface at `p` seen from `ro`.
fn apply_fog(color: vec3<f32>, ro: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let tr = fog_transmittance(ro, p);
    let rd = normalize(p - ro);
    return atmosphere(mix(fog_color(rd), color, tr), p);
}
