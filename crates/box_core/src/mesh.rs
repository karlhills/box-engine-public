//! CPU mesh data and built-in primitives.

use crate::pmath;
use std::f32::consts::{PI, TAU};

use glam::{Vec2, Vec3, Vec4};

use crate::math::Aabb;

/// A range of indices drawn with one material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialGroup {
    pub first_index: u32,
    pub index_count: u32,
    pub material_slot: u32,
}

/// The engine's only CPU mesh format: an indexed triangle list.
///
/// `positions` and `normals` are required and have one entry per vertex.
/// Optional channels are either empty or have one entry per vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// xyz tangent, w handedness.
    pub tangents: Vec<Vec4>,
    /// Up to four UV sets. Extra sets carry game data as well as texture coordinates.
    pub uvs: Vec<Vec<Vec2>>,
    /// Linear RGBA vertex colours.
    pub colors: Vec<[f32; 4]>,
    /// Up to two sets of four free numbers per vertex for material shaders
    /// (`custom0` and `custom1` of `VertexInput` and `SurfaceInput`): growth
    /// stamps, flow directions, wetness. The engine uploads them unchanged
    /// (zero where a mesh has none) and never reads them.
    pub custom: Vec<Vec<Vec4>>,
    /// Reserved for skinning (M7).
    pub joints: Vec<[u16; 4]>,
    /// Reserved for skinning (M7).
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Material ranges. Empty means one group covering every index.
    pub groups: Vec<MaterialGroup>,
}

/// Why a mesh is invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    ChannelLength {
        channel: &'static str,
        expected: usize,
        actual: usize,
    },
    TooManyUvSets(usize),
    TooManyCustomSets(usize),
    IndexCountNotMultipleOfThree(usize),
    IndexOutOfRange {
        index: u32,
        vertex_count: usize,
    },
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for MeshError {}

impl MeshData {
    pub const MAX_UV_SETS: usize = 4;
    pub const MAX_CUSTOM_SETS: usize = 2;

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.positions.iter().copied())
    }

    pub fn validate(&self) -> Result<(), MeshError> {
        let n = self.positions.len();
        let check = |channel: &'static str, len: usize, optional: bool| {
            if (optional && len == 0) || len == n {
                Ok(())
            } else {
                Err(MeshError::ChannelLength {
                    channel,
                    expected: n,
                    actual: len,
                })
            }
        };
        check("normals", self.normals.len(), false)?;
        check("tangents", self.tangents.len(), true)?;
        check("colors", self.colors.len(), true)?;
        check("joints", self.joints.len(), true)?;
        check("weights", self.weights.len(), true)?;
        if self.uvs.len() > Self::MAX_UV_SETS {
            return Err(MeshError::TooManyUvSets(self.uvs.len()));
        }
        for uv in &self.uvs {
            check("uvs", uv.len(), false)?;
        }
        if self.custom.len() > Self::MAX_CUSTOM_SETS {
            return Err(MeshError::TooManyCustomSets(self.custom.len()));
        }
        for set in &self.custom {
            check("custom", set.len(), false)?;
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err(MeshError::IndexCountNotMultipleOfThree(self.indices.len()));
        }
        if let Some(&index) = self.indices.iter().find(|&&i| i as usize >= n) {
            return Err(MeshError::IndexOutOfRange {
                index,
                vertex_count: n,
            });
        }
        Ok(())
    }

    /// Fill `colors` with one colour.
    pub fn with_vertex_color(mut self, color: [f32; 4]) -> Self {
        self.colors = vec![color; self.positions.len()];
        self
    }

    /// [`generate_tangents`](Self::generate_tangents), as a builder.
    pub fn with_tangents(mut self) -> Self {
        self.generate_tangents();
        self
    }

    /// Fills `tangents` from UV set 0, for normal mapping. Per-triangle
    /// tangents are accumulated per vertex and made perpendicular to the
    /// normal; `w` is the bitangent sign, so `cross(normal, tangent) * w`
    /// points along +V (the MikkTSpace convention glTF uses, up to its
    /// per-triangle weighting). A vertex whose triangles have degenerate UVs
    /// gets some unit tangent perpendicular to its normal. Never produces
    /// NaNs. Does nothing when the mesh has no UVs or mismatched channels.
    pub fn generate_tangents(&mut self) {
        let n = self.positions.len();
        let Some(uv) = self.uvs.first() else {
            return;
        };
        if uv.len() != n || self.normals.len() != n {
            return;
        }
        let mut tangent = vec![Vec3::ZERO; n];
        let mut bitangent = vec![Vec3::ZERO; n];
        for tri in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = tri.map(|i| i as usize);
            if a >= n || b >= n || c >= n {
                continue;
            }
            let (e1, e2) = (
                self.positions[b] - self.positions[a],
                self.positions[c] - self.positions[a],
            );
            let (d1, d2) = (uv[b] - uv[a], uv[c] - uv[a]);
            let det = d1.x * d2.y - d2.x * d1.y;
            if det.abs() < 1e-12 {
                continue;
            }
            let t = (e1 * d2.y - e2 * d1.y) / det;
            let bt = (e2 * d1.x - e1 * d2.x) / det;
            if !t.is_finite() || !bt.is_finite() {
                continue;
            }
            for i in [a, b, c] {
                tangent[i] += t;
                bitangent[i] += bt;
            }
        }
        self.tangents = (0..n)
            .map(|i| {
                let normal = self.normals[i].normalize_or(Vec3::Y);
                let t = tangent[i] - normal * normal.dot(tangent[i]);
                let t = if t.length_squared() > 1e-20 && t.is_finite() {
                    t.normalize()
                } else {
                    normal.any_orthonormal_vector()
                };
                let w = if normal.cross(t).dot(bitangent[i]) < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                t.extend(w)
            })
            .collect();
    }

    // ----- primitives -----

    /// An axis-aligned box centred on the origin, with hard edges.
    pub fn cuboid(size: Vec3) -> Self {
        let h = size * 0.5;
        let faces: [(Vec3, Vec3, Vec3); 6] = [
            (Vec3::X, Vec3::NEG_Z, Vec3::Y),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::X, Vec3::NEG_Z),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::NEG_X, Vec3::Y),
        ];
        let mut m = MeshData::default();
        let mut uv = Vec::new();
        for (n, u, v) in faces {
            let base = m.positions.len() as u32;
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                m.positions.push((n + u * su + v * sv) * h);
                m.normals.push(n);
                uv.push(Vec2::new((su + 1.0) * 0.5, 1.0 - (sv + 1.0) * 0.5));
            }
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        m.uvs.push(uv);
        m
    }

    /// A flat plane on XZ, facing +Y, centred on the origin.
    pub fn plane(size_x: f32, size_z: f32) -> Self {
        Self::grid(size_x, size_z, 1, 1)
    }

    /// A subdivided XZ plane facing +Y, centred on the origin.
    pub fn grid(size_x: f32, size_z: f32, cells_x: u32, cells_z: u32) -> Self {
        let (cx, cz) = (cells_x.max(1), cells_z.max(1));
        let mut m = MeshData::default();
        let mut uv = Vec::new();
        for j in 0..=cz {
            for i in 0..=cx {
                let (u, v) = (i as f32 / cx as f32, j as f32 / cz as f32);
                m.positions
                    .push(Vec3::new((u - 0.5) * size_x, 0.0, (v - 0.5) * size_z));
                m.normals.push(Vec3::Y);
                uv.push(Vec2::new(u, v));
            }
        }
        let row = cx + 1;
        for j in 0..cz {
            for i in 0..cx {
                let a = j * row + i;
                let (b, c, d) = (a + 1, a + row, a + row + 1);
                m.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        m.uvs.push(uv);
        m
    }

    /// A UV sphere centred on the origin.
    pub fn uv_sphere(radius: f32, sectors: u32, stacks: u32) -> Self {
        let (sectors, stacks) = (sectors.max(3), stacks.max(2));
        let mut m = MeshData::default();
        let mut uv = Vec::new();
        for j in 0..=stacks {
            let v = j as f32 / stacks as f32;
            let phi = v * PI;
            for i in 0..=sectors {
                let u = i as f32 / sectors as f32;
                let theta = u * TAU;
                let n = Vec3::new(
                    pmath::sin(phi) * pmath::cos(theta),
                    pmath::cos(phi),
                    -pmath::sin(phi) * pmath::sin(theta),
                );
                m.positions.push(n * radius);
                m.normals.push(n);
                uv.push(Vec2::new(u, v));
            }
        }
        let row = sectors + 1;
        for j in 0..stacks {
            for i in 0..sectors {
                let a = j * row + i;
                let (b, c, d) = (a + 1, a + row, a + row + 1);
                if j != 0 {
                    m.indices.extend_from_slice(&[a, c, b]);
                }
                if j != stacks - 1 {
                    m.indices.extend_from_slice(&[b, c, d]);
                }
            }
        }
        m.uvs.push(uv);
        m
    }

    /// A capped cylinder along Y, centred on the origin.
    pub fn cylinder(radius: f32, height: f32, segments: u32) -> Self {
        let segments = segments.max(3);
        let h = height * 0.5;
        let mut m = MeshData::default();
        let mut uv = Vec::new();
        // Side.
        for i in 0..=segments {
            let u = i as f32 / segments as f32;
            let a = u * TAU;
            let n = Vec3::new(pmath::cos(a), 0.0, -pmath::sin(a));
            for (y, v) in [(-h, 1.0), (h, 0.0)] {
                m.positions.push(n * radius + Vec3::Y * y);
                m.normals.push(n);
                uv.push(Vec2::new(u, v));
            }
        }
        for i in 0..segments {
            let a = i * 2;
            m.indices
                .extend_from_slice(&[a, a + 2, a + 1, a + 1, a + 2, a + 3]);
        }
        // Caps.
        for (y, n) in [(h, Vec3::Y), (-h, Vec3::NEG_Y)] {
            let center = m.positions.len() as u32;
            m.positions.push(Vec3::Y * y);
            m.normals.push(n);
            uv.push(Vec2::splat(0.5));
            for i in 0..segments {
                let a = i as f32 / segments as f32 * TAU;
                let d = Vec3::new(pmath::cos(a), 0.0, -pmath::sin(a));
                m.positions.push(d * radius + Vec3::Y * y);
                m.normals.push(n);
                uv.push(Vec2::new(0.5 + d.x * 0.5, 0.5 + d.z * 0.5));
            }
            for i in 0..segments {
                let a = center + 1 + i;
                let b = center + 1 + (i + 1) % segments;
                if n.y > 0.0 {
                    m.indices.extend_from_slice(&[center, a, b]);
                } else {
                    m.indices.extend_from_slice(&[center, b, a]);
                }
            }
        }
        m.uvs.push(uv);
        m
    }

    /// A capsule along Y: a cylinder of `half_height * 2` with hemispherical ends.
    pub fn capsule(radius: f32, half_height: f32, segments: u32, rings: u32) -> Self {
        let (segments, rings) = (segments.max(3), rings.max(1));
        let mut m = MeshData::default();
        let mut uv = Vec::new();
        // Rows from top pole to bottom pole: rings rows per hemisphere, two at the equator seam.
        let mut rows: Vec<(f32, f32, f32)> = Vec::new(); // (y_offset, ring_radius_factor, normal_y)
        for r in 0..=rings {
            let phi = r as f32 / rings as f32 * PI * 0.5;
            rows.push((half_height, pmath::sin(phi), pmath::cos(phi)));
        }
        for r in 0..=rings {
            let phi = PI * 0.5 + r as f32 / rings as f32 * PI * 0.5;
            rows.push((-half_height, pmath::sin(phi), pmath::cos(phi)));
        }
        let total = rows.len() as f32 - 1.0;
        for (j, (y_off, rr, ny)) in rows.iter().enumerate() {
            for i in 0..=segments {
                let u = i as f32 / segments as f32;
                let a = u * TAU;
                let n =
                    Vec3::new(rr * pmath::cos(a), *ny, -rr * pmath::sin(a)).normalize_or(Vec3::Y);
                m.positions.push(n * radius + Vec3::Y * *y_off);
                m.normals.push(n);
                uv.push(Vec2::new(u, j as f32 / total));
            }
        }
        let row = segments + 1;
        for j in 0..rows.len() as u32 - 1 {
            for i in 0..segments {
                let a = j * row + i;
                let (b, c, d) = (a + 1, a + row, a + row + 1);
                m.indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
        m.uvs.push(uv);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_outward_winding(m: &MeshData) {
        // Counter-clockwise triangles (seen from outside) have geometric normals
        // agreeing with the vertex normals.
        for t in m.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.positions[i as usize]);
            let geo = (b - a).cross(c - a);
            if geo.length_squared() < 1e-12 {
                continue;
            }
            let n = m.normals[t[0] as usize] + m.normals[t[1] as usize] + m.normals[t[2] as usize];
            assert!(geo.dot(n) > 0.0, "inward-facing triangle {t:?}");
        }
    }

    #[test]
    fn custom_sets_are_validated() {
        let mut m = MeshData::plane(1.0, 1.0);
        m.custom = vec![vec![Vec4::ONE; m.vertex_count()]; 2];
        m.validate().unwrap();
        m.custom.push(vec![Vec4::ONE; m.vertex_count()]);
        assert_eq!(m.validate(), Err(MeshError::TooManyCustomSets(3)));
        m.custom.truncate(1);
        m.custom[0].pop();
        assert!(matches!(
            m.validate(),
            Err(MeshError::ChannelLength {
                channel: "custom",
                ..
            })
        ));
    }

    #[test]
    fn cuboid_counts_bounds_winding() {
        let m = MeshData::cuboid(Vec3::new(2.0, 4.0, 6.0));
        m.validate().unwrap();
        assert_eq!((m.vertex_count(), m.triangle_count()), (24, 12));
        assert_eq!(
            m.bounds(),
            Aabb::new(Vec3::new(-1.0, -2.0, -3.0), Vec3::new(1.0, 2.0, 3.0))
        );
        assert_outward_winding(&m);
    }

    #[test]
    fn grid_counts_and_winding() {
        let m = MeshData::grid(10.0, 4.0, 5, 2);
        m.validate().unwrap();
        assert_eq!((m.vertex_count(), m.triangle_count()), (18, 20));
        assert_eq!(
            m.bounds(),
            Aabb::new(Vec3::new(-5.0, 0.0, -2.0), Vec3::new(5.0, 0.0, 2.0))
        );
        assert_outward_winding(&m);
    }

    #[test]
    fn sphere_counts_bounds_winding() {
        let m = MeshData::uv_sphere(2.0, 16, 8);
        m.validate().unwrap();
        assert_eq!(m.vertex_count(), 17 * 9);
        assert_eq!(m.triangle_count(), 16 * 2 + 16 * 6 * 2);
        let b = m.bounds();
        assert!((b.max.y - 2.0).abs() < 1e-5 && (b.min.y + 2.0).abs() < 1e-5);
        assert_outward_winding(&m);
    }

    #[test]
    fn cylinder_counts_bounds_winding() {
        let m = MeshData::cylinder(1.0, 3.0, 12);
        m.validate().unwrap();
        assert_eq!(m.vertex_count(), 13 * 2 + 2 * 13);
        assert_eq!(m.triangle_count(), 12 * 2 + 12 * 2);
        let b = m.bounds();
        assert!((b.max.y - 1.5).abs() < 1e-5 && (b.max.x - 1.0).abs() < 1e-5);
        assert_outward_winding(&m);
    }

    #[test]
    fn capsule_counts_bounds_winding() {
        let m = MeshData::capsule(0.5, 1.0, 12, 4);
        m.validate().unwrap();
        assert_eq!(m.vertex_count(), 10 * 13);
        assert_eq!(m.triangle_count(), 9 * 12 * 2);
        let b = m.bounds();
        assert!((b.max.y - 1.5).abs() < 1e-5 && (b.min.y + 1.5).abs() < 1e-5);
        assert_outward_winding(&m);
    }

    #[test]
    fn tangents_follow_u_and_stay_finite() {
        // The plane's U runs along +X and V along +Z, with the normal +Y:
        // cross(Y, X) = -Z, so the bitangent sign is -1.
        let m = MeshData::plane(2.0, 2.0).with_tangents();
        m.validate().unwrap();
        for t in &m.tangents {
            assert!((t.truncate() - Vec3::X).length() < 1e-5, "{t}");
            assert_eq!(t.w, -1.0);
        }
        // Spheres have degenerate pole triangles; every tangent is still a
        // finite unit vector perpendicular to its normal.
        let s = MeshData::uv_sphere(1.0, 12, 6).with_tangents();
        for (t, n) in s.tangents.iter().zip(&s.normals) {
            assert!(t.is_finite());
            assert!((t.truncate().length() - 1.0).abs() < 1e-4);
            assert!(t.truncate().dot(*n).abs() < 1e-4);
        }
        // Degenerate UVs: some perpendicular tangent, no NaN.
        let mut d = MeshData::plane(1.0, 1.0);
        d.uvs[0].iter_mut().for_each(|uv| *uv = Vec2::ZERO);
        d.generate_tangents();
        assert!(d.tangents.iter().all(|t| t.is_finite() && t.y.abs() < 1e-5));
        // No UVs: left alone.
        let mut none = MeshData::plane(1.0, 1.0);
        none.uvs.clear();
        none.generate_tangents();
        assert!(none.tangents.is_empty());
    }

    #[test]
    fn validation_catches_errors() {
        let mut m = MeshData::cuboid(Vec3::ONE);
        m.indices.push(999);
        assert!(m.validate().is_err());
        let mut m = MeshData::cuboid(Vec3::ONE);
        m.colors = vec![[1.0; 4]; 3];
        assert!(matches!(
            m.validate(),
            Err(MeshError::ChannelLength {
                channel: "colors",
                ..
            })
        ));
    }
}
