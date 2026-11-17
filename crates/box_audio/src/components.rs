//! The game-facing audio types: [`AudioSource`], [`AudioListener`],
//! [`PlaySound`], [`AudioBus`] and the [`AudioSettings`] resource.

use box_core::asset_ref::AssetRef;
use box_core::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, ReflectResource, Unit};
use glam::Vec3;

/// The mixer bus a sound plays on. Each bus has its own volume in
/// [`AudioSettings`], under the master volume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum AudioBus {
    /// One-shot effects: pickups, footsteps, placement sounds.
    #[default]
    Effects,
    /// Background loops: wind, water, crowds.
    Ambience,
    /// Music tracks.
    Music,
}

impl AudioBus {
    /// Every bus, in mixer order.
    pub const ALL: [AudioBus; 3] = [AudioBus::Effects, AudioBus::Ambience, AudioBus::Music];

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// A sound attached to an entity: an ambience loop, a music track, or a
/// sound that plays once when the entity appears. It plays while the entity
/// exists in a running game (the player, editor Play mode; never in the edit
/// world), fades in when it starts and fades out when the entity is
/// despawned (including by a scene switch) or `playing` is turned off.
///
/// Every field can change while it plays: `volume` and `pitch` glide to the
/// new value; changing `clip`, `bus`, `looping` or `spatial` restarts the
/// sound. A clip that is hot-reloaded restarts a looping source.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AudioSource {
    /// The sound file, such as `audio/wind.ogg`.
    pub clip: AssetRef,
    /// The mixer bus (its volume is in the audio project settings).
    pub bus: AudioBus,
    /// Loudness: 1 plays the file as it is, 0 is silent.
    #[reflect(@Range { min: 0.0, max: 2.0 })]
    pub volume: f32,
    /// Playback speed and pitch: 2 is an octave up, 0.5 an octave down.
    #[reflect(@Range { min: 0.25, max: 4.0 })]
    pub pitch: f32,
    /// Start again at the end (ambience, music). Off: play once.
    pub looping: bool,
    /// Whether it plays. Turning it off fades the sound out; turning it on
    /// again starts it from the beginning.
    pub playing: bool,
    /// Fade-in time when the sound starts.
    #[reflect(@Range { min: 0.0, max: 30.0 }, @Unit("s"))]
    pub fade_in: f32,
    /// Fade-out time when the sound stops (despawn, scene switch, `playing`
    /// off).
    #[reflect(@Range { min: 0.0, max: 30.0 }, @Unit("s"))]
    pub fade_out: f32,
    /// Positional: panned and attenuated by the entity's position relative
    /// to the listener. Off: heard everywhere at the same volume.
    pub spatial: bool,
    /// Positional sounds are at full volume within this distance.
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"))]
    pub min_distance: f32,
    /// Positional sounds are silent beyond this distance.
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"))]
    pub max_distance: f32,
}

impl Default for AudioSource {
    fn default() -> Self {
        Self {
            clip: AssetRef::NONE,
            bus: AudioBus::Ambience,
            volume: 1.0,
            pitch: 1.0,
            looping: true,
            playing: true,
            fade_in: 0.5,
            fade_out: 0.5,
            spatial: false,
            min_distance: 1.0,
            max_distance: 30.0,
        }
    }
}

impl AudioSource {
    /// A looping, non-positional source on the ambience bus playing the
    /// file at `path` (relative to `assets/`).
    pub fn ambience(path: &str) -> Self {
        Self {
            clip: AssetRef::path(path),
            ..Default::default()
        }
    }

    /// Makes it positional, at full volume within `min_distance` and silent
    /// beyond `max_distance`.
    pub fn spatial(mut self, min_distance: f32, max_distance: f32) -> Self {
        self.spatial = true;
        self.min_distance = min_distance;
        self.max_distance = max_distance;
        self
    }

    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    pub fn with_bus(mut self, bus: AudioBus) -> Self {
        self.bus = bus;
        self
    }
}

/// Where sounds are heard from: positional sounds are panned and attenuated
/// relative to the first entity with this component (its global position
/// and rotation; -Z is forward, +X is right). Without one, the active camera
/// listens.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AudioListener;

/// Event: play a clip once. Send it with `world.send(PlaySound::new(..))`
/// from any stage; it starts in the same frame's `PostUpdate` (a clip that
/// is not loaded yet starts when it is, or is dropped with a warning after
/// [`PENDING_TIMEOUT`](crate::PENDING_TIMEOUT) seconds).
///
/// ```
/// use box_audio::PlaySound;
/// use glam::Vec3;
/// let pickup = PlaySound::new("audio/pickup.wav")
///     .at(Vec3::new(2.0, 0.5, -3.0))
///     .volume(0.8)
///     .pitch(1.2);
/// assert_eq!(pickup.position, Some(Vec3::new(2.0, 0.5, -3.0)));
/// ```
///
/// A registered event type (`Registry::register_event`), so scripts send it
/// by name: `events.send("PlaySound", { clip = "audio/pickup.wav" })`.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct PlaySound {
    /// The sound file.
    pub clip: AssetRef,
    /// World position for a positional sound; `None` plays it everywhere at
    /// the same volume (interface sounds).
    pub position: Option<Vec3>,
    /// Loudness: 1 plays the file as it is.
    pub volume: f32,
    /// Playback speed and pitch: 2 is an octave up.
    pub pitch: f32,
    /// The mixer bus (default [`AudioBus::Effects`]).
    pub bus: AudioBus,
    /// Positional: full volume within this distance.
    pub min_distance: f32,
    /// Positional: silent beyond this distance.
    pub max_distance: f32,
}

/// No clip (nothing plays until `clip` is set), with [`PlaySound::clip`]'s
/// other defaults.
impl Default for PlaySound {
    fn default() -> Self {
        Self::clip(AssetRef::NONE)
    }
}

impl PlaySound {
    /// The file at `path` (relative to `assets/`), everywhere, at volume 1
    /// and normal pitch, on the effects bus.
    pub fn new(path: &str) -> Self {
        Self::clip(AssetRef::path(path))
    }

    /// [`new`](Self::new) for an asset reference.
    pub fn clip(clip: AssetRef) -> Self {
        Self {
            clip,
            position: None,
            volume: 1.0,
            pitch: 1.0,
            bus: AudioBus::Effects,
            min_distance: 1.0,
            max_distance: 30.0,
        }
    }

    /// Positional, at `position`.
    pub fn at(mut self, position: Vec3) -> Self {
        self.position = Some(position);
        self
    }

    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    pub fn pitch(mut self, pitch: f32) -> Self {
        self.pitch = pitch;
        self
    }

    pub fn bus(mut self, bus: AudioBus) -> Self {
        self.bus = bus;
        self
    }

    /// Full volume within `min`, silent beyond `max` (positional sounds).
    pub fn distances(mut self, min: f32, max: f32) -> Self {
        self.min_distance = min;
        self.max_distance = max;
        self
    }
}

/// Project settings resource (`assets/settings/AudioSettings.json`, Game ▸
/// Project Settings in the editor): the master and bus volumes. Hot-reloads,
/// also during Play.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Resource, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AudioSettings {
    /// Volume of everything.
    #[reflect(@Range { min: 0.0, max: 2.0 })]
    pub master: f32,
    /// Volume of the effects bus (one-shots).
    #[reflect(@Range { min: 0.0, max: 2.0 })]
    pub effects: f32,
    /// Volume of the ambience bus.
    #[reflect(@Range { min: 0.0, max: 2.0 })]
    pub ambience: f32,
    /// Volume of the music bus.
    #[reflect(@Range { min: 0.0, max: 2.0 })]
    pub music: f32,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            effects: 1.0,
            ambience: 1.0,
            music: 1.0,
        }
    }
}

impl AudioSettings {
    /// The volume of `bus` (not including the master volume).
    pub fn bus(&self, bus: AudioBus) -> f32 {
        match bus {
            AudioBus::Effects => self.effects,
            AudioBus::Ambience => self.ambience,
            AudioBus::Music => self.music,
        }
    }
}
