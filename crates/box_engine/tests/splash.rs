//! The startup splash in a headless run: drawn over pure black, centred,
//! fully shown at 1 s, and the game does not tick until it has finished.
//!
//! Skipped when `BOX_SKIP_GPU_TESTS` is set.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use box_engine::prelude::*;
use box_engine::run_with;

const W: u32 = 640;
const H: u32 = 360;

/// Runs `frames` headless frames with the splash and returns the last frame
/// and how often the game ticked.
fn run(frames: u64, name: &str) -> (image::RgbaImage, u32) {
    let ticks = Arc::new(AtomicU32::new(0));
    let mut app = Game::new().new_app();
    let counter = ticks.clone();
    app.add_system(Stage::Update, move |_: &mut World| {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let out = std::env::temp_dir().join(format!("box_splash_{name}_{}.png", std::process::id()));
    let mut options = RunOptions::new("splash");
    options.width = W;
    options.height = H;
    options.frames = Some(frames);
    options.capture = Some(out.clone());
    options.splash = true;
    let summary = run_with(app, options).unwrap();
    assert_eq!(summary.validation_errors, 0);
    let image = image::open(&out).unwrap().to_rgba8();
    let _ = std::fs::remove_file(&out);
    (image, ticks.load(Ordering::Relaxed))
}

#[test]
fn splash_shows_then_the_game_starts() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return;
    }
    // 1 s in: fully shown, the game has not ticked.
    let (image, ticks) = run(60, "shown");
    assert_eq!(ticks, 0, "the game ticked during the splash");
    for (x, y) in [(0, 0), (W - 1, 0), (0, H - 1), (W - 1, H - 1), (W / 2, 20)] {
        assert_eq!(image.get_pixel(x, y).0, [0, 0, 0, 255], "({x}, {y})");
    }
    let lit: Vec<(u32, u32)> = image
        .enumerate_pixels()
        .filter(|(_, _, p)| p.0[..3] != [0, 0, 0])
        .map(|(x, y, _)| (x, y))
        .collect();
    assert!(
        image.pixels().any(|p| p.0 == [255, 255, 255, 255]),
        "white wordmark"
    );
    let (x0, x1) = (lit.iter().map(|p| p.0).min(), lit.iter().map(|p| p.0).max());
    let (y0, y1) = (lit.iter().map(|p| p.1).min(), lit.iter().map(|p| p.1).max());
    let centre = (
        (x0.unwrap() + x1.unwrap()) as f32 / 2.0,
        (y0.unwrap() + y1.unwrap()) as f32 / 2.0,
    );
    assert!((centre.0 - W as f32 / 2.0).abs() < 3.0, "{centre:?}");
    assert!((centre.1 - H as f32 / 2.0).abs() < 6.0, "{centre:?}");

    // 2.5 s at 60 fps is 150 splash frames; the game runs the rest.
    let (image, ticks) = run(160, "after");
    assert_eq!(ticks, 10);
    // The game's frame (its sky), not the splash's black.
    assert_ne!(image.get_pixel(0, 0).0, [0, 0, 0, 255]);
}
