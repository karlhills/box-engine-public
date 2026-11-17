//! `particles`: particle emitters from scripts.
//!
//! ```lua
//! particles.burst(self.entity:world_position(), 24, { speed = vector.create(2, 4, 0) })
//! particles.stop(torch)        -- live particles finish
//! particles.restart(fountain)
//! ```

use box_core::identity::Name;
use box_core::reflect::bevy_reflect::PartialReflect;
use box_core::transform::Transform;
use box_core::world::{Entity, World};
use box_render::{ParticleEmitter, ParticleState};
use box_script::convert::{apply_lua, check_entity, entity_value, from_vector};
use box_script::mlua::{self, Lua, Result as LuaResult, Table, Value, Vector};
use box_script::{ScriptApi, with_world, with_world_mut};

pub struct ParticlesApi;

fn missing(w: &World, e: Entity) -> mlua::Error {
    mlua::Error::runtime(if w.contains(e) {
        "the entity has no ParticleEmitter"
    } else {
        "the entity no longer exists"
    })
}

fn emitter_mut(lua: &Lua, e: Entity, f: impl FnOnce(&mut World, Entity)) -> LuaResult<()> {
    with_world_mut(lua, |w| {
        if w.has::<ParticleEmitter>(e) {
            f(w, e);
            Ok(())
        } else {
            Err(missing(w, e))
        }
    })?
}

impl ScriptApi for ParticlesApi {
    fn name(&self) -> &str {
        "particles"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "burst",
            lua.create_function(|lua, (at, count, options): (Vector, u32, Value)| {
                with_world_mut(lua, |w| {
                    let mut emitter = ParticleEmitter::burst(count);
                    if !options.is_nil() {
                        let registry = w.registry().clone();
                        apply_lua(
                            lua,
                            &registry,
                            emitter.as_partial_reflect_mut(),
                            &options,
                            "particles.burst",
                        )?;
                        // A burst stays a burst whatever the options say.
                        emitter.max_particles = emitter.max_particles.max(count);
                    }
                    let e = w.spawn((
                        Name::new("Particle burst"),
                        Transform::from_translation(from_vector(at)),
                        emitter,
                    ));
                    Ok(entity_value(e))
                })?
            })?,
        )?;
        t.set(
            "start",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "particles.start")?;
                emitter_mut(lua, e, |w, e| {
                    if let Some(p) = w.component_mut::<ParticleEmitter>(e) {
                        p.playing = true;
                    }
                })
            })?,
        )?;
        t.set(
            "stop",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "particles.stop")?;
                emitter_mut(lua, e, |w, e| {
                    if let Some(p) = w.component_mut::<ParticleEmitter>(e) {
                        p.playing = false;
                    }
                })
            })?,
        )?;
        t.set(
            "restart",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "particles.restart")?;
                emitter_mut(lua, e, |w, e| {
                    let seed = w.component_mut::<ParticleEmitter>(e).map(|p| {
                        p.playing = true;
                        p.seed
                    });
                    if let (Some(seed), Some(state)) = (seed, w.component_mut::<ParticleState>(e)) {
                        state.restart(seed);
                    }
                })
            })?,
        )?;
        t.set(
            "count",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "particles.count")?;
                with_world(lua, |w| {
                    // A despawned one-shot burst has none left.
                    if w.contains(e) && !w.has::<ParticleEmitter>(e) {
                        return Err(missing(w, e));
                    }
                    Ok(w.get::<ParticleState>(e).map_or(0, |s| s.len()))
                })?
            })?,
        )?;
        t.set(
            "is_finished",
            lua.create_function(|lua, e: Value| {
                let e = check_entity(&e, "particles.is_finished")?;
                with_world(lua, |w| {
                    // A one-shot burst despawns when it finishes.
                    if !w.contains(e) {
                        return Ok(true);
                    }
                    if !w.has::<ParticleEmitter>(e) {
                        return Err(missing(w, e));
                    }
                    Ok(w.get::<ParticleState>(e).is_some_and(|s| s.is_finished()))
                })?
            })?,
        )?;
        g.set("particles", t)
    }

    fn definitions(&self) -> String {
        r#"--- Particle effects: one-shot bursts and control of `ParticleEmitter` entities.
declare particles: {
    --- Spawns a one-shot burst of `count` particles at `position` (world space) and returns its emitter entity, which despawns itself when the last particle dies. `options` patches the `ParticleEmitter` fields (speed, colours, size, lifetime, gravity and so on).
    burst: (position: vector, count: number, options: ParticleEmitterPatch?) -> Entity,
    --- Starts (or resumes) emitting.
    start: (emitter: Entity) -> (),
    --- Stops emitting; live particles finish their lives.
    stop: (emitter: Entity) -> (),
    --- Clears the emitter's particles and starts it again from the beginning (bursts included).
    restart: (emitter: Entity) -> (),
    --- How many particles are alive.
    count: (emitter: Entity) -> number,
    --- True when a non-looping emitter has finished emitting and every particle has died.
    is_finished: (emitter: Entity) -> boolean,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r##"Camera-facing particles simulated on the CPU. Place emitters in the editor (a `ParticleEmitter` component), or burst one from a script for hits, pickups and explosions.

```lua
function Gem.on_trigger_enter(self, other)
    particles.burst(self.entity:world_position(), 24, {
        speed = vector.create(2, 4, 0),          -- slowest and fastest (x, y)
        color = {
            { t = 0, color = Color.hex("#7fe3ff") },
            { t = 1, color = Color.rgb(1, 1, 1, 0) },
        },
    })
    self.entity:despawn()
end
```"##
    }
}
