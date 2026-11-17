//! [`AnimationClip`]: keyframed translation, rotation and scale channels
//! that target entities by name, and their sampling.

use glam::{Quat, Vec3};

/// How a channel's value changes between two keyframes (glTF sampler
/// interpolation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// The earlier keyframe's value until the next keyframe.
    Step,
    /// Straight lines; rotations use spherical interpolation (`slerp`).
    #[default]
    Linear,
    /// Cubic Hermite splines with an in- and out-tangent per keyframe
    /// (glTF `CUBICSPLINE`); rotations are normalized after evaluation.
    CubicSpline,
}

/// The values of a channel, one per keyframe, or three per keyframe
/// (in-tangent, value, out-tangent) for [`Interpolation::CubicSpline`].
#[derive(Clone, Debug, PartialEq)]
pub enum Keyframes {
    Translation(Vec<Vec3>),
    Rotation(Vec<Quat>),
    Scale(Vec<Vec3>),
}

impl Keyframes {
    pub fn len(&self) -> usize {
        match self {
            Self::Translation(v) | Self::Scale(v) => v.len(),
            Self::Rotation(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One sampled channel value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sample {
    Translation(Vec3),
    Rotation(Quat),
    Scale(Vec3),
}

/// One animated property of one target: keyframe times in seconds
/// (non-decreasing) and their values.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// The animated entity's `Name`, looked up among the descendants of the
    /// entity with the [`AnimationPlayer`](super::AnimationPlayer) (glTF
    /// node names; an unnamed glTF node is `Node <index>`, as in the
    /// imported scene). Matching by name lets a clip drive any skeleton
    /// with the same joint names.
    pub target: String,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    pub values: Keyframes,
}

impl Channel {
    /// Checks that times are finite and non-decreasing and that there is
    /// one value per time (three for cubic splines).
    pub fn validate(&self) -> Result<(), String> {
        if self.times.is_empty() {
            return Err(format!("channel of `{}` has no keyframes", self.target));
        }
        if self.times.iter().any(|t| !t.is_finite()) {
            return Err(format!(
                "channel of `{}` has a non-finite time",
                self.target
            ));
        }
        if self.times.windows(2).any(|w| w[1] < w[0]) {
            return Err(format!(
                "channel of `{}` has decreasing keyframe times",
                self.target
            ));
        }
        let per_key = if self.interpolation == Interpolation::CubicSpline {
            3
        } else {
            1
        };
        if self.values.len() != self.times.len() * per_key {
            return Err(format!(
                "channel of `{}` has {} values for {} keyframes",
                self.target,
                self.values.len(),
                self.times.len()
            ));
        }
        Ok(())
    }

    /// The time of the last keyframe.
    pub fn end(&self) -> f32 {
        self.times.last().copied().unwrap_or(0.0)
    }

    /// The value at time `t` (seconds): the first value before the first
    /// keyframe, the last after the last.
    pub fn sample(&self, t: f32) -> Sample {
        let mut hint = 0;
        self.sample_with_hint(t, &mut hint)
    }

    /// [`sample`](Self::sample), starting the keyframe search at `*hint`
    /// (the keyframe found last time) and updating it, so playing forward
    /// costs O(1) per sample instead of a binary search.
    pub fn sample_with_hint(&self, t: f32, hint: &mut usize) -> Sample {
        let (k, f) = self.segment(t, hint);
        let dt = if k + 1 < self.times.len() {
            self.times[k + 1] - self.times[k]
        } else {
            0.0
        };
        match &self.values {
            Keyframes::Translation(v) => {
                Sample::Translation(interpolate_vec3(v, self.interpolation, k, f, dt))
            }
            Keyframes::Scale(v) => Sample::Scale(interpolate_vec3(v, self.interpolation, k, f, dt)),
            Keyframes::Rotation(v) => {
                Sample::Rotation(interpolate_quat(v, self.interpolation, k, f, dt))
            }
        }
    }

    /// The keyframe `k` at or before `t` and the fraction `f` towards
    /// keyframe `k + 1` (0 at or after the last keyframe).
    fn segment(&self, t: f32, hint: &mut usize) -> (usize, f32) {
        let times = &self.times;
        let n = times.len();
        if n <= 1 || t <= times[0] {
            *hint = 0;
            return (0, 0.0);
        }
        if t >= times[n - 1] {
            *hint = n - 1;
            return (n - 1, 0.0);
        }
        // times[k] <= t < times[k + 1]
        let mut k = (*hint).min(n - 2);
        if !(times[k] <= t && t < times[k + 1]) {
            if k + 2 < n && times[k + 1] <= t && t < times[k + 2] {
                k += 1;
            } else {
                k = times
                    .partition_point(|&x| x <= t)
                    .saturating_sub(1)
                    .min(n - 2);
            }
        }
        *hint = k;
        let span = times[k + 1] - times[k];
        let f = if span > 0.0 {
            ((t - times[k]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (k, f)
    }
}

/// Hermite basis: value `p0` with out-tangent `m0` to `p1` with in-tangent
/// `m1` (tangents already scaled by the keyframe interval).
fn hermite<T>(p0: T, m0: T, p1: T, m1: T, f: f32) -> T
where
    T: std::ops::Mul<f32, Output = T> + std::ops::Add<Output = T>,
{
    let f2 = f * f;
    let f3 = f2 * f;
    p0 * (2.0 * f3 - 3.0 * f2 + 1.0)
        + m0 * (f3 - 2.0 * f2 + f)
        + p1 * (-2.0 * f3 + 3.0 * f2)
        + m1 * (f3 - f2)
}

fn interpolate_vec3(v: &[Vec3], mode: Interpolation, k: usize, f: f32, dt: f32) -> Vec3 {
    let keys = if mode == Interpolation::CubicSpline {
        v.len() / 3
    } else {
        v.len()
    };
    let last = keys.saturating_sub(1);
    let k = k.min(last);
    match mode {
        Interpolation::Step => v[k],
        Interpolation::Linear => {
            if k == last {
                v[k]
            } else {
                v[k].lerp(v[k + 1], f)
            }
        }
        Interpolation::CubicSpline => {
            if k == last {
                v[3 * k + 1]
            } else {
                let (p0, m0) = (v[3 * k + 1], v[3 * k + 2] * dt);
                let (p1, m1) = (v[3 * (k + 1) + 1], v[3 * (k + 1)] * dt);
                hermite(p0, m0, p1, m1, f)
            }
        }
    }
}

fn interpolate_quat(v: &[Quat], mode: Interpolation, k: usize, f: f32, dt: f32) -> Quat {
    let keys = if mode == Interpolation::CubicSpline {
        v.len() / 3
    } else {
        v.len()
    };
    let last = keys.saturating_sub(1);
    let k = k.min(last);
    let q = match mode {
        Interpolation::Step => v[k],
        Interpolation::Linear => {
            if k == last {
                v[k]
            } else {
                v[k].slerp(v[k + 1], f)
            }
        }
        Interpolation::CubicSpline => {
            if k == last {
                v[3 * k + 1]
            } else {
                let p0 = glam::Vec4::from(v[3 * k + 1]);
                let m0 = glam::Vec4::from(v[3 * k + 2]) * dt;
                let p1 = glam::Vec4::from(v[3 * (k + 1) + 1]);
                let m1 = glam::Vec4::from(v[3 * (k + 1)]) * dt;
                Quat::from_vec4(hermite(p0, m0, p1, m1, f))
            }
        }
    };
    let len = q.length();
    if len > 1e-12 && len.is_finite() {
        q / len
    } else {
        Quat::IDENTITY
    }
}

/// A keyframed animation: channels that move named entities' translation,
/// rotation or scale. Imported from glTF animations (`file#Animation/<name>`)
/// or built in code with [`AnimationClip::new`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationClip {
    /// Seconds: the latest keyframe of any channel.
    pub duration: f32,
    pub channels: Vec<Channel>,
}

impl AnimationClip {
    /// A clip of `channels`, its duration their last keyframe. Invalid
    /// channels (see [`Channel::validate`]) are dropped with a warning.
    pub fn new(channels: Vec<Channel>) -> Self {
        let channels: Vec<Channel> = channels
            .into_iter()
            .filter(|c| match c.validate() {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(target: "box_core::animation", "{e}; dropped");
                    false
                }
            })
            .collect();
        let duration = channels.iter().map(Channel::end).fold(0.0, f32::max);
        Self { duration, channels }
    }

    /// The names of the entities the clip animates, once each, in channel
    /// order.
    pub fn targets(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for c in &self.channels {
            if !out.contains(&c.target.as_str()) {
                out.push(&c.target);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(interpolation: Interpolation, times: Vec<f32>, values: Keyframes) -> Channel {
        Channel {
            target: "Bone".into(),
            interpolation,
            times,
            values,
        }
    }

    fn translation(s: Sample) -> Vec3 {
        match s {
            Sample::Translation(v) => v,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn linear_step_and_clamping() {
        let c = channel(
            Interpolation::Linear,
            vec![1.0, 2.0, 4.0],
            Keyframes::Translation(vec![Vec3::ZERO, Vec3::X, Vec3::new(3.0, 0.0, 0.0)]),
        );
        assert_eq!(translation(c.sample(0.0)), Vec3::ZERO);
        assert_eq!(translation(c.sample(1.5)), Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(translation(c.sample(3.0)), Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(translation(c.sample(9.0)), Vec3::new(3.0, 0.0, 0.0));
        let s = Channel {
            interpolation: Interpolation::Step,
            ..c.clone()
        };
        assert_eq!(translation(s.sample(1.99)), Vec3::ZERO);
        assert_eq!(translation(s.sample(2.0)), Vec3::X);
        assert_eq!(translation(s.sample(3.9)), Vec3::X);
    }

    #[test]
    fn hints_match_binary_search_in_any_order() {
        let times: Vec<f32> = (0..20).map(|i| i as f32 * 0.1).collect();
        let values = times.iter().map(|&t| Vec3::splat(t * t)).collect();
        let c = channel(Interpolation::Linear, times, Keyframes::Translation(values));
        let mut hint = 0;
        for t in [0.05, 0.12, 0.13, 0.55, 1.95, 0.3, 0.31, -1.0, 5.0, 0.0, 1.9] {
            assert_eq!(c.sample_with_hint(t, &mut hint), c.sample(t), "t = {t}");
        }
    }

    #[test]
    fn rotations_slerp_and_cubic_splines_follow_tangents() {
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let c = channel(
            Interpolation::Linear,
            vec![0.0, 1.0],
            Keyframes::Rotation(vec![Quat::IDENTITY, q]),
        );
        let Sample::Rotation(mid) = c.sample(0.5) else {
            panic!()
        };
        assert!(mid.angle_between(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)) < 1e-5);

        // Zero tangents: a smoothstep between the values.
        let z = Vec3::ZERO;
        let cubic = channel(
            Interpolation::CubicSpline,
            vec![0.0, 2.0],
            Keyframes::Translation(vec![z, z, z, z, Vec3::Y, z]),
        );
        assert!((translation(cubic.sample(0.5)).y - 0.15625).abs() < 1e-6);
        assert_eq!(translation(cubic.sample(1.0)).y, 0.5);
        assert_eq!(translation(cubic.sample(2.0)), Vec3::Y);
        // A constant velocity of 1/s expressed through tangents is linear.
        let v = Vec3::new(0.5, 0.0, 0.0);
        let line = channel(
            Interpolation::CubicSpline,
            vec![0.0, 2.0],
            Keyframes::Translation(vec![v, z, v, v, Vec3::X, v]),
        );
        assert!((translation(line.sample(0.5)) - Vec3::new(0.25, 0.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn invalid_channels_are_dropped_and_duration_is_the_last_key() {
        let good = channel(
            Interpolation::Linear,
            vec![0.0, 1.5],
            Keyframes::Scale(vec![Vec3::ONE, Vec3::ONE]),
        );
        let bad = channel(
            Interpolation::CubicSpline,
            vec![0.0, 1.0],
            Keyframes::Scale(vec![Vec3::ONE; 2]),
        );
        let decreasing = channel(
            Interpolation::Linear,
            vec![1.0, 0.0],
            Keyframes::Scale(vec![Vec3::ONE; 2]),
        );
        let clip = AnimationClip::new(vec![good, bad, decreasing]);
        assert_eq!(clip.channels.len(), 1);
        assert_eq!(clip.duration, 1.5);
        assert_eq!(clip.targets(), ["Bone"]);
    }
}
