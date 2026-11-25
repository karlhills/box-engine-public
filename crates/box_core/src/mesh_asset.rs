//! [`MeshAsset`]: a saved component that draws a mesh asset with material
//! assets (imported models), as `Primitive` does for built-in shapes.
//! [`MaterialRef`]: a saved link from a `Primitive` entity to a material
//! asset file.

use std::sync::Arc;

use crate::asset_ref::AssetRef;
use crate::assets::Handle;
use crate::mesh::MeshData;
use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};

/// Draws the mesh asset `mesh` with `materials`, one per material group of
/// the mesh (`MeshData::groups`, by slot). An empty list uses the materials
/// an imported mesh (`model.glb#Mesh/<name>`) was imported with, in slot
/// order (other meshes: the default material). A missing or empty material
/// slot in a non-empty list uses the default material; a mesh that is not loaded yet draws nothing
/// until it is, and a mesh that failed to load draws a placeholder.
///
/// The renderer's `MeshRenderer` for this entity is derived runtime data,
/// built by the asset glue from these references; it is never saved.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct MeshAsset {
    /// The mesh, usually a glTF sub-asset such as `models/tower.glb#Mesh/Roof`.
    pub mesh: AssetRef,
    /// Material assets by material group slot (glTF sub-assets such as
    /// `models/tower.glb#Material/Stone`, or `.material.json` files). Empty:
    /// the imported mesh's own materials.
    pub materials: Vec<AssetRef>,
    /// Whether the mesh casts shadows.
    pub cast_shadows: bool,
}

impl Default for MeshAsset {
    fn default() -> Self {
        Self {
            mesh: AssetRef::NONE,
            materials: Vec::new(),
            cast_shadows: true,
        }
    }
}

/// Runtime-only link from a generator entity (a `Primitive` or any other
/// registered generator) to the mesh the generator runtime made for it, so
/// systems below the generator crate (physics: `ColliderShape::Mesh` and
/// `ConvexHull`) can use generated meshes like `MeshAsset` meshes.
///
/// Inserted empty when the entity becomes a generator entity, set after
/// each flush (`mesh` is `None` when the generator emitted nothing) and
/// removed with the generator. Never saved (not registered); never insert it
/// by hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeneratedMesh {
    pub mesh: Option<Handle<MeshData>>,
    /// The same surface as a regular height grid, when the generator gave
    /// one (terrain chunks): `Mesh` colliders use it as a heightfield,
    /// which is much cheaper to build and query than the triangles.
    pub heights: Option<Arc<HeightGrid>>,
}

/// A surface as a regular grid of heights in its entity's space: samples
/// `columns` along X and `rows` along Z, `cell_size` apart, starting at
/// `origin` (x, z). Each cell is two triangles split along one diagonal,
/// as the drawn mesh splits it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeightGrid {
    pub origin: glam::Vec2,
    pub cell_size: f32,
    /// Samples along X (at least 2).
    pub columns: u32,
    /// Samples along Z (at least 2).
    pub rows: u32,
    /// Row by row (Z), X within a row: `heights[row * columns + column]`.
    pub heights: Vec<f32>,
    /// Per cell (row by row, `(rows - 1) * (columns - 1)`): the cell is
    /// split from its (x0, z0) corner to its (x1, z1) corner; otherwise from
    /// (x1, z0) to (x0, z1).
    pub main_diagonal: Vec<bool>,
}

impl HeightGrid {
    /// Whether the sizes agree with each other.
    pub fn is_valid(&self) -> bool {
        let (c, r) = (self.columns as usize, self.rows as usize);
        c >= 2
            && r >= 2
            && self.heights.len() == c * r
            && self.main_diagonal.len() == (c - 1) * (r - 1)
            && self.cell_size > 0.0
            && self.heights.iter().all(|h| h.is_finite())
    }
}

/// Draws this entity's `Primitive` with a material asset (a
/// `.material.json` file, standard or shader, or a glTF material) instead
/// of its inline material, following edits to the file (hot reload).
///
/// Precedence on a `Primitive` entity: `MaterialRef` (when it names an
/// asset), then an inline `ShaderMaterial`, then an inline
/// `StandardMaterial`, then the default material. While the asset loads
/// the inline material is drawn; an asset that cannot be loaded draws the
/// placeholder material. Ignored on entities without a `Primitive`
/// (`MeshAsset` has its own material slots).
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct MaterialRef {
    /// The material asset, such as `materials/stone.material.json`.
    pub material: AssetRef,
}

impl MaterialRef {
    /// A link to `material`.
    pub fn new(material: AssetRef) -> Self {
        Self { material }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::Registry;

    #[test]
    fn round_trips_through_the_scene_json_form() {
        let registry = Registry::with_core_types();
        let m = MeshAsset {
            mesh: AssetRef::path("models/tower.glb#Mesh/Roof"),
            materials: vec![
                AssetRef::path("models/tower.glb#Material/Stone"),
                AssetRef::NONE,
            ],
            cast_shadows: false,
        };
        let json = registry.value_to_json(&m).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "mesh": {"label": "Mesh/Roof", "path": "models/tower.glb"},
                "materials": [{"label": "Material/Stone", "path": "models/tower.glb"}, null],
                "cast_shadows": false
            })
        );
        let ty = registry.component("MeshAsset").unwrap();
        let back = registry.value_from_json(ty.type_id(), &json).unwrap();
        let back = <MeshAsset as bevy_reflect::FromReflect>::from_reflect(back.as_ref()).unwrap();
        assert_eq!(back, m);
        // Agents may write plain path strings.
        let short = serde_json::json!({
            "mesh": "models/tower.glb#Mesh/Roof",
            "materials": ["materials/stone.material.json"],
            "cast_shadows": true
        });
        assert!(registry.value_from_json(ty.type_id(), &short).is_ok());
    }

    #[test]
    fn material_ref_round_trips() {
        let registry = Registry::with_core_types();
        let m = MaterialRef::new(AssetRef::path("materials/stone.material.json"));
        let json = registry.value_to_json(&m).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"material": {"path": "materials/stone.material.json"}})
        );
        let ty = registry.component("MaterialRef").unwrap();
        let back = registry.value_from_json(ty.type_id(), &json).unwrap();
        let back = <MaterialRef as bevy_reflect::FromReflect>::from_reflect(back.as_ref()).unwrap();
        assert_eq!(back, m);
    }
}
