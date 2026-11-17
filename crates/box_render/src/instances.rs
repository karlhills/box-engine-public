//! Per-object instance data for instanced drawing.
//!
//! Every drawn entity owns a persistent slot in the `objects` storage
//! buffer (`Object` in object.wgsl: model and normal matrices, palette
//! offset of a skinned object, outline id, winding sign). A slot is rewritten only when the entity's world transform
//! or selection state changes, so an idle scene uploads nothing. Each frame,
//! culling and batching produce `draw_slots`, the slots of every instanced
//! draw back to back: a draw of `count` instances from `first` reads
//! `objects[draw_slots[first + i]]`. That list is uploaded only when it
//! differs from the previous frame's.

use std::collections::HashMap;

use box_core::world::Entity;
use glam::{Affine3A, Mat3, Mat4};

use crate::pipelines::Layouts;
use crate::uniforms::ObjectUniform;

const INITIAL_SLOTS: u64 = 256;
const OBJECT_SIZE: u64 = std::mem::size_of::<ObjectUniform>() as u64;
/// Slots per dirty-tracking chunk (9 KiB of instance data): consecutive
/// dirty chunks are written with one `write_buffer`.
const CHUNK: usize = 64;

struct SlotState {
    entity: Entity,
    transform: Affine3A,
    outline: f32,
    /// First joint palette matrix (skinned objects; 0 otherwise).
    palette: u32,
    /// Frame the entity was last seen; slots not seen this frame are freed.
    seen: u64,
}

/// Persistent instance slots and the per-frame draw slot list.
pub(crate) struct InstanceStore {
    by_entity: HashMap<Entity, u32>,
    slots: Vec<Option<SlotState>>,
    free: Vec<u32>,
    /// CPU copy of the `objects` buffer, by slot.
    data: Vec<ObjectUniform>,
    /// Chunks of `CHUNK` slots written since the last upload.
    dirty: Vec<bool>,
    any_dirty: bool,
    frame: u64,
    objects: wgpu::Buffer,
    object_capacity: u64,
    /// This frame's draw slot list and the last one uploaded.
    pub(crate) list: Vec<u32>,
    uploaded_list: Vec<u32>,
    list_buffer: wgpu::Buffer,
    list_capacity: u64,
    pub(crate) bind_group: wgpu::BindGroup,
    /// Slots rewritten by the last `upload` (statistics, tests).
    pub(crate) written: u32,
}

/// What an upload cost.
#[derive(Default)]
pub(crate) struct InstanceUpload {
    pub bytes: u64,
    pub reallocations: u32,
}

impl InstanceStore {
    pub(crate) fn new(device: &wgpu::Device, layouts: &Layouts, joints: &wgpu::Buffer) -> Self {
        let objects = storage_buffer(device, "instance objects", INITIAL_SLOTS * OBJECT_SIZE);
        let list_buffer = storage_buffer(device, "instance draw slots", INITIAL_SLOTS * 4);
        let bind_group = bind_group(device, layouts, &objects, &list_buffer, joints);
        Self {
            by_entity: HashMap::new(),
            slots: Vec::new(),
            free: Vec::new(),
            data: Vec::new(),
            dirty: Vec::new(),
            any_dirty: false,
            frame: 0,
            objects,
            object_capacity: INITIAL_SLOTS,
            list: Vec::new(),
            uploaded_list: Vec::new(),
            list_buffer,
            list_capacity: INITIAL_SLOTS,
            bind_group,
            written: 0,
        }
    }

    /// Starts a frame: every entity must be seen again with
    /// [`slot`](Self::slot) to keep its slot.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
        self.list.clear();
    }

    /// The slot of `entity` drawn at `transform` with selection outline id
    /// `outline` (1 primary, 0.5 secondary, 0 none) and, for a skinned
    /// object, its first joint palette matrix `palette` (0 otherwise). Its
    /// instance data is recomputed only when one of them changed.
    pub(crate) fn slot(
        &mut self,
        entity: Entity,
        transform: &Affine3A,
        outline: f32,
        palette: u32,
    ) -> u32 {
        let frame = self.frame;
        let slot = match self.by_entity.get(&entity) {
            Some(&slot) => {
                let state = self.slots[slot as usize]
                    .as_mut()
                    .expect("mapped slots are live");
                state.seen = frame;
                if state.transform == *transform
                    && state.outline == outline
                    && state.palette == palette
                {
                    return slot;
                }
                state.transform = *transform;
                state.outline = outline;
                state.palette = palette;
                slot
            }
            None => {
                let slot = self.free.pop().unwrap_or_else(|| {
                    self.slots.push(None);
                    self.data.push(bytemuck::Zeroable::zeroed());
                    self.slots.len() as u32 - 1
                });
                self.slots[slot as usize] = Some(SlotState {
                    entity,
                    transform: *transform,
                    outline,
                    palette,
                    seen: frame,
                });
                self.by_entity.insert(entity, slot);
                slot
            }
        };
        let mirrored = transform.matrix3.determinant() < 0.0;
        self.data[slot as usize] = ObjectUniform {
            model: Mat4::from(*transform).to_cols_array_2d(),
            normal_matrix: normal_matrix(transform).to_cols_array_2d(),
            params: [
                // Bit-cast in skin.wgsl.
                f32::from_bits(palette),
                0.0,
                outline_mask_id(slot + 1, outline),
                if mirrored { -1.0 } else { 1.0 },
            ],
        };
        let chunk = slot as usize / CHUNK;
        if chunk >= self.dirty.len() {
            self.dirty.resize(chunk + 1, false);
        }
        self.dirty[chunk] = true;
        self.any_dirty = true;
        slot
    }

    /// Frees the slots of entities not seen this frame (despawned, hidden,
    /// without a mesh). Their data stays in the buffer until reused.
    pub(crate) fn end_frame(&mut self) {
        if self.by_entity.len() == self.slots.len() - self.free.len()
            && self.slots.iter().flatten().all(|s| s.seen == self.frame)
        {
            return;
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.as_ref().is_some_and(|s| s.seen != self.frame) {
                let s = slot.take().expect("checked");
                self.by_entity.remove(&s.entity);
                self.free.push(i as u32);
            }
        }
    }

    /// Writes changed instance data and, when it changed, the draw slot
    /// list; grows (and fully rewrites) either buffer when it is too small.
    /// `joints` is the joint palette buffer; `joints_replaced` says it is a
    /// new buffer since the last upload.
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: &Layouts,
        joints: &wgpu::Buffer,
        joints_replaced: bool,
    ) -> InstanceUpload {
        let mut out = InstanceUpload::default();
        self.written = 0;
        let mut rebind = joints_replaced;
        let slots = self.data.len() as u64;
        if slots > self.object_capacity {
            self.object_capacity = (self.object_capacity * 2).max(slots);
            self.objects = storage_buffer(
                device,
                "instance objects",
                self.object_capacity * OBJECT_SIZE,
            );
            self.dirty.iter_mut().for_each(|d| *d = true);
            self.any_dirty = true;
            rebind = true;
            out.reallocations += 1;
        }
        let list = self.list.len() as u64;
        let list_grew = list > self.list_capacity;
        if list_grew {
            self.list_capacity = (self.list_capacity * 2).max(list);
            self.list_buffer =
                storage_buffer(device, "instance draw slots", self.list_capacity * 4);
            rebind = true;
            out.reallocations += 1;
        }
        if rebind {
            self.bind_group = bind_group(device, layouts, &self.objects, &self.list_buffer, joints);
        }
        if std::mem::take(&mut self.any_dirty) {
            let mut chunk = 0;
            while chunk < self.dirty.len() {
                if !self.dirty[chunk] {
                    chunk += 1;
                    continue;
                }
                let start = chunk;
                while chunk < self.dirty.len() && self.dirty[chunk] {
                    self.dirty[chunk] = false;
                    chunk += 1;
                }
                let range = &self.data[start * CHUNK..(chunk * CHUNK).min(self.data.len())];
                queue.write_buffer(
                    &self.objects,
                    (start * CHUNK) as u64 * OBJECT_SIZE,
                    bytemuck::cast_slice(range),
                );
                out.bytes += std::mem::size_of_val(range) as u64;
                self.written += range.len() as u32;
            }
        }
        if list_grew || self.list != self.uploaded_list {
            if !self.list.is_empty() {
                queue.write_buffer(&self.list_buffer, 0, bytemuck::cast_slice(&self.list));
                out.bytes += list * 4;
            }
            self.uploaded_list.clone_from(&self.list);
        }
        out
    }

    /// Live slots.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_entity.len()
    }
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn bind_group(
    device: &wgpu::Device,
    layouts: &Layouts,
    objects: &wgpu::Buffer,
    list: &wgpu::Buffer,
    joints: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("instance bind group"),
        layout: &layouts.object,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: objects.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: list.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: joints.as_entire_binding(),
            },
        ],
    })
}

/// The selection outline mask value of the object in slot
/// `slot_plus_one - 1` with outline id `outline_id` (1 primary, 0.5
/// secondary, 0 none): (slot + 1) * 2, plus 1 for the primary selection, so
/// each selected object is its own silhouette (outline_mask.wgsl). Exact in
/// an f32 for fewer than 2^22 objects.
pub(crate) fn outline_mask_id(slot_plus_one: u32, outline_id: f32) -> f32 {
    if outline_id <= 0.0 {
        return 0.0;
    }
    (slot_plus_one * 2 + u32::from(outline_id > 0.75)) as f32
}

pub(crate) fn normal_matrix(a: &Affine3A) -> Mat4 {
    let m = Mat3::from(a.matrix3);
    let n = if m.determinant().abs() > 1e-12 {
        m.inverse().transpose()
    } else {
        m
    };
    Mat4::from_mat3(n)
}

/// One instanced draw: the draw item it was made from (mesh, index range,
/// material, pipeline) and its instances, `count` slots from `first` in the
/// draw slot list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Batch {
    pub item: u32,
    pub first: u32,
    pub count: u32,
}

/// Appends the slots of `order` (draw item indices) to `list`, merging
/// consecutive items with equal `key` into one batch when `merge` is set.
/// Items whose key is `None` are skipped.
pub(crate) fn batch<K: PartialEq>(
    order: &[u32],
    key: impl Fn(u32) -> Option<(K, u32)>,
    merge: bool,
    list: &mut Vec<u32>,
    out: &mut Vec<Batch>,
) {
    out.clear();
    let mut current: Option<K> = None;
    for &i in order {
        let Some((k, slot)) = key(i) else { continue };
        let first = list.len() as u32;
        list.push(slot);
        match out.last_mut() {
            Some(b) if merge && current.as_ref() == Some(&k) => b.count += 1,
            _ => {
                out.push(Batch {
                    item: i,
                    first,
                    count: 1,
                });
                current = Some(k);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_merge_runs_of_equal_keys() {
        // Items 0..6 with keys a a b - a a (item 3 skipped) and slots 10+i.
        let keys = ['a', 'a', 'b', '-', 'a', 'a'];
        let mut list = Vec::new();
        let mut out = Vec::new();
        let key = |i: u32| {
            let k = keys[i as usize];
            (k != '-').then_some((k, 10 + i))
        };
        batch(&[0, 1, 2, 3, 4, 5], key, true, &mut list, &mut out);
        assert_eq!(list, [10, 11, 12, 14, 15]);
        assert_eq!(
            out,
            [
                Batch {
                    item: 0,
                    first: 0,
                    count: 2
                },
                Batch {
                    item: 2,
                    first: 2,
                    count: 1
                },
                Batch {
                    item: 4,
                    first: 3,
                    count: 2
                },
            ]
        );
        // Without merging every item is its own draw, appended after.
        batch(&[1, 0], key, false, &mut list, &mut out);
        assert_eq!(list[5..], [11, 10]);
        assert_eq!(out.len(), 2);
        assert_eq!((out[1].item, out[1].first), (0, 6));
    }

    #[test]
    fn outline_ids() {
        assert_eq!(outline_mask_id(1, 1.0), 3.0);
        assert_eq!(outline_mask_id(4, 0.5), 8.0);
        assert_eq!(outline_mask_id(4, 0.0), 0.0);
    }

    #[test]
    fn normal_matrix_handles_non_uniform_scale() {
        use glam::Vec3;
        let a = Affine3A::from_scale(Vec3::new(2.0, 1.0, 1.0));
        let n = normal_matrix(&a);
        let v = n.transform_vector3(Vec3::new(1.0, 1.0, 0.0)).normalize();
        assert!((v - Vec3::new(0.5, 1.0, 0.0).normalize()).length() < 1e-5);
        let degenerate = normal_matrix(&Affine3A::from_scale(Vec3::ZERO));
        assert!(degenerate.is_finite());
    }
}
