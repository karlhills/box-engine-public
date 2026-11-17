// Shared by the ambient occlusion passes. Requires common.wgsl.

// Linear view depth stored for sky texels (nothing to occlude).
const AO_SKY_DEPTH: f32 = 1.0e9;

struct FullscreenOutput {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vertex_index: u32) -> FullscreenOutput {
    var out: FullscreenOutput;
    out.clip = vec4<f32>(fullscreen_ndc(vertex_index), 0.0, 1.0);
    return out;
}

// View depth (distance along the camera's forward axis) of a reverse-Z
// infinite-projection depth value.
fn linear_depth(z: f32) -> f32 {
    if z <= 0.0 {
        return AO_SKY_DEPTH;
    }
    return globals.camera_pos.w / z;
}
