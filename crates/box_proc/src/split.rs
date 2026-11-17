//! Split and repeat: divide a length into bays and place things along a
//! path or up a height. Plain functions, no grammar language
//! (docs/PROCEDURAL.md section 4).
//!
//! ```
//! use box_proc::split::{divide_size, place_bays, stack, divide_count, Anchor, Remainder};
//! use box_proc::spline::{Orientation, Path};
//! use glam::Vec3;
//!
//! // Fence posts every ~2 m along a polyline, at bay ends; three rails up each post.
//! let path = Path::from_polyline(&[Vec3::ZERO, Vec3::new(9.0, 0.0, 0.0)], false, Orientation::Up(Vec3::Y));
//! let bays = divide_size(path.length, 2.0, Remainder::Stretch);
//! assert_eq!(bays.len(), 5); // 9 m / 2 m rounds to 5 bays of 1.8 m
//! let posts = place_bays(&path, &bays, Anchor::End);
//! let rails = stack(&posts[0], &divide_count(1.2, 3), Anchor::Centre);
//! assert_eq!(rails.len(), 3);
//! ```

use crate::spline::{Frame, Path};

/// Most bays any function here returns.
pub const MAX_BAYS: usize = 100_000;

/// One division of a length: `[start, end]` in metres from the origin of
/// the divided length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bay {
    pub index: usize,
    pub start: f32,
    pub end: f32,
}

impl Bay {
    pub fn len(&self) -> f32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    pub fn centre(&self) -> f32 {
        (self.start + self.end) * 0.5
    }

    /// The position the anchor names.
    pub fn at(&self, anchor: Anchor) -> f32 {
        match anchor {
            Anchor::Start => self.start,
            Anchor::Centre => self.centre(),
            Anchor::End => self.end,
        }
    }
}

/// What to do with the length left over when bays have a fixed size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Remainder {
    /// Round the bay count and stretch or shrink bays to fill exactly.
    #[default]
    Stretch,
    /// Whole bays, the leftover split equally at both ends.
    Centre,
    /// Whole bays from the start; the leftover at the end.
    Start,
    /// Whole bays ending at the end; the leftover at the start.
    End,
}

/// Which point of a bay a placement uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    Start,
    #[default]
    Centre,
    End,
}

fn usable(x: f32) -> bool {
    x.is_finite() && x > 0.0
}

fn bays(offset: f32, size: f32, n: usize) -> Vec<Bay> {
    (0..n)
        .map(|i| Bay {
            index: i,
            start: offset + size * i as f32,
            end: offset + size * (i + 1) as f32,
        })
        .collect()
}

/// `count` equal bays over `length` (clamped to [`MAX_BAYS`]). Empty for a
/// non-positive or non-finite length or a zero count.
pub fn divide_count(length: f32, count: u32) -> Vec<Bay> {
    if !usable(length) || count == 0 {
        return Vec::new();
    }
    let n = (count as usize).min(MAX_BAYS);
    let mut out = bays(0.0, length / n as f32, n);
    if let Some(last) = out.last_mut() {
        last.end = length; // exact end despite rounding
    }
    out
}

/// Bays of `size` metres over `length`, with the leftover handled by
/// `remainder`. With [`Remainder::Stretch`] there is always at least one
/// bay; with the others, none when not even one bay fits.
pub fn divide_size(length: f32, size: f32, remainder: Remainder) -> Vec<Bay> {
    if !usable(length) || !usable(size) {
        return Vec::new();
    }
    let ratio = length / size;
    if remainder == Remainder::Stretch {
        let n = (ratio.round() as usize).clamp(1, MAX_BAYS);
        return divide_count(length, n as u32);
    }
    let n = ((ratio + 1e-5).floor() as usize).min(MAX_BAYS);
    if n == 0 {
        return Vec::new();
    }
    let left = (length - size * n as f32).max(0.0);
    let offset = match remainder {
        Remainder::Start | Remainder::Stretch => 0.0,
        Remainder::Centre => left * 0.5,
        Remainder::End => left,
    };
    bays(offset, size, n)
}

/// Frames at arc lengths along a path (see [`Path::frame_at`]; clamped,
/// or wrapped on closed paths).
pub fn place_along(path: &Path, distances: &[f32]) -> Vec<Frame> {
    distances.iter().map(|d| path.frame_at(*d)).collect()
}

/// One frame per bay, at the anchor point of each bay along the path.
pub fn place_bays(path: &Path, bays: &[Bay], anchor: Anchor) -> Vec<Frame> {
    bays.iter().map(|b| path.frame_at(b.at(anchor))).collect()
}

/// Repeats a frame up its own `up` axis: one frame per level, raised to the
/// level's anchor point (storeys, rails, courses).
pub fn stack(frame: &Frame, levels: &[Bay], anchor: Anchor) -> Vec<Frame> {
    levels
        .iter()
        .map(|l| {
            let mut f = *frame;
            f.position += f.up * l.at(anchor);
            f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spline::Orientation;
    use glam::Vec3;

    #[test]
    fn remainder_policies() {
        let s = divide_size(10.0, 3.0, Remainder::Stretch);
        assert_eq!(s.len(), 3);
        assert!((s[0].len() - 10.0 / 3.0).abs() < 1e-6 && s[2].end == 10.0);
        let c = divide_size(10.0, 3.0, Remainder::Centre);
        assert_eq!((c.len(), c[0].start, c[2].end), (3, 0.5, 9.5));
        let st = divide_size(10.0, 3.0, Remainder::Start);
        assert_eq!((st[0].start, st[2].end), (0.0, 9.0));
        let e = divide_size(10.0, 3.0, Remainder::End);
        assert_eq!((e[0].start, e[2].end), (1.0, 10.0));
        // Exact fits keep every bay despite rounding.
        assert_eq!(divide_size(0.9, 0.3, Remainder::Start).len(), 3);
        assert!(divide_size(1.0, 3.0, Remainder::Centre).is_empty());
        assert_eq!(divide_size(1.0, 3.0, Remainder::Stretch).len(), 1);
    }

    #[test]
    fn bad_input_gives_nothing() {
        assert!(divide_count(f32::NAN, 3).is_empty());
        assert!(divide_count(5.0, 0).is_empty());
        assert!(divide_size(5.0, 0.0, Remainder::Stretch).is_empty());
        assert!(divide_size(-1.0, 1.0, Remainder::Start).is_empty());
        assert_eq!(divide_size(1e9, 1e-3, Remainder::Stretch).len(), MAX_BAYS);
    }

    #[test]
    fn placement_along_an_l_path() {
        let path = Path::from_polyline(
            &[
                Vec3::ZERO,
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, -4.0),
            ],
            false,
            Orientation::Up(Vec3::Y),
        );
        let bays = divide_count(path.length, 4);
        let f = place_bays(&path, &bays, Anchor::Centre);
        assert!(f[0].position.distance(Vec3::new(1.0, 0.0, 0.0)) < 1e-5);
        assert!(f[3].position.distance(Vec3::new(4.0, 0.0, -3.0)) < 1e-5);
        assert!((f[3].forward - Vec3::NEG_Z).length() < 1e-5);
        let up = stack(&f[0], &divide_count(3.0, 3), Anchor::Start);
        assert!(up[2].position.distance(Vec3::new(1.0, 2.0, 0.0)) < 1e-5);
    }
}
