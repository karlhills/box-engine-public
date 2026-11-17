//! Rigid-body dynamics on `rapier3d` (D12, M7), behind the `dynamics`
//! feature. Everything a game names is engine-owned and reflected:
//!
//! - [`RigidBody`]: makes an entity's [`Collider`](crate::Collider) a
//!   simulated body (dynamic, kinematic or fixed).
//! - [`Velocity`]: initial velocity, and the body's velocity while it runs.
//! - [`Joint`]: connects a body to another body or to the world (fixed,
//!   hinge, ball, prismatic).
//! - [`PhysicsSettings`]: gravity and solver iterations (a settings
//!   resource, `assets/settings/PhysicsSettings.json`).
//! - [`Physics::apply_impulse`] and friends: forces and impulses for the next
//!   step.
//! - [`CollisionEvent`]: contact start and stop between solid colliders,
//!   with the impulse, next to the query layer's trigger events.
//!
//! The rapier world is a [`Dynamics`] resource created by [`step_dynamics`]
//! the first fixed step a `RigidBody` exists, and dropped when the last one
//! is gone: scenes without bodies never initialize it. It mirrors every
//! solid collider (bodies, the static level, kinematic characters) with the
//! same parry shapes the query world uses; rapier 0.36.1 and the query layer
//! share parry3d 0.31.1 and glam 0.33, so no type crosses a conversion.
//!
//! **Order.** [`step_dynamics`] is the first `FixedUpdate` system (label
//! `physics_step`): it reads `Transform`, `Velocity`, joints and queued
//! forces, steps, writes bodies back to `Transform` and `Velocity`, moves
//! their colliders in the query world at once, then sends collision events.
//! Game systems after it see this step's poses; forces they queue apply in
//! the next step.

mod sim;

pub use sim::{ContactPoint, Dynamics, DynamicsStats, step_dynamics};

use box_core::identity::Uid;
use box_core::reflect::{
    Range, Reflect, ReflectComponent, ReflectDefault, ReflectResource, Step, Unit,
};
use box_core::world::Entity;
use glam::{Vec2, Vec3};

use crate::Physics;

/// How a [`RigidBody`] moves.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum BodyKind {
    /// Moved by the simulation: gravity, contacts, joints, forces.
    #[default]
    Dynamic,
    /// Moved by the game through its `Transform` or `Velocity`; pushes
    /// dynamic bodies and carries them (and characters) as a platform.
    Kinematic,
    /// Never moves (unless teleported); like a plain collider, but it can
    /// anchor joints.
    Fixed,
}

/// Axes along or about which a [`RigidBody`] cannot move (world axes).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AxisLocks {
    pub translate_x: bool,
    pub translate_y: bool,
    pub translate_z: bool,
    pub rotate_x: bool,
    pub rotate_y: bool,
    pub rotate_z: bool,
}

impl AxisLocks {
    /// No rotation at all (a character-like body that only slides).
    pub const ROTATION: Self = Self {
        translate_x: false,
        translate_y: false,
        translate_z: false,
        rotate_x: true,
        rotate_y: true,
        rotate_z: true,
    };
}

/// Simulates the entity as a rigid body, using the entity's
/// [`Collider`](crate::Collider) (non-trigger) as its shape. Saved in scenes;
/// the simulation state lives in the [`Dynamics`] resource and is never
/// saved.
///
/// The body is placed by the entity's world transform; the simulation
/// writes its pose back to `Transform` every fixed step (and to `Velocity`
/// if the entity has one). Setting `Transform` yourself teleports the body.
/// Dynamic and kinematic bodies get [`Interpolated`] when added, so they
/// render smoothly at any display rate.
///
/// Colliders on child entities of a body are not part of it (they stay in
/// the query world only); give each simulated part its own body and join
/// them with a [`Joint`].
///
/// [`Interpolated`]: box_core::interpolate::Interpolated
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct RigidBody {
    /// How the body moves: `Dynamic` (simulated), `Kinematic` (moved by
    /// the game, pushes dynamic bodies) or `Fixed` (never moves).
    pub kind: BodyKind,
    /// Mass in kilograms; the inertia follows from the collider's shape.
    #[reflect(@Range { min: 0.001, max: 100000.0 }, @Unit("kg"), @Step(0.1))]
    pub mass: f32,
    /// Slows linear motion over time (air drag); 0 for none.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Step(0.01))]
    pub linear_damping: f32,
    /// Slows rotation over time; 0 for none.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Step(0.01))]
    pub angular_damping: f32,
    /// Multiplies gravity for this body (0: floats, -1: falls up).
    #[reflect(@Range { min: -10.0, max: 10.0 }, @Step(0.1))]
    pub gravity_scale: f32,
    /// Continuous collision detection against moving bodies. A fast body
    /// never passes through static colliders and fixed bodies between two
    /// steps (that sweep is always on); with `ccd` it also cannot pass
    /// through kinematic and dynamic bodies (a projectile hitting a door).
    /// Costs more; use it on small fast bodies.
    pub ccd: bool,
    /// Let the body fall asleep when it comes to rest (it wakes when
    /// touched, pushed or teleported). Sleeping bodies cost almost nothing.
    pub can_sleep: bool,
    /// World axes the body cannot move along or turn about.
    pub locks: AxisLocks,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            kind: BodyKind::Dynamic,
            mass: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            ccd: false,
            can_sleep: true,
            locks: AxisLocks::default(),
        }
    }
}

impl RigidBody {
    /// A dynamic body of `mass` kilograms.
    pub fn dynamic(mass: f32) -> Self {
        Self {
            mass,
            ..Self::default()
        }
    }

    /// A kinematic body: moved by its `Transform` or `Velocity`.
    pub fn kinematic() -> Self {
        Self {
            kind: BodyKind::Kinematic,
            ..Self::default()
        }
    }

    /// A fixed body (an anchor for joints).
    pub fn fixed() -> Self {
        Self {
            kind: BodyKind::Fixed,
            ..Self::default()
        }
    }

    pub fn with_ccd(mut self) -> Self {
        self.ccd = true;
        self
    }

    pub fn with_damping(mut self, linear: f32, angular: f32) -> Self {
        self.linear_damping = linear;
        self.angular_damping = angular;
        self
    }

    pub fn with_locks(mut self, locks: AxisLocks) -> Self {
        self.locks = locks;
        self
    }
}

/// A body's velocity. Saved values are the initial velocity; while the
/// simulation runs it writes the current velocity here every fixed step,
/// and a value you write is applied to the body at the next step. On a
/// kinematic body it moves the body (a moving platform), in world space.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Velocity {
    /// Linear velocity, world space.
    #[reflect(@Unit("m/s"), @Step(0.1))]
    pub linear: Vec3,
    /// Angular velocity, world space: the axis times the speed.
    #[reflect(@Unit("rad/s"), @Step(0.1))]
    pub angular: Vec3,
}

impl Velocity {
    pub fn linear(linear: Vec3) -> Self {
        Self {
            linear,
            angular: Vec3::ZERO,
        }
    }
}

/// What a [`Joint`] lets the two bodies do relative to each other.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum JointKind {
    /// Welded together: no relative motion.
    Fixed,
    /// Turns about `axis` through `anchor` (a door, a wheel).
    #[default]
    Hinge,
    /// Turns freely about `anchor` (a chain link, a ragdoll shoulder).
    Ball,
    /// Slides along `axis` (a drawer, a piston).
    Prismatic,
}

/// Connects this entity's [`RigidBody`] to another body, or to the world.
/// Saved in scenes. One joint per entity: build a chain by giving each
/// link a joint to the previous one.
///
/// `anchor` and `axis` are in this entity's local space. The joint is made
/// when the simulation first sees it, from the bodies' poses at that moment:
/// whatever their relative placement is then becomes the joint's rest pose
/// (angle 0 for a hinge, distance 0 for a prismatic joint). Changing a field
/// remakes the joint from the current poses.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Joint {
    /// What the joint allows: `Fixed` (welded), `Hinge` (turns about
    /// `axis`), `Ball` (turns freely) or `Prismatic` (slides along `axis`).
    pub kind: JointKind,
    /// The other body, by uid; none joins this body to the world at the
    /// anchor.
    pub connected: Option<Uid>,
    /// The joint point, in this entity's local space.
    #[reflect(@Unit("m"), @Step(0.01))]
    pub anchor: Vec3,
    /// Hinge rotation axis or prismatic slide axis, in this entity's local
    /// space. Ignored by fixed and ball joints.
    pub axis: Vec3,
    /// Restrict a hinge's angle (radians) or a prismatic joint's travel
    /// (metres) to `limits`. Ignored by fixed and ball joints.
    pub limits_enabled: bool,
    /// Lowest and highest hinge angle (radians) or travel (metres) from the
    /// rest pose, when `limits_enabled`.
    #[reflect(@Step(0.01))]
    pub limits: Vec2,
    /// Let the two joined bodies collide with each other.
    pub collide_connected: bool,
}

impl Default for Joint {
    fn default() -> Self {
        Self {
            kind: JointKind::Hinge,
            connected: None,
            anchor: Vec3::ZERO,
            axis: Vec3::Y,
            limits_enabled: false,
            limits: Vec2::new(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2),
            collide_connected: false,
        }
    }
}

impl Joint {
    pub fn new(kind: JointKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// A hinge about local `axis` through local `anchor`.
    pub fn hinge(anchor: Vec3, axis: Vec3) -> Self {
        Self {
            kind: JointKind::Hinge,
            anchor,
            axis,
            ..Self::default()
        }
    }

    /// A ball joint at local `anchor`.
    pub fn ball(anchor: Vec3) -> Self {
        Self {
            kind: JointKind::Ball,
            anchor,
            ..Self::default()
        }
    }

    /// A weld at local `anchor`.
    pub fn fixed(anchor: Vec3) -> Self {
        Self {
            kind: JointKind::Fixed,
            anchor,
            ..Self::default()
        }
    }

    /// A slider along local `axis` through local `anchor`.
    pub fn prismatic(anchor: Vec3, axis: Vec3) -> Self {
        Self {
            kind: JointKind::Prismatic,
            anchor,
            axis,
            ..Self::default()
        }
    }

    /// Join to the body of the entity with `uid` instead of the world.
    pub fn connected_to(mut self, uid: Uid) -> Self {
        self.connected = Some(uid);
        self
    }

    /// Limit a hinge's angle or a prismatic joint's travel to `min..=max`.
    pub fn with_limits(mut self, min: f32, max: f32) -> Self {
        self.limits_enabled = true;
        self.limits = Vec2::new(min, max);
        self
    }
}

/// Project-wide simulation settings: a settings resource
/// (`assets/settings/PhysicsSettings.json`, Game ▸ Project Settings).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct PhysicsSettings {
    /// Acceleration of dynamic bodies (times their `gravity_scale`).
    #[reflect(@Unit("m/s²"), @Step(0.1))]
    pub gravity: Vec3,
    /// Solver iterations per step: more make stacks and chains stiffer and
    /// cost more.
    #[reflect(@Range { min: 1.0, max: 32.0 })]
    pub solver_iterations: u32,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            solver_iterations: 4,
        }
    }
}

/// Whether two colliders started or stopped touching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CollisionKind {
    Started,
    Stopped,
}

/// Sent through `Events<CollisionEvent>` (after the dynamics step, so read
/// it in later `FixedUpdate` systems or in `Update`) when two solid
/// colliders start or stop touching and at least one belongs to a dynamic
/// or kinematic body or a kinematic character. Triggers report through
/// [`TriggerEvent`](crate::TriggerEvent) instead. A `Stopped` event for a
/// despawned entity names it although it no longer exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionEvent {
    pub a: Entity,
    pub b: Entity,
    pub kind: CollisionKind,
    /// Total contact impulse of the step the contact started in (N·s), how
    /// hard they hit: 0 for `Stopped`, and for a touch that pushed nothing.
    pub impulse: f32,
}

impl CollisionEvent {
    /// Whether `entity` is one of the two.
    pub fn involves(&self, entity: Entity) -> bool {
        self.a == entity || self.b == entity
    }

    /// The entity `entity` collided with, if it is one of the two.
    pub fn other(&self, entity: Entity) -> Option<Entity> {
        if self.a == entity {
            Some(self.b)
        } else if self.b == entity {
            Some(self.a)
        } else {
            None
        }
    }
}

/// A force or impulse queued on [`Physics`] for the next step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BodyCommand {
    Impulse(Entity, Vec3),
    ImpulseAt(Entity, Vec3, Vec3),
    TorqueImpulse(Entity, Vec3),
    Force(Entity, Vec3),
    Torque(Entity, Vec3),
}

/// Forces and impulses on rigid bodies. Each applies at the next dynamics
/// step (the start of the next fixed step); commands for entities without a
/// dynamic body are dropped. Sleeping bodies wake up.
impl Physics {
    /// Change `entity`'s momentum by `impulse` (N·s, world space) through
    /// its centre of mass: a kick, a jump, an explosion.
    pub fn apply_impulse(&mut self, entity: Entity, impulse: Vec3) {
        self.commands.push(BodyCommand::Impulse(entity, impulse));
    }

    /// [`apply_impulse`](Self::apply_impulse) at the world point `point`,
    /// which also spins the body.
    pub fn apply_impulse_at_point(&mut self, entity: Entity, impulse: Vec3, point: Vec3) {
        self.commands
            .push(BodyCommand::ImpulseAt(entity, impulse, point));
    }

    /// Change `entity`'s angular momentum by `impulse` (N·m·s, world axis
    /// times magnitude).
    pub fn apply_torque_impulse(&mut self, entity: Entity, impulse: Vec3) {
        self.commands
            .push(BodyCommand::TorqueImpulse(entity, impulse));
    }

    /// Push `entity` with `force` (N, world space) during the next step
    /// only: call it every fixed step for a continuous force (thrust, wind).
    pub fn add_force(&mut self, entity: Entity, force: Vec3) {
        self.commands.push(BodyCommand::Force(entity, force));
    }

    /// Turn `entity` with `torque` (N·m) during the next step only.
    pub fn add_torque(&mut self, entity: Entity, torque: Vec3) {
        self.commands.push(BodyCommand::Torque(entity, torque));
    }
}
