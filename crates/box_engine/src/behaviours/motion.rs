//! Motion behaviours: [`Spinner`], [`Bobber`] and [`Mover`].
//!
//! They move the entity's own `Transform` from where it was when Play
//! started (its *origin*), so moving the entity in the editor moves the
//! whole motion. The origin and the progress are runtime state
//! ([`MotionState`]), saved in save games through the `"motion"` hook, never
//! in scene files.

use std::collections::BTreeMap;

use box_core::identity::Uid;
use box_core::interpolate::Interpolated;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::time::Time;
use box_core::transform::Transform;
use box_core::world::{Entity, World};
use glam::{Quat, Vec3};
use serde_json::{Value as Json, json};

/// Turns the entity around an axis, forever (coins, fans, windmills).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Spinner {
    /// Off: it stops where it is.
    pub enabled: bool,
    /// The axis it turns around, in its parent's axes (Y: like a coin).
    pub axis: Vec3,
    /// Turn rate; negative turns the other way.
    #[reflect(@Range { min: -50.0, max: 50.0 }, @Unit("rad/s"), @Step(0.1))]
    pub speed: f32,
}

impl Default for Spinner {
    fn default() -> Self {
        Self {
            enabled: true,
            axis: Vec3::Y,
            speed: 2.0,
        }
    }
}

/// Floats the entity up and down around where it started. Copies placed in
/// different spots bob out of step.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Bobber {
    /// Off: it stays where it is.
    pub enabled: bool,
    /// How far above and below its start it goes.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("m"), @Step(0.01))]
    pub height: f32,
    /// Time for one full up-and-down.
    #[reflect(@Range { min: 0.1, max: 60.0 }, @Unit("s"), @Step(0.1))]
    pub period: f32,
}

impl Default for Bobber {
    fn default() -> Self {
        Self {
            enabled: true,
            height: 0.15,
            period: 3.0,
        }
    }
}

/// What a [`Mover`] does at the end of its points.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum MoverMode {
    /// Back along the same points, and again (a lift, a sliding wall).
    #[default]
    PingPong,
    /// From the last point straight to the first, and round again (a loop).
    Loop,
    /// Stop at the last point (a drawbridge, a door that opens once).
    Once,
}

/// Moves the entity through a list of points, measured from where it starts,
/// and carries characters standing on it (moving platforms). Runs at the
/// fixed rate before characters move.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Mover {
    /// Off: it stops where it is (an action can switch it on).
    pub enabled: bool,
    /// The points it moves through, relative to its start position, in its
    /// parent's axes. The first is usually (0, 0, 0).
    pub points: Vec<Vec3>,
    /// Travel speed.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m/s"), @Step(0.1))]
    pub speed: f32,
    /// What it does at the end.
    pub mode: MoverMode,
    /// Pause at each end.
    #[reflect(@Range { min: 0.0, max: 60.0 }, @Unit("s"), @Step(0.1))]
    pub wait: f32,
}

impl Default for Mover {
    fn default() -> Self {
        Self {
            enabled: true,
            points: vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
            speed: 2.0,
            mode: MoverMode::PingPong,
            wait: 0.5,
        }
    }
}

impl Mover {
    /// Length of one pass through the points (with the closing segment for
    /// `Loop`).
    fn length(&self) -> f32 {
        let mut l: f32 = self.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.mode == MoverMode::Loop
            && let (Some(a), Some(b)) = (self.points.last(), self.points.first())
        {
            l += a.distance(*b);
        }
        l
    }

    /// The offset from the origin at distance `s` along one pass.
    fn offset_at(&self, s: f32) -> Vec3 {
        let mut points: Vec<Vec3> = self.points.clone();
        if self.mode == MoverMode::Loop
            && let Some(first) = points.first().copied()
        {
            points.push(first);
        }
        let Some(&first) = points.first() else {
            return Vec3::ZERO;
        };
        let mut left = s.max(0.0);
        for w in points.windows(2) {
            let d = w[0].distance(w[1]);
            if left <= d {
                return if d > 0.0 {
                    w[0].lerp(w[1], left / d)
                } else {
                    w[0]
                };
            }
            left -= d;
        }
        points.last().copied().unwrap_or(first)
    }

    /// The offset at travel distance `travelled`, which runs over one cycle
    /// (`2 * length` for `PingPong`, `length` otherwise).
    fn offset_at_travel(&self, travelled: f32) -> Vec3 {
        let len = self.length();
        match self.mode {
            MoverMode::PingPong if travelled > len => self.offset_at(2.0 * len - travelled),
            _ => self.offset_at(travelled),
        }
    }

    /// Distance of one full cycle.
    fn cycle(&self) -> f32 {
        match self.mode {
            MoverMode::PingPong => 2.0 * self.length(),
            MoverMode::Loop | MoverMode::Once => self.length(),
        }
    }
}

/// Runtime state of a moving entity: where it started and how far it is.
/// Not registered (never in scene files); save games keep it through the
/// `"motion"` hook.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionState {
    /// The position the motion is measured from.
    pub origin: Vec3,
    /// The rotation the spin is measured from.
    pub base_rotation: Quat,
    /// Spin angle so far.
    pub angle: f32,
    /// Mover: distance travelled in the current cycle.
    pub travelled: f32,
    /// Mover: seconds left to wait at an end.
    pub waiting: f32,
    /// Mover (`Once`): reached the end.
    pub finished: bool,
}

impl MotionState {
    fn at(t: &Transform) -> Self {
        Self {
            origin: t.translation,
            base_rotation: t.rotation,
            angle: 0.0,
            travelled: 0.0,
            waiting: 0.0,
            finished: false,
        }
    }

    fn to_json(self) -> Json {
        json!({
            "origin": [self.origin.x, self.origin.y, self.origin.z],
            "base_rotation": [self.base_rotation.x, self.base_rotation.y, self.base_rotation.z, self.base_rotation.w],
            "angle": self.angle,
            "travelled": self.travelled,
            "waiting": self.waiting,
            "finished": self.finished,
        })
    }

    fn from_json(j: &Json) -> Option<Self> {
        let f = |v: &Json| v.as_f64().map(|x| x as f32);
        let o = j["origin"].as_array()?;
        let r = j["base_rotation"].as_array()?;
        Some(Self {
            origin: Vec3::new(f(&o[0])?, f(&o[1])?, f(&o[2])?),
            base_rotation: Quat::from_xyzw(f(&r[0])?, f(&r[1])?, f(&r[2])?, f(&r[3])?),
            angle: f(&j["angle"]).unwrap_or(0.0),
            travelled: f(&j["travelled"]).unwrap_or(0.0),
            waiting: f(&j["waiting"]).unwrap_or(0.0),
            finished: j["finished"].as_bool().unwrap_or(false),
        })
    }
}

/// Gives every moving entity its [`MotionState`] from its current
/// `Transform`, and `Interpolated` to movers (drawn smoothly between fixed
/// steps).
pub(crate) fn ensure_motion_state(world: &mut World) {
    let fresh: Vec<(Entity, MotionState, bool)> = world
        .query::<(
            Entity,
            &Transform,
            Option<&Spinner>,
            Option<&Bobber>,
            Option<&Mover>,
        )>()
        .without::<&MotionState>()
        .iter()
        .filter(|(_, _, s, b, m)| s.is_some() || b.is_some() || m.is_some())
        .map(|(e, t, _, _, m)| (e, MotionState::at(t), m.is_some()))
        .collect();
    for (e, state, mover) in fresh {
        let _ = world.insert_one(e, state);
        if mover && !world.has::<Interpolated>(e) {
            let _ = world.insert_one(e, Interpolated::default());
        }
    }
}

/// Spinners and bobbers, every frame (`Update`): cost one transform write
/// each.
pub(crate) fn spin_and_bob(world: &mut World) {
    ensure_motion_state(world);
    let (dt, now) = world
        .get_resource::<Time>()
        .map_or((0.0, 0.0), |t| (t.delta, t.elapsed_f64));
    for (t, state, spinner, bobber) in world
        .query_mut::<(
            &mut Transform,
            &mut MotionState,
            Option<&Spinner>,
            Option<&Bobber>,
        )>()
        .without::<&Mover>()
    {
        if let Some(s) = spinner.filter(|s| s.enabled) {
            state.angle = (state.angle + s.speed * dt) % std::f32::consts::TAU;
            let axis = s.axis.try_normalize().unwrap_or(Vec3::Y);
            t.rotation = Quat::from_axis_angle(axis, state.angle) * state.base_rotation;
        }
        if let Some(b) = bobber.filter(|b| b.enabled) {
            let phase = (state.origin.x * 0.7 + state.origin.z * 1.3) as f64;
            let w = std::f64::consts::TAU / b.period.max(0.1) as f64;
            let lift = (now * w + phase).sin() as f32 * b.height;
            t.translation = state.origin + Vec3::Y * lift;
        }
    }
}

/// Movers, every fixed step before characters move, so a character standing
/// on one is carried in the same step.
pub(crate) fn move_movers(world: &mut World) {
    ensure_motion_state(world);
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    for (t, state, mover, spinner) in
        world.query_mut::<(&mut Transform, &mut MotionState, &Mover, Option<&Spinner>)>()
    {
        if mover.enabled && !state.finished {
            advance(mover, state, dt);
        }
        t.translation = state.origin + mover.offset_at_travel(state.travelled);
        if let Some(s) = spinner.filter(|s| s.enabled) {
            state.angle = (state.angle + s.speed * dt) % std::f32::consts::TAU;
            let axis = s.axis.try_normalize().unwrap_or(Vec3::Y);
            t.rotation = Quat::from_axis_angle(axis, state.angle) * state.base_rotation;
        }
    }
}

fn advance(mover: &Mover, state: &mut MotionState, dt: f32) {
    let mut left = dt;
    if state.waiting > 0.0 {
        let used = state.waiting.min(left);
        state.waiting -= used;
        left -= used;
    }
    let cycle = mover.cycle();
    if cycle <= 0.0 || mover.speed <= 0.0 {
        return;
    }
    let len = mover.length();
    let ends: &[f32] = match mover.mode {
        MoverMode::PingPong => &[len, 2.0 * len],
        MoverMode::Loop | MoverMode::Once => &[len],
    };
    while left > 0.0 && state.waiting <= 0.0 {
        let next_end = ends
            .iter()
            .copied()
            .find(|e| *e > state.travelled + 1e-6)
            .unwrap_or(cycle);
        let step = mover.speed * left;
        if state.travelled + step < next_end {
            state.travelled += step;
            return;
        }
        left -= (next_end - state.travelled) / mover.speed;
        state.travelled = next_end;
        if mover.mode == MoverMode::Once {
            state.finished = true;
            return;
        }
        if state.travelled >= cycle - 1e-6 {
            state.travelled = 0.0;
        }
        state.waiting = mover.wait;
        if state.waiting > 0.0 {
            let used = state.waiting.min(left);
            state.waiting -= used;
            left -= used;
        }
    }
}

/// The `"motion"` save hook: the state of every saved moving entity by uid.
pub(crate) fn save_motion(world: &World) -> Json {
    let mut map = BTreeMap::new();
    for (uid, state) in world.query::<(&Uid, &MotionState)>().iter() {
        map.insert(uid.to_string(), state.to_json());
    }
    json!(map)
}

/// Restores [`save_motion`]'s states onto the loaded entities.
pub(crate) fn load_motion(world: &mut World, saved: &Json) {
    let Some(map) = saved.as_object() else {
        return;
    };
    for (uid, j) in map {
        let (Some(uid), Some(state)) = (Uid::parse(uid), MotionState::from_json(j)) else {
            continue;
        };
        if let Some(e) = world.entity_by_uid(uid) {
            let _ = world.insert_one(e, state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(mover: &Mover, steps: usize, dt: f32) -> (MotionState, Vec3) {
        let mut s = MotionState::at(&Transform::default());
        for _ in 0..steps {
            advance(mover, &mut s, dt);
        }
        (s, mover.offset_at_travel(s.travelled))
    }

    #[test]
    fn ping_pong_goes_out_waits_and_comes_back() {
        let m = Mover {
            points: vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
            speed: 2.0,
            wait: 1.0,
            ..Default::default()
        };
        // 2 s out to x = 4.
        let (_, at) = run(&m, 120, 1.0 / 60.0);
        assert!((at.x - 4.0).abs() < 1e-3, "{at}");
        // Still there during the 1 s wait.
        let (_, at) = run(&m, 170, 1.0 / 60.0);
        assert!((at.x - 4.0).abs() < 1e-3, "{at}");
        // 1 s back: x = 2.
        let (_, at) = run(&m, 240, 1.0 / 60.0);
        assert!((at.x - 2.0).abs() < 0.05, "{at}");
        // Home after 2 s back, then waits there.
        let (s, at) = run(&m, 310, 1.0 / 60.0);
        assert!(at.length() < 1e-3, "{at}");
        assert!(s.waiting > 0.0);
    }

    #[test]
    fn loop_closes_and_once_stops() {
        let square = vec![
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
        ];
        let lp = Mover {
            points: square.clone(),
            speed: 1.0,
            wait: 0.0,
            mode: MoverMode::Loop,
            ..Default::default()
        };
        assert_eq!(lp.length(), 4.0);
        let (_, at) = run(&lp, 350, 0.01); // 3.5 m: halfway along the closing edge
        assert!((at - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-3, "{at}");
        let once = Mover {
            mode: MoverMode::Once,
            ..lp
        };
        let (s, at) = run(&once, 1000, 0.01);
        assert!(s.finished);
        assert!((at - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-3, "{at}");
    }

    #[test]
    fn state_round_trips_through_json() {
        let s = MotionState {
            origin: Vec3::new(1.0, 2.0, 3.0),
            base_rotation: Quat::from_rotation_y(0.5),
            angle: 1.25,
            travelled: 3.5,
            waiting: 0.25,
            finished: true,
        };
        assert_eq!(MotionState::from_json(&s.to_json()), Some(s));
    }
}
