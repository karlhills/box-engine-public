//! A minimal animation state machine: [`AnimationGraph`] (states, and
//! transitions with conditions on named parameters) and the
//! [`AnimationStateMachine`] component that runs one on an
//! [`AnimationPlayer`].

use crate::asset_ref::AssetRef;
use crate::assets::{AssetLookup, Assets};
use crate::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Unit};
use crate::world::{Entity, World};

use super::player::{AnimationPlayer, AnimationPlayerState};

/// One state: a clip and how to play it.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationGraphState {
    /// Unique within the graph; transitions name states by it.
    pub name: String,
    pub clip: AssetRef,
    #[reflect(@Range { min: -4.0, max: 4.0 })]
    pub speed: f32,
    pub looping: bool,
}

impl Default for AnimationGraphState {
    fn default() -> Self {
        Self {
            name: String::new(),
            clip: AssetRef::NONE,
            speed: 1.0,
            looping: true,
        }
    }
}

impl AnimationGraphState {
    /// A looping state playing `clip` at normal speed.
    pub fn new(name: impl Into<String>, clip: AssetRef) -> Self {
        Self {
            name: name.into(),
            clip,
            ..Self::default()
        }
    }

    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    pub fn once(mut self) -> Self {
        self.looping = false;
        self
    }
}

/// What a condition requires of its parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ConditionTest {
    /// A float parameter above the value.
    Greater(f32),
    /// A float parameter below the value.
    Less(f32),
    /// A bool parameter is true.
    #[default]
    IsTrue,
    /// A bool parameter is false.
    IsFalse,
    /// A trigger parameter is set; firing the transition resets it.
    Triggered,
}

/// A test of one parameter.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationCondition {
    pub parameter: String,
    pub test: ConditionTest,
}

impl AnimationCondition {
    pub fn new(parameter: impl Into<String>, test: ConditionTest) -> Self {
        Self {
            parameter: parameter.into(),
            test,
        }
    }
}

/// A move from one state to another when every condition holds.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationTransition {
    /// The state it leaves; empty for any state (except `to` itself).
    pub from: String,
    pub to: String,
    /// All must hold (none: as soon as `exit_time` allows).
    pub conditions: Vec<AnimationCondition>,
    /// Crossfade seconds.
    #[reflect(@Range { min: 0.0, max: 5.0 }, @Unit("s"))]
    pub duration: f32,
    /// The transition waits until the current clip has played this
    /// fraction of its length (1 = to its end, for one-shot clips such as a
    /// jump's landing); 0 does not wait.
    #[reflect(@Range { min: 0.0, max: 1.0 })]
    pub exit_time: f32,
}

impl AnimationTransition {
    /// A transition from `from` (empty: any state) to `to` with a
    /// crossfade of `duration` seconds and no conditions.
    pub fn new(from: impl Into<String>, to: impl Into<String>, duration: f32) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            duration,
            ..Self::default()
        }
    }

    pub fn when(mut self, parameter: impl Into<String>, test: ConditionTest) -> Self {
        self.conditions
            .push(AnimationCondition::new(parameter, test));
        self
    }

    pub fn after(mut self, exit_time: f32) -> Self {
        self.exit_time = exit_time;
        self
    }
}

/// States and transitions. The machine starts in `start` (the first state
/// when empty or unknown) and, each frame, takes the first transition (in
/// order) out of the current state whose conditions hold. Saved inline in an
/// [`AnimationStateMachine`] or as a `.animgraph.json` file (the same JSON
/// form) referenced by its `source`.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationGraph {
    pub states: Vec<AnimationGraphState>,
    pub transitions: Vec<AnimationTransition>,
    pub start: String,
}

impl AnimationGraph {
    pub fn state(&self, name: &str) -> Option<&AnimationGraphState> {
        self.states.iter().find(|s| s.name == name)
    }

    /// Problems a user should fix: duplicate or empty state names, and
    /// transitions naming states that do not exist.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, s) in self.states.iter().enumerate() {
            if s.name.is_empty() {
                out.push(format!("state {i} has no name"));
            } else if self.states[..i].iter().any(|o| o.name == s.name) {
                out.push(format!("state `{}` is defined twice", s.name));
            }
        }
        for t in &self.transitions {
            if !t.from.is_empty() && self.state(&t.from).is_none() {
                out.push(format!("transition from unknown state `{}`", t.from));
            }
            if self.state(&t.to).is_none() {
                out.push(format!("transition to unknown state `{}`", t.to));
            }
        }
        out
    }
}

/// A parameter value.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ParameterValue {
    Float(f32),
    Bool(bool),
    /// Set by game code, reset when a transition testing it fires.
    Trigger(bool),
}

impl Default for ParameterValue {
    fn default() -> Self {
        Self::Float(0.0)
    }
}

/// A named parameter of a state machine.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationParameter {
    pub name: String,
    pub value: ParameterValue,
}

/// Runs an [`AnimationGraph`] on this entity's [`AnimationPlayer`] (added
/// with defaults when missing): entering a state sets the player's clip,
/// speed and looping, crossfading by the transition's duration. Game code
/// (and scripts) steer it through parameters: [`set_float`](Self::set_float),
/// [`set_bool`](Self::set_bool), [`trigger`](Self::trigger). Saved in
/// scenes (parameters as their initial values); the current state is
/// runtime only ([`AnimationStateMachineState`]).
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationStateMachine {
    /// A `.animgraph.json` asset used instead of `graph` when set.
    pub source: AssetRef,
    /// The graph, when `source` is empty.
    pub graph: AnimationGraph,
    /// The parameters transitions test, with their initial values (floats,
    /// bools and triggers), set from code with `set_float` and friends.
    pub parameters: Vec<AnimationParameter>,
}

impl AnimationStateMachine {
    /// A machine running `graph` inline.
    pub fn new(graph: AnimationGraph) -> Self {
        Self {
            graph,
            ..Self::default()
        }
    }

    /// A machine running the graph file `source`.
    pub fn from_asset(source: AssetRef) -> Self {
        Self {
            source,
            ..Self::default()
        }
    }

    fn set(&mut self, name: &str, value: ParameterValue) {
        set_parameter(&mut self.parameters, name, value);
    }

    /// Sets (or adds) a float parameter.
    pub fn set_float(&mut self, name: &str, value: f32) {
        self.set(name, ParameterValue::Float(value));
    }

    /// Sets (or adds) a bool parameter.
    pub fn set_bool(&mut self, name: &str, value: bool) {
        self.set(name, ParameterValue::Bool(value));
    }

    /// Sets a trigger: it stays set until a transition testing it fires.
    pub fn trigger(&mut self, name: &str) {
        self.set(name, ParameterValue::Trigger(true));
    }

    pub fn parameter(&self, name: &str) -> Option<ParameterValue> {
        parameter(&self.parameters, name)
    }

    /// A float parameter (0 when missing or not a float).
    pub fn float(&self, name: &str) -> f32 {
        float(&self.parameters, name)
    }

    /// A bool or trigger parameter (false when missing or a float).
    pub fn bool(&self, name: &str) -> bool {
        boolean(&self.parameters, name)
    }
}

fn set_parameter(parameters: &mut Vec<AnimationParameter>, name: &str, value: ParameterValue) {
    match parameters.iter_mut().find(|p| p.name == name) {
        Some(p) => p.value = value,
        None => parameters.push(AnimationParameter {
            name: name.to_string(),
            value,
        }),
    }
}

fn parameter(parameters: &[AnimationParameter], name: &str) -> Option<ParameterValue> {
    parameters.iter().find(|p| p.name == name).map(|p| p.value)
}

fn float(parameters: &[AnimationParameter], name: &str) -> f32 {
    match parameter(parameters, name) {
        Some(ParameterValue::Float(v)) => v,
        _ => 0.0,
    }
}

fn boolean(parameters: &[AnimationParameter], name: &str) -> bool {
    matches!(
        parameter(parameters, name),
        Some(ParameterValue::Bool(true) | ParameterValue::Trigger(true))
    )
}

fn holds(parameters: &[AnimationParameter], c: &AnimationCondition) -> bool {
    match c.test {
        ConditionTest::Greater(v) => float(parameters, &c.parameter) > v,
        ConditionTest::Less(v) => float(parameters, &c.parameter) < v,
        ConditionTest::IsTrue | ConditionTest::Triggered => boolean(parameters, &c.parameter),
        ConditionTest::IsFalse => !boolean(parameters, &c.parameter),
    }
}

/// Runtime state of an [`AnimationStateMachine`], added by
/// [`update_state_machines`]. Not registered, so never saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationStateMachineState {
    current: String,
    /// Transitions taken since the machine started.
    transitions: u64,
    /// Problems already logged.
    warned: bool,
}

impl AnimationStateMachineState {
    /// The current state's name (empty before the first frame).
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Transitions taken so far.
    pub fn transitions(&self) -> u64 {
        self.transitions
    }
}

/// Enters each new machine's start state, then takes at most one
/// transition per machine per frame (the first, in graph order, out of the
/// current state whose conditions hold and whose exit time has passed),
/// resetting the triggers it tested. Runs in `PostUpdate` before
/// [`animate_players`](super::animate_players). Cost per frame: one pass
/// over the transitions of each machine with string compares; nothing is
/// allocated unless a transition fires.
pub fn update_state_machines(world: &mut World) {
    profiling::scope!("update_state_machines");
    if world.count::<&AnimationStateMachine>() == 0 {
        return;
    }
    let missing: Vec<Entity> = world
        .query::<crate::world::Without<(Entity, &AnimationStateMachine), &AnimationPlayer>>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in missing {
        let _ = world.insert_one(e, AnimationPlayer::default());
    }
    let new: Vec<Entity> = world
        .query::<crate::world::Without<(Entity, &AnimationStateMachine), &AnimationStateMachineState>>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in new {
        let _ = world.insert_one(e, AnimationStateMachineState::default());
    }
    let graphs = world.remove_resource::<Assets<AnimationGraph>>();
    let lookup = world.remove_resource::<AssetLookup>();
    for (_, machine, state, player, playing) in world.query_mut::<(
        Entity,
        &mut AnimationStateMachine,
        &mut AnimationStateMachineState,
        &mut AnimationPlayer,
        Option<&AnimationPlayerState>,
    )>() {
        let AnimationStateMachine {
            source,
            graph,
            parameters,
        } = machine;
        let graph = if source.is_some() {
            match lookup
                .as_ref()
                .and_then(|l| l.get::<AnimationGraph>(source))
                .and_then(|h| graphs.as_ref()?.get(h))
            {
                Some(g) => g,
                // Loading: wait.
                None => continue,
            }
        } else {
            &*graph
        };
        step(graph, parameters, state, player, playing);
    }
    if let Some(g) = graphs {
        world.insert_resource(g);
    }
    if let Some(l) = lookup {
        world.insert_resource(l);
    }
}

fn enter(player: &mut AnimationPlayer, s: &AnimationGraphState, fade: f32) {
    player.clip = s.clip.clone();
    player.speed = s.speed;
    player.looping = s.looping;
    player.crossfade = fade;
}

fn step(
    graph: &AnimationGraph,
    parameters: &mut Vec<AnimationParameter>,
    state: &mut AnimationStateMachineState,
    player: &mut AnimationPlayer,
    playing: Option<&AnimationPlayerState>,
) {
    if graph.states.is_empty() {
        return;
    }
    if !state.warned {
        state.warned = true;
        for p in graph.problems() {
            tracing::warn!(target: "box_core::animation", "animation graph: {p}");
        }
    }
    let Some(current) = graph.state(&state.current) else {
        let start = graph.state(&graph.start).unwrap_or(&graph.states[0]);
        state.current = start.name.clone();
        enter(player, start, 0.0);
        return;
    };
    let progress = playing.map_or(0.0, AnimationPlayerState::normalized_time);
    let finished = playing.is_some_and(AnimationPlayerState::finished);
    let fired = graph.transitions.iter().find(|t| {
        let from_here = if t.from.is_empty() {
            t.to != current.name
        } else {
            t.from == current.name
        };
        from_here
            && (t.exit_time <= 0.0 || finished || progress >= t.exit_time)
            && t.conditions.iter().all(|c| holds(parameters, c))
    });
    let Some(t) = fired else { return };
    let Some(next) = graph.state(&t.to) else {
        return;
    };
    for c in &t.conditions {
        if c.test == ConditionTest::Triggered {
            set_parameter(parameters, &c.parameter, ParameterValue::Trigger(false));
        }
    }
    state.current = next.name.clone();
    state.transitions += 1;
    enter(player, next, t.duration);
}
