//! Behaviour gizmos (docs/BEHAVIOURS.md, B2b): with an Enemy, a Moving
//! Platform and a Pickup selected, the viewport draws the enemy's sight and
//! give-up rings and patrol, the platform's path and the pickup's reach.
//! Needs a GPU: capture `target/captures/behaviours/editor_gizmos.png`.

use std::path::PathBuf;

use box_core::math::Aabb;
use box_core::prelude::Transform;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};
use box_engine::behaviours::Chaser;
use glam::Vec3;

/// Pixels close to an sRGB colour.
fn count_near(img: &image::RgbaImage, rgb: [u8; 3], tol: i32) -> usize {
    img.pixels()
        .filter(|p| (0..3).all(|i| (p.0[i] as i32 - rgb[i] as i32).abs() <= tol))
        .count()
}

#[test]
fn selected_behaviours_draw_their_gizmos() {
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
    // An empty stage: only the sun stays of the scratch scene, plus ground.
    {
        let w = &mut editor.state.world;
        let roots: Vec<_> = w
            .query::<(box_core::world::Entity, &box_core::identity::Name)>()
            .iter()
            .filter(|(e, n)| w.parent(*e).is_none() && n.as_str() != "Sun")
            .map(|(e, _)| e)
            .collect();
        for e in roots {
            w.despawn(e);
        }
        w.spawn((
            box_core::identity::Uid::new(),
            box_core::identity::Name::new("Ground"),
            Transform::from_translation(Vec3::new(0.0, -0.5, 0.0)),
            box_core::prelude::Primitive::cuboid(Vec3::new(30.0, 1.0, 30.0)),
        ));
    }
    let place = |editor: &mut box_editor::Editor, preset: &str, at: Vec3| {
        let uid = editor.state.add_preset(preset).expect("placed");
        let w = &mut editor.state.world;
        let e = w.entity_by_uid(uid).unwrap();
        let y = w.get::<Transform>(e).unwrap().translation.y;
        w.component_mut::<Transform>(e).unwrap().translation = Vec3::new(at.x, y, at.z);
        uid
    };
    let enemy = place(&mut editor, "enemy", Vec3::new(-3.0, 0.0, 0.0));
    {
        let w = &mut editor.state.world;
        let e = w.entity_by_uid(enemy).unwrap();
        w.component_mut::<Chaser>(e).unwrap().patrol =
            vec![Vec3::new(0.0, 0.0, 3.0), Vec3::new(3.0, 0.0, 3.0)];
    }
    let platform = place(&mut editor, "moving_platform", Vec3::new(6.0, 0.0, -4.0));
    let pickup = place(&mut editor, "pickup", Vec3::new(3.0, 0.0, 4.0));
    editor.state.select(Some(enemy));
    editor.state.toggle_selected(platform);
    editor.state.toggle_selected(pickup);
    editor.state.camera.frame(&Aabb::new(
        Vec3::new(-9.0, 0.0, -8.0),
        Vec3::new(9.0, 1.0, 8.0),
    ));
    for _ in 0..6 {
        h.frame(&mut editor, Vec::new());
    }
    let img = h.image().unwrap();
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/behaviours");
    std::fs::create_dir_all(&out).unwrap();
    img.save(out.join("editor_gizmos.png")).unwrap();
    // Sight rings (orange), paths (yellow) and reach (green) are drawn.
    assert!(
        count_near(&img, [0xf2, 0x8c, 0x3c], 40) > 200,
        "sight rings"
    );
    assert!(count_near(&img, [0xf2, 0xc9, 0x4c], 40) > 100, "paths");
    assert!(count_near(&img, [0x4c, 0xd9, 0x7b], 40) > 50, "reach");
}
