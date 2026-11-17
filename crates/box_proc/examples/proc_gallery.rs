//! A gallery of the box_proc geometry building blocks, for visual review.
//!
//! Back row, left to right: extrude with a hole, chamfered hexagon,
//! round-bevelled block, lathe vase, partial lathe (270°, capped), lathe
//! dome. Front row: spline tube (rotation-minimizing frames), a closed wall
//! loop with mitred corners, two separate walls mitred against each other,
//! a butt junction (owner capped, subordinate cut on its face), a corner
//! sharper than the mitre limit (bevelled), and a fence (posts by split,
//! rails by sweep).
//!
//! `cargo run -p box_proc --example proc_gallery -- --capture target/captures/proc_gallery.png --size 1920x1080`
//! (add `--view joints` or `--view bevel` for close-ups).

use box_core::pmath;
use box_engine::prelude::*;
use box_proc::extrude::{ExtrudeParams, extrude};
use box_proc::lathe::{LatheParams, lathe};
use box_proc::mesh_builder::{BuiltMesh, MeshBuilder};
use box_proc::poly::{self, Polygon};
use box_proc::spline::{Orientation, Path, Sampling, Spline, SplineMode};
use box_proc::split::{Anchor, Remainder, divide_size, place_bays};
use box_proc::sweep::{JointEnd, JointPlane, Profile, SweepParams, sweep};

fn main() -> anyhow::Result<()> {
    let mut app = box_engine::default_app();
    build(&mut app.world);
    app.world.spawn((
        Transform::default().looking_to(Vec3::new(0.45, -0.75, -0.5), Vec3::Y),
        DirectionalLight {
            shadows: true,
            shadow_distance: 40.0,
            intensity: 3.0,
            ..Default::default()
        },
    ));
    // `--view joints` frames the wall junctions, `--view bevel` the
    // bevelled corner and the fence; the default is the whole gallery.
    let args: Vec<String> = std::env::args().collect();
    let view = args
        .iter()
        .position(|a| a == "--view")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    let (target, distance, yaw, pitch) = match view {
        Some("joints") => (Vec3::new(-1.0, 0.4, 1.4), 7.5, 0.5, 0.75),
        Some("bevel") => (Vec3::new(5.5, 0.4, 1.4), 5.5, -0.5, 0.8),
        _ => (Vec3::new(0.0, 0.4, -0.6), 15.5, 0.2, 0.5),
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
    box_engine::run(app, RunOptions::new("box_proc gallery"))
}

fn material(world: &mut World, hex: u32, roughness: f32) -> MaterialHandle {
    world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::hex(hex),
            roughness,
            ..Default::default()
        })
        .into()
}

/// Spawns a built mesh with one material per slot, logging its warnings.
fn spawn(world: &mut World, name: &str, built: BuiltMesh, materials: &[MaterialHandle], at: Vec3) {
    for w in &built.warnings {
        tracing::info!("{name}: {w}");
    }
    let tris = built.mesh.triangle_count();
    let mesh = world.resource_mut::<Assets<MeshData>>().add(built.mesh);
    world.spawn((
        Name::new(name),
        Transform::from_translation(at),
        MeshRenderer::with_materials(mesh, materials.to_vec()),
    ));
    tracing::info!("{name}: {tris} triangles");
}

fn build(world: &mut World) {
    let ground = material(world, 0x9a9384, 0.95);
    let stone = material(world, 0xb8ad9c, 0.85);
    let plaster = material(world, 0xe9dfcb, 0.9);
    let terracotta = material(world, 0xb5603f, 0.7);
    let wood = material(world, 0x7a5638, 0.8);
    let teal = material(world, 0x3f7f86, 0.4);
    let slate = material(world, 0x58606b, 0.6);

    let floor = world
        .resource_mut::<Assets<MeshData>>()
        .add(MeshData::plane(60.0, 60.0));
    world.spawn((Transform::default(), MeshRenderer::new(floor, ground)));

    let back = -3.5;
    let xs = [-7.5, -4.5, -1.5, 1.5, 4.5, 7.5];

    // Extrude with a hole: rounded rectangle minus a circle.
    let outer = poly::round_corners(
        &Polygon::rect(Vec2::new(-1.2, -0.8), Vec2::new(1.2, 0.8)).outer,
        0.3,
        4,
    );
    let holed = Polygon::new(outer).with_hole(Polygon::circle(Vec2::ZERO, 0.45, 32).outer);
    spawn(
        world,
        "extrude hole",
        extrude(
            &holed,
            &ExtrudeParams {
                depth: 0.8,
                ..Default::default()
            },
        ),
        &[plaster, stone],
        Vec3::new(xs[0], 0.0, back),
    );
    // Chamfered hexagon (bevel on the 2D profile).
    spawn(
        world,
        "extrude chamfer",
        extrude(
            &Polygon::circle(Vec2::ZERO, 1.0, 6),
            &ExtrudeParams {
                depth: 1.0,
                bevel_width: 0.15,
                bevel_slot: 2,
                ..Default::default()
            },
        ),
        &[stone, plaster, terracotta],
        Vec3::new(xs[1], 0.0, back),
    );
    // Round bevel on an L-shaped block whose vertical corners are chamfered in 2D.
    let l_shape = Polygon::new(poly::chamfer_corners(
        &[
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(-1.0, 1.0),
        ],
        0.12,
    ));
    spawn(
        world,
        "extrude round bevel",
        extrude(
            &l_shape,
            &ExtrudeParams {
                depth: 0.7,
                bevel_width: 0.12,
                bevel_segments: 4,
                ..Default::default()
            },
        ),
        &[teal, teal],
        Vec3::new(xs[2], 0.0, back),
    );
    // Lathe vase: open profile from the axis, hard foot and lip, smooth belly.
    let vase = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(0.5, 0.0),
        Vec2::new(0.62, 0.35),
        Vec2::new(0.58, 0.7),
        Vec2::new(0.35, 1.05),
        Vec2::new(0.3, 1.3),
        Vec2::new(0.42, 1.5),
    ];
    spawn(
        world,
        "lathe vase",
        lathe(&LatheParams {
            profile: Profile::new(vase, false),
            segments: 48,
            ..Default::default()
        }),
        &[terracotta],
        Vec3::new(xs[3], 0.0, back),
    );
    // Partial lathe: a ring section swept 270° with caps.
    spawn(
        world,
        "lathe partial",
        lathe(&LatheParams {
            profile: Profile::new(
                vec![
                    Vec2::new(0.6, 0.0),
                    Vec2::new(1.0, 0.0),
                    Vec2::new(1.0, 1.2),
                    Vec2::new(0.6, 1.2),
                ],
                true,
            ),
            segments: 36,
            start_angle: 0.6,
            sweep_angle: 270f32.to_radians(),
            caps: true,
            cap_slot: 1,
        }),
        &[stone, terracotta],
        Vec3::new(xs[4], 0.0, back),
    );
    // Dome with a finial: profile touches the axis at both poles.
    let mut dome = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(1.0, 0.0),
        Vec2::new(1.0, 0.5),
    ];
    dome.extend((1..=12).map(|i| {
        let a = std::f32::consts::FRAC_PI_2 * i as f32 / 12.0;
        Vec2::new(pmath::cos(a), 0.5 + pmath::sin(a) * 0.9)
    }));
    dome.pop();
    dome.extend([
        Vec2::new(0.08, 1.42),
        Vec2::new(0.12, 1.6),
        Vec2::new(0.0, 1.75),
    ]);
    spawn(
        world,
        "lathe dome",
        lathe(&LatheParams {
            profile: Profile::new(dome, false)
                .with_slots(vec![0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2]),
            segments: 40,
            ..Default::default()
        }),
        &[plaster, slate, terracotta],
        Vec3::new(xs[5], 0.0, back),
    );

    // ---- front row: sweeps
    let front = 1.5;
    // Spline tube along a closed Catmull-Rom loop.
    let tube = Spline::from_positions(
        &[
            Vec3::new(-1.0, 0.3, 0.0),
            Vec3::new(0.0, 1.3, -1.0),
            Vec3::new(1.0, 0.3, 0.0),
            Vec3::new(0.0, 0.8, 1.0),
        ],
        SplineMode::CatmullRom,
        true,
    );
    spawn(
        world,
        "spline tube",
        sweep(
            &Path::from_spline(
                &tube,
                Sampling::Adaptive {
                    tolerance: 0.005,
                    max_step: 0.3,
                },
                Orientation::RotationMinimizing(Vec3::Y),
            ),
            &SweepParams {
                profile: Profile::circle(0.12, 16),
                ..Default::default()
            },
        ),
        &[teal],
        Vec3::new(xs[0], 0.0, front),
    );
    // Closed wall loop (mitred corners), with a top edge in a second slot.
    let wall = |w: f32, h: f32| Profile::rect(w, h).with_slots(vec![0, 0, 1, 0]);
    spawn(
        world,
        "wall loop",
        sweep(
            &Path::from_polyline(
                &[
                    Vec3::new(-1.0, 0.0, 0.8),
                    Vec3::new(1.0, 0.0, 0.8),
                    Vec3::new(0.6, 0.0, -0.8),
                    Vec3::new(-1.0, 0.0, -0.6),
                ],
                true,
                Orientation::Up(Vec3::Y),
            ),
            &SweepParams {
                profile: wall(0.2, 1.0),
                ..Default::default()
            },
        ),
        &[plaster, stone],
        Vec3::new(xs[1], 0.0, front),
    );
    // Two separate walls mitred against each other (bit-identical joint).
    let pa = [Vec3::new(-1.0, 0.0, 0.8), Vec3::new(0.6, 0.0, 0.8)];
    let pb = [pa[1], Vec3::new(0.9, 0.0, -0.9)];
    let walls_at = Vec3::new(xs[2], 0.0, front);
    for (name, pts, start, end, mat) in [
        (
            "mitre wall a",
            pa,
            JointEnd::Cap,
            JointEnd::Mitre {
                neighbour_dir: pb[1] - pb[0],
            },
            plaster,
        ),
        (
            "mitre wall b",
            pb,
            JointEnd::Mitre {
                neighbour_dir: pa[1] - pa[0],
            },
            JointEnd::Cap,
            terracotta,
        ),
    ] {
        spawn(
            world,
            name,
            sweep(
                &Path::from_polyline(&pts, false, Orientation::Up(Vec3::Y)),
                &SweepParams {
                    profile: wall(0.2, 1.0),
                    start,
                    end,
                    ..Default::default()
                },
            ),
            &[mat, stone],
            walls_at,
        );
    }
    // Butt: the owner runs through and is capped; the subordinate stops on the owner's face.
    let butt_at = Vec3::new(xs[3], 0.0, front);
    spawn(
        world,
        "butt owner",
        sweep(
            &Path::from_polyline(
                &[Vec3::new(-1.0, 0.0, -0.6), Vec3::new(1.0, 0.0, -0.6)],
                false,
                Orientation::Up(Vec3::Y),
            ),
            &SweepParams {
                profile: wall(0.3, 1.2),
                ..Default::default()
            },
        ),
        &[plaster, stone],
        butt_at,
    );
    spawn(
        world,
        "butt subordinate",
        sweep(
            &Path::from_polyline(
                &[Vec3::new(0.2, 0.0, 1.0), Vec3::new(0.2, 0.0, -0.6)],
                false,
                Orientation::Up(Vec3::Y),
            ),
            &SweepParams {
                profile: wall(0.2, 0.9),
                end: JointEnd::Butt {
                    plane: JointPlane::through_line(Vec3::new(0.0, 0.0, -0.45), Vec3::X, Vec3::Y),
                    cap: false,
                },
                ..Default::default()
            },
        ),
        &[terracotta, stone],
        butt_at,
    );
    // A corner sharper than the mitre limit is bevelled.
    spawn(
        world,
        "bevelled corner",
        sweep(
            &Path::from_polyline(
                &[
                    Vec3::new(-1.0, 0.0, 0.7),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(-1.0, 0.0, -0.4),
                ],
                false,
                Orientation::Up(Vec3::Y),
            ),
            &SweepParams {
                profile: wall(0.25, 1.0),
                ..Default::default()
            },
        ),
        &[plaster, stone],
        Vec3::new(xs[4], 0.0, front),
    );
    // Fence: posts placed per bay with split, rails swept along the same path.
    let fence_path = Path::from_polyline(
        &[
            Vec3::new(-1.1, 0.0, 0.9),
            Vec3::new(0.2, 0.0, 0.6),
            Vec3::new(1.0, 0.0, -0.8),
        ],
        false,
        Orientation::Up(Vec3::Y),
    );
    let mut fence = MeshBuilder::new();
    let posts = place_bays(
        &fence_path,
        &divide_size(fence_path.length, 0.6, Remainder::Stretch),
        Anchor::End,
    );
    let start_post = fence_path.frame_at(0.0);
    for f in std::iter::once(start_post).chain(posts) {
        let post = extrude(
            &Polygon::rect(Vec2::splat(-0.05), Vec2::splat(0.05)),
            &ExtrudeParams {
                depth: 0.9,
                bevel_width: 0.02,
                cap_slot: 0,
                ..Default::default()
            },
        );
        fence.append(&post.mesh, glam::Affine3A::from_translation(f.position));
    }
    for y in [0.35, 0.7] {
        let mut rail = Profile::rect(0.04, 0.08);
        rail.points.iter_mut().for_each(|p| p.y += y);
        box_proc::sweep::sweep_into(
            &mut fence,
            &fence_path,
            &SweepParams {
                profile: rail,
                ..Default::default()
            },
        );
    }
    spawn(
        world,
        "fence",
        fence.build(),
        &[wood],
        Vec3::new(xs[5], 0.0, front),
    );
}
