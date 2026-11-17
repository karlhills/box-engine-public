//! Skins: the [`Skin`] asset (inverse bind matrices of a skeleton's joints),
//! the saved [`SkinBinding`] and the runtime [`SkinnedMesh`].

use glam::{Affine3A, Mat4};

use crate::asset_ref::AssetRef;
use crate::assets::Handle;
use crate::identity::Uid;
use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};
use crate::world::Entity;

/// A skeleton as a skinned mesh sees it (glTF skin, `file#Skin/<i>`): its
/// joints in order (the indices in `MeshData::joints`), their parents and
/// inverse bind matrices. The joints themselves are entities (the imported
/// scene's nodes); [`SkinBinding`] names them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skin {
    /// Joint names (glTF node names, `Node <index>` when unnamed).
    pub joint_names: Vec<String>,
    /// Index of each joint's parent joint, `None` for skeleton roots.
    pub parents: Vec<Option<u32>>,
    /// Per joint: from the mesh's bind space into the joint's space.
    pub inverse_bind_matrices: Vec<Mat4>,
}

impl Skin {
    pub fn joint_count(&self) -> usize {
        self.inverse_bind_matrices.len()
    }
}

/// Binds this entity's mesh (its `MeshAsset`) to a skeleton: the skin asset
/// and the joint entities, by uid, in the skin's joint order. Saved; the
/// glTF importer writes it into model scenes, and prefab instancing remaps
/// the uids to the instance's joints. The asset glue turns it into a
/// runtime [`SkinnedMesh`]; joints that do not exist are left at the mesh's
/// bind pose (with a warning).
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct SkinBinding {
    /// The skin, usually a glTF sub-asset such as `models/hero.glb#Skin/0`.
    pub skin: AssetRef,
    /// Joint entities in skin joint order.
    pub joints: Vec<Uid>,
}

/// Runtime: draws this entity's mesh skinned by `joints` (GPU skinning,
/// RS4). Built by the asset glue from [`SkinBinding`]; may also be inserted
/// by code. Joint `i` of the mesh follows entity `joints[i]`'s
/// `GlobalTransform` times the skin's inverse bind matrix `i`; the entity's
/// own transform only places the bounds and decides winding, as in glTF.
/// Not registered, so never saved.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinnedMesh {
    pub skin: Handle<Skin>,
    pub joints: Vec<Entity>,
}

/// The skinning matrix of joint `i` relative to the mesh entity: `mesh⁻¹ ×
/// joint × inverse_bind`. A vertex `v` in bind space ends at
/// `mesh × Σ wᵢ · palette(i) · v` in the world, the same as glTF's
/// `Σ wᵢ · joint × inverse_bind · v` whatever the mesh node's transform.
pub fn joint_matrix(mesh_inverse: &Affine3A, joint: &Affine3A, inverse_bind: &Mat4) -> Mat4 {
    Mat4::from(*mesh_inverse * *joint) * *inverse_bind
}
