//! Render targets and parameters of the RS3 effects: the depth + normal
//! prepass with half-resolution ambient occlusion (ao_resolve.wgsl,
//! ao.wgsl), the bloom mip chain (bloom.wgsl), colour grading and vignette
//! (tonemap.wgsl), and the uniforms of the sky, atmosphere (fog.wgsl) and
//! wind (wind.wgsl).

use glam::{Mat3, Vec2, Vec3};

use crate::components::{ColorGrading, Environment, SkyGradient};
use crate::pipelines::{AO_DEPTH_FORMAT, AO_FORMAT, DEPTH_FORMAT, Layouts, NORMAL_FORMAT};
use crate::uniforms::GlobalsUniform;

/// Most bloom mips (from half resolution down).
const MAX_BLOOM_MIPS: u32 = 5;
/// Bloom stops halving below this many pixels on the short side.
const MIN_BLOOM_SIZE: u32 = 8;

fn texture(
    device: &wgpu::Device,
    label: &str,
    (w, h): (u32, u32),
    format: wgpu::TextureFormat,
    samples: u32,
    mips: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    entries: &[(u32, wgpu::BindingResource<'_>)],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = entries
        .iter()
        .map(|(binding, resource)| wgpu::BindGroupEntry {
            binding: *binding,
            resource: resource.clone(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

/// Half of a size, rounded up.
pub(crate) fn half_size((w, h): (u32, u32)) -> (u32, u32) {
    (w.div_ceil(2).max(1), h.div_ceil(2).max(1))
}

/// The prepass and ambient occlusion targets of one render target size,
/// created the first time the prepass runs for it (ambient occlusion, or a
/// visible material with a depth fade). The prepass depth is the scene
/// depth that soft transparency reads.
pub(crate) struct AoTargets {
    _textures: Vec<wgpu::Texture>,
    pub prepass_depth: wgpu::TextureView,
    pub prepass_normal: wgpu::TextureView,
    /// Half resolution: linear view depth, normals, raw and blurred AO.
    pub half_depth: wgpu::TextureView,
    pub half_normal: wgpu::TextureView,
    pub raw: wgpu::TextureView,
    pub ao: wgpu::TextureView,
    pub resolve_bind_group: wgpu::BindGroup,
    pub compute_bind_group: wgpu::BindGroup,
    pub blur_bind_group: wgpu::BindGroup,
    /// The frame bind group with this target's AO (the main pass binds
    /// it), with the frame resources generation it was made for: it is
    /// made again when the frame's shared bindings change (the local
    /// shadow map was reallocated).
    frame_bind_group: std::sync::Mutex<Option<(u64, wgpu::BindGroup)>>,
}

impl AoTargets {
    pub(crate) fn new(device: &wgpu::Device, layouts: &Layouts, size: (u32, u32)) -> Self {
        let half = half_size(size);
        let prepass_depth = texture(device, "prepass depth", size, DEPTH_FORMAT, 1, 1);
        let prepass_normal = texture(device, "prepass normals", size, NORMAL_FORMAT, 1, 1);
        let half_depth = texture(device, "ao view depth", half, AO_DEPTH_FORMAT, 1, 1);
        let half_normal = texture(device, "ao normals", half, NORMAL_FORMAT, 1, 1);
        let raw = texture(device, "ao raw", half, AO_FORMAT, 1, 1);
        let ao = texture(device, "ao", half, AO_FORMAT, 1, 1);
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let (pd, pn, hd, hn, rv, av) = (
            view(&prepass_depth),
            view(&prepass_normal),
            view(&half_depth),
            view(&half_normal),
            view(&raw),
            view(&ao),
        );
        let tv = wgpu::BindingResource::TextureView;
        let resolve_bind_group = bind_group(
            device,
            "ao resolve bind group",
            &layouts.ao_resolve,
            &[(0, tv(&pd)), (1, tv(&pn))],
        );
        let compute_bind_group = bind_group(
            device,
            "ao bind group",
            &layouts.ao_compute,
            &[(0, tv(&hd)), (1, tv(&hn))],
        );
        let blur_bind_group = bind_group(
            device,
            "ao blur bind group",
            &layouts.ao_blur,
            &[(0, tv(&hd)), (2, tv(&rv))],
        );
        Self {
            _textures: vec![
                prepass_depth,
                prepass_normal,
                half_depth,
                half_normal,
                raw,
                ao,
            ],
            prepass_depth: pd,
            prepass_normal: pn,
            half_depth: hd,
            half_normal: hn,
            raw: rv,
            ao: av,
            resolve_bind_group,
            compute_bind_group,
            blur_bind_group,
            frame_bind_group: std::sync::Mutex::new(None),
        }
    }

    /// The frame bind group with this target's AO, AO depth and prepass
    /// depth, made by `make(ao, ao_depth, prepass_depth)` when missing or
    /// made for another `generation`.
    pub(crate) fn frame_bind_group(
        &self,
        generation: u64,
        make: impl FnOnce(&wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView) -> wgpu::BindGroup,
    ) -> wgpu::BindGroup {
        let mut slot = self
            .frame_bind_group
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*slot {
            Some((g, group)) if *g == generation => group.clone(),
            _ => {
                let group = make(&self.ao, &self.half_depth, &self.prepass_depth);
                *slot = Some((generation, group.clone()));
                group
            }
        }
    }
}

/// The bloom mip chain of one render target size and its bind groups,
/// created the first time bloom is drawn into it.
pub(crate) struct BloomTargets {
    _texture: wgpu::Texture,
    /// One view per mip (mip 0 is half resolution).
    pub mips: Vec<wgpu::TextureView>,
    /// `down[i]` reads the source of mip `i`: the scene for 0, else mip i-1.
    pub down: Vec<wgpu::BindGroup>,
    /// `up[i]` reads mip `i` (blended into mip i-1).
    pub up: Vec<wgpu::BindGroup>,
    /// The tone-mapping bind group with bloom mip 0.
    pub tonemap_bind_group: wgpu::BindGroup,
}

impl BloomTargets {
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        hdr: &wgpu::TextureView,
        hdr_format: wgpu::TextureFormat,
        sampler: &wgpu::Sampler,
        size: (u32, u32),
    ) -> Self {
        let half = half_size(size);
        let mut count = 1;
        while count < MAX_BLOOM_MIPS && (half.0.min(half.1) >> count) >= MIN_BLOOM_SIZE {
            count += 1;
        }
        let texture = texture(device, "bloom chain", half, hdr_format, 1, count);
        let mips: Vec<wgpu::TextureView> = (0..count)
            .map(|mip| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("bloom mip"),
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let sampler = wgpu::BindingResource::Sampler(sampler);
        let reads = |view: &wgpu::TextureView| {
            bind_group(
                device,
                "bloom bind group",
                &layouts.bloom,
                &[
                    (0, wgpu::BindingResource::TextureView(view)),
                    (1, sampler.clone()),
                ],
            )
        };
        let down = std::iter::once(hdr)
            .chain(mips.iter().take(count as usize - 1))
            .map(reads)
            .collect();
        let up = mips.iter().map(reads).collect();
        let tonemap_bind_group = tonemap_bind_group(device, layouts, hdr, &mips[0], &sampler);
        Self {
            _texture: texture,
            mips,
            down,
            up,
            tonemap_bind_group,
        }
    }
}

/// The tone-mapping pass's group 1: scene, bloom mip 0, linear sampler.
pub(crate) fn tonemap_bind_group(
    device: &wgpu::Device,
    layouts: &Layouts,
    hdr: &wgpu::TextureView,
    bloom: &wgpu::TextureView,
    sampler: &wgpu::BindingResource<'_>,
) -> wgpu::BindGroup {
    let tv = wgpu::BindingResource::TextureView;
    bind_group(
        device,
        "tonemap bind group",
        &layouts.tonemap,
        &[(0, tv(hdr)), (1, tv(bloom)), (2, sampler.clone())],
    )
}

/// Linear sRGB to LMS cone space and back (Unity's colour balance).
const LIN_TO_LMS: Mat3 = Mat3::from_cols_array(&[
    3.90405e-1, 7.08416e-2, 2.31082e-2, // column 0
    5.49941e-1, 9.63172e-1, 1.28021e-1, // column 1
    8.92632e-3, 1.35775e-3, 9.36245e-1, // column 2
]);
const LMS_TO_LIN: Mat3 = Mat3::from_cols_array(&[
    2.85847,
    -2.10182e-1,
    -4.18120e-2, //
    -1.62879,
    1.15820,
    -1.18169e-1, //
    -2.48910e-2,
    3.24281e-4,
    1.06867, //
]);

/// The white balance matrix (linear RGB) for `temperature` and `tint` in
/// -1..1: von Kries adaptation in LMS from the D65 white to a white point
/// shifted along the daylight locus (temperature) and across it (tint).
pub(crate) fn white_balance(temperature: f32, tint: f32) -> Mat3 {
    let t1 = temperature * 100.0 / 65.0;
    let t2 = tint * 100.0 / 65.0;
    let x = 0.31271 - t1 * if t1 < 0.0 { 0.1 } else { 0.05 };
    let standard_y = 2.87 * x - 3.0 * x * x - 0.275_095_07;
    let y = standard_y + t2 * 0.05;
    let xyz = Vec3::new(x / y, 1.0, (1.0 - x - y) / y);
    let lms = Vec3::new(
        0.7328 * xyz.x + 0.4296 * xyz.y - 0.1624 * xyz.z,
        -0.7036 * xyz.x + 1.6975 * xyz.y + 0.0061 * xyz.z,
        0.0030 * xyz.x + 0.0136 * xyz.y + 0.9834 * xyz.z,
    );
    let d65 = Vec3::new(0.949_237, 1.035_42, 1.087_28);
    LMS_TO_LIN * Mat3::from_diagonal(d65 / lms) * LIN_TO_LMS
}

/// Writes the grading fields of `globals`.
pub(crate) fn write_grading(g: &ColorGrading, globals: &mut GlobalsUniform) {
    let smh = g.shadows != Vec3::ONE || g.midtones != Vec3::ONE || g.highlights != Vec3::ONE;
    let scene =
        g.temperature != 0.0 || g.tint != 0.0 || g.contrast != 1.0 || g.saturation != 1.0 || smh;
    let tint = |v: Vec3, w: f32| v.max(Vec3::ZERO).extend(w).to_array();
    globals.shadows_tint = tint(g.shadows, g.shadows_end.max(1e-4));
    globals.midtones_tint = tint(g.midtones, g.highlights_start);
    globals.highlights_tint = tint(g.highlights, if smh { 1.0 } else { 0.0 });
    let display = g.lift != Vec3::ZERO || g.gamma != Vec3::ONE || g.gain != Vec3::ONE;
    globals.grade = [
        g.contrast.max(0.0),
        g.saturation.max(0.0),
        if scene { 1.0 } else { 0.0 },
        if display { 1.0 } else { 0.0 },
    ];
    let wb = white_balance(g.temperature.clamp(-1.0, 1.0), g.tint.clamp(-1.0, 1.0));
    globals.white_balance = [
        wb.x_axis.extend(0.0).to_array(),
        wb.y_axis.extend(0.0).to_array(),
        wb.z_axis.extend(0.0).to_array(),
    ];
    globals.lift = g.lift.extend(0.0).to_array();
    globals.inv_gamma = g
        .gamma
        .max(Vec3::splat(0.01))
        .recip()
        .extend(0.0)
        .to_array();
    globals.gain = g.gain.max(Vec3::ZERO).extend(0.0).to_array();
}

/// Writes the sky, atmosphere, vignette and wind fields of `globals` from
/// `env` (`aspect` is the target's width / height). Disabled effects write
/// zeros, which the shaders skip.
pub(crate) fn write_look(env: &Environment, aspect: f32, globals: &mut GlobalsUniform) {
    let sky = &env.sky;
    globals.sky_sun = [
        sky.sun_glow.max(0.0),
        sky.sun_disc.max(0.0),
        sky.horizon_glow.max(0.0),
        sky.stars.max(0.0),
    ];
    if let SkyGradient::Layered {
        upper,
        below_horizon,
    } = sky.gradient
    {
        globals.sky_upper = upper.to_vec3().extend(1.0).to_array();
        globals.sky_below = below_horizon.to_vec3().extend(0.0).to_array();
    }
    let h = &env.aerial_haze;
    if h.enabled && h.max_opacity > 0.0 && h.density > 0.0 {
        globals.haze = [
            h.density,
            h.max_opacity.clamp(0.0, 1.0),
            h.start_distance.max(0.0),
            h.sun_scatter.max(0.0),
        ];
        globals.haze_color = h
            .color
            .to_vec3()
            .extend(h.sky_height.clamp(0.0, 1.0))
            .to_array();
    }
    let m = &env.height_mist;
    if m.enabled && m.max_opacity > 0.0 {
        globals.mist = [
            m.top,
            m.bottom.min(m.top),
            m.max_opacity.clamp(0.0, 1.0),
            m.near_fade.max(0.0),
        ];
        globals.mist_color = m.color.to_vec3().extend(0.0).to_array();
    }
    let q = &env.distance_quieting;
    if q.enabled && (q.desaturate > 0.0 || q.contrast > 0.0) {
        globals.quiet = [
            q.start.max(0.0),
            q.end.max(q.start.max(0.0) + 1e-3),
            q.desaturate.clamp(0.0, 1.0),
            q.contrast.clamp(0.0, 1.0),
        ];
        globals.quiet_tone = q.tone.to_vec3().extend(1.0).to_array();
        // The sky below the haze's sky height counts as far scenery.
        globals.haze_color[3] = env.aerial_haze.sky_height.clamp(0.0, 1.0);
    }
    let v = &env.vignette;
    if v.intensity > 0.0 {
        globals.vignette = [
            v.intensity.clamp(0.0, 1.0) * 3.0,
            v.smoothness.clamp(0.01, 1.0) * 5.0,
            if v.rounded { aspect } else { 1.0 },
            0.0,
        ];
    }
    let w = &env.wind;
    let dir = w.direction.normalize_or_zero();
    if dir != Vec2::ZERO && w.strength > 0.0 {
        globals.wind = [dir.x, dir.y, w.strength, w.gust_length.max(0.01)];
        globals.wind_gust = [w.gust_speed, 0.0, 0.0, 0.0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_balance_is_identity_at_zero_and_warms_with_temperature() {
        let id = white_balance(0.0, 0.0);
        let grey = Vec3::splat(0.5);
        assert!(
            (id * grey - grey).abs().max_element() < 2e-3,
            "{}",
            id * grey
        );
        let warm = white_balance(0.5, 0.0) * grey;
        let cool = white_balance(-0.5, 0.0) * grey;
        assert!(warm.x > warm.z * 1.1, "warm {warm}");
        assert!(cool.z > cool.x * 1.1, "cool {cool}");
        let magenta = white_balance(0.0, 0.5) * grey;
        assert!(magenta.y < magenta.x && magenta.y < magenta.z, "{magenta}");
    }

    #[test]
    fn bloom_chain_and_half_sizes() {
        assert_eq!(half_size((2560, 1440)), (1280, 720));
        assert_eq!(half_size((5, 3)), (3, 2));
        assert_eq!(half_size((1, 1)), (1, 1));
    }
}
