//! Helpers that compute [`WallEnd`] parameters for walls meeting at a
//! point: the *mechanisms* of spike S6's junction rules. Choosing which
//! junction to use, and who owns a corner, is the game rule's job (the
//! taller wall, then the lower uid, is what `examples/walls` and S6 use).
//!
//! Every helper takes [`Arm`]s: an end of a wall at the junction point,
//! with the direction *along that wall away from the point*. All points
//! and directions are plan (x, z) in one shared space (the rule's); a wall
//! whose entity is rotated or moved needs its results mapped into its own
//! space before they are written into it.
//!
//! | Situation | Helper |
//! |---|---|
//! | Two ends, same thickness | [`mitre`] |
//! | Two ends, different thickness or height (L) | [`butt`]: the owner runs to the other's outer face and caps; the other is cut on the owner's inner face |
//! | An end against another wall's side (T) | [`tee`]: cut on the body's near face, no cap, concave on both sides |
//! | Mitre too sharp | [`can_mitre`] is false: use [`butt`] or cap both |

use box_core::pmath;
use glam::Vec2;

use crate::consts::{MITRE_LIMIT, PLINTH_PROUD};
use crate::wall::{WallEnd, perp};

/// One wall end at a junction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arm {
    /// The junction point (the wall's first or last centreline point).
    pub point: Vec2,
    /// Unit direction along the wall, away from the point.
    pub dir: Vec2,
    pub thickness: f32,
    /// True when this is the wall's last point (`Wall::end`), false for its
    /// first (`Wall::start`).
    pub at_end: bool,
}

impl Arm {
    /// The arm at the first point of a wall running `first → second`.
    pub fn start(first: Vec2, second: Vec2, thickness: f32) -> Self {
        Self {
            point: first,
            dir: (second - first).normalize_or_zero(),
            thickness,
            at_end: false,
        }
    }

    /// The arm at the last point of a wall running `before_last → last`.
    pub fn end(before_last: Vec2, last: Vec2, thickness: f32) -> Self {
        Self {
            point: last,
            dir: (before_last - last).normalize_or_zero(),
            thickness,
            at_end: true,
        }
    }

    /// The wall's outer normal (+w) at this end.
    fn outer(&self) -> Vec2 {
        let travel = if self.at_end { -self.dir } else { self.dir };
        perp(travel)
    }

    /// `[outer, inner]` concave flags for a neighbour lying along `toward`.
    fn facing(&self, toward: Vec2) -> [bool; 2] {
        let s = self.outer().dot(toward);
        [s > 1e-6, s < -1e-6]
    }
}

/// Whether two arms can mitre within the mitre limit.
pub fn can_mitre(a: &Arm, b: &Arm) -> bool {
    // The interior angle between the arms; the section stretches by
    // 1 / sin(angle / 2).
    let cos = a.dir.dot(b.dir).clamp(-1.0, 1.0);
    let half = pmath::sin(pmath::acos(cos) * 0.5);
    half > 1.0 / MITRE_LIMIT
}

/// A mitre between two walls of the same thickness: `(a's end, b's end)`.
pub fn mitre(a: &Arm, b: &Arm) -> (WallEnd, WallEnd) {
    (
        WallEnd::Mitre { neighbour: b.dir },
        WallEnd::Mitre { neighbour: a.dir },
    )
}

/// An owner/subordinate butt (spike S6, rule J2): the owner runs to the
/// subordinate's outer face and caps there, the subordinate is cut on the
/// owner's inner face without a cap. Plinths stop at each other's plinth
/// face. Returns `(owner's end, subordinate's end)`.
pub fn butt(owner: &Arm, sub: &Arm) -> (WallEnd, WallEnd) {
    let p = owner.point;
    // The subordinate's face away from the owner's body.
    let mut ns = perp(sub.dir);
    if ns.dot(owner.dir) > 0.0 {
        ns = -ns;
    }
    let owner_end = WallEnd::Butt {
        point: p + ns * (sub.thickness * 0.5),
        direction: sub.dir,
        cap: true,
        band_shift: -PLINTH_PROUD,
        concave: owner.facing(sub.dir),
    };
    // The owner's face toward the subordinate's body.
    let mut no = perp(owner.dir);
    if no.dot(sub.dir) < 0.0 {
        no = -no;
    }
    let sub_end = WallEnd::Butt {
        point: sub.point + no * (owner.thickness * 0.5),
        direction: owner.dir,
        cap: false,
        band_shift: PLINTH_PROUD,
        concave: sub.facing(owner.dir),
    };
    (owner_end, sub_end)
}

/// A T junction: `stem` ends against the side of a wall whose centreline
/// passes through `stem.point` with direction `body_dir` and thickness
/// `body_thickness`. The stem is cut on the body's near face, with no cap
/// and concave occlusion on both sides; the body runs through untouched.
pub fn tee(stem: &Arm, body_dir: Vec2, body_thickness: f32) -> WallEnd {
    let bd = body_dir.normalize_or_zero();
    let mut nb = perp(bd);
    if nb.dot(stem.dir) < 0.0 {
        nb = -nb;
    }
    WallEnd::Butt {
        point: stem.point + nb * (body_thickness * 0.5),
        direction: bd,
        cap: false,
        band_shift: PLINTH_PROUD,
        concave: [true, true],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_angle_can_mitre_and_a_sliver_cannot() {
        let a = Arm::end(Vec2::new(-3.0, 0.0), Vec2::ZERO, 0.3);
        let b = Arm::start(Vec2::ZERO, Vec2::new(0.0, 3.0), 0.3);
        assert!(can_mitre(&a, &b));
        let c = Arm::start(Vec2::ZERO, Vec2::new(-3.0, 0.4), 0.3);
        assert!(!can_mitre(&a, &c));
    }

    #[test]
    fn butt_flags_the_inside_of_an_l() {
        // Owner along -x ending at the origin, subordinate leaving along +z.
        let owner = Arm::end(Vec2::new(-3.0, 0.0), Vec2::ZERO, 0.4);
        let sub = Arm::start(Vec2::ZERO, Vec2::new(0.0, 3.0), 0.25);
        let (o, s) = butt(&owner, &sub);
        let WallEnd::Butt {
            concave: co,
            cap: true,
            ..
        } = o
        else {
            panic!("{o:?}")
        };
        let WallEnd::Butt {
            concave: cs,
            cap: false,
            ..
        } = s
        else {
            panic!("{s:?}")
        };
        // Owner travels +x; its outer side (right) is +z, toward the sub.
        assert_eq!(co, [true, false]);
        // Sub travels +z; its outer side (right) is -x, toward the owner.
        assert_eq!(cs, [true, false]);
    }
}
