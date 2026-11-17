//! M8, the second game: `examples/cinder_keep`, a data project (Luau only)
//! played by the stock engine (`build_app` with `Game::new()`, no game
//! code), headlessly.
//!
//! - `cinder_keep_levels_are_imported_with_colliders`: the two levels are
//!   glTF files made outside the editor, placed as prefabs; the importer's
//!   `colliders` setting gives every level mesh a triangle-mesh collider.
//! - `cinder_keep_plays_start_to_finish`: the scripted playthrough in
//!   `m8/playthrough.rs` (gamepad input only): dialogue, checkpoints,
//!   crates, rockfall, death and reload, the jump, the full wave, the gate,
//!   the scene transition, the pause menu's restart, the throne. CI runs it
//!   with `BOX_SKIP_GPU_TESTS` (no captures).
//! - `cinder_keep_wave_performance`: frame, script and render cost with the
//!   full wave on screen at 1920x1080 (numbers printed; run it with
//!   `cargo test --release -p box_engine --test m8_game wave -- --nocapture`).
//!
//! Captures go to `target/captures/m8/`.

#[path = "m8/playthrough.rs"]
mod playthrough;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use box_engine::box_assets::AssetServer;
use box_engine::box_physics::{ColliderShape, Physics};
use box_engine::box_script::ScriptRuntime;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_render::RenderError;
use playthrough::Host;

const DT: f32 = 1.0 / 60.0;

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cinder_keep")
}

fn captures() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m8");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fresh save folder per app, so tests do not share slots.
fn save_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "box_m8_saves_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

fn app(scene: &str) -> App {
    let options = PlayerOptions {
        project: Some(project()),
        scene: Some(scene.into()),
        watch: false,
        ..Default::default()
    };
    let (mut app, _) = build_app(&Game::new(), &options).unwrap();
    let dir = save_dir();
    let _ = std::fs::remove_dir_all(&dir);
    app.world.resource_mut::<SaveConfig>().dir = Some(dir);
    app
}

fn renderer(w: u32, h: u32) -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return None;
    }
    match Renderer::new_headless(w, h) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping captures");
            None
        }
        Err(e) => panic!("renderer: {e}"),
    }
}

/// The player app as a playthrough host, capturing at `size` when a GPU is
/// there.
struct PlayerHost {
    app: App,
    renderer: Option<Renderer>,
    frames: usize,
}

impl PlayerHost {
    fn new(scene: &str, w: u32, h: u32) -> Self {
        let mut app = app(scene);
        app.world.insert_resource(WindowSize {
            width: w,
            height: h,
            scale_factor_milli: 1000,
        });
        Self {
            app,
            renderer: renderer(w, h),
            frames: 0,
        }
    }
}

impl Host for PlayerHost {
    fn world(&mut self) -> &mut World {
        &mut self.app.world
    }

    fn step(&mut self) {
        if let Some(s) = self.app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut self.app.world, Duration::from_millis(50));
        }
        self.app.run_frames(1, DT);
        // As the runner does: per-frame input edges end with the frame.
        self.app.world.resource_mut::<Input>().end_frame();
        self.frames += 1;
    }

    fn capture(&mut self, name: &str) {
        let Some(r) = &mut self.renderer else { return };
        let img = r.capture(&mut self.app.world).expect("capture");
        img.save(captures().join(name)).unwrap();
        assert_eq!(
            r.stats().validation_errors,
            0,
            "{name}: GPU validation errors"
        );
    }
}

#[test]
fn cinder_keep_levels_are_imported_with_colliders() {
    for (scene, root, at, top) in [
        ("outdoor", "Fields", Vec3::new(0.0, 10.0, 0.0), 0.0),
        ("outdoor", "Fields", Vec3::new(0.0, 10.0, -45.0), 2.0),
        ("keep", "Hall", Vec3::new(4.0, 5.0, -26.0), 2.0),
    ] {
        let mut h = PlayerHost {
            app: app(scene),
            renderer: None,
            frames: 0,
        };
        playthrough::frames(&mut h, 3);
        let w = &h.app.world;
        let root = w.find_by_name(root).expect("the level prefab");
        // The prefab's contents: named glTF nodes with mesh colliders.
        let meshes: Vec<String> = w
            .query::<(&Name, &Collider, &box_core::prefab::Instanced)>()
            .iter()
            .filter(|(_, c, i)| c.shape == ColliderShape::Mesh && i.instance == root)
            .map(|(n, _, _)| n.as_str().to_string())
            .collect();
        assert!(meshes.len() >= 6, "{scene}: {meshes:?}");
        let hit = w
            .resource::<Physics>()
            .ray(at, Vec3::NEG_Y, 20.0)
            .unwrap_or_else(|| panic!("{scene}: nothing under {at}"));
        assert!((hit.point.y - top).abs() < 1e-3, "{scene}: {:?}", hit.point);
        let name = w.get::<Name>(hit.entity).unwrap().as_str().to_string();
        assert!(meshes.contains(&name), "{scene}: hit {name}");
    }
}

#[test]
fn cinder_keep_plays_start_to_finish() {
    let mut h = PlayerHost::new("outdoor", 1280, 720);
    let started = Instant::now();
    let mut report = playthrough::play_through(&mut h);
    report.frames = h.frames;
    eprintln!(
        "cinder keep: {:?} in {:.1} s wall",
        report,
        started.elapsed().as_secs_f64()
    );
    assert!(report.kills >= 120, "{report:?}");
    assert!(report.score >= 1200, "{report:?}");
}

/// The committed luau-lsp definitions match the engine (regenerate with
/// `cargo run -p box_cli -- script types --project examples/cinder_keep`);
/// `luau_definitions` runs luau-lsp over the game's scripts.
#[test]
fn cinder_keep_definitions_are_current() {
    let path = project().join(box_engine::box_script::types::DEFINITIONS_PATH);
    let committed = std::fs::read_to_string(&path).expect("committed definitions");
    assert!(
        committed == Game::new().script_definitions(),
        "{} is out of date: run `cargo run -p box_cli -- script types --project examples/cinder_keep`",
        path.display()
    );
    assert!(committed.contains("declare app:"));
}

/// The pause menu by gamepad: language switch (German string tables),
/// back, and Quit (asks the runner to exit). Plus the keyboard: Escape.
#[test]
fn cinder_keep_menu_switches_language_and_quits() {
    use playthrough::{focused, tap, ui_shown, ui_text};
    let mut h = PlayerHost::new("outdoor", 1280, 720);
    playthrough::wait_for(&mut h, "the HUD", 600, |w| {
        w.find_by_name("Game").is_some()
            && playthrough::field::<f64>(w, playthrough::game(w), "health").is_some()
    });
    playthrough::frames(&mut h, 60);
    tap(&mut h, GamepadButton::Start);
    assert!(ui_shown(&h.app.world, "pause_menu"));
    for expected in ["restart", "language"] {
        tap(&mut h, GamepadButton::DPadDown);
        assert_eq!(focused(&h.app.world).as_deref(), Some(expected));
    }
    tap(&mut h, GamepadButton::South);
    playthrough::frames(&mut h, 2);
    let w = &h.app.world;
    assert_eq!(ui_text(w, "language"), "Sprache: Deutsch");
    assert_eq!(ui_text(w, "restart"), "Vom Kontrollpunkt neu starten");
    assert_eq!(ui_text(w, "health"), "Leben 100");
    h.capture("pause_menu_de.png");
    // East closes it; Escape opens and closes it too.
    tap(&mut h, GamepadButton::East);
    assert!(!ui_shown(&h.app.world, "pause_menu"));
    for open in [true, false] {
        h.app
            .world
            .resource_mut::<Input>()
            .press_key(KeyCode::Escape);
        h.step();
        h.app
            .world
            .resource_mut::<Input>()
            .release_key(KeyCode::Escape);
        h.step();
        assert_eq!(ui_shown(&h.app.world, "pause_menu"), open);
    }
    // Quit: down three times from Resume, then South.
    tap(&mut h, GamepadButton::Start);
    for _ in 0..3 {
        tap(&mut h, GamepadButton::DPadDown);
    }
    assert_eq!(focused(&h.app.world).as_deref(), Some("quit"));
    assert!(!h.app.exit_requested());
    tap(&mut h, GamepadButton::South);
    assert!(h.app.exit_requested(), "Quit asks the runner to exit");
}

/// A high view over the whole outdoor level (the imported glTF level with
/// the scene's gameplay entities), for the record.
#[test]
fn cinder_keep_outdoor_overview_capture() {
    let mut h = PlayerHost::new("outdoor", 1920, 1080);
    if h.renderer.is_none() {
        return;
    }
    playthrough::frames(&mut h, 30);
    let camera = h.app.world.find_by_name("Camera").unwrap();
    h.app.world.remove_one::<Script>(camera).unwrap();
    playthrough::frames(&mut h, 2);
    let eye = Vec3::new(26.0, 30.0, 18.0);
    let target = Vec3::new(0.0, 0.0, -24.0);
    let t = h.app.world.component_mut::<Transform>(camera).unwrap();
    t.translation = eye;
    t.rotation = Transform::from_translation(eye)
        .looking_at(target, Vec3::Y)
        .rotation;
    // Hide the HUD and title for the overview.
    h.app.world.resource_mut::<box_engine::box_ui::Ui>().clear();
    playthrough::frames(&mut h, 3);
    h.capture("outdoor_overview.png");
}

/// Per-frame numbers of one measured phase.
#[derive(Default)]
struct Phase {
    update: Vec<f64>,
    scripts: Vec<f64>,
    render: Vec<f64>,
    draws: Vec<f64>,
    enemies: Vec<f64>,
    embers: Vec<f64>,
    particles: Vec<f64>,
}

impl Phase {
    fn print(&self, title: &str) {
        let stat = |v: &[f64]| {
            let mut s = v.to_vec();
            s.sort_by(f64::total_cmp);
            let mean = s.iter().sum::<f64>() / s.len() as f64;
            (mean, s[s.len() / 2], s[s.len() * 95 / 100], s[s.len() - 1])
        };
        eprintln!("{title} ({} frames)", self.update.len());
        for (name, v) in [
            ("update ms", &self.update),
            ("scripts ms", &self.scripts),
            ("render ms", &self.render),
            ("draws", &self.draws),
            ("enemies", &self.enemies),
            ("embers", &self.embers),
            ("particles", &self.particles),
        ] {
            let (mean, median, p95, max) = stat(v);
            eprintln!(
                "  {name:>10}: mean {mean:8.3}  median {median:8.3}  p95 {p95:8.3}  max {max:8.3}"
            );
        }
    }
}

fn count_named(world: &World, names: &[&str]) -> usize {
    world
        .query::<&Name>()
        .iter()
        .filter(|n| names.contains(&n.as_str()))
        .count()
}

/// Frame cost with the full wave at 1920x1080: update (all systems; the
/// scripts' share from `ScriptStats`) and render (GPU waited for), first
/// while the whole wave swarms the hero and spitters throw embers, then
/// while the hero sweeps every half second (kills, bursts of sparks). Run
/// with 120 cinderlings (the game) and 200 (the R24 stress).
#[test]
fn cinder_keep_wave_performance() {
    let (w, h) = (1920, 1080);
    for count in [120, 200] {
        let mut host = PlayerHost::new("outdoor", w, h);
        let Some(mut renderer) = host.renderer.take() else {
            return;
        };
        playthrough::wait_for(&mut host, "the game", 600, |w| {
            w.find_by_name("Game").is_some()
        });
        let world = &mut host.app.world;
        let wave = world.find_by_name("Wave").unwrap();
        world
            .component_mut::<Script>(wave)
            .unwrap()
            .properties
            .push(box_engine::box_script::ScriptProperty {
                name: "count".into(),
                value: ScriptValue::Integer(count),
            });
        playthrough::wait_for(&mut host, "the count", 60, |w| {
            let wave = w.find_by_name("Wave").unwrap();
            let props: Option<box_engine::box_script::mlua::Table> =
                playthrough::field(w, wave, "props");
            props.and_then(|p| p.get::<i64>("count").ok()) == Some(count)
        });
        // Straight to the courtyard: the wave starts around the hero, who
        // cannot be hurt here (the measurement is of the wave, not of the
        // hero's death).
        let world = &mut host.app.world;
        let p = playthrough::player(world);
        world.component_mut::<Transform>(p).unwrap().translation = Vec3::new(0.0, 2.92, -42.0);
        playthrough::wait_for(&mut host, "the wave", 60, |w| {
            w.find_by_name("Cinderling").is_some()
        });
        let mut swarm = Phase::default();
        let mut fight = Phase::default();
        for frame in 0..720 {
            let attacking = frame >= 420;
            if attacking && frame % 30 == 0 {
                playthrough::press(&mut host, GamepadButton::West);
            } else {
                playthrough::release(&mut host, GamepadButton::West);
            }
            // Keep the hero alive through the measurement.
            let world = &mut host.app.world;
            let game = playthrough::game(world);
            if let Some(t) = world.resource::<ScriptRuntime>().instance(game) {
                t.set("health", 100.0).unwrap();
            }
            let t0 = Instant::now();
            host.app.run_frames(1, DT);
            let u = t0.elapsed();
            let t1 = Instant::now();
            renderer.render(&mut host.app.world).unwrap();
            let r = t1.elapsed();
            host.app.world.resource_mut::<Input>().end_frame();
            let phase = match frame {
                120..420 => &mut swarm,
                420.. => &mut fight,
                _ => continue,
            };
            let world = &host.app.world;
            let s = world.resource::<ScriptRuntime>().stats();
            let stats = renderer.stats();
            phase.update.push(u.as_secs_f64() * 1000.0);
            phase
                .scripts
                .push((s.update_time + s.fixed_time).as_secs_f64() * 1000.0);
            phase.render.push(r.as_secs_f64() * 1000.0);
            phase.draws.push(f64::from(stats.draws));
            phase.particles.push(f64::from(stats.particles));
            phase
                .enemies
                .push(count_named(world, &["Cinderling", "Spitter"]) as f64);
            // Embers in flight: the pool's unused ones are hidden.
            let embers = world
                .query::<(&Name, Option<&Visible>)>()
                .iter()
                .filter(|(n, v)| n.as_str() == "Ember" && v.is_none_or(|v| v.0))
                .count();
            phase.embers.push(embers as f64);
            if frame == 400 {
                let img = renderer.capture(&mut host.app.world).unwrap();
                img.save(captures().join(format!("wave_{count}_1080p.png")))
                    .unwrap();
            }
        }
        assert_eq!(renderer.stats().validation_errors, 0);
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        swarm.print(&format!(
            "{count} cinderlings at {w}x{h} ({build}): swarming, embers flying"
        ));
        fight.print(&format!(
            "{count} cinderlings at {w}x{h} ({build}): sweeping every 0.5 s"
        ));
        assert!(
            swarm.enemies.iter().all(|&n| n == count as f64),
            "the whole wave was alive while swarming"
        );
        let (errors, disabled) = playthrough::script_errors(&host.app.world);
        assert_eq!((errors, disabled), (0, 0));
    }
}
