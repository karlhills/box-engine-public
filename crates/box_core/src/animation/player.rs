//! [`AnimationPlayer`]: plays clips on an entity's descendants, with
//! crossfades, and [`animate_players`], the system that samples them.

use std::collections::HashMap;

use glam::{Quat, Vec3};

use super::clip::{AnimationClip, Sample};
use crate::asset_ref::AssetRef;
use crate::assets::{AssetLookup, Assets, Handle};
use crate::identity::Name;
use crate::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Unit};
use crate::time::Time;
use crate::transform::Transform;
use crate::world::{Entity, World};

/// Plays an [`AnimationClip`] on this entity and its descendants: each
/// channel moves the entity whose `Name` matches its target (the joints of
/// an instanced glTF character). Changing [`clip`](Self::clip) crossfades
/// from the clip playing to the new one over [`crossfade`](Self::crossfade)
/// seconds. Saved in scenes; the playback state is runtime only
/// ([`AnimationPlayerState`]). An [`AnimationStateMachine`] on the same
/// entity drives these fields.
///
/// [`AnimationStateMachine`]: super::AnimationStateMachine
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AnimationPlayer {
    /// The clip to play, such as `models/hero.glb#Animation/Walk`.
    pub clip: AssetRef,
    /// Playback rate; negative plays backwards.
    #[reflect(@Range { min: -4.0, max: 4.0 })]
    pub speed: f32,
    /// Wrap around at the end; otherwise hold the last frame.
    pub looping: bool,
    /// Freeze playback (crossfades included).
    pub paused: bool,
    /// Seconds a change of `clip` blends from the previous clip (0 cuts).
    #[reflect(@Range { min: 0.0, max: 5.0 }, @Unit("s"))]
    pub crossfade: f32,
}

impl Default for AnimationPlayer {
    fn default() -> Self {
        Self {
            clip: AssetRef::NONE,
            speed: 1.0,
            looping: true,
            paused: false,
            crossfade: 0.2,
        }
    }
}

impl AnimationPlayer {
    /// A looping player of `clip`.
    pub fn new(clip: AssetRef) -> Self {
        Self {
            clip,
            ..Self::default()
        }
    }

    /// Switches to `clip`, blending from the current clip over `seconds`.
    pub fn crossfade_to(&mut self, clip: AssetRef, seconds: f32) {
        self.clip = clip;
        self.crossfade = seconds;
    }
}

/// One clip being played: the current one (last) or one fading out.
#[derive(Clone, Debug)]
struct Layer {
    clip: AssetRef,
    handle: Option<Handle<AnimationClip>>,
    /// Asset version the channel bindings were made for (0: none yet).
    bound_version: u64,
    duration: f32,
    time: f32,
    speed: f32,
    looping: bool,
    /// Seconds to fade in over (0: at once).
    fade: f32,
    /// Seconds since the layer started (unpaused).
    age: f32,
    /// Target slot of each channel (`None`: no entity with that name).
    slots: Vec<Option<u32>>,
    /// Keyframe search hint per channel.
    hints: Vec<usize>,
}

impl Layer {
    fn new(player: &AnimationPlayer, fade: f32) -> Self {
        Self {
            clip: player.clip.clone(),
            handle: None,
            bound_version: 0,
            duration: 0.0,
            time: 0.0,
            speed: player.speed,
            looping: player.looping,
            fade,
            age: 0.0,
            slots: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// How far the layer has faded in, 0..=1.
    fn fade_in(&self) -> f32 {
        if self.fade <= 0.0 {
            1.0
        } else {
            (self.age / self.fade).clamp(0.0, 1.0)
        }
    }
}

/// One playing clip as reported by [`AnimationPlayerState::layers`].
#[derive(Clone, Debug, PartialEq)]
pub struct LayerInfo {
    pub clip: AssetRef,
    /// Playback time in seconds.
    pub time: f32,
    /// Share of the final pose, 0..=1; the weights of all layers sum to 1.
    pub weight: f32,
}

/// Runtime playback state of an [`AnimationPlayer`], added by
/// [`animate_players`]. Read it to know where a clip is (a one-shot jump
/// finishing); not registered, so never saved.
#[derive(Debug, Default)]
pub struct AnimationPlayerState {
    /// Oldest first; the last is the current clip.
    layers: Vec<Layer>,
    lookup_generation: u64,
    /// Bound entities by slot.
    targets: Vec<Entity>,
    /// The player entity's children when the targets were bound.
    children: Vec<Entity>,
    /// Per-slot pose scratch.
    poses: Vec<Pose>,
    /// Some channel found no entity at the last binding.
    unbound: bool,
    /// Sampled at least once since the last change (paused players with
    /// nothing fading are then skipped).
    settled: bool,
}

impl AnimationPlayerState {
    /// The clips playing, oldest first, with their share of the pose.
    pub fn layers(&self) -> Vec<LayerInfo> {
        let fades: Vec<f32> = self.layers.iter().map(Layer::fade_in).collect();
        self.layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let base = if i == 0 { 1.0 } else { fades[i] };
                let covered: f32 = fades[i + 1..].iter().map(|w| 1.0 - w).product();
                LayerInfo {
                    clip: l.clip.clone(),
                    time: l.time,
                    weight: base * covered,
                }
            })
            .collect()
    }

    /// The current clip's playback time in seconds (0 before it starts).
    pub fn time(&self) -> f32 {
        self.layers.last().map_or(0.0, |l| l.time)
    }

    /// The current clip's time as a fraction of its duration (0 while the
    /// clip is loading).
    pub fn normalized_time(&self) -> f32 {
        match self.layers.last() {
            Some(l) if l.duration > 0.0 => l.time / l.duration,
            _ => 0.0,
        }
    }

    /// A non-looping current clip reached its end (its start, playing
    /// backwards).
    pub fn finished(&self) -> bool {
        self.layers.last().is_some_and(|l| {
            l.handle.is_some()
                && !l.looping
                && if l.speed >= 0.0 {
                    l.time >= l.duration
                } else {
                    l.time <= 0.0
                }
        })
    }

    /// The entities the clips animate (bound by name).
    pub fn targets(&self) -> &[Entity] {
        &self.targets
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Pose {
    translation: Option<Vec3>,
    rotation: Option<Quat>,
    scale: Option<Vec3>,
}

impl Pose {
    fn apply(&mut self, sample: Sample, weight: f32) {
        match sample {
            Sample::Translation(v) => {
                self.translation = Some(self.translation.map_or(v, |a| a.lerp(v, weight)))
            }
            Sample::Rotation(q) => {
                self.rotation = Some(self.rotation.map_or(q, |a| a.slerp(q, weight)))
            }
            Sample::Scale(v) => self.scale = Some(self.scale.map_or(v, |a| a.lerp(v, weight))),
        }
    }
}

/// Advances every [`AnimationPlayer`] by [`Time::delta`], crossfades, and
/// writes the blended pose into the target entities' `Transform`s (change-
/// driven transform propagation then moves them, and skinned meshes follow
/// their joints). Runs in `PostUpdate` before propagation
/// ([`AnimationPlugin`](super::AnimationPlugin)).
///
/// Cost per frame: one keyframe lookup per channel and playing layer
/// (amortised O(1) with search hints), one blend per channel and fading
/// layer, and one `Transform` write per animated entity. Targets are bound
/// by name once per clip (and again only when a clip reloads, a target is
/// despawned, or, while some channel is unbound, the player's children
/// change). Paused players with nothing fading are skipped.
pub fn animate_players(world: &mut World) {
    profiling::scope!("animate_players");
    if world.count::<&AnimationPlayer>() == 0 {
        return;
    }
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    let new: Vec<Entity> = world
        .query::<crate::world::Without<(Entity, &AnimationPlayer), &AnimationPlayerState>>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in new {
        let _ = world.insert_one(e, AnimationPlayerState::default());
    }
    let clips = world.remove_resource::<Assets<AnimationClip>>();
    let lookup = world.remove_resource::<AssetLookup>();
    let empty = Assets::new();
    let clips_ref = clips.as_ref().unwrap_or(&empty);

    // 1. Layers, handles and time.
    let mut rebind: Vec<Entity> = Vec::new();
    let mut waiting: Vec<Entity> = Vec::new();
    for (e, player, state) in
        world.query_mut::<(Entity, &AnimationPlayer, &mut AnimationPlayerState)>()
    {
        if advance(player, state, lookup.as_ref(), clips_ref, dt) {
            rebind.push(e);
        } else {
            waiting.push(e);
        }
    }
    // A target was despawned (a prefab reloaded), or some channel found no
    // target (a prefab still instancing) and the player's children changed.
    for e in waiting {
        let stale = world.get::<AnimationPlayerState>(e).is_some_and(|s| {
            s.targets.iter().any(|&t| !world.contains(t))
                || (s.unbound && s.children != world.children(e))
        });
        if stale {
            rebind.push(e);
        }
    }
    // 2. Bind channels to entities by name (rare).
    for e in rebind {
        bind(world, e, clips_ref);
    }
    // 3. Sample and blend into poses, then write transforms.
    let mut writes: Vec<(Entity, Pose)> = Vec::new();
    for (_, player, state) in
        world.query_mut::<(Entity, &AnimationPlayer, &mut AnimationPlayerState)>()
    {
        if player.paused && state.settled && state.layers.len() <= 1 {
            continue;
        }
        sample(state, clips_ref, &mut writes);
        state.settled = true;
    }
    for (e, pose) in writes {
        if let Some(t) = world.component_mut::<Transform>(e) {
            if let Some(v) = pose.translation {
                t.translation = v;
            }
            if let Some(q) = pose.rotation {
                t.rotation = q;
            }
            if let Some(v) = pose.scale {
                t.scale = v;
            }
        }
    }
    if let Some(c) = clips {
        world.insert_resource(c);
    }
    if let Some(l) = lookup {
        world.insert_resource(l);
    }
}

/// Starts a layer for a changed clip, resolves handles, advances time and
/// drops layers hidden by a fully faded-in one. True when channel bindings
/// must be rebuilt.
fn advance(
    player: &AnimationPlayer,
    state: &mut AnimationPlayerState,
    lookup: Option<&AssetLookup>,
    clips: &Assets<AnimationClip>,
    dt: f32,
) -> bool {
    let mut rebind = false;
    let current = state.layers.last().map(|l| &l.clip);
    if current != Some(&player.clip) {
        let fade = if state.layers.is_empty() {
            0.0
        } else {
            player.crossfade.max(0.0)
        };
        state.layers.push(Layer::new(player, fade));
        state.settled = false;
        rebind = true;
    }
    let generation = lookup.map_or(0, AssetLookup::generation);
    let relookup = generation != state.lookup_generation;
    state.lookup_generation = generation;
    let top = state.layers.len() - 1;
    for (i, layer) in state.layers.iter_mut().enumerate() {
        if i == top {
            layer.speed = player.speed;
            layer.looping = player.looping;
        }
        if layer.handle.is_none() || relookup {
            let handle = lookup.and_then(|l| l.get::<AnimationClip>(&layer.clip));
            if handle != layer.handle {
                layer.handle = handle;
                layer.bound_version = 0;
            }
        }
        let Some(handle) = layer.handle else {
            continue;
        };
        let version = clips.version(handle).unwrap_or(0);
        if version != layer.bound_version {
            rebind = true;
        }
        if let Some(clip) = clips.get(handle) {
            layer.duration = clip.duration;
        }
        if player.paused {
            continue;
        }
        layer.age += dt;
        layer.time += dt * layer.speed;
        if layer.looping && layer.duration > 0.0 {
            layer.time = layer.time.rem_euclid(layer.duration);
        } else {
            layer.time = layer.time.clamp(0.0, layer.duration);
        }
    }
    if let Some(k) = state.layers.iter().rposition(|l| l.fade_in() >= 1.0)
        && k > 0
    {
        state.layers.drain(..k);
    }
    rebind
}

/// Binds every layer's channels to entities named like their targets among
/// `root` and its descendants.
fn bind(world: &mut World, root: Entity, clips: &Assets<AnimationClip>) {
    let children = world.children(root);
    let mut names: HashMap<String, Entity> = HashMap::new();
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if let Some(name) = world.get::<Name>(e) {
            names.entry(name.0.clone()).or_insert(e);
        }
        stack.extend(world.children(e));
    }
    let Some(state) = world.component_mut::<AnimationPlayerState>(root) else {
        return;
    };
    state.children = children;
    state.targets.clear();
    state.unbound = false;
    let mut slot_of: HashMap<Entity, u32> = HashMap::new();
    for layer in &mut state.layers {
        layer.slots.clear();
        layer.hints.clear();
        let Some(handle) = layer.handle else {
            continue;
        };
        let Some(clip) = clips.get(handle) else {
            continue;
        };
        layer.bound_version = clips.version(handle).unwrap_or(0);
        for channel in &clip.channels {
            let slot = names.get(&channel.target).map(|&e| {
                *slot_of.entry(e).or_insert_with(|| {
                    state.targets.push(e);
                    state.targets.len() as u32 - 1
                })
            });
            state.unbound |= slot.is_none();
            layer.slots.push(slot);
            layer.hints.push(0);
        }
    }
    state.settled = false;
}

/// Samples every layer into the per-slot poses (later layers blended over
/// earlier ones by their fade) and queues the transform writes.
fn sample(
    state: &mut AnimationPlayerState,
    clips: &Assets<AnimationClip>,
    writes: &mut Vec<(Entity, Pose)>,
) {
    state.poses.clear();
    state.poses.resize(state.targets.len(), Pose::default());
    for (i, layer) in state.layers.iter_mut().enumerate() {
        let Some(clip) = layer.handle.and_then(|h| clips.get(h)) else {
            continue;
        };
        if layer.slots.len() != clip.channels.len() {
            continue;
        }
        let weight = if i == 0 { 1.0 } else { layer.fade_in() };
        for ((channel, slot), hint) in clip.channels.iter().zip(&layer.slots).zip(&mut layer.hints)
        {
            let Some(slot) = *slot else { continue };
            let value = channel.sample_with_hint(layer.time, hint);
            state.poses[slot as usize].apply(value, weight);
        }
    }
    writes.extend(
        state
            .targets
            .iter()
            .copied()
            .zip(state.poses.iter().copied()),
    );
}
