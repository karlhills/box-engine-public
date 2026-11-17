//! box-engine physics: [`Collider`] components and spatial queries on
//! `parry3d` (D12), kinematic characters, and, with the `dynamics` feature,
//! rigid bodies on `rapier3d`.
//!
//! - [`Collider`]: a saved component (shape, offset, trigger flag, layers,
//!   friction and restitution). [`ColliderShape::Mesh`] collides with the
//!   triangles of the entity's `MeshAsset` (imported level geometry; the
//!   glTF importer's `colliders` setting adds it to every mesh node, except
//!   nodes named `*-nocol`);
//!   [`ColliderShape::ConvexHull`] is the solid hull of those vertices, for
//!   props on dynamic bodies.
//! - [`Physics`]: resource with [`raycast`](Physics::raycast),
//!   [`raycast_all`](Physics::raycast_all), [`shapecast`](Physics::shapecast),
//!   [`overlap`](Physics::overlap) and
//!   [`contains_point`](Physics::contains_point), all taking a
//!   [`QueryFilter`].
//! - [`sync_physics`]: the change-driven `PostUpdate` system that mirrors
//!   colliders into `Physics` and sends [`TriggerEvent`]s.
//! - [`PhysicsPlugin`]: registers the types and adds all of the above.
//! - [`Physics::move_and_slide`], [`KinematicCharacter`] and
//!   [`CharacterPlugin`]: kinematic character movement (collide and slide,
//!   steps, slopes, gravity, jumps, ground snapping, moving platforms, and
//!   with `dynamics` pushing bodies).
//! - [`MeshRayCache`]: ray casts against render meshes, for editor picking.
//! - `dynamics` feature: [`RigidBody`], [`Velocity`], [`Joint`],
//!   [`PhysicsSettings`], [`CollisionEvent`], forces and impulses on
//!   [`Physics`] (see the `dynamics` module).
//!
//! parry's math types come from `glamx`, which re-exports the workspace's
//! `glam` (0.33), so `Vec3` and `Quat` cross the boundary without conversion.
//! parry and rapier types never appear in this crate's public API.
//!
//! ```
//! use box_core::prelude::*;
//! use box_physics::{Collider, Physics, PhysicsPlugin, QueryFilter};
//!
//! let mut app = App::new();
//! app.add_plugin(&PhysicsPlugin);
//! app.world.spawn((Transform::from_xyz(0.0, 0.0, -5.0), Collider::cuboid(Vec3::ONE)));
//! app.update(); // transforms propagate, then colliders sync
//! let ray = Ray::new(Vec3::ZERO, Vec3::NEG_Z);
//! let hit = app.world.resource::<Physics>().raycast(&ray, 100.0, &QueryFilter::new());
//! assert!((hit.unwrap().toi - 4.5).abs() < 1e-4);
//! ```

mod character;
mod collider;
#[cfg(feature = "dynamics")]
pub mod dynamics;
mod mesh;
mod physics;
mod query;

pub use character::{
    CharacterPlugin, KinematicCharacter, SlideOptions, SlideResult, move_characters,
};
pub use collider::{Collider, ColliderShape};
#[cfg(feature = "dynamics")]
pub use dynamics::{
    AxisLocks, BodyKind, CollisionEvent, CollisionKind, ContactPoint, Dynamics, DynamicsStats,
    Joint, JointKind, PhysicsSettings, RigidBody, Velocity, step_dynamics,
};
pub use mesh::{MeshHit, MeshRayCache};
pub use physics::{Physics, SyncStats, TriggerEvent, TriggerKind, sync_physics};
pub use query::{Pose, QueryFilter, QueryShape, RayHit, ShapeHit};

use box_core::app::{App, Plugin, Stage};
use box_core::reflect::Registry;

/// Register the physics scene types (`Collider`, and through it
/// `ColliderShape`, and `KinematicCharacter`; with `dynamics`, also
/// `RigidBody`, `Velocity`, `Joint` and the `PhysicsSettings` settings
/// resource). Registering twice is harmless.
pub fn register_physics_types(registry: &mut Registry) {
    register_query_types(registry);
    #[cfg(feature = "dynamics")]
    for result in [
        registry.register::<RigidBody>(),
        registry.register::<Velocity>(),
        registry.register::<Joint>(),
        registry.register_settings::<PhysicsSettings>(),
    ] {
        if let Err(e) = result {
            tracing::error!("physics type registration failed: {e}");
        }
    }
}

/// The physics types without the `dynamics` feature's (`Collider`,
/// `KinematicCharacter`): what a build without rigid bodies registers.
pub fn register_query_types(registry: &mut Registry) {
    for result in [
        registry.register::<Collider>(),
        registry.register::<KinematicCharacter>(),
    ] {
        if let Err(e) = result {
            tracing::error!("physics type registration failed: {e}");
        }
    }
}

/// Physics as a plugin: registers the physics types, inserts the
/// [`Physics`] resource and `Events<TriggerEvent>`, and adds
/// [`sync_physics`] to `PostUpdate` (after transform propagation).
///
/// Triggers are computed in `PostUpdate`, once per frame, from the same
/// transforms the frame renders with; game systems read the events in the
/// next frame's `FixedUpdate` or `Update`.
///
/// With the `dynamics` feature it also adds `Events<CollisionEvent>`,
/// [`step_dynamics`] as the first `FixedUpdate` system (label
/// `physics_step`) and an `on_add` hook giving new dynamic and kinematic
/// bodies `Interpolated`. The simulation starts only when a `RigidBody`
/// exists.
pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn register(&self, registry: &mut Registry) {
        register_physics_types(registry);
    }

    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<Physics>();
        app.add_event::<TriggerEvent>();
        app.add_system(Stage::PostUpdate, sync_physics);
        #[cfg(feature = "dynamics")]
        {
            app.add_event::<CollisionEvent>();
            app.system(Stage::FixedUpdate, step_dynamics)
                .label("physics_step");
            app.on_add::<RigidBody>(|world, e| {
                let moves = world
                    .get::<RigidBody>(e)
                    .is_some_and(|b| b.kind != BodyKind::Fixed);
                if moves && !world.has::<box_core::interpolate::Interpolated>(e) {
                    let _ = world.insert_one(e, box_core::interpolate::Interpolated::default());
                }
            });
        }
    }
}

pub mod prelude {
    #[cfg(feature = "dynamics")]
    pub use crate::{
        AxisLocks, BodyKind, CollisionEvent, CollisionKind, Joint, JointKind, PhysicsSettings,
        RigidBody, Velocity,
    };
    pub use crate::{
        CharacterPlugin, Collider, ColliderShape, KinematicCharacter, Physics, PhysicsPlugin, Pose,
        QueryFilter, QueryShape, RayHit, ShapeHit, SlideOptions, SlideResult, TriggerEvent,
        TriggerKind,
    };
}
