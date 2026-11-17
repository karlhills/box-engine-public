//! A minimal glTF 2.0 binary (`.glb`) writer for "Bake to mesh": one mesh,
//! one primitive per used material slot, and plain PBR materials, in the
//! form `box_assets`' glTF importer reads back exactly. [`write_skinned_glb`]
//! adds a skeleton, a skin and animations (the procedural test character);
//! [`write_scene_glb`] writes a hierarchy of named nodes with a mesh each
//! (levels authored outside the editor, such as the M8 game's).

use std::collections::BTreeMap;

use box_core::animation::{AnimationClip, Interpolation, Keyframes};
use box_core::material::{AlphaMode, StandardMaterial};
use box_core::mesh::{MeshData, MeshError};
use glam::{Quat, Vec3};
use serde_json::{Value as Json, json};

/// A material written into the file, for one material slot.
#[derive(Clone, Copy, Debug)]
pub struct GlbMaterial<'a> {
    /// The glTF material name (the importer's `Material/<name>` label,
    /// after sanitizing).
    pub name: &'a str,
    /// Factors, alpha mode (additive and multiply become `BLEND`) and
    /// sidedness are written; texture references are not (see [`write_glb`]).
    pub material: &'a StandardMaterial,
}

/// A written file.
#[derive(Clone, Debug, PartialEq)]
pub struct Glb {
    pub bytes: Vec<u8>,
    /// The material slot of each glTF primitive, in order: the imported
    /// mesh's group `i` (and `MeshAsset::materials[i]`) is slot `slots[i]`.
    /// Slots without triangles get no primitive.
    pub slots: Vec<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum GlbError {
    #[error("the mesh is invalid: {0}")]
    InvalidMesh(MeshError),
    #[error("the mesh has no triangles")]
    Empty,
    #[error("the mesh is too large for one buffer")]
    TooLarge,
    #[error("a skinned mesh needs joints, and joint indices and weights on every vertex")]
    NotSkinned,
    #[error("a joint index is out of range, or a joint's parent comes after it")]
    BadSkeleton,
    #[error("a node's parent comes after it")]
    BadHierarchy,
}

/// Writes `mesh` as a `.glb` with one mesh named `mesh_name` and one node.
///
/// Every material slot with triangles becomes a primitive holding only the
/// vertices it uses (re-indexed, `u32` indices), with `POSITION`,
/// `NORMAL`, `TANGENT` (when the mesh has tangents), `TEXCOORD_0..3` and
/// `COLOR_0` as float attributes. Primitive `i` uses glTF material
/// `slots[i]` when `materials` has that many entries. Materials carry base
/// colour, metallic, roughness, emissive (with
/// `KHR_materials_emissive_strength` above 1), alpha mode and
/// double-sidedness; texture references are not written (the caller
/// reports them). Positions are written as is, so a round trip through the
/// importer reproduces the triangles exactly.
pub fn write_glb(
    mesh: &MeshData,
    mesh_name: &str,
    materials: &[GlbMaterial<'_>],
) -> Result<Glb, GlbError> {
    let by_slot = slot_indices(mesh)?;
    let mut w = Writer::default();
    let (primitives, slots) = write_primitives(&mut w, mesh, by_slot, materials.len());

    let mut extensions_used = Vec::new();
    let materials: Vec<Json> = materials
        .iter()
        .map(|m| material_json(m, &mut extensions_used))
        .collect();
    if w.bin.len() > u32::MAX as usize - 64 {
        return Err(GlbError::TooLarge);
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "box-engine bake to mesh"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": mesh_name}],
        "meshes": [{"name": mesh_name, "primitives": primitives}],
        "buffers": [{"byteLength": w.bin.len()}],
        "bufferViews": w.views,
        "accessors": w.accessors,
    });
    if !materials.is_empty() {
        doc["materials"] = json!(materials);
    }
    if !extensions_used.is_empty() {
        doc["extensionsUsed"] = json!(extensions_used);
    }
    Ok(Glb {
        bytes: container(&doc, w.bin),
        slots,
    })
}

/// The mesh's indices by material slot (slots with triangles only).
fn slot_indices(mesh: &MeshData) -> Result<BTreeMap<u32, Vec<u32>>, GlbError> {
    mesh.validate().map_err(GlbError::InvalidMesh)?;
    let mut by_slot: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if mesh.groups.is_empty() {
        by_slot.insert(0, mesh.indices.clone());
    } else {
        for g in &mesh.groups {
            let start = (g.first_index as usize).min(mesh.indices.len());
            let end = (g.first_index as usize + g.index_count as usize).min(mesh.indices.len());
            by_slot
                .entry(g.material_slot)
                .or_default()
                .extend_from_slice(&mesh.indices[start..end]);
        }
    }
    by_slot.retain(|_, idx| idx.len() >= 3);
    if by_slot.is_empty() {
        return Err(GlbError::Empty);
    }
    Ok(by_slot)
}

/// One primitive per material slot (see [`write_glb`]); `JOINTS_0` and
/// `WEIGHTS_0` are written when the mesh has joints and weights.
fn write_primitives(
    w: &mut Writer,
    mesh: &MeshData,
    by_slot: BTreeMap<u32, Vec<u32>>,
    material_count: usize,
) -> (Vec<Json>, Vec<u32>) {
    let n = mesh.positions.len();
    let mut primitives = Vec::new();
    let mut slots = Vec::new();
    for (slot, indices) in by_slot {
        let mut remap = vec![u32::MAX; n];
        let mut used: Vec<usize> = Vec::new();
        let local: Vec<u32> = indices
            .iter()
            .map(|&i| {
                let i = i as usize;
                if remap[i] == u32::MAX {
                    remap[i] = used.len() as u32;
                    used.push(i);
                }
                remap[i]
            })
            .collect();
        let gather3 =
            |v: &[glam::Vec3]| -> Vec<[f32; 3]> { used.iter().map(|&i| v[i].to_array()).collect() };
        let mut attributes = serde_json::Map::new();
        let positions = gather3(&mesh.positions);
        let (min, max) = bounds(&positions);
        let pos = w.accessor(&positions, "VEC3", Some((min.to_vec(), max.to_vec())));
        attributes.insert("POSITION".into(), json!(pos));
        attributes.insert(
            "NORMAL".into(),
            json!(w.accessor(&gather3(&mesh.normals), "VEC3", None)),
        );
        if mesh.tangents.len() == n {
            let t: Vec<[f32; 4]> = used.iter().map(|&i| mesh.tangents[i].to_array()).collect();
            attributes.insert("TANGENT".into(), json!(w.accessor(&t, "VEC4", None)));
        }
        for (set, uv) in mesh.uvs.iter().enumerate() {
            let uv: Vec<[f32; 2]> = used.iter().map(|&i| uv[i].to_array()).collect();
            attributes.insert(
                format!("TEXCOORD_{set}"),
                json!(w.accessor(&uv, "VEC2", None)),
            );
        }
        if mesh.colors.len() == n {
            let c: Vec<[f32; 4]> = used.iter().map(|&i| mesh.colors[i]).collect();
            attributes.insert("COLOR_0".into(), json!(w.accessor(&c, "VEC4", None)));
        }
        if mesh.joints.len() == n && mesh.weights.len() == n {
            let j: Vec<[u16; 4]> = used.iter().map(|&i| mesh.joints[i]).collect();
            let wt: Vec<[f32; 4]> = used.iter().map(|&i| mesh.weights[i]).collect();
            attributes.insert("JOINTS_0".into(), json!(w.joints(&j)));
            attributes.insert("WEIGHTS_0".into(), json!(w.accessor(&wt, "VEC4", None)));
        }
        let idx = w.indices(&local);
        let mut prim = json!({"attributes": attributes, "indices": idx, "mode": 4});
        if (slot as usize) < material_count {
            prim["material"] = json!(slot);
        }
        primitives.push(prim);
        slots.push(slot);
    }

    (primitives, slots)
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    (min, max)
}

fn material_json(m: &GlbMaterial<'_>, extensions_used: &mut Vec<&'static str>) -> Json {
    let mat = m.material;
    let c = mat.base_color;
    let e = [mat.emissive.r, mat.emissive.g, mat.emissive.b].map(|v| v.max(0.0));
    let strength = e.iter().copied().fold(0.0f32, f32::max);
    let mut out = json!({
        "name": m.name,
        "pbrMetallicRoughness": {
            "baseColorFactor": [c.r, c.g, c.b, c.a],
            "metallicFactor": mat.metallic,
            "roughnessFactor": mat.roughness,
        },
        "doubleSided": mat.double_sided,
    });
    if strength > 1.0 {
        out["emissiveFactor"] = json!(e.map(|v| v / strength));
        out["extensions"] =
            json!({"KHR_materials_emissive_strength": {"emissiveStrength": strength}});
        if !extensions_used.contains(&"KHR_materials_emissive_strength") {
            extensions_used.push("KHR_materials_emissive_strength");
        }
    } else if strength > 0.0 {
        out["emissiveFactor"] = json!(e);
    }
    match mat.alpha_mode {
        AlphaMode::Opaque => {}
        AlphaMode::Mask { cutoff } => {
            out["alphaMode"] = json!("MASK");
            out["alphaCutoff"] = json!(cutoff);
        }
        // glTF has no additive or multiply blending: blended is closest.
        AlphaMode::Blend | AlphaMode::Additive | AlphaMode::Multiply => {
            out["alphaMode"] = json!("BLEND")
        }
    }
    out
}

#[derive(Default)]
struct Writer {
    bin: Vec<u8>,
    views: Vec<Json>,
    accessors: Vec<Json>,
}

impl Writer {
    fn view(&mut self, bytes: &[u8], target: u32) -> usize {
        let view = self.data_view(bytes);
        self.views[view]["target"] = json!(target);
        view
    }

    /// A buffer view without a target (skin and animation data).
    fn data_view(&mut self, bytes: &[u8]) -> usize {
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        self.views.push(json!({
            "buffer": 0,
            "byteOffset": offset,
            "byteLength": bytes.len(),
        }));
        self.views.len() - 1
    }

    /// `JOINTS_0`: unsigned short `VEC4`.
    fn joints(&mut self, data: &[[u16; 4]]) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flat_map(|v| v.iter().flat_map(|j| j.to_le_bytes()))
            .collect();
        let view = self.view(&bytes, 34962);
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": 5123,
            "count": data.len(),
            "type": "VEC4",
        }));
        self.accessors.len() - 1
    }

    /// A float accessor of non-vertex data (`SCALAR` times get min/max, as
    /// glTF requires for animation inputs).
    fn data<const N: usize>(&mut self, data: &[[f32; N]], kind: &str) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flat_map(|v| v.iter().flat_map(|f| f.to_le_bytes()))
            .collect();
        let view = self.data_view(&bytes);
        let mut a = json!({
            "bufferView": view,
            "componentType": 5126,
            "count": data.len(),
            "type": kind,
        });
        if N == 1 {
            let min = data.iter().map(|v| v[0]).fold(f32::INFINITY, f32::min);
            let max = data.iter().map(|v| v[0]).fold(f32::NEG_INFINITY, f32::max);
            a["min"] = json!([min]);
            a["max"] = json!([max]);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    /// A float vertex attribute accessor.
    fn accessor<const N: usize>(
        &mut self,
        data: &[[f32; N]],
        kind: &str,
        min_max: Option<(Vec<f32>, Vec<f32>)>,
    ) -> usize {
        let bytes: Vec<u8> = data
            .iter()
            .flat_map(|v| v.iter().flat_map(|f| f.to_le_bytes()))
            .collect();
        let view = self.view(&bytes, 34962);
        let mut a = json!({
            "bufferView": view,
            "componentType": 5126,
            "count": data.len(),
            "type": kind,
        });
        if let Some((min, max)) = min_max {
            a["min"] = json!(min);
            a["max"] = json!(max);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|i| i.to_le_bytes()).collect();
        let view = self.view(&bytes, 34963);
        self.accessors.push(json!({
            "bufferView": view,
            "componentType": 5125,
            "count": data.len(),
            "type": "SCALAR",
        }));
        self.accessors.len() - 1
    }
}

/// A joint of a skeleton written by [`write_skinned_glb`]: a node with a
/// name, a parent joint and a bind-pose local transform.
#[derive(Clone, Debug)]
pub struct GlbJoint<'a> {
    pub name: &'a str,
    /// Index of the parent joint; `None` for a skeleton root.
    pub parent: Option<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
}

/// Writes a skinned character as a `.glb`: a root node named `root_name`
/// with the mesh node (`mesh_name`, skinned by skin 0) and the joint nodes
/// beneath it, the skin (inverse bind matrices from the joints' bind
/// poses, mesh space = root space), and one glTF animation per clip.
///
/// Mesh primitives and materials are written as by [`write_glb`]; the mesh
/// must have `joints` (indices into `joints`) and `weights`. Clip channel
/// targets name joints (channels naming no joint are skipped); step,
/// linear and cubic-spline samplers are written as they are.
pub fn write_skinned_glb(
    mesh: &MeshData,
    mesh_name: &str,
    root_name: &str,
    materials: &[GlbMaterial<'_>],
    joints: &[GlbJoint<'_>],
    clips: &[(&str, &AnimationClip)],
) -> Result<Glb, GlbError> {
    let n = mesh.positions.len();
    if joints.is_empty() || mesh.joints.len() != n || mesh.weights.len() != n {
        return Err(GlbError::NotSkinned);
    }
    if mesh
        .joints
        .iter()
        .flatten()
        .any(|&j| j as usize >= joints.len())
        || joints
            .iter()
            .enumerate()
            .any(|(i, j)| j.parent.is_some_and(|p| p >= i))
    {
        return Err(GlbError::BadSkeleton);
    }
    let by_slot = slot_indices(mesh)?;
    let mut w = Writer::default();
    let (primitives, slots) = write_primitives(&mut w, mesh, by_slot, materials.len());

    // Nodes: 0 root, 1 mesh, 2.. joints.
    let node_of = |joint: usize| joint + 2;
    let mut globals: Vec<glam::Mat4> = Vec::with_capacity(joints.len());
    let mut nodes = vec![
        json!({"name": root_name, "children": [1]}),
        json!({"name": mesh_name, "mesh": 0, "skin": 0}),
    ];
    for (i, j) in joints.iter().enumerate() {
        let local = glam::Mat4::from_rotation_translation(j.rotation, j.translation);
        let global = match j.parent {
            Some(p) => globals[p] * local,
            None => local,
        };
        globals.push(global);
        let r = j.rotation.normalize();
        nodes.push(json!({
            "name": j.name,
            "translation": j.translation.to_array(),
            "rotation": [r.x, r.y, r.z, r.w],
        }));
        let parent_node = j.parent.map_or(0, node_of);
        let children = nodes[parent_node]
            .as_object_mut()
            .expect("nodes are objects")
            .entry("children")
            .or_insert_with(|| json!([]));
        if let Some(c) = children.as_array_mut() {
            c.push(json!(node_of(i)));
        }
    }
    let ibm: Vec<[f32; 16]> = globals
        .iter()
        .map(|g| g.inverse().to_cols_array())
        .collect();
    let skin = json!({
        "joints": (0..joints.len()).map(node_of).collect::<Vec<_>>(),
        "inverseBindMatrices": w.data(&ibm, "MAT4"),
        "skeleton": node_of(0),
    });

    let mut animations = Vec::new();
    for (name, clip) in clips {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for c in &clip.channels {
            let Some(joint) = joints.iter().position(|j| j.name == c.target) else {
                continue;
            };
            let times: Vec<[f32; 1]> = c.times.iter().map(|&t| [t]).collect();
            let input = w.data(&times, "SCALAR");
            let (path, output) = match &c.values {
                Keyframes::Translation(v) => {
                    let v: Vec<[f32; 3]> = v.iter().map(|v| v.to_array()).collect();
                    ("translation", w.data(&v, "VEC3"))
                }
                Keyframes::Scale(v) => {
                    let v: Vec<[f32; 3]> = v.iter().map(|v| v.to_array()).collect();
                    ("scale", w.data(&v, "VEC3"))
                }
                Keyframes::Rotation(v) => {
                    let v: Vec<[f32; 4]> = v.iter().map(|q| q.to_array()).collect();
                    ("rotation", w.data(&v, "VEC4"))
                }
            };
            let interpolation = match c.interpolation {
                Interpolation::Step => "STEP",
                Interpolation::Linear => "LINEAR",
                Interpolation::CubicSpline => "CUBICSPLINE",
            };
            channels.push(json!({
                "sampler": samplers.len(),
                "target": {"node": node_of(joint), "path": path},
            }));
            samplers.push(json!({
                "input": input,
                "output": output,
                "interpolation": interpolation,
            }));
        }
        animations.push(json!({"name": name, "samplers": samplers, "channels": channels}));
    }

    let mut extensions_used = Vec::new();
    let materials: Vec<Json> = materials
        .iter()
        .map(|m| material_json(m, &mut extensions_used))
        .collect();
    if w.bin.len() > u32::MAX as usize - 64 {
        return Err(GlbError::TooLarge);
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "box-engine"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": nodes,
        "meshes": [{"name": mesh_name, "primitives": primitives}],
        "skins": [skin],
        "buffers": [{"byteLength": w.bin.len()}],
        "bufferViews": w.views,
        "accessors": w.accessors,
    });
    if !animations.is_empty() {
        doc["animations"] = json!(animations);
    }
    if !materials.is_empty() {
        doc["materials"] = json!(materials);
    }
    if !extensions_used.is_empty() {
        doc["extensionsUsed"] = json!(extensions_used);
    }
    Ok(Glb {
        bytes: container(&doc, w.bin),
        slots,
    })
}

/// A node written by [`write_scene_glb`]: a name, a parent node, a local
/// transform and an optional mesh.
#[derive(Clone, Debug)]
pub struct GlbNode<'a> {
    /// The glTF node name (the imported scene entity's name); a node with a
    /// mesh also names its glTF mesh (the importer's `Mesh/<name>` label).
    pub name: &'a str,
    /// Index of the parent node in the list; `None` for a child of the root.
    pub parent: Option<usize>,
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
    /// Material slots of the mesh index the file's `materials`.
    pub mesh: Option<&'a MeshData>,
}

/// Writes a hierarchy of named nodes as a `.glb` (a level or a prop set made
/// outside the editor): a root node named `root_name` with `nodes` beneath
/// it, one glTF mesh per node that has one (primitives per used material
/// slot, as by [`write_glb`]), and `materials` shared by all of them.
/// `slots` lists the used slots of every mesh in node order.
pub fn write_scene_glb(
    root_name: &str,
    nodes: &[GlbNode<'_>],
    materials: &[GlbMaterial<'_>],
) -> Result<Glb, GlbError> {
    if nodes
        .iter()
        .enumerate()
        .any(|(i, n)| n.parent.is_some_and(|p| p >= i))
    {
        return Err(GlbError::BadHierarchy);
    }
    let mut w = Writer::default();
    let mut json_nodes = vec![json!({"name": root_name})];
    let mut meshes = Vec::new();
    let mut all_slots = Vec::new();
    for n in nodes {
        let r = n.rotation.normalize();
        let mut node = json!({
            "name": n.name,
            "translation": n.translation.to_array(),
            "rotation": [r.x, r.y, r.z, r.w],
            "scale": n.scale.to_array(),
        });
        if let Some(mesh) = n.mesh {
            let by_slot = slot_indices(mesh)?;
            let (primitives, slots) = write_primitives(&mut w, mesh, by_slot, materials.len());
            node["mesh"] = json!(meshes.len());
            meshes.push(json!({"name": n.name, "primitives": primitives}));
            all_slots.extend(slots);
        }
        json_nodes.push(node);
    }
    for (i, n) in nodes.iter().enumerate() {
        let parent = n.parent.map_or(0, |p| p + 1);
        let children = json_nodes[parent]
            .as_object_mut()
            .expect("nodes are objects")
            .entry("children")
            .or_insert_with(|| json!([]));
        if let Some(c) = children.as_array_mut() {
            c.push(json!(i + 1));
        }
    }
    let mut extensions_used = Vec::new();
    let materials: Vec<Json> = materials
        .iter()
        .map(|m| material_json(m, &mut extensions_used))
        .collect();
    if w.bin.len() > u32::MAX as usize - 64 {
        return Err(GlbError::TooLarge);
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "box-engine"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": json_nodes,
        "buffers": [{"byteLength": w.bin.len()}],
        "bufferViews": w.views,
        "accessors": w.accessors,
    });
    if !meshes.is_empty() {
        doc["meshes"] = json!(meshes);
    }
    if !materials.is_empty() {
        doc["materials"] = json!(materials);
    }
    if !extensions_used.is_empty() {
        doc["extensionsUsed"] = json!(extensions_used);
    }
    Ok(Glb {
        bytes: container(&doc, w.bin),
        slots: all_slots,
    })
}

/// The GLB container: header, JSON chunk (space padded), BIN chunk.
fn container(doc: &Json, mut bin: Vec<u8>) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).unwrap_or_default();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::mesh::MaterialGroup;
    use glam::Vec3;

    #[test]
    fn writes_a_valid_container_with_one_primitive_per_used_slot() {
        let mut mesh = MeshData::cuboid(Vec3::ONE);
        mesh.groups = vec![
            MaterialGroup {
                first_index: 0,
                index_count: 12,
                material_slot: 1,
            },
            MaterialGroup {
                first_index: 12,
                index_count: 24,
                material_slot: 0,
            },
        ];
        let stone = StandardMaterial::default();
        let mats = [
            GlbMaterial {
                name: "Stone",
                material: &stone,
            },
            GlbMaterial {
                name: "Trim",
                material: &stone,
            },
        ];
        let glb = write_glb(&mesh, "Block", &mats).unwrap();
        assert_eq!(glb.slots, [0, 1]);
        assert_eq!(&glb.bytes[0..4], b"glTF");
        let total = u32::from_le_bytes(glb.bytes[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.bytes.len());
        assert!(total.is_multiple_of(4));
        let json_len = u32::from_le_bytes(glb.bytes[12..16].try_into().unwrap()) as usize;
        let doc: Json = serde_json::from_slice(&glb.bytes[20..20 + json_len]).unwrap();
        let prims = doc["meshes"][0]["primitives"].as_array().unwrap();
        assert_eq!(prims.len(), 2);
        assert_eq!(prims[0]["material"], 0);
        assert_eq!(prims[1]["material"], 1);
        assert_eq!(doc["materials"][1]["name"], "Trim");

        assert!(matches!(
            write_glb(&MeshData::default(), "x", &[]),
            Err(GlbError::Empty)
        ));
    }

    #[test]
    fn writes_a_node_hierarchy_with_one_mesh_per_node() {
        let floor = MeshData::cuboid(Vec3::new(4.0, 0.5, 4.0));
        let mut post = MeshData::cuboid(Vec3::ONE);
        post.groups = vec![MaterialGroup {
            first_index: 0,
            index_count: 36,
            material_slot: 1,
        }];
        let stone = StandardMaterial::default();
        let mats = [
            GlbMaterial {
                name: "Stone",
                material: &stone,
            },
            GlbMaterial {
                name: "Wood",
                material: &stone,
            },
        ];
        let node = |name, parent, mesh| GlbNode {
            name,
            parent,
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            mesh,
        };
        let nodes = [
            node("Floor", None, Some(&floor)),
            node("Props", None, None),
            node("Post", Some(1), Some(&post)),
        ];
        let glb = write_scene_glb("Level", &nodes, &mats).unwrap();
        assert_eq!(glb.slots, [0, 1]);
        let json_len = u32::from_le_bytes(glb.bytes[12..16].try_into().unwrap()) as usize;
        let doc: Json = serde_json::from_slice(&glb.bytes[20..20 + json_len]).unwrap();
        assert_eq!(doc["nodes"][0]["name"], "Level");
        assert_eq!(doc["nodes"][0]["children"], json!([1, 2]));
        assert_eq!(doc["nodes"][2]["children"], json!([3]));
        assert_eq!(doc["nodes"][3]["mesh"], 1);
        assert_eq!(doc["meshes"][1]["name"], "Post");
        assert_eq!(doc["meshes"][1]["primitives"][0]["material"], 1);
        assert_eq!(doc["nodes"][1]["translation"], json!([1.0, 2.0, 3.0]));
        let bad = [node("A", Some(0), None)];
        assert!(matches!(
            write_scene_glb("Level", &bad, &mats),
            Err(GlbError::BadHierarchy)
        ));
    }
}
