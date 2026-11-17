//! Writes the `procedural` project template's scatter model
//! (`templates/projects/procedural/assets/models/garden.glb`): a shrub and
//! a rock, each a root glTF node with its own mesh, and their materials.
//! Creates a missing `.meta` file.
//!
//! `cargo run -p box_proc --example gen_template_props [-- <assets dir>]`
//!
//! The bytes are deterministic; the file is committed.

use std::path::PathBuf;

use box_core::color::Color;
use box_core::material::StandardMaterial;
use box_core::mesh::{MaterialGroup, MeshData};
use box_proc::glb::{GlbMaterial, GlbNode, write_scene_glb};
use box_proc::mesh_builder::MeshBuilder;
use glam::{Affine3A, Quat, Vec3};

const LEAVES: u32 = 0;
const ROCK: u32 = 1;

fn main() -> anyhow::Result<()> {
    let assets = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../templates/projects/procedural/assets")
        });
    let models = assets.join("models");
    std::fs::create_dir_all(&models)?;
    let path = models.join("garden.glb");
    let bytes = garden()?;
    std::fs::write(&path, &bytes)?;
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
    // Opening an asset server on the folder creates the missing `.meta`.
    box_engine::asset_server(&assets)?;
    Ok(())
}

/// A low-poly sphere in material `slot`, scaled and placed by `at`.
fn blob(b: &mut MeshBuilder, slot: u32, at: Affine3A, sectors: u32, stacks: u32) {
    let mut m = MeshData::uv_sphere(1.0, sectors, stacks);
    m.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: m.indices.len() as u32,
        material_slot: slot,
    }];
    m.colors = vec![[1.0; 4]; m.positions.len()];
    b.append(&m, at);
}

fn garden() -> anyhow::Result<Vec<u8>> {
    let place = |scale: Vec3, yaw: f32, at: Vec3| {
        Affine3A::from_scale_rotation_translation(scale, Quat::from_rotation_y(yaw), at)
    };

    // A shrub: three overlapping leaf clumps, standing on its origin.
    let mut shrub = MeshBuilder::new().with_colors();
    for (scale, at) in [
        (Vec3::new(0.55, 0.45, 0.55), Vec3::new(0.0, 0.4, 0.0)),
        (Vec3::new(0.4, 0.35, 0.4), Vec3::new(0.3, 0.3, 0.15)),
        (Vec3::new(0.38, 0.32, 0.38), Vec3::new(-0.25, 0.28, -0.2)),
    ] {
        blob(&mut shrub, LEAVES, place(scale, 0.0, at), 9, 6);
    }

    // A rock: a flattened, tilted lump, sunk a little into the ground.
    let mut rock = MeshBuilder::new().with_colors();
    blob(
        &mut rock,
        ROCK,
        Affine3A::from_scale_rotation_translation(
            Vec3::new(0.5, 0.3, 0.38),
            Quat::from_rotation_z(0.2) * Quat::from_rotation_y(0.6),
            Vec3::new(0.0, 0.12, 0.0),
        ),
        7,
        5,
    );

    let (shrub, rock) = (shrub.finish(), rock.finish());
    let node = |name, mesh| GlbNode {
        name,
        parent: None,
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
        mesh: Some(mesh),
    };
    let leaves = StandardMaterial {
        base_color: Color::srgb(0.30, 0.52, 0.22),
        roughness: 0.9,
        ..StandardMaterial::default()
    };
    let stone = StandardMaterial {
        base_color: Color::srgb(0.55, 0.53, 0.50),
        roughness: 0.95,
        ..StandardMaterial::default()
    };
    let materials = [
        GlbMaterial {
            name: "Leaves",
            material: &leaves,
        },
        GlbMaterial {
            name: "Rock",
            material: &stone,
        },
    ];
    Ok(write_scene_glb(
        "Garden",
        &[node("Shrub", &shrub), node("Rock", &rock)],
        &materials,
    )?
    .bytes)
}
