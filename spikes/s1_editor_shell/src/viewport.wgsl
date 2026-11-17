// Stand-in viewport: a ray-marched stylized block scene on a grid.
// Output is gamma-encoded Rgba8Unorm, which is what egui-wgpu expects from native textures.

struct U {
    res: vec2<f32>,
    time: f32,
    pixel_angle: f32,
    cam_pos: vec4<f32>,
    cam_fwd: vec4<f32>,
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    accent: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

const NB: i32 = 9;
const SEL: i32 = 6;

// (center x, center z, half width, height)
var<private> B: array<vec4<f32>, 9> = array<vec4<f32>, 9>(
    vec4<f32>(-3.2, -1.0, 1.1, 2.6),
    vec4<f32>(-0.9, -1.4, 0.9, 3.4),
    vec4<f32>(1.3, -0.8, 1.0, 2.2),
    vec4<f32>(3.6, -1.6, 1.2, 4.2),
    vec4<f32>(-2.2, 2.4, 0.8, 1.6),
    vec4<f32>(0.4, 2.0, 1.2, 1.4),
    vec4<f32>(3.0, 2.6, 0.9, 2.0),
    vec4<f32>(-5.6, 1.2, 0.9, 1.8),
    vec4<f32>(5.8, 0.8, 0.8, 2.8),
);
// (half depth, gable roof 0/1, wall colour index, roof colour index)
var<private> D: array<vec4<f32>, 9> = array<vec4<f32>, 9>(
    vec4<f32>(1.0, 1.0, 0.0, 0.0),
    vec4<f32>(0.9, 0.0, 1.0, 0.0),
    vec4<f32>(1.0, 1.0, 2.0, 1.0),
    vec4<f32>(1.0, 0.0, 3.0, 0.0),
    vec4<f32>(0.8, 1.0, 2.0, 0.0),
    vec4<f32>(0.9, 1.0, 0.0, 1.0),
    vec4<f32>(0.9, 0.0, 1.0, 0.0),
    vec4<f32>(0.9, 1.0, 3.0, 1.0),
    vec4<f32>(0.8, 1.0, 0.0, 0.0),
);

fn wall_col(i: i32) -> vec3<f32> {
    switch i {
        case 0: { return vec3<f32>(0.74, 0.60, 0.42); }
        case 1: { return vec3<f32>(0.52, 0.24, 0.17); }
        case 2: { return vec3<f32>(0.46, 0.56, 0.62); }
        default: { return vec3<f32>(0.44, 0.54, 0.38); }
    }
}
fn roof_col(i: i32) -> vec3<f32> {
    if i == 0 { return vec3<f32>(0.20, 0.23, 0.28); }
    return vec3<f32>(0.56, 0.24, 0.15);
}

fn sd_box(p: vec3<f32>, b: vec3<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec3<f32>(r);
    return length(max(q, vec3<f32>(0.0))) + min(max(q.x, max(q.y, q.z)), 0.0) - r;
}

// Gable roof: triangular prism along z, base width 2*w, height h, depth 2*d.
fn sd_roof(p: vec3<f32>, w: f32, h: f32, d: f32) -> f32 {
    let q = vec2<f32>(abs(p.x), p.y);
    let n = normalize(vec2<f32>(h, w));
    let slope = dot(q - vec2<f32>(0.0, h), n);
    let tri = max(slope, -p.y);
    return max(tri, abs(p.z) - d);
}

fn sd_building(p: vec3<f32>, i: i32) -> f32 {
    let b = B[i];
    let dd = D[i];
    let lp = p - vec3<f32>(b.x, 0.0, b.y);
    var d = sd_box(lp - vec3<f32>(0.0, b.w * 0.5, 0.0), vec3<f32>(b.z, b.w * 0.5, dd.x), 0.03);
    if dd.y > 0.5 {
        d = min(d, sd_roof(lp - vec3<f32>(0.0, b.w, 0.0), b.z + 0.12, b.z * 0.75, dd.x + 0.12));
    } else {
        // flat roof parapet
        d = min(d, sd_box(lp - vec3<f32>(0.0, b.w + 0.06, 0.0), vec3<f32>(b.z + 0.05, 0.06, dd.x + 0.05), 0.01));
    }
    return d;
}

fn sd_tree(p: vec3<f32>, c: vec3<f32>, r: f32) -> f32 {
    let lp = p - c;
    let crown = length(lp - vec3<f32>(0.0, r * 1.6, 0.0)) - r;
    let trunk = sd_box(lp - vec3<f32>(0.0, r * 0.5, 0.0), vec3<f32>(0.06, r * 0.5, 0.06), 0.02);
    return min(crown, trunk);
}

// returns (distance, material). material: 0 ground, 1..=NB building (i+1), 50 tree
fn map(p: vec3<f32>) -> vec2<f32> {
    var res = vec2<f32>(p.y, 0.0);
    for (var i = 0; i < NB; i++) {
        let d = sd_building(p, i);
        if d < res.x { res = vec2<f32>(d, f32(i + 1)); }
    }
    let t1 = sd_tree(p, vec3<f32>(-1.0, 0.0, 4.6), 0.55);
    let t2 = sd_tree(p, vec3<f32>(1.9, 0.0, 4.9), 0.45);
    let t3 = sd_tree(p, vec3<f32>(-4.4, 0.0, 3.9), 0.5);
    let t = min(t1, min(t2, t3));
    if t < res.x { res = vec2<f32>(t, 50.0); }
    return res;
}

fn normal(p: vec3<f32>) -> vec3<f32> {
    let e = vec2<f32>(0.0015, 0.0);
    return normalize(vec3<f32>(
        map(p + e.xyy).x - map(p - e.xyy).x,
        map(p + e.yxy).x - map(p - e.yxy).x,
        map(p + e.yyx).x - map(p - e.yyx).x,
    ));
}

fn soft_shadow(ro: vec3<f32>, rd: vec3<f32>) -> f32 {
    var res = 1.0;
    var t = 0.02;
    for (var i = 0; i < 32; i++) {
        let h = map(ro + rd * t).x;
        res = min(res, 10.0 * h / t);
        t += clamp(h, 0.03, 0.5);
        if res < 0.002 || t > 20.0 { break; }
    }
    return clamp(res, 0.0, 1.0);
}

fn ao(p: vec3<f32>, n: vec3<f32>) -> f32 {
    var occ = 0.0;
    var sca = 1.0;
    for (var i = 1; i <= 5; i++) {
        let h = 0.04 + 0.12 * f32(i);
        occ += (h - map(p + n * h).x) * sca;
        sca *= 0.8;
    }
    return clamp(1.0 - 1.6 * occ, 0.0, 1.0);
}

fn grid_line(coord: vec2<f32>, spacing: f32, footprint: f32, width: f32) -> f32 {
    let g = coord / spacing;
    let fw = max(footprint / spacing, 1e-4);
    let l = abs(fract(g - 0.5) - 0.5) / fw;
    let line = 1.0 - clamp(min(l.x, l.y) - width * 0.5 + 0.5, 0.0, 1.0);
    // fade lines out when they get denser than a pixel
    return line * clamp(1.0 - (fw - 0.15) * 3.0, 0.0, 1.0);
}

fn sky(rd: vec3<f32>) -> vec3<f32> {
    let h = clamp(rd.y, 0.0, 1.0);
    return mix(vec3<f32>(0.50, 0.55, 0.60), vec3<f32>(0.24, 0.34, 0.50), pow(h, 0.5));
}

fn tonemap(c: vec3<f32>) -> vec3<f32> {
    // ACES fitted (Narkowicz)
    let a = c * (2.51 * c + 0.03);
    let b = c * (2.43 * c + 0.59) + 0.14;
    return clamp(a / b, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = (frag.xy - 0.5 * u.res) * u.pixel_angle;
    let rd = normalize(u.cam_fwd.xyz + uv.x * u.cam_right.xyz - uv.y * u.cam_up.xyz);
    let ro = u.cam_pos.xyz;

    var t = 0.0;
    var m = -1.0;
    var sel_min = 1e9;
    var sel_t = 0.0;
    for (var i = 0; i < 128; i++) {
        let p = ro + rd * t;
        let r = map(p);
        let ds = sd_building(p, SEL);
        if ds < sel_min { sel_min = ds; sel_t = t; }
        if r.x < 0.0007 * t { m = r.y; break; }
        t += r.x;
        if t > 60.0 { break; }
    }

    let sun = normalize(vec3<f32>(0.55, 0.75, 0.35));
    var col = sky(rd);
    if m >= 0.0 {
        let p = ro + rd * t;
        let n = normal(p);
        var albedo = vec3<f32>(0.30, 0.30, 0.29);
        if m > 0.5 && m < 49.0 {
            let i = i32(m) - 1;
            let b = B[i];
            if p.y > b.w - 0.01 && D[i].y > 0.5 {
                albedo = roof_col(i32(D[i].w));
            } else {
                albedo = wall_col(i32(D[i].z));
                // window rows: darker inset bands on walls
                let lp = p - vec3<f32>(b.x, 0.0, b.y);
                let along = select(lp.x, lp.z, abs(n.x) > 0.5);
                let wy = fract(p.y / 0.8);
                let wx = fract(along / 0.6);
                if p.y > 0.5 && p.y < b.w - 0.25 && wy > 0.35 && wy < 0.75 && wx > 0.3 && wx < 0.7 && abs(n.y) < 0.5 {
                    albedo = vec3<f32>(0.16, 0.19, 0.22);
                }
            }
        } else if m > 49.0 {
            albedo = select(vec3<f32>(0.22, 0.36, 0.16), vec3<f32>(0.30, 0.21, 0.14), p.y < 0.5);
        }
        let dif = clamp(dot(n, sun), 0.0, 1.0) * soft_shadow(p + n * 0.01, sun);
        let occ = ao(p, n);
        let sky_l = (0.55 + 0.45 * n.y) * vec3<f32>(0.42, 0.50, 0.62);
        let bounce = clamp(-n.y, 0.0, 1.0) * 0.15 + clamp(0.3 - n.y, 0.0, 1.0) * 0.08;
        var lit = albedo * (dif * vec3<f32>(1.75, 1.62, 1.42) + occ * sky_l * 0.8 + bounce * occ);

        if m < 0.5 {
            // Editor grid on the ground plane.
            let fp = t * u.pixel_angle / max(abs(rd.y), 0.05);
            let minor = grid_line(p.xz, 1.0, fp, 1.0);
            let major = grid_line(p.xz, 5.0, fp, 1.6);
            lit = mix(lit, lit * 0.55, minor * 0.6);
            lit = mix(lit, lit * 0.35, major * 0.7);
            let ax = 1.0 - clamp(abs(p.z) / max(fp, 1e-4) - 0.5, 0.0, 1.0);
            let az = 1.0 - clamp(abs(p.x) / max(fp, 1e-4) - 0.5, 0.0, 1.0);
            lit = mix(lit, vec3<f32>(0.75, 0.22, 0.20), ax * 0.8);
            lit = mix(lit, vec3<f32>(0.22, 0.38, 0.78), az * 0.8);
        }
        let fog = 1.0 - exp(-t * 0.018);
        col = mix(lit, sky(rd), fog);
    }

    // Selection outline: rays that pass close to the selected object without hitting it.
    let hit_sel = abs(m - f32(SEL + 1)) < 0.1;
    let px = sel_min / max(sel_t * u.pixel_angle, 1e-5);
    let outline = (1.0 - smoothstep(2.5, 3.5, px)) * select(1.0, 0.0, hit_sel) * u.accent.a;

    var out = pow(tonemap(col * 0.95), vec3<f32>(1.0 / 2.2));
    out = mix(out, u.accent.rgb, outline);
    return vec4<f32>(out, 1.0);
}
