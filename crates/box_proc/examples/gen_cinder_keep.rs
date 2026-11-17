//! Writes the M8 game's (`examples/cinder_keep`) imported models: the two
//! levels, built here outside the editor as named glTF nodes with several
//! materials, and the combat character (the procedural humanoid with
//! `Attack` and `Hit` clips). Creates missing `.meta` files and turns on
//! the `colliders` import setting for the levels, so their geometry
//! collides as drawn.
//!
//! `cargo run -p box_proc --example gen_cinder_keep [-- <assets dir>]`
//!
//! The bytes are deterministic; the files are committed (regenerate after
//! changing a level here, then re-run the game's tests and look at its
//! captures). Coordinates the scenes depend on are listed in
//! `examples/cinder_keep/README.md`.

use box_core::pmath;
use std::path::PathBuf;

use box_core::color::Color;
use box_core::material::StandardMaterial;
use box_core::mesh::{MaterialGroup, MeshData};
use box_proc::glb::{GlbMaterial, GlbNode, write_scene_glb};
use box_proc::mesh_builder::MeshBuilder;
use glam::{Affine3A, Quat, Vec2, Vec3};

fn main() -> anyhow::Result<()> {
    let assets = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cinder_keep/assets")
        });
    let models = assets.join("models");
    std::fs::create_dir_all(&models)?;
    let write = |name: &str, bytes: &[u8]| -> anyhow::Result<()> {
        let path = models.join(name);
        std::fs::write(&path, bytes)?;
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
        Ok(())
    };
    write("outdoor.glb", &outdoor()?)?;
    write("keep.glb", &keep()?)?;
    write("props.glb", &props()?)?;
    write(
        "character.glb",
        &box_proc::humanoid::combat_character_glb()?.bytes,
    )?;
    // Opening an asset server on the folder creates the missing `.meta`s.
    box_engine::asset_server(&assets)?;
    for level in ["outdoor.glb", "keep.glb"] {
        let path = models.join(format!("{level}.meta"));
        let mut meta: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        if meta["settings"]["colliders"] != serde_json::Value::Bool(true) {
            meta["settings"]["colliders"] = true.into();
            let mut text = serde_json::to_string_pretty(&meta)?;
            text.push('\n');
            std::fs::write(&path, text)?;
            println!("{}: colliders on", path.display());
        }
    }
    Ok(())
}

// ---- geometry helpers ----------------------------------------------------

fn material(
    name: &'static str,
    srgb: [f32; 3],
    roughness: f32,
) -> (&'static str, StandardMaterial) {
    (
        name,
        StandardMaterial {
            base_color: Color::srgb(srgb[0], srgb[1], srgb[2]),
            roughness,
            ..StandardMaterial::default()
        },
    )
}

/// A deterministic hash in 0..1 for per-face colour variation.
fn noise(x: f32, z: f32) -> f32 {
    let h = pmath::sin(x * 12.9898 + z * 78.233) * 43_758.547;
    h - h.floor()
}

/// A slightly varied grey for vertex colours (stylized, not flat).
fn shade(v: f32) -> [f32; 4] {
    let s = 0.86 + 0.14 * v;
    [s, s, s, 1.0]
}

/// Appends a quad facing `outward` (the winding is fixed up).
fn quad(b: &mut MeshBuilder, slot: u32, p: [Vec3; 4], outward: Vec3) {
    let n = (p[2] - p[0]).cross(p[3] - p[1]);
    let p = if n.dot(outward) >= 0.0 {
        p
    } else {
        [p[0], p[3], p[2], p[1]]
    };
    let uv = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
    b.flat_quad(slot, p, uv);
}

/// An axis-aligned box from `min` to `max`.
fn cuboid(b: &mut MeshBuilder, slot: u32, min: Vec3, max: Vec3) {
    let mut m = MeshData::cuboid(max - min);
    m.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: m.indices.len() as u32,
        material_slot: slot,
    }];
    b.set_color(shade(noise(min.x + max.z, min.z + max.y)));
    let colour = b.color();
    m.colors = vec![colour; m.positions.len()];
    b.append(&m, Affine3A::from_translation((min + max) * 0.5));
}

/// A capped cylinder standing on `base`.
fn cylinder(b: &mut MeshBuilder, slot: u32, base: Vec3, radius: f32, height: f32) {
    let mut m = MeshData::cylinder(radius, height, 12);
    m.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: m.indices.len() as u32,
        material_slot: slot,
    }];
    m.colors = vec![shade(noise(base.x, base.z)); m.positions.len()];
    b.append(
        &m,
        Affine3A::from_translation(base + Vec3::Y * height * 0.5),
    );
}

/// A wedge: a slope rising along +Z from `z0` (height `y0`) to `z1`
/// (height `y1`) over `x0..x1`, with a flat bottom at `bottom`.
fn wedge_z(
    b: &mut MeshBuilder,
    slot: u32,
    x: (f32, f32),
    z: (f32, f32),
    y: (f32, f32),
    bottom: f32,
) {
    let (x0, x1) = x;
    let (z0, z1) = z;
    let (y0, y1) = y;
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let up = Vec3::new(0.0, (z1 - z0).abs(), -(y1 - y0) * (z1 - z0).signum());
    quad(
        b,
        slot,
        [p(x0, y0, z0), p(x1, y0, z0), p(x1, y1, z1), p(x0, y1, z1)],
        up,
    );
    for (xs, out) in [(x0, -Vec3::X), (x1, Vec3::X)] {
        quad(
            b,
            slot,
            [
                p(xs, bottom, z0),
                p(xs, y0, z0),
                p(xs, y1, z1),
                p(xs, bottom, z1),
            ],
            out,
        );
    }
    let toward = |z: f32, other: f32| Vec3::Z * (z - other).signum();
    quad(
        b,
        slot,
        [
            p(x0, bottom, z0),
            p(x1, bottom, z0),
            p(x1, y0, z0),
            p(x0, y0, z0),
        ],
        toward(z0, z1),
    );
    quad(
        b,
        slot,
        [
            p(x0, bottom, z1),
            p(x1, bottom, z1),
            p(x1, y1, z1),
            p(x0, y1, z1),
        ],
        toward(z1, z0),
    );
    quad(
        b,
        slot,
        [
            p(x0, bottom, z0),
            p(x1, bottom, z0),
            p(x1, bottom, z1),
            p(x0, bottom, z1),
        ],
        -Vec3::Y,
    );
}

/// The same along X (rising from `x0` to `x1`).
fn wedge_x(
    b: &mut MeshBuilder,
    slot: u32,
    x: (f32, f32),
    z: (f32, f32),
    y: (f32, f32),
    bottom: f32,
) {
    let (x0, x1) = x;
    let (z0, z1) = z;
    let (y0, y1) = y;
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let up = Vec3::new(-(y1 - y0) * (x1 - x0).signum(), (x1 - x0).abs(), 0.0);
    quad(
        b,
        slot,
        [p(x0, y0, z0), p(x0, y0, z1), p(x1, y1, z1), p(x1, y1, z0)],
        up,
    );
    for (zs, out) in [(z0, -Vec3::Z), (z1, Vec3::Z)] {
        quad(
            b,
            slot,
            [
                p(x0, bottom, zs),
                p(x0, y0, zs),
                p(x1, y1, zs),
                p(x1, bottom, zs),
            ],
            out,
        );
    }
    let toward = |x: f32, other: f32| Vec3::X * (x - other).signum();
    quad(
        b,
        slot,
        [
            p(x0, bottom, z0),
            p(x0, bottom, z1),
            p(x0, y0, z1),
            p(x0, y0, z0),
        ],
        toward(x0, x1),
    );
    quad(
        b,
        slot,
        [
            p(x1, bottom, z0),
            p(x1, bottom, z1),
            p(x1, y1, z1),
            p(x1, y1, z0),
        ],
        toward(x1, x0),
    );
    quad(
        b,
        slot,
        [
            p(x0, bottom, z0),
            p(x0, bottom, z1),
            p(x1, bottom, z1),
            p(x1, bottom, z0),
        ],
        -Vec3::Y,
    );
}

fn builder() -> MeshBuilder {
    MeshBuilder::new().with_colors()
}

struct Node {
    name: &'static str,
    parent: Option<usize>,
    mesh: Option<MeshData>,
}

fn group(name: &'static str) -> Node {
    Node {
        name,
        parent: None,
        mesh: None,
    }
}

fn part(name: &'static str, parent: usize, b: MeshBuilder) -> Node {
    Node {
        name,
        parent: Some(parent),
        mesh: Some(b.finish()),
    }
}

fn write(
    root: &str,
    nodes: &[Node],
    materials: &[(&'static str, StandardMaterial)],
) -> anyhow::Result<Vec<u8>> {
    let glb_nodes: Vec<GlbNode<'_>> = nodes
        .iter()
        .map(|n| GlbNode {
            name: n.name,
            parent: n.parent,
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
            mesh: n.mesh.as_ref(),
        })
        .collect();
    let mats: Vec<GlbMaterial<'_>> = materials
        .iter()
        .map(|(name, material)| GlbMaterial { name, material })
        .collect();
    Ok(write_scene_glb(root, &glb_nodes, &mats)?.bytes)
}

// ---- the outdoor level -----------------------------------------------------
//
// South (+Z) to north (-Z): the meadow (y 0, a flat dirt path |x| < 3.5
// between rolling hills) from z 14 to -20; a ramp (x -3..3) from z -12 up
// to the upper ledge (y 2, z -20..-28) with a rock slope to its east
// (x 4..12, rising to y 5); the lava gap (z -28..-31, lava at y -5); the
// courtyard (y 2, z -31..-62) where the wave is fought; the keep facade at
// z -57..-58 with a doorway (x -1.8..1.8, y 2..5.5) into the gatehouse.

const GRASS: u32 = 0;
const DIRT: u32 = 1;
const ROCK: u32 = 2;
const STONE: u32 = 3;
const LAVA: u32 = 4;
const DARK: u32 = 5;

fn meadow_height(x: f32, z: f32) -> f32 {
    let smooth = |a: f32, b: f32, v: f32| {
        let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let sides = smooth(3.5, 13.0, x.abs()) * (1.3 + 0.35 * pmath::sin(0.45 * z + 0.3 * x));
    let back = smooth(12.0, 14.0, z) * 1.4;
    sides.max(back)
}

fn outdoor() -> anyhow::Result<Vec<u8>> {
    let materials = [
        material("Grass", [0.36, 0.55, 0.25], 0.95),
        material("Dirt", [0.55, 0.42, 0.29], 0.95),
        material("Rock", [0.43, 0.41, 0.40], 0.9),
        material("Stone", [0.62, 0.57, 0.50], 0.85),
        (
            "Lava",
            StandardMaterial {
                base_color: Color::srgb(1.0, 0.35, 0.05),
                emissive: Color::linear(6.0, 1.6, 0.2),
                roughness: 0.6,
                ..StandardMaterial::default()
            },
        ),
        material("Dark", [0.05, 0.04, 0.04], 0.9),
    ];
    let mut nodes = vec![group("Terrain"), group("Keep")];
    const TERRAIN: usize = 0;
    const KEEP: usize = 1;

    // Meadow: 1 m cells, a flat dirt path down the middle.
    let mut b = builder();
    for iz in -20..14 {
        for ix in -16..16 {
            let (x0, z0) = (ix as f32, iz as f32);
            let corners = [
                (x0, z0),
                (x0 + 1.0, z0),
                (x0 + 1.0, z0 + 1.0),
                (x0, z0 + 1.0),
            ]
            .map(|(x, z)| Vec3::new(x, meadow_height(x, z), z));
            let centre_x = x0 + 0.5;
            let slot = if centre_x.abs() < 3.5 { DIRT } else { GRASS };
            b.set_color(shade(noise(x0, z0)));
            quad(&mut b, slot, corners, Vec3::Y);
        }
    }
    // Skirt under the edges so the meadow reads as solid from below.
    cuboid(
        &mut b,
        ROCK,
        Vec3::new(-16.0, -6.0, -20.0),
        Vec3::new(16.0, -0.05, 14.0),
    );
    nodes.push(part("Meadow", TERRAIN, b));

    let mut b = builder();
    cuboid(
        &mut b,
        ROCK,
        Vec3::new(-17.0, -1.0, -20.0),
        Vec3::new(-16.0, 4.0, 15.0),
    );
    cuboid(
        &mut b,
        ROCK,
        Vec3::new(16.0, -1.0, -20.0),
        Vec3::new(17.0, 4.0, 15.0),
    );
    cuboid(
        &mut b,
        ROCK,
        Vec3::new(-17.0, -1.0, 14.0),
        Vec3::new(17.0, 2.5, 15.0),
    );
    nodes.push(part("Boundary", TERRAIN, b));

    let mut b = builder();
    wedge_z(&mut b, DIRT, (-3.0, 3.0), (-12.0, -20.0), (0.0, 2.0), -0.5);
    nodes.push(part("Ramp", TERRAIN, b));

    let mut b = builder();
    cuboid(
        &mut b,
        ROCK,
        Vec3::new(-16.0, -8.0, -28.0),
        Vec3::new(16.0, 2.0, -20.0),
    );
    nodes.push(part("Upper Ledge", TERRAIN, b));

    let mut b = builder();
    wedge_x(&mut b, ROCK, (4.0, 12.0), (-26.0, -21.0), (2.0, 5.0), 1.9);
    cuboid(
        &mut b,
        ROCK,
        Vec3::new(12.0, 1.9, -27.0),
        Vec3::new(14.0, 6.5, -20.0),
    );
    nodes.push(part("Rock Slope", TERRAIN, b));

    let mut b = builder();
    cuboid(
        &mut b,
        LAVA,
        Vec3::new(-16.0, -9.0, -31.0),
        Vec3::new(16.0, -5.0, -28.0),
    );
    nodes.push(part("Lava", TERRAIN, b));

    let mut b = builder();
    cuboid(
        &mut b,
        STONE,
        Vec3::new(-16.0, -8.0, -62.0),
        Vec3::new(16.0, 2.0, -31.0),
    );
    // Paving: lighter slabs on the arena floor (a thin raised pattern
    // would trip the character; colour only).
    nodes.push(part("Courtyard", TERRAIN, b));

    let mut b = builder();
    for (x0, x1) in [(-16.0, -15.0), (15.0, 16.0)] {
        cuboid(
            &mut b,
            STONE,
            Vec3::new(x0, 2.0, -28.0),
            Vec3::new(x1, 3.5, -20.0),
        );
        cuboid(
            &mut b,
            STONE,
            Vec3::new(x0, 2.0, -62.0),
            Vec3::new(x1, 3.5, -31.0),
        );
    }
    for (x, z, h) in [
        (-13.0, -36.0, 3.0),
        (13.0, -36.0, 3.0),
        (-13.0, -50.0, 1.6),
        (13.0, -50.0, 3.0),
    ] {
        cylinder(&mut b, STONE, Vec3::new(x, 2.0, z), 0.6, h);
    }
    nodes.push(part("Courtyard Walls", TERRAIN, b));

    let mut b = builder();
    cuboid(
        &mut b,
        STONE,
        Vec3::new(-16.0, 2.0, -58.0),
        Vec3::new(-1.8, 10.0, -57.0),
    );
    cuboid(
        &mut b,
        STONE,
        Vec3::new(1.8, 2.0, -58.0),
        Vec3::new(16.0, 10.0, -57.0),
    );
    cuboid(
        &mut b,
        STONE,
        Vec3::new(-1.8, 5.5, -58.0),
        Vec3::new(1.8, 10.0, -57.0),
    );
    // Battlements.
    for i in 0..8 {
        let x = -15.0 + i as f32 * 4.0;
        cuboid(
            &mut b,
            STONE,
            Vec3::new(x, 10.0, -58.0),
            Vec3::new(x + 2.0, 11.0, -57.0),
        );
    }
    nodes.push(part("Facade", KEEP, b));

    let mut b = builder();
    for x in [-10.0, 10.0] {
        cylinder(&mut b, STONE, Vec3::new(x, 2.0, -59.5), 2.5, 11.0);
        cylinder(&mut b, DARK, Vec3::new(x, 13.0, -59.5), 2.7, 0.4);
    }
    nodes.push(part("Towers", KEEP, b));

    let mut b = builder();
    cuboid(
        &mut b,
        STONE,
        Vec3::new(-2.8, 2.0, -62.0),
        Vec3::new(-1.8, 5.5, -58.0),
    );
    cuboid(
        &mut b,
        STONE,
        Vec3::new(1.8, 2.0, -62.0),
        Vec3::new(2.8, 5.5, -58.0),
    );
    cuboid(
        &mut b,
        DARK,
        Vec3::new(-2.8, 2.0, -63.0),
        Vec3::new(2.8, 5.5, -62.0),
    );
    cuboid(
        &mut b,
        STONE,
        Vec3::new(-2.8, 5.5, -62.0),
        Vec3::new(2.8, 6.0, -58.0),
    );
    nodes.push(part("Gatehouse", KEEP, b));

    write("Outdoor", &nodes, &materials)
}

// ---- props -------------------------------------------------------------------
//
// Meshes the scenes and scripts place many times (one mesh asset each, so
// the renderer draws all copies of one together): `Cinderling` (r 0.35),
// `Spitter` (r 0.42), `Ember` (r 0.18), `Crate` (1 m), `Boulder` (r 0.6),
// `Gate` (3.4 x 3.4 x 0.2), `Brazier` (stands on its origin, 1.25 m).
// Each is centred on its origin unless noted.

fn sphere(b: &mut MeshBuilder, slot: u32, at: Vec3, radius: f32, sectors: u32, stacks: u32) {
    let mut m = MeshData::uv_sphere(radius, sectors, stacks);
    m.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: m.indices.len() as u32,
        material_slot: slot,
    }];
    m.colors = vec![[1.0; 4]; m.positions.len()];
    b.append(&m, Affine3A::from_translation(at));
}

fn props() -> anyhow::Result<Vec<u8>> {
    const CINDER: u32 = 0;
    const SPIT: u32 = 1;
    const EMBER: u32 = 2;
    const EYE: u32 = 3;
    const PROP_WOOD: u32 = 4;
    const IRON: u32 = 5;
    const PROP_ROCK: u32 = 6;
    const PROP_STONE: u32 = 7;
    let glow = |name, base: [f32; 3], emissive: [f32; 3]| {
        (
            name,
            StandardMaterial {
                base_color: Color::srgb(base[0], base[1], base[2]),
                emissive: Color::linear(emissive[0], emissive[1], emissive[2]),
                roughness: 0.7,
                ..StandardMaterial::default()
            },
        )
    };
    let materials = [
        glow("Cinder", [0.25, 0.08, 0.03], [2.2, 0.55, 0.08]),
        glow("Spit", [0.20, 0.06, 0.22], [1.6, 0.35, 2.4]),
        glow("Ember", [1.0, 0.6, 0.2], [8.0, 3.0, 0.6]),
        glow("Eye", [1.0, 0.9, 0.5], [6.0, 5.0, 1.5]),
        material("Wood", [0.47, 0.31, 0.17], 0.85),
        (
            "Iron",
            StandardMaterial {
                base_color: Color::srgb(0.25, 0.25, 0.27),
                metallic: 0.9,
                roughness: 0.45,
                ..StandardMaterial::default()
            },
        ),
        material("Rock", [0.40, 0.38, 0.37], 0.9),
        material("Stone", [0.62, 0.57, 0.50], 0.85),
    ];
    let mut nodes = Vec::new();
    let root = |nodes: &mut Vec<Node>, name, b: MeshBuilder| {
        nodes.push(Node {
            name,
            parent: None,
            mesh: Some(b.finish()),
        })
    };

    let eyes = |b: &mut MeshBuilder, r: f32| {
        for x in [-0.12, 0.12] {
            let c = Vec3::new(x, 0.08, -r * 0.9);
            cuboid(b, EYE, c - Vec3::splat(0.045), c + Vec3::splat(0.045));
        }
    };
    let mut b = builder();
    sphere(&mut b, CINDER, Vec3::ZERO, 0.35, 10, 7);
    eyes(&mut b, 0.35);
    root(&mut nodes, "Cinderling", b);

    let mut b = builder();
    sphere(&mut b, SPIT, Vec3::ZERO, 0.42, 10, 7);
    for (x, z) in [(0.0, 0.0), (0.25, 0.2), (-0.25, 0.2)] {
        cuboid(
            &mut b,
            PROP_ROCK,
            Vec3::new(x - 0.06, 0.3, z - 0.06),
            Vec3::new(x + 0.06, 0.62, z + 0.06),
        );
    }
    eyes(&mut b, 0.42);
    root(&mut nodes, "Spitter", b);

    let mut b = builder();
    sphere(&mut b, EMBER, Vec3::ZERO, 0.18, 8, 6);
    root(&mut nodes, "Ember", b);

    let mut b = builder();
    cuboid(&mut b, PROP_WOOD, Vec3::splat(-0.5), Vec3::splat(0.5));
    for (lo, hi) in [
        (Vec3::new(-0.52, -0.52, -0.52), Vec3::new(0.52, -0.4, 0.52)),
        (Vec3::new(-0.52, 0.4, -0.52), Vec3::new(0.52, 0.52, 0.52)),
    ] {
        cuboid(&mut b, IRON, lo, hi);
    }
    root(&mut nodes, "Crate", b);

    let mut b = builder();
    sphere(&mut b, PROP_ROCK, Vec3::ZERO, 0.6, 9, 6);
    root(&mut nodes, "Boulder", b);

    let mut b = builder();
    for i in 0..5 {
        let x0 = -1.7 + i as f32 * 0.68;
        cuboid(
            &mut b,
            PROP_WOOD,
            Vec3::new(x0 + 0.02, -1.7, -0.1),
            Vec3::new(x0 + 0.66, 1.7, 0.1),
        );
    }
    for y in [-1.0, 1.0] {
        cuboid(
            &mut b,
            IRON,
            Vec3::new(-1.7, y - 0.12, -0.13),
            Vec3::new(1.7, y + 0.12, 0.13),
        );
    }
    root(&mut nodes, "Gate", b);

    let mut b = builder();
    cylinder(&mut b, PROP_STONE, Vec3::ZERO, 0.28, 1.0);
    cylinder(&mut b, IRON, Vec3::new(0.0, 1.0, 0.0), 0.55, 0.22);
    sphere(&mut b, EMBER, Vec3::new(0.0, 1.22, 0.0), 0.3, 8, 4);
    root(&mut nodes, "Brazier", b);

    write("Props", &nodes, &materials)
}

// ---- the keep interior -----------------------------------------------------
//
// A hall x -8..8, z 0..-30, floor y 0, walls to y 7 under a ceiling. The
// entrance doorway is in the south wall (x -1.8..1.8, y 0..3.5). Eight
// 0.25 m steps (0.5 m deep, z -18..-22) climb to the throne dais (y 2,
// z -22..-30) with the throne at z -28.

const FLOOR: u32 = 0;
const WALL: u32 = 1;
const CARPET: u32 = 2;
const GOLD: u32 = 3;
const WOOD: u32 = 4;

fn keep() -> anyhow::Result<Vec<u8>> {
    let materials = [
        material("Flagstone", [0.36, 0.33, 0.31], 0.9),
        material("Wall", [0.52, 0.48, 0.43], 0.9),
        material("Carpet", [0.55, 0.08, 0.08], 0.95),
        (
            "Gold",
            StandardMaterial {
                base_color: Color::srgb(0.86, 0.66, 0.26),
                metallic: 0.8,
                roughness: 0.35,
                ..StandardMaterial::default()
            },
        ),
        material("Wood", [0.40, 0.26, 0.15], 0.8),
    ];
    let mut nodes = vec![group("Hall"), group("Furniture")];
    const HALL: usize = 0;
    const FURNITURE: usize = 1;

    let mut b = builder();
    cuboid(
        &mut b,
        FLOOR,
        Vec3::new(-9.0, -1.0, -31.0),
        Vec3::new(9.0, 0.0, 4.0),
    );
    // The carpet runner is flush with the floor (colour only).
    nodes.push(part("Floor", HALL, b));

    let mut b = builder();
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-9.0, 0.0, -31.0),
        Vec3::new(-8.0, 7.0, 1.0),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(8.0, 0.0, -31.0),
        Vec3::new(9.0, 7.0, 1.0),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-8.0, 0.0, -31.0),
        Vec3::new(8.0, 7.0, -30.0),
    );
    // South wall with the doorway.
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-8.0, 0.0, 0.0),
        Vec3::new(-1.8, 7.0, 1.0),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(1.8, 0.0, 0.0),
        Vec3::new(8.0, 7.0, 1.0),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-1.8, 3.5, 0.0),
        Vec3::new(1.8, 7.0, 1.0),
    );
    // A short entrance passage behind the doorway, closed at its end.
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-2.8, 0.0, 1.0),
        Vec3::new(-1.8, 3.5, 4.0),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(1.8, 0.0, 1.0),
        Vec3::new(2.8, 3.5, 4.0),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-2.8, 0.0, 4.0),
        Vec3::new(2.8, 3.5, 4.5),
    );
    cuboid(
        &mut b,
        WALL,
        Vec3::new(-2.8, 3.5, 1.0),
        Vec3::new(2.8, 4.0, 4.5),
    );
    nodes.push(part("Walls", HALL, b));

    let mut b = builder();
    cuboid(
        &mut b,
        WOOD,
        Vec3::new(-9.0, 7.0, -31.0),
        Vec3::new(9.0, 7.5, 1.0),
    );
    for z in [-6.0, -12.0] {
        cuboid(
            &mut b,
            WOOD,
            Vec3::new(-8.0, 6.4, z - 0.3),
            Vec3::new(8.0, 7.0, z + 0.3),
        );
    }
    nodes.push(part("Ceiling", HALL, b));

    let mut b = builder();
    for x in [-5.0, 5.0] {
        for z in [-4.0, -10.0, -16.0] {
            cylinder(&mut b, WALL, Vec3::new(x, 0.0, z), 0.55, 7.0);
            cuboid(
                &mut b,
                WALL,
                Vec3::new(x - 0.75, 0.0, z - 0.75),
                Vec3::new(x + 0.75, 0.4, z + 0.75),
            );
        }
    }
    nodes.push(part("Pillars", HALL, b));

    let mut b = builder();
    for i in 0..8 {
        let top = 0.25 * (i + 1) as f32;
        let z1 = -18.0 - 0.5 * i as f32;
        let slot = if i % 2 == 0 { FLOOR } else { WALL };
        cuboid(
            &mut b,
            slot,
            Vec3::new(-8.0, 0.0, z1 - 0.5),
            Vec3::new(8.0, top, z1),
        );
    }
    cuboid(
        &mut b,
        FLOOR,
        Vec3::new(-8.0, 0.0, -30.0),
        Vec3::new(8.0, 2.0, -22.0),
    );
    nodes.push(part("Stairs and Dais", HALL, b));

    let mut b = builder();
    cuboid(
        &mut b,
        CARPET,
        Vec3::new(-1.0, 2.0, -29.5),
        Vec3::new(1.0, 2.02, -22.0),
    );
    cuboid(
        &mut b,
        GOLD,
        Vec3::new(-1.2, 2.0, -28.8),
        Vec3::new(1.2, 2.6, -27.6),
    );
    cuboid(
        &mut b,
        GOLD,
        Vec3::new(-1.2, 2.6, -28.8),
        Vec3::new(1.2, 5.0, -28.4),
    );
    cuboid(
        &mut b,
        CARPET,
        Vec3::new(-0.9, 2.6, -28.4),
        Vec3::new(0.9, 2.7, -27.7),
    );
    for x in [-1.2, 1.0] {
        cuboid(
            &mut b,
            GOLD,
            Vec3::new(x, 2.6, -28.8),
            Vec3::new(x + 0.2, 3.3, -27.6),
        );
    }
    nodes.push(part("Throne", FURNITURE, b));

    let mut b = builder();
    for z in [-6.0, -12.0] {
        for x in [-6.8, 6.8] {
            cuboid(
                &mut b,
                WOOD,
                Vec3::new(x - 0.5, 0.0, z - 1.5),
                Vec3::new(x + 0.5, 0.8, z + 1.5),
            );
        }
    }
    nodes.push(part("Benches", FURNITURE, b));

    write("Keep", &nodes, &materials)
}
