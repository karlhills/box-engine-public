//! Spike S3: how much dynamic mesh upload can a frame take?
//!
//! Rewrites the vertices of many grid meshes every frame and periodically grows
//! some of them, forcing buffer reallocation.
//!
//! `cargo run --release --example upload_stress -- --meshes 200 --frames 600 --report docs/measurements/upload.json`

mod common;

use box_engine::prelude::*;

const BASE_CELLS: u32 = 32;

struct Stress {
    meshes: Vec<Handle<MeshData>>,
    cells: Vec<u32>,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let count: usize = args
        .iter()
        .position(|a| a == "--meshes")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let mut app = box_engine::default_app();
    let w = &mut app.world;
    let mat = w
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::hex(0x8fb3c9),
            roughness: 0.6,
            metallic: 0.0,
            emissive: Color::BLACK,
            ..Default::default()
        });
    let side = (count as f32).sqrt().ceil() as usize;
    let mut meshes = Vec::with_capacity(count);
    for i in 0..count {
        let h = w
            .resource_mut::<Assets<MeshData>>()
            .add(MeshData::grid(2.0, 2.0, BASE_CELLS, BASE_CELLS));
        let (x, z) = ((i % side) as f32 * 2.2, (i / side) as f32 * 2.2);
        let offset = side as f32 * 1.1;
        w.spawn((
            Transform::from_xyz(x - offset, 0.0, z - offset),
            MeshRenderer {
                mesh: h,
                materials: vec![mat.into()],
                cast_shadows: false,
            },
        ));
        meshes.push(h);
    }
    w.insert_resource(Stress {
        cells: vec![BASE_CELLS; count],
        meshes,
    });
    common::spawn_sun_and_camera(
        w,
        OrbitCamera {
            distance: side as f32 * 3.0,
            pitch: 0.7,
            ..Default::default()
        },
    );
    common::set_time_of_day(w, common::TimeOfDay::Day);
    tracing::info!("upload_stress: {count} meshes of {BASE_CELLS}x{BASE_CELLS} cells");
    app.add_system(Stage::Update, animate);
    box_engine::run(app, RunOptions::new("box-engine upload stress"))
}

fn animate(world: &mut World) {
    let time = *world.resource::<Time>();
    world.resource_scope(|world, stress: &mut Stress| {
        let assets = world.resource_mut::<Assets<MeshData>>();
        // Every 120 frames, grow every 8th mesh (to force reallocation), cycling.
        let grow_phase = (time.frame / 120) as usize;
        for (i, h) in stress.meshes.iter().enumerate() {
            if time.frame.is_multiple_of(120) && i % 8 == grow_phase % 8 {
                stress.cells[i] += 8;
                let c = stress.cells[i];
                if let Some(m) = assets.get_mut(*h) {
                    *m = MeshData::grid(2.0, 2.0, c, c);
                }
            }
            let Some(m) = assets.get_mut(*h) else {
                continue;
            };
            let phase = i as f32 * 0.37 + time.elapsed * 2.0;
            for (p, n) in m.positions.iter_mut().zip(m.normals.iter_mut()) {
                let s = (p.x * 2.0 + phase).sin();
                let c = (p.z * 2.0 + phase).cos();
                p.y = 0.15 * s * c;
                *n = Vec3::new(
                    -0.3 * (p.x * 2.0 + phase).cos() * c,
                    1.0,
                    0.3 * s * (p.z * 2.0 + phase).sin(),
                )
                .normalize();
            }
        }
    });
}
