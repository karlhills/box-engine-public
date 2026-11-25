//! View-relative movement helpers shared by the player and the camera.

use glam::{Vec2, Vec3};

/// The horizontal direction for a `move` value (x right, y forward) seen
/// from a view with this yaw, at most length 1.
pub fn wish_dir(input: Vec2, yaw: f32) -> Vec3 {
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    let dir = right * input.x + forward * input.y;
    if dir.length() > 1.0 {
        dir.normalize()
    } else {
        dir
    }
}

/// The yaw of a direction (0 looks down −Z, positive turns left).
pub fn yaw_of(dir: Vec3) -> f32 {
    if dir.x.abs() + dir.z.abs() < 1e-4 {
        return 0.0;
    }
    (-dir.x).atan2(-dir.z)
}

/// `from` turned toward `to` by at most `step`, the short way round.
pub fn turn_towards(from: f32, to: f32, step: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let diff = (to - from + PI).rem_euclid(TAU) - PI;
    if diff.abs() <= step {
        to
    } else {
        from + diff.signum() * step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_relative_movement() {
        // Yaw 0 looks down -Z: forward input moves along -Z, right along +X.
        assert!((wish_dir(Vec2::Y, 0.0) - Vec3::NEG_Z).length() < 1e-6);
        assert!((wish_dir(Vec2::X, 0.0) - Vec3::X).length() < 1e-6);
        // Turned left a quarter: forward is -X.
        let d = wish_dir(Vec2::Y, std::f32::consts::FRAC_PI_2);
        assert!((d - Vec3::NEG_X).length() < 1e-5, "{d}");
        // Diagonals are not faster.
        assert!((wish_dir(Vec2::ONE, 0.3).length() - 1.0).abs() < 1e-5);
        assert!((yaw_of(Vec3::NEG_X) - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn turns_the_short_way() {
        let a = turn_towards(3.0, -3.0, 0.1);
        assert!(a > 3.0, "{a}");
        assert_eq!(turn_towards(0.0, 0.05, 0.1), 0.05);
    }
}
