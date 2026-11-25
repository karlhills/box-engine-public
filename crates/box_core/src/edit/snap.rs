//! Snap to Ground: an entity's origin moved onto the ground under it, as one
//! undoable `Transform` edit. The ground comes from the registry's
//! [`GroundSampler`](super::GroundSampler) (installed by box_proc, which owns
//! terrains).

use glam::{Affine3A, Mat3, Quat, Vec3};

use super::{EditError, EditStack, editable};
use crate::identity::Uid;
use crate::transform::{Transform, compute_global};
use crate::world::World;

/// Where [`EditStack::snap_to_ground`] put an entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Snapped {
    /// Moved (or already there): the origin's new world position.
    Moved(Vec3),
    /// There is no ground under the entity (outside every terrain); nothing
    /// changed. Holds the world x, z that was sampled.
    NoGround { x: f32, z: f32 },
}

/// `rotation` turned so its up axis is `normal`, keeping its heading: the
/// forward axis (−Z) projected onto the ground plane (+X where it looks
/// straight along the normal).
fn aligned(rotation: Quat, normal: Vec3) -> Quat {
    let up = normal.normalize_or(Vec3::Y);
    let flat = |v: Vec3| (v - up * v.dot(up)).try_normalize();
    let back = flat(rotation * Vec3::Z)
        .or_else(|| flat(rotation * Vec3::X).map(|x| x.cross(up)))
        .unwrap_or(Vec3::Z);
    let x = up.cross(back);
    Quat::from_mat3(&Mat3::from_cols(x, up, back)).normalize()
}

impl EditStack {
    /// Move the entity's origin onto the ground under it, `offset` metres
    /// above it (world up). With `align` its up axis also turns to the
    /// ground's normal, keeping its heading. One `Transform` edit; the
    /// entity's parent may be anything (its local transform is solved).
    ///
    /// The origin is what lands: a model whose origin is at its base stands
    /// on the ground; a shape centred on its origin needs `offset` of half
    /// its height. Errors when no crate installed a
    /// [`GroundSampler`](super::GroundSampler) or the entity has no
    /// `Transform`.
    pub fn snap_to_ground(
        &mut self,
        world: &mut World,
        uid: Uid,
        offset: f32,
        align: bool,
    ) -> Result<Snapped, EditError> {
        let e = editable(world, uid)?;
        let Some(local) = world.get::<Transform>(e).map(|t| *t) else {
            return Err(EditError::Invalid(
                "has no Transform, so it has no position to snap".into(),
            ));
        };
        if !offset.is_finite() {
            return Err(EditError::Invalid(format!("offset {offset} is not finite")));
        }
        let sampler = world.registry().ground_sampler().ok_or_else(|| {
            EditError::Invalid(
                "nothing provides ground heights in this build (box_proc's terrains do)".into(),
            )
        })?;
        let global = compute_global(world, e);
        let (scale, rotation, position) = global.to_scale_rotation_translation();
        let Some((height, normal)) = sampler(world, position.x, position.z) else {
            return Ok(Snapped::NoGround {
                x: position.x,
                z: position.z,
            });
        };
        let target = Vec3::new(position.x, height + offset, position.z);
        let parent = world
            .parent(e)
            .map_or(Affine3A::IDENTITY, |p| compute_global(world, p));
        let det = parent.matrix3.determinant();
        if det == 0.0 || !det.is_finite() {
            return Err(EditError::Invalid(
                "its parent's transform cannot be inverted (a zero scale)".into(),
            ));
        }
        let to_local = parent.inverse();
        let new = if align {
            let world_new =
                Affine3A::from_scale_rotation_translation(scale, aligned(rotation, normal), target);
            let (scale, rotation, translation) =
                (to_local * world_new).to_scale_rotation_translation();
            Transform {
                translation,
                rotation: rotation.normalize(),
                scale,
            }
        } else {
            // Only the position changes: keep the local rotation and scale
            // exactly, without a round trip through a matrix.
            Transform {
                translation: to_local.transform_point3(target),
                ..local
            }
        };
        let path = world.field_path("Transform")?;
        self.set_field(world, uid, &path, &new)?;
        Ok(Snapped::Moved(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligning_keeps_the_heading_and_matches_the_normal() {
        let yaw = Quat::from_rotation_y(0.7);
        let n = Vec3::new(0.3, 1.0, -0.2).normalize();
        let r = aligned(yaw, n);
        assert!((r * Vec3::Y - n).length() < 1e-5);
        let fwd = |q: Quat| {
            let f = q * Vec3::NEG_Z;
            Vec3::new(f.x, 0.0, f.z).normalize()
        };
        assert!(fwd(r).dot(fwd(yaw)) > 0.98, "{:?}", fwd(r));
        // Flat ground gives the yaw back; aligning twice changes nothing.
        assert!(aligned(yaw, Vec3::Y).angle_between(yaw) < 1e-5);
        assert!(aligned(r, n).angle_between(r) < 1e-5);
        // Looking straight down still gives an upright result.
        let down = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        assert!((aligned(down, Vec3::Y) * Vec3::Y - Vec3::Y).length() < 1e-5);
    }
}
