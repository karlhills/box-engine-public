//! Frame time, with time scale and pause.

/// Time since the app started. Advanced by the app once per frame
/// ([`App::tick`](crate::app::App::tick)).
///
/// **Scale and pause.** [`scale`](Self::scale) multiplies game time (0.5 is
/// slow motion) and [`paused`](Self::paused) stops it. Both apply to
/// [`delta`](Self::delta), [`elapsed`](Self::elapsed) and the fixed-step
/// accumulator, so `FixedUpdate` runs less often (or not at all) while the
/// step itself stays the same, and [`Timer`](crate::timer::Timer)s and
/// tweens ticked with `delta` slow down or stop with them. Menus and cameras
/// that must keep moving while the game is paused read
/// [`unscaled_delta`](Self::unscaled_delta).
#[derive(Clone, Copy, Debug)]
pub struct Time {
    /// Game seconds since the previous frame (scaled, zero while paused).
    /// Inside `FixedUpdate` it is the fixed step.
    pub delta: f32,
    /// Game seconds since start, accumulated in f64 to avoid drift. Prefer
    /// it over [`elapsed`](Self::elapsed) for anything that runs for hours.
    pub elapsed_f64: f64,
    /// `elapsed_f64` as f32, for shaders and short animations. Loses
    /// precision after a few hours (about 1 ms resolution after 4.5 hours).
    pub elapsed: f32,
    /// Frames advanced so far.
    pub frame: u64,
    /// Real seconds since the previous frame (clamped to
    /// [`MAX_DELTA`](Self::MAX_DELTA)), ignoring scale and pause.
    pub unscaled_delta: f32,
    /// Game time per real time: 1 is normal speed, 0.5 slow motion, 0
    /// frozen. Negative values count as 0.
    pub scale: f32,
    /// Stops game time: `delta` is 0 and no fixed step runs.
    pub paused: bool,
}

impl Default for Time {
    fn default() -> Self {
        Self {
            delta: 0.0,
            elapsed_f64: 0.0,
            elapsed: 0.0,
            frame: 0,
            unscaled_delta: 0.0,
            scale: 1.0,
            paused: false,
        }
    }
}

impl Time {
    /// Frame times are clamped to this to avoid huge steps after a stall.
    pub const MAX_DELTA: f32 = 0.25;

    /// Advance one frame of `dt` real seconds: clamp it, then apply
    /// [`scale`](Self::scale) and [`paused`](Self::paused).
    pub fn advance(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, Self::MAX_DELTA);
        self.unscaled_delta = dt;
        self.delta = if self.paused {
            0.0
        } else {
            dt * self.scale.max(0.0)
        };
        self.elapsed_f64 += self.delta as f64;
        self.elapsed = self.elapsed_f64 as f32;
        self.frame += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_frames_are_clamped() {
        let mut t = Time::default();
        t.advance(5.0);
        assert_eq!(t.delta, Time::MAX_DELTA);
        t.advance(-1.0);
        assert_eq!(t.delta, 0.0);
        assert_eq!(t.frame, 2);
    }

    #[test]
    fn scale_and_pause_apply_to_game_time_only() {
        let mut t = Time {
            scale: 0.5,
            ..Default::default()
        };
        t.advance(0.1);
        assert_eq!((t.delta, t.unscaled_delta), (0.05, 0.1));
        t.paused = true;
        t.advance(0.1);
        assert_eq!((t.delta, t.unscaled_delta), (0.0, 0.1));
        t.paused = false;
        t.scale = -3.0;
        t.advance(0.1);
        assert_eq!(t.delta, 0.0);
        assert!((t.elapsed_f64 - 0.05).abs() < 1e-9);
        assert_eq!(t.frame, 3);
    }
}
