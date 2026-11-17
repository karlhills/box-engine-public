//! M1 showcase: lit, shadowed primitives under a sky with fog and an orbit camera.
//!
//! Keys: 1 day, 2 evening, C cascade view, O ambient occlusion view, G grid,
//! T cycle tone mapper, Esc quit. Flags: `--evening`, `--cascades`,
//! `--ao-view`, `--no-ao`, `--ao-radius <m>`, `--ao-intensity <x>`,
//! `--no-bloom`, `--bloom <intensity>`, `--bloom-threshold <exposed luminance>`,
//! `--tonemap <agx|aces|neutral|none>`, `--grade` (the reference afternoon grade:
//! white balance, contrast, shadows/midtones/highlights, saturation, and its
//! vignette), `--vignette` (the vignette alone), `--no-shadows`,
//! `--orbit <radians per second>` (turns the camera continuously, for capture
//! sequences).
//!
//! `cargo run --example showcase -- --capture captures/showcase.png`

mod common;

use box_engine::prelude::*;
use common::*;

fn main() -> anyhow::Result<()> {
    let mut app = box_engine::default_app();
    let evening = std::env::args().any(|a| a == "--evening");
    build_showcase(&mut app.world);
    spawn_sun_and_camera(&mut app.world, showcase_camera());
    set_time_of_day(
        &mut app.world,
        if evening {
            TimeOfDay::Evening
        } else {
            TimeOfDay::Day
        },
    );
    app.world.insert_resource(ShowGrid(false));
    if std::env::args().any(|a| a == "--no-shadows") {
        for l in app.world.query::<&mut DirectionalLight>().iter() {
            l.shadows = false;
        }
    }
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let settings = app.world.resource_mut::<RenderSettings>();
    settings.show_cascades = flag("--cascades");
    settings.show_ambient_occlusion = flag("--ao-view");
    let env = app.world.resource_mut::<Environment>();
    env.ambient_occlusion.enabled = !flag("--no-ao");
    env.bloom.enabled = !flag("--no-bloom");
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<f32>().ok())
    };
    if let Some(v) = value("--ao-radius") {
        env.ambient_occlusion.radius = v;
    }
    if let Some(v) = value("--ao-intensity") {
        env.ambient_occlusion.intensity = v;
    }
    if let Some(v) = value("--bloom") {
        env.bloom.intensity = v;
    }
    if let Some(v) = value("--bloom-threshold") {
        env.bloom.threshold = v;
    }
    if let Some(name) = args
        .iter()
        .position(|a| a == "--tonemap")
        .and_then(|i| args.get(i + 1))
    {
        env.tonemapper = look::parse_tonemapper(name)?;
    }
    if flag("--grade") {
        // The reference afternoon grade and vignette.
        env.color_grading = look::reference_grade();
        env.vignette = look::reference_vignette();
    }
    if flag("--vignette") {
        env.vignette = look::reference_vignette();
    }
    if let Some(speed) = args
        .iter()
        .position(|a| a == "--orbit")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<f32>().ok())
    {
        app.add_system(Stage::Update, move |w: &mut World| {
            let dt = w.resource::<Time>().delta;
            for o in w.query::<&mut OrbitCamera>().iter() {
                o.yaw += speed * dt;
            }
        });
    }
    app.add_system(Stage::Update, spin_system);
    app.add_system(Stage::Update, keys);
    box_engine::run(app, RunOptions::new("box-engine showcase"))
}

struct ShowGrid(bool);

fn keys(world: &mut World) {
    let input = world.resource::<Input>().clone();
    if input.key_pressed(KeyCode::Escape) {
        world.resource_mut::<AppExit>().0 = true;
    }
    if input.key_pressed(KeyCode::Digit1) {
        set_time_of_day(world, TimeOfDay::Day);
    }
    if input.key_pressed(KeyCode::Digit2) {
        set_time_of_day(world, TimeOfDay::Evening);
    }
    if input.key_pressed(KeyCode::C) {
        let s = world.resource_mut::<RenderSettings>();
        s.show_cascades = !s.show_cascades;
    }
    if input.key_pressed(KeyCode::O) {
        let s = world.resource_mut::<RenderSettings>();
        s.show_ambient_occlusion = !s.show_ambient_occlusion;
    }
    if input.key_pressed(KeyCode::G) {
        let g = world.resource_mut::<ShowGrid>();
        g.0 = !g.0;
    }
    if input.key_pressed(KeyCode::T) {
        let env = world.resource_mut::<Environment>();
        env.tonemapper = match env.tonemapper {
            Tonemapper::AgX => Tonemapper::AcesFitted,
            Tonemapper::AcesFitted => Tonemapper::Neutral,
            Tonemapper::Neutral => Tonemapper::None,
            Tonemapper::None => Tonemapper::AgX,
        };
        tracing::info!("tone mapper: {:?}", env.tonemapper);
    }
    if world.resource::<ShowGrid>().0 {
        world.resource_mut::<DebugDraw>().grid(
            Vec3::ZERO,
            40.0,
            1.0,
            Color::linear(0.15, 0.15, 0.15),
        );
    }
}
