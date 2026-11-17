//! RS3 instancing stress: 10,000 meshes on a grid, all the same mesh and
//! material (`--mode identical`) or 20 meshes x 5 materials (`--mode mixed`).
//! `--move <fraction>` moves that fraction of the objects every frame (the
//! change-driven instance upload); without it the scene is idle.
//! `--no-post` turns ambient occlusion and bloom off.
//!
//! `cargo run --release --example instancing_stress -- --mode mixed --headless --frames 300 --report target/instancing_mixed.json`

mod common;

use box_engine::prelude::*;

struct Moving {
    entities: Vec<(Entity, Vec3)>,
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str) -> Option<T> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let count: usize = arg(&args, "--count").unwrap_or(10_000);
    let mixed = arg::<String>(&args, "--mode").is_some_and(|m| m == "mixed");
    let move_fraction: f32 = arg(&args, "--move").unwrap_or(0.0);

    let mut app = box_engine::default_app();
    let w = &mut app.world;
    let meshes: Vec<Handle<MeshData>> = if mixed {
        (0..20)
            .map(|i| {
                let s = 0.5 + 0.05 * i as f32;
                let m = if i % 2 == 0 {
                    MeshData::cuboid(Vec3::new(s, 1.4 - s * 0.5, 0.6))
                } else {
                    MeshData::uv_sphere(s * 0.5, 8 + i, 6 + i / 2)
                };
                w.resource_mut::<Assets<MeshData>>().add(m)
            })
            .collect()
    } else {
        vec![
            w.resource_mut::<Assets<MeshData>>()
                .add(MeshData::cuboid(Vec3::splat(0.8))),
        ]
    };
    let colors = [0xc9a27e, 0x8fb3c9, 0x9bc28a, 0xd98c7a, 0xb8b0d0];
    let materials: Vec<Handle<StandardMaterial>> = colors[..if mixed { 5 } else { 1 }]
        .iter()
        .map(|&c| {
            w.resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::hex(c),
                    roughness: 0.7,
                    ..Default::default()
                })
        })
        .collect();
    let side = (count as f32).sqrt().ceil() as usize;
    let half = side as f32 * 0.75;
    let mut moving = Vec::new();
    let every = if move_fraction > 0.0 {
        (1.0 / move_fraction).round().max(1.0) as usize
    } else {
        0
    };
    for i in 0..count {
        let pos = Vec3::new(
            (i % side) as f32 * 1.5 - half,
            0.5,
            (i / side) as f32 * 1.5 - half,
        );
        let e = w.spawn((
            Transform::from_translation(pos),
            MeshRenderer::new(
                meshes[i % meshes.len()],
                materials[(i / meshes.len()) % materials.len()],
            ),
        ));
        if every > 0 && i % every == 0 {
            moving.push((e, pos));
        }
    }
    let ground = w
        .resource_mut::<Assets<MeshData>>()
        .add(MeshData::plane(side as f32 * 1.6, side as f32 * 1.6));
    let ground_material = w
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::color(Color::hex(0xb5a890)));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(ground, ground_material),
    ));
    common::spawn_sun_and_camera(
        w,
        OrbitCamera {
            distance: side as f32 * 1.1,
            pitch: 0.75,
            ..Default::default()
        },
    );
    common::set_time_of_day(w, common::TimeOfDay::Day);
    if args.iter().any(|a| a == "--no-post") {
        let env = w.resource_mut::<Environment>();
        env.ambient_occlusion.enabled = false;
        env.bloom.enabled = false;
    }
    tracing::info!(
        "instancing_stress: {count} objects, {} meshes x {} materials, {} moving",
        meshes.len(),
        materials.len(),
        moving.len()
    );
    w.insert_resource(Moving { entities: moving });
    app.add_system(Stage::Update, animate);
    box_engine::run(app, RunOptions::new("box-engine instancing stress"))
}

fn animate(world: &mut World) {
    let t = world.resource::<Time>().elapsed;
    world.resource_scope(|world, moving: &mut Moving| {
        for &(e, base) in &moving.entities {
            if let Some(tr) = world.component_mut::<Transform>(e) {
                tr.translation = base + Vec3::Y * (0.3 * (t * 2.0 + base.x).sin());
            }
        }
    });
}
