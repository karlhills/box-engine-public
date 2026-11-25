//! Edit ▸ Snap to Ground (End): floating and buried shapes land on the
//! terrain under them, their lowest point on the ground, as one undo step;
//! a shape outside the terrain stays put. Needs a GPU (meshes give the
//! bounds): captures in `target/captures/snap_to_ground/`.

use std::path::PathBuf;

use box_core::color::Color;
use box_core::identity::{Name, Uid};
use box_core::material::StandardMaterial;
use box_core::math::Aabb;
use box_core::prelude::{Primitive, Transform};
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, entity_bounds, layout};
use box_proc::ground::Ground;
use box_proc::terrain::{Terrain, TerrainStamp};
use egui::{Event, Key, Modifiers};
use glam::{Vec2, Vec3};

fn key(k: Key) -> Vec<Event> {
    [true, false]
        .map(|pressed| Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        })
        .to_vec()
}

#[test]
fn snap_to_ground_lands_the_selection_on_the_terrain() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).expect("editor");
    editor.tree = layout::default_layout();
    let options = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped (no GPU): {e:#}");
            return;
        }
    };
    let w = &mut editor.state.world;
    // An empty stage: only the sun stays of the scratch scene.
    let roots: Vec<_> = w
        .query::<(box_core::world::Entity, &Name)>()
        .iter()
        .filter(|(e, n)| w.parent(*e).is_none() && n.as_str() != "Sun")
        .map(|(e, _)| e)
        .collect();
    for e in roots {
        w.despawn(e);
    }
    w.spawn((
        Uid::new(),
        Name::new("Land"),
        Transform::IDENTITY,
        Terrain {
            size: Vec2::splat(24.0),
            cell_size: 0.5,
            stamps: vec![TerrainStamp::Hill {
                center: Vec2::new(1.0, 0.0),
                radius: 7.0,
                height: 2.5,
            }],
            ..Terrain::default()
        },
    ));
    let shape = |w: &mut box_core::world::World, name: &str, p: Primitive, at: Vec3, c: u32| {
        let uid = Uid::new();
        w.spawn((
            uid,
            Name::new(name),
            Transform::from_translation(at),
            p,
            StandardMaterial::color(Color::hex(c)),
        ));
        uid
    };
    let shapes = [
        shape(
            w,
            "Crate",
            Primitive::cuboid(Vec3::splat(1.0)),
            Vec3::new(-1.0, 6.0, 0.0),
            0xd08040,
        ),
        shape(
            w,
            "Ball",
            Primitive::sphere(0.6),
            Vec3::new(3.0, 5.0, 2.0),
            0x4080d0,
        ),
        shape(
            w,
            "Buried",
            Primitive::cuboid(Vec3::new(2.0, 0.6, 1.0)),
            Vec3::new(1.0, -2.0, -3.0),
            0x60b060,
        ),
    ];
    let outside = shape(
        w,
        "Outside",
        Primitive::cuboid(Vec3::splat(1.0)),
        Vec3::new(30.0, 4.0, 0.0),
        0xb04040,
    );
    editor.state.camera.frame(&Aabb::new(
        Vec3::new(-7.0, 0.0, -7.0),
        Vec3::new(7.0, 6.0, 7.0),
    ));
    for _ in 0..8 {
        h.frame(&mut editor, Vec::new());
    }
    let out =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/snap_to_ground");
    std::fs::create_dir_all(&out).unwrap();
    h.image().unwrap().save(out.join("before.png")).unwrap();

    let s = &mut editor.state;
    s.selection = shapes.iter().copied().chain([outside]).collect();
    let undo_before = s.edits.undo_len();
    h.frame(&mut editor, key(Key::End));
    for _ in 0..4 {
        h.frame(&mut editor, Vec::new());
    }
    h.image().unwrap().save(out.join("after.png")).unwrap();

    let s = &editor.state;
    assert_eq!(s.edits.undo_len(), undo_before + 1, "one undo step");
    assert_eq!(s.edits.undo_label(), Some("Snap 4 entities to Ground"));
    let ground = s.world.resource::<Ground>();
    for uid in shapes {
        let e = s.world.entity_by_uid(uid).unwrap();
        let t = s.world.get::<Transform>(e).unwrap().translation;
        let b = entity_bounds(&s.world, e).expect("mesh bounds");
        let g = ground.height(t.x, t.z).unwrap();
        assert!(
            (b.min.y - g).abs() < 1e-3,
            "lowest point {} on ground {g}",
            b.min.y
        );
    }
    let e = s.world.entity_by_uid(outside).unwrap();
    assert_eq!(
        s.world.get::<Transform>(e).unwrap().translation.y,
        4.0,
        "no terrain there"
    );

    editor.state.undo_scene();
    let e = editor.state.world.entity_by_uid(shapes[0]).unwrap();
    assert_eq!(
        editor
            .state
            .world
            .get::<Transform>(e)
            .unwrap()
            .translation
            .y,
        6.0
    );
}

#[test]
fn a_dropped_model_lands_on_the_terrain_by_its_lowest_point() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    // A project with a 1 x 2 x 1 box model centred on its origin.
    let dir = std::env::temp_dir().join(format!("box_editor_drop_terrain_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, "drop").unwrap();
    let models = dir.join("assets/models");
    std::fs::create_dir_all(&models).unwrap();
    let glb = box_proc::glb::write_glb(
        &box_core::mesh::MeshData::cuboid(Vec3::new(1.0, 2.0, 1.0)),
        "Box",
        &[],
    )
    .unwrap();
    std::fs::write(models.join("box.glb"), glb.bytes).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.tree = layout::default_layout();
    let capture = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut editor, &capture) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped (no GPU): {e:#}");
            return;
        }
    };
    editor.state.world.spawn((
        Uid::new(),
        Name::new("Land"),
        Transform::from_xyz(0.0, 1.0, 0.0),
        Terrain {
            size: Vec2::splat(24.0),
            cell_size: 0.5,
            stamps: vec![TerrainStamp::Hill {
                center: Vec2::new(1.0, 0.0),
                radius: 7.0,
                height: 2.5,
            }],
            ..Terrain::default()
        },
    ));
    editor.state.camera.frame(&Aabb::new(
        Vec3::new(-7.0, 0.0, -7.0),
        Vec3::new(7.0, 6.0, 7.0),
    ));
    for _ in 0..8 {
        h.frame(&mut editor, Vec::new());
    }
    let id = editor
        .state
        .asset_server()
        .unwrap()
        .id_of("models/box.glb")
        .unwrap();
    editor.state.request_file(id);
    assert!(
        editor
            .state
            .wait_assets_idle(std::time::Duration::from_secs(10))
    );

    let at = editor
        .state
        .viewport_drop_point(Vec2::new(0.1, -0.1), 16.0 / 9.0);
    let ground = editor
        .state
        .world
        .resource::<Ground>()
        .height(at.x, at.z)
        .unwrap();
    assert!(
        (at.y - ground).abs() < 1e-3,
        "the drop hits the terrain: {at} vs {ground}"
    );
    let steps = editor.state.edits.undo_len();
    let uid = editor
        .state
        .drop_asset_at(&box_editor::assets::AssetItem::file(id), at)
        .unwrap();
    assert_eq!(editor.state.edits.undo_len(), steps + 1, "one undo step");
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let t = editor.state.world.get::<Transform>(e).unwrap().translation;
    assert!(
        (t.y - (ground + 1.0)).abs() < 1e-3,
        "origin a half height up: {t}"
    );
    for _ in 0..4 {
        h.frame(&mut editor, Vec::new());
    }
    let e = editor.state.world.entity_by_uid(uid).unwrap();
    let b = entity_bounds(&editor.state.world, e).unwrap();
    assert!(
        (b.min.y - ground).abs() < 1e-3,
        "standing on the ground: {}",
        b.min.y
    );
    let out =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/snap_to_ground");
    std::fs::create_dir_all(&out).unwrap();
    h.image().unwrap().save(out.join("dropped.png")).unwrap();

    editor.state.undo_scene();
    assert!(
        editor.state.world.entity_by_uid(uid).is_none(),
        "undo removes it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
