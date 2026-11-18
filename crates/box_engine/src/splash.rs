//! The startup splash: a black screen with "made with box engine" that
//! fades in, holds and fades out before an exported game starts, while the
//! game's assets load.
//!
//! The splash renders its own small world (only an
//! [`Overlay`](box_render::Overlay), painted by [`box_ui::SplashPainter`])
//! with the game's [`Renderer`] instead of the game world. The game app is
//! not ticked meanwhile: no gameplay, no script `on_start`, no sound. Each
//! splash frame runs one [`settle_step`](crate::asset_glue::settle_step) on
//! the game world so its assets load in the background. Timeline: fade in
//! 0.5 s, hold until at least 2 s have passed and the assets are loaded
//! (at most until 9.5 s, so a stuck load never traps the player), fade out
//! 0.5 s. It cannot be skipped.
//!
//! **When it shows** ([`should_show`]): in windowed runs of exported games
//! unless the project's `export` settings say `"splash": false`, or when a
//! player is started with `--splash` (also headless, for captures). Never in
//! the editor, in other headless or capture runs, or in tests.
//!
//! **Custom runners** (a game with its own event loop) call
//! [`Splash::frame`] each frame before ticking the game, and start ticking
//! once it returns `false`:
//!
//! ```no_run
//! # fn f(renderer: &mut box_render::Renderer, app: &mut box_core::app::App, dt: f32)
//! # -> anyhow::Result<()> {
//! # let mut splash = Some(box_engine::splash::Splash::new());
//! // Once: `let mut splash = Some(Splash::new());` (or `None` to skip it).
//! // Each frame:
//! if let Some(s) = &mut splash {
//!     if s.frame(renderer, &mut app.world, dt)? {
//!         return Ok(()); // the splash drew this frame; the game waits
//!     }
//!     splash = None; // finished: the game starts this frame
//! }
//! app.tick(dt);
//! renderer.render(&mut app.world)?;
//! # Ok(()) }
//! ```
//!
//! A runner that draws into its own target (a `ViewportTarget` under an
//! egui UI) calls [`Splash::update`] instead and renders
//! [`Splash::world_mut`] wherever it likes.

use box_core::input::Input;
use box_core::world::World;
use box_render::{Overlay, RenderError, Renderer};
use box_ui::SplashPainter;

use crate::runner::WindowSize;

/// Seconds the text takes to fade in.
pub const FADE_IN: f32 = 0.5;
/// Seconds the text takes to fade out.
pub const FADE_OUT: f32 = 0.5;
/// The shortest splash, in seconds (fades included).
pub const MIN_DURATION: f32 = 2.5;
/// The longest splash, in seconds: the hold ends here even if assets are
/// still loading.
pub const MAX_DURATION: f32 = 10.0;

/// Slack for times summed from frame steps (120 steps of 1/60 s fall just
/// short of 2 s in `f32`).
const EPSILON: f32 = 1e-4;

/// Whether a player run shows the splash: `forced` (`--splash`) always;
/// otherwise only a windowed run (`headless` covers `--capture` too) of an
/// exported game (`packed`) whose `export.splash` setting is not `false`.
pub fn should_show(packed: bool, setting: Option<bool>, forced: bool, headless: bool) -> bool {
    forced || (packed && !headless && setting != Some(false))
}

/// The splash's clock: how far it is, and when the fade-out started.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SplashTimeline {
    /// Seconds since the splash started.
    pub time: f32,
    /// When the fade-out started (`None` while holding).
    pub fade_out_from: Option<f32>,
}

impl SplashTimeline {
    /// Moves the clock on by `dt` seconds. `assets_idle`: the game's assets
    /// are loaded, so the hold may end (after [`MIN_DURATION`] minus the
    /// fade-out; at [`MAX_DURATION`] minus the fade-out it ends anyway).
    pub fn advance(&mut self, dt: f32, assets_idle: bool) {
        self.time += dt.max(0.0);
        if self.fade_out_from.is_none() {
            let earliest = MIN_DURATION - FADE_OUT;
            let latest = MAX_DURATION - FADE_OUT;
            let reached = |at: f32| self.time + EPSILON >= at;
            if (assets_idle && reached(earliest)) || reached(latest) {
                self.fade_out_from = Some(self.time);
            }
        }
    }

    /// Opacity of the text, 0 to 1 (smoothstep fades).
    pub fn alpha(&self) -> f32 {
        if self.finished() {
            return 0.0;
        }
        let fade_in = (self.time / FADE_IN).clamp(0.0, 1.0);
        let fade_out = match self.fade_out_from {
            Some(from) => 1.0 - ((self.time - from) / FADE_OUT).clamp(0.0, 1.0),
            None => 1.0,
        };
        let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
        smooth(fade_in.min(fade_out))
    }

    /// The fade-out is over.
    pub fn finished(&self) -> bool {
        self.fade_out_from
            .is_some_and(|from| self.time - from + EPSILON >= FADE_OUT)
    }
}

/// A running splash (see the module docs).
pub struct Splash {
    world: World,
    painter: SplashPainter,
    timeline: SplashTimeline,
    started: bool,
}

impl Default for Splash {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Splash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Splash")
            .field("timeline", &self.timeline)
            .finish_non_exhaustive()
    }
}

impl Splash {
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(Overlay::default());
        Self {
            world,
            painter: SplashPainter::new(),
            timeline: SplashTimeline::default(),
            started: false,
        }
    }

    /// One splash frame for a runner that renders with
    /// [`Renderer::render`]: [`update`](Self::update) at the renderer's
    /// size and the game world's [`WindowSize`] scale factor, then renders
    /// the splash world. Returns whether the splash is still showing; once
    /// it returns `false` it has drawn nothing this frame and the game
    /// should tick and render instead.
    pub fn frame(
        &mut self,
        renderer: &mut Renderer,
        game: &mut World,
        dt: f32,
    ) -> Result<bool, RenderError> {
        let (width, height) = renderer.size();
        let scale = scale_factor(game);
        if !self.update(game, dt, width, height, scale) {
            return Ok(false);
        }
        renderer.render(&mut self.world)?;
        Ok(true)
    }

    /// Advances the splash by `dt` seconds (the first call starts it at 0),
    /// loads the game's assets one step, clears the game's per-frame input
    /// (so presses during the splash do not reach the game's first frame)
    /// and paints the frame for a target of `width` x `height` physical
    /// pixels at `scale` pixels per point. Returns whether the splash is
    /// still showing; then render [`world_mut`](Self::world_mut).
    pub fn update(
        &mut self,
        game: &mut World,
        dt: f32,
        width: u32,
        height: u32,
        scale: f32,
    ) -> bool {
        let idle = crate::asset_glue::settle_step(game);
        if self.started {
            self.timeline.advance(dt, idle);
        } else {
            self.started = true;
        }
        if let Some(input) = game.get_resource_mut::<Input>() {
            input.end_frame();
        }
        if self.timeline.finished() {
            return false;
        }
        let alpha = self.timeline.alpha();
        let overlay = self.world.resource_or_default::<Overlay>();
        self.painter.paint(overlay, width, height, scale, alpha);
        true
    }

    /// The world the splash draws: render it while
    /// [`update`](Self::update) returns `true`.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    pub fn timeline(&self) -> &SplashTimeline {
        &self.timeline
    }
}

/// The game world's pixels per point (1 without a [`WindowSize`]).
fn scale_factor(world: &World) -> f32 {
    world
        .get_resource::<WindowSize>()
        .map(|s| s.scale_factor_milli as f32 / 1000.0)
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn run(timeline: &mut SplashTimeline, seconds: f32, idle: bool) {
        for _ in 0..(seconds / DT).round() as u32 {
            timeline.advance(DT, idle);
        }
    }

    #[test]
    fn fades_in_holds_and_fades_out() {
        let mut t = SplashTimeline::default();
        assert_eq!(t.alpha(), 0.0);
        run(&mut t, 0.25, true);
        assert!((t.alpha() - 0.5).abs() < 0.01, "{}", t.alpha());
        run(&mut t, 0.25, true);
        assert_eq!(t.alpha(), 1.0);
        run(&mut t, 1.4, true);
        assert_eq!(t.alpha(), 1.0, "holding at {}", t.time);
        assert!(t.fade_out_from.is_none());
        run(&mut t, 0.1, true);
        assert!(t.fade_out_from.is_some(), "the fade-out starts at 2 s");
        run(&mut t, 0.25, true);
        assert!((t.alpha() - 0.5).abs() < 0.05, "{}", t.alpha());
        assert!(!t.finished());
        run(&mut t, 0.25, true);
        assert!(t.finished());
        assert_eq!(t.alpha(), 0.0);
        assert!((t.time - MIN_DURATION).abs() < 0.01, "{}", t.time);
    }

    #[test]
    fn holds_while_assets_load_up_to_the_cap() {
        let mut t = SplashTimeline::default();
        run(&mut t, 5.0, false);
        assert_eq!(t.alpha(), 1.0);
        assert!(t.fade_out_from.is_none());
        // Loaded at 5 s: the fade-out starts then.
        t.advance(DT, true);
        assert!(t.fade_out_from.is_some());
        run(&mut t, FADE_OUT, true);
        assert!(t.finished());

        // Never loaded: the hold ends at the cap.
        let mut stuck = SplashTimeline::default();
        run(&mut stuck, MAX_DURATION - FADE_OUT - 0.05, false);
        assert!(stuck.fade_out_from.is_none());
        run(&mut stuck, 0.1, false);
        assert!(stuck.fade_out_from.is_some());
        run(&mut stuck, FADE_OUT, false);
        assert!(stuck.finished());
        assert!(stuck.time <= MAX_DURATION + 0.1, "{}", stuck.time);
    }

    #[test]
    fn shows_in_windowed_exported_games_unless_turned_off() {
        // Exported game in a window: on unless `"splash": false`.
        assert!(should_show(true, None, false, false));
        assert!(should_show(true, Some(true), false, false));
        assert!(!should_show(true, Some(false), false, false));
        // A project folder, or a headless/capture run: off.
        assert!(!should_show(false, None, false, false));
        assert!(!should_show(true, None, false, true));
        // `--splash` forces it anywhere.
        assert!(should_show(false, None, true, true));
        assert!(should_show(true, Some(false), true, false));
    }

    #[test]
    fn the_game_waits_for_the_splash() {
        let mut splash = Splash::new();
        let mut game = World::new();
        let mut frames = 0;
        while splash.update(&mut game, DT, 640, 360, 1.0) {
            frames += 1;
            assert!(frames < 1000, "the splash never ends");
            assert!(!splash.world_mut().resource::<Overlay>().is_empty());
        }
        // 2.5 s at 60 fps, plus the first frame at time 0.
        assert_eq!(frames, 150);
    }
}
