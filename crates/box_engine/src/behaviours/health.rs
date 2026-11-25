//! [`Health`]: hit points with a short invulnerability after each hit, and
//! action lists that run when it is hit and when it reaches zero.
//!
//! Damage comes from the `Damage` interaction action (a hazard, an enemy's
//! touch), from [`damage`] in Rust, or from anything that lowers `current`
//! (a script's `e:set("Health", ...)`): the `health` system notices the drop
//! and runs `on_zero` the same way.

use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::time::Time;
use box_core::visibility::Transient;
use box_core::world::{Entity, World};

use crate::interact::{InteractAction, InteractRequests, Target, find_interactor};

/// Hit points. Hits (the `Damage` action) lower `current`, then nothing can
/// hurt it for `invulnerable` seconds; `Heal` raises it up to `max`. When it
/// reaches 0, `on_zero` runs. Left empty, `on_zero` sends the player back to
/// the last checkpoint (costing a life when the rules count lives, and
/// refilling its health), and removes anything else (hides it, or despawns a
/// spawned copy). Saved: `current` is its state.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Health {
    /// Off: it cannot be hurt or healed.
    pub enabled: bool,
    /// Full health.
    #[reflect(@Range { min: 1.0, max: 10000.0 }, @Step(1.0))]
    pub max: f32,
    /// Health now (saved in save games).
    #[reflect(@Range { min: 0.0, max: 10000.0 }, @Step(1.0))]
    pub current: f32,
    /// After a hit, how long nothing can hurt it.
    #[reflect(@Range { min: 0.0, max: 30.0 }, @Unit("s"), @Step(0.05))]
    pub invulnerable: f32,
    /// What a hit does besides lowering health (a sound, a burst), in order.
    /// `Interactor` targets whoever hit it.
    pub on_damage: Vec<InteractAction>,
    /// What reaching zero does, in order. Empty: the player respawns,
    /// anything else disappears.
    pub on_zero: Vec<InteractAction>,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            enabled: true,
            max: 3.0,
            current: 3.0,
            invulnerable: 0.8,
            on_damage: Vec::new(),
            on_zero: Vec::new(),
        }
    }
}

/// Runtime state of a [`Health`] (never saved).
#[derive(Clone, Copy, Debug, Default)]
pub struct HealthState {
    /// Game time until which hits are ignored.
    pub invulnerable_until: f64,
    /// `current` as last seen, to notice changes made elsewhere.
    seen: f32,
    /// `on_zero` already ran for this time at zero.
    down: bool,
}

impl HealthState {
    /// True while hits are ignored.
    pub fn is_invulnerable(&self, now: f64) -> bool {
        now < self.invulnerable_until
    }
}

fn now(world: &World) -> f64 {
    world.get_resource::<Time>().map_or(0.0, |t| t.elapsed_f64)
}

/// Hurts `e` by `amount` from `source` (who gets `Interactor` actions).
/// Returns false when nothing happened: no enabled `Health`, already at
/// zero, invulnerable, or `amount <= 0`. Its `on_damage` actions, and
/// `on_zero` when it reaches zero, run in this frame's interaction pass.
pub fn damage(world: &mut World, e: Entity, amount: f32, source: Option<Entity>) -> bool {
    if amount <= 0.0 {
        return false;
    }
    let t = now(world);
    let state = world.get::<HealthState>(e).map(|s| *s).unwrap_or_default();
    let Some(h) = world.component_mut::<Health>(e) else {
        return false;
    };
    if !h.enabled || h.current <= 0.0 || state.is_invulnerable(t) {
        return false;
    }
    h.current = (h.current - amount).max(0.0);
    let current = h.current;
    let invulnerable = h.invulnerable;
    let on_damage = h.on_damage.clone();
    let mut next = state;
    next.invulnerable_until = t + invulnerable as f64;
    next.seen = current;
    let _ = world.insert_one(e, next);
    tracing::debug!(target: "box_engine", "damage {amount} → {current}");
    if !on_damage.is_empty() {
        world
            .resource_or_default::<InteractRequests>()
            .run_actions(e, source, on_damage);
    }
    if current <= 0.0 {
        reached_zero(world, e, source);
    }
    true
}

/// Heals `e` by `amount`, up to its `max`. Returns false without an enabled
/// `Health`, at zero (a defeated thing stays defeated), or at full health.
pub fn heal(world: &mut World, e: Entity, amount: f32) -> bool {
    let Some(h) = world.component_mut::<Health>(e) else {
        return false;
    };
    if !h.enabled || h.current <= 0.0 || h.current >= h.max || amount <= 0.0 {
        return false;
    }
    h.current = (h.current + amount).min(h.max);
    let current = h.current;
    if let Some(s) = world.component_mut::<HealthState>(e) {
        s.seen = current;
    }
    true
}

/// Back to full health with no invulnerability left (respawns, restarts).
pub fn refill(world: &mut World, e: Entity) {
    if let Some(h) = world.component_mut::<Health>(e) {
        h.current = h.max;
        let current = h.current;
        let _ = world.insert_one(
            e,
            HealthState {
                invulnerable_until: 0.0,
                seen: current,
                down: false,
            },
        );
    }
}

/// Runs `on_zero` (or the default) once for this time at zero.
fn reached_zero(world: &mut World, e: Entity, source: Option<Entity>) {
    let Some(state) = world.component_mut::<HealthState>(e) else {
        return;
    };
    if state.down {
        return;
    }
    state.down = true;
    let on_zero = world
        .get::<Health>(e)
        .map(|h| h.on_zero.clone())
        .unwrap_or_default();
    let actions = if on_zero.is_empty() {
        default_on_zero(world, e)
    } else {
        on_zero
    };
    world
        .resource_or_default::<InteractRequests>()
        .run_actions(e, source, actions);
}

/// The player respawns; a spawned copy despawns; anything else hides.
fn default_on_zero(world: &World, e: Entity) -> Vec<InteractAction> {
    if find_interactor(world) == Some(e) {
        // Respawn acts on whoever used it: here, the player itself.
        return vec![InteractAction::Respawn];
    }
    if world.has::<Transient>(e) {
        vec![InteractAction::Despawn {
            target: Target::This,
        }]
    } else {
        vec![InteractAction::Hide {
            target: Target::This,
            collide: true,
        }]
    }
}

/// The `health` system (Update, before `interact`): notices health lowered
/// or raised outside [`damage`]/[`heal`] (scripts, the inspector, a loaded
/// save) and runs `on_zero` when it reached zero. One pass over the entities
/// with `Health`; no allocation unless something changed.
pub fn track_health(world: &mut World) {
    let mut changed: Vec<(Entity, f32)> = Vec::new();
    for (e, h, s) in world
        .query::<(Entity, &Health, Option<&HealthState>)>()
        .iter()
    {
        match s {
            Some(s) if s.seen == h.current => {}
            _ => changed.push((e, h.current)),
        }
    }
    for (e, current) in changed {
        let mut state = world.get::<HealthState>(e).map(|s| *s).unwrap_or_default();
        state.seen = current;
        if current > 0.0 {
            state.down = false;
        }
        let first = !world.has::<HealthState>(e);
        let _ = world.insert_one(e, state);
        // A thing loaded at zero (a defeated enemy in a save) stays down
        // without running its actions again.
        if current <= 0.0 && !state.down {
            if first {
                if let Some(s) = world.component_mut::<HealthState>(e) {
                    s.down = true;
                }
            } else {
                reached_zero(world, e, None);
            }
        }
    }
}
