//! Scatter demo content, used by `examples/scatter.rs` and the scatter
//! tests: `Hills` (a game-defined ground generator: rolling hills and a
//! steep-sided mesa), low-poly species meshes built with the engine's own
//! generators (no third-party assets), and a scene with three scatters on
//! the hills: a meadow (grass and bushes) with a slope limit, rocks
//! anywhere, and flowers in a painted region.

use box_engine::box_core::asset_ref::{AssetId, AssetRef};
use box_engine::box_core::assets::AssetLookup;
use box_engine::box_proc::mesh_builder::{MeshBuilder, Vertex};
use box_engine::box_proc::scatter::{Scatter, ScatterSpecies};
use box_engine::prelude::*;

/// Rolling hills with a steep-sided mesa, as a height grid.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct Hills {
    /// Edge length of the square.
    #[reflect(@Range { min: 1.0, max: 500.0 }, @Unit("m"))]
    pub size: f32,
    /// Grid cells along one edge.
    #[reflect(@Range { min: 1.0, max: 512.0 })]
    pub resolution: u32,
    /// Height of the rolling hills.
    #[reflect(@Range { min: 0.0, max: 20.0 }, @Unit("m"))]
    pub height: f32,
    /// Height of the mesa (steep sides).
    #[reflect(@Range { min: 0.0, max: 20.0 }, @Unit("m"))]
    pub mesa: f32,
}

impl Default for Hills {
    fn default() -> Self {
        Self {
            size: 48.0,
            resolution: 96,
            height: 1.0,
            mesa: 3.0,
        }
    }
}

impl Hills {
    /// Ground height at (x, z).
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let rolling = 0.9 * (x * 0.21).sin() * (z * 0.17).cos()
            + 0.6 * (x * 0.09 + z * 0.13).sin()
            + 0.35 * ((x - z) * 0.33).sin();
        let d = Vec2::new(x - 7.0, z + 5.0).length();
        let t = ((d - 4.0) / 1.5).clamp(0.0, 1.0);
        let mesa = self.mesa * (1.0 - t * t * (3.0 - 2.0 * t));
        rolling * self.height + mesa
    }
}

impl Generator for Hills {
    const VERSION: u32 = 1;
    const CATEGORY: &'static str = "Demo";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let n = self.resolution.clamp(1, 512) as usize;
        let size = if self.size.is_finite() && self.size > 0.0 {
            self.size
        } else {
            48.0
        };
        let step = size / n as f32;
        let half = size * 0.5;
        // Rows run towards −Z so the quads face up.
        let rows: Vec<Vec<Vertex>> = (0..=n)
            .map(|j| {
                let z = half - j as f32 * step;
                (0..=n)
                    .map(|i| {
                        let x = -half + i as f32 * step;
                        Vertex::new(
                            Vec3::new(x, self.height_at(x, z), z),
                            Vec3::ZERO,
                            Vec2::new(x, z) * 0.25,
                        )
                    })
                    .collect()
            })
            .collect();
        let refs: Vec<&[Vertex]> = rows.iter().map(Vec::as_slice).collect();
        let mut b = MeshBuilder::new();
        b.grid(0, &refs);
        GenOutput::mesh(b.build().mesh)
    }
}

/// Registers `Hills`.
pub struct ScatterDemoPlugin;

impl Plugin for ScatterDemoPlugin {
    fn register(&self, registry: &mut Registry) {
        if let Err(e) = registry.register_generator::<Hills>() {
            error!("Hills registration failed: {e}");
        }
    }

    fn build(&self, _app: &mut App) {}
}

fn lathe_mesh(profile: &[(f32, f32)], segments: u32, smooth: f32) -> MeshData {
    let lathe = Lathe {
        shape: LatheShape::Custom {
            profile: profile.iter().map(|&(r, y)| Vec2::new(r, y)).collect(),
            closed: false,
        },
        segments,
        smooth_angle: smooth,
        ..Lathe::default()
    };
    lathe
        .generate(&GenContext::new(0))
        .mesh
        .unwrap_or_else(|| MeshData::cuboid(Vec3::splat(0.2)))
}

/// A low-poly bush (7 sides).
pub fn bush_mesh() -> MeshData {
    lathe_mesh(
        &[
            (0.0, 0.0),
            (0.38, 0.04),
            (0.55, 0.3),
            (0.5, 0.58),
            (0.28, 0.8),
            (0.0, 0.86),
        ],
        7,
        1.2,
    )
}

/// A grass tuft: a thin four-sided spike.
pub fn grass_mesh() -> MeshData {
    lathe_mesh(&[(0.0, 0.0), (0.07, 0.0), (0.05, 0.2), (0.0, 0.55)], 4, 0.0)
}

/// A flat, faceted rock (5 sides).
pub fn rock_mesh() -> MeshData {
    lathe_mesh(
        &[
            (0.0, -0.15),
            (0.55, -0.1),
            (0.6, 0.12),
            (0.4, 0.38),
            (0.0, 0.45),
        ],
        5,
        0.0,
    )
}

/// A flower: a stem and a cup.
pub fn flower_mesh() -> MeshData {
    lathe_mesh(
        &[
            (0.0, 0.0),
            (0.02, 0.0),
            (0.02, 0.3),
            (0.1, 0.34),
            (0.13, 0.44),
            (0.0, 0.4),
        ],
        6,
        0.5,
    )
}

/// Puts `mesh` in the world's assets under `path` (an in-memory asset, no
/// file) and returns its reference.
pub fn add_mesh_asset(world: &mut World, path: &str, mesh: MeshData) -> AssetRef {
    let h = world.resource_mut::<Assets<MeshData>>().add(mesh);
    world
        .resource_or_default::<AssetLookup>()
        .insert(AssetId::new(), None, Some(path), h);
    AssetRef::path(path)
}

/// Puts a standard material of `color` in the world's assets under `path`.
pub fn add_material_asset(world: &mut World, path: &str, color: Color) -> AssetRef {
    let h = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            roughness: 0.9,
            ..StandardMaterial::color(color)
        });
    world
        .resource_or_default::<AssetLookup>()
        .insert(AssetId::new(), None, Some(path), h);
    AssetRef::path(path)
}

/// The demo's species: (grass, bush, rock, pink flower, yellow flower).
pub struct DemoSpecies {
    pub grass: ScatterSpecies,
    pub bush: ScatterSpecies,
    pub rock: ScatterSpecies,
    pub pink: ScatterSpecies,
    pub yellow: ScatterSpecies,
}

/// Registers the demo meshes and materials as in-memory assets.
pub fn demo_species(world: &mut World) -> DemoSpecies {
    let grass_mesh = add_mesh_asset(world, "scatter/grass.mesh", grass_mesh());
    let bush_mesh = add_mesh_asset(world, "scatter/bush.mesh", bush_mesh());
    let rock_mesh = add_mesh_asset(world, "scatter/rock.mesh", rock_mesh());
    let flower_mesh = add_mesh_asset(world, "scatter/flower.mesh", flower_mesh());
    let grass = add_material_asset(world, "scatter/grass.material", Color::hex(0x8fae4a));
    let leaf = add_material_asset(world, "scatter/leaf.material", Color::hex(0x4f7a35));
    let stone = add_material_asset(world, "scatter/stone.material", Color::hex(0x9a948a));
    let pink = add_material_asset(world, "scatter/pink.material", Color::hex(0xe07aa8));
    let yellow = add_material_asset(world, "scatter/yellow.material", Color::hex(0xf2cf4a));
    DemoSpecies {
        grass: ScatterSpecies {
            weight: 6.0,
            scale: Vec2::new(0.7, 1.4),
            tint_jitter: 0.25,
            align_to_normal: 0.5,
            cast_shadows: false,
            ..ScatterSpecies::mesh(grass_mesh, vec![grass])
        },
        bush: ScatterSpecies {
            weight: 0.5,
            scale: Vec2::new(0.45, 1.0),
            tint_jitter: 0.15,
            align_to_normal: 0.2,
            ..ScatterSpecies::mesh(bush_mesh, vec![leaf])
        },
        rock: ScatterSpecies {
            scale: Vec2::new(0.35, 1.0),
            align_to_normal: 1.0,
            tint_jitter: 0.1,
            ..ScatterSpecies::mesh(rock_mesh, vec![stone])
        },
        pink: ScatterSpecies {
            scale: Vec2::new(0.8, 1.2),
            cast_shadows: false,
            ..ScatterSpecies::mesh(flower_mesh.clone(), vec![pink])
        },
        yellow: ScatterSpecies {
            scale: Vec2::new(0.8, 1.2),
            cast_shadows: false,
            ..ScatterSpecies::mesh(flower_mesh, vec![yellow])
        },
    }
}

/// A square area of edge `size` centred on the origin.
pub fn square(size: f32) -> Vec<Vec2> {
    let h = size * 0.5;
    vec![
        Vec2::new(-h, -h),
        Vec2::new(h, -h),
        Vec2::new(h, h),
        Vec2::new(-h, h),
    ]
}

/// The hills, the three scatters and their uids: (hills, meadow, rocks,
/// flowers).
pub fn build_scatter_scene(world: &mut World) -> (Uid, Uid, Uid, Uid) {
    let species = demo_species(world);
    let hills_uid = Uid::new();
    world.spawn((
        hills_uid,
        Name::new("Hills"),
        Transform::IDENTITY,
        Hills::default(),
        StandardMaterial {
            roughness: 0.95,
            ..StandardMaterial::color(Color::hex(0x7d8a52))
        },
    ));
    let area = square(46.0);
    let meadow = Uid::new();
    world.spawn((
        meadow,
        Name::new("Meadow"),
        Transform::IDENTITY,
        Scatter {
            species: vec![species.grass, species.bush],
            density: 2.0,
            min_distance: 0.35,
            max_slope: 0.42,
            area: area.clone(),
            surface: Some(hills_uid),
            seed: 1,
            ..Scatter::default()
        },
    ));
    let rocks = Uid::new();
    world.spawn((
        rocks,
        Name::new("Rocks"),
        Transform::IDENTITY,
        Scatter {
            species: vec![species.rock],
            density: 0.03,
            min_distance: 2.5,
            max_slope: 1.571,
            area,
            surface: Some(hills_uid),
            seed: 2,
            ..Scatter::default()
        },
    ));
    // Flowers only where painted: a winding stroke across the meadow.
    let mut flowers = Scatter {
        species: vec![species.pink, species.yellow],
        density: 4.0,
        min_distance: 0.2,
        max_slope: 0.42,
        surface: Some(hills_uid),
        seed: 3,
        ..Scatter::default()
    };
    for k in 0..=60 {
        let t = k as f32 / 60.0;
        let p = Vec2::new(-18.0 + 30.0 * t, 8.0 + 5.0 * (t * 5.0).sin());
        flowers.mask.paint(&[], p, 2.6, 0.5, false);
    }
    let flowers_uid = Uid::new();
    world.spawn((
        flowers_uid,
        Name::new("Flowers"),
        Transform::IDENTITY,
        flowers,
    ));
    (hills_uid, meadow, rocks, flowers_uid)
}

/// Looking over the hills, the mesa on the right.
pub fn scatter_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.0, 0.5, 2.0),
        distance: 34.0,
        yaw: 0.35,
        pitch: 0.55,
        ..OrbitCamera::default()
    }
}
