//! The event loop: a winit window, or a headless capture run.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use box_core::prelude::*;
use box_render::Renderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::gamepad::GamepadPoller;
use crate::input::CursorController;
use crate::live::LiveControl;
use crate::splash::Splash;

/// Size of the render target in physical pixels. Updated by the runner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
    pub scale_factor_milli: u32,
}

/// How to run an app. Command-line flags override these.
///
/// Flags:
/// - `--capture <file.png>`: render headlessly (no window), save the last frame, exit.
///   With `{frame}` in the path (`out_{frame}.png`), every frame is saved.
/// - `--frames <n>`: exit after `n` frames (default 3 with `--capture`).
/// - `--size <W>x<H>`: window size in logical pixels, or capture size in pixels.
/// - `--report <file.json>`: write frame timing and render statistics as JSON on exit
///   (headless runs include the GPU frame time and pass intervals, from timestamp queries).
/// - `--headless`: no window and no capture; run `--frames` frames (default 3)
///   offscreen, for timings and smoke tests.
/// - `--control`: open the live control socket (`box mcp` live tools) for the
///   app's project, the [`Scenes`](crate::Scenes) resource's project root (the
///   player sets it); an error without one. Requests wait until the first
///   frame was rendered with the scene's assets loaded (windowed: at most
///   600 frames), so a first screenshot shows the scene. Headless runs are then paced to
///   60 frames per second, and `--headless --control` without `--frames` runs
///   until the control `stop` method (or [`AppExit`]) ends it.
/// - `--splash`: show the startup splash ([`crate::splash`]) before the
///   game, also in headless and capture runs (whose timeline is
///   deterministic: `--splash --frames 60 --capture out.png` captures it
///   fully shown). The player turns it on by itself for exported games in a
///   window.
#[derive(Clone, Debug)]
pub struct RunOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub capture: Option<PathBuf>,
    pub frames: Option<u64>,
    pub report: Option<PathBuf>,
    pub headless: bool,
    /// Serve the live control socket (`--control`).
    pub control: bool,
    /// Show the startup splash before the game (`--splash`).
    pub splash: bool,
}

/// What a run did, returned by [`run_with`].
#[derive(Clone, Debug, Default)]
pub struct RunSummary {
    /// Frames run (presented frames for a windowed run).
    pub frames: u64,
    /// wgpu validation errors over the whole run.
    pub validation_errors: u32,
    /// Images written by `--capture`.
    pub captures: Vec<PathBuf>,
    /// The `--report` JSON (timings and render statistics). Always present
    /// for headless runs; for windowed runs only with `--report`.
    pub report: Option<serde_json::Value>,
}

impl RunOptions {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            width: 1280,
            height: 720,
            capture: None,
            frames: None,
            report: None,
            headless: false,
            control: false,
            splash: false,
        }
    }

    /// Apply flags from `std::env::args()`.
    pub fn with_args(self) -> Result<Self> {
        self.with_arg_list(std::env::args().skip(1))
    }

    pub fn with_arg_list(mut self, args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--capture" => self.capture = Some(value()?.into()),
                "--frames" => {
                    self.frames = Some(value()?.parse().context("--frames expects a number")?)
                }
                "--report" => self.report = Some(value()?.into()),
                "--headless" => self.headless = true,
                "--control" => self.control = true,
                "--splash" => self.splash = true,
                "--size" => {
                    let v = value()?;
                    let (w, h) = v.split_once('x').context("--size expects WxH")?;
                    self.width = w.parse().context("bad width")?;
                    self.height = h.parse().context("bad height")?;
                }
                // Unknown flags are left for the example to interpret.
                _ => {}
            }
        }
        Ok(self)
    }
}

/// Run the app with these options (after applying command-line flags).
pub fn run(app: App, options: RunOptions) -> Result<()> {
    run_with(app, options.with_args()?).map(drop)
}

/// Run the app with exactly these options (command-line flags are not
/// read): headless with `capture` or `headless`, otherwise in a window.
/// A headless run fails if wgpu reported validation errors.
pub fn run_with(mut app: App, options: RunOptions) -> Result<RunSummary> {
    crate::init_logging();
    crate::insert_engine_resources(&mut app.world);
    if options.control {
        crate::log_capture::enable();
    }
    let result = if options.capture.is_some() || options.headless {
        run_headless(app, &options)
    } else {
        run_windowed(app, options)
    };
    if let Err(e) = &result {
        crate::crash::report_device_lost(e);
    }
    result
}

const FIXED_DT: f32 = 1.0 / 60.0;

/// The control socket when `--control` was given.
fn start_control(app: &App, options: &RunOptions) -> Result<Option<LiveControl>> {
    options
        .control
        .then(|| LiveControl::start(&app.world))
        .transpose()
}

/// Sleep until `next`, then move it one fixed step on (headless runs with a
/// control socket keep real time instead of running flat out).
fn pace(next: &mut Instant) {
    *next += Duration::from_secs_f32(FIXED_DT);
    let now = Instant::now();
    if *next > now {
        std::thread::sleep(*next - now);
    } else {
        *next = now;
    }
}

/// How long a headless run waits for assets before its first frame.
const ASSET_SETTLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn run_headless(mut app: App, options: &RunOptions) -> Result<RunSummary> {
    let pattern = options
        .capture
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    // `{frame}` in the path saves every frame as a numbered sequence.
    let sequence = pattern.as_ref().is_some_and(|p| p.contains("{frame}"));
    let mut control = start_control(&app, options)?;
    // A live headless run without `--frames` lasts until it is stopped.
    let default_frames = if control.is_some() && pattern.is_none() {
        u64::MAX
    } else {
        3
    };
    let frames = options.frames.unwrap_or(default_frames).max(1);
    // Captures never show a half-loaded scene: wait for the world's assets
    // (prefab contents, meshes, textures) before the first frame. With a
    // splash they load while it shows.
    let mut splash = options.splash.then(Splash::new);
    if splash.is_none() && app.world.contains_resource::<box_assets::AssetServer>() {
        crate::asset_glue::settle_assets(&mut app.world, ASSET_SETTLE_TIMEOUT);
    }
    let mut renderer = Renderer::new_headless(options.width, options.height)
        .context("could not create a headless renderer")?;
    crate::crash::set_gpu(renderer.adapter_info());
    // Headless frames wait for the GPU anyway, so per-pass GPU timings
    // cost nothing extra; they go into the report.
    renderer.set_gpu_timing(true);
    set_window_size(&mut app.world, options.width, options.height, 1.0);
    let mut report = Report::new(&renderer);
    let start = Instant::now();
    let mut saved = Vec::new();
    let mut next = Instant::now();
    let mut frame = 0;
    while frame < frames && !app.exit_requested() {
        // Requests are answered from the second frame on, so the first
        // answer (a screenshot) sees a ticked, rendered world.
        if frame > 0
            && let Some(c) = &mut control
        {
            c.poll(&mut app, Some(&mut renderer));
            if app.exit_requested() {
                break;
            }
            // Paused: nothing changes and nobody sees the frame, so neither
            // tick nor render (screenshots render on demand).
            if c.paused() {
                pace(&mut next);
                continue;
            }
        }
        let t0 = Instant::now();
        // The splash shows instead of the game until it has finished.
        let (w, h) = renderer.size();
        let showing = splash
            .as_mut()
            .is_some_and(|s| s.update(&mut app.world, FIXED_DT, w, h, 1.0));
        if !showing {
            splash = None;
            app.tick(FIXED_DT);
        }
        let update_ms = ms_since(t0);
        let t1 = Instant::now();
        let save_this = pattern.is_some() && (sequence || frame + 1 == frames);
        let world = match &mut splash {
            Some(s) => s.world_mut(),
            None => &mut app.world,
        };
        let image = if save_this {
            Some(renderer.capture(world)?)
        } else {
            renderer.render(world)?;
            None
        };
        let render_ms = ms_since(t1);
        report.record(FIXED_DT * 1000.0, update_ms, render_ms, &renderer);
        app.world.resource_mut::<Input>().end_frame();
        if let Some(c) = &mut control {
            c.frame_done(true, FIXED_DT * 1000.0, update_ms, render_ms, &renderer);
            pace(&mut next);
        }
        if let (Some(image), Some(pattern)) = (image, &pattern) {
            let out = PathBuf::from(pattern.replace("{frame}", &format!("{frame:04}")));
            if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            image
                .save(&out)
                .with_context(|| format!("could not write {}", out.display()))?;
            saved.push(out);
        }
        frame += 1;
    }
    let frames = frame;
    drop(control);
    tracing::info!(
        "headless run: {}x{}, {} frames, {:.0} ms{}",
        options.width,
        options.height,
        frames,
        ms_since(start),
        match saved.last() {
            Some(last) => format!(
                ", {} image(s) captured, last {}",
                saved.len(),
                last.display()
            ),
            None => String::new(),
        }
    );
    if let Some(r) = &options.report {
        report.write(r, &options.title)?;
    }
    let errors = renderer.stats().validation_errors;
    if errors > 0 {
        bail!("{errors} GPU validation error(s) during the headless run");
    }
    Ok(RunSummary {
        frames,
        validation_errors: errors,
        captures: saved,
        report: Some(report.to_json(&options.title)),
    })
}

fn run_windowed(app: App, options: RunOptions) -> Result<RunSummary> {
    let control = start_control(&app, &options)?;
    let splash = options.splash.then(Splash::new);
    // A window is someone listening: sounds go to the device (opened on the
    // first sound). Headless runs stay silent.
    #[cfg(feature = "audio")]
    box_audio::enable_device_output();
    let event_loop = EventLoop::new().context("could not create the event loop")?;
    let mut runner = Runner {
        app,
        options,
        window: None,
        renderer: None,
        last_frame: None,
        frames: 0,
        title_timer: 0.0,
        title_frames: 0,
        report: None,
        error: None,
        occluded: false,
        gamepads: GamepadPoller::new(),
        cursor: CursorController::new(),
        control,
        splash,
    };
    event_loop.run_app(&mut runner)?;
    // Close the socket and remove the discovery file before writing reports.
    runner.control = None;
    if let Some(e) = runner.error {
        return Err(e);
    }
    if let (Some(report), Some(path)) = (&runner.report, &runner.options.report) {
        report.write(path, &runner.options.title)?;
    }
    Ok(RunSummary {
        frames: runner.frames,
        validation_errors: runner
            .renderer
            .as_ref()
            .map_or(0, |r| r.stats().validation_errors),
        captures: Vec::new(),
        report: runner
            .report
            .as_ref()
            .map(|r| r.to_json(&runner.options.title)),
    })
}

fn set_window_size(world: &mut World, width: u32, height: u32, scale: f64) {
    *world.resource_or_default::<WindowSize>() = WindowSize {
        width,
        height,
        scale_factor_milli: (scale * 1000.0).round() as u32,
    };
}

fn ms_since(t: Instant) -> f32 {
    t.elapsed().as_secs_f32() * 1000.0
}

struct Runner {
    app: App,
    options: RunOptions,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    last_frame: Option<Instant>,
    frames: u64,
    title_timer: f32,
    title_frames: u32,
    report: Option<Report>,
    error: Option<anyhow::Error>,
    /// The OS reports the window as fully hidden; redraws pause until it is shown.
    occluded: bool,
    gamepads: GamepadPoller,
    cursor: CursorController,
    /// The live control socket (`--control`).
    control: Option<LiveControl>,
    /// The startup splash, while it shows.
    splash: Option<Splash>,
}

impl Runner {
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        tracing::error!("{error:#}");
        self.error = Some(error);
        event_loop.exit();
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let now = Instant::now();
        let mut dt = self
            .last_frame
            .map(|t| (now - t).as_secs_f32())
            .unwrap_or(FIXED_DT);
        self.last_frame = Some(now);

        self.gamepads.update(self.app.world.resource_mut::<Input>());
        if let Some(splash) = &mut self.splash {
            match splash.frame(renderer, &mut self.app.world, dt) {
                Ok(true) => {
                    // Splash frames count towards `--frames`, as in headless
                    // runs. As below: back off while the surface is
                    // unavailable.
                    let presented = renderer.stats().presented;
                    if presented {
                        self.frames += 1;
                    }
                    if self.options.frames.is_some_and(|n| self.frames >= n) {
                        event_loop.exit();
                    } else if presented {
                        window.request_redraw();
                    } else if !self.occluded {
                        std::thread::sleep(std::time::Duration::from_millis(8));
                        window.request_redraw();
                    }
                    return;
                }
                // Finished: the game starts this frame, with a normal step
                // (a slow last splash frame must not become a huge one).
                Ok(false) => {
                    self.splash = None;
                    dt = FIXED_DT;
                }
                Err(e) => return self.fail(event_loop, e.into()),
            }
        }
        // Requests are answered once a frame was presented and the assets
        // loaded (or after 600 frames), so the first answer (a screenshot)
        // never shows an empty, half-loaded world.
        let ready = self.frames >= 600
            || (self.frames > 0
                && self
                    .app
                    .world
                    .get_resource::<box_assets::AssetServer>()
                    .is_none_or(box_assets::AssetServer::is_idle));
        let paused = match &mut self.control {
            Some(c) if ready => {
                c.poll(&mut self.app, Some(renderer));
                c.paused()
            }
            _ => false,
        };
        let t0 = Instant::now();
        // Paused by the control socket: keep rendering, stop ticking.
        if !paused {
            self.app.tick(dt);
        }
        let update_ms = ms_since(t0);
        self.cursor.update(window, &mut self.app.world);
        let t1 = Instant::now();
        let result = renderer.render(&mut self.app.world);
        let render_ms = ms_since(t1);
        self.app.world.resource_mut::<Input>().end_frame();
        let presented = renderer.stats().presented;
        if let Some(c) = &mut self.control {
            c.frame_done(!paused, dt * 1000.0, update_ms, render_ms, renderer);
        }
        if presented && let Some(report) = &mut self.report {
            report.record(dt * 1000.0, update_ms, render_ms, renderer);
        }

        self.title_timer += dt;
        self.title_frames += 1;
        if self.title_timer >= 0.5 {
            let ms = self.title_timer * 1000.0 / self.title_frames as f32;
            window.set_title(&format!(
                "{} — {:.2} ms ({:.0} fps)",
                self.options.title,
                ms,
                1000.0 / ms
            ));
            self.title_timer = 0.0;
            self.title_frames = 0;
        }

        if presented {
            self.frames += 1;
        }
        let done = self.options.frames.is_some_and(|n| self.frames >= n);
        if let Err(e) = result {
            self.fail(event_loop, e.into());
        } else if done || self.app.exit_requested() {
            event_loop.exit();
        } else if presented {
            window.request_redraw();
        } else if !self.occluded {
            // The surface was not available (for example the window is still
            // being shown). Back off briefly instead of spinning.
            std::thread::sleep(std::time::Duration::from_millis(8));
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.options.title)
            .with_inner_size(LogicalSize::new(self.options.width, self.options.height));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("could not create window"),
                );
            }
        };
        let size = window.inner_size();
        match Renderer::new_windowed(window.clone(), size.width, size.height) {
            Ok(r) => {
                crate::crash::set_gpu(r.adapter_info());
                tracing::info!(
                    "adapter: {} ({:?})",
                    r.adapter_info().name,
                    r.adapter_info().backend
                );
                if self.options.report.is_some() {
                    self.report = Some(Report::new(&r));
                }
                self.renderer = Some(r);
            }
            Err(e) => {
                return self.fail(
                    event_loop,
                    anyhow::Error::new(e).context("could not create renderer"),
                );
            }
        }
        set_window_size(
            &mut self.app.world,
            size.width,
            size.height,
            window.scale_factor(),
        );
        window.request_redraw();
        self.window = Some(window);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let scale = self
            .window
            .as_ref()
            .map(|w| w.scale_factor())
            .unwrap_or(1.0);
        let world = &mut self.app.world;
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
                set_window_size(world, size.width, size.height, scale);
            }
            WindowEvent::Focused(focused) => {
                crate::input::apply_window_event(world.resource_mut::<Input>(), &event, scale);
                self.gamepads.set_focused(focused);
                if let Some(w) = &self.window {
                    self.cursor.set_focused(w, focused);
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                // Avoid a huge time step when the window comes back.
                self.last_frame = None;
                if !occluded && let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            other => {
                crate::input::apply_window_event(world.resource_mut::<Input>(), &other, scale);
            }
        }
    }

    /// While the window is occluded no redraws arrive, so the control socket
    /// is polled from here on a 60 Hz timer instead (the game does not tick).
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(control), Some(renderer), true) =
            (&mut self.control, &mut self.renderer, self.occluded)
        else {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        };
        control.poll(&mut self.app, Some(renderer));
        if self.app.exit_requested() {
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::wait_duration(Duration::from_secs_f32(
                FIXED_DT,
            )));
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.app
                .world
                .resource_mut::<Input>()
                .add_mouse_motion(Vec2::new(delta.0 as f32, delta.1 as f32));
        }
    }
}

/// Frame timing and render statistics, written as JSON with `--report`.
struct Report {
    adapter: String,
    backend: String,
    frame_ms: Vec<f32>,
    update_ms: Vec<f32>,
    render_ms: Vec<f32>,
    /// CPU time until the frame was submitted (extraction, uploads,
    /// encoding, submission).
    submit_ms: Vec<f32>,
    /// GPU time of all passes (empty without timestamp queries).
    gpu_ms: Vec<f32>,
    /// GPU milliseconds per frame by pass label (passes with the same label
    /// in one frame, the shadow cascades, add up), in first-seen order.
    gpu_passes: Vec<(&'static str, Vec<f32>)>,
    upload_bytes: u64,
    reallocations: u64,
    max_draws: u32,
    max_shadow_draws: u32,
    max_shadow_casters: u32,
    max_triangles: u64,
}

impl Report {
    fn new(r: &Renderer) -> Self {
        Self {
            adapter: r.adapter_info().name.clone(),
            backend: format!("{:?}", r.adapter_info().backend),
            frame_ms: Vec::new(),
            update_ms: Vec::new(),
            render_ms: Vec::new(),
            submit_ms: Vec::new(),
            gpu_ms: Vec::new(),
            gpu_passes: Vec::new(),
            upload_bytes: 0,
            reallocations: 0,
            max_draws: 0,
            max_shadow_draws: 0,
            max_shadow_casters: 0,
            max_triangles: 0,
        }
    }

    fn record(&mut self, frame_ms: f32, update_ms: f32, render_ms: f32, r: &Renderer) {
        self.frame_ms.push(frame_ms);
        self.update_ms.push(update_ms);
        self.render_ms.push(render_ms);
        let s = r.stats();
        self.submit_ms.push(s.cpu_submit_ms);
        if !s.gpu_passes.is_empty() {
            let frame = self.gpu_ms.len();
            self.gpu_ms.push(s.gpu_ms);
            for &(label, ms) in &s.gpu_passes {
                let i = match self.gpu_passes.iter().position(|p| p.0 == label) {
                    Some(i) => i,
                    None => {
                        self.gpu_passes.push((label, Vec::new()));
                        self.gpu_passes.len() - 1
                    }
                };
                let samples = &mut self.gpu_passes[i].1;
                // A pass that did not run in a frame counts as 0 there.
                samples.resize(frame + 1, 0.0);
                samples[frame] += ms;
            }
        }
        self.upload_bytes += s.upload_bytes;
        self.reallocations += s.buffer_reallocations as u64;
        self.max_draws = self.max_draws.max(s.draws);
        self.max_shadow_draws = self.max_shadow_draws.max(s.shadow_draws);
        self.max_shadow_casters = self.max_shadow_casters.max(s.shadow_casters);
        self.max_triangles = self.max_triangles.max(s.triangles);
    }

    fn to_json(&self, title: &str) -> serde_json::Value {
        // The text is built by hand (stable key order, rounded numbers) and
        // always parses; `Null` would only mean a formatting bug.
        serde_json::from_str(&self.to_text(title)).unwrap_or(serde_json::Value::Null)
    }

    fn write(&self, path: &std::path::Path, title: &str) -> Result<()> {
        let json = self.to_text(title);
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, json)
            .with_context(|| format!("could not write {}", path.display()))?;
        tracing::info!("report written to {}", path.display());
        Ok(())
    }

    fn to_text(&self, title: &str) -> String {
        let frames = self.frame_ms.len().max(1) as f64;
        format!(
            "{{\n  \"title\": {title:?},\n  \"adapter\": {adapter:?},\n  \"backend\": {backend:?},\n  \"frames\": {n},\n  \"frame_ms\": {f},\n  \"update_ms\": {u},\n  \"render_cpu_ms\": {r},\n  \"render_submit_ms\": {pr},\n  \"gpu_ms\": {g},\n  \"gpu_pass_ms\": {gp},\n  \"upload_bytes_per_frame\": {up:.0},\n  \"buffer_reallocations\": {re},\n  \"max_draws\": {d},\n  \"max_shadow_draws\": {sd},\n  \"max_shadow_casters\": {sc},\n  \"max_triangles\": {t},\n  \"slowest_render_frames\": {slow}\n}}\n",
            adapter = self.adapter,
            backend = self.backend,
            n = self.frame_ms.len(),
            f = summary(&self.frame_ms),
            u = summary(&self.update_ms),
            r = summary(&self.render_ms),
            pr = summary(&self.submit_ms),
            g = summary(&self.gpu_ms),
            gp = self.gpu_pass_summaries(),
            up = self.upload_bytes as f64 / frames,
            re = self.reallocations,
            d = self.max_draws,
            sd = self.max_shadow_draws,
            sc = self.max_shadow_casters,
            t = self.max_triangles,
            slow = slowest(&self.render_ms, 5),
        )
    }
}

impl Report {
    /// `{"label": summary, ...}` per GPU pass label (see [`summary`]), `{}`
    /// without timings. A frame without the pass counts as 0 for it.
    fn gpu_pass_summaries(&self) -> String {
        let frames = self.gpu_ms.len();
        let items: Vec<String> = self
            .gpu_passes
            .iter()
            .map(|(label, samples)| {
                let mut v = samples.clone();
                v.resize(frames, 0.0);
                format!("{label:?}: {}", summary(&v))
            })
            .collect();
        format!("{{{}}}", items.join(", "))
    }
}

/// The `n` slowest samples as `[{"frame": i, "ms": x}, ...]`, slowest first.
fn slowest(samples: &[f32], n: usize) -> String {
    let mut idx: Vec<usize> = (0..samples.len()).collect();
    idx.sort_by(|&a, &b| samples[b].total_cmp(&samples[a]));
    let items: Vec<String> = idx
        .iter()
        .take(n)
        .map(|&i| format!("{{\"frame\": {i}, \"ms\": {:.3}}}", samples[i]))
        .collect();
    format!("[{}]", items.join(", "))
}

/// `{"min":..,"mean":..,"p50":..,"p95":..,"p99":..,"max":..}` of a sample, skipping the
/// first 10 % (warm-up).
fn summary(samples: &[f32]) -> String {
    let skip = samples.len() / 10;
    let mut v: Vec<f32> = samples[skip..].to_vec();
    if v.is_empty() {
        return "null".into();
    }
    v.sort_by(f32::total_cmp);
    let pct = |p: f32| v[((v.len() - 1) as f32 * p).round() as usize];
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    format!(
        "{{\"min\": {:.3}, \"mean\": {mean:.3}, \"p50\": {:.3}, \"p95\": {:.3}, \"p99\": {:.3}, \"max\": {:.3}}}",
        v[0],
        pct(0.5),
        pct(0.95),
        pct(0.99),
        v[v.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags() {
        let o = RunOptions::new("t")
            .with_arg_list(
                [
                    "--capture",
                    "out/a.png",
                    "--frames",
                    "5",
                    "--size",
                    "640x360",
                    "--report",
                    "r.json",
                    "--headless",
                    "--control",
                    "--splash",
                    "--other",
                ]
                .map(String::from),
            )
            .unwrap();
        assert_eq!(
            o.capture.as_deref(),
            Some(std::path::Path::new("out/a.png"))
        );
        assert_eq!((o.frames, o.width, o.height), (Some(5), 640, 360));
        assert!(o.report.is_some() && o.headless && o.control && o.splash);
        assert!(!RunOptions::new("t").control && !RunOptions::new("t").splash);
        assert!(
            RunOptions::new("t")
                .with_arg_list(["--frames".to_string()])
                .is_err()
        );
    }

    #[test]
    fn summary_percentiles() {
        let s: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let out = summary(&s);
        assert!(out.contains("\"max\": 100.000"), "{out}");
        assert!(
            out.contains("\"p50\": 55.000") || out.contains("\"p50\": 56.000"),
            "{out}"
        );
    }
}
