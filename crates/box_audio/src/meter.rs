//! Per-voice level meters and the mixer's solo and mute state.
//!
//! Every voice plays on its own *strip*: a kira sub-track of its bus holding
//! the sound (or the sound's positional track) with a [`MeterEffect`]. kira
//! runs a track's effects after its sounds and child tracks are mixed (so
//! after a positional child's attenuation and panning) and before the
//! track's own volume, so the meter hears the voice as it reaches the
//! listener, and solo and mute (the strip's volume) silence it without
//! hiding its level.
//!
//! The effect runs on kira's audio thread: it adds each block into a local
//! sum and hands it over through a `try_lock` (never waiting on the main
//! thread); [`MeterShared::take`] collects it once per frame.

use std::sync::{Arc, Mutex};

use box_core::asset_ref::AssetRef;
use box_core::world::{Entity, World};
use glam::{Quat, Vec3};
use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

use crate::components::AudioBus;

/// Sums of a meter since the last [`MeterShared::take`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Accum {
    sum_l: f64,
    sum_r: f64,
    peak: f32,
    frames: u64,
}

impl Accum {
    fn add(&mut self, other: &Accum) {
        self.sum_l += other.sum_l;
        self.sum_r += other.sum_r;
        self.peak = self.peak.max(other.peak);
        self.frames += other.frames;
    }

    /// The level these sums describe, scaled by `gain` (bus and master
    /// volume, applied after the strip).
    pub(crate) fn level(&self, gain: f32) -> VoiceLevel {
        if self.frames == 0 {
            return VoiceLevel::default();
        }
        let n = self.frames as f64;
        VoiceLevel {
            left: (self.sum_l / n).sqrt() as f32 * gain,
            right: (self.sum_r / n).sqrt() as f32 * gain,
            peak: self.peak * gain,
            frames: self.frames.min(u64::from(u32::MAX)) as u32,
        }
    }
}

/// The main thread's side of a voice meter.
#[derive(Debug, Default)]
pub(crate) struct MeterShared {
    acc: Mutex<Accum>,
}

impl MeterShared {
    /// What the meter heard since the last call (blocks the audio thread
    /// never: it keeps its block for the next try).
    pub(crate) fn take(&self) -> Accum {
        match self.acc.lock() {
            Ok(mut a) => std::mem::take(&mut *a),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }
}

/// The kira effect measuring a strip.
pub(crate) struct MeterEffect {
    shared: Arc<MeterShared>,
    local: Accum,
}

impl Effect for MeterEffect {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info) {
        for f in input.iter() {
            self.local.sum_l += f64::from(f.left * f.left);
            self.local.sum_r += f64::from(f.right * f.right);
            self.local.peak = self.local.peak.max(f.left.abs()).max(f.right.abs());
        }
        self.local.frames += input.len() as u64;
        if let Ok(mut a) = self.shared.acc.try_lock() {
            a.add(&self.local);
            self.local = Accum::default();
        }
    }
}

/// Builds a [`MeterEffect`] for a strip.
pub(crate) struct MeterBuilder(pub Arc<MeterShared>);

impl EffectBuilder for MeterBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        (
            Box::new(MeterEffect {
                shared: self.0,
                local: Accum::default(),
            }),
            (),
        )
    }
}

/// How loud a voice is at the listener over the last frame: after its own
/// volume and fades, distance attenuation and panning, the bus volume and
/// the master volume, but before solo and mute (a muted voice still shows
/// what it would play; [`VoiceInfo::audible`](crate::VoiceInfo::audible)
/// says whether it does). Linear amplitude (1 = full scale).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoiceLevel {
    /// Root mean square of the left channel.
    pub left: f32,
    /// Root mean square of the right channel.
    pub right: f32,
    /// Largest absolute sample of either channel.
    pub peak: f32,
    /// Sample frames measured this frame; 0 when none arrived (paused, or a
    /// device that has not asked for audio since): the level is then the
    /// last one measured.
    pub frames: u32,
}

impl VoiceLevel {
    /// Mean of both channels' RMS.
    pub fn rms(&self) -> f32 {
        (self.left + self.right) * 0.5
    }

    /// [`rms`](Self::rms) in dBFS (-inf for silence).
    pub fn rms_db(&self) -> f32 {
        amplitude_db(self.rms())
    }

    /// [`peak`](Self::peak) in dBFS (-inf for silence).
    pub fn peak_db(&self) -> f32 {
        amplitude_db(self.peak)
    }
}

/// Linear amplitude in decibels relative to full scale (-inf for 0).
pub fn amplitude_db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amplitude.log10()
    }
}

/// Where a positional voice is relative to the listener and what that does
/// to it, computed the way kira mixes it (linear attenuation in decibels
/// from 0 dB at `min_distance` to silence at `max_distance`; panning by ear
/// direction at kira's default spatialization strength).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spatial {
    /// Metres from the listener.
    pub distance: f32,
    /// Gain from distance alone: 1 within `min_distance`, 0 beyond
    /// `max_distance`.
    pub attenuation: f32,
    /// Gain of the left and right ear from direction alone (0.25..1).
    pub left: f32,
    pub right: f32,
    /// -1 (hard left) .. 1 (hard right).
    pub pan: f32,
}

/// kira's default spatialization strength (`SpatialTrackBuilder`).
const SPATIALIZATION_STRENGTH: f32 = 0.75;
/// kira's ear distance from the head centre.
const EAR_DISTANCE: f32 = 0.1;

/// [`Spatial`] of a sound at `position` heard by a listener at
/// `listener` (position, orientation).
pub fn spatial(
    listener: (Vec3, Quat),
    position: Vec3,
    min_distance: f32,
    max_distance: f32,
) -> Spatial {
    let (lp, lr) = listener;
    let distance = (lp - position).length();
    let min = min_distance.max(0.0);
    let max = max_distance.max(min + 0.01);
    let relative = (distance.clamp(min, max) - min) / (max - min);
    let db = -60.0 * relative;
    let attenuation = if db <= -60.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    };
    let min_ear = 1.0 - SPATIALIZATION_STRENGTH;
    let left_ear = lp + lr * (Vec3::NEG_X * EAR_DISTANCE);
    let right_ear = lp + lr * (Vec3::X * EAR_DISTANCE);
    let eighth = std::f32::consts::FRAC_PI_8;
    let left_dir = lr * (Quat::from_rotation_y(-eighth) * Vec3::NEG_X);
    let right_dir = lr * (Quat::from_rotation_y(eighth) * Vec3::X);
    let lv = (left_dir.dot((position - left_ear).normalize_or_zero()) + 1.0) / 2.0;
    let rv = (right_dir.dot((position - right_ear).normalize_or_zero()) + 1.0) / 2.0;
    let left = min_ear + (1.0 - min_ear) * lv;
    let right = min_ear + (1.0 - min_ear) * rv;
    Spatial {
        distance,
        attenuation,
        left,
        right,
        pan: (right - left) / (right + left).max(1e-6),
    }
}

/// What a solo or mute applies to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MixTarget {
    /// An [`AudioSource`](crate::AudioSource) entity and every source below
    /// it in the hierarchy (a group: "the orchard").
    Entity(Entity),
    /// Every voice on a bus.
    Bus(AudioBus),
    /// Every voice playing a clip (one-shots included).
    Clip(AssetRef),
}

/// The mixer's runtime solo and mute state ([`Audio::mute`](crate::Audio::mute),
/// [`Audio::solo`](crate::Audio::solo)). Never saved: it lives in the
/// world's [`Audio`](crate::Audio) resource, so it ends with the play world
/// (editor Stop) or the game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mix {
    pub muted: Vec<MixTarget>,
    pub soloed: Vec<MixTarget>,
}

impl Mix {
    /// Adds (`on`) or removes `target` from `list`; whether it changed.
    pub(crate) fn set(list: &mut Vec<MixTarget>, target: MixTarget, on: bool) -> bool {
        let has = list.contains(&target);
        if on && !has {
            list.push(target);
            true
        } else if !on && has {
            list.retain(|t| *t != target);
            true
        } else {
            false
        }
    }

    pub fn is_empty(&self) -> bool {
        self.muted.is_empty() && self.soloed.is_empty()
    }

    /// Whether a voice matches any target of `list`.
    pub(crate) fn matches(
        list: &[MixTarget],
        world: &World,
        bus: AudioBus,
        clip: &AssetRef,
        source: Option<Entity>,
    ) -> bool {
        list.iter()
            .any(|t| t.matches_parts(world, bus, clip, source))
    }
}

impl MixTarget {
    /// Whether `voice` is one this target applies to (an entity target:
    /// the voice's source is the entity or below it).
    pub fn matches(&self, world: &World, voice: &crate::VoiceInfo) -> bool {
        self.matches_parts(world, voice.bus, &voice.clip, voice.source)
    }

    fn matches_parts(
        &self,
        world: &World,
        bus: AudioBus,
        clip: &AssetRef,
        source: Option<Entity>,
    ) -> bool {
        match self {
            MixTarget::Bus(b) => *b == bus,
            MixTarget::Clip(c) => same_clip(c, clip),
            MixTarget::Entity(e) => {
                let mut at = source;
                // A short walk up the hierarchy; bounded against cycles.
                for _ in 0..256 {
                    match at {
                        Some(x) if x == *e => return true,
                        Some(x) => at = world.parent(x),
                        None => return false,
                    }
                }
                false
            }
        }
    }
}

/// Two references to the same clip: same id, or same path.
pub(crate) fn same_clip(a: &AssetRef, b: &AssetRef) -> bool {
    (a.id.is_some() && a.id == b.id) || (a.path.is_some() && a.path == b.path)
}
