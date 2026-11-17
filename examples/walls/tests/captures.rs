//! Captures for review, in `target/captures/walls/` (GPU; skipped with
//! `BOX_SKIP_GPU_TESTS`): both scenes from the player, close-ups of each
//! junction kind, a gallery of the instability cases, the editor with a
//! warned wall selected (the Generated section and the warning) and the
//! editor in Play. Every run asserts zero wgpu validation errors.

mod common;

use std::path::PathBuf;

use box_engine::RunOptions;
use box_engine::box_core::derived::Derived;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use walls::authoring;

fn skip() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

fn out(file: &str) -> PathBuf {
    let dir = authoring::project_dir().join("../../target/captures/walls");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(file)
}

/// Renders `app` headlessly to `file` (1600x900) after a few frames.
fn capture(app: App, file: &str) {
    let path = out(file);
    let mut run = RunOptions::new("walls");
    run.capture = Some(path.clone());
    run.frames = Some(4);
    run.width = 1600;
    run.height = 900;
    let summary = box_engine::run_with(app, run).unwrap();
    assert_eq!(summary.validation_errors, 0, "{file}");
    assert!(path.is_file());
}

fn scene_app(scene: &str) -> App {
    let options = PlayerOptions {
        project: Some(authoring::project_dir()),
        scene: Some(scene.into()),
        ..Default::default()
    };
    build_app(&authoring::game(), &options).unwrap().0
}

/// Points the scene's camera from `eye` at `target`.
fn aim(app: &mut App, eye: Vec3, target: Vec3) {
    let cam = app
        .world
        .query::<(Entity, &Camera)>()
        .iter()
        .map(|(e, _)| e)
        .next()
        .unwrap();
    app.world
        .insert_one(
            cam,
            Transform::from_translation(eye).looking_at(target, Vec3::Y),
        )
        .unwrap();
}

#[test]
fn scenes_and_junction_close_ups() {
    if skip() {
        return;
    }
    capture(scene_app("yard"), "yard_overview.png");
    capture(scene_app("court"), "court_overview.png");
    // Close-ups with each wall part tinted differently, so the joints show
    // (all walls share one material otherwise).
    let shots: [(&str, Vec3, Vec3); 8] = [
        (
            "closeup_mitre.png",
            Vec3::new(-8.8, 5.0, -2.8),
            Vec3::new(-6.0, 2.2, -6.0),
        ),
        (
            "closeup_butt.png",
            Vec3::new(0.4, 5.0, -2.8),
            Vec3::new(3.0, 2.2, -6.0),
        ),
        (
            "closeup_t.png",
            Vec3::new(-6.0, 5.0, 5.2),
            Vec3::new(-8.5, 2.2, 2.0),
        ),
        (
            "closeup_x.png",
            Vec3::new(3.6, 5.5, 6.6),
            Vec3::new(1.2, 2.2, 4.0),
        ),
        (
            "closeup_acute_bevel.png",
            Vec3::new(4.2, 4.2, -8.4),
            Vec3::new(7.2, 1.8, -5.9),
        ),
        (
            "closeup_acute_top.png",
            Vec3::new(5.6, 5.4, -5.2),
            Vec3::new(7.3, 2.5, -5.9),
        ),
        (
            "closeup_fence_meets_wall.png",
            Vec3::new(-3.0, 2.2, -1.2),
            Vec3::new(-5.8, 0.6, -3.5),
        ),
        (
            "closeup_fence_posts.png",
            Vec3::new(-2.6, 2.4, 2.6),
            Vec3::new(0.0, 0.4, -0.6),
        ),
    ];
    for (file, eye, target) in shots {
        let mut app = scene_app("yard");
        aim(&mut app, eye, target);
        tint_walls(&mut app);
        capture(app, file);
    }
}

/// Runs one frame (so the parts exist), then gives each wall part its own
/// colour. The rule does not run again (no intent changes), so the tint
/// stays for the capture.
fn tint_walls(app: &mut App) {
    app.update();
    let palette = [0xe4d9c6, 0xb9c7d6, 0xd9b8a8, 0xc3d3b2];
    let mut parts: Vec<(String, Entity)> = app
        .world
        .query::<(Entity, &Derived, &walls::WallGen)>()
        .iter()
        .map(|(e, d, _)| (d.key.to_string(), e))
        .collect();
    parts.sort();
    for (i, (_, e)) in parts.into_iter().enumerate() {
        let mut m = walls::plan::wall_material();
        m.base_color = Color::hex(palette[i % palette.len()]);
        app.world.insert_one(e, m).unwrap();
    }
}

/// Every instability case side by side, built in code, wall parts tinted
/// (back row left to right, then front row): three walls at a point plus
/// an overlapping fourth (warned), an acute corner (bevelled, warned),
/// mixed thickness (butt), a T mid-side; two crossing walls (warned), a
/// wall with zero-length segments (warned), a stem dragged through a body
/// (crossing, warned), two collinear overlapping walls (warned).
#[test]
fn instability_gallery() {
    if skip() {
        return;
    }
    let mut app = scene_app("court");
    // Replace the court's paths with the gallery.
    let paths: Vec<Entity> = app
        .world
        .query::<(Entity, Option<&walls::WallPath>, Option<&walls::FencePath>)>()
        .iter()
        .filter(|(_, w, f)| w.is_some() || f.is_some())
        .map(|(e, ..)| e)
        .collect();
    for e in paths {
        app.world.despawn(e);
    }
    let w = &mut app.world;
    // Two rows of four cells, 6 m apart.
    let mut wall = |cell: usize, name: &str, pts: &[[f32; 2]], thickness: f32| {
        let (ox, oz) = ((cell % 4) as f32 * 6.0 - 9.0, (cell / 4) as f32 * 6.0 - 3.0);
        let moved: Vec<[f32; 2]> = pts.iter().map(|p| [p[0] + ox, p[1] + oz]).collect();
        common::spawn_wall(w, name, &moved, thickness);
    };
    // 0: three at a point plus an overlapping fourth.
    for end in [[2.0, 0.0], [-2.0, 0.0], [0.0, 2.0], [0.35, 1.97]] {
        wall(0, "three", &[[0.0, 0.0], end], 0.3);
    }
    // 1: acute corner (20°).
    let r = 20f32.to_radians();
    wall(1, "acute a", &[[1.5, 0.0], [-2.0, 0.0]], 0.3);
    wall(
        1,
        "acute b",
        &[[1.5, 0.0], [1.5 - 3.5 * r.cos(), 3.5 * r.sin()]],
        0.3,
    );
    // 2: mixed thickness.
    wall(2, "thick", &[[-1.5, 0.0], [1.0, 0.0]], 0.5);
    wall(2, "thin", &[[1.0, 0.0], [1.0, 2.0]], 0.15);
    // 3: T mid-side.
    wall(3, "t body", &[[-2.0, 0.0], [2.0, 0.0]], 0.3);
    wall(3, "t stem", &[[0.0, 0.2], [0.0, 2.0]], 0.2);
    // 4: crossing.
    wall(4, "cross a", &[[-2.0, 0.0], [2.0, 0.0]], 0.3);
    wall(4, "cross b", &[[0.0, -1.5], [0.0, 2.0]], 0.3);
    // 5: zero-length segments.
    wall(
        5,
        "zero",
        &[
            [-1.5, 0.0],
            [-1.5, 0.0],
            [0.0, 0.0],
            [0.0, 0.0005],
            [0.0, 2.0],
        ],
        0.3,
    );
    // 6: dragged through: a stem past the far face.
    wall(6, "through body", &[[-2.0, 0.0], [2.0, 0.0]], 0.3);
    wall(6, "through stem", &[[0.0, -0.6], [0.0, 2.0]], 0.2);
    // 7: collinear overlap.
    wall(7, "overlap a", &[[-2.0, 0.0], [1.0, 0.0]], 0.3);
    wall(7, "overlap b", &[[-0.5, 0.0], [2.0, 0.0]], 0.3);
    aim(
        &mut app,
        Vec3::new(0.0, 15.0, 15.0),
        Vec3::new(0.0, 0.0, 0.5),
    );
    tint_walls(&mut app);
    capture(app, "instability_gallery.png");
}

fn editor(play: bool) -> box_editor::Editor {
    let options = box_editor::EditorOptions {
        project: Some(authoring::project_dir()),
        play,
        ..Default::default()
    };
    box_editor::build_game_editor(
        &options,
        box_editor::log::LogBuffer::default(),
        authoring::game(),
    )
    .unwrap()
}

/// The editor with "Acute A" selected: the inspector shows its `WallPath`,
/// the bevel warning and the Generated section; the viewport shows the
/// warning badge.
#[test]
fn editor_with_a_wall_selected_and_in_play() {
    if skip() {
        return;
    }
    let mut ed = editor(false);
    let uid = common::uid_by_name(&ed.state.world, "Acute A");
    ed.state.select(Some(uid));
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(8.5, 1.0, -5.5);
    orbit.distance = 12.0;
    orbit.yaw = -0.5;
    orbit.pitch = 0.5;
    let image = box_editor::headless::capture(&mut ed, &Default::default()).unwrap();
    image.save(out("editor_wall_selected.png")).unwrap();
    let mark = ed.state.generated_mark(uid).expect("an owner of parts");
    assert_eq!(mark.parts, 1);
    assert!(
        mark.warnings.iter().any(|w| w.contains("bevelled")),
        "{:?}",
        mark.warnings
    );

    let mut ed = editor(true);
    assert!(ed.state.is_playing());
    let image = box_editor::headless::capture(&mut ed, &Default::default()).unwrap();
    image.save(out("editor_play.png")).unwrap();
    assert!(
        common::part_hashes(&ed.state.world).len() >= 13,
        "Play builds the parts"
    );
}
