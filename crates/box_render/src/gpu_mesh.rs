//! GPU copies of `MeshData` assets: one vertex and one index buffer per mesh,
//! allocated with headroom and updated only when the asset version changes.

use std::collections::HashMap;

use box_core::assets::Assets;
use box_core::math::Aabb;
use box_core::mesh::{MaterialGroup, MeshData};

use crate::skinning::{GpuSkin, joint_bounds, skin_vertices};
use crate::uniforms::GpuVertex;

pub(crate) struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub bounds: Aabb,
    /// Index ranges by material slot; never empty.
    pub groups: Vec<MaterialGroup>,
    /// Joints and weights (second vertex buffer) of a mesh that has them.
    pub skin: Option<GpuSkin>,
    version: u64,
}

/// The mesh's draw ranges: its groups, or one group of every index in slot
/// 0 when it has none. Groups outside the index buffer, or not a whole
/// number of triangles, are dropped (with a warning).
pub(crate) fn draw_groups(mesh: &MeshData, id: u64) -> Vec<MaterialGroup> {
    let total = mesh.indices.len() as u64;
    if mesh.groups.is_empty() {
        return vec![MaterialGroup {
            first_index: 0,
            index_count: total as u32,
            material_slot: 0,
        }];
    }
    let groups: Vec<_> = mesh
        .groups
        .iter()
        .copied()
        .filter(|g| {
            let ok = g.index_count > 0
                && g.index_count.is_multiple_of(3)
                && g.first_index as u64 + g.index_count as u64 <= total;
            if !ok {
                tracing::warn!("mesh {id}: material group {g:?} is out of range and is not drawn");
            }
            ok
        })
        .collect();
    groups
}

/// Upload counters for one frame.
#[derive(Default)]
pub(crate) struct UploadStats {
    pub bytes: u64,
    pub reallocations: u32,
}

#[derive(Default)]
pub(crate) struct MeshCache {
    meshes: HashMap<u64, GpuMesh>,
    /// Version of each asset that was not uploaded (invalid, or nothing to
    /// draw), so it is validated and reported once per version.
    rejected: HashMap<u64, u64>,
    scratch: Vec<GpuVertex>,
    /// Calls to `upload` (validation plus buffer writes).
    #[cfg(test)]
    pub(crate) upload_attempts: u32,
}

/// Initial capacity for `needed` bytes: 50% headroom, 256-byte granularity.
pub(crate) fn initial_capacity(needed: u64) -> u64 {
    (needed + needed / 2).max(256).next_multiple_of(256)
}

/// Capacity after growing past `current`: doubling, or more if still too small.
pub(crate) fn grown_capacity(current: u64, needed: u64) -> u64 {
    (current * 2).max(initial_capacity(needed))
}

impl MeshCache {
    pub(crate) fn get(&self, id: u64) -> Option<&GpuMesh> {
        self.meshes.get(&id)
    }

    pub(crate) fn remove(&mut self, ids: &[u64]) {
        for id in ids {
            self.meshes.remove(id);
            self.rejected.remove(id);
        }
    }

    /// Uploads new or changed meshes and frees the ones that are not in
    /// `assets`. Only touches meshes whose version differs from the uploaded
    /// (or rejected) copy.
    #[profiling::function]
    pub(crate) fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &Assets<MeshData>,
        stats: &mut UploadStats,
    ) {
        for (id, version, mesh) in assets.iter() {
            if self.meshes.get(&id).is_some_and(|m| m.version == version)
                || self.rejected.get(&id) == Some(&version)
            {
                continue;
            }
            self.upload(device, queue, id, version, mesh, stats);
        }
        // Every asset is now uploaded or rejected, so a larger cache holds
        // meshes of another store (a replaced world never reports them as
        // removed). The count check keeps the common frame free of the scan.
        if self.meshes.len() + self.rejected.len() > assets.len() {
            self.meshes.retain(|&id, _| assets.get_by_id(id).is_some());
            self.rejected
                .retain(|&id, _| assets.get_by_id(id).is_some());
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: u64,
        version: u64,
        mesh: &MeshData,
        stats: &mut UploadStats,
    ) {
        #[cfg(test)]
        {
            self.upload_attempts += 1;
        }
        let invalid = mesh.validate().err();
        if let Some(e) = &invalid {
            tracing::warn!("mesh {id} is invalid and will not be drawn: {e}");
        }
        if invalid.is_some() || mesh.indices.is_empty() {
            self.meshes.remove(&id);
            self.rejected.insert(id, version);
            return;
        }
        self.rejected.remove(&id);

        self.scratch.clear();
        let uv0 = mesh.uvs.first();
        let uv1 = mesh.uvs.get(1);
        let custom = [mesh.custom.first(), mesh.custom.get(1)];
        self.scratch
            .extend((0..mesh.positions.len()).map(|i| GpuVertex {
                position: mesh.positions[i].to_array(),
                normal: mesh.normals[i].to_array(),
                uv: uv0.map_or([0.0; 2], |uv| uv[i].to_array()),
                color: mesh.colors.get(i).copied().unwrap_or([1.0; 4]),
                tangent: mesh.tangents.get(i).map_or([0.0; 4], |t| t.to_array()),
                uv1: uv1.map_or([0.0; 2], |uv| uv[i].to_array()),
                custom: custom.map(|set| set.map_or([0.0; 4], |c| c[i].to_array())),
            }));
        let vertex_bytes: &[u8] = bytemuck::cast_slice(&self.scratch);
        let index_bytes: &[u8] = bytemuck::cast_slice(&mesh.indices);

        let entry = self.meshes.remove(&id);
        let skin = skin_vertices(mesh).map(|vertices| {
            let bytes: &[u8] = bytemuck::cast_slice(&vertices);
            let old = entry.as_ref().and_then(|e| e.skin.as_ref());
            let buffer = match old {
                Some(s) if s.buffer.size() >= bytes.len() as u64 => s.buffer.clone(),
                Some(s) => {
                    stats.reallocations += 1;
                    let capacity = grown_capacity(s.buffer.size(), bytes.len() as u64);
                    create(
                        device,
                        bytes.len() as u64,
                        wgpu::BufferUsages::VERTEX,
                        id,
                        Some(capacity),
                    )
                }
                None => create(
                    device,
                    bytes.len() as u64,
                    wgpu::BufferUsages::VERTEX,
                    id,
                    None,
                ),
            };
            queue.write_buffer(&buffer, 0, bytes);
            stats.bytes += bytes.len() as u64;
            GpuSkin {
                buffer,
                joint_bounds: joint_bounds(mesh),
            }
        });
        let (vertex_buffer, index_buffer) = match entry {
            Some(old) => {
                let vb = ensure_capacity(
                    device,
                    old.vertex_buffer,
                    vertex_bytes.len() as u64,
                    wgpu::BufferUsages::VERTEX,
                    id,
                    stats,
                );
                let ib = ensure_capacity(
                    device,
                    old.index_buffer,
                    index_bytes.len() as u64,
                    wgpu::BufferUsages::INDEX,
                    id,
                    stats,
                );
                (vb, ib)
            }
            None => (
                create(
                    device,
                    vertex_bytes.len() as u64,
                    wgpu::BufferUsages::VERTEX,
                    id,
                    None,
                ),
                create(
                    device,
                    index_bytes.len() as u64,
                    wgpu::BufferUsages::INDEX,
                    id,
                    None,
                ),
            ),
        };
        queue.write_buffer(&vertex_buffer, 0, vertex_bytes);
        queue.write_buffer(&index_buffer, 0, index_bytes);
        stats.bytes += (vertex_bytes.len() + index_bytes.len()) as u64;

        self.meshes.insert(
            id,
            GpuMesh {
                vertex_buffer,
                index_buffer,
                groups: draw_groups(mesh, id),
                bounds: mesh.bounds(),
                skin,
                version,
            },
        );
    }

    pub(crate) fn len(&self) -> usize {
        self.meshes.len()
    }

    /// Uploaded plus rejected meshes.
    #[cfg(test)]
    pub(crate) fn tracked(&self) -> usize {
        self.meshes.len() + self.rejected.len()
    }
}

fn create(
    device: &wgpu::Device,
    needed: u64,
    usage: wgpu::BufferUsages,
    id: u64,
    capacity: Option<u64>,
) -> wgpu::Buffer {
    let kind = if usage.contains(wgpu::BufferUsages::VERTEX) {
        "vertices"
    } else {
        "indices"
    };
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("mesh {id} {kind}")),
        size: capacity.unwrap_or_else(|| initial_capacity(needed)),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn ensure_capacity(
    device: &wgpu::Device,
    buffer: wgpu::Buffer,
    needed: u64,
    usage: wgpu::BufferUsages,
    id: u64,
    stats: &mut UploadStats,
) -> wgpu::Buffer {
    if needed <= buffer.size() {
        return buffer;
    }
    stats.reallocations += 1;
    let capacity = grown_capacity(buffer.size(), needed);
    create(device, needed, usage, id, Some(capacity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_default_to_one_and_drop_bad_ranges() {
        let mut m = MeshData::cuboid(glam::Vec3::ONE);
        let all = draw_groups(&m, 1);
        assert_eq!(
            all,
            vec![MaterialGroup {
                first_index: 0,
                index_count: 36,
                material_slot: 0
            }]
        );
        let group = |first_index, index_count, material_slot| MaterialGroup {
            first_index,
            index_count,
            material_slot,
        };
        m.groups = vec![
            group(0, 18, 1),
            group(18, 18, 0),
            group(30, 12, 2),
            group(0, 4, 3),
        ];
        assert_eq!(draw_groups(&m, 1), vec![group(0, 18, 1), group(18, 18, 0)]);
    }

    #[test]
    fn capacity_has_headroom_and_doubles() {
        assert_eq!(initial_capacity(0), 256);
        assert_eq!(initial_capacity(1000), 1536);
        assert_eq!(grown_capacity(1536, 1600), 3072);
        assert_eq!(grown_capacity(256, 10_000), initial_capacity(10_000));
        for n in [1u64, 4, 255, 4097, 1 << 20] {
            assert!(initial_capacity(n) >= n && initial_capacity(n).is_multiple_of(256));
        }
    }
}
