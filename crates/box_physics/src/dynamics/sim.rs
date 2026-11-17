//! The [`Dynamics`] resource (the rapier world and its mirror of the ECS)
//! and [`step_dynamics`], the fixed-step system that drives it.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use box_core::event::Events;
use box_core::primitive::{Primitive, Shape};
use box_core::time::Time;
use box_core::transform::{GlobalTransform, Parent, Transform, compute_global};
use box_core::world::{Entity, Without, World};
use glam::{Affine3A, Quat, Vec3};
use parry3d::shape::SharedShape;
use rapier3d::math::Pose;
use rapier3d::prelude as rp;

use super::{
    BodyCommand, BodyKind, CollisionEvent, CollisionKind, Joint, JointKind, PhysicsSettings,
    RigidBody, Velocity,
};
use crate::character::KinematicCharacter;
use box_core::mesh_asset::MeshAsset;

use crate::collider::{Collider, ColliderShape, MeshKey, MeshShapes, Resolved, mesh_key};
use crate::physics::Physics;

/// Counts from the last step, for tools and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DynamicsStats {
    /// Rigid bodies (dynamic, kinematic and fixed).
    pub bodies: usize,
    /// Dynamic and kinematic bodies that are not asleep.
    pub awake: usize,
    /// Static (body-less) colliders mirrored from the level.
    pub static_colliders: usize,
    /// Kinematic characters mirrored as bodies.
    pub characters: usize,
    /// Joints in the simulation.
    pub joints: usize,
    /// Collision events sent.
    pub events: usize,
}

/// A contact point between two colliders, for debug drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactPoint {
    /// World-space point on the first collider.
    pub point: Vec3,
    /// World-space contact normal (from the first collider to the second).
    pub normal: Vec3,
    /// Impulse the solver applied at this point in the last step (N·s).
    pub impulse: f32,
}

/// What a collider was built from; a difference rebuilds it.
#[derive(Clone)]
struct ShapeKey {
    collider: Collider,
    primitive: Option<Shape>,
    /// The mesh asset version, for `ColliderShape::Mesh`.
    mesh: Option<MeshKey>,
    /// Its triangle mesh (not compared: it follows from `mesh` and `scale`).
    mesh_shape: Option<SharedShape>,
    scale: Vec3,
}

impl ShapeKey {
    fn new(
        collider: &Collider,
        primitive: Option<&Primitive>,
        mesh: Option<(MeshKey, SharedShape)>,
        scale: Vec3,
    ) -> Self {
        let primitive = match collider.shape {
            ColliderShape::Primitive => primitive.map(|p| p.shape.clone()),
            _ => None,
        };
        let (mesh, mesh_shape) = match mesh {
            Some((k, s)) if collider.shape.uses_mesh() => (Some(k), Some(s)),
            _ => (None, None),
        };
        Self {
            collider: collider.clone(),
            primitive,
            mesh,
            mesh_shape,
            scale: scale.abs(),
        }
    }

    /// Same inputs, allowing round-off in the scale decomposed from a
    /// rotated matrix.
    fn same(&self, other: &Self) -> bool {
        self.collider == other.collider
            && self.primitive == other.primitive
            && self.mesh == other.mesh
            && same_scale(self.scale, other.scale)
    }

    fn shape(&self) -> Option<SharedShape> {
        if self.collider.shape.uses_mesh() {
            return self.mesh_shape.clone();
        }
        Some(Resolved::new(&self.collider.shape, self.primitive.as_ref())?.build(self.scale))
    }

    /// The shape's offset from the body origin, in the body's frame.
    fn offset(&self) -> Vec3 {
        let o = self.collider.offset;
        if !self.collider.shape.uses_mesh() && o.is_finite() {
            o * self.scale
        } else {
            Vec3::ZERO
        }
    }
}

struct BodyRec {
    handle: rp::RigidBodyHandle,
    collider: Option<rp::ColliderHandle>,
    settings: RigidBody,
    shape: Option<ShapeKey>,
    /// The entity's `Transform` after our last write (or as last seen).
    written: Transform,
    /// The entity's `Velocity` after our last write (or as last seen).
    velocity: Option<Velocity>,
    /// A kinematic body's velocity (from its `Velocity` component).
    kinematic_velocity: Velocity,
    /// Written back once since it fell asleep.
    asleep: bool,
    seen: u64,
}

struct StaticRec {
    /// None for a collider under a body (not simulated) or without a shape
    /// yet.
    collider: Option<rp::ColliderHandle>,
    under_body: bool,
    shape: ShapeKey,
    global: Affine3A,
    seen: u64,
}

struct CharacterRec {
    handle: rp::RigidBodyHandle,
    collider: rp::ColliderHandle,
    size: (f32, f32),
    written: Vec3,
    seen: u64,
}

struct JointRec {
    handle: rp::ImpulseJointHandle,
    joint: Joint,
    bodies: (rp::RigidBodyHandle, rp::RigidBodyHandle),
    seen: u64,
}

/// Collects rapier's collision events during a step.
#[derive(Default)]
struct EventCollector(Mutex<Vec<rp::CollisionEvent>>);

impl rp::EventHandler for EventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &rp::RigidBodySet,
        _colliders: &rp::ColliderSet,
        event: rp::CollisionEvent,
        _contact_pair: Option<&rp::ContactPair>,
    ) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &rp::RigidBodySet,
        _colliders: &rp::ColliderSet,
        _contact_pair: &rp::ContactPair,
        _total_force_magnitude: f32,
    ) {
    }

    fn handle_soft_body_tear_event(
        &self,
        _soft_bodies: &rp::SoftBodySet,
        _event: &rp::SoftBodyTearEvent,
    ) {
    }
}

/// Resource: the rapier simulation, mirrored from `RigidBody`, `Collider`,
/// `KinematicCharacter`, `Joint` and `Velocity` by [`step_dynamics`]. It
/// exists only while the world has rigid bodies. Read-only to games except
/// through components and [`Physics`]' force methods.
pub struct Dynamics {
    rapier: rp::PhysicsWorld,
    /// A fixed body at the origin: the "world" end of world joints.
    anchor: rp::RigidBodyHandle,
    bodies: HashMap<Entity, BodyRec>,
    statics: HashMap<Entity, StaticRec>,
    characters: HashMap<Entity, CharacterRec>,
    joints: HashMap<Entity, JointRec>,
    /// Colliders removed this step, for their `Stopped` events.
    removed: HashMap<rp::ColliderHandle, Entity>,
    /// Bodies with a force or torque to clear after the step.
    forced: Vec<rp::RigidBodyHandle>,
    writes: Vec<(Entity, Transform, Option<Velocity>)>,
    warned: HashSet<(Entity, &'static str)>,
    pass: u64,
    stats: DynamicsStats,
    /// Triangle meshes of `ColliderShape::Mesh` colliders.
    mesh_shapes: MeshShapes,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self::new()
    }
}

impl Dynamics {
    pub fn new() -> Self {
        let mut rapier = rp::PhysicsWorld::new();
        let anchor = rapier.insert_body(rp::RigidBodyBuilder::fixed());
        Self {
            rapier,
            anchor,
            bodies: HashMap::new(),
            statics: HashMap::new(),
            characters: HashMap::new(),
            joints: HashMap::new(),
            removed: HashMap::new(),
            forced: Vec::new(),
            writes: Vec::new(),
            warned: HashSet::new(),
            pass: 0,
            stats: DynamicsStats::default(),
            mesh_shapes: MeshShapes::default(),
        }
    }

    /// Counts from the last step.
    pub fn stats(&self) -> DynamicsStats {
        self.stats
    }

    /// The body's current velocity (dynamic or kinematic body), as of the
    /// last step.
    pub fn velocity(&self, entity: Entity) -> Option<Velocity> {
        let body = self.rapier.bodies.get(self.bodies.get(&entity)?.handle)?;
        Some(Velocity {
            linear: body.linvel(),
            angular: body.angvel(),
        })
    }

    /// Whether `entity`'s body is asleep (at rest and not simulated).
    pub fn is_sleeping(&self, entity: Entity) -> Option<bool> {
        let body = self.rapier.bodies.get(self.bodies.get(&entity)?.handle)?;
        Some(body.is_sleeping())
    }

    /// Whether `entity` has a body in the simulation.
    pub fn has_body(&self, entity: Entity) -> bool {
        self.bodies.contains_key(&entity)
    }

    /// The angle of `entity`'s hinge joint (radians from its rest pose),
    /// or the travel of its prismatic joint (metres).
    pub fn joint_position(&self, entity: Entity) -> Option<f32> {
        let rec = self.joints.get(&entity)?;
        let joint = self.rapier.impulse_joints.get(rec.handle)?;
        let b1 = self.rapier.bodies.get(rec.bodies.0)?.position();
        let b2 = self.rapier.bodies.get(rec.bodies.1)?.position();
        let f1 = *b1 * joint.data.local_frame1;
        let f2 = *b2 * joint.data.local_frame2;
        // Body 1 is the other body (or the world), body 2 the joint's entity.
        match rec.joint.kind {
            JointKind::Hinge => {
                // Rotation of frame 2 relative to frame 1, about frame 1's X.
                let rel = f1.rotation.inverse() * f2.rotation;
                let (axis, angle) = rel.to_axis_angle();
                Some(if axis.x < 0.0 { -angle } else { angle })
            }
            JointKind::Prismatic => {
                let axis = f1.rotation * Vec3::X;
                Some((f2.translation - f1.translation).dot(axis))
            }
            JointKind::Fixed | JointKind::Ball => None,
        }
    }

    /// Every contact point between touching colliders, for debug drawing.
    pub fn contact_points(&self) -> Vec<ContactPoint> {
        let mut out = Vec::new();
        for pair in self.rapier.narrow_phase.contact_pairs() {
            if !pair.has_any_active_contact() {
                continue;
            }
            let Some(c1) = self.rapier.colliders.get(pair.collider1) else {
                continue;
            };
            for m in pair.manifolds() {
                for p in &m.points {
                    if p.dist > 0.0 {
                        continue;
                    }
                    out.push(ContactPoint {
                        point: *c1.position() * p.local_p1,
                        normal: m.data.normal,
                        impulse: p.data.impulse,
                    });
                }
            }
        }
        out
    }

    /// The mesh asset version and shape of a `ColliderShape::Mesh` or
    /// `ConvexHull` collider on `e` (its `MeshAsset`), once loaded.
    fn mesh_shape(
        &mut self,
        world: &World,
        e: Entity,
        collider: &Collider,
        scale: Vec3,
    ) -> Option<(MeshKey, SharedShape)> {
        if !collider.shape.uses_mesh() {
            return None;
        }
        let key = mesh_key(world, world.get::<MeshAsset>(e).as_deref())?;
        Some((
            key,
            self.mesh_shapes.get(world, key, scale, &collider.shape)?,
        ))
    }

    fn warn_once(&mut self, entity: Entity, what: &'static str, message: impl FnOnce() -> String) {
        if self.warned.insert((entity, what)) {
            tracing::warn!("{}", message());
        }
    }

    /// One fixed step of `dt` seconds: mirror the world, apply queued forces,
    /// step, write back, send events.
    #[profiling::function]
    fn step(&mut self, world: &mut World) {
        let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
        let commands = world
            .get_resource_mut::<Physics>()
            .map(|p| std::mem::take(&mut p.commands))
            .unwrap_or_default();
        if dt.is_nan() || dt <= 0.0 {
            return;
        }
        self.pass += 1;
        let settings = world
            .get_resource::<PhysicsSettings>()
            .cloned()
            .unwrap_or_default();
        self.sync_bodies(world, dt);
        self.sync_statics(world);
        self.sync_characters(world);
        self.sync_joints(world);
        self.remove_unseen();
        self.apply_commands(&commands);

        let collector = EventCollector::default();
        {
            profiling::scope!("rapier step");
            self.rapier.gravity = if settings.gravity.is_finite() {
                settings.gravity
            } else {
                Vec3::ZERO
            };
            self.rapier.integration_parameters.dt = dt;
            self.rapier.integration_parameters.num_solver_iterations =
                settings.solver_iterations.clamp(1, 32) as usize;
            self.rapier.step_with_events(&(), &collector);
        }
        for h in self.forced.drain(..) {
            if let Some(b) = self.rapier.bodies.get_mut(h) {
                b.reset_forces(false);
                b.reset_torques(false);
            }
        }
        self.write_back(world);
        let events = self.collision_events(collector);
        self.stats = DynamicsStats {
            bodies: self.bodies.len(),
            awake: self
                .bodies
                .values()
                .filter(|r| r.settings.kind != BodyKind::Fixed)
                .filter(|r| {
                    self.rapier
                        .bodies
                        .get(r.handle)
                        .is_some_and(|b| !b.is_sleeping())
                })
                .count(),
            static_colliders: self
                .statics
                .values()
                .filter(|s| s.collider.is_some())
                .count(),
            characters: self.characters.len(),
            joints: self.joints.len(),
            events: events.len(),
        };
        if !events.is_empty()
            && let Some(queue) = world.get_resource_mut::<Events<CollisionEvent>>()
        {
            queue.send_batch(events);
        }
    }

    #[profiling::function]
    fn sync_bodies(&mut self, world: &World, dt: f32) {
        let pass = self.pass;
        let mut query = world.query::<(
            Entity,
            &RigidBody,
            &Transform,
            Option<&Velocity>,
            Option<&Collider>,
            Option<&Primitive>,
            Option<&Parent>,
        )>();
        for (e, rb, t, vel, collider, primitive, parent) in query.iter() {
            let global = if parent.is_some() {
                compute_global(world, e)
            } else {
                t.to_affine()
            };
            let Some((scale, pose)) = decompose(&global) else {
                self.warn_once(e, "pose", || {
                    format!("rigid body {e:?} has a non-finite transform; it is not simulated")
                });
                continue;
            };
            let mut collider = collider.filter(|c| !c.is_trigger);
            if collider.is_some_and(|c| c.shape == ColliderShape::Mesh)
                && rb.kind != BodyKind::Fixed
            {
                self.warn_once(e, "mesh body", || {
                    format!(
                        "rigid body {e:?} has a Mesh collider: triangle meshes are static, so \
                         it is simulated without a collider (make it Fixed, or use a ConvexHull \
                         or primitive shape)"
                    )
                });
                collider = None;
            }
            let mesh = collider.and_then(|c| self.mesh_shape(world, e, c, scale));
            let key = collider.map(|c| ShapeKey::new(c, primitive, mesh, scale));
            let Some(rec) = self.bodies.get_mut(&e) else {
                let rec = create_body(&mut self.rapier, e, rb, pose, vel, key, t);
                self.bodies.insert(e, BodyRec { seen: pass, ..rec });
                continue;
            };
            rec.seen = pass;
            if rec.settings != *rb {
                if let Some(body) = self.rapier.bodies.get_mut(rec.handle) {
                    apply_settings(body, rb);
                    if rec.collider.is_none() {
                        body.set_additional_mass(rb.mass.max(1e-3), true);
                    }
                }
                if rec.settings.mass != rb.mass
                    && let Some(c) = rec.collider.and_then(|h| self.rapier.colliders.get_mut(h))
                {
                    c.set_mass(rb.mass.max(1e-3));
                }
                rec.settings = rb.clone();
            }
            let shape_changed = match (&rec.shape, &key) {
                (Some(a), Some(b)) => !a.same(b),
                (None, None) => false,
                _ => true,
            };
            if shape_changed {
                if let Some(old) = rec.collider.take() {
                    self.rapier.remove_collider(old);
                    self.removed.insert(old, e);
                }
                rec.collider = key
                    .as_ref()
                    .and_then(|k| body_collider(k, e, rb.mass))
                    .map(|c| self.rapier.insert_collider(c, Some(rec.handle)));
                if rec.collider.is_none()
                    && let Some(body) = self.rapier.bodies.get_mut(rec.handle)
                {
                    body.set_additional_mass(rb.mass.max(1e-3), true);
                }
                rec.shape = key;
            }
            let Some(body) = self.rapier.bodies.get_mut(rec.handle) else {
                continue;
            };
            let teleport = !same_transform(t, &rec.written);
            if teleport {
                rec.written = *t;
            }
            let user_velocity = match (vel, rec.velocity) {
                (Some(v), Some(old)) if *v != old => Some(*v),
                (Some(v), None) => Some(*v),
                (None, Some(_)) => Some(Velocity::default()),
                _ => None,
            };
            rec.velocity = vel.copied();
            match rb.kind {
                BodyKind::Kinematic => {
                    if let Some(v) = user_velocity {
                        rec.kinematic_velocity = v;
                    }
                    let v = rec.kinematic_velocity;
                    if teleport {
                        body.set_next_kinematic_position(pose);
                    } else if v.linear != Vec3::ZERO || v.angular != Vec3::ZERO {
                        let p = *body.position();
                        let rotation =
                            (Quat::from_scaled_axis(v.angular * dt) * p.rotation).normalize();
                        body.set_next_kinematic_position(Pose::from_parts(
                            p.translation + v.linear * dt,
                            rotation,
                        ));
                    }
                }
                BodyKind::Dynamic | BodyKind::Fixed => {
                    if teleport {
                        body.set_position(pose, true);
                    }
                    if let Some(v) = user_velocity
                        && vel.is_some()
                    {
                        body.set_linvel(finite(v.linear), true);
                        body.set_angvel(finite(v.angular), true);
                    }
                }
            }
        }
    }

    #[profiling::function]
    fn sync_statics(&mut self, world: &World) {
        let pass = self.pass;
        let mut query = world.query::<Without<
            Without<(Entity, &Collider, &GlobalTransform, Option<&Primitive>), &RigidBody>,
            &KinematicCharacter,
        >>();
        for (e, collider, global, primitive) in query.iter() {
            if collider.is_trigger {
                continue;
            }
            let Some((scale, pose)) = decompose(&global.0) else {
                continue;
            };
            let mesh = self.mesh_shape(world, e, collider, scale);
            let key = ShapeKey::new(collider, primitive, mesh, scale);
            if let Some(rec) = self.statics.get_mut(&e) {
                rec.seen = pass;
                let Some(handle) = rec.collider else {
                    // Not simulated: under a body, or no shape yet (a mesh
                    // still loading, a primitive not added yet).
                    if !rec.under_body && !rec.shape.same(&key) {
                        rec.collider = static_collider(&key, e, &global.0)
                            .map(|c| self.rapier.insert_collider(c, None));
                        rec.shape = key;
                        rec.global = global.0;
                    }
                    continue;
                };
                if !rec.shape.same(&key) {
                    self.rapier.remove_collider(handle);
                    self.removed.insert(handle, e);
                    rec.collider = static_collider(&key, e, &global.0)
                        .map(|c| self.rapier.insert_collider(c, None));
                    rec.shape = key;
                    rec.global = global.0;
                } else if !same_affine(&rec.global, &global.0) {
                    if let Some(c) = self.rapier.colliders.get_mut(handle) {
                        c.set_position(collider_pose(&key, &global.0, pose));
                    }
                    rec.global = global.0;
                }
                continue;
            }
            let under_body = has_body_ancestor(world, e);
            if under_body {
                self.warn_once(e, "child collider", || {
                    format!(
                        "collider on {e:?} is under a RigidBody entity: it is not part of the \
                         body (give it its own RigidBody and a Joint)"
                    )
                });
            }
            let handle = if under_body {
                None
            } else {
                static_collider(&key, e, &global.0).map(|c| self.rapier.insert_collider(c, None))
            };
            self.statics.insert(
                e,
                StaticRec {
                    collider: handle,
                    under_body,
                    shape: key,
                    global: global.0,
                    seen: pass,
                },
            );
        }
    }

    #[profiling::function]
    fn sync_characters(&mut self, world: &World) {
        let pass = self.pass;
        let mut query =
            world.query::<Without<(Entity, &KinematicCharacter, &Transform), &RigidBody>>();
        for (e, character, t) in query.iter() {
            let size = (character.radius, character.height);
            let at = t.translation;
            if !at.is_finite() {
                continue;
            }
            if let Some(rec) = self.characters.get_mut(&e) {
                rec.seen = pass;
                if rec.size != size {
                    self.rapier.remove_collider(rec.collider);
                    self.removed.insert(rec.collider, e);
                    rec.collider = self
                        .rapier
                        .insert_collider(character_collider(character, e), Some(rec.handle));
                    rec.size = size;
                }
                if at != rec.written
                    && let Some(body) = self.rapier.bodies.get_mut(rec.handle)
                {
                    body.set_next_kinematic_translation(at);
                    rec.written = at;
                }
                continue;
            }
            let handle = self.rapier.insert_body(
                rp::RigidBodyBuilder::kinematic_position_based()
                    .translation(at)
                    .user_data(entity_data(e)),
            );
            let collider = self
                .rapier
                .insert_collider(character_collider(character, e), Some(handle));
            self.characters.insert(
                e,
                CharacterRec {
                    handle,
                    collider,
                    size,
                    written: at,
                    seen: pass,
                },
            );
        }
    }

    #[profiling::function]
    fn sync_joints(&mut self, world: &World) {
        let pass = self.pass;
        let mut query = world.query::<(Entity, &Joint)>();
        for (e, joint) in query.iter() {
            let Some(b1) = self.bodies.get(&e).map(|r| r.handle) else {
                self.warn_once(e, "joint body", || {
                    format!("Joint on {e:?} needs a RigidBody on the same entity")
                });
                continue;
            };
            let b2 = match joint.connected {
                None => self.anchor,
                Some(uid) => {
                    let target = world
                        .entity_by_uid(uid)
                        .and_then(|t| self.bodies.get(&t).map(|r| r.handle));
                    match target {
                        Some(h) if h != b1 => h,
                        _ => {
                            self.warn_once(e, "joint target", || {
                                format!(
                                    "Joint on {e:?}: connected entity {uid} has no RigidBody \
                                     (or is the same entity); the joint waits for it"
                                )
                            });
                            continue;
                        }
                    }
                }
            };
            if let Some(rec) = self.joints.get_mut(&e) {
                if rec.joint == *joint
                    && rec.bodies == (b2, b1)
                    && self.rapier.impulse_joints.get(rec.handle).is_some()
                {
                    rec.seen = pass;
                    continue;
                }
                if self.rapier.impulse_joints.get(rec.handle).is_some() {
                    self.rapier.remove_impulse_joint(rec.handle);
                }
            }
            let Some(data) = joint_data(&self.rapier, joint, b2, b1, &self.bodies, e) else {
                continue;
            };
            let handle = self.rapier.insert_impulse_joint(b2, b1, data);
            self.joints.insert(
                e,
                JointRec {
                    handle,
                    joint: joint.clone(),
                    bodies: (b2, b1),
                    seen: pass,
                },
            );
        }
    }

    /// Remove what the world no longer has, in entity order (handles are
    /// reused, so the order must not depend on hashing).
    fn remove_unseen(&mut self) {
        let pass = self.pass;
        let mut gone: Vec<Entity> = self
            .joints
            .iter()
            .filter(|(_, r)| r.seen != pass)
            .map(|(e, _)| *e)
            .collect();
        gone.sort_unstable();
        for e in gone {
            if let Some(rec) = self.joints.remove(&e)
                && self.rapier.impulse_joints.get(rec.handle).is_some()
            {
                self.rapier.remove_impulse_joint(rec.handle);
            }
        }
        let mut gone: Vec<Entity> = self
            .bodies
            .iter()
            .filter(|(_, r)| r.seen != pass)
            .map(|(e, _)| *e)
            .collect();
        gone.sort_unstable();
        for e in gone {
            if let Some(rec) = self.bodies.remove(&e) {
                if let Some(c) = rec.collider {
                    self.removed.insert(c, e);
                }
                self.rapier.remove_body(rec.handle);
            }
        }
        let mut gone: Vec<Entity> = self
            .statics
            .iter()
            .filter(|(_, r)| r.seen != pass)
            .map(|(e, _)| *e)
            .collect();
        gone.sort_unstable();
        for e in gone {
            if let Some(StaticRec {
                collider: Some(c), ..
            }) = self.statics.remove(&e)
            {
                self.removed.insert(c, e);
                self.rapier.remove_collider(c);
            }
        }
        let mut gone: Vec<Entity> = self
            .characters
            .iter()
            .filter(|(_, r)| r.seen != pass)
            .map(|(e, _)| *e)
            .collect();
        gone.sort_unstable();
        for e in gone {
            if let Some(rec) = self.characters.remove(&e) {
                self.removed.insert(rec.collider, e);
                self.rapier.remove_body(rec.handle);
            }
        }
    }

    fn apply_commands(&mut self, commands: &[BodyCommand]) {
        for cmd in commands {
            let (BodyCommand::Impulse(e, _)
            | BodyCommand::ImpulseAt(e, _, _)
            | BodyCommand::TorqueImpulse(e, _)
            | BodyCommand::Force(e, _)
            | BodyCommand::Torque(e, _)) = *cmd;
            let Some(rec) = self.bodies.get(&e) else {
                continue;
            };
            if rec.settings.kind != BodyKind::Dynamic {
                continue;
            }
            let Some(body) = self.rapier.bodies.get_mut(rec.handle) else {
                continue;
            };
            match *cmd {
                BodyCommand::Impulse(_, j) => body.apply_impulse(finite(j), true),
                BodyCommand::ImpulseAt(_, j, p) if p.is_finite() => {
                    body.apply_impulse_at_point(finite(j), p, true)
                }
                BodyCommand::ImpulseAt(_, j, _) => body.apply_impulse(finite(j), true),
                BodyCommand::TorqueImpulse(_, j) => body.apply_torque_impulse(finite(j), true),
                BodyCommand::Force(_, f) => {
                    body.add_force(finite(f), true);
                    self.forced.push(rec.handle);
                }
                BodyCommand::Torque(_, t) => {
                    body.add_torque(finite(t), true);
                    self.forced.push(rec.handle);
                }
            }
        }
    }

    /// Write moved bodies back to `Transform` and `Velocity`, and move their
    /// colliders in the query world.
    #[profiling::function]
    fn write_back(&mut self, world: &mut World) {
        let mut writes = std::mem::take(&mut self.writes);
        writes.clear();
        {
            let mut query = world.query::<(Entity, &RigidBody, Option<&Parent>)>();
            for (e, _, parent) in query.iter() {
                let Some(rec) = self.bodies.get_mut(&e) else {
                    continue;
                };
                if rec.settings.kind == BodyKind::Fixed {
                    continue;
                }
                let Some(body) = self.rapier.bodies.get(rec.handle) else {
                    continue;
                };
                let sleeping = body.is_sleeping();
                if sleeping && rec.asleep {
                    continue;
                }
                rec.asleep = sleeping;
                let p = *body.position();
                let mut t = rec.written;
                match parent {
                    Some(parent) => {
                        let parent_global = compute_global(world, parent.0);
                        let local = parent_global.inverse()
                            * Affine3A::from_rotation_translation(p.rotation, p.translation);
                        let (_, rotation, translation) = local.to_scale_rotation_translation();
                        t.translation = translation;
                        t.rotation = rotation.normalize();
                    }
                    None => {
                        t.translation = p.translation;
                        t.rotation = p.rotation;
                    }
                }
                let velocity = (rec.settings.kind == BodyKind::Dynamic && rec.velocity.is_some())
                    .then(|| Velocity {
                        linear: body.linvel(),
                        angular: body.angvel(),
                    });
                rec.written = t;
                if velocity.is_some() {
                    rec.velocity = velocity;
                }
                writes.push((e, t, velocity));
            }
        }
        let mut refit = false;
        world.resource_or_default::<Physics>();
        world.resource_scope::<Physics, _>(|world, physics| {
            for (e, t, velocity) in &writes {
                let parent = world.parent(*e);
                if let Some(tc) = world.component_mut::<Transform>(*e) {
                    *tc = *t;
                }
                if let Some(v) = velocity
                    && let Some(vc) = world.component_mut::<Velocity>(*e)
                {
                    *vc = *v;
                }
                let global = match parent {
                    Some(_) => compute_global(world, *e),
                    None => t.to_affine(),
                };
                refit |= physics.move_collider(*e, global);
            }
            if refit {
                physics.refit();
            }
        });
        self.writes = writes;
    }

    fn entity_of(&self, handle: rp::ColliderHandle) -> Option<Entity> {
        match self.rapier.colliders.get(handle) {
            Some(c) => Entity::from_bits(c.user_data as u64),
            None => self.removed.get(&handle).copied(),
        }
    }

    fn collision_events(&mut self, collector: EventCollector) -> Vec<CollisionEvent> {
        let raw = collector.0.into_inner().unwrap_or_default();
        let mut out = Vec::with_capacity(raw.len());
        for event in raw {
            let (h1, h2) = (event.collider1(), event.collider2());
            let (Some(a), Some(b)) = (self.entity_of(h1), self.entity_of(h2)) else {
                continue;
            };
            let (kind, impulse) = if event.started() {
                let impulse = self
                    .rapier
                    .narrow_phase
                    .contact_pair(h1, h2)
                    .map_or(0.0, |p| p.total_impulse_magnitude());
                (CollisionKind::Started, impulse)
            } else {
                (CollisionKind::Stopped, 0.0)
            };
            out.push(CollisionEvent {
                a,
                b,
                kind,
                impulse,
            });
        }
        self.removed.clear();
        out
    }
}

fn entity_data(e: Entity) -> u128 {
    u128::from(e.to_bits().get())
}

fn finite(v: Vec3) -> Vec3 {
    if v.is_finite() { v } else { Vec3::ZERO }
}

fn decompose(global: &Affine3A) -> Option<(Vec3, Pose)> {
    let (scale, rotation, translation) = global.to_scale_rotation_translation();
    let rotation = rotation.normalize();
    (scale.is_finite() && rotation.is_finite() && translation.is_finite())
        .then(|| (scale.abs(), Pose::from_parts(translation, rotation)))
}

/// The world pose of a static collider's shape (its offset applied).
fn collider_pose(key: &ShapeKey, global: &Affine3A, pose: Pose) -> Pose {
    let o = key.collider.offset;
    let o = if !key.collider.shape.uses_mesh() && o.is_finite() {
        o
    } else {
        Vec3::ZERO
    };
    Pose::from_parts(global.transform_point3(o), pose.rotation)
}

fn groups(c: &Collider) -> rp::InteractionGroups {
    rp::InteractionGroups::new(
        rp::Group::from_bits_truncate(c.layers),
        rp::Group::from_bits_truncate(c.mask),
        rp::InteractionTestMode::And,
    )
}

fn material(builder: rp::ColliderBuilder, c: &Collider) -> rp::ColliderBuilder {
    let friction = if c.friction.is_finite() {
        c.friction.max(0.0)
    } else {
        0.5
    };
    let restitution = if c.restitution.is_finite() {
        c.restitution.clamp(0.0, 1.0)
    } else {
        0.0
    };
    builder
        .friction(friction)
        .restitution(restitution)
        .collision_groups(groups(c))
}

fn body_collider(key: &ShapeKey, e: Entity, mass: f32) -> Option<rp::Collider> {
    let builder = rp::ColliderBuilder::new(key.shape()?)
        .position(Pose::from_translation(key.offset()))
        .mass(mass.max(1e-3))
        .active_events(rp::ActiveEvents::COLLISION_EVENTS)
        .user_data(entity_data(e));
    Some(material(builder, &key.collider).build())
}

fn static_collider(key: &ShapeKey, e: Entity, global: &Affine3A) -> Option<rp::Collider> {
    let (_, pose) = decompose(global)?;
    let builder = rp::ColliderBuilder::new(key.shape()?)
        .position(collider_pose(key, global, pose))
        .user_data(entity_data(e));
    Some(material(builder, &key.collider).build())
}

fn character_collider(c: &KinematicCharacter, e: Entity) -> rp::Collider {
    let radius = c.radius.max(0.01);
    let half = (c.half_height() - radius).max(0.0);
    rp::ColliderBuilder::capsule_y(half, radius)
        .active_events(rp::ActiveEvents::COLLISION_EVENTS)
        .user_data(entity_data(e))
        .build()
}

fn body_type(kind: BodyKind) -> rp::RigidBodyType {
    match kind {
        BodyKind::Dynamic => rp::RigidBodyType::Dynamic,
        BodyKind::Kinematic => rp::RigidBodyType::KinematicPositionBased,
        BodyKind::Fixed => rp::RigidBodyType::Fixed,
    }
}

fn locked_axes(rb: &RigidBody) -> rp::LockedAxes {
    let l = &rb.locks;
    let mut axes = rp::LockedAxes::empty();
    for (on, flag) in [
        (l.translate_x, rp::LockedAxes::TRANSLATION_LOCKED_X),
        (l.translate_y, rp::LockedAxes::TRANSLATION_LOCKED_Y),
        (l.translate_z, rp::LockedAxes::TRANSLATION_LOCKED_Z),
        (l.rotate_x, rp::LockedAxes::ROTATION_LOCKED_X),
        (l.rotate_y, rp::LockedAxes::ROTATION_LOCKED_Y),
        (l.rotate_z, rp::LockedAxes::ROTATION_LOCKED_Z),
    ] {
        if on {
            axes |= flag;
        }
    }
    axes
}

fn nonneg(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

fn apply_settings(body: &mut rp::RigidBody, rb: &RigidBody) {
    body.set_body_type(body_type(rb.kind), true);
    body.set_linear_damping(nonneg(rb.linear_damping));
    body.set_angular_damping(nonneg(rb.angular_damping));
    body.set_gravity_scale(
        if rb.gravity_scale.is_finite() {
            rb.gravity_scale
        } else {
            1.0
        },
        true,
    );
    body.enable_ccd(rb.ccd);
    body.set_locked_axes(locked_axes(rb), true);
    let activation = body.activation_mut();
    if rb.can_sleep {
        let defaults = rp::RigidBodyActivation::active();
        activation.normalized_linear_threshold = defaults.normalized_linear_threshold;
        activation.angular_threshold = defaults.angular_threshold;
    } else {
        let never = rp::RigidBodyActivation::cannot_sleep();
        activation.normalized_linear_threshold = never.normalized_linear_threshold;
        activation.angular_threshold = never.angular_threshold;
        activation.wake_up(true);
    }
}

fn create_body(
    rapier: &mut rp::PhysicsWorld,
    e: Entity,
    rb: &RigidBody,
    pose: Pose,
    velocity: Option<&Velocity>,
    shape: Option<ShapeKey>,
    t: &Transform,
) -> BodyRec {
    let v = velocity.copied().unwrap_or_default();
    let mut builder = rp::RigidBodyBuilder::new(body_type(rb.kind))
        .pose(pose)
        .linear_damping(nonneg(rb.linear_damping))
        .angular_damping(nonneg(rb.angular_damping))
        .gravity_scale(if rb.gravity_scale.is_finite() {
            rb.gravity_scale
        } else {
            1.0
        })
        .ccd_enabled(rb.ccd)
        .can_sleep(rb.can_sleep)
        .locked_axes(locked_axes(rb))
        .user_data(entity_data(e));
    if rb.kind == BodyKind::Dynamic {
        builder = builder.linvel(finite(v.linear)).angvel(finite(v.angular));
    }
    let handle = rapier.insert_body(builder);
    let collider = shape
        .as_ref()
        .and_then(|k| body_collider(k, e, rb.mass))
        .map(|c| rapier.insert_collider(c, Some(handle)));
    if collider.is_none()
        && let Some(body) = rapier.bodies.get_mut(handle)
    {
        body.set_additional_mass(rb.mass.max(1e-3), false);
    }
    BodyRec {
        handle,
        collider,
        settings: rb.clone(),
        shape,
        written: *t,
        velocity: velocity.copied(),
        kinematic_velocity: if rb.kind == BodyKind::Kinematic {
            v
        } else {
            Velocity::default()
        },
        asleep: false,
        seen: 0,
    }
}

/// The rapier joint for `joint` between `other` (body 1: the connected body
/// or the world anchor) and `this` (body 2: the joint's entity), with frames
/// placed so the bodies' current poses are its rest pose. With the joint's
/// entity second, a hinge's angle is this body's rotation about the axis
/// relative to the other (right-handed) and a prismatic joint's travel is
/// its displacement along the axis.
fn joint_data(
    rapier: &rp::PhysicsWorld,
    joint: &Joint,
    other: rp::RigidBodyHandle,
    this: rp::RigidBodyHandle,
    bodies: &HashMap<Entity, BodyRec>,
    e: Entity,
) -> Option<rp::GenericJoint> {
    let p_other = *rapier.bodies.get(other)?.position();
    let p_this = *rapier.bodies.get(this)?.position();
    // The anchor scales with the entity, like a collider offset.
    let scale = bodies
        .get(&e)
        .and_then(|r| r.shape.as_ref().map(|s| s.scale))
        .unwrap_or(Vec3::ONE);
    let anchor = if joint.anchor.is_finite() {
        joint.anchor * scale
    } else {
        Vec3::ZERO
    };
    let axis = joint.axis.try_normalize().unwrap_or(Vec3::Y);
    let frame2 = Pose::from_parts(anchor, rp::GenericJoint::complete_ang_frame(axis));
    let frame1 = p_other.inverse() * (p_this * frame2);
    let (mask, limit_axis) = match joint.kind {
        JointKind::Fixed => (rp::JointAxesMask::LOCKED_FIXED_AXES, None),
        JointKind::Hinge => (
            rp::JointAxesMask::LOCKED_REVOLUTE_AXES,
            Some(rp::JointAxis::AngX),
        ),
        JointKind::Ball => (rp::JointAxesMask::LOCKED_SPHERICAL_AXES, None),
        JointKind::Prismatic => (
            rp::JointAxesMask::LOCKED_PRISMATIC_AXES,
            Some(rp::JointAxis::LinX),
        ),
    };
    let mut builder = rp::GenericJointBuilder::new(mask)
        .local_frame1(frame1)
        .local_frame2(frame2)
        .contacts_enabled(joint.collide_connected);
    if joint.limits_enabled
        && let Some(axis) = limit_axis
        && joint.limits.is_finite()
    {
        let (lo, hi) = (
            joint.limits.x.min(joint.limits.y),
            joint.limits.x.max(joint.limits.y),
        );
        builder = builder.limits(axis, [lo, hi]);
    }
    Some(builder.build())
}

fn has_body_ancestor(world: &World, e: Entity) -> bool {
    let mut cur = world.parent(e);
    for _ in 0..256 {
        let Some(p) = cur else { return false };
        if world.has::<RigidBody>(p) {
            return true;
        }
        cur = world.parent(p);
    }
    false
}

/// Bitwise equality, so a NaN does not count as a change every step.
fn same_transform(a: &Transform, b: &Transform) -> bool {
    a.translation.to_array().map(f32::to_bits) == b.translation.to_array().map(f32::to_bits)
        && a.rotation.to_array().map(f32::to_bits) == b.rotation.to_array().map(f32::to_bits)
        && a.scale.to_array().map(f32::to_bits) == b.scale.to_array().map(f32::to_bits)
}

fn same_affine(a: &Affine3A, b: &Affine3A) -> bool {
    a.to_cols_array().map(f32::to_bits) == b.to_cols_array().map(f32::to_bits)
}

fn same_scale(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() <= 1e-5 * a.max_element().max(b.max_element()).max(1e-6)
}

/// Run one dynamics step (the `physics_step` system, first in
/// `FixedUpdate`; [`PhysicsPlugin`](crate::PhysicsPlugin) adds it with the
/// `dynamics` feature).
///
/// Costs nothing measurable while the world has no `RigidBody`: one
/// archetype scan, and the [`Dynamics`] resource is not created (or is
/// dropped when the last body goes). Otherwise, per step: one compare pass
/// over bodies, solid colliders, characters and joints (only differences
/// touch rapier), the rapier step, and a write-back of awake bodies only.
pub fn step_dynamics(world: &mut World) {
    if world.first_with::<RigidBody>().is_none() {
        if world.remove_resource::<Dynamics>().is_some() {
            tracing::info!("dynamics stopped: no rigid bodies left");
        }
        if let Some(p) = world.get_resource_mut::<Physics>() {
            p.commands.clear();
        }
        return;
    }
    if !world.contains_resource::<Dynamics>() {
        tracing::info!("dynamics started");
        world.insert_resource(Dynamics::new());
    }
    world.resource_scope::<Dynamics, _>(|world, dynamics| dynamics.step(world));
}
