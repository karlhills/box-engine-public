//! Bake to mesh: freeze a procedural object into an ordinary mesh asset.
//!
//! [`bake_to_mesh`] writes the entity's generated mesh as a `.glb` file
//! under the project's `assets/` (one glTF primitive per used material
//! slot, plus the entity's inline `StandardMaterial` as a glTF material per
//! slot), then, as **one undo step**, removes the generator component (and
//! `Procedural`) and adds a `MeshAsset` that draws the file's mesh with the
//! same materials. The asset server imports the file like any other model,
//! so the result is an ordinary, editable-elsewhere mesh; undo brings the
//! generator back (the file stays).
//!
//! Materials of the baked `MeshAsset`, per slot: the slot's
//! `Procedural::materials` asset if set; else the entity's `MaterialRef`
//! asset; else the glTF material written from the inline
//! `StandardMaterial` (or the default). An inline `ShaderMaterial` cannot
//! be written to glTF: those slots draw the default material, with a
//! warning. Textures of an inline `StandardMaterial` are not written
//! either (a warning names them).
//!
//! Any generator bakes, `Primitive` included. A baked primitive keeps its
//! shadow setting, and a `Collider` that followed the primitive
//! (`ColliderShape::Primitive`) gets the primitive's shape written into it,
//! so it keeps colliding the same way.

use box_assets::{AssetError, AssetServer};
use box_core::asset_ref::AssetRef;
use box_core::assets::Assets;
use box_core::edit::{EditError, EditStack};
use box_core::identity::{Name, Uid};
use box_core::material::StandardMaterial;
use box_core::mesh::MeshData;
use box_core::mesh_asset::{MaterialRef, MeshAsset};
use box_core::primitive::Primitive;
use box_core::reflect::PartialReflect;
use box_core::world::World;
use box_proc::glb::{GlbError, GlbMaterial, write_glb};
use box_proc::{GeneratorRegistry, ProcState, Procedural};
use box_render::ShaderMaterial;

/// What [`bake_to_mesh`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct Baked {
    /// The new file, relative to `assets/`.
    pub path: String,
    /// The `MeshAsset` now on the entity.
    pub mesh_asset: MeshAsset,
    /// Things that did not carry over (shader or textured inline materials).
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    #[error("no entity with uid {0}")]
    NoEntity(Uid),
    #[error("the entity has no generator to bake")]
    NotProcedural,
    #[error("the generator produced no mesh to bake{0}")]
    NoMesh(String),
    #[error("cannot write the mesh: {0}")]
    Glb(#[from] GlbError),
    #[error("cannot create the mesh file: {0}")]
    Asset(#[from] AssetError),
    #[error("cannot replace the generator: {0}")]
    Edit(#[from] EditError),
}

/// A glTF name as the importer labels it (`Mesh/<name>`, `Material/<name>`):
/// letters, digits, `-`, `_` and `.` kept, anything else `_`.
fn label(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Bakes the procedural object `uid` into `path` (relative to the server's
/// `assets/` folder, e.g. `baked/steps.glb`; must not exist) and replaces
/// its generator with a `MeshAsset`, as one undo step labelled "Bake to
/// Mesh". See the [module docs](self).
pub fn bake_to_mesh(
    world: &mut World,
    edits: &mut EditStack,
    server: &AssetServer,
    uid: Uid,
    path: &str,
) -> Result<Baked, BakeError> {
    let entity = world.entity_by_uid(uid).ok_or(BakeError::NoEntity(uid))?;
    // Bake what the parameters say now, not a throttled older output.
    box_proc::settle(world);
    let (generator, mesh, slot_count, gen_warnings) = {
        let state = world
            .get::<ProcState>(entity)
            .ok_or(BakeError::NotProcedural)?;
        (
            state.generator(),
            state.mesh(),
            state.slot_count(),
            state.warnings().to_vec(),
        )
    };
    let mesh: MeshData = mesh
        .and_then(|h| world.get_resource::<Assets<MeshData>>()?.get(h).cloned())
        .ok_or_else(|| {
            BakeError::NoMesh(if gen_warnings.is_empty() {
                String::new()
            } else {
                format!(" ({})", gen_warnings.join("; "))
            })
        })?;
    let slot_names: Vec<&'static str> = world
        .registry()
        .generator(generator)
        .map(|g| g.material_slots().to_vec())
        .unwrap_or_default();

    let mut warnings = Vec::new();
    let inline = world.get::<StandardMaterial>(entity).map(|m| (*m).clone());
    let has_shader = world.has::<ShaderMaterial>(entity);
    let linked = world
        .get::<MaterialRef>(entity)
        .map(|m| m.material.clone())
        .filter(AssetRef::is_some);
    let procedural = world
        .get::<Procedural>(entity)
        .map(|p| (*p).clone())
        .unwrap_or_default();
    let cast_shadows = crate::proc_glue::casts_shadows(
        Some(&procedural),
        world.get::<Primitive>(entity).as_deref(),
    );
    if linked.is_none() && has_shader {
        warnings.push(
            "the inline ShaderMaterial cannot be written to glTF; slots without a material asset draw the default material"
                .to_string(),
        );
    }
    if let Some(m) = &inline
        && linked.is_none()
        && !has_shader
        && [
            &m.base_color_texture,
            &m.normal_texture,
            &m.orm_texture,
            &m.emissive_texture,
        ]
        .iter()
        .any(|t| t.is_some())
    {
        warnings.push(
            "textures of the inline StandardMaterial are not baked; assign a material asset to keep them"
                .to_string(),
        );
    }

    // One glTF material per slot, from the inline material, uniquely named.
    let standard = inline.unwrap_or_default();
    let mut names: Vec<String> = Vec::new();
    for i in 0..slot_count.max(1) {
        let base = slot_names
            .get(i)
            .map(|n| label(n))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Slot{i}"));
        let mut name = base.clone();
        if names.contains(&name) {
            name = format!("{base}_{i}");
        }
        names.push(name);
    }
    let materials: Vec<GlbMaterial<'_>> = names
        .iter()
        .map(|name| GlbMaterial {
            name,
            material: &standard,
        })
        .collect();
    let mesh_name = world
        .get::<Name>(entity)
        .map(|n| label(&n.0))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Baked".to_string());
    let glb = write_glb(&mesh, &mesh_name, &materials)?;

    let id = server.create(path, &glb.bytes)?;
    let file = AssetRef::id(id).with_path(path.replace('\\', "/"));
    let mesh_asset = MeshAsset {
        mesh: file.clone().with_label(format!("Mesh/{mesh_name}")),
        materials: glb
            .slots
            .iter()
            .map(|&slot| {
                let slot = slot as usize;
                match procedural.materials.get(slot).filter(|r| r.is_some()) {
                    Some(r) => r.clone(),
                    None => match (&linked, has_shader) {
                        (Some(r), _) => r.clone(),
                        (None, true) => AssetRef::NONE,
                        (None, false) => file
                            .clone()
                            .with_label(format!("Material/{}", names[slot.min(names.len() - 1)])),
                    },
                }
            })
            .collect(),
        cast_shadows,
    };

    edits.begin("Bake to Mesh");
    let result = (|| -> Result<(), EditError> {
        #[cfg(feature = "physics")]
        pin_primitive_collider(world, edits, uid)?;
        edits.remove_component(world, uid, generator)?;
        if world.has::<Procedural>(entity) {
            edits.remove_component(world, uid, "Procedural")?;
        }
        if world.has::<MeshAsset>(entity) {
            edits.remove_component(world, uid, "MeshAsset")?;
        }
        edits.add_component(
            world,
            uid,
            "MeshAsset",
            Some(mesh_asset.as_partial_reflect()),
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            edits.commit();
        }
        Err(e) => {
            let _ = edits.rollback(world);
            return Err(e.into());
        }
    }
    for w in &warnings {
        tracing::warn!("bake {path}: {w}");
    }
    Ok(Baked {
        path: path.to_string(),
        mesh_asset,
        warnings,
    })
}

/// A `Collider` that follows the entity's `Primitive`
/// (`ColliderShape::Primitive`) would vanish with it, so baking writes the
/// primitive's current shape into the collider (part of the same undo step).
#[cfg(feature = "physics")]
fn pin_primitive_collider(
    world: &mut World,
    edits: &mut EditStack,
    uid: Uid,
) -> Result<(), EditError> {
    use box_core::primitive::Shape;
    use box_physics::{Collider, ColliderShape};
    let Some(entity) = world.entity_by_uid(uid) else {
        return Ok(());
    };
    let follows = world
        .get::<Collider>(entity)
        .is_some_and(|c| c.shape == ColliderShape::Primitive);
    let Some(shape) = world.get::<Primitive>(entity).map(|p| p.shape.clone()) else {
        return Ok(());
    };
    if !follows {
        return Ok(());
    }
    let pinned = match shape {
        Shape::Cuboid { size } => ColliderShape::Cuboid { size },
        Shape::Sphere { radius, .. } => ColliderShape::Sphere { radius },
        Shape::Cylinder { radius, height, .. } => ColliderShape::Cylinder { radius, height },
        Shape::Capsule {
            radius,
            half_height,
            ..
        } => ColliderShape::Capsule {
            radius,
            half_height,
        },
        // A flat box, as `ColliderShape::Primitive` resolves a plane.
        Shape::Plane { size, .. } => ColliderShape::Cuboid {
            size: glam::Vec3::new(size.x, 0.0, size.y),
        },
    };
    let path = world.field_path("Collider.shape")?;
    edits.set_field(world, uid, &path, pinned.as_partial_reflect())
}

/// An unused `baked/<name>.glb` path under `assets/` for the entity `uid`
/// (`baked/<name> 2.glb`, ... when taken).
pub fn bake_path(world: &World, server: &AssetServer, uid: Uid) -> String {
    let base = world
        .entity_by_uid(uid)
        .and_then(|e| world.get::<Name>(e).map(|n| n.0.clone()))
        .map(|n| {
            n.chars()
                .map(|c| {
                    if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "baked".to_string());
    let dir = server.assets_dir().join("baked");
    let mut n = 1;
    loop {
        let file = if n == 1 {
            format!("{base}.glb")
        } else {
            format!("{base} {n}.glb")
        };
        if !dir.join(&file).exists() {
            return format!("baked/{file}");
        }
        n += 1;
    }
}
