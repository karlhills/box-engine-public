//! Local lights (point and spot): gathering, culling, selection, the one
//! shadowed local light, ambient override volumes, and packing into the
//! counted light storage buffer (`Lights` in common.wgsl).

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};

use crate::components::LocalShadow;
use crate::uniforms::{
    GpuAmbientVolume, GpuLocalLight, LIGHTS_HEADER, LOCAL_SHADOW_VIEWS, MAX_AMBIENT_VOLUMES,
};

/// Most point and spot lights drawn per frame. Beyond this the most
/// important ones (brightness over distance from the camera) are kept. The shader loops over
/// every light for every lit fragment, so this is a cap, not a target:
/// clustered lighting (RS5) is the answer for many lights.
pub const MAX_LOCAL_LIGHTS: usize = 256;

/// Size of the light buffer: header plus [`MAX_LOCAL_LIGHTS`] lights.
pub(crate) const LIGHT_BUFFER_SIZE: u64 =
    LIGHTS_HEADER + (MAX_LOCAL_LIGHTS * std::mem::size_of::<GpuLocalLight>()) as u64;

/// A visible point or spot light this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightCandidate {
    pub position: Vec3,
    pub range: f32,
    /// Linear colour times intensity.
    pub radiance: Vec3,
    /// Spot axis (unit), zero for point lights.
    pub direction: Vec3,
    /// Cone factor parameters ([`crate::components::spot_scale_offset`]);
    /// (0, 1) for point lights.
    pub spot: (f32, f32),
    /// Spot outer angle (radians); 0 for point lights.
    pub outer_angle: f32,
    /// The light asks for shadows with these settings.
    pub shadow: Option<LocalShadow>,
}

impl LightCandidate {
    pub(crate) fn point(position: Vec3, range: f32, radiance: Vec3) -> Self {
        Self {
            position,
            range,
            radiance,
            direction: Vec3::ZERO,
            spot: (0.0, 1.0),
            outer_angle: 0.0,
            shadow: None,
        }
    }

    pub(crate) fn is_spot(&self) -> bool {
        self.direction != Vec3::ZERO
    }

    /// How much the light matters from `eye`: its brightness over one plus
    /// the squared distance from the eye to its sphere of influence (lights
    /// around the camera rank by brightness alone).
    pub(crate) fn importance(&self, eye: Vec3) -> f32 {
        let brightness = self.radiance.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        let gap = ((self.position - eye).length() - self.range).max(0.0);
        brightness / (1.0 + gap * gap)
    }
}

/// Keeps at most `max` lights, most important first, when there are more.
/// Below the cap the order is left alone (no sort cost).
pub(crate) fn select_lights(lights: &mut Vec<LightCandidate>, eye: Vec3, max: usize) {
    if lights.len() <= max {
        return;
    }
    lights.sort_by(|a, b| b.importance(eye).total_cmp(&a.importance(eye)));
    lights.truncate(max);
}

/// The index of the frame's shadowed local light: the most important of
/// `lights` that asks for shadows (the first on ties).
pub(crate) fn shadowed_light(lights: &[LightCandidate], eye: Vec3) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, l) in lights.iter().enumerate() {
        if l.shadow.is_none() {
            continue;
        }
        let w = l.importance(eye);
        if best.is_none_or(|(_, b)| w > b) {
            best = Some((i, w));
        }
    }
    best.map(|(i, _)| i)
}

/// Near plane of local shadow views (m).
const SHADOW_NEAR: f32 = 0.05;

/// The views of a shadowed light: one for a spot light, six (+X, −X, +Y,
/// −Y, +Z, −Z, the face order of `local_shadow` in lighting.wgsl) for a
/// point light. `resolution` widens each view by two texels on every side
/// so the PCF kernel never reads past a cube face's edge. Standard depth
/// (0 near, 1 at `range`).
pub(crate) fn shadow_views(light: &LightCandidate, resolution: u32) -> ShadowViews {
    let far = light.range.max(SHADOW_NEAR * 2.0);
    let margin = 1.0 + 4.0 / resolution.max(1) as f32;
    let look = |dir: Vec3, up: Vec3, tan_half: f32| {
        let proj = directx::perspective((tan_half * margin).atan() * 2.0, 1.0, SHADOW_NEAR, far);
        proj * view::look_to_mat4(light.position, dir, up)
    };
    let mut views = ShadowViews {
        view_proj: [Mat4::IDENTITY; LOCAL_SHADOW_VIEWS],
        count: 1,
        texel_scale: 0.0,
    };
    if light.is_spot() {
        // At most 170° across, whatever the cone.
        let half = light.outer_angle.clamp(0.01, 85f32.to_radians());
        let up = if light.direction.y.abs() > 0.99 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let tan_half = half.tan();
        views.view_proj[0] = look(light.direction, up, tan_half);
        views.texel_scale = 2.0 * tan_half * margin / resolution.max(1) as f32;
    } else {
        let faces = [
            (Vec3::X, Vec3::NEG_Y),
            (Vec3::NEG_X, Vec3::NEG_Y),
            (Vec3::Y, Vec3::Z),
            (Vec3::NEG_Y, Vec3::NEG_Z),
            (Vec3::Z, Vec3::NEG_Y),
            (Vec3::NEG_Z, Vec3::NEG_Y),
        ];
        for (i, (dir, up)) in faces.into_iter().enumerate() {
            views.view_proj[i] = look(dir, up, 1.0);
        }
        views.count = 6;
        views.texel_scale = 2.0 * margin / resolution.max(1) as f32;
    }
    views
}

/// The shadowed light's view-projection matrices.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShadowViews {
    pub view_proj: [Mat4; LOCAL_SHADOW_VIEWS],
    /// Views in use (1 spot, 6 point).
    pub count: usize,
    /// World size of one shadow texel per metre of distance from the light.
    pub texel_scale: f32,
}

/// The shadow block of the light buffer header.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ShadowHeader {
    /// 0 none, 1 spot, 2 point.
    pub kind: u32,
    pub normal_bias: f32,
    pub depth_bias: f32,
    pub texel_scale: f32,
    pub view_proj: [Mat4; LOCAL_SHADOW_VIEWS],
}

/// Writes the light buffer contents: counts, the shadow block, the ambient
/// volumes, then the lights. `shadowed` is the index of the light whose
/// shadow is in the local shadow map.
pub(crate) fn pack_lights(
    lights: &[LightCandidate],
    shadowed: Option<usize>,
    shadow: &ShadowHeader,
    volumes: &[GpuAmbientVolume],
    out: &mut Vec<u8>,
) {
    out.clear();
    let count = lights.len().min(MAX_LOCAL_LIGHTS);
    let volume_count = volumes.len().min(MAX_AMBIENT_VOLUMES);
    out.extend_from_slice(bytemuck::cast_slice(&[
        count as u32,
        volume_count as u32,
        0,
        0,
    ]));
    out.extend_from_slice(bytemuck::cast_slice(&[
        shadow.kind as f32,
        shadow.normal_bias,
        shadow.depth_bias,
        shadow.texel_scale,
    ]));
    for m in &shadow.view_proj {
        out.extend_from_slice(bytemuck::cast_slice(&m.to_cols_array()));
    }
    for i in 0..MAX_AMBIENT_VOLUMES {
        let v = volumes
            .get(i)
            .filter(|_| i < volume_count)
            .copied()
            .unwrap_or_else(bytemuck::Zeroable::zeroed);
        out.extend_from_slice(bytemuck::bytes_of(&v));
    }
    debug_assert_eq!(out.len() as u64, LIGHTS_HEADER);
    for (i, l) in lights[..count].iter().enumerate() {
        out.extend_from_slice(bytemuck::bytes_of(&GpuLocalLight {
            position: l.position.to_array(),
            range: l.range,
            color: l.radiance.to_array(),
            shadow: if shadowed == Some(i) { 1.0 } else { 0.0 },
            direction: l.direction.to_array(),
            spot_scale: l.spot.0,
            spot_offset: l.spot.1,
            _pad: [0.0; 3],
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(x: f32, range: f32, brightness: f32) -> LightCandidate {
        LightCandidate::point(Vec3::new(x, 0.0, 0.0), range, Vec3::splat(brightness))
    }

    #[test]
    fn packing_matches_the_shader_layout() {
        let mut spot = light(-1.0, 2.0, 0.5);
        spot.direction = Vec3::NEG_Y;
        spot.spot = (3.0, -2.0);
        let lights = [
            LightCandidate::point(Vec3::new(1.0, 2.0, 3.0), 4.0, Vec3::new(5.0, 6.0, 7.0)),
            spot,
        ];
        let header = ShadowHeader {
            kind: 1,
            normal_bias: 0.5,
            depth_bias: 0.25,
            texel_scale: 0.125,
            view_proj: [Mat4::from_scale(Vec3::splat(2.0)); LOCAL_SHADOW_VIEWS],
        };
        let volume = GpuAmbientVolume {
            world_to_local: Mat4::IDENTITY.to_cols_array_2d(),
            half_extents: [1.0, 2.0, 3.0, 0.5],
            ambient: [0.1, 0.2, 0.3, 0.0],
        };
        let mut bytes = Vec::new();
        pack_lights(&lights, Some(1), &header, &[volume], &mut bytes);
        assert_eq!(bytes.len() as u64, LIGHTS_HEADER + 2 * 64);
        let words: &[u32] = bytemuck::cast_slice(&bytes[..16]);
        assert_eq!(words, &[2, 1, 0, 0], "count header");
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!(&floats[4..8], &[1.0, 0.5, 0.25, 0.125], "shadow block");
        assert_eq!(floats[8], 2.0, "first shadow matrix");
        let volumes = 8 + 6 * 16;
        assert_eq!(
            floats[volumes + 16..volumes + 24],
            [1.0, 2.0, 3.0, 0.5, 0.1, 0.2, 0.3, 0.0]
        );
        let first = LIGHTS_HEADER as usize / 4;
        // position.xyz, range, colour.rgb, shadow, direction.xyz, scale,
        // offset, pad: the WGSL struct layout.
        assert_eq!(
            &floats[first..first + 16],
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0
            ]
        );
        assert_eq!(
            &floats[first + 16..first + 30],
            &[
                -1.0, 0.0, 0.0, 2.0, 0.5, 0.5, 0.5, 1.0, 0.0, -1.0, 0.0, 3.0, -2.0, 0.0
            ]
        );
        // Nothing: just the header.
        pack_lights(&[], None, &ShadowHeader::default(), &[], &mut bytes);
        assert_eq!(bytes.len() as u64, LIGHTS_HEADER);
        // The buffer holds the cap exactly.
        let many = vec![light(0.0, 1.0, 1.0); MAX_LOCAL_LIGHTS + 10];
        pack_lights(&many, None, &ShadowHeader::default(), &[], &mut bytes);
        assert_eq!(bytes.len() as u64, LIGHT_BUFFER_SIZE);
    }

    #[test]
    fn selection_keeps_the_nearest_and_brightest() {
        let eye = Vec3::ZERO;
        // Far and dim, around the camera, far and bright, near and dim.
        let mut lights = vec![
            light(100.0, 5.0, 1.0),
            light(1.0, 5.0, 0.2),
            light(100.0, 5.0, 50.0),
            light(8.0, 5.0, 1.0),
        ];
        let untouched = lights.clone();
        select_lights(&mut lights, eye, 4);
        assert_eq!(lights, untouched, "under the cap: no reordering");
        select_lights(&mut lights, eye, 2);
        assert_eq!(lights, vec![untouched[1], untouched[3]]);
        let inside = untouched[1].importance(eye);
        assert!(
            (inside - 0.2).abs() < 1e-6,
            "inside its range: brightness only"
        );
    }

    #[test]
    fn the_most_important_light_asking_for_shadows_gets_them() {
        let eye = Vec3::ZERO;
        let mut lights = vec![
            light(1.0, 5.0, 4.0),
            light(2.0, 5.0, 1.0),
            light(30.0, 5.0, 2.0),
        ];
        assert_eq!(shadowed_light(&lights, eye), None, "nobody asks");
        lights[1].shadow = Some(LocalShadow::default());
        lights[2].shadow = Some(LocalShadow::default());
        assert_eq!(shadowed_light(&lights, eye), Some(1), "near beats far");
        lights[0].shadow = Some(LocalShadow::default());
        assert_eq!(shadowed_light(&lights, eye), Some(0), "brighter wins");
    }

    #[test]
    fn shadow_views_cover_the_cone_and_the_cube() {
        let mut spot = light(0.0, 10.0, 1.0);
        spot.position = Vec3::new(0.0, 3.0, 0.0);
        spot.direction = Vec3::NEG_Y;
        spot.outer_angle = 30f32.to_radians();
        let v = shadow_views(&spot, 1024);
        assert_eq!(v.count, 1);
        let project = |m: Mat4, p: Vec3| {
            let c = m * p.extend(1.0);
            c.truncate() / c.w
        };
        // Straight below: the centre of the map, in front of the far plane.
        let below = project(v.view_proj[0], Vec3::ZERO);
        assert!(below.x.abs() < 1e-5 && below.y.abs() < 1e-5);
        assert!(below.z > 0.0 && below.z < 1.0);
        // On the cone's edge: just inside the map.
        let edge = Vec3::new(3.0 * 30f32.to_radians().tan(), 0.0, 0.0);
        let e = project(v.view_proj[0], edge);
        let off = e.x.abs().max(e.y.abs());
        assert!(off < 1.0 && off > 0.98, "{e}");
        // Each cube face sees its own axis at its centre.
        let point = light(0.0, 10.0, 1.0);
        let v = shadow_views(&point, 512);
        assert_eq!(v.count, 6);
        for (i, dir) in [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .into_iter()
        .enumerate()
        {
            let c = project(v.view_proj[i], dir * 2.0);
            assert!(
                c.x.abs() < 1e-5 && c.y.abs() < 1e-5 && c.z > 0.0,
                "face {i}: {c}"
            );
        }
    }
}
