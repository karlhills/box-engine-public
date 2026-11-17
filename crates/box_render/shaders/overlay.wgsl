// Screen-space overlay (game UI): 2D triangles in physical pixels, drawn
// after tone mapping and the display lines, with premultiplied alpha:
// `*_gamma` entry points into a plain view of the output (blending in sRGB
// space), `*_linear` into an sRGB view. Self-contained (no includes); loaded by
// overlay.rs, not by the shader library.

struct Screen {
    // Target size in physical pixels.
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var tex_sampler: sampler;

struct VertexInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    // sRGB-encoded, premultiplied (as the UI painter writes them).
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc = in.pos / screen.size * 2.0 - 1.0;
    out.clip = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

fn linear_from_srgb(c: vec3<f32>) -> vec3<f32> {
    let cutoff = c < vec3<f32>(0.04045);
    let lower = c / 12.92;
    let higher = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

// Managed textures (the font atlas): premultiplied sRGB-encoded values in a
// plain unorm texture.

// Plain output view: write sRGB values, blend in sRGB space.
@fragment
fn fs_managed_gamma(in: VertexOutput) -> @location(0) vec4<f32> {
    return in.color * textureSample(tex, tex_sampler, in.uv);
}

// sRGB output view: convert to linear first.
@fragment
fn fs_managed_linear(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = in.color * textureSample(tex, tex_sampler, in.uv);
    return vec4<f32>(linear_from_srgb(c.rgb), c.a);
}

fn srgb_from_linear(c: vec3<f32>) -> vec3<f32> {
    let cutoff = c < vec3<f32>(0.0031308);
    let lower = c * 12.92;
    let higher = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(higher, lower, cutoff);
}

// Image assets: straight alpha in an sRGB (or linear) texture, sampled as
// linear values, tinted by the vertex colour (premultiplied sRGB). Returns
// sRGB-encoded colour with straight alpha.
fn image_color(in: VertexOutput) -> vec4<f32> {
    let t = textureSample(tex, tex_sampler, in.uv);
    let tint_a = in.color.a;
    var tint = vec3<f32>(1.0);
    if tint_a > 0.0 {
        tint = in.color.rgb / tint_a;
    }
    return vec4<f32>(srgb_from_linear(t.rgb) * tint, t.a * tint_a);
}

@fragment
fn fs_image_gamma(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = image_color(in);
    return vec4<f32>(c.rgb * c.a, c.a);
}

@fragment
fn fs_image_linear(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = image_color(in);
    return vec4<f32>(linear_from_srgb(c.rgb) * c.a, c.a);
}
