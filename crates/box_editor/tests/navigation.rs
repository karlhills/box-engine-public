//! View ▸ Navigation: the navmesh draws over the edit world (baked from the
//! scene's colliders) and follows an edit; it also draws during Play. Needs
//! a GPU: captures in `target/captures/navigation/` (`editor.png`,
//! `editor_moved.png`).

use std::path::PathBuf;

use box_core::color::Color;
use box_core::identity::{Name, Uid};
use box_core::material::StandardMaterial;
use box_core::math::Aabb;
use box_core::prelude::{Primitive, Transform};
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};
use box_engine::box_physics::Collider;
use box_engine::nav_glue::{Navigation, nav_reachable};
use glam::Vec3;

fn wait_for_bake(h: &mut Headless, editor: &mut box_editor::Editor) {
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(16));
        h.frame(editor, Vec::new());
        let ready = editor
            .state
            .world
            .get_resource::<Navigation>()
            .is_some_and(|n| !n.mesh().is_empty() && !n.is_baking());
        if ready {
            for _ in 0..2 {
                h.frame(editor, Vec::new());
            }
            return;
        }
    }
    panic!("the navmesh was not baked");
}

#[test]
fn view_navigation_draws_and_follows_edits() {
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
    let roots: Vec<_> = w
        .query::<(box_core::world::Entity, &Name)>()
        .iter()
        .filter(|(e, n)| w.parent(*e).is_none() && n.as_str() != "Sun")
        .map(|(e, _)| e)
        .collect();
    for e in roots {
        w.despawn(e);
    }
    let block = |w: &mut box_core::world::World, name: &str, size: Vec3, at: Vec3, c: u32| {
        w.spawn((
            Uid::new(),
            Name::new(name),
            Transform::from_translation(at),
            Primitive::cuboid(size),
            StandardMaterial::color(Color::hex(c)),
            Collider::from_primitive(),
        ))
    };
    block(
        w,
        "Ground",
        Vec3::new(24.0, 1.0, 16.0),
        Vec3::new(0.0, -0.5, 0.0),
        0x8a9a7a,
    );
    block(
        w,
        "Wall",
        Vec3::new(10.0, 2.5, 0.4),
        Vec3::new(-2.0, 1.25, 0.0),
        0xb0a090,
    );
    let pillar = block(
        w,
        "Pillar",
        Vec3::new(2.0, 2.5, 2.0),
        Vec3::new(6.0, 1.25, -4.0),
        0xa08070,
    );
    editor.state.run_action("view.navigation").unwrap();
    assert!(editor.state.viewport.view.nav);
    editor.state.camera.frame(&Aabb::new(
        Vec3::new(-12.0, 0.0, -8.0),
        Vec3::new(12.0, 3.0, 8.0),
    ));
    wait_for_bake(&mut h, &mut editor);
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/navigation");
    std::fs::create_dir_all(&out).unwrap();
    h.image().unwrap().save(out.join("editor.png")).unwrap();
    let w = &editor.state.world;
    assert!(nav_reachable(
        w,
        Vec3::new(-2.0, 0.0, -5.0),
        Vec3::new(-2.0, 0.0, 5.0)
    ));
    let near_pillar = w
        .resource::<Navigation>()
        .mesh()
        .closest_point(Vec3::new(6.0, 0.0, -4.0))
        .unwrap();
    assert!(
        glam::Vec2::new(near_pillar.x - 6.0, near_pillar.z + 4.0).length() > 1.2,
        "{near_pillar}"
    );

    // Moving the pillar: the mesh follows.
    editor
        .state
        .world
        .component_mut::<Transform>(pillar)
        .unwrap()
        .translation = Vec3::new(-6.0, 1.25, 4.0);
    for _ in 0..30 {
        h.frame(&mut editor, Vec::new());
    }
    wait_for_bake(&mut h, &mut editor);
    h.image()
        .unwrap()
        .save(out.join("editor_moved.png"))
        .unwrap();
    let mesh = editor.state.world.resource::<Navigation>().mesh();
    let freed = mesh.closest_point(Vec3::new(6.0, 0.0, -4.0)).unwrap();
    assert!(
        glam::Vec2::new(freed.x - 6.0, freed.z + 4.0).length() < 0.2,
        "{freed}"
    );
    let blocked = mesh.closest_point(Vec3::new(-6.0, 0.0, 4.0)).unwrap();
    assert!(
        glam::Vec2::new(blocked.x + 6.0, blocked.z - 4.0).length() > 1.2,
        "{blocked}"
    );
}
