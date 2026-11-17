//! `physics`: spatial queries against colliders, `move_and_slide`, and
//! with the `dynamics` feature forces, impulses and body reads; `character`:
//! kinematic characters (`KinematicCharacter`). Trigger contacts reach
//! scripts as the `on_trigger_enter` / `on_trigger_exit` callbacks, and
//! collisions of rigid bodies and characters as `on_collision_enter` /
//! `on_collision_exit` (see the runtime).

use box_core::math::Ray;
use box_core::transform::Transform;
use box_core::world::{Entity, World};
use box_physics::{
    KinematicCharacter, Physics, Pose, QueryFilter, QueryShape, RayHit, ShapeHit, SlideOptions,
};
use mlua::{Lua, Result as LuaResult, Table, Value, Vector};

use super::{ScriptApi, with_world, with_world_mut};
use crate::convert::{check_entity, entity_value, from_vector, to_quat, vector};

pub(crate) struct PhysicsApi;
pub(crate) struct CharacterApi;

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

/// `{ exclude = Entity, mask = number, triggers = boolean }`.
fn filter(opts: &Option<Table>) -> LuaResult<QueryFilter> {
    let mut f = QueryFilter::new();
    if let Some(t) = opts {
        let exclude: Value = t.get("exclude")?;
        if !exclude.is_nil() {
            f = f.excluding(check_entity(&exclude, "physics filter exclude")?);
        }
        if let Some(mask) = t.get::<Option<u32>>("mask")? {
            f = f.with_mask(mask);
        }
        if t.get::<Option<bool>>("triggers")?.unwrap_or(false) {
            f = f.with_triggers();
        }
    }
    Ok(f)
}

fn ray_hit(lua: &Lua, h: &RayHit) -> LuaResult<Table> {
    let t = lua.create_table_with_capacity(0, 4)?;
    t.raw_set("entity", entity_value(h.entity))?;
    t.raw_set("distance", h.toi)?;
    t.raw_set("point", vector(h.point))?;
    t.raw_set("normal", vector(h.normal))?;
    Ok(t)
}

fn shape_hit(lua: &Lua, h: &ShapeHit) -> LuaResult<Table> {
    let t = lua.create_table_with_capacity(0, 5)?;
    t.raw_set("entity", entity_value(h.entity))?;
    t.raw_set("distance", h.toi)?;
    t.raw_set("point", vector(h.point))?;
    t.raw_set("normal", vector(h.normal))?;
    t.raw_set("penetrating", h.penetrating)?;
    Ok(t)
}

fn physics<R>(lua: &Lua, f: impl FnOnce(&Physics) -> R) -> LuaResult<Option<R>> {
    with_world(lua, |w| w.get_resource::<Physics>().map(f))
}

fn entities(lua: &Lua, list: Vec<box_core::world::Entity>) -> LuaResult<Table> {
    let t = lua.create_table_with_capacity(list.len(), 0)?;
    for (i, e) in list.into_iter().enumerate() {
        t.raw_set(i + 1, entity_value(e))?;
    }
    Ok(t)
}

impl ScriptApi for PhysicsApi {
    fn name(&self) -> &str {
        "physics"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "raycast",
            lua.create_function(
                |lua, (origin, dir, max, opts): (Vector, Vector, Option<f32>, Option<Table>)| {
                    let f = filter(&opts)?;
                    let ray = Ray::new(from_vector(origin), from_vector(dir));
                    let hit =
                        physics(lua, |p| p.raycast(&ray, max.unwrap_or(1000.0), &f))?.flatten();
                    hit.map(|h| ray_hit(lua, &h)).transpose()
                },
            )?,
        )?;
        t.set(
            "raycast_all",
            lua.create_function(
                |lua, (origin, dir, max, opts): (Vector, Vector, Option<f32>, Option<Table>)| {
                    let f = filter(&opts)?;
                    let ray = Ray::new(from_vector(origin), from_vector(dir));
                    let hits = physics(lua, |p| p.raycast_all(&ray, max.unwrap_or(1000.0), &f))?
                        .unwrap_or_default();
                    let out = lua.create_table_with_capacity(hits.len(), 0)?;
                    for (i, h) in hits.iter().enumerate() {
                        out.raw_set(i + 1, ray_hit(lua, h)?)?;
                    }
                    Ok(out)
                },
            )?,
        )?;
        t.set(
            "spherecast",
            lua.create_function(
                |lua,
                 (origin, radius, dir, max, opts): (
                    Vector,
                    f32,
                    Vector,
                    Option<f32>,
                    Option<Table>,
                )| {
                    let f = filter(&opts)?;
                    let hit = physics(lua, |p| {
                        p.shapecast(
                            &QueryShape::sphere(radius),
                            from_vector(origin),
                            from_vector(dir).normalize_or_zero(),
                            max.unwrap_or(1000.0),
                            &f,
                        )
                    })?
                    .flatten();
                    hit.map(|h| shape_hit(lua, &h)).transpose()
                },
            )?,
        )?;
        t.set(
            "overlap_sphere",
            lua.create_function(
                |lua, (center, radius, opts): (Vector, f32, Option<Table>)| {
                    let f = filter(&opts)?;
                    let found = physics(lua, |p| {
                        p.overlap(&QueryShape::sphere(radius), from_vector(center), &f)
                    })?
                    .unwrap_or_default();
                    entities(lua, found)
                },
            )?,
        )?;
        t.set(
            "overlap_box",
            lua.create_function(
                |lua, (center, size, rotation, opts): (Vector, Vector, Value, Option<Table>)| {
                    let f = filter(&opts)?;
                    let rot = if rotation.is_nil() {
                        glam::Quat::IDENTITY
                    } else {
                        to_quat(&rotation, glam::Quat::IDENTITY, "overlap_box rotation")?
                    };
                    let pose = Pose::new(from_vector(center), rot);
                    let found = physics(lua, |p| {
                        p.overlap(&QueryShape::cuboid(from_vector(size)), pose, &f)
                    })?
                    .unwrap_or_default();
                    entities(lua, found)
                },
            )?,
        )?;
        t.set(
            "contains_point",
            lua.create_function(|lua, (point, opts): (Vector, Option<Table>)| {
                let f = filter(&opts)?;
                let found =
                    physics(lua, |p| p.contains_point(from_vector(point), &f))?.unwrap_or_default();
                entities(lua, found)
            })?,
        )?;
        t.set(
            "move_and_slide",
            lua.create_function(
                |lua, (position, motion, opts): (Vector, Vector, Option<Table>)| {
                    move_and_slide(lua, position, motion, opts)
                },
            )?,
        )?;
        #[cfg(feature = "dynamics")]
        dynamics::install(lua, &t)?;
        g.set("physics", t)
    }

    fn definitions(&self) -> String {
        let mut out = String::from(
            r#"--- Which colliders a query sees. By default every non-trigger collider on any layer.
type QueryOptions = {
    --- An entity to ignore (usually the one asking, so a ray from inside its own collider does not hit it).
    exclude: Entity?,
    --- Collision layers to test, one bit per layer (`Collider.layers`); default all.
    mask: number?,
    --- Include trigger colliders (default false).
    triggers: boolean?,
}
--- A ray hit.
type RayHit = {
    --- The entity whose collider was hit.
    entity: Entity,
    --- Distance from the ray's origin in metres.
    distance: number,
    --- The hit point in world space.
    point: vector,
    --- The surface normal at the hit point.
    normal: vector,
}
--- A shape cast hit.
type ShapeHit = {
    --- The entity whose collider was hit.
    entity: Entity,
    --- How far the shape travelled before touching, in metres.
    distance: number,
    --- The contact point in world space.
    point: vector,
    --- The surface normal at the contact (when `penetrating`, the direction to push out along).
    normal: vector,
    --- The shape already overlapped the collider where it started (`distance` is 0).
    penetrating: boolean,
}
--- Options of `physics.move_and_slide`: the capsule's size, what blocks it and how it walks.
type SlideOptions = {
    --- Capsule radius in metres (default 0.4).
    radius: number?,
    --- Capsule total height in metres (default 1.8).
    height: number?,
    --- An entity to ignore (the moving one's own collider).
    exclude: Entity?,
    --- Collision layers that block (`Collider.layers` bits); default all.
    mask: number?,
    --- Gap kept between the capsule and what it touches, in metres (default 0.02).
    skin: number?,
    --- Highest step climbed without jumping when `on_ground`, in metres (default 0).
    step_height: number?,
    --- After moving, look this far down for a floor and snap onto it, in metres (default 0.1).
    ground_snap: number?,
    --- The steepest walkable slope, in radians (default about 0.93, 53 degrees); steeper surfaces are walls.
    max_slope: number?,
    --- The capsule stood on the ground before this move; enables `step_height`.
    on_ground: boolean?,
}
--- Where `physics.move_and_slide` left the capsule.
type SlideResult = {
    --- The new position (the capsule's centre).
    position: vector,
    --- A walkable floor is under the capsule after the move.
    grounded: boolean,
    --- The floor's normal when `grounded`, else zero.
    ground_normal: vector,
    --- The entity whose collider is the floor, when `grounded`.
    ground_entity: Entity?,
    --- Moving up, the capsule hit a ceiling.
    hit_ceiling: boolean,
    --- Every surface hit while sliding, in order.
    hits: { ShapeHit },
}

--- Spatial queries against colliders, kinematic movement, and forces on rigid bodies.
declare physics: {
    --- The nearest collider hit by a ray from `origin` along `direction` (normalized for you) within `max_distance` metres (default 1000), or nil.
    raycast: (origin: vector, direction: vector, max_distance: number?, options: QueryOptions?) -> RayHit?,
    --- Every collider hit by the ray, nearest first (one hit per collider).
    raycast_all: (origin: vector, direction: vector, max_distance: number?, options: QueryOptions?) -> { RayHit },
    --- Sweeps a sphere of `radius` from `origin` along `direction` and returns the first collider it touches within `max_distance` (default 1000), or nil.
    spherecast: (origin: vector, radius: number, direction: vector, max_distance: number?, options: QueryOptions?) -> ShapeHit?,
    --- Every collider overlapping a sphere.
    overlap_sphere: (center: vector, radius: number, options: QueryOptions?) -> { Entity },
    --- Every collider overlapping a box with edge lengths `size`, centred at `center` and turned by `rotation`.
    overlap_box: (center: vector, size: vector, rotation: Quat?, options: QueryOptions?) -> { Entity },
    --- Every collider containing `point` (solid shapes: a point inside a box counts).
    contains_point: (point: vector, options: QueryOptions?) -> { Entity },
    --- Moves a capsule from `position` by `motion`, sliding along walls, climbing slopes and steps, and snapping to the ground; returns where it ended. For your own character controllers (a `KinematicCharacter` already does this every fixed step).
    move_and_slide: (position: vector, motion: vector, options: SlideOptions?) -> SlideResult,
"#,
        );
        #[cfg(feature = "dynamics")]
        out.push_str(dynamics::DEFINITIONS);
        out.push_str("}\n");
        out
    }

    fn doc(&self) -> &str {
        r#"Physics queries see `Collider` components as of the last physics sync (the end of the previous frame), so a collider spawned this frame is found from the next one. Exclude the asking entity with `{ exclude = self.entity }`. Contacts reach scripts as callbacks: `on_trigger_enter` / `on_trigger_exit` for trigger colliders and `on_collision_enter` / `on_collision_exit` for rigid bodies and characters (see [Script modules](script-modules.md)).

Forces, impulses and the body reads (`apply_impulse` to `joint_position`) need the engine's `dynamics` feature, which the prebuilt editor and player have; a native game built without it does not declare them. Bodies are `RigidBody` components with a non-trigger `Collider`.

```lua
function Lift.on_start(self)
    wait(0.5)   -- queries see colliders from the next frame on
    local here = self.entity:world_position()
    local hit = physics.raycast(here, vector.create(0, -1, 0), 10, { exclude = self.entity })
    if hit then
        log.info("above", hit.entity:name(), "at", hit.distance, "m")
    end
    for _, e in physics.overlap_sphere(here, 3) do
        if e:has("RigidBody") then
            physics.apply_impulse(e, vector.create(0, 50, 0))   -- N·s: 5 m/s for 10 kg
        end
    end
end
```"#
    }
}

/// `physics.move_and_slide(position, motion, options)`: a capsule (radius
/// 0.4, height 1.8 unless given) slid from `position` by `motion`.
fn move_and_slide(
    lua: &Lua,
    position: Vector,
    motion: Vector,
    opts: Option<Table>,
) -> LuaResult<Table> {
    let mut options = SlideOptions::default();
    let (mut radius, mut height) = (0.4_f32, 1.8_f32);
    if let Some(t) = &opts {
        radius = t.get::<Option<f32>>("radius")?.unwrap_or(radius);
        height = t.get::<Option<f32>>("height")?.unwrap_or(height);
        options.filter = filter(&opts)?;
        if let Some(v) = t.get::<Option<f32>>("skin")? {
            options.skin = v;
        }
        if let Some(v) = t.get::<Option<f32>>("step_height")? {
            options.step_height = v;
        }
        if let Some(v) = t.get::<Option<f32>>("ground_snap")? {
            options.ground_snap = v;
        }
        if let Some(v) = t.get::<Option<f32>>("max_slope")? {
            options.walkable_normal_y = v.cos();
        }
        if let Some(v) = t.get::<Option<bool>>("on_ground")? {
            options.on_ground = v;
        }
    }
    let radius = radius.max(0.01);
    let shape = QueryShape::capsule(radius, (height * 0.5 - radius).max(0.0));
    let result = physics(lua, |p| {
        p.move_and_slide(&shape, from_vector(position), from_vector(motion), &options)
    })?
    .ok_or_else(|| rt("physics.move_and_slide: this world has no physics"))?;
    let t = lua.create_table_with_capacity(0, 6)?;
    t.raw_set("position", vector(result.position))?;
    t.raw_set("grounded", result.grounded)?;
    t.raw_set("ground_normal", vector(result.ground_normal))?;
    t.raw_set("ground_entity", result.ground_entity.map(entity_value))?;
    t.raw_set("hit_ceiling", result.hit_ceiling)?;
    let hits = lua.create_table_with_capacity(result.hits.len(), 0)?;
    for (i, h) in result.hits.iter().enumerate() {
        hits.raw_set(i + 1, shape_hit(lua, h)?)?;
    }
    t.raw_set("hits", hits)?;
    Ok(t)
}

/// Forces, impulses and body reads (`dynamics` feature).
#[cfg(feature = "dynamics")]
mod dynamics {
    use box_physics::Dynamics;
    use glam::Vec3;
    use mlua::{Lua, Result as LuaResult, Table, Value, Vector};

    use super::{Physics, with_world, with_world_mut};
    use crate::convert::{check_entity, from_vector, vector};

    pub(super) const DEFINITIONS: &str = r#"    --- Changes a dynamic body's momentum by `impulse` (N·s, world space) through its centre of mass, at the next physics step: a kick, a jump, an explosion. Wakes the body.
    apply_impulse: (entity: Entity, impulse: vector) -> (),
    --- Like `apply_impulse`, at the world point `point`, which also spins the body.
    apply_impulse_at_point: (entity: Entity, impulse: vector, point: vector) -> (),
    --- Changes a dynamic body's angular momentum by `impulse` (N·m·s; world axis times magnitude) at the next step.
    apply_torque_impulse: (entity: Entity, impulse: vector) -> (),
    --- Pushes a dynamic body with `force` (N, world space) during the next step only: call it every fixed step for a continuous force (thrust, wind).
    add_force: (entity: Entity, force: vector) -> (),
    --- Turns a dynamic body with `torque` (N·m) during the next step only.
    add_torque: (entity: Entity, torque: vector) -> (),
    --- A body's linear and angular velocity (m/s, rad/s) as of the last step, or nil, nil when it has none.
    velocity: (entity: Entity) -> (vector?, vector?),
    --- Whether a body is asleep (at rest and not simulated), or nil when it has no body.
    is_sleeping: (entity: Entity) -> boolean?,
    --- The angle of the entity's hinge `Joint` (radians from its rest pose) or the travel of its prismatic joint (metres), or nil.
    joint_position: (entity: Entity) -> number?,
"#;

    fn command(lua: &Lua, f: impl FnOnce(&mut Physics)) -> LuaResult<()> {
        with_world_mut(lua, |w| match w.get_resource_mut::<Physics>() {
            Some(p) => {
                f(p);
                Ok(())
            }
            None => Err(mlua::Error::runtime("this world has no physics")),
        })?
    }

    fn vec_fn(
        lua: &Lua,
        t: &Table,
        name: &'static str,
        apply: fn(&mut Physics, box_core::world::Entity, Vec3),
    ) -> LuaResult<()> {
        t.set(
            name,
            lua.create_function(move |lua, (e, v): (Value, Vector)| {
                let e = check_entity(&e, name)?;
                command(lua, |p| apply(p, e, from_vector(v)))
            })?,
        )
    }

    pub(super) fn install(lua: &Lua, t: &Table) -> LuaResult<()> {
        vec_fn(lua, t, "apply_impulse", Physics::apply_impulse)?;
        vec_fn(
            lua,
            t,
            "apply_torque_impulse",
            Physics::apply_torque_impulse,
        )?;
        vec_fn(lua, t, "add_force", Physics::add_force)?;
        vec_fn(lua, t, "add_torque", Physics::add_torque)?;
        t.set(
            "apply_impulse_at_point",
            lua.create_function(|lua, (e, v, at): (Value, Vector, Vector)| {
                let e = check_entity(&e, "apply_impulse_at_point")?;
                command(lua, |p| {
                    p.apply_impulse_at_point(e, from_vector(v), from_vector(at))
                })
            })?,
        )?;
        t.set(
            "velocity",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "velocity")?;
                let v = with_world(lua, |w| {
                    w.get_resource::<Dynamics>()
                        .and_then(|d| d.velocity(e))
                        .or_else(|| w.get::<box_physics::Velocity>(e).map(|v| *v))
                })?;
                Ok(match v {
                    Some(v) => (Some(vector(v.linear)), Some(vector(v.angular))),
                    None => (None, None),
                })
            })?,
        )?;
        t.set(
            "is_sleeping",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "is_sleeping")?;
                with_world(lua, |w| {
                    w.get_resource::<Dynamics>().and_then(|d| d.is_sleeping(e))
                })
            })?,
        )?;
        t.set(
            "joint_position",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "joint_position")?;
                with_world(lua, |w| {
                    w.get_resource::<Dynamics>()
                        .and_then(|d| d.joint_position(e))
                })
            })?,
        )?;
        Ok(())
    }
}

// ----- characters -----

fn character<R>(lua: &Lua, e: Entity, f: impl FnOnce(&KinematicCharacter) -> R) -> LuaResult<R> {
    with_world(lua, |w| {
        w.get::<KinematicCharacter>(e)
            .map(|c| f(&c))
            .ok_or_else(|| no_character(w, e))
    })?
}

fn character_mut<R>(
    lua: &Lua,
    e: Entity,
    f: impl FnOnce(&mut KinematicCharacter) -> R,
) -> LuaResult<R> {
    with_world_mut(lua, |w| match w.component_mut::<KinematicCharacter>(e) {
        Some(c) => Ok(f(c)),
        None => Err(no_character(w, e)),
    })?
}

fn no_character(w: &World, e: Entity) -> mlua::Error {
    if w.contains(e) {
        rt(
            "the entity has no KinematicCharacter (add one in the scene or with e:add(\"KinematicCharacter\"))",
        )
    } else {
        rt("the entity no longer exists")
    }
}

impl ScriptApi for CharacterApi {
    fn name(&self) -> &str {
        "character"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "move",
            lua.create_function(|lua, (e, v): (Value, Vector)| {
                let e = check_entity(&e, "character.move")?;
                let v = from_vector(v);
                character_mut(lua, e, |c| {
                    c.velocity.x = v.x;
                    c.velocity.z = v.z;
                })
            })?,
        )?;
        t.set(
            "set_velocity",
            lua.create_function(|lua, (e, v): (Value, Vector)| {
                let e = check_entity(&e, "character.set_velocity")?;
                character_mut(lua, e, |c| c.velocity = from_vector(v))
            })?,
        )?;
        t.set(
            "velocity",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "character.velocity")?;
                character(lua, e, |c| vector(c.velocity))
            })?,
        )?;
        t.set(
            "jump",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "character.jump")?;
                character_mut(lua, e, KinematicCharacter::jump)
            })?,
        )?;
        t.set(
            "is_grounded",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "character.is_grounded")?;
                character(lua, e, |c| c.grounded)
            })?,
        )?;
        t.set(
            "ground_entity",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "character.ground_entity")?;
                character(lua, e, |c| c.ground_entity.map(entity_value))
            })?,
        )?;
        t.set(
            "ground_normal",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "character.ground_normal")?;
                character(lua, e, |c| vector(c.ground_normal))
            })?,
        )?;
        t.set(
            "teleport",
            lua.create_function(|lua, (e, at): (Value, Vector)| {
                let e = check_entity(&e, "character.teleport")?;
                with_world_mut(lua, |w| {
                    match w.component_mut::<Transform>(e) {
                        Some(t) => t.translation = from_vector(at),
                        None => return Err(rt("the entity has no Transform")),
                    }
                    if let Some(c) = w.component_mut::<KinematicCharacter>(e) {
                        c.velocity = glam::Vec3::ZERO;
                        c.grounded = false;
                        c.ground_entity = None;
                        c.ground_pose = None;
                    }
                    box_core::interpolate::Interpolated::reset(w, e);
                    Ok(())
                })?
            })?,
        )?;
        g.set("character", t)
    }

    fn definitions(&self) -> String {
        r#"--- Kinematic characters: entities with a `KinematicCharacter` component, moved by the engine every fixed step.
declare character: {
    --- Sets the horizontal velocity (x and z of `velocity`, m/s); gravity keeps the vertical part. Call it every fixed step from `on_fixed_update`.
    move: (entity: Entity, velocity: vector) -> (),
    --- Sets the whole velocity, vertical part included (a launch pad, a knockback).
    set_velocity: (entity: Entity, velocity: vector) -> (),
    --- The current velocity in m/s.
    velocity: (entity: Entity) -> vector,
    --- Jumps with the character's `jump_speed` if it stands on the ground. Returns true if it jumped.
    jump: (entity: Entity) -> boolean,
    --- True while the character stands on a walkable floor (as the last step left it).
    is_grounded: (entity: Entity) -> boolean,
    --- The entity the character stands on, or nil (a moving platform carries the character).
    ground_entity: (entity: Entity) -> Entity?,
    --- The floor's normal while grounded, else zero.
    ground_normal: (entity: Entity) -> vector,
    --- Moves the character to `position` at once: stops it and resets interpolation, so it does not slide there visibly.
    teleport: (entity: Entity, position: vector) -> (),
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"Characters move with a capsule (`KinematicCharacter`: radius, height, gravity, jump speed, slope and step limits): every fixed step the engine adds gravity, slides along walls, climbs steps and snaps to the ground. Scripts set the horizontal velocity and ask for jumps from `on_fixed_update`, which runs before the move, so input applies in the same step; read where the character ended up in `on_update`. Every function fails if the entity has no `KinematicCharacter`.

```lua
function Player.on_fixed_update(self, dt)
    local move = input.axis2("move")
    character.move(self.entity, vector.create(move.x, 0, -move.y) * self.props.speed)
    if input.pressed("jump") and character.jump(self.entity) then
        events.send("jumped")
    end
end
```"#
    }
}
