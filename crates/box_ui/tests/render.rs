//! The UI overlay on the GPU: drawn over the scene in a capture and in an
//! offscreen viewport target (the editor's game view), at the expected
//! pixels, with zero wgpu validation errors. Skipped with
//! `BOX_SKIP_GPU_TESTS`.

use box_engine::box_render::Renderer;
use box_engine::box_ui::{Edges, Size, Ui, UiNode};
use box_engine::prelude::*;
use box_engine::{RunOptions, WindowSize};

fn skip() -> bool {
    std::env::var_os("BOX_SKIP_GPU_TESTS").is_some()
}

/// A red box at (50, 50) points, 200 x 100, and white text below it.
fn red_box(world: &mut World) {
    let ui = world.resource_mut::<Ui>();
    ui.add(
        None,
        UiNode::stack()
            .anchored([0.0, 0.0], [60.0, 60.0])
            .with_width(Size::Px(240.0))
            .with_height(Size::Px(120.0))
            .with_style(|s| {
                s.background = Some("#ff0000".into());
                s.padding = Some(Edges::all(0.0));
            }),
    );
    // Translucent black at 50 % over the red box's right half.
    ui.add(
        None,
        UiNode::stack()
            .anchored([0.0, 0.0], [180.0, 60.0])
            .with_width(Size::Px(120.0))
            .with_height(Size::Px(120.0))
            .with_style(|s| s.background = Some("#00000080".into())),
    );
    ui.add(
        None,
        UiNode::text("Hello UI")
            .anchored([0.0, 0.0], [60.0, 240.0])
            .with_style(|s| {
                s.text_size = Some(96.0);
                s.text_color = Some("#ffffff".into());
            }),
    );
}

fn pixel(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    img.get_pixel(x, y).0
}

#[test]
fn the_ui_draws_over_a_capture_blended_in_srgb_space() {
    if skip() {
        return;
    }
    let mut app = box_engine::default_app();
    red_box(&mut app.world);
    let dir = std::env::temp_dir().join(format!("box_ui_render_{}", std::process::id()));
    let out = dir.join("ui.png");
    // 960 x 540: half the reference size, so 1 point = 0.5 px.
    let summary = box_engine::run_with(
        app,
        RunOptions {
            width: 960,
            height: 540,
            capture: Some(out.clone()),
            frames: Some(3),
            ..RunOptions::new("ui render test")
        },
    )
    .expect("headless run");
    assert_eq!(summary.validation_errors, 0);
    let img = image::open(&out).unwrap().to_rgba8();
    // Inside the opaque red box (points 60..180 x 60..180 → px 30..90).
    assert_eq!(pixel(&img, 50, 50), [255, 0, 0, 255]);
    // Black at 50 % over red, blended in sRGB space: 255 * 0.5.
    let p = pixel(&img, 110, 50);
    assert!((p[0] as i32 - 127).abs() <= 2 && p[1] == 0, "{p:?}");
    // The text has white pixels.
    let white = (30..300)
        .flat_map(|x| (120..180).map(move |y| (x, y)))
        .filter(|&(x, y)| pixel(&img, x, y)[..3].iter().all(|c| *c > 240))
        .count();
    assert!(white > 200, "{white} white text pixels");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_ui_draws_into_an_offscreen_viewport_target_like_the_editor_game_view() {
    if skip() {
        return;
    }
    let mut app = box_engine::default_app();
    red_box(&mut app.world);
    *app.world.resource_mut::<WindowSize>() = WindowSize {
        width: 960,
        height: 540,
        scale_factor_milli: 1000,
    };
    app.tick(1.0 / 60.0);
    let mut renderer = Renderer::new_headless(64, 64).expect("renderer");
    let target = renderer.create_viewport_target(960, 540);
    renderer.render_to(&mut app.world, &target, None);
    let img = renderer.read_viewport_target(&target).unwrap();
    assert_eq!(pixel(&img, 50, 50), [255, 0, 0, 255]);
    let p = pixel(&img, 110, 50);
    assert!((p[0] as i32 - 127).abs() <= 2, "{p:?}");
    assert_eq!(renderer.validation_errors(), 0);
    // Removing the UI clears the overlay.
    app.world.resource_mut::<Ui>().clear();
    app.tick(1.0 / 60.0);
    renderer.render_to(&mut app.world, &target, None);
    let img = renderer.read_viewport_target(&target).unwrap();
    assert_ne!(pixel(&img, 50, 50), [255, 0, 0, 255]);
    assert_eq!(renderer.validation_errors(), 0);
}
