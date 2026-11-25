//! GPU skinning (RS4): joint palettes and skinned bounds.
//!
//! A mesh with joints and weights gets a second vertex buffer of
//! [`GpuSkinVertex`] (gpu_mesh.rs). Each frame, every drawn entity with a
//! [`SkinnedMesh`] whose skin asset is loaded gets a palette: one matrix
//! per joint, `mesh⁻¹ × joint × inverse bind`, appended to one storage
//! buffer (group 1, binding 2). The entity's instance slot records where
//! its palette starts (`Object.params.x`), and the `*_skinned` vertex
//! entry points (skin.wgsl) blend up to four palette matrices per vertex
//! in every pass (main, shadow, prepass, outline mask).
//!
//! Batching: the palette offset is per instance, so skinned entities with
//! equal (pipeline, material, mesh, range) are still one instanced draw
//! whatever their poses. Skinned and static draws never merge (their
//! pipelines differ).
//!
//! Bounds: per mesh, the bind-space box of the vertices each joint
//! influences is computed once at upload ([`joint_bounds`]); per frame the
//! skinned box is the union of those boxes moved by their palette matrix,
//! a conservative bound of every posed vertex (a weighted average of
//! points inside the moved boxes stays inside their union) used for
//! frustum and shadow culling and the outline scissor.
//!
//! Per-frame cost: a palette matrix and a box transform per joint of each
//! skinned entity (CPU), and one upload of every palette when any changed
//! (64 bytes per joint). Idle scenes upload nothing.

use std::collections::{HashMap, HashSet};

use box_core::animation::{Skin, SkinnedMesh, joint_matrix};
use box_core::assets::Assets;
use box_core::math::Aabb;
use box_core::mesh::MeshData;
use box_core::transform::GlobalTransform;
use box_core::world::{Entity, Without, World};
use bytemuck::{Pod, Zeroable};
use glam::{Affine3A, Mat4};

use crate::components::{Hidden, MeshRenderer};
use crate::gpu_mesh::MeshCache;

/// Joints and weights of one vertex (`SkinnedVertexInput` locations 8 and
/// 9 in skin.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuSkinVertex {
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

const SKIN_ATTRIBUTES: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![
    8 => Uint16x4,
    9 => Float32x4,
];

/// The second vertex buffer of skinned pipelines.
pub(crate) fn skin_buffer() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GpuSkinVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &SKIN_ATTRIBUTES,
    }
}

/// Skinning data of an uploaded mesh.
pub(crate) struct GpuSkin {
    pub buffer: wgpu::Buffer,
    /// Bind-space bounds of the vertices each joint influences (empty for
    /// joints that influence none).
    pub joint_bounds: Vec<Aabb>,
}

/// The skin vertices of `mesh`, or `None` when it has no joints and
/// weights.
pub(crate) fn skin_vertices(mesh: &MeshData) -> Option<Vec<GpuSkinVertex>> {
    let n = mesh.positions.len();
    (n > 0 && mesh.joints.len() == n && mesh.weights.len() == n).then(|| {
        mesh.joints
            .iter()
            .zip(&mesh.weights)
            .map(|(&joints, &weights)| GpuSkinVertex { joints, weights })
            .collect()
    })
}

/// Per joint: the bind-space box of the vertices it influences (weight
/// above zero).
pub(crate) fn joint_bounds(mesh: &MeshData) -> Vec<Aabb> {
    let mut out: Vec<Aabb> = Vec::new();
    for ((p, j), w) in mesh.positions.iter().zip(&mesh.joints).zip(&mesh.weights) {
        for k in 0..4 {
            if w[k] <= 0.0 {
                continue;
            }
            let joint = j[k] as usize;
            if joint >= out.len() {
                out.resize(joint + 1, Aabb::EMPTY);
            }
            out[joint] = out[joint].including(*p);
        }
    }
    out
}

/// The posed model-space bounds: each joint's box moved by its palette
/// matrix, united.
pub(crate) fn skinned_bounds(joint_bounds: &[Aabb], palette: &[Mat4]) -> Aabb {
    let mut out = Aabb::EMPTY;
    for (b, m) in joint_bounds.iter().zip(palette) {
        if b.is_empty() {
            continue;
        }
        let moved = b.transformed(&Affine3A::from_mat4(*m));
        out = Aabb::new(out.min.min(moved.min), out.max.max(moved.max));
    }
    out
}

/// A skinned entity drawn this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SkinnedDraw {
    /// First palette matrix in the joint buffer.
    pub base: u32,
    /// Posed bounds in the entity's model space.
    pub bounds: Aabb,
}

/// The frame's joint palettes and the GPU buffer they live in.
pub(crate) struct JointPalettes {
    data: Vec<Mat4>,
    uploaded: Vec<Mat4>,
    buffer: wgpu::Buffer,
    capacity: u64,
    /// Skinned entities drawn this frame.
    pub draws: HashMap<Entity, SkinnedDraw>,
    /// A skinned entity whose palette needs more joints than its skin has
    /// (warned once per entity).
    warned: HashSet<Entity>,
}

const MATRIX: u64 = 64;
const INITIAL_JOINTS: u64 = 64;

impl JointPalettes {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            data: Vec::new(),
            uploaded: Vec::new(),
            buffer: buffer(device, INITIAL_JOINTS),
            capacity: INITIAL_JOINTS,
            draws: HashMap::new(),
            warned: HashSet::new(),
        }
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Palette matrices this frame.
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    /// Builds this frame's palettes for every visible-candidate skinned
    /// entity (with a mesh that has joints and a loaded skin). Entities
    /// without one draw unskinned, in bind pose.
    #[profiling::function]
    pub(crate) fn prepare(&mut self, world: &World, meshes: &MeshCache) {
        self.data.clear();
        self.draws.clear();
        let Some(skins) = world.get_resource::<Assets<Skin>>() else {
            return;
        };
        type Skinned<'a> = Without<
            (
                Entity,
                &'a SkinnedMesh,
                &'a MeshRenderer,
                &'a GlobalTransform,
            ),
            &'a Hidden,
        >;
        let mut palette: Vec<Mat4> = Vec::new();
        let game_hidden = box_core::visibility::hidden_entities(world);
        for (e, sm, mr, gt) in world.query::<Skinned>().iter() {
            if game_hidden.contains(&e) {
                continue;
            }
            let Some(gpu) = meshes.get(mr.mesh.id()) else {
                continue;
            };
            let Some(gpu_skin) = &gpu.skin else { continue };
            let Some(skin) = skins.get(sm.skin) else {
                continue;
            };
            let count = gpu_skin.joint_bounds.len().max(skin.joint_count());
            if gpu_skin.joint_bounds.len() > skin.joint_count() && self.warned.insert(e) {
                tracing::warn!(
                    target: "box_render::skinning",
                    "a skinned mesh uses {} joints but its skin has {}; the extra joints stay in bind pose",
                    gpu_skin.joint_bounds.len(),
                    skin.joint_count()
                );
            }
            let mesh_inverse = gt.0.inverse();
            palette.clear();
            palette.extend((0..count).map(|i| {
                let ibm = skin.inverse_bind_matrices.get(i);
                let joint = sm
                    .joints
                    .get(i)
                    .and_then(|&j| world.get::<GlobalTransform>(j).map(|g| g.0));
                match (ibm, joint) {
                    (Some(ibm), Some(joint)) => joint_matrix(&mesh_inverse, &joint, ibm),
                    _ => Mat4::IDENTITY,
                }
            }));
            if !palette.iter().all(Mat4::is_finite) {
                continue;
            }
            let bounds = skinned_bounds(&gpu_skin.joint_bounds, &palette);
            let base = self.data.len() as u32;
            self.data.extend_from_slice(&palette);
            self.draws.insert(e, SkinnedDraw { base, bounds });
        }
        self.warned.retain(|e| world.contains(*e));
    }

    /// Writes the palettes when they changed (growing the buffer when too
    /// small). Returns the bytes written and whether the buffer was
    /// replaced (the instance bind group must be rebuilt).
    pub(crate) fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> (u64, bool) {
        let needed = self.data.len() as u64;
        let mut grown = false;
        if needed > self.capacity {
            self.capacity = (self.capacity * 2).max(needed);
            self.buffer = buffer(device, self.capacity);
            grown = true;
        }
        if !grown && self.data == self.uploaded {
            return (0, false);
        }
        if !self.data.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.data));
        }
        self.uploaded.clone_from(&self.data);
        (needed * MATRIX, grown)
    }
}

fn buffer(device: &wgpu::Device, joints: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("joint palettes"),
        size: joints * MATRIX,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    /// Two joints along y; the top half follows joint 1.
    fn bar() -> MeshData {
        let mut m = MeshData::cuboid(Vec3::new(0.2, 2.0, 0.2));
        m.joints = m.positions.iter().map(|_| [0, 1, 0, 0]).collect();
        m.weights = m
            .positions
            .iter()
            .map(|p| {
                if p.y > 0.0 {
                    [0.0, 1.0, 0.0, 0.0]
                } else {
                    [1.0, 0.0, 0.0, 0.0]
                }
            })
            .collect();
        m
    }

    #[test]
    fn skinned_bounds_contain_every_posed_vertex() {
        let mesh = bar();
        let boxes = joint_bounds(&mesh);
        assert_eq!(boxes.len(), 2);
        assert!(boxes[0].max.y <= 0.0 && boxes[1].min.y >= 0.0);
        // Joint 1 bends the top half 90 degrees about z at the origin and
        // lifts it.
        let bend = Mat4::from_rotation_translation(Quat::from_rotation_z(1.2), Vec3::Y * 0.5);
        let palette = [Mat4::IDENTITY, bend];
        let b = skinned_bounds(&boxes, &palette);
        for ((p, j), w) in mesh.positions.iter().zip(&mesh.joints).zip(&mesh.weights) {
            let posed: Vec3 = (0..4)
                .map(|k| palette[j[k] as usize].transform_point3(*p) * w[k])
                .sum();
            assert!(
                posed.cmpge(b.min - 1e-5).all() && posed.cmple(b.max + 1e-5).all(),
                "{posed} outside {b:?}"
            );
        }
        // The bend moves the top half sideways: the bounds follow it.
        assert!(b.min.x < -0.5, "{b:?}");
        assert!(skin_vertices(&mesh).is_some());
        assert!(skin_vertices(&MeshData::cuboid(Vec3::ONE)).is_none());
    }
}
