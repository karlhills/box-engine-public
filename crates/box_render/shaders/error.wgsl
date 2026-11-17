// The error material: an unlit magenta and black checker, drawn instead of a
// material whose shader failed to compile.

#include "common.wgsl"
#include "object.wgsl"

struct ErrorOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> ErrorOutput {
    load_object(in.instance);
    let world = object.model * vec4<f32>(in.position, 1.0);
    var out: ErrorOutput;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    return out;
}

@fragment
fn fs_main(in: ErrorOutput) -> @location(0) vec4<f32> {
    let cell = vec3<i32>(floor(in.world_pos * 4.0));
    let odd = ((cell.x + cell.y + cell.z) & 1) == 1;
    // Divided by the exposure so the colour reads the same at any exposure.
    let exposure = max(globals.post.x, 1e-4);
    return vec4<f32>(select(vec3<f32>(2.0, 0.0, 2.0), vec3<f32>(0.02), odd) / exposure, 1.0);
}
