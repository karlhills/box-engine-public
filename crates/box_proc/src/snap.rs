//! Snapping with hysteresis for whatever moves a path point by hand (a
//! drawing tool, a handle drag, a scripted drag): a point connects within
//! [`SNAP`] (10 cm) of a target and lets go only beyond [`SNAP_RELEASE`]
//! (15 cm), so a hand hovering around the threshold does not make a
//! junction flicker (docs/PROCEDURAL_DIRECTION.md §6).
//!
//! **Where the state lives.** Rules are pure functions of the saved scene,
//! so they cannot remember that an end was connected a frame ago. The
//! memory is per gesture: a [`Snapper`] lives as long as one drag or one
//! drawing and the tool writes the *snapped* position (exactly on the
//! target) into the intent through `box_core::edit`. What is saved is
//! therefore already coincident (or more than 10 cm away), and a rule's
//! fixed 10 cm test gives the same junctions after undo, redo, save and
//! reload. Nothing about the gesture is saved or kept in the world.
//!
//! Targets are points (other paths' ends: they win) and segments (other
//! paths' bodies, with the body's half width as extra reach: a point
//! snapped to a body lands on its centreline).
//!
//! ```
//! use box_proc::snap::{SnapSegment, SnapTargets, Snapper};
//! use glam::Vec3;
//!
//! let end = Vec3::new(4.0, 0.0, 0.0);
//! let body = SnapSegment::new(Vec3::new(0.0, 0.0, 2.0), Vec3::new(4.0, 0.0, 2.0), 0.15);
//! let targets = SnapTargets { points: vec![end], segments: vec![body] };
//! let mut s = Snapper::new();
//! assert_eq!(s.snap_to(Vec3::new(4.08, 0.0, 0.0), &targets).0, end); // within 10 cm
//! assert_eq!(s.snap_to(Vec3::new(4.14, 0.0, 0.0), &targets).0, end); // held to 15 cm
//! // 20 cm from the body's centreline: within half width + 10 cm.
//! let (p, _) = s.snap_to(Vec3::new(2.0, 0.0, 1.8), &targets);
//! assert_eq!(p, Vec3::new(2.0, 0.0, 2.0));
//! ```

use glam::Vec3;

/// Points closer than this to a target connect (metres).
pub const SNAP: f32 = 0.10;
/// Distance beyond which a held snap lets go (metres).
pub const SNAP_RELEASE: f32 = 0.15;

/// A body a point may snap onto: the segment `a`–`b` (a path's centreline)
/// with `reach` (its half width) added to the snap distances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapSegment {
    pub a: Vec3,
    pub b: Vec3,
    pub reach: f32,
}

impl SnapSegment {
    pub fn new(a: Vec3, b: Vec3, reach: f32) -> Self {
        Self {
            a,
            b,
            reach: reach.max(0.0),
        }
    }

    /// The closest point of the segment to `p`.
    pub fn closest(&self, p: Vec3) -> Vec3 {
        let ab = self.b - self.a;
        let len2 = ab.length_squared();
        if len2 <= f32::EPSILON {
            return self.a;
        }
        self.a + ab * ((p - self.a).dot(ab) / len2).clamp(0.0, 1.0)
    }
}

/// What a point may snap to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapTargets {
    /// Path ends; preferred over bodies.
    pub points: Vec<Vec3>,
    /// Path bodies.
    pub segments: Vec<SnapSegment>,
}

impl SnapTargets {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.segments.is_empty()
    }
}

/// What a snapped point is held to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapHit {
    /// A path end.
    Point(Vec3),
    /// A body, at `point` on its centreline.
    Segment { segment: SnapSegment, point: Vec3 },
}

impl SnapHit {
    /// Where the snapped point is.
    pub fn point(&self) -> Vec3 {
        match self {
            Self::Point(p) | Self::Segment { point: p, .. } => *p,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Held {
    Point(Vec3),
    Segment(SnapSegment),
}

/// The snapping state of one moved point for one gesture (see the module
/// docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snapper {
    held: Option<Held>,
}

impl Snapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// The point target the point is snapped to, if any.
    pub fn held(&self) -> Option<Vec3> {
        match self.held {
            Some(Held::Point(p)) => Some(p),
            _ => None,
        }
    }

    /// Forgets the held target (a new gesture).
    pub fn reset(&mut self) {
        self.held = None;
    }

    /// [`Snapper::snap_to`] with point targets only; returns where to write
    /// the point.
    pub fn snap(&mut self, raw: Vec3, targets: &[Vec3]) -> Vec3 {
        let targets = SnapTargets {
            points: targets.to_vec(),
            segments: Vec::new(),
        };
        self.snap_to(raw, &targets).0
    }

    /// Where to write a point the pointer puts at `raw`, and what it is
    /// snapped to. A held target is kept while `raw` stays within
    /// [`SNAP_RELEASE`] of it (plus a body's reach) and it is still offered;
    /// otherwise the nearest end within [`SNAP`] is taken, else the nearest
    /// body within its reach plus [`SNAP`]. An end within [`SNAP`] wins
    /// over a held body.
    pub fn snap_to(&mut self, raw: Vec3, targets: &SnapTargets) -> (Vec3, Option<SnapHit>) {
        if !raw.is_finite() {
            self.held = None;
            return (raw, None);
        }
        if let Some(Held::Point(t)) = self.held {
            if raw.distance(t) <= SNAP_RELEASE && targets.points.contains(&t) {
                return (t, Some(SnapHit::Point(t)));
            }
            self.held = None;
        }
        let nearest = targets
            .points
            .iter()
            .map(|t| (*t, t.distance(raw)))
            .filter(|(_, d)| *d <= SNAP)
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((t, _)) = nearest {
            self.held = Some(Held::Point(t));
            return (t, Some(SnapHit::Point(t)));
        }
        if let Some(Held::Segment(s)) = self.held {
            let c = s.closest(raw);
            if raw.distance(c) <= s.reach + SNAP_RELEASE && targets.segments.contains(&s) {
                return (
                    c,
                    Some(SnapHit::Segment {
                        segment: s,
                        point: c,
                    }),
                );
            }
            self.held = None;
        }
        let nearest = targets
            .segments
            .iter()
            .map(|s| {
                let c = s.closest(raw);
                (*s, c, raw.distance(c) - s.reach)
            })
            .filter(|(_, _, d)| *d <= SNAP)
            .min_by(|a, b| a.2.total_cmp(&b.2));
        match nearest {
            Some((s, c, _)) => {
                self.held = Some(Held::Segment(s));
                (
                    c,
                    Some(SnapHit::Segment {
                        segment: s,
                        point: c,
                    }),
                )
            }
            None => (raw, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connects_at_10_cm_and_lets_go_beyond_15_cm() {
        let target = Vec3::new(4.0, 0.0, 0.0);
        let mut s = Snapper::new();
        let at = |d: f32| target + Vec3::new(d, 0.0, 0.0);
        assert_eq!(s.snap(at(0.12), &[target]), at(0.12));
        assert_eq!(s.snap(at(0.09), &[target]), target);
        assert_eq!(s.snap(at(0.14), &[target]), target, "held");
        assert_eq!(s.snap(at(0.16), &[target]), at(0.16), "released");
        assert_eq!(s.held(), None);
    }

    #[test]
    fn bodies_hold_with_their_reach_and_ends_win() {
        let body = SnapSegment::new(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), 0.15);
        let end = Vec3::new(10.0, 0.0, 0.0);
        let targets = SnapTargets {
            points: vec![end],
            segments: vec![body],
        };
        let mut s = Snapper::new();
        let at = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        // 0.15 + 0.12 away: free.
        assert_eq!(s.snap_to(at(5.0, 0.27), &targets), (at(5.0, 0.27), None));
        // 0.15 + 0.09: on the centreline.
        let (p, hit) = s.snap_to(at(5.0, 0.24), &targets);
        assert_eq!(p, at(5.0, 0.0));
        assert!(matches!(hit, Some(SnapHit::Segment { .. })));
        // Held to 0.15 + 0.15, sliding along the body.
        assert_eq!(s.snap_to(at(6.0, 0.29), &targets).0, at(6.0, 0.0));
        assert_eq!(s.snap_to(at(6.0, 0.31), &targets).0, at(6.0, 0.31));
        // An end within 10 cm wins over a held body.
        assert_eq!(s.snap_to(at(9.5, 0.2), &targets).0, at(9.5, 0.0));
        assert_eq!(
            s.snap_to(at(9.95, 0.05), &targets),
            (end, Some(SnapHit::Point(end)))
        );
        // A target no longer offered is let go.
        let none = SnapTargets::default();
        assert_eq!(s.snap_to(at(9.95, 0.05), &none).0, at(9.95, 0.05));
    }
}
