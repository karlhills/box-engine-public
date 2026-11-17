//! Kinematic character movement: [`Physics::move_and_slide`] (collide and
//! slide with shape casts), the [`KinematicCharacter`] component and
//! [`CharacterPlugin`], which moves characters in `FixedUpdate` with
//! gravity, jumps and ground snapping. The model is Godot's
//! `CharacterBody3D.move_and_slide` and Unity's `CharacterController.Move`.

use std::sync::atomic::{AtomicBool, Ordering};

use box_core::app::{App, Plugin, Stage};
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Unit};
use box_core::time::Time;
use box_core::transform::Transform;
use box_core::world::{Entity, World};
use glam::{Quat, Vec3};

use crate::physics::Physics;
use crate::query::{Pose, QueryFilter, QueryShape, ShapeHit};

/// How [`Physics::move_and_slide`] moves a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlideOptions {
    /// Slide iterations per pass (a corner needs 2, a crease 3).
    pub max_slides: usize,
    /// Gap kept between the shape and what it touches, in metres. Casts start
    /// this far from surfaces, so sliding along a floor or a wall never
    /// starts in contact.
    pub skin: f32,
    /// Surfaces whose normal's Y is at least this are floors (0.6 is about
    /// 53 degrees); steeper ones are walls, which block horizontal motion
    /// without lifting the shape (walls cannot be climbed).
    pub walkable_normal_y: f32,
    /// After moving, look this far below for a floor and snap onto it
    /// (unless moving up): keeps a character on the ground down slopes and
    /// steps. 0 only detects a floor within `skin`.
    pub ground_snap: f32,
    /// Highest step climbed without jumping, in metres, when `on_ground`: a
    /// wall lower than this is stepped onto. While `on_ground`, the ground
    /// probe also reaches this far down, so the shape walks down stairs
    /// instead of falling off each step. 0 disables both.
    pub step_height: f32,
    /// The shape stands on the ground before this move (a character's
    /// `grounded` from its last step). Enables `step_height`.
    pub on_ground: bool,
    /// Which colliders block (exclude the moving entity's own collider).
    pub filter: QueryFilter,
}

impl Default for SlideOptions {
    fn default() -> Self {
        Self {
            max_slides: 4,
            skin: 0.02,
            walkable_normal_y: 0.6,
            ground_snap: 0.1,
            step_height: 0.0,
            on_ground: false,
            filter: QueryFilter::default(),
        }
    }
}

/// How far past an edge contact the ground probe looks for the face the
/// shape stands on.
const EDGE_PROBE: f32 = 0.02;

/// What [`Physics::move_and_slide`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlideResult {
    /// Where the shape ended up.
    pub position: Vec3,
    /// Every surface hit while sliding, in order (not the ground probe).
    pub hits: Vec<ShapeHit>,
    /// A walkable floor is under the shape after the move.
    pub grounded: bool,
    /// The floor's normal when `grounded`, else zero.
    pub ground_normal: Vec3,
    /// The entity whose collider is the floor, when `grounded`.
    pub ground_entity: Option<Entity>,
    /// Moving up, the shape hit a ceiling (a surface facing down).
    pub hit_ceiling: bool,
}

impl Physics {
    /// Move `shape` from `pose` by `motion`, sliding along what it hits,
    /// then find and snap onto the ground. For kinematic characters: pass
    /// `velocity * dt` every fixed step.
    ///
    /// The horizontal part of `motion` moves first, sliding along walls
    /// (steep surfaces block only horizontally, so they cannot be climbed;
    /// walkable slopes are followed; with
    /// [`on_ground`](SlideOptions::on_ground), a wall no higher than
    /// [`step_height`](SlideOptions::step_height) is stepped onto); then the
    /// vertical part, stopping on floors and ceilings (steep surfaces are
    /// slid down); then, unless moving up, a downward probe of
    /// [`ground_snap`](SlideOptions::ground_snap) (at least `step_height`
    /// when `on_ground`) finds a walkable floor and puts the shape on it.
    /// Standing on an edge counts as standing on the face beyond it. Every
    /// cast is swept, so nothing tunnels through thin walls at any speed.
    /// Colliders are as of the last [`sync_physics`](crate::sync_physics),
    /// or the last dynamics step for rigid bodies.
    pub fn move_and_slide(
        &self,
        shape: &QueryShape,
        pose: impl Into<Pose>,
        motion: Vec3,
        options: &SlideOptions,
    ) -> SlideResult {
        let pose = pose.into();
        let mut out = SlideResult::default();
        let rotation = pose.rotation;
        let horizontal = Vec3::new(motion.x, 0.0, motion.z);
        let mut pos = self.slide(
            shape,
            pose.translation,
            rotation,
            horizontal,
            true,
            options,
            &mut out.hits,
        );
        if motion.y != 0.0 {
            let first = out.hits.len();
            pos = self.slide(
                shape,
                pos,
                rotation,
                Vec3::Y * motion.y,
                false,
                options,
                &mut out.hits,
            );
            out.hit_ceiling = motion.y > 0.0 && out.hits[first..].iter().any(|h| h.normal.y < -0.5);
        }
        let mut snap = options.ground_snap.max(0.0);
        if options.on_ground {
            snap = snap.max(options.step_height);
        }
        if motion.y <= 0.0
            && let Some(hit) = self.shapecast(
                shape,
                Pose::new(pos, rotation),
                Vec3::NEG_Y,
                snap + options.skin,
                &options.filter,
            )
            && let Some(normal) = self.walkable_normal(&hit, options)
        {
            out.grounded = true;
            out.ground_normal = normal;
            out.ground_entity = Some(hit.entity);
            if hit.penetrating {
                pos.y += options.skin;
            } else {
                pos.y -= (hit.toi - options.skin).max(0.0);
            }
        }
        out.position = pos;
        out
    }

    /// One collide-and-slide pass of `motion` from `pos`. In the horizontal
    /// pass steep surfaces block only horizontally.
    #[allow(clippy::too_many_arguments)]
    fn slide(
        &self,
        shape: &QueryShape,
        mut pos: Vec3,
        rotation: Quat,
        motion: Vec3,
        horizontal: bool,
        options: &SlideOptions,
        hits: &mut Vec<ShapeHit>,
    ) -> Vec3 {
        let skin = options.skin.max(0.0);
        let mut remaining = motion;
        for _ in 0..options.max_slides {
            let dist = remaining.length();
            if dist <= 1e-6 || !dist.is_finite() {
                break;
            }
            let dir = remaining / dist;
            let Some(hit) = self.shapecast(
                shape,
                Pose::new(pos, rotation),
                dir,
                dist + skin,
                &options.filter,
            ) else {
                pos += remaining;
                break;
            };
            let stepping =
                horizontal && options.on_ground && options.step_height > 0.0 && !hit.penetrating;
            // While stepping is on, an edge (a stair nosing) is an obstacle
            // to step onto, even where its normal looks like a gentle slope;
            // otherwise the round bottom would climb past `step_height`.
            let wall = hit.normal.y < options.walkable_normal_y
                || (stepping && self.is_edge(&hit, options));
            if stepping
                && wall
                && let Some((stepped, rest)) =
                    self.step_up(shape, pos, rotation, remaining, hit.toi, options)
            {
                pos = stepped;
                remaining = rest;
                continue;
            }
            hits.push(hit);
            let mut n = hit.normal;
            if horizontal && wall {
                n = Vec3::new(n.x, 0.0, n.z).normalize_or(-dir);
            }
            if hit.penetrating {
                // Already touching: step out along the normal and keep the
                // motion that does not push further in.
                pos += n * skin;
                remaining -= n * remaining.dot(n).min(0.0);
                continue;
            }
            let approach = -dir.dot(n);
            let back = skin / approach.max(0.2);
            let travel = (hit.toi - back).max(0.0);
            if travel >= dist {
                pos += remaining;
                break;
            }
            pos += dir * travel;
            remaining = dir * (dist - travel);
            remaining -= n * remaining.dot(n);
        }
        pos
    }

    /// Try to climb the obstacle that stopped a horizontal `motion` after
    /// `blocked_toi`: up by at most `step_height`, forward, then down onto a
    /// walkable top. Returns the landing position and the motion left.
    fn step_up(
        &self,
        shape: &QueryShape,
        pos: Vec3,
        rotation: Quat,
        motion: Vec3,
        blocked_toi: f32,
        options: &SlideOptions,
    ) -> Option<(Vec3, Vec3)> {
        let skin = options.skin.max(0.0);
        let dist = motion.length();
        let dir = motion / dist;
        let up = match self.shapecast(
            shape,
            Pose::new(pos, rotation),
            Vec3::Y,
            options.step_height + skin,
            &options.filter,
        ) {
            Some(h) if h.penetrating => return None,
            Some(h) => (h.toi - skin).max(0.0),
            None => options.step_height,
        };
        if up <= skin {
            return None;
        }
        let raised = pos + Vec3::Y * up;
        let forward = match self.shapecast(
            shape,
            Pose::new(raised, rotation),
            dir,
            dist + skin,
            &options.filter,
        ) {
            Some(h) if h.penetrating => return None,
            Some(h) => (h.toi - skin).clamp(0.0, dist),
            None => dist,
        };
        // Only a step if the raised shape gets further than the blocked one.
        if forward <= (blocked_toi - skin).max(0.0) + 1e-3 {
            return None;
        }
        let advanced = raised + dir * forward;
        let down = self.shapecast(
            shape,
            Pose::new(advanced, rotation),
            Vec3::NEG_Y,
            up + skin * 2.0,
            &options.filter,
        )?;
        if down.penetrating {
            return None;
        }
        // What it lands on must be at most `step_height` above the feet.
        let feet = shape_bottom(shape, pos, rotation)?;
        if down.point.y - feet > options.step_height + skin {
            return None;
        }
        self.walkable_normal(&down, options)?;
        let landed = advanced - Vec3::Y * (down.toi - skin).max(0.0);
        Some((landed, dir * (dist - forward)))
    }

    /// Whether a walkable-looking `hit` is on an edge or corner: the face
    /// just past the contact (a short ray down) faces another way.
    fn is_edge(&self, hit: &ShapeHit, options: &SlideOptions) -> bool {
        if hit.normal.y <= 0.0 {
            return false;
        }
        let away = Vec3::new(-hit.normal.x, 0.0, -hit.normal.z).normalize_or_zero();
        let origin = hit.point + away * EDGE_PROBE + Vec3::Y * EDGE_PROBE;
        match self.raycast(
            &box_core::math::Ray::new(origin, Vec3::NEG_Y),
            EDGE_PROBE * 2.0,
            &options.filter,
        ) {
            Some(face) => face.normal.dot(hit.normal) < 0.99,
            None => true,
        }
    }

    /// The normal of the walkable face under a downward cast's `hit`, if
    /// there is one: the hit normal when it is walkable, or, for a contact
    /// on an edge or a corner (whose normal leans between two faces), the
    /// normal of the face just past it, found with a short ray straight
    /// down. So a capsule standing on the edge of a step stands on the step.
    fn walkable_normal(&self, hit: &ShapeHit, options: &SlideOptions) -> Option<Vec3> {
        if hit.normal.y >= options.walkable_normal_y {
            return Some(hit.normal);
        }
        if hit.normal.y <= 0.0 {
            return None;
        }
        let away = Vec3::new(-hit.normal.x, 0.0, -hit.normal.z).normalize_or_zero();
        let origin = hit.point + away * EDGE_PROBE + Vec3::Y * EDGE_PROBE;
        let face = self.raycast(
            &box_core::math::Ray::new(origin, Vec3::NEG_Y),
            EDGE_PROBE * 2.0,
            &options.filter,
        )?;
        (face.entity == hit.entity && face.normal.y >= options.walkable_normal_y)
            .then_some(face.normal)
    }
}

/// The lowest point of `shape` at `pos` (its bounding box bottom).
fn shape_bottom(shape: &QueryShape, pos: Vec3, rotation: Quat) -> Option<f32> {
    let local = shape.to_parry()?;
    let pose = Pose::new(pos, rotation).to_parry()?;
    Some(local.as_dyn().compute_aabb(&pose).mins.y)
}

/// A capsule character moved by [`CharacterPlugin`] every fixed step:
/// set the horizontal part of [`velocity`](Self::velocity) (and call
/// [`jump`](Self::jump)) from input in `FixedUpdate` before
/// `move_characters`; the system adds gravity, moves with
/// [`Physics::move_and_slide`] and updates
/// [`grounded`](Self::grounded). The entity's origin is the capsule centre
/// and its `Transform` is taken as world space (make it a root entity). The
/// capsule stays upright whatever the entity's rotation.
///
/// The settings are saved in scenes; `velocity`, `grounded` and
/// `ground_normal` are runtime state.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct KinematicCharacter {
    /// Capsule radius.
    #[reflect(@Range { min: 0.01, max: 5.0 }, @Unit("m"))]
    pub radius: f32,
    /// Total capsule height (at least twice the radius).
    #[reflect(@Range { min: 0.02, max: 10.0 }, @Unit("m"))]
    pub height: f32,
    /// Downward acceleration while airborne.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m/s²"))]
    pub gravity: f32,
    /// Falling speed limit.
    #[reflect(@Range { min: 0.0, max: 200.0 }, @Unit("m/s"))]
    pub max_fall_speed: f32,
    /// Upward speed given by [`jump`](Self::jump).
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m/s"))]
    pub jump_speed: f32,
    /// Steepest walkable slope; steeper surfaces are walls.
    #[reflect(@Range { min: 0.0, max: 1.5 }, @Unit("rad"))]
    pub max_slope: f32,
    /// How far below its feet the character looks for ground to snap onto.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"))]
    pub ground_snap: f32,
    /// Gap kept between the capsule and surfaces.
    #[reflect(@Range { min: 0.001, max: 0.2 }, @Unit("m"))]
    pub skin: f32,
    /// Highest step (stair, kerb) climbed without jumping. While grounded
    /// the character also follows steps down this far instead of falling.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("m"))]
    pub step_height: f32,
    /// Mass used to push dynamic rigid bodies it walks into (heavier bodies
    /// move less); 0 pushes nothing. Needs the `dynamics` feature.
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("kg"))]
    pub mass: f32,
    /// Current velocity. Write the horizontal part every step; the vertical
    /// part is gravity and jumps.
    #[reflect(ignore)]
    pub velocity: Vec3,
    /// Standing on a walkable floor after the last step.
    #[reflect(ignore)]
    pub grounded: bool,
    /// The floor's normal while grounded, else zero.
    #[reflect(ignore)]
    pub ground_normal: Vec3,
    /// The entity the character stands on while grounded. When it moves (a
    /// platform, a kinematic body), the character moves with it.
    #[reflect(ignore)]
    pub ground_entity: Option<Entity>,
    /// Where `ground_entity`'s collider was after the last step.
    #[reflect(ignore)]
    pub ground_pose: Option<Pose>,
}

impl Default for KinematicCharacter {
    fn default() -> Self {
        Self {
            radius: 0.4,
            height: 1.8,
            gravity: 20.0,
            max_fall_speed: 40.0,
            jump_speed: 7.5,
            // Floors face up at least 0.6 (about 53 degrees).
            max_slope: 0.6_f32.acos(),
            ground_snap: 0.1,
            skin: 0.02,
            step_height: 0.3,
            mass: 80.0,
            velocity: Vec3::ZERO,
            grounded: false,
            ground_normal: Vec3::ZERO,
            ground_entity: None,
            ground_pose: None,
        }
    }
}

impl KinematicCharacter {
    /// The capsule used for movement queries.
    pub fn shape(&self) -> QueryShape {
        let radius = self.radius.max(0.01);
        QueryShape::capsule(radius, (self.height * 0.5 - radius).max(0.0))
    }

    /// Distance from the entity origin (capsule centre) to the feet.
    pub fn half_height(&self) -> f32 {
        self.height.max(self.radius * 2.0) * 0.5
    }

    /// Start a jump if standing on the ground. True if it jumped.
    pub fn jump(&mut self) -> bool {
        if self.grounded {
            self.velocity.y = self.jump_speed;
        }
        self.grounded
    }

    /// Slide options for this character, excluding `entity`'s own collider.
    pub fn slide_options(&self, entity: Entity) -> SlideOptions {
        SlideOptions {
            skin: self.skin,
            walkable_normal_y: self.max_slope.cos(),
            ground_snap: self.ground_snap,
            step_height: self.step_height,
            on_ground: self.grounded,
            filter: QueryFilter::new().excluding(entity),
            ..SlideOptions::default()
        }
    }

    /// One fixed step of `dt` seconds from `position`: carried along by
    /// the ground if it moved, gravity (or the jump started with
    /// [`jump`](Self::jump)), then [`Physics::move_and_slide`] by
    /// `velocity * dt`. Updates the velocity, `grounded` and the ground;
    /// returns the new position.
    pub fn step(&mut self, physics: &Physics, entity: Entity, position: Vec3, dt: f32) -> Vec3 {
        self.advance(physics, entity, position, dt).position
    }

    /// [`step`](Self::step), returning the whole slide result (the
    /// surfaces hit, for pushing bodies).
    fn advance(
        &mut self,
        physics: &Physics,
        entity: Entity,
        mut position: Vec3,
        dt: f32,
    ) -> SlideResult {
        if let (Some(ground), Some(old)) = (self.ground_entity, self.ground_pose)
            && let Some(new) = physics.collider_pose(ground)
            && new != old
        {
            let carried = new.translation
                + new.rotation * (old.rotation.inverse() * (position - old.translation));
            if carried.is_finite() {
                position = carried;
            }
        }
        if !self.grounded {
            self.velocity.y = (self.velocity.y - self.gravity * dt).max(-self.max_fall_speed);
        } else if self.velocity.y < 0.0 {
            self.velocity.y = 0.0;
        }
        let result = physics.move_and_slide(
            &self.shape(),
            position,
            self.velocity * dt,
            &self.slide_options(entity),
        );
        if result.hit_ceiling && self.velocity.y > 0.0 {
            self.velocity.y = 0.0;
        }
        self.grounded = result.grounded;
        self.ground_normal = result.ground_normal;
        self.ground_entity = result.ground_entity;
        self.ground_pose = result.ground_entity.and_then(|g| physics.collider_pose(g));
        if self.grounded {
            self.velocity.y = 0.0;
        }
        result
    }
}

/// Resource marking that [`move_characters`] is installed.
struct CharactersMoved;

/// Moves [`KinematicCharacter`]s: adds [`move_characters`] to
/// `FixedUpdate`. Not part of [`PhysicsPlugin`](crate::PhysicsPlugin);
/// box_engine's apps add it (so data projects and scripts have characters
/// without Rust), and adding it again is harmless. Order input systems
/// before it (`app.system(Stage::FixedUpdate, input).before("move_characters")`);
/// box_script's `ScriptPlugin` orders `on_fixed_update` before it too, so
/// script input moves the character in the same fixed step.
pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        if !app.world.contains_resource::<CharactersMoved>() {
            app.world.insert_resource(CharactersMoved);
            app.add_system(Stage::FixedUpdate, move_characters);
        }
    }
}

/// One fixed step for every [`KinematicCharacter`] ([`KinematicCharacter::step`]).
/// With the `dynamics` feature, characters then push the dynamic bodies
/// they walked into (see [`KinematicCharacter::mass`]).
pub fn move_characters(world: &mut World) {
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    let Some(physics) = world.get_resource::<Physics>() else {
        return;
    };
    #[cfg(feature = "dynamics")]
    let mut pushes: Vec<push::Push> = Vec::new();
    for (e, t, c) in world
        .query::<(Entity, &mut Transform, &mut KinematicCharacter)>()
        .iter()
    {
        let result = c.advance(physics, e, t.translation, dt);
        t.translation = result.position;
        #[cfg(feature = "dynamics")]
        push::collect(c, &result.hits, &mut pushes);
    }
    #[cfg(feature = "dynamics")]
    push::apply(world, &pushes);
}

/// Characters pushing dynamic bodies.
#[cfg(feature = "dynamics")]
mod push {
    use super::*;
    use crate::dynamics::{BodyKind, Dynamics, RigidBody};

    pub(super) struct Push {
        body: Entity,
        point: Vec3,
        /// Horizontal push direction (into the body).
        dir: Vec3,
        speed: f32,
        mass: f32,
    }

    /// One push per body hit while sliding with a horizontal velocity.
    pub(super) fn collect(c: &KinematicCharacter, hits: &[ShapeHit], out: &mut Vec<Push>) {
        let mass = if c.mass.is_finite() { c.mass } else { 0.0 };
        let horizontal = Vec3::new(c.velocity.x, 0.0, c.velocity.z);
        if mass <= 0.0 || horizontal == Vec3::ZERO || !horizontal.is_finite() {
            return;
        }
        let first = out.len();
        for hit in hits {
            let dir = Vec3::new(-hit.normal.x, 0.0, -hit.normal.z).normalize_or_zero();
            let speed = horizontal.dot(dir);
            if speed <= 0.0 || out[first..].iter().any(|p| p.body == hit.entity) {
                continue;
            }
            out.push(Push {
                body: hit.entity,
                point: hit.point,
                dir,
                speed,
                mass,
            });
        }
    }

    /// Queue an impulse on each pushed dynamic body: the momentum an
    /// inelastic hit between the character's mass and the body's would
    /// give it, enough to bring the body up to the character's speed along
    /// the push. Applied at the body's centre height, so a push slides a
    /// body rather than tipping it, and off-centre pushes still turn it.
    pub(super) fn apply(world: &mut World, pushes: &[Push]) {
        if pushes.is_empty() {
            return;
        }
        let Some(dynamics) = world.get_resource::<Dynamics>() else {
            return;
        };
        let mut impulses = Vec::new();
        for p in pushes {
            let Some(body) = world.get::<RigidBody>(p.body) else {
                continue;
            };
            if body.kind != BodyKind::Dynamic {
                continue;
            }
            let body_mass = body.mass.max(1e-3);
            let current = dynamics
                .velocity(p.body)
                .map_or(0.0, |v| v.linear.dot(p.dir));
            let dv = p.speed - current;
            if dv <= 0.0 {
                continue;
            }
            let effective = p.mass * body_mass / (p.mass + body_mass);
            let centre_y = world
                .get_resource::<Physics>()
                .and_then(|ph| ph.collider_pose(p.body))
                .map_or(p.point.y, |pose| pose.translation.y);
            let point = Vec3::new(p.point.x, centre_y, p.point.z);
            impulses.push((p.body, p.dir * effective * dv, point));
        }
        if let Some(physics) = world.get_resource_mut::<Physics>() {
            for (body, impulse, point) in impulses {
                physics.apply_impulse_at_point(body, impulse, point);
            }
        }
    }
}

/// Warns once if characters exist but nothing moves them. Called by
/// [`sync_physics`](crate::sync_physics).
pub(crate) fn warn_unmoved_characters(world: &World) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if WARNED.load(Ordering::Relaxed)
        || world.contains_resource::<CharactersMoved>()
        || world.first_with::<KinematicCharacter>().is_none()
    {
        return;
    }
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "entities have a KinematicCharacter but nothing moves them; add \
             app.add_plugin(&box_physics::CharacterPlugin) in your game plugin's build (warned once)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Collider, PhysicsPlugin};

    const DT: f32 = 1.0 / 60.0;
    const SKIN: f32 = 0.02;
    /// Capsule centre height standing on a floor whose top is at y = 0.
    const STAND: f32 = 0.9 + SKIN;

    fn solid(app: &mut App, at: Vec3, size: Vec3, rotation: Quat) {
        app.world.spawn((
            Transform::from_translation(at).with_rotation(rotation),
            Collider::cuboid(size),
        ));
    }

    /// The collector arena's shapes: four 12 m floor tiles meeting in seams
    /// at x = 0 and z = 0, walls with inner faces at ±12, and a 15 degree
    /// ramp (x 3.5..9.47) up onto a 1.6 m ledge (x 9.47..13.47) around z = -4.
    fn arena() -> App {
        let mut app = App::new();
        app.add_plugin(&PhysicsPlugin);
        app.add_plugin(&CharacterPlugin);
        for (x, z) in [(-6.0, -6.0), (6.0, -6.0), (-6.0, 6.0), (6.0, 6.0)] {
            solid(
                &mut app,
                Vec3::new(x, -0.25, z),
                Vec3::new(12.0, 0.5, 12.0),
                Quat::IDENTITY,
            );
        }
        for (at, size) in [
            (Vec3::new(0.0, 0.8, -12.25), Vec3::new(25.0, 1.6, 0.5)),
            (Vec3::new(0.0, 0.8, 12.25), Vec3::new(25.0, 1.6, 0.5)),
            (Vec3::new(12.25, 0.8, 0.0), Vec3::new(0.5, 1.6, 25.0)),
            (Vec3::new(-12.25, 0.8, 0.0), Vec3::new(0.5, 1.6, 25.0)),
        ] {
            solid(&mut app, at, size, Quat::IDENTITY);
        }
        app.update();
        app
    }

    fn ramp(app: &mut App) {
        solid(
            app,
            Vec3::new(6.524464, 0.655111, -4.0),
            Vec3::new(6.2019, 0.3, 3.0),
            Quat::from_xyzw(0.0, 0.0, 0.130526, 0.991445),
        );
        solid(
            app,
            Vec3::new(11.471281, 0.8, -4.0),
            Vec3::new(4.0, 1.6, 4.0),
            Quat::IDENTITY,
        );
        app.update();
    }

    fn character(app: &mut App, at: Vec3) -> Entity {
        let e = app.world.spawn((
            Transform::from_translation(at),
            KinematicCharacter::default(),
        ));
        app.run_frames(10, DT);
        e
    }

    fn pos(app: &App, e: Entity) -> Vec3 {
        app.world.get::<Transform>(e).unwrap().translation
    }

    fn state(app: &App, e: Entity) -> KinematicCharacter {
        KinematicCharacter::clone(&app.world.get::<KinematicCharacter>(e).unwrap())
    }

    /// Walk with horizontal velocity `v` for `frames` fixed steps.
    fn walk(app: &mut App, e: Entity, v: Vec3, frames: usize) {
        for _ in 0..frames {
            if let Some(c) = app.world.component_mut::<KinematicCharacter>(e) {
                c.velocity.x = v.x;
                c.velocity.z = v.z;
            }
            app.run_frames(1, DT);
        }
        if let Some(c) = app.world.component_mut::<KinematicCharacter>(e) {
            c.velocity.x = 0.0;
            c.velocity.z = 0.0;
        }
    }

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn walks_across_floor_seams_without_snagging() {
        let mut app = arena();
        // Along the x = 0 seam, across the z = 0 seam.
        let e = character(&mut app, Vec3::new(0.0, STAND + 0.05, 3.0));
        let start = pos(&app, e);
        assert!(state(&app, e).grounded, "{start}");
        assert!(close(start.y, STAND, 0.01), "{start}");
        walk(&mut app, e, Vec3::new(0.0, 0.0, -6.0), 60);
        let end = pos(&app, e);
        assert!(close(start.z - end.z, 6.0, 0.05), "{start} -> {end}");
        assert!(
            close(end.x, 0.0, 0.01) && close(end.y, STAND, 0.01),
            "{end}"
        );
        assert!(state(&app, e).grounded);
    }

    #[test]
    fn walls_stop_the_character_even_at_absurd_speed() {
        let limit = 12.0 - 0.4;
        for speed in [6.0, 300.0] {
            let mut app = arena();
            let e = character(&mut app, Vec3::new(9.0, STAND, -1.0));
            walk(&mut app, e, Vec3::new(speed, 0.0, 0.0), 60);
            let end = pos(&app, e);
            assert!(end.x <= limit, "speed {speed}: through the wall at {end}");
            assert!(end.x > limit - 0.1, "speed {speed}: stopped short at {end}");
            assert!(close(end.y, STAND, 0.01), "speed {speed}: {end}");
        }
    }

    #[test]
    fn slides_along_a_wall_hit_at_an_angle() {
        let mut app = arena();
        let e = character(&mut app, Vec3::new(10.0, STAND, 0.0));
        // Diagonally into the east wall: keeps the motion along it.
        walk(&mut app, e, Vec3::new(4.0, 0.0, -4.0), 30);
        let end = pos(&app, e);
        assert!(end.x <= 11.6 && end.x > 11.5, "{end}");
        assert!(close(end.z, -2.0, 0.05), "slid along the wall: {end}");
    }

    #[test]
    fn walks_up_the_ramp_onto_the_ledge() {
        let mut app = arena();
        ramp(&mut app);
        let e = character(&mut app, Vec3::new(1.5, STAND, -4.0));
        walk(&mut app, e, Vec3::new(6.0, 0.0, 0.0), 100);
        app.run_frames(5, DT);
        let end = pos(&app, e);
        assert!(end.x > 9.8 && end.x < 12.0, "{end}");
        assert!(close(end.y, 1.6 + STAND, 0.02), "{end}");
        assert!(close(end.z, -4.0, 0.05), "{end}");
        assert!(state(&app, e).grounded);
    }

    #[test]
    fn falls_lands_and_jumps() {
        let mut app = arena();
        let e = character(&mut app, Vec3::new(2.0, 6.0, 2.0));
        assert!(!state(&app, e).grounded);
        app.run_frames(90, DT);
        let end = pos(&app, e);
        assert!(close(end.y, STAND, 0.01), "{end}");
        assert!(close(end.x, 2.0, 1e-4) && close(end.z, 2.0, 1e-4), "{end}");
        let s = state(&app, e);
        assert!(s.grounded && s.velocity == Vec3::ZERO, "{s:?}");
        assert!(close(s.ground_normal.y, 1.0, 1e-4));

        // v^2 / 2g = 7.5^2 / 40 = 1.4 m.
        assert!(
            app.world
                .component_mut::<KinematicCharacter>(e)
                .unwrap()
                .jump()
        );
        let mut top: f32 = 0.0;
        for _ in 0..90 {
            app.run_frames(1, DT);
            top = top.max(pos(&app, e).y);
        }
        assert!(close(top - STAND, 1.4, 0.1), "apex {top}");
        assert!(close(pos(&app, e).y, STAND, 0.01));
        // No jump in the air.
        let c = app.world.component_mut::<KinematicCharacter>(e).unwrap();
        c.grounded = false;
        assert!(!c.jump());
    }

    #[test]
    fn ceilings_stop_a_jump() {
        let mut app = arena();
        solid(
            &mut app,
            Vec3::new(-5.0, 2.5, -5.0),
            Vec3::new(4.0, 0.2, 4.0),
            Quat::IDENTITY,
        );
        app.update();
        let e = character(&mut app, Vec3::new(-5.0, STAND, -5.0));
        app.world
            .component_mut::<KinematicCharacter>(e)
            .unwrap()
            .jump();
        let mut top: f32 = 0.0;
        for _ in 0..60 {
            app.run_frames(1, DT);
            top = top.max(pos(&app, e).y);
        }
        // Head (0.9 above the centre) stops under the slab bottom at 2.4.
        assert!(top <= 2.4 - 0.9 + 1e-3, "{top}");
        assert!(top > 2.4 - 0.9 - 0.05, "{top}");
    }

    #[test]
    fn move_and_slide_reports_hits_and_ground() {
        let app = arena();
        let physics = app.world.resource::<Physics>();
        let shape = KinematicCharacter::default().shape();
        let opts = SlideOptions::default();
        let r = physics.move_and_slide(
            &shape,
            Vec3::new(0.0, STAND, 0.0),
            Vec3::new(0.0, 0.0, 30.0),
            &opts,
        );
        assert!(r.grounded && close(r.ground_normal.y, 1.0, 1e-4));
        assert!(close(r.position.z, 12.0 - 0.4, 0.05), "{}", r.position);
        assert!(
            r.hits.iter().any(|h| close(h.normal.z, -1.0, 1e-3)),
            "{:?}",
            r.hits
        );
        // In the air: no ground; down onto the floor.
        let r = physics.move_and_slide(&shape, Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO, &opts);
        assert!(!r.grounded && r.position.y == 5.0);
        let r = physics.move_and_slide(
            &shape,
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -10.0, 0.0),
            &opts,
        );
        assert!(
            r.grounded && close(r.position.y, STAND, 0.01),
            "{}",
            r.position
        );
        // Bad input does not move or panic.
        let r = physics.move_and_slide(&shape, Vec3::new(0.0, 5.0, 0.0), Vec3::NAN, &opts);
        assert_eq!(r.position.x, 0.0);
    }
}
