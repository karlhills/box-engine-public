//! UI frame cost at 1920 x 1080 for a HUD (health bar, counter, 8 world
//! labels) plus an open menu (title, 6 buttons, slider, toggles): CPU time
//! of the UI systems when idle, when a HUD value changes every frame and
//! when the camera moves (labels follow), and the GPU time of the overlay
//! pass. Run in release for real numbers:
//!
//! `cargo test --release -p box_ui --test cost -- --nocapture`

use std::time::Instant;

use box_engine::box_ui::{Size, Ui, UiNode};
use box_engine::prelude::*;
use box_engine::{RunOptions, WindowSize};

fn build(world: &mut World) {
    world.spawn((
        Transform::from_xyz(0.0, 6.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y),
        Camera::default(),
    ));
    let ui = world.resource_mut::<Ui>();
    ui.add(
        None,
        UiNode::column()
            .with_class("hud")
            .anchored([0.0, 0.0], [32.0, 32.0])
            .children([
                UiNode::row().children([UiNode::text("Health"), UiNode::progress("health", 0.8)]),
                UiNode::text("Coins [color=$accent]{count}[/color] / 40")
                    .with_id("coins")
                    .with_arg("count", 0),
            ]),
    );
    for i in 0..8 {
        let x = i as f32 * 2.0 - 7.0;
        ui.add(
            None,
            UiNode::text(format!("Enemy {i}"))
                .with_class("label")
                .at_point(Vec3::new(x, 1.0, 0.0))
                .anchored([0.5, 1.0], [0.0, 0.0]),
        );
    }
    ui.add(
        None,
        UiNode::panel()
            .modal()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .with_width(Size::Px(640.0))
            .children([
                UiNode::text("Options").with_class("title"),
                UiNode::slider("volume", "", 0.5, 0.0, 1.0, 0.1).with_text("Music volume"),
                UiNode::toggle("subtitles", "", true).with_text("Subtitles"),
                UiNode::toggle("vibration", "", false).with_text("Vibration"),
            ])
            .children(
                (0..6)
                    .map(|i| UiNode::button(&format!("b{i}"), "").with_text(format!("Button {i}"))),
            ),
    );
}

/// Milliseconds spent in the UI systems, measured by systems ordered
/// around them.
#[derive(Default)]
struct UiTime {
    start: Option<Instant>,
    frame_ms: f64,
}

fn start(w: &mut World) {
    w.resource_mut::<UiTime>().start = Some(Instant::now());
}

fn stop(w: &mut World) {
    let t = w.resource_mut::<UiTime>();
    if let Some(s) = t.start.take() {
        t.frame_ms += s.elapsed().as_secs_f64() * 1000.0;
    }
}

fn ui_app() -> App {
    let mut app = box_engine::default_app();
    *app.world.resource_mut::<WindowSize>() = WindowSize {
        width: 1920,
        height: 1080,
        scale_factor_milli: 1000,
    };
    app.world.insert_resource(UiTime::default());
    app.system(Stage::Update, start).before("ui_input");
    app.system(Stage::Update, stop).after("ui_input");
    app.system(Stage::PostUpdate, start).before("ui_strings");
    app.system(Stage::PostUpdate, stop).after("ui_layout");
    build(&mut app.world);
    for _ in 0..3 {
        app.tick(1.0 / 60.0);
    }
    app
}

/// Mean milliseconds of the UI systems per frame over `n` frames, `change`
/// applied before each.
fn time_systems(app: &mut App, n: u32, mut change: impl FnMut(&mut World, u32)) -> f64 {
    let mut total = 0.0;
    for i in 0..n {
        change(&mut app.world, i);
        app.world.resource_mut::<UiTime>().frame_ms = 0.0;
        app.tick(1.0 / 60.0);
        total += app.world.resource::<UiTime>().frame_ms;
    }
    total / n as f64
}

#[test]
fn ui_frame_cost_hud_and_menu_1080p() {
    let mut app = ui_app();
    let n = if cfg!(debug_assertions) { 20 } else { 200 };
    let idle = time_systems(&mut app, n, |_, _| {});
    let coins = app.world.resource::<Ui>().find(None, "coins").unwrap();
    let health = app.world.resource::<Ui>().find(None, "health").unwrap();
    let hud = time_systems(&mut app, n, |w, i| {
        let ui = w.resource_mut::<Ui>();
        ui.set_arg(coins, "count", i);
        ui.set_value(health, (i % 100) as f32 / 100.0);
    });
    let cam = app.world.first_with::<Camera>().unwrap();
    let labels = time_systems(&mut app, n, |w, i| {
        w.component_mut::<Transform>(cam).unwrap().translation.x = (i as f32 * 0.05).sin();
    });
    println!(
        "UI CPU per frame (HUD + 8 labels + open menu, 1920x1080): idle {idle:.3} ms, HUD value changed {hud:.3} ms, camera moving {labels:.3} ms"
    );

    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    // GPU: the overlay pass time from a headless run's report, with the HUD
    // changing every frame.
    let mut app = ui_app();
    app.add_system(Stage::Update, move |w| {
        let f = w.resource::<Time>().frame;
        w.resource_mut::<Ui>().set_arg(coins, "count", f);
    });
    let summary = box_engine::run_with(
        app,
        RunOptions {
            width: 1920,
            height: 1080,
            headless: true,
            frames: Some(120),
            ..RunOptions::new("ui cost")
        },
    )
    .expect("headless run");
    assert_eq!(summary.validation_errors, 0);
    let report = summary.report.unwrap();
    let ui = &report["gpu_pass_ms"]["ui"];
    println!("UI GPU pass (1920x1080): {ui}");
    println!("whole frame GPU with the UI: {}", report["gpu_ms"]);
    // The same scene without UI, for the whole-frame comparison (the
    // overlay pass alone is near the timer's resolution).
    let mut app = ui_app();
    app.world.resource_mut::<Ui>().clear();
    let base = box_engine::run_with(
        app,
        RunOptions {
            width: 1920,
            height: 1080,
            headless: true,
            frames: Some(120),
            ..RunOptions::new("ui cost baseline")
        },
    )
    .expect("headless run");
    println!(
        "whole frame GPU without UI: {}",
        base.report.unwrap()["gpu_ms"]
    );
    if !cfg!(debug_assertions) {
        assert!(hud < 2.0, "relayout + paint {hud} ms");
        assert!(idle < 0.1, "idle {idle} ms");
    }
}
