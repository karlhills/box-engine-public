//! Stable hashing: cache keys over reflected parameters and
//! [`mesh_hash`] over quantized generator output.
//!
//! Both use [`StableHasher`] (64-bit FNV-1a), which has no per-process seed,
//! so a hash is the same in every run and on every machine for the same
//! input bytes. Cache keys only live in memory; mesh hashes are stored in
//! golden fixtures ([`crate::golden`]).

use bevy_reflect::{PartialReflect, ReflectRef};
use box_core::mesh::MeshData;

/// 64-bit FNV-1a. Deterministic across runs and platforms (all integers
/// are written little-endian).
#[derive(Clone, Copy, Debug)]
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;

    pub fn new() -> Self {
        Self(Self::OFFSET)
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    pub fn write_u8(&mut self, v: u8) {
        self.write(&[v]);
    }

    pub fn write_u32(&mut self, v: u32) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.write(&v.to_le_bytes());
    }

    /// A length-prefixed string, so `("ab", "c")` and `("a", "bc")` differ.
    pub fn write_str(&mut self, s: &str) {
        self.write_u64(s.len() as u64);
        self.write(s.as_bytes());
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Canonical hash of a reflected value: its structure and leaf values, not
/// its memory layout. Concrete and dynamic values of the same shape hash
/// alike; struct fields are hashed by name in declaration order; **map and
/// set entries are sorted** (by their own hashes), so insertion or iteration
/// order never changes the result. Floats hash their bits with `-0.0`
/// folded into `0.0` and every NaN folded into one. Opaque values other
/// than numbers, `bool`, `char` and `String` use their reflected hash
/// (`#[reflect(Hash)]`, as `AssetRef` and `Uid` have), else their debug
/// text.
pub fn reflect_hash(value: &dyn PartialReflect) -> u64 {
    let mut h = StableHasher::new();
    hash_reflect(value, &mut h);
    h.finish()
}

/// Feeds `value` into `h` as [`reflect_hash`] does.
pub fn hash_reflect(value: &dyn PartialReflect, h: &mut StableHasher) {
    match value.reflect_ref() {
        ReflectRef::Struct(s) => {
            h.write_u8(1);
            h.write_u64(s.field_len() as u64);
            for i in 0..s.field_len() {
                h.write_str(s.name_at(i).unwrap_or(""));
                if let Some(f) = s.field_at(i) {
                    hash_reflect(f, h);
                }
            }
        }
        ReflectRef::TupleStruct(s) => {
            h.write_u8(2);
            h.write_u64(s.field_len() as u64);
            for i in 0..s.field_len() {
                if let Some(f) = s.field(i) {
                    hash_reflect(f, h);
                }
            }
        }
        ReflectRef::Tuple(t) => {
            h.write_u8(3);
            h.write_u64(t.field_len() as u64);
            for i in 0..t.field_len() {
                if let Some(f) = t.field(i) {
                    hash_reflect(f, h);
                }
            }
        }
        ReflectRef::List(l) => {
            h.write_u8(4);
            h.write_u64(l.len() as u64);
            for item in l.iter() {
                hash_reflect(item, h);
            }
        }
        ReflectRef::Array(a) => {
            h.write_u8(4);
            h.write_u64(a.len() as u64);
            for item in a.iter() {
                hash_reflect(item, h);
            }
        }
        ReflectRef::Map(m) => {
            let mut entries: Vec<(u64, u64)> = m
                .iter()
                .map(|(k, v)| (reflect_hash(k), reflect_hash(v)))
                .collect();
            entries.sort_unstable();
            h.write_u8(5);
            h.write_u64(entries.len() as u64);
            for (k, v) in entries {
                h.write_u64(k);
                h.write_u64(v);
            }
        }
        ReflectRef::Set(s) => {
            let mut items: Vec<u64> = s.iter().map(reflect_hash).collect();
            items.sort_unstable();
            h.write_u8(6);
            h.write_u64(items.len() as u64);
            for i in items {
                h.write_u64(i);
            }
        }
        ReflectRef::Enum(e) => {
            h.write_u8(7);
            h.write_str(e.variant_name());
            h.write_u64(e.field_len() as u64);
            for i in 0..e.field_len() {
                if let Some(name) = e.name_at(i) {
                    h.write_str(name);
                }
                if let Some(f) = e.field_at(i) {
                    hash_reflect(f, h);
                }
            }
        }
        ReflectRef::Opaque(v) => {
            h.write_u8(8);
            hash_opaque(v, h);
        }
        #[allow(unreachable_patterns)]
        _ => {
            h.write_u8(9);
            h.write_str(&format!("{value:?}"));
        }
    }
}

/// Canonical float bits: `-0.0` as `0.0`, every NaN as one NaN.
fn f32_bits(v: f32) -> u32 {
    if v == 0.0 {
        0
    } else if v.is_nan() {
        f32::NAN.to_bits()
    } else {
        v.to_bits()
    }
}

fn f64_bits(v: f64) -> u64 {
    if v == 0.0 {
        0
    } else if v.is_nan() {
        f64::NAN.to_bits()
    } else {
        v.to_bits()
    }
}

fn hash_opaque(v: &dyn PartialReflect, h: &mut StableHasher) {
    macro_rules! ints {
        ($($t:ty),*) => {
            $(if let Some(x) = v.try_downcast_ref::<$t>() {
                h.write_str(stringify!($t));
                h.write(&(*x as i128).to_le_bytes());
                return;
            })*
        };
    }
    if let Some(x) = v.try_downcast_ref::<f32>() {
        h.write_str("f32");
        h.write_u32(f32_bits(*x));
        return;
    }
    if let Some(x) = v.try_downcast_ref::<f64>() {
        h.write_str("f64");
        h.write_u64(f64_bits(*x));
        return;
    }
    ints!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
    if let Some(x) = v.try_downcast_ref::<i128>() {
        h.write_str("i128");
        h.write(&x.to_le_bytes());
        return;
    }
    if let Some(x) = v.try_downcast_ref::<u128>() {
        h.write_str("u128");
        h.write(&x.to_le_bytes());
        return;
    }
    if let Some(x) = v.try_downcast_ref::<bool>() {
        h.write_str("bool");
        h.write_u8(u8::from(*x));
        return;
    }
    if let Some(x) = v.try_downcast_ref::<char>() {
        h.write_str("char");
        h.write_u32(u32::from(*x));
        return;
    }
    if let Some(x) = v.try_downcast_ref::<String>() {
        h.write_str("String");
        h.write_str(x);
        return;
    }
    h.write_str(v.reflect_type_path());
    match v.reflect_hash() {
        Some(x) => h.write_u64(x),
        None => h.write_str(&format!("{v:?}")),
    }
}

/// Grid steps of [`mesh_hash`]: positions and UVs to 1e-4, normals,
/// tangents and colours to 1e-3.
const POSITION_STEP: f32 = 1e4;
const DIRECTION_STEP: f32 = 1e3;

fn q(v: f32, scale: f32) -> i64 {
    // `as` saturates and maps NaN to 0; -0.0 becomes the integer 0.
    (v * scale).round() as i64
}

/// A stable hash of what a mesh draws, for determinism and golden tests.
///
/// Values are **quantized** (positions and UVs to 0.1 mm / 1e-4, normals,
/// tangents and vertex colours to 1e-3) so float noise below that does not
/// change the hash. The hash covers the triangles **as drawn**: triangles
/// are grouped by material slot (ascending; groups with the same slot keep
/// their order), and every triangle corner hashes its vertex's attributes,
/// so re-indexing, welding or splitting vertices does not change it, while
/// moving a triangle to another material slot does. Which optional channels
/// exist (tangents, UV sets, colours) is part of the hash. A mesh without
/// groups counts as one group in slot 0. Indices out of range hash as a
/// marker instead of panicking.
pub fn mesh_hash(mesh: &MeshData) -> u64 {
    let mut h = StableHasher::new();
    let n = mesh.positions.len();
    let has = |len: usize| len == n && n > 0;
    let tangents = has(mesh.tangents.len());
    let colors = has(mesh.colors.len());
    let uv_sets: Vec<&Vec<glam::Vec2>> = mesh.uvs.iter().filter(|uv| has(uv.len())).collect();
    let normals = has(mesh.normals.len());
    h.write_str("box_proc mesh_hash v1");
    h.write_u8(u8::from(normals));
    h.write_u8(u8::from(tangents));
    h.write_u8(u8::from(colors));
    h.write_u64(uv_sets.len() as u64);

    let mut groups: Vec<(u32, u32, u32)> = if mesh.groups.is_empty() {
        vec![(0, 0, mesh.indices.len() as u32)]
    } else {
        mesh.groups
            .iter()
            .map(|g| (g.material_slot, g.first_index, g.index_count))
            .collect()
    };
    groups.sort_by_key(|g| g.0);
    let mut slot = None;
    for (s, first, count) in groups {
        if slot != Some(s) {
            h.write_str("slot");
            h.write_u32(s);
            slot = Some(s);
        }
        let start = (first as usize).min(mesh.indices.len());
        let end = (first as usize + count as usize).min(mesh.indices.len());
        for &i in &mesh.indices[start..end] {
            let i = i as usize;
            if i >= n {
                h.write_str("bad index");
                continue;
            }
            let p = mesh.positions[i];
            for c in [p.x, p.y, p.z] {
                h.write_i64(q(c, POSITION_STEP));
            }
            if normals {
                let v = mesh.normals[i];
                for c in [v.x, v.y, v.z] {
                    h.write_i64(q(c, DIRECTION_STEP));
                }
            }
            if tangents {
                let t = mesh.tangents[i];
                for c in [t.x, t.y, t.z, t.w] {
                    h.write_i64(q(c, DIRECTION_STEP));
                }
            }
            for uv in &uv_sets {
                let v = uv[i];
                h.write_i64(q(v.x, POSITION_STEP));
                h.write_i64(q(v.y, POSITION_STEP));
            }
            if colors {
                for c in mesh.colors[i] {
                    h.write_i64(q(c, DIRECTION_STEP));
                }
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_reflect::map::DynamicMap;
    use box_core::mesh::MaterialGroup;
    use glam::Vec3;

    #[test]
    fn fnv_matches_reference_values() {
        let mut h = StableHasher::new();
        h.write(b"a");
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(StableHasher::new().finish(), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn map_order_does_not_change_the_hash() {
        let mut a = DynamicMap::default();
        a.insert("width".to_string(), 1.0f32);
        a.insert("height".to_string(), 2.0f32);
        let mut b = DynamicMap::default();
        b.insert("height".to_string(), 2.0f32);
        b.insert("width".to_string(), 1.0f32);
        assert_eq!(reflect_hash(&a), reflect_hash(&b));
        b.insert("width".to_string(), 1.5f32);
        assert_ne!(reflect_hash(&a), reflect_hash(&b));
    }

    #[test]
    fn floats_fold_signed_zero_and_nan() {
        assert_eq!(reflect_hash(&0.0f32), reflect_hash(&-0.0f32));
        assert_eq!(reflect_hash(&f32::NAN), reflect_hash(&-f32::NAN));
        assert_ne!(reflect_hash(&1.0f32), reflect_hash(&1.0f64));
        assert_ne!(reflect_hash(&1u32), reflect_hash(&1i32));
    }

    #[test]
    fn concrete_and_dynamic_structs_hash_alike() {
        let v = Vec3::new(1.0, 2.0, 3.0);
        let dynamic = v.to_dynamic();
        assert_eq!(reflect_hash(&v), reflect_hash(dynamic.unwrap().as_ref()));
    }

    fn quad() -> MeshData {
        let mut m = MeshData::cuboid(Vec3::ONE);
        m.groups = vec![
            MaterialGroup {
                first_index: 0,
                index_count: 18,
                material_slot: 0,
            },
            MaterialGroup {
                first_index: 18,
                index_count: 18,
                material_slot: 1,
            },
        ];
        m
    }

    #[test]
    fn mesh_hash_ignores_vertex_layout_and_noise_but_not_slots() {
        let m = quad();
        let base = mesh_hash(&m);
        // Unweld: every corner its own vertex.
        let mut unwelded = m.clone();
        unwelded.positions = m.indices.iter().map(|&i| m.positions[i as usize]).collect();
        unwelded.normals = m.indices.iter().map(|&i| m.normals[i as usize]).collect();
        unwelded.uvs = m
            .uvs
            .iter()
            .map(|uv| m.indices.iter().map(|&i| uv[i as usize]).collect())
            .collect();
        if !m.tangents.is_empty() {
            unwelded.tangents = m.indices.iter().map(|&i| m.tangents[i as usize]).collect();
        }
        unwelded.indices = (0..m.indices.len() as u32).collect();
        assert_eq!(mesh_hash(&unwelded), base);
        // Sub-quantum noise.
        let mut noisy = m.clone();
        noisy.positions[0].x += 1e-6;
        assert_eq!(mesh_hash(&noisy), base);
        // A real change.
        let mut moved = m.clone();
        moved.positions[0].x += 0.01;
        assert_ne!(mesh_hash(&moved), base);
        // Another material slot.
        let mut reslotted = m.clone();
        reslotted.groups[1].material_slot = 2;
        assert_ne!(mesh_hash(&reslotted), base);
        // Groups listed in another order hash alike.
        let mut swapped = m.clone();
        swapped.groups.swap(0, 1);
        assert_eq!(mesh_hash(&swapped), base);
        // Bad indices do not panic.
        let mut bad = m;
        bad.indices[0] = 9999;
        let _ = mesh_hash(&bad);
    }
}
