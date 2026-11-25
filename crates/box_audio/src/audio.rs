//! The [`Audio`] resource and the per-frame system that keeps kira in step
//! with the world ([`update_audio`]).
//!
//! Everything here runs on the main thread inside a world system; kira's
//! audio thread only ever sees commands and sound data sent through its
//! handles, never the world.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use box_assets::{AssetServer, LoadState};
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::event::{EventCursor, Events};
use box_core::time::Time;
use box_core::transform::GlobalTransform;
use box_core::world::{Entity, World};
use glam::{Quat, Vec3};
use kira::listener::{ListenerHandle, ListenerId};
use kira::sound::static_sound::StaticSoundHandle;
use kira::sound::streaming::{StreamingSoundData, StreamingSoundHandle};
use kira::sound::{FromFileError, PlaybackState, Region};
use kira::track::{SpatialTrackBuilder, SpatialTrackHandle, TrackBuilder, TrackHandle};
use kira::{Decibels, Easing, Tween};

use crate::clip::{AudioClip, ClipData};
use crate::components::{AudioBus, AudioListener, AudioSettings, AudioSource, PlaySound};
use crate::meter::{MeterBuilder, MeterShared, Mix, MixTarget, Spatial, VoiceLevel, spatial};
use crate::output::{AudioOutput, Output, OutputLevels};

/// Seconds a [`PlaySound`] waits for its clip to load before it is dropped
/// (with a warning).
pub const PENDING_TIMEOUT: f32 = 2.0;

/// Glide time of live volume, pitch and bus volume changes.
const GLIDE: Duration = Duration::from_millis(50);
/// Fade of a pause or resume.
const PAUSE_FADE: Duration = Duration::from_millis(100);
/// Crossfade when a hot-reloaded clip restarts a looping source.
const RELOAD_FADE: f32 = 0.1;
/// Positional sources moving less than this per frame are not updated.
const MOVE_EPSILON: f32 = 1e-3;

/// What a world's audio is playing through (see [`AudioOutput`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    /// Nothing has needed sound yet; no output was chosen or opened.
    NotStarted,
    /// The device is being opened on a background thread; sounds wait.
    Opening,
    Silent,
    Device,
    Capture,
}

/// Playback state of a voice, as kira reports it (or as tracked, when
/// silent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceState {
    Playing,
    /// Fading out before it stops.
    Stopping,
    /// Paused by kira (not used for bus pauses: see [`Audio::is_paused`]).
    Paused,
}

/// One sound playing now, for inspection, the mixer and tests
/// ([`Audio::voices`]).
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceInfo {
    /// Unique per world for its lifetime (a one-shot played twice is two
    /// voices).
    pub id: u64,
    pub clip: AssetRef,
    pub bus: AudioBus,
    /// The [`AudioSource`] entity, or `None` for a [`PlaySound`] one-shot.
    pub source: Option<Entity>,
    /// Requested loudness (before bus and master volumes; 1 = as recorded).
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
    /// World position of a positional sound.
    pub position: Option<Vec3>,
    /// Positional: full volume within this distance.
    pub min_distance: f32,
    /// Positional: silent beyond this distance.
    pub max_distance: f32,
    /// The clip is streamed rather than decoded in memory.
    pub streaming: bool,
    pub state: VoiceState,
    /// How loud it is at the listener (measured; before solo and mute).
    /// `None` on a silent output, or before the first measurement.
    pub level: Option<VoiceLevel>,
    /// It matches a mute target ([`Audio::mute`]).
    pub muted: bool,
    /// It matches a solo target ([`Audio::solo`]).
    pub soloed: bool,
    /// It is heard: not muted, and nothing is soloed or it is.
    pub audible: bool,
}

impl VoiceInfo {
    /// Distance, attenuation and panning relative to `listener` (see
    /// [`Audio::listener`]); `None` for a non-positional voice.
    pub fn spatial(&self, listener: (Vec3, Quat)) -> Option<Spatial> {
        self.position
            .map(|p| spatial(listener, p, self.min_distance, self.max_distance))
    }
}

enum Sound {
    Static(StaticSoundHandle),
    Streaming(StreamingSoundHandle<FromFileError>),
}

impl Sound {
    fn state(&self) -> PlaybackState {
        match self {
            Self::Static(h) => h.state(),
            Self::Streaming(h) => h.state(),
        }
    }

    fn stop(&mut self, tween: Tween) {
        match self {
            Self::Static(h) => h.stop(tween),
            Self::Streaming(h) => h.stop(tween),
        }
    }

    fn set_volume(&mut self, volume: f32) {
        let tween = glide();
        match self {
            Self::Static(h) => h.set_volume(decibels(volume), tween),
            Self::Streaming(h) => h.set_volume(decibels(volume), tween),
        }
    }

    fn set_pitch(&mut self, pitch: f32) {
        let tween = glide();
        match self {
            Self::Static(h) => h.set_playback_rate(f64::from(pitch), tween),
            Self::Streaming(h) => h.set_playback_rate(f64::from(pitch), tween),
        }
    }
}

struct Voice {
    id: u64,
    info: VoiceInfo,
    /// The kira sound; `None` on a silent output.
    sound: Option<Sound>,
    /// The positional track the sound plays on (inside the strip).
    track: Option<SpatialTrackHandle>,
    /// The voice's own sub-track of its bus: meter, solo and mute.
    strip: Option<TrackHandle>,
    meter: Option<Arc<MeterShared>>,
    /// Silent output: seconds until the voice ends (`None`: looping).
    silent_left: Option<f32>,
}

/// What [`update_audio`] last did for an [`AudioSource`] entity.
struct SourceState {
    /// The component as last applied.
    source: AudioSource,
    clip: Option<Handle<AudioClip>>,
    /// Version of the clip asset the voice plays.
    clip_version: Option<u64>,
    voice: Option<u64>,
    /// Played once to the end, or could not play: waits for a change.
    finished: bool,
    position: Vec3,
    /// Frame counter when last seen in the world.
    seen: u64,
    /// The next start is a hot-reload restart: crossfade quickly.
    crossfade: bool,
}

struct Pending {
    sound: PlaySound,
    waited: f32,
}

/// A clip requested from the asset server (or looked up).
#[derive(Clone, Copy)]
enum ClipSlot {
    Handle(Handle<AudioClip>),
    /// The reference names no file the server knows.
    Invalid,
}

enum ClipStatus<'a> {
    Ready(&'a AudioClip, Handle<AudioClip>, u64),
    Loading,
    Failed,
}

/// Resource: the world's audio state. Inserted by
/// [`AudioPlugin`](crate::AudioPlugin); systems read it for inspection
/// ([`voices`](Self::voices), [`output`](Self::output),
/// [`levels`](Self::levels)), and hosts pause it
/// ([`set_paused`](Self::set_paused)).
pub struct Audio {
    output: Option<Output>,
    kind: OutputKind,
    listener_handle: Option<ListenerHandle>,
    /// One kira sub-track per [`AudioBus`], by index.
    buses: Vec<TrackHandle>,
    voices: Vec<Voice>,
    sources: HashMap<Entity, SourceState>,
    pending: Vec<Pending>,
    clips: HashMap<AssetRef, ClipSlot>,
    preload: Vec<AssetRef>,
    applied_settings: Option<AudioSettings>,
    host_paused: bool,
    /// Pause state applied to kira (host or game pause).
    paused: bool,
    listener: (Vec3, Quat),
    default_listener: Option<fn(&World) -> Option<GlobalTransform>>,
    next_voice: u64,
    frame: u64,
    levels: Option<OutputLevels>,
    mix: Mix,
    /// The mix changed since it was last applied to the voices.
    mix_dirty: bool,
    cursor: EventCursor<PlaySound>,
    logged: HashSet<String>,
    started_in: Option<Duration>,
    opening_since: Option<Instant>,
    opening: Option<std::sync::mpsc::Receiver<Result<Output, String>>>,
}

fn glide() -> Tween {
    Tween {
        duration: GLIDE,
        ..Default::default()
    }
}

fn fade(seconds: f32) -> Tween {
    Tween {
        duration: Duration::from_secs_f32(seconds.max(0.0)),
        easing: Easing::Linear,
        ..Default::default()
    }
}

/// Linear amplitude (1 = unchanged) as kira decibels; at or below 0.001
/// (-60 dB) it is silence.
pub(crate) fn decibels(amplitude: f32) -> Decibels {
    if amplitude <= 0.001 || !amplitude.is_finite() {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * amplitude.log10())
    }
}

fn v3(v: Vec3) -> mint::Vector3<f32> {
    mint::Vector3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn quat(q: Quat) -> mint::Quaternion<f32> {
    mint::Quaternion {
        v: mint::Vector3 {
            x: q.x,
            y: q.y,
            z: q.z,
        },
        s: q.w,
    }
}

/// What to start.
struct Request<'a> {
    clip_ref: &'a AssetRef,
    bus: AudioBus,
    source: Option<Entity>,
    volume: f32,
    pitch: f32,
    looping: bool,
    position: Option<Vec3>,
    min_distance: f32,
    max_distance: f32,
    fade_in: f32,
}

impl Audio {
    pub(crate) fn new(default_listener: Option<fn(&World) -> Option<GlobalTransform>>) -> Self {
        Self {
            output: None,
            kind: OutputKind::NotStarted,
            listener_handle: None,
            buses: Vec::new(),
            voices: Vec::new(),
            sources: HashMap::new(),
            pending: Vec::new(),
            clips: HashMap::new(),
            preload: Vec::new(),
            applied_settings: None,
            host_paused: false,
            paused: false,
            listener: (Vec3::ZERO, Quat::IDENTITY),
            default_listener,
            next_voice: 1,
            frame: 0,
            levels: None,
            mix: Mix::default(),
            mix_dirty: false,
            cursor: EventCursor::default(),
            logged: HashSet::new(),
            started_in: None,
            opening_since: None,
            opening: None,
        }
    }

    /// The output in use.
    pub fn output(&self) -> OutputKind {
        self.kind
    }

    /// How long opening the output took (the device, on the first sound,
    /// on a background thread).
    pub fn startup_time(&self) -> Option<Duration> {
        self.started_in
    }

    /// The sounds playing now (including ones fading out).
    pub fn voices(&self) -> impl Iterator<Item = &VoiceInfo> {
        self.voices.iter().map(|v| &v.info)
    }

    /// The voice of an [`AudioSource`] entity, if it is playing.
    pub fn source_voice(&self, entity: Entity) -> Option<&VoiceInfo> {
        let id = self.sources.get(&entity)?.voice?;
        self.voices.iter().find(|v| v.id == id).map(|v| &v.info)
    }

    /// [`PlaySound`]s waiting for their clip to load.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Where sounds are heard from (position, orientation), as of the last
    /// frame.
    pub fn listener(&self) -> (Vec3, Quat) {
        self.listener
    }

    /// Loudness of the last frame's mix ([`AudioOutput::Capture`] only).
    pub fn levels(&self) -> Option<OutputLevels> {
        self.levels
    }

    /// Whether playback is paused (by the host or by `Time::paused`).
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Pauses or resumes every bus at once, with a short fade. For hosts
    /// that stop running the world's systems (editor Play pause); games
    /// pause with `Time::paused`, which the audio system follows.
    pub fn set_paused(&mut self, paused: bool) {
        self.host_paused = paused;
        self.apply_pause(paused);
    }

    /// The solo and mute targets.
    pub fn mix(&self) -> &Mix {
        &self.mix
    }

    /// Mutes (`on`) or unmutes `target` (an entity with the sources below
    /// it, a bus or a clip): its voices keep playing, silently, and keep
    /// their measured level. Runtime only (never saved). Returns whether
    /// anything changed.
    pub fn mute(&mut self, target: MixTarget, on: bool) -> bool {
        let changed = Mix::set(&mut self.mix.muted, target, on);
        self.mix_dirty |= changed;
        changed
    }

    /// Solos (`on`) or unsolos `target`: while anything is soloed, only
    /// soloed voices that are not muted are heard. Runtime only. Returns
    /// whether anything changed.
    pub fn solo(&mut self, target: MixTarget, on: bool) -> bool {
        let changed = Mix::set(&mut self.mix.soloed, target, on);
        self.mix_dirty |= changed;
        changed
    }

    /// Clears every solo and mute.
    pub fn clear_mix(&mut self) {
        if !self.mix.is_empty() {
            self.mix = Mix::default();
            self.mix_dirty = true;
        }
    }

    /// Requests `clip` now so a later [`PlaySound`] of it starts without
    /// waiting for the import.
    pub fn preload(&mut self, clip: AssetRef) {
        self.preload.push(clip);
    }

    fn apply_pause(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        let tween = Tween {
            duration: PAUSE_FADE,
            ..Default::default()
        };
        for bus in &mut self.buses {
            if paused {
                bus.pause(tween);
            } else {
                bus.resume(tween);
            }
        }
    }

    fn warn_once(&mut self, message: String) {
        if self.logged.insert(message.clone()) {
            tracing::warn!(target: "box_audio", "{message}");
        }
    }

    fn error_once(&mut self, message: String) {
        if self.logged.insert(message.clone()) {
            tracing::error!(target: "box_audio", "{message}");
        }
    }

    /// Chooses the output (once, on the first sound). A device is opened
    /// on a background thread (it takes 100-200 ms): until it is ready the
    /// output is [`OutputKind::Opening`] and sounds wait.
    fn start_output(&mut self, world: &World) {
        if self.kind != OutputKind::NotStarted {
            return;
        }
        let choice = world
            .get_resource::<AudioOutput>()
            .copied()
            .unwrap_or_else(AudioOutput::process_default);
        self.opening_since = Some(Instant::now());
        match choice {
            AudioOutput::Silent => self.finish_output(None, OutputKind::Silent),
            AudioOutput::Capture { sample_rate } => {
                self.finish_output(Some(Output::capture(sample_rate)), OutputKind::Capture);
            }
            AudioOutput::Device => {
                let (tx, rx) = std::sync::mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("box-audio-open".into())
                    .spawn(move || {
                        let _ = tx.send(Output::device());
                    });
                match spawned {
                    Ok(_) => {
                        self.opening = Some(rx);
                        self.kind = OutputKind::Opening;
                    }
                    Err(e) => self.device_failed(e.to_string()),
                }
            }
        }
    }

    /// Takes the device once the opening thread is done.
    fn poll_opening(&mut self) {
        let Some(rx) = &self.opening else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("the device thread ended".to_owned())
            }
        };
        self.opening = None;
        match result {
            Ok(output) => self.finish_output(Some(output), OutputKind::Device),
            Err(e) => self.device_failed(e),
        }
    }

    fn device_failed(&mut self, e: String) {
        self.warn_once(format!(
            "no audio output device could be opened ({e}); sound is off. Connect or enable an output device in the system sound settings and restart the game"
        ));
        self.finish_output(None, OutputKind::Silent);
    }

    /// Creates the buses and the listener on a new output.
    fn finish_output(&mut self, output: Option<Output>, kind: OutputKind) {
        self.kind = kind;
        self.started_in = self.opening_since.take().map(|t| t.elapsed());
        let Some(mut output) = output else {
            self.kind = OutputKind::Silent;
            return;
        };
        let mut buses = Vec::new();
        for _ in AudioBus::ALL {
            match output.add_bus() {
                Ok(t) => buses.push(t),
                Err(e) => {
                    self.error_once(format!("audio buses could not be created: {e}"));
                    self.kind = OutputKind::Silent;
                    return;
                }
            }
        }
        match output.add_listener(v3(self.listener.0), quat(self.listener.1)) {
            Ok(l) => self.listener_handle = Some(l),
            Err(e) => {
                self.error_once(format!("audio listener could not be created: {e}"));
                self.kind = OutputKind::Silent;
                return;
            }
        }
        self.buses = buses;
        self.output = Some(output);
        // Settings and pause apply to the new tracks.
        self.applied_settings = None;
        if self.paused {
            self.paused = false;
            self.apply_pause(true);
        }
        if kind == OutputKind::Device {
            tracing::debug!(target: "box_audio", "audio device opened in {:?}", self.started_in);
        }
    }

    fn apply_settings(&mut self, settings: &AudioSettings) {
        if self.applied_settings.as_ref() == Some(settings) {
            return;
        }
        if let Some(output) = &mut self.output {
            output
                .main_track()
                .set_volume(decibels(settings.master), glide());
            for (bus, track) in AudioBus::ALL.iter().zip(&mut self.buses) {
                track.set_volume(decibels(settings.bus(*bus)), glide());
            }
        }
        self.applied_settings = Some(settings.clone());
    }

    /// The clip `r` names, requesting it on first use.
    fn clip<'w>(
        &mut self,
        world: &'w World,
        server: Option<&AssetServer>,
        r: &AssetRef,
    ) -> ClipStatus<'w> {
        if r.is_none() {
            return ClipStatus::Failed;
        }
        let slot = match self.clips.get(r) {
            Some(s) => *s,
            None => {
                let slot = match server {
                    Some(server) => match server.load::<AudioClip>(r) {
                        Ok(h) => ClipSlot::Handle(h),
                        Err(e) => {
                            self.error_once(format!("audio clip {}: {e}", describe(r)));
                            ClipSlot::Invalid
                        }
                    },
                    None => match world
                        .get_resource::<AssetLookup>()
                        .and_then(|l| l.get::<AudioClip>(r))
                    {
                        Some(h) => ClipSlot::Handle(h),
                        // Not loaded by anyone yet: look again next frame.
                        None => return ClipStatus::Loading,
                    },
                };
                self.clips.insert(r.clone(), slot);
                slot
            }
        };
        let ClipSlot::Handle(handle) = slot else {
            return ClipStatus::Failed;
        };
        let store = world.get_resource::<Assets<AudioClip>>();
        if let Some((clip, version)) =
            store.and_then(|s| Some((s.get(handle)?, s.version(handle)?)))
        {
            return ClipStatus::Ready(clip, handle, version);
        }
        match server.and_then(|s| s.handle_state(handle)) {
            Some(LoadState::Failed(message)) => {
                self.error_once(format!(
                    "audio clip {} failed to load: {message}",
                    describe(r)
                ));
                ClipStatus::Failed
            }
            _ => ClipStatus::Loading,
        }
    }

    /// Starts a voice; returns its id.
    fn start_voice(&mut self, clip: &AudioClip, req: Request) -> Option<u64> {
        let id = self.next_voice;
        self.next_voice += 1;
        let info = VoiceInfo {
            id,
            clip: req.clip_ref.clone(),
            bus: req.bus,
            source: req.source,
            volume: req.volume,
            pitch: req.pitch,
            looping: req.looping,
            position: req.position,
            min_distance: req.min_distance,
            max_distance: req.max_distance,
            streaming: clip.is_streaming(),
            state: VoiceState::Playing,
            level: None,
            muted: false,
            soloed: false,
            audible: true,
        };
        let mut voice = Voice {
            id,
            info,
            sound: None,
            track: None,
            strip: None,
            meter: None,
            silent_left: None,
        };
        // The mix is applied to the new voice this frame.
        self.mix_dirty |= !self.mix.is_empty();
        if self.output.is_none() {
            voice.silent_left =
                (!req.looping).then(|| clip.duration().as_secs_f32() / req.pitch.max(0.01));
            self.voices.push(voice);
            return Some(id);
        }
        let listener = self.listener_handle.as_ref().map(|l| l.id())?;
        let bus = &mut self.buses[req.bus.index()];
        match play_on(bus, listener, clip, &req) {
            Ok(played) => {
                voice.sound = Some(played.sound);
                voice.track = played.track;
                voice.strip = Some(played.strip);
                voice.meter = Some(played.meter);
                self.voices.push(voice);
                Some(id)
            }
            Err(e) => {
                self.warn_once(format!("{} could not play: {e}", describe(req.clip_ref)));
                None
            }
        }
    }

    fn voice_mut(&mut self, id: u64) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    fn stop_voice(&mut self, id: u64, fade_out: f32) {
        let Some(v) = self.voice_mut(id) else {
            return;
        };
        if v.info.state == VoiceState::Stopping {
            return;
        }
        v.info.state = VoiceState::Stopping;
        match &mut v.sound {
            Some(s) => s.stop(fade(fade_out)),
            None => v.silent_left = Some(fade_out.max(0.0)),
        }
    }

    /// Drops voices that ended; refreshes the others' states.
    fn poll_voices(&mut self, dt: f32) {
        let paused = self.paused;
        self.voices.retain_mut(|v| match &v.sound {
            Some(sound) => {
                v.info.state = match sound.state() {
                    PlaybackState::Stopped => return false,
                    // A stop sent this frame has not reached the audio
                    // thread yet.
                    _ if v.info.state == VoiceState::Stopping => VoiceState::Stopping,
                    PlaybackState::Stopping => VoiceState::Stopping,
                    PlaybackState::Paused
                    | PlaybackState::Pausing
                    | PlaybackState::WaitingToResume => VoiceState::Paused,
                    PlaybackState::Playing | PlaybackState::Resuming => VoiceState::Playing,
                };
                true
            }
            None => match &mut v.silent_left {
                Some(left) => {
                    if !paused {
                        *left -= dt;
                    }
                    *left > 0.0
                }
                None => true,
            },
        });
    }

    fn update_listener(&mut self, world: &World) {
        let pose = world
            .query::<(&AudioListener, &GlobalTransform)>()
            .iter()
            .next()
            .map(|(_, g)| *g)
            .or_else(|| self.default_listener.and_then(|f| f(world)));
        let (position, rotation) = pose.map_or((Vec3::ZERO, Quat::IDENTITY), |g| {
            let (_, r, t) = g.0.to_scale_rotation_translation();
            (t, r)
        });
        if (position, rotation) == self.listener {
            return;
        }
        self.listener = (position, rotation);
        if let Some(l) = &mut self.listener_handle {
            l.set_position(v3(position), Tween::default());
            l.set_orientation(quat(rotation), Tween::default());
        }
    }

    fn update_sources(&mut self, world: &World, server: Option<&AssetServer>) {
        self.frame += 1;
        let frame = self.frame;
        let mut query = world.query::<(Entity, &AudioSource, Option<&GlobalTransform>)>();
        for (entity, source, global) in query.iter() {
            let position = global.map_or(Vec3::ZERO, |g| g.0.translation.into());
            let mut state = self.sources.remove(&entity).unwrap_or_else(|| SourceState {
                source: source.clone(),
                clip: None,
                clip_version: None,
                voice: None,
                finished: false,
                position,
                seen: frame,
                crossfade: false,
            });
            state.seen = frame;
            self.update_source(world, server, entity, source, position, &mut state);
            self.sources.insert(entity, state);
        }
        drop(query);
        // Despawned entities (or a removed component): fade out.
        let gone: Vec<Entity> = self
            .sources
            .iter()
            .filter(|(_, s)| s.seen != frame)
            .map(|(e, _)| *e)
            .collect();
        for e in gone {
            if let Some(state) = self.sources.remove(&e)
                && let Some(id) = state.voice
            {
                self.stop_voice(id, state.source.fade_out);
            }
        }
    }

    fn update_source(
        &mut self,
        world: &World,
        server: Option<&AssetServer>,
        entity: Entity,
        source: &AudioSource,
        position: Vec3,
        state: &mut SourceState,
    ) {
        // The voice ended by itself (a one-shot source reached its end).
        if let Some(id) = state.voice
            && !self.voices.iter().any(|v| v.id == id)
        {
            state.voice = None;
            state.finished = true;
        }
        let old = &state.source;
        let changed = old != source;
        if changed {
            let restart = old.clip != source.clip
                || old.bus != source.bus
                || old.looping != source.looping
                || old.spatial != source.spatial
                || (!old.playing && source.playing)
                || (source.spatial
                    && (old.min_distance != source.min_distance
                        || old.max_distance != source.max_distance));
            if old.clip != source.clip {
                state.clip = None;
                state.clip_version = None;
            }
            if let Some(id) = state.voice {
                if restart || !source.playing {
                    self.stop_voice(id, source.fade_out);
                    state.voice = None;
                } else if let Some(v) = self.voice_mut(id) {
                    if v.info.volume != source.volume {
                        v.info.volume = source.volume;
                        if let Some(s) = &mut v.sound {
                            s.set_volume(source.volume);
                        }
                    }
                    if v.info.pitch != source.pitch {
                        v.info.pitch = source.pitch;
                        if let Some(s) = &mut v.sound {
                            s.set_pitch(source.pitch);
                        }
                    }
                }
            }
            if restart {
                state.finished = false;
            }
            state.source = source.clone();
        }
        if !source.playing {
            return;
        }
        // Hot reload: a looping source restarts with the new clip.
        if let (Some(id), Some(handle)) = (state.voice, state.clip)
            && source.looping
        {
            let version = world
                .get_resource::<Assets<AudioClip>>()
                .and_then(|s| s.version(handle));
            if version.is_some() && version != state.clip_version {
                self.stop_voice(id, RELOAD_FADE);
                state.voice = None;
                state.crossfade = true;
            }
        }
        // Follow the entity.
        if let Some(id) = state.voice {
            if source.spatial
                && position.distance_squared(state.position) > MOVE_EPSILON * MOVE_EPSILON
            {
                state.position = position;
                if let Some(v) = self.voice_mut(id) {
                    v.info.position = Some(position);
                    if let Some(t) = &mut v.track {
                        t.set_position(v3(position), Tween::default());
                    }
                }
            }
            return;
        }
        if state.finished {
            return;
        }
        self.start_output(world);
        if self.kind == OutputKind::Opening {
            return;
        }
        match self.clip(world, server, &source.clip) {
            ClipStatus::Ready(clip, handle, version) => {
                state.clip = Some(handle);
                state.clip_version = Some(version);
                state.position = position;
                let req = Request {
                    clip_ref: &source.clip,
                    bus: source.bus,
                    source: Some(entity),
                    volume: source.volume,
                    pitch: source.pitch,
                    looping: source.looping,
                    position: source.spatial.then_some(position),
                    min_distance: source.min_distance,
                    max_distance: source.max_distance,
                    fade_in: if std::mem::take(&mut state.crossfade) {
                        RELOAD_FADE
                    } else {
                        source.fade_in
                    },
                };
                state.voice = self.start_voice(clip, req);
                if state.voice.is_none() {
                    state.finished = true;
                }
            }
            // A failed clip is logged once and tried again each frame, so a
            // fixed file (hot reload) starts the source.
            ClipStatus::Loading | ClipStatus::Failed => {}
        }
    }

    fn update_one_shots(&mut self, world: &World, server: Option<&AssetServer>, dt: f32) {
        if let Some(events) = world.get_resource::<Events<PlaySound>>() {
            for e in self.cursor.read(events) {
                self.pending.push(Pending {
                    sound: e.clone(),
                    waited: 0.0,
                });
            }
        }
        for r in std::mem::take(&mut self.preload) {
            let _ = self.clip(world, server, &r);
        }
        if self.pending.is_empty() {
            return;
        }
        self.start_output(world);
        if self.kind == OutputKind::Opening {
            return;
        }
        let mut pending = std::mem::take(&mut self.pending);
        pending.retain_mut(|p| {
            let sound = &p.sound;
            match self.clip(world, server, &sound.clip) {
                ClipStatus::Ready(clip, _, _) => {
                    let req = Request {
                        clip_ref: &sound.clip,
                        bus: sound.bus,
                        source: None,
                        volume: sound.volume,
                        pitch: sound.pitch,
                        looping: false,
                        position: sound.position,
                        min_distance: sound.min_distance,
                        max_distance: sound.max_distance,
                        fade_in: 0.0,
                    };
                    self.start_voice(clip, req);
                    false
                }
                ClipStatus::Failed => false,
                ClipStatus::Loading => {
                    p.waited += dt;
                    if p.waited > PENDING_TIMEOUT {
                        tracing::warn!(
                            target: "box_audio",
                            "{} was not loaded after {PENDING_TIMEOUT} s; the sound is dropped",
                            describe(&sound.clip)
                        );
                        return false;
                    }
                    true
                }
            }
        });
        self.pending = pending;
    }
}

/// Plays `clip` on `bus`: on a new strip (the voice's metered sub-track),
/// inside it on a new positional track for a positional request.
fn play_on(
    bus: &mut TrackHandle,
    listener: ListenerId,
    clip: &AudioClip,
    req: &Request,
) -> Result<Played, String> {
    let fade_in = (req.fade_in > 0.0).then(|| fade(req.fade_in));
    let loop_region = req.looping.then(|| Region::from(..));
    let volume = decibels(req.volume);
    let rate = f64::from(req.pitch.max(0.01));
    let meter = Arc::new(MeterShared::default());
    let mut strip = bus
        .add_sub_track(
            TrackBuilder::new()
                .sub_track_capacity(1)
                .sound_capacity(1)
                .persist_until_sounds_finish(true)
                .with_effect(MeterBuilder(meter.clone())),
        )
        .map_err(|e| format!("too many sounds at once on one bus ({e})"))?;
    let mut track = match req.position {
        Some(p) => {
            let min = req.min_distance.max(0.0);
            let builder = SpatialTrackBuilder::new()
                .distances((min, req.max_distance.max(min + 0.01)))
                .persist_until_sounds_finish(true);
            Some(
                strip
                    .add_spatial_sub_track(listener, v3(p), builder)
                    .map_err(|e| format!("too many positional sounds at once ({e})"))?,
            )
        }
        None => None,
    };
    let sound = match clip.data() {
        ClipData::Static(data) => {
            let data = data
                .clone()
                .volume(volume)
                .playback_rate(rate)
                .loop_region(loop_region)
                .fade_in_tween(fade_in);
            match &mut track {
                Some(t) => t.play(data),
                None => strip.play(data),
            }
            .map(Sound::Static)
            .map_err(|e| format!("{e:?}"))?
        }
        ClipData::Streaming(bytes) => {
            let data = StreamingSoundData::from_cursor(Cursor::new(bytes.clone()))
                .map_err(|e| e.to_string())?
                .volume(volume)
                .playback_rate(rate)
                .loop_region(loop_region)
                .fade_in_tween(fade_in);
            match &mut track {
                Some(t) => t.play(data),
                None => strip.play(data),
            }
            .map(Sound::Streaming)
            .map_err(|e| format!("{e:?}"))?
        }
    };
    Ok(Played {
        sound,
        track,
        strip,
        meter,
    })
}

/// A sound started by [`play_on`].
struct Played {
    sound: Sound,
    track: Option<SpatialTrackHandle>,
    strip: TrackHandle,
    meter: Arc<MeterShared>,
}

fn describe(r: &AssetRef) -> String {
    match (&r.path, r.id) {
        (Some(p), _) => format!("`{p}`"),
        (None, Some(id)) => format!("asset {id}"),
        (None, None) => "(no clip)".to_owned(),
    }
}

/// Linear gain of a volume setting as [`decibels`] applies it.
fn gain(amplitude: f32) -> f32 {
    if amplitude <= 0.001 || !amplitude.is_finite() {
        0.0
    } else {
        amplitude
    }
}

impl Audio {
    /// Solo and mute: each voice's strip is silenced or opened when what it
    /// matches changes. Recomputed when the mix changed or a voice started,
    /// and every frame while an entity is a target (sources can move in the
    /// hierarchy): one match per voice, entity targets walking up the
    /// hierarchy. Kira is told only about changes.
    fn apply_mix(&mut self, world: &World) {
        let entity_targets = self
            .mix
            .muted
            .iter()
            .chain(&self.mix.soloed)
            .any(|t| matches!(t, MixTarget::Entity(_)));
        if !self.mix_dirty && !entity_targets {
            return;
        }
        self.mix_dirty = false;
        let any_solo = !self.mix.soloed.is_empty();
        for v in &mut self.voices {
            let i = &mut v.info;
            i.muted = Mix::matches(&self.mix.muted, world, i.bus, &i.clip, i.source);
            i.soloed = any_solo && Mix::matches(&self.mix.soloed, world, i.bus, &i.clip, i.source);
            let audible = !i.muted && (!any_solo || i.soloed);
            if audible != i.audible {
                i.audible = audible;
                if let Some(strip) = &mut v.strip {
                    let volume = if audible {
                        Decibels::IDENTITY
                    } else {
                        Decibels::SILENCE
                    };
                    strip.set_volume(volume, glide());
                }
            }
        }
    }

    /// Takes each voice's meter (what the audio thread mixed since the last
    /// frame) and scales it by its bus and the master volume.
    fn read_meters(&mut self, settings: &AudioSettings) {
        let master = gain(settings.master);
        for v in &mut self.voices {
            let Some(meter) = &v.meter else {
                continue;
            };
            let acc = meter.take();
            let level = acc.level(master * gain(settings.bus(v.info.bus)));
            if level.frames > 0 {
                v.info.level = Some(level);
            } else if let Some(l) = &mut v.info.level {
                l.frames = 0;
            }
        }
    }
}

/// The audio system (`PostUpdate`, after transform propagation): follows
/// the settings, the pause state and the listener, starts, updates and
/// stops [`AudioSource`] voices, starts [`PlaySound`] one-shots, drops
/// finished voices and advances a capture output. Per frame it costs one
/// query over `AudioSource` entities and one pass over playing voices;
/// kira commands are sent only for changes.
pub fn update_audio(world: &mut World) {
    profiling::scope!("update_audio");
    let Some(mut audio) = world.remove_resource::<Audio>() else {
        return;
    };
    let (dt, game_paused) = world
        .get_resource::<Time>()
        .map_or((0.0, false), |t| (t.unscaled_delta, t.paused));
    let server = box_assets::loader(world);
    let settings = world
        .get_resource::<AudioSettings>()
        .cloned()
        .unwrap_or_default();
    audio.poll_opening();
    audio.apply_settings(&settings);
    audio.apply_pause(audio.host_paused || game_paused);
    audio.update_listener(world);
    audio.update_sources(world, server.as_ref());
    audio.update_one_shots(world, server.as_ref(), dt);
    audio.apply_settings(&settings);
    audio.poll_voices(dt);
    audio.apply_mix(world);
    if let Some(output) = &mut audio.output {
        let mut errors = Vec::new();
        audio.levels = output.frame(f64::from(dt), &mut |e| errors.push(e));
        for e in errors {
            audio.warn_once(e);
        }
    }
    audio.read_meters(&settings);
    world.insert_resource(audio);
}
