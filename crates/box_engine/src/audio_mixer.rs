//! The live audio mixer and mix reports, for people (the editor's Mixer
//! window) and agents (the control socket's `audio_levels`, `audio_solo`
//! and `audio_mute`, `box run --audio-report`).
//!
//! Everything reads [`Audio`]'s voices: each one's level at the listener
//! (measured by box_audio's per-voice meters, after distance, panning, bus
//! and master volume), its distance, attenuation and pan, and whether solo
//! and mute let it be heard. Solo and mute are runtime only: they live in
//! the playing world's `Audio` resource and end with it (editor Stop, the
//! game exiting).
//!
//! Mix targets are named the way agents and people think of them
//! ([`resolve_target`]): an entity (uid or unique name: the `Pond` source,
//! or an `Orchard` group with sources below it), a bus (`bus:ambience`, or
//! just `ambience`) or a clip (`clip:audio/pond.ogg`, or a path with an
//! audio extension).

use std::collections::HashMap;
use std::path::PathBuf;

use box_audio::{
    Audio, AudioBus, AudioSettings, AudioSource, MixTarget, OutputKind, VoiceInfo, VoiceLevel,
    VoiceState, amplitude_db,
};
use box_core::app::{App, Stage};
use box_core::asset_ref::AssetRef;
use box_core::identity::Name;
use box_core::time::Time;
use box_core::world::{Entity, World};
use serde_json::{Value as Json, json};

/// Audio file extensions a bare clip target may end with.
const CLIP_EXTENSIONS: [&str; 4] = [".wav", ".ogg", ".mp3", ".flac"];

fn bus_name(bus: AudioBus) -> &'static str {
    match bus {
        AudioBus::Effects => "Effects",
        AudioBus::Ambience => "Ambience",
        AudioBus::Music => "Music",
    }
}

fn parse_bus(name: &str) -> Option<AudioBus> {
    AudioBus::ALL
        .into_iter()
        .find(|b| bus_name(*b).eq_ignore_ascii_case(name.trim()))
}

fn output_name(kind: OutputKind) -> &'static str {
    match kind {
        OutputKind::NotStarted => "not_started",
        OutputKind::Opening => "opening",
        OutputKind::Silent => "silent",
        OutputKind::Device => "device",
        OutputKind::Capture => "capture",
    }
}

/// dBFS rounded to 0.1 dB; `null` for silence (JSON has no -inf).
fn db(amplitude: f32) -> Json {
    let d = amplitude_db(amplitude);
    if d.is_finite() && d > -120.0 {
        json!((d * 10.0).round() / 10.0)
    } else {
        Json::Null
    }
}

fn round(v: f32, places: i32) -> Json {
    let k = 10f32.powi(places);
    json!((v * k).round() / k)
}

fn clip_text(r: &AssetRef) -> String {
    match (&r.path, r.id) {
        (Some(p), _) => p.clone(),
        (None, Some(id)) => format!("asset {id}"),
        (None, None) => String::new(),
    }
}

/// An entity's name (or `entity <id>`), for people.
pub fn entity_label(world: &World, e: Entity) -> String {
    world
        .get::<Name>(e)
        .map(|n| n.0.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("entity {}", e.id()))
}

/// What a voice is called in the mixer: its source entity's name, or for a
/// one-shot its clip.
pub fn voice_label(world: &World, v: &VoiceInfo) -> String {
    match v.source {
        Some(e) => entity_label(world, e),
        None => clip_text(&v.clip),
    }
}

/// A mix target as JSON: `{"kind": "entity", "uid", "name"}`,
/// `{"kind": "bus", "bus"}` or `{"kind": "clip", "clip"}`.
pub fn target_json(world: &World, t: &MixTarget) -> Json {
    match t {
        MixTarget::Entity(e) => json!({
            "kind": "entity",
            "uid": world.uid(*e).map(|u| u.to_string()),
            "name": entity_label(world, *e),
        }),
        MixTarget::Bus(b) => json!({ "kind": "bus", "bus": bus_name(*b) }),
        MixTarget::Clip(c) => json!({ "kind": "clip", "clip": clip_text(c) }),
    }
}

/// A mix target for people: `Pond`, `bus Ambience`, `clip audio/pond.ogg`.
pub fn target_label(world: &World, t: &MixTarget) -> String {
    match t {
        MixTarget::Entity(e) => entity_label(world, *e),
        MixTarget::Bus(b) => format!("bus {}", bus_name(*b)),
        MixTarget::Clip(c) => format!("clip {}", clip_text(c)),
    }
}

/// Names of entities that have an [`AudioSource`] or have one below them,
/// for error messages.
fn audio_names(world: &World) -> Vec<String> {
    let mut names = Vec::new();
    let sources: Vec<Entity> = world
        .query::<(Entity, &AudioSource)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in sources {
        let mut at = Some(e);
        while let Some(x) = at {
            if let Some(n) = world.get::<Name>(x)
                && !n.0.is_empty()
                && !names.contains(&n.0)
            {
                names.push(n.0.clone());
            }
            at = world.parent(x);
        }
    }
    names.sort();
    names
}

/// Resolves a mix target named by a person or an agent:
///
/// - `bus:<name>`, `clip:<path>`, `entity:<uid or name>`: that kind;
/// - otherwise an entity (uid or unique name), then a bus name
///   (`effects`, `ambience`, `music`, any case), then a clip path ending in
///   `.wav`, `.ogg`, `.mp3` or `.flac` (relative to `assets/`).
///
/// An entity target applies to its own source and every source below it.
pub fn resolve_target(world: &World, text: &str) -> Result<MixTarget, String> {
    let text = text.trim();
    let entity = |reference: &str| -> Result<MixTarget, box_core::edit::EditError> {
        let uid = box_core::edit::resolve_entity(world, reference)?;
        world
            .entity_by_uid(uid)
            .map(MixTarget::Entity)
            .ok_or(box_core::edit::EditError::NoEntity(uid))
    };
    if let Some(rest) = text.strip_prefix("bus:") {
        return parse_bus(rest)
            .map(MixTarget::Bus)
            .ok_or_else(|| format!("no bus `{rest}`; buses: Effects, Ambience, Music"));
    }
    if let Some(rest) = text.strip_prefix("clip:") {
        return Ok(MixTarget::Clip(AssetRef::path(rest.trim())));
    }
    if let Some(rest) = text.strip_prefix("entity:") {
        return entity(rest.trim()).map_err(|e| e.to_string());
    }
    match entity(text) {
        Ok(t) => return Ok(t),
        Err(e @ box_core::edit::EditError::AmbiguousName { .. }) => return Err(e.to_string()),
        Err(_) => {}
    }
    if let Some(bus) = parse_bus(text) {
        return Ok(MixTarget::Bus(bus));
    }
    let lower = text.to_ascii_lowercase();
    if CLIP_EXTENSIONS.iter().any(|x| lower.ends_with(x)) {
        return Ok(MixTarget::Clip(AssetRef::path(text)));
    }
    let names = audio_names(world);
    let mut message = format!(
        "no entity, bus or clip `{text}`; sound entities: {}; buses: Effects, Ambience, Music; clips: `clip:audio/<file>`",
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    );
    let same_case = names
        .iter()
        .find(|n| n.eq_ignore_ascii_case(text))
        .map(String::as_str);
    if let Some(g) = same_case
        .or_else(|| box_core::reflect::did_you_mean(text, names.iter().map(String::as_str)))
    {
        message.push_str(&format!(" (did you mean `{g}`?)"));
    }
    Err(message)
}

fn level_json(level: Option<&VoiceLevel>) -> Json {
    match level {
        None => Json::Null,
        Some(l) => json!({
            "rms": round(l.rms(), 5),
            "rms_db": db(l.rms()),
            "peak": round(l.peak, 5),
            "peak_db": db(l.peak),
            "left": round(l.left, 5),
            "right": round(l.right, 5),
        }),
    }
}

fn gain(v: f32) -> f32 {
    if v <= 0.001 || !v.is_finite() { 0.0 } else { v }
}

/// One voice of [`levels_json`].
pub fn voice_json(world: &World, audio: &Audio, settings: &AudioSettings, v: &VoiceInfo) -> Json {
    let spatial = v.spatial(audio.listener());
    let attenuation = spatial.map_or(1.0, |s| s.attenuation);
    let expected = v.volume * attenuation * gain(settings.bus(v.bus)) * gain(settings.master);
    json!({
        "id": v.id,
        "kind": if v.source.is_some() { "source" } else { "one_shot" },
        "name": voice_label(world, v),
        "entity": v.source.and_then(|e| world.uid(e)).map(|u| u.to_string()),
        "clip": clip_text(&v.clip),
        "bus": bus_name(v.bus),
        "state": match v.state {
            VoiceState::Playing => "playing",
            VoiceState::Stopping => "stopping",
            VoiceState::Paused => "paused",
        },
        "looping": v.looping,
        "volume": round(v.volume, 4),
        "pitch": round(v.pitch, 4),
        "spatial": v.position.is_some(),
        "position": v.position.map(|p| [p.x, p.y, p.z]),
        "min_distance": v.position.map(|_| v.min_distance),
        "max_distance": v.position.map(|_| v.max_distance),
        "distance": spatial.map(|s| round(s.distance, 3)),
        "attenuation": round(attenuation, 4),
        "attenuation_db": db(attenuation),
        "pan": spatial.map_or(json!(0.0), |s| round(s.pan, 3)),
        "gain_left": spatial.map(|s| round(s.left, 3)),
        "gain_right": spatial.map(|s| round(s.right, 3)),
        "expected_gain": round(expected, 4),
        "level": level_json(v.level.as_ref()),
        "muted": v.muted,
        "soloed": v.soloed,
        "audible": v.audible,
    })
}

/// The mix state: `{"muted": [target...], "soloed": [target...]}`.
pub fn mix_json(world: &World, audio: &Audio) -> Json {
    let mix = audio.mix();
    json!({
        "muted": mix.muted.iter().map(|t| target_json(world, t)).collect::<Vec<_>>(),
        "soloed": mix.soloed.iter().map(|t| target_json(world, t)).collect::<Vec<_>>(),
    })
}

/// Combined RMS of independent voices (energies add).
fn combined_rms<'a>(levels: impl Iterator<Item = &'a VoiceLevel>) -> f32 {
    levels.map(|l| l.rms() * l.rms()).sum::<f32>().sqrt()
}

/// `audio_levels`: the output, listener, bus volumes, solo and mute, and
/// every voice with its level at the listener, loudest first. `filter`
/// keeps only the voices a target applies to. `Err` when the world has no
/// audio.
pub fn levels_json(world: &World, filter: Option<&MixTarget>) -> Result<Json, String> {
    let audio = world
        .get_resource::<Audio>()
        .ok_or("this world has no audio (no AudioPlugin)")?;
    let settings = world
        .get_resource::<AudioSettings>()
        .cloned()
        .unwrap_or_default();
    let mut voices: Vec<&VoiceInfo> = audio
        .voices()
        .filter(|v| filter.is_none_or(|t| t.matches(world, v)))
        .collect();
    let loudness = |v: &VoiceInfo| v.level.map_or(-1.0, |l| l.rms());
    voices.sort_by(|a, b| loudness(b).total_cmp(&loudness(a)).then(a.id.cmp(&b.id)));
    let (lp, lr) = audio.listener();
    let forward = lr * glam::Vec3::NEG_Z;
    let buses: Vec<Json> = AudioBus::ALL
        .into_iter()
        .map(|b| {
            let on_bus: Vec<&&VoiceInfo> = voices.iter().filter(|v| v.bus == b).collect();
            let heard = combined_rms(
                on_bus
                    .iter()
                    .filter(|v| v.audible)
                    .filter_map(|v| v.level.as_ref()),
            );
            json!({
                "bus": bus_name(b),
                "volume": settings.bus(b),
                "voices": on_bus.len(),
                "rms": round(heard, 5),
                "rms_db": db(heard),
            })
        })
        .collect();
    let measured = !matches!(audio.output(), OutputKind::Silent | OutputKind::NotStarted);
    Ok(json!({
        "output": output_name(audio.output()),
        "measured": measured,
        "paused": audio.is_paused(),
        "listener": {
            "position": [lp.x, lp.y, lp.z],
            "forward": [forward.x, forward.y, forward.z],
        },
        "settings": {
            "master": settings.master,
            "effects": settings.effects,
            "ambience": settings.ambience,
            "music": settings.music,
        },
        "mix": mix_json(world, audio),
        "output_level": audio.levels().map(|l| json!({
            "rms": round(l.rms(), 5),
            "rms_db": db(l.rms()),
            "peak": round(l.peak, 5),
            "peak_db": db(l.peak),
        })),
        "buses": buses,
        "voices": voices.iter().map(|v| voice_json(world, audio, &settings, v)).collect::<Vec<_>>(),
    }))
}

/// Which list [`set_mix`] changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixList {
    Solo,
    Mute,
}

/// `audio_solo` / `audio_mute`: turns `target` on or off in a list, or
/// with `clear` empties the list. Returns the change, how many voices the
/// target applies to now, and the mix.
pub fn set_mix(
    world: &mut World,
    list: MixList,
    target: Option<&str>,
    on: bool,
    clear: bool,
) -> Result<Json, String> {
    if !world.contains_resource::<Audio>() {
        return Err("this world has no audio (no AudioPlugin)".into());
    }
    let resolved = match (target, clear) {
        (Some(_), true) => return Err("give `target` or `clear`, not both".into()),
        (None, false) => return Err("`target` is required (or `clear`: true)".into()),
        (Some(t), false) => Some(resolve_target(world, t)?),
        (None, true) => None,
    };
    let mut audio = world.remove_resource::<Audio>().expect("checked above");
    let changed = match &resolved {
        Some(t) => match list {
            MixList::Solo => audio.solo(t.clone(), on),
            MixList::Mute => audio.mute(t.clone(), on),
        },
        None => {
            let targets = match list {
                MixList::Solo => audio.mix().soloed.clone(),
                MixList::Mute => audio.mix().muted.clone(),
            };
            let any = !targets.is_empty();
            for t in targets {
                match list {
                    MixList::Solo => audio.solo(t, false),
                    MixList::Mute => audio.mute(t, false),
                };
            }
            any
        }
    };
    let result = {
        let matching: Vec<String> = match &resolved {
            Some(t) => audio
                .voices()
                .filter(|v| t.matches(world, v))
                .map(|v| voice_label(world, v))
                .collect(),
            None => Vec::new(),
        };
        let mut r = json!({
            "list": match list { MixList::Solo => "solo", MixList::Mute => "mute" },
            "on": on && !clear,
            "cleared": clear,
            "changed": changed,
            "mix": mix_json(world, &audio),
        });
        if let Some(t) = &resolved {
            r["target"] = target_json(world, t);
            r["playing"] = json!(matching);
            if matching.is_empty() {
                r["note"] = json!(
                    "nothing it applies to is playing now; it applies to voices that start later"
                );
            }
        }
        r
    };
    world.insert_resource(audio);
    Ok(result)
}

// ---------------------------------------------------------- report ----

/// Per-source totals of an [`AudioReport`].
#[derive(Clone, Debug, Default)]
struct SourceStats {
    name: String,
    uid: Option<String>,
    clip: String,
    bus: &'static str,
    one_shot: bool,
    plays: u32,
    /// Sample frames measured, and the sums of squares over them.
    samples: u64,
    sum_l: f64,
    sum_r: f64,
    peak: f32,
    loudest_rms: f32,
    seconds: f64,
    heard_seconds: f64,
    muted_seconds: f64,
    first: Option<f64>,
    last: f64,
}

/// Resource: per-source peak and RMS over a run, written as JSON to `path`
/// when the world is dropped (the end of a `box run --audio-report` or
/// `box-player --audio-report` run). Added by [`add_audio_report`].
pub struct AudioReport {
    path: PathBuf,
    frames: u64,
    seconds: f64,
    /// Voice id → key of `sources` (entity uid, or `clip:` + path).
    seen: HashMap<u64, String>,
    sources: HashMap<String, SourceStats>,
    mix_sum: f64,
    mix_seconds: f64,
    mix_peak: f32,
    bus_sum: [f64; 3],
    output: &'static str,
    settings: AudioSettings,
}

impl AudioReport {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            frames: 0,
            seconds: 0.0,
            seen: HashMap::new(),
            sources: HashMap::new(),
            mix_sum: 0.0,
            mix_seconds: 0.0,
            mix_peak: 0.0,
            bus_sum: [0.0; 3],
            output: "not_started",
            settings: AudioSettings::default(),
        }
    }

    /// Where the report is written.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Adds one frame of `world`'s audio (after `update_audio`).
    pub fn record(&mut self, world: &World) {
        let Some(audio) = world.get_resource::<Audio>() else {
            return;
        };
        let dt = world
            .get_resource::<Time>()
            .map_or(0.0, |t| f64::from(t.unscaled_delta));
        self.frames += 1;
        self.seconds += dt;
        self.output = output_name(audio.output());
        if let Some(s) = world.get_resource::<AudioSettings>() {
            self.settings = s.clone();
        }
        if let Some(l) = audio.levels() {
            self.mix_sum += f64::from(l.rms() * l.rms()) * dt;
            self.mix_seconds += dt;
            self.mix_peak = self.mix_peak.max(l.peak);
        }
        let now = self.seconds;
        for v in audio.voices() {
            let key = match self.seen.get(&v.id) {
                Some(k) => k.clone(),
                None => {
                    let uid = v.source.and_then(|e| world.uid(e)).map(|u| u.to_string());
                    let key = match (&uid, v.source) {
                        (Some(u), _) => u.clone(),
                        (None, Some(e)) => format!("entity:{}", e.id()),
                        (None, None) => format!("clip:{}", clip_text(&v.clip)),
                    };
                    self.seen.insert(v.id, key.clone());
                    let s = self
                        .sources
                        .entry(key.clone())
                        .or_insert_with(|| SourceStats {
                            name: voice_label(world, v),
                            uid,
                            clip: clip_text(&v.clip),
                            bus: bus_name(v.bus),
                            one_shot: v.source.is_none(),
                            ..Default::default()
                        });
                    s.plays += 1;
                    key
                }
            };
            let Some(s) = self.sources.get_mut(&key) else {
                continue;
            };
            s.seconds += dt;
            s.last = now;
            if !v.audible {
                s.muted_seconds += dt;
            }
            let Some(l) = v.level.filter(|l| l.frames > 0) else {
                continue;
            };
            let n = f64::from(l.frames);
            s.samples += u64::from(l.frames);
            s.sum_l += f64::from(l.left * l.left) * n;
            s.sum_r += f64::from(l.right * l.right) * n;
            s.peak = s.peak.max(l.peak);
            s.loudest_rms = s.loudest_rms.max(l.rms());
            if l.rms() > 1e-4 {
                s.heard_seconds += dt;
                s.first.get_or_insert(now);
            }
            if v.audible {
                self.bus_sum[v.bus as usize] += f64::from(l.rms() * l.rms()) * dt;
            }
        }
    }

    /// The report as JSON (see docs/guide audio section).
    pub fn to_json(&self) -> Json {
        let mut sources: Vec<(&String, &SourceStats)> = self.sources.iter().collect();
        let rms = |s: &SourceStats| {
            if s.samples == 0 {
                0.0
            } else {
                (((s.sum_l + s.sum_r) / 2.0 / s.samples as f64).sqrt()) as f32
            }
        };
        sources.sort_by(|a, b| rms(b.1).total_cmp(&rms(a.1)).then(a.0.cmp(b.0)));
        let mix_rms = if self.mix_seconds > 0.0 {
            Some((self.mix_sum / self.mix_seconds).sqrt() as f32)
        } else {
            None
        };
        let secs = |v: f64| (v * 1000.0).round() / 1000.0;
        json!({
            "version": 1,
            "frames": self.frames,
            "seconds": secs(self.seconds),
            "output": self.output,
            "measured": self.output == "capture" || self.output == "device",
            "settings": {
                "master": self.settings.master,
                "effects": self.settings.effects,
                "ambience": self.settings.ambience,
                "music": self.settings.music,
            },
            "mix": mix_rms.map(|r| json!({
                "rms": round(r, 5),
                "rms_db": db(r),
                "peak": round(self.mix_peak, 5),
                "peak_db": db(self.mix_peak),
            })),
            "buses": AudioBus::ALL.into_iter().map(|b| {
                let r = if self.seconds > 0.0 {
                    (self.bus_sum[b as usize] / self.seconds).sqrt() as f32
                } else {
                    0.0
                };
                json!({ "bus": bus_name(b), "rms": round(r, 5), "rms_db": db(r) })
            }).collect::<Vec<_>>(),
            "sources": sources.iter().map(|(_, s)| {
                let r = rms(s);
                json!({
                    "name": s.name,
                    "entity": s.uid,
                    "kind": if s.one_shot { "one_shot" } else { "source" },
                    "clip": s.clip,
                    "bus": s.bus,
                    "plays": s.plays,
                    "seconds": secs(s.seconds),
                    "heard_seconds": secs(s.heard_seconds),
                    "muted_seconds": secs(s.muted_seconds),
                    "first_heard": s.first.map(secs),
                    "last_playing": secs(s.last),
                    "rms": round(r, 5),
                    "rms_db": db(r),
                    "peak": round(s.peak, 5),
                    "peak_db": db(s.peak),
                    "loudest_rms_db": db(s.loudest_rms),
                })
            }).collect::<Vec<_>>(),
        })
    }

    /// Writes the report now (also done when the resource is dropped).
    pub fn write(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.to_json()).unwrap_or_default();
        std::fs::write(&self.path, text + "\n")
    }
}

impl Drop for AudioReport {
    fn drop(&mut self) {
        match self.write() {
            Ok(()) => tracing::info!(target: "box_audio", "audio report: {}", self.path.display()),
            Err(e) => tracing::error!(
                target: "box_audio",
                "could not write the audio report {}: {}",
                self.path.display(),
                crate::io_hint::text(&e)
            ),
        }
    }
}

/// The system recording an [`AudioReport`] each frame.
pub fn record_audio_report(world: &mut World) {
    let Some(mut report) = world.remove_resource::<AudioReport>() else {
        return;
    };
    report.record(world);
    world.insert_resource(report);
}

/// Records a per-source audio report over the run, written to `path` when
/// the app's world is dropped. Call after the plugins are built (the
/// recording runs after `update_audio`, at the end of `PostUpdate`).
pub fn add_audio_report(app: &mut App, path: PathBuf) {
    app.world.insert_resource(AudioReport::new(path));
    app.add_system(Stage::PostUpdate, record_audio_report);
}
