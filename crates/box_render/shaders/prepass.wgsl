// Depth + normal prepass output, run before the main pass when ambient
// occlusion needs it: the world-space geometric normal, encoded n * 0.5 + 0.5
// in an Rgb10a2Unorm target. `flip` turns back faces of double-sided
// materials around.

fn prepass_normal(n: vec3<f32>, flip: bool) -> vec4<f32> {
    let len_sq = dot(n, n);
    var m = vec3<f32>(0.0, 1.0, 0.0);
    if len_sq > 1e-20 {
        m = n * inverseSqrt(len_sq);
    }
    if flip {
        m = -m;
    }
    return vec4<f32>(m * 0.5 + vec3<f32>(0.5), 1.0);
}
