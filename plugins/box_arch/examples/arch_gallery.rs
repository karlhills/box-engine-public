//! A reference small house and junction gallery for box_arch, for visual
//! review.
//!
//! - A two-storey house: a closed loop of mitred walls with a door, flat
//!   and arched windows, a plinth and a cornice, under a hip roof whose
//!   underside the walls follow (eave walls tucked into the slab).
//! - A lower annex: three walls meeting the house in T junctions, under a
//!   gable roof abutting the house wall (its gable wall takes its top from
//!   the roof).
//! - A round tower (a closed wall of two arcs) with a door and a window
//!   under a cone roof, and a garden wall running into it (Extend).
//! - Garden walls: a mitred corner with an archway and coping, a thinner
//!   wall butting against it (owner/subordinate L), a T junction, and a
//!   corner sharper than the mitre limit (split with a bevel fill).
//! - A terrace (platform with a trim band) reached by a solid stair, a
//!   round basin (platform with a lip), an open stair.
//! - Decorations on anchors: a lantern over the door (wall opening), a
//!   planter on the garden wall's top, a finial on the tower's apex.
//! - Ambient occlusion on/off: the same corner twice, right one with
//!   `ao = 0`.
//!
//! `cargo run -p box_arch --example arch_gallery -- --capture target/captures/arch/gallery.png --size 1920x1080`
//! Close-ups: `--view reveals`, `--view eaves`, `--view junctions`,
//! `--view ao`, `--view tower`, `--view back`.

use box_arch::anchor::resolve_anchors;
use box_arch::junction::{self, Arm};
use box_arch::{
    Anchor, AnchorSurface, Footprint, Opening, OpeningTop, Platform, PlatformEdge, RidgeAxis, Roof,
    RoofEnd, RoofShape, Stair, StairStyle, Wall, WallEnd, WallPath, WallPoint, WallSide,
};
use box_engine::box_proc::generator::{GenContext, GenOutput, Generator};
use box_engine::box_proc::mesh_builder::MeshBuilder;
use box_engine::prelude::*;

/// A warm Mediterranean palette (sRGB).
mod palette {
    pub const IVORY: [f32; 3] = [0.93, 0.90, 0.84];
    pub const BLUSH: [f32; 3] = [0.90, 0.74, 0.68];
    pub const OCHRE: [f32; 3] = [0.88, 0.76, 0.56];
    pub const CREAM: [f32; 3] = [0.94, 0.87, 0.74];
    pub const TRIM: [f32; 3] = [0.95, 0.92, 0.86];
    pub const SAGE: [f32; 3] = [0.56, 0.64, 0.56];
    pub const IRON: [f32; 3] = [0.13, 0.12, 0.11];
    pub const TERRACOTTA: [f32; 3] = [0.74, 0.40, 0.27];
    pub const SOFFIT: [f32; 3] = [0.80, 0.72, 0.62];
    pub const STONE: [f32; 3] = [0.78, 0.74, 0.66];
    pub const GROUND: [f32; 3] = [0.70, 0.71, 0.55];
    pub const GLASS: [f32; 3] = [0.22, 0.30, 0.34];
}

struct Mats {
    ivory: MaterialHandle,
    blush: MaterialHandle,
    ochre: MaterialHandle,
    cream: MaterialHandle,
    trim: MaterialHandle,
    sage: MaterialHandle,
    iron: MaterialHandle,
    tile: MaterialHandle,
    soffit: MaterialHandle,
    stone: MaterialHandle,
    ground: MaterialHandle,
    glass: MaterialHandle,
    lamp: MaterialHandle,
}

fn mat(world: &mut World, c: [f32; 3], roughness: f32) -> MaterialHandle {
    world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(c[0], c[1], c[2]),
            roughness,
            ..Default::default()
        })
        .into()
}

impl Mats {
    fn new(world: &mut World) -> Self {
        use palette::*;
        let lamp = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.85, 0.55),
                emissive: Color::linear(2.0, 1.4, 0.6),
                ..Default::default()
            })
            .into();
        Self {
            ivory: mat(world, IVORY, 0.9),
            blush: mat(world, BLUSH, 0.9),
            ochre: mat(world, OCHRE, 0.9),
            cream: mat(world, CREAM, 0.9),
            trim: mat(world, TRIM, 0.8),
            sage: mat(world, SAGE, 0.7),
            iron: mat(world, IRON, 0.5),
            tile: mat(world, TERRACOTTA, 0.75),
            soffit: mat(world, SOFFIT, 0.9),
            stone: mat(world, STONE, 0.9),
            ground: mat(world, GROUND, 0.95),
            glass: mat(world, GLASS, 0.15),
            lamp,
        }
    }

    /// Slots: Wall, Reveal, Top, Trim, Glass, Door.
    fn wall(&self, plaster: MaterialHandle) -> Vec<MaterialHandle> {
        vec![plaster, plaster, plaster, self.trim, self.glass, self.sage]
    }

    /// Slots: Roof, Underside, Trim, Ridge.
    fn roof(&self) -> Vec<MaterialHandle> {
        vec![self.tile, self.soffit, self.trim, self.tile]
    }
}

/// Spawns a generator's output (with the generator component, so anchors
/// can find it) at `at`; returns its uid.
fn spawn<G: Generator>(
    world: &mut World,
    name: &str,
    generator: G,
    at: Transform,
    materials: Vec<MaterialHandle>,
) -> Uid {
    let out: GenOutput = generator.generate(&GenContext::new(0));
    for w in &out.warnings {
        tracing::warn!("{name}: {w}");
    }
    let uid = Uid::new();
    let mesh = out.mesh.unwrap_or_default();
    tracing::info!("{name}: {} triangles", mesh.triangle_count());
    let handle = world.resource_mut::<Assets<MeshData>>().add(mesh);
    world
        .spawn_saved(
            uid,
            name,
            (
                at,
                generator,
                MeshRenderer::with_materials(handle, materials),
            ),
        )
        .expect("fresh uid");
    uid
}

fn pts(p: &[(f32, f32)]) -> Vec<Vec2> {
    p.iter().map(|&(x, z)| Vec2::new(x, z)).collect()
}

/// The roof underside in a wall's space when the wall entity sits at the
/// origin and the roof at `roof_at`.
fn underside_for(roof: &Roof, roof_at: Vec3) -> box_arch::RoofUnderside {
    roof.underside()
        .transformed(&Affine3A::from_translation(roof_at))
}

fn build(world: &mut World) {
    let m = Mats::new(world);

    // The island: a round platform, its top the ground.
    spawn(
        world,
        "island",
        Platform {
            footprint: Footprint::Circle {
                radius: 17.0,
                segments: 20,
            },
            thickness: 4.0,
            ..Platform::default()
        },
        Transform::default(),
        vec![m.ground, m.stone, m.stone],
    );

    // ---- The house: 5.0 x 4.0 m outer faces, eaves at 5.6 m. ----
    let t = 0.3;
    let (hx, hz) = (2.5 - t * 0.5, 2.0 - t * 0.5);
    let eave = 5.6;
    let hip = Roof {
        shape: RoofShape::Hip {
            size: Vec2::new(5.0, 4.0),
            ridge: RidgeAxis::Longer,
        },
        ..Roof::default()
    };
    // Counter-clockwise seen from above (the outer side is +w).
    let loop_pts = pts(&[(hx, hz), (hx, -hz), (-hx, -hz), (-hx, hz)]);
    // U: east side 0..3.7, north 3.7..8.4, west 8.4..12.1, south 12.1..16.8.
    let house = Wall {
        roof: Some(underside_for(&hip, Vec3::new(0.0, eave, 0.0))),
        openings: vec![
            Opening::door(13.5),
            Opening::window(15.5),
            Opening::window(13.5).with_sill(3.9),
            Opening::arched_window(15.5).with_sill(3.7),
            Opening::arched_window(1.85),
            Opening::window(1.85).with_sill(3.9),
            Opening::window(6.05).with_sill(3.9),
            Opening::window(10.25).with_sill(3.95),
        ],
        plinth: true,
        cornice: true,
        ..Wall::closed_loop(&loop_pts)
    };
    let house_uid = spawn(
        world,
        "house walls",
        house,
        Transform::default(),
        m.wall(m.blush),
    );
    spawn(
        world,
        "house roof",
        hip,
        Transform::from_xyz(0.0, eave, 0.0),
        m.roof(),
    );

    // ---- The annex: T junctions into the house's west wall, gable roof
    // abutting it. ----
    let ta = 0.25;
    let annex_eave = 2.6;
    let annex_roof = Roof {
        shape: RoofShape::Gable {
            size: Vec2::new(3.0, 3.05),
            ridge: RidgeAxis::X,
        },
        end: RoofEnd::Abut,
        pitch: 25f32.to_radians(),
        ..Roof::default()
    };
    let annex_roof_at = Vec3::new(-4.0, annex_eave, 0.0);
    let ax = -2.5 - 3.0 + ta * 0.5;
    let az = 1.525 - ta * 0.5;
    let annex_pts = pts(&[(-hx, -az), (ax, -az), (ax, az), (-hx, az)]);
    let body = Vec2::Y; // plan z
    let start = junction::tee(&Arm::start(annex_pts[0], annex_pts[1], ta), body, t);
    let end = junction::tee(&Arm::end(annex_pts[2], annex_pts[3], ta), body, t);
    spawn(
        world,
        "annex walls",
        Wall {
            thickness: ta,
            roof: Some(underside_for(&annex_roof, annex_roof_at)),
            start,
            end,
            openings: vec![
                Opening::door(4.4).with_top(OpeningTop::Round),
                Opening::window(1.7),
            ],
            plinth: true,
            ..Wall::through(&annex_pts)
        },
        Transform::default(),
        m.wall(m.ochre),
    );
    spawn(
        world,
        "annex roof",
        annex_roof,
        Transform::from_translation(annex_roof_at),
        m.roof(),
    );

    // ---- The tower: two arcs, cone roof; a garden wall runs into it. ----
    let tc = Vec2::new(7.0, -4.0);
    let tr = 1.2;
    let tower_eave = 6.4;
    let cone = Roof {
        shape: RoofShape::Cone {
            radius: tr + 0.15,
            segments: 40,
        },
        pitch: 52f32.to_radians(),
        overhang: 0.3,
        ..Roof::default()
    };
    // Starting at the west point, bulging right (south) first.
    let tower_uid = spawn(
        world,
        "tower",
        Wall {
            path: WallPath::Points {
                points: vec![
                    WallPoint::new(-tr, 0.0).with_bulge(1.0),
                    WallPoint::new(tr, 0.0).with_bulge(1.0),
                ],
                closed: true,
            },
            roof: Some(underside_for(&cone, Vec3::new(0.0, tower_eave, 0.0))),
            openings: vec![
                Opening::arched_door(1.885),
                Opening::arched_window(1.3).with_sill(3.6),
                Opening::window(5.6).with_sill(3.4),
            ],
            plinth: true,
            ..Wall::default()
        },
        Transform::from_xyz(tc.x, 0.0, tc.y),
        m.wall(m.cream),
    );
    let cone_uid = spawn(
        world,
        "tower roof",
        cone,
        Transform::from_xyz(tc.x, tower_eave, tc.y),
        m.roof(),
    );
    spawn(
        world,
        "wall into tower",
        Wall {
            height: 1.5,
            start: WallEnd::Extend { distance: 0.6 },
            coping: true,
            plinth: true,
            ..Wall::through(&pts(&[(tc.x, tc.y + tr + 0.15), (tc.x, 1.2)]))
        },
        Transform::default(),
        m.wall(m.ivory),
    );

    // ---- Terrace with a trim band, solid stair up to it. ----
    spawn(
        world,
        "terrace",
        Platform {
            footprint: Footprint::Rectangle {
                size: Vec2::new(3.0, 4.0),
            },
            thickness: 1.2,
            edge: PlatformEdge::Trim {
                height: 0.12,
                proud: 0.04,
            },
            ..Platform::default()
        },
        Transform::from_xyz(4.0, 1.2, 0.0),
        vec![m.stone, m.ivory, m.trim],
    );
    spawn(
        world,
        "terrace stair",
        Stair {
            start: Vec3::new(4.0, 0.0, 4.0),
            end: Vec3::new(4.0, 1.2, 2.0),
            width: 1.2,
            ..Stair::default()
        },
        Transform::default(),
        vec![m.stone, m.ivory, m.ivory],
    );
    // A round basin (lip) and an open stair.
    spawn(
        world,
        "basin",
        Platform {
            footprint: Footprint::Circle {
                radius: 1.1,
                segments: 28,
            },
            thickness: 0.25,
            edge: PlatformEdge::Lip {
                height: 0.55,
                width: 0.22,
            },
            ..Platform::default()
        },
        Transform::from_xyz(3.2, 0.25, 6.6),
        vec![m.sage, m.stone, m.stone],
    );
    spawn(
        world,
        "open stair",
        Stair {
            start: Vec3::new(9.2, 0.0, 2.6),
            end: Vec3::new(9.2, 1.6, -0.2),
            width: 0.9,
            style: StairStyle::Open,
            ..Stair::default()
        },
        Transform::default(),
        vec![m.sage, m.sage, m.iron],
    );
    spawn(
        world,
        "landing",
        Platform {
            footprint: Footprint::Rectangle {
                size: Vec2::new(1.6, 1.4),
            },
            thickness: 0.2,
            ..Platform::default()
        },
        Transform::from_xyz(9.2, 1.6, -0.9),
        vec![m.sage, m.sage, m.sage],
    );

    // ---- Garden walls: mitre + archway, butt (mixed thickness), T, acute. ----
    let a_pts = pts(&[(-8.5, 2.6), (-8.5, 6.8), (-3.0, 6.8)]);
    let (ta, tb) = (0.4, 0.25);
    let owner = Arm::end(a_pts[1], a_pts[2], ta);
    let c_pts = pts(&[(-3.0, 6.8), (-3.0, 4.3)]);
    let sub = Arm::start(c_pts[0], c_pts[1], tb);
    let (a_end, c_start) = junction::butt(&owner, &sub);
    let garden_uid = spawn(
        world,
        "garden wall",
        Wall {
            thickness: ta,
            height: 3.0,
            end: a_end,
            openings: vec![Opening::archway(4.2 + 2.9)],
            coping: true,
            plinth: true,
            ..Wall::through(&a_pts)
        },
        Transform::default(),
        m.wall(m.ivory),
    );
    spawn(
        world,
        "butted wall",
        Wall {
            thickness: tb,
            height: 1.8,
            start: c_start,
            coping: true,
            plinth: true,
            ..Wall::through(&c_pts)
        },
        Transform::default(),
        m.wall(m.ivory),
    );
    let stem_pts = pts(&[(-8.5, 4.2), (-6.0, 4.2)]);
    spawn(
        world,
        "tee wall",
        Wall {
            thickness: tb,
            height: 1.6,
            start: junction::tee(&Arm::start(stem_pts[0], stem_pts[1], tb), Vec2::Y, ta),
            coping: true,
            plinth: true,
            ..Wall::through(&stem_pts)
        },
        Transform::default(),
        m.wall(m.cream),
    );
    spawn(
        world,
        "acute wall",
        Wall {
            thickness: 0.3,
            height: 1.2,
            coping: true,
            ..Wall::through(&pts(&[(-11.5, -3.0), (-11.5, 2.0), (-9.8, -3.0)]))
        },
        Transform::default(),
        m.wall(m.cream),
    );

    // ---- AO on and off: the same corner with a door and a window. ----
    for (i, ao) in [1.0f32, 0.0].into_iter().enumerate() {
        let x = 9.0 + i as f32 * 4.2;
        spawn(
            world,
            if ao > 0.0 {
                "corner ao on"
            } else {
                "corner ao off"
            },
            Wall {
                height: 2.6,
                ao,
                openings: vec![Opening::door(1.3), Opening::window(4.1)],
                ..Wall::through(&pts(&[(x, 5.0), (x, 7.5), (x + 3.0, 7.5)]))
            },
            Transform::default(),
            m.wall(m.ivory),
        );
    }

    // ---- Decorations on anchors. ----
    let lantern = lantern_mesh();
    let lantern = world.resource_mut::<Assets<MeshData>>().add(lantern);
    world.spawn((
        Name::new("door lantern"),
        Transform::default(),
        Anchor::new(
            house_uid,
            AnchorSurface::WallOpening {
                index: 0,
                side: WallSide::Outer,
            },
            Vec3::new(0.75, -0.35, 0.0),
        ),
        MeshRenderer::with_materials(lantern, vec![m.iron, m.lamp]),
    ));
    let planter = world.resource_mut::<Assets<MeshData>>().add(planter_mesh());
    world.spawn((
        Name::new("planter"),
        Transform::default(),
        Anchor::new(
            garden_uid,
            AnchorSurface::WallTop,
            Vec3::new(2.0, 0.06, 0.0),
        ),
        MeshRenderer::with_materials(planter, vec![m.tile, m.sage]),
    ));
    let finial = world.resource_mut::<Assets<MeshData>>().add(finial_mesh());
    world.spawn((
        Name::new("finial"),
        Transform::default(),
        Anchor::new(
            cone_uid,
            AnchorSurface::RoofRidge,
            Vec3::new(0.0, -0.05, 0.0),
        ),
        MeshRenderer::new(finial, m.iron),
    ));
    let sign = world.resource_mut::<Assets<MeshData>>().add(sign_mesh());
    world.spawn((
        Name::new("tower sign"),
        Transform::default(),
        Anchor::new(
            tower_uid,
            AnchorSurface::WallFace {
                side: WallSide::Outer,
            },
            Vec3::new(2.95, 2.3, 0.0),
        ),
        MeshRenderer::with_materials(sign, vec![m.iron, m.sage]),
    ));
}

trait WithSill {
    fn with_sill(self, sill: f32) -> Self;
}

impl WithSill for Opening {
    fn with_sill(mut self, sill: f32) -> Self {
        self.sill = sill;
        self
    }
}

/// A wall lantern modelled with +Z out of the wall: bracket (slot 0) and
/// glowing lamp (slot 1).
fn lantern_mesh() -> MeshData {
    let mut mb = MeshBuilder::new();
    let at = |p: Vec3| Affine3A::from_translation(p);
    mb.append(
        &MeshData::cuboid(Vec3::new(0.12, 0.2, 0.03)),
        at(Vec3::new(0.0, 0.0, 0.015)),
    );
    mb.append(
        &MeshData::cuboid(Vec3::new(0.03, 0.03, 0.3)),
        at(Vec3::new(0.0, 0.08, 0.15)),
    );
    mb.append(
        &MeshData::cuboid(Vec3::new(0.2, 0.03, 0.2)),
        at(Vec3::new(0.0, 0.03, 0.3)),
    );
    let mut lamp = MeshData::cuboid(Vec3::new(0.15, 0.22, 0.15));
    for g in &mut lamp.groups {
        g.material_slot = 1;
    }
    if lamp.groups.is_empty() {
        lamp.groups.push(MaterialGroup {
            first_index: 0,
            index_count: lamp.indices.len() as u32,
            material_slot: 1,
        });
    }
    mb.append(&lamp, at(Vec3::new(0.0, -0.1, 0.3)));
    mb.finish()
}

/// A planter box (slot 0) with a mound of greenery (slot 1).
fn planter_mesh() -> MeshData {
    let mut mb = MeshBuilder::new();
    mb.append(
        &MeshData::cuboid(Vec3::new(0.9, 0.3, 0.35)),
        Affine3A::from_translation(Vec3::new(0.0, 0.15, 0.0)),
    );
    let mut green = Primitive::sphere(0.3).mesh();
    green.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: green.indices.len() as u32,
        material_slot: 1,
    }];
    for x in [-0.25f32, 0.05, 0.3] {
        mb.append(
            &green,
            Affine3A::from_scale_rotation_translation(
                Vec3::new(0.9, 0.7, 0.6),
                Quat::IDENTITY,
                Vec3::new(x, 0.32, 0.0),
            ),
        );
    }
    mb.finish()
}

/// A small finial: a rod and a ball.
fn finial_mesh() -> MeshData {
    let mut mb = MeshBuilder::new();
    mb.append(
        &Primitive::cylinder(0.035, 0.7).mesh(),
        Affine3A::from_translation(Vec3::new(0.0, 0.35, 0.0)),
    );
    mb.append(
        &Primitive::sphere(0.09).mesh(),
        Affine3A::from_translation(Vec3::new(0.0, 0.62, 0.0)),
    );
    mb.finish()
}

/// A hanging sign on a bracket (+Z out of the wall).
fn sign_mesh() -> MeshData {
    let mut mb = MeshBuilder::new();
    let at = |p: Vec3| Affine3A::from_translation(p);
    mb.append(
        &MeshData::cuboid(Vec3::new(0.04, 0.04, 0.7)),
        at(Vec3::new(0.0, 0.0, 0.35)),
    );
    let mut board = MeshData::cuboid(Vec3::new(0.04, 0.4, 0.5));
    board.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: board.indices.len() as u32,
        material_slot: 1,
    }];
    mb.append(&board, at(Vec3::new(0.0, -0.25, 0.4)));
    mb.finish()
}

fn main() -> anyhow::Result<()> {
    box_engine::init_logging();
    let mut app = box_engine::default_app();
    build(&mut app.world);
    app.add_system(Stage::PostUpdate, |w| {
        resolve_anchors(w);
    });
    {
        let env = app.world.resource_mut::<Environment>();
        *env = Environment::day();
        env.fog_density = 0.004;
    }
    app.world.spawn((
        Transform::default().looking_to(Vec3::new(-0.55, -0.55, -0.62), Vec3::Y),
        DirectionalLight {
            shadows: true,
            shadow_distance: 45.0,
            intensity: 3.4,
            color: Color::linear(1.0, 0.9, 0.76),
        },
    ));
    let args: Vec<String> = std::env::args().collect();
    let view = args
        .iter()
        .position(|a| a == "--view")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    // (target, distance, yaw, pitch)
    let (target, distance, yaw, pitch) = match view {
        Some("reveals") => (Vec3::new(0.6, 2.2, 2.0), 6.5, 0.35, 0.12),
        Some("eaves") => (Vec3::new(2.5, 5.3, 2.0), 3.2, 0.75, -0.2),
        Some("junctions") => (Vec3::new(-6.0, 1.0, 4.6), 10.0, 3.6, 0.6),
        Some("ao") => (Vec3::new(12.6, 1.1, 6.4), 9.0, 3.45, 0.3),
        Some("tower") => (Vec3::new(6.5, 3.5, -2.5), 10.0, 0.45, 0.25),
        Some("back") => (Vec3::new(-2.0, 2.5, -1.0), 20.0, 3.6, 0.45),
        _ => (Vec3::new(0.5, 1.5, 1.2), 27.0, 0.45, 0.5),
    };
    let orbit = OrbitCamera {
        target,
        distance,
        yaw,
        pitch,
        ..Default::default()
    };
    app.world
        .spawn((orbit.transform(), Camera::default(), orbit));
    box_engine::run(app, RunOptions::new("box_arch gallery"))
}
