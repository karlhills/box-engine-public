//! [`Timer`]: count down a duration, once or repeating.

/// Counts game time up to a duration: cooldowns, spawn waves, delays.
/// Tick it with [`Time::delta`](crate::time::Time::delta) (scaled and
/// paused with the game; inside `FixedUpdate` it is the fixed step).
///
/// ```
/// use box_core::timer::Timer;
///
/// let mut spawn = Timer::repeating(0.5);
/// let mut fired = 0;
/// for _ in 0..60 {
///     if spawn.tick(1.0 / 60.0) {
///         fired += 1;
///     }
/// }
/// assert_eq!(fired, 2);
///
/// let mut delay = Timer::once(1.0);
/// delay.tick(0.25);
/// assert_eq!(delay.fraction(), 0.25);
/// assert!(!delay.finished());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timer {
    /// Seconds to count. Zero or less finishes on the first tick.
    pub duration: f32,
    /// Start over after finishing (keeping the overshoot) instead of
    /// stopping at the end.
    pub repeating: bool,
    /// Seconds counted, in `0..=duration`.
    pub elapsed: f32,
    /// Times it finished during the last [`tick`](Self::tick) (more than 1
    /// for a short repeating timer and a long tick).
    finished_count: u32,
    /// A one-shot timer reached the end.
    done: bool,
}

impl Timer {
    pub fn new(duration: f32, repeating: bool) -> Self {
        Self {
            duration,
            repeating,
            elapsed: 0.0,
            finished_count: 0,
            done: false,
        }
    }

    /// A timer that finishes once after `duration` seconds.
    pub fn once(duration: f32) -> Self {
        Self::new(duration, false)
    }

    /// A timer that finishes every `duration` seconds.
    pub fn repeating(duration: f32) -> Self {
        Self::new(duration, true)
    }

    /// Count `dt` seconds. True if the timer finished during this tick (for a
    /// repeating timer: at least once; see [`times_finished`](Self::times_finished)).
    /// A finished one-shot timer stays finished and returns false.
    pub fn tick(&mut self, dt: f32) -> bool {
        self.finished_count = 0;
        if self.done {
            return false;
        }
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if self.duration <= 0.0 {
            self.finished_count = 1;
            self.done = !self.repeating;
            self.elapsed = 0.0;
            return true;
        }
        self.elapsed += dt;
        if self.elapsed < self.duration {
            return false;
        }
        if self.repeating {
            let laps = (self.elapsed / self.duration).floor();
            self.finished_count = laps as u32;
            self.elapsed -= laps * self.duration;
        } else {
            self.finished_count = 1;
            self.elapsed = self.duration;
            self.done = true;
        }
        true
    }

    /// A one-shot timer reached its duration (a repeating one: it finished in
    /// the last tick).
    pub fn finished(&self) -> bool {
        self.done || self.finished_count > 0
    }

    /// Times the timer finished during the last tick.
    pub fn times_finished(&self) -> u32 {
        self.finished_count
    }

    /// Progress in `0..=1` (1 for a zero duration).
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.duration).clamp(0.0, 1.0)
        }
    }

    /// Seconds left until the end.
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }

    /// Start counting from zero again.
    pub fn reset(&mut self) {
        self.elapsed = 0.0;
        self.finished_count = 0;
        self.done = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shot_finishes_once_and_stays_finished() {
        let mut t = Timer::once(1.0);
        assert!(!t.tick(0.6));
        assert!((t.fraction() - 0.6).abs() < 1e-6);
        assert!((t.remaining() - 0.4).abs() < 1e-6);
        assert!(t.tick(0.6));
        assert!(t.finished());
        assert_eq!(t.fraction(), 1.0);
        assert!(!t.tick(1.0), "fires once");
        assert!(t.finished());
        t.reset();
        assert!(!t.finished() && t.elapsed == 0.0);
    }

    #[test]
    fn repeating_keeps_the_overshoot_and_counts_laps() {
        let mut t = Timer::repeating(0.5);
        assert!(!t.tick(0.4));
        assert!(t.tick(0.2));
        assert!((t.elapsed - 0.1).abs() < 1e-6);
        assert!(t.finished());
        assert!(!t.tick(0.1));
        assert!(!t.finished(), "only during the tick it fired");
        assert!(t.tick(1.35));
        assert_eq!(t.times_finished(), 3);
        assert!((t.elapsed - 0.05).abs() < 1e-5);
    }

    #[test]
    fn degenerate_inputs() {
        let mut t = Timer::once(0.0);
        assert!(t.tick(0.0));
        assert_eq!(t.fraction(), 1.0);
        let mut t = Timer::repeating(1.0);
        assert!(!t.tick(f32::NAN));
        assert!(!t.tick(-5.0));
        assert_eq!(t.elapsed, 0.0);
    }
}
