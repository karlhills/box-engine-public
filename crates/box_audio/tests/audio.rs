//! box_audio on the capture output (kira mixing in memory, no device):
//! what plays, how loud, where, fades, despawn, pause, settings, hot reload,
//! importer decisions and the silent fallback.

use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use box_assets::{AssetPlugin, AssetServer, LoadState};
use box_audio::{
    Audio, AudioBus, AudioClip, AudioListener, AudioOutput, AudioPlugin, AudioSettings,
    AudioSource, MixTarget, OutputKind, OutputLevels, PlaySound, VoiceInfo, VoiceState, wav,
};
use box_core::asset_ref::AssetRef;
use box_core::prelude::*;

const RATE: u32 = 8000;
const DT: f32 = 1.0 / 60.0;

/// A sine of `amp` at a frequency with a whole number of cycles per second,
/// so a clip of whole seconds loops without a click.
fn sine(seconds: f32, amp: f32) -> Vec<u8> {
    let n = (seconds * RATE as f32) as usize;
    let samples: Vec<f32> = (0..n)
        .map(|i| (i as f32 * 400.0 * TAU / RATE as f32).sin() * amp)
        .collect();
    wav::encode_pcm16(&samples, RATE, 1)
}

struct Fixture {
    dir: PathBuf,
    server: AssetServer,
    app: App,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Fixture {
    /// A project with `audio/tone.wav` (0.5 s), `audio/loop.wav` (1 s) and
    /// `audio/quiet.wav` (1 s at a fifth of the level), and an app with the
    /// asset and audio plugins on `output`.
    fn new(name: &str, output: Option<AudioOutput>) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "box_audio_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        std::fs::write(dir.join("audio/tone.wav"), sine(0.5, 0.5)).unwrap();
        std::fs::write(dir.join("audio/loop.wav"), sine(1.0, 0.5)).unwrap();
        std::fs::write(dir.join("audio/quiet.wav"), sine(1.0, 0.1)).unwrap();
        let server = AssetServer::new(&dir).unwrap();
        box_audio::register_audio_assets(&server);
        let mut app = App::new();
        if let Some(output) = output {
            app.world.insert_resource(output);
        }
        app.add_plugin(&AssetPlugin::new(server.clone()));
        app.add_plugin(&AudioPlugin::new());
        let mut f = Self { dir, server, app };
        for clip in ["audio/tone.wav", "audio/loop.wav", "audio/quiet.wav"] {
            f.server.load::<AudioClip>(&AssetRef::path(clip)).unwrap();
        }
        f.settle();
        f
    }

    fn capture(name: &str) -> Self {
        Self::new(name, Some(AudioOutput::Capture { sample_rate: RATE }))
    }

    fn settle(&mut self) {
        assert!(
            self.server
                .update_until_idle(&mut self.app.world, Duration::from_secs(20)),
            "imports did not finish"
        );
    }

    fn run(&mut self, seconds: f32) {
        let frames = (seconds / DT).round().max(1.0) as usize;
        self.app.run_frames(frames, DT);
    }

    fn audio(&self) -> &Audio {
        self.app.world.resource::<Audio>()
    }

    fn levels(&self) -> OutputLevels {
        self.audio().levels().expect("capture output has levels")
    }
}

fn near(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance * b.abs().max(1e-6)
}

#[test]
fn short_clips_decode_to_memory_and_long_ones_stream() {
    let mut f = Fixture::capture("import");
    std::fs::write(f.dir.join("audio/long.wav"), sine(12.0, 0.2)).unwrap();
    std::fs::write(f.dir.join("audio/broken.ogg"), b"not an ogg file").unwrap();
    f.server.rescan();
    let long = f
        .server
        .load::<AudioClip>(&AssetRef::path("audio/long.wav"))
        .unwrap();
    let broken = f
        .server
        .load::<AudioClip>(&AssetRef::path("audio/broken.ogg"))
        .unwrap();
    f.settle();
    let tone = f
        .server
        .get_handle::<AudioClip>(&AssetRef::path("audio/tone.wav"))
        .unwrap();
    let clips = f.app.world.resource::<Assets<AudioClip>>();
    let (tone, long) = (clips.get(tone).unwrap(), clips.get(long).unwrap());
    assert!(!tone.is_streaming() && near(tone.duration().as_secs_f32(), 0.5, 0.01));
    assert!(long.is_streaming() && near(long.duration().as_secs_f32(), 12.0, 0.01));
    assert!(matches!(
        f.server.handle_state(broken),
        Some(LoadState::Failed(_))
    ));
    assert!(
        clips.get(broken).is_none(),
        "no placeholder for a failed clip"
    );

    // `"stream": "never"` decodes even a long clip.
    let id = f.server.id_of("audio/long.wav").unwrap();
    f.server
        .set_settings(id, serde_json::json!({ "stream": "never" }))
        .unwrap();
    f.settle();
    let h = f
        .server
        .get_handle::<AudioClip>(&AssetRef::path("audio/long.wav"))
        .unwrap();
    assert!(
        !f.app
            .world
            .resource::<Assets<AudioClip>>()
            .get(h)
            .unwrap()
            .is_streaming()
    );

    // Playing a broken clip plays nothing and does not panic.
    f.app.world.send(PlaySound::new("audio/broken.ogg"));
    f.run(0.1);
    assert_eq!(f.audio().voices().count(), 0);
    assert_eq!(f.audio().pending(), 0);
}

#[test]
fn an_ambient_source_fades_in_loops_and_fades_out_on_despawn() {
    let mut f = Fixture::capture("ambient");
    let e = f.app.world.spawn((
        Transform::default(),
        AudioSource {
            fade_in: 0.5,
            fade_out: 0.25,
            ..AudioSource::ambience("audio/loop.wav")
        },
    ));
    f.run(DT);
    assert_eq!(f.audio().output(), OutputKind::Capture);
    let voice = f.audio().source_voice(e).expect("playing").clone();
    assert_eq!(
        (voice.bus, voice.looping, voice.state),
        (AudioBus::Ambience, true, VoiceState::Playing)
    );
    let start = f.levels().rms();
    f.run(0.6);
    let full = f.levels().rms();
    eprintln!("fade-in: first frame rms {start:.4}, after 0.6 s {full:.4}");
    // A 0.5-amplitude sine: RMS ~0.35 per channel at full volume.
    assert!(full > 0.2, "full level {full}");
    assert!(
        start < full * 0.2,
        "fade-in starts quiet: {start} vs {full}"
    );
    // Still playing after the clip's length: it loops.
    f.run(1.5);
    assert!(near(f.levels().rms(), full, 0.1));
    assert!(f.audio().source_voice(e).is_some());

    f.app.world.despawn(e);
    f.run(DT);
    let stopping: Vec<_> = f.audio().voices().map(|v| v.state).collect();
    assert_eq!(stopping, [VoiceState::Stopping]);
    f.run(0.1);
    let mid = f.levels().rms();
    eprintln!("fade-out: rms {mid:.4} after 0.1 s of 0.25 s");
    assert!(mid < full * 0.9 && mid > 0.01, "fading out: {mid}");
    f.run(0.3);
    assert_eq!(f.audio().voices().count(), 0, "stopped after the fade");
    assert!(f.levels().peak < 1e-4);
}

#[test]
fn a_one_shot_plays_once_and_is_removed() {
    let mut f = Fixture::capture("one_shot");
    f.app
        .world
        .send(PlaySound::new("audio/tone.wav").volume(0.5).pitch(2.0));
    f.run(DT);
    let v = f.audio().voices().next().expect("one voice").clone();
    assert_eq!(
        (v.source, v.volume, v.pitch, v.looping),
        (None, 0.5, 2.0, false)
    );
    assert!(f.levels().rms() > 0.05);
    // 0.5 s at double speed lasts 0.25 s.
    f.run(0.3);
    assert_eq!(f.audio().voices().count(), 0);
    assert!(f.levels().peak < 1e-4);
}

#[test]
fn master_and_bus_volumes_scale_the_mix() {
    let mut f = Fixture::capture("volumes");
    f.app.world.spawn((
        Transform::default(),
        AudioSource {
            fade_in: 0.0,
            ..AudioSource::ambience("audio/loop.wav")
        },
    ));
    f.run(0.2);
    let full = f.levels().rms();
    f.app.world.insert_resource(AudioSettings {
        master: 0.5,
        ..Default::default()
    });
    f.run(0.2);
    let half = f.levels().rms();
    eprintln!("master volume: rms {full:.4} -> {half:.4} at 0.5");
    assert!(near(half, full * 0.5, 0.05), "master 0.5: {half} vs {full}");
    f.app.world.insert_resource(AudioSettings {
        ambience: 0.0,
        ..Default::default()
    });
    f.run(0.2);
    assert!(f.levels().peak < 1e-4, "ambience bus muted");
    // Other buses are unaffected.
    f.app.world.send(PlaySound::new("audio/tone.wav"));
    f.run(DT * 3.0);
    assert!(f.levels().rms() > 0.1);
}

#[test]
fn live_volume_and_pitch_edits_glide_on_the_playing_voice() {
    let mut f = Fixture::capture("live_edit");
    let e = f.app.world.spawn((
        Transform::default(),
        AudioSource {
            fade_in: 0.0,
            ..AudioSource::ambience("audio/loop.wav")
        },
    ));
    f.run(0.2);
    let full = f.levels().rms();
    {
        let s = f.app.world.component_mut::<AudioSource>(e).unwrap();
        s.volume = 0.25;
        s.pitch = 1.5;
    }
    f.run(0.2);
    let v = f.audio().source_voice(e).unwrap();
    assert_eq!((v.volume, v.pitch), (0.25, 1.5));
    assert!(near(f.levels().rms(), full * 0.25, 0.05));
    // `playing` off fades out; on again restarts.
    f.app.world.component_mut::<AudioSource>(e).unwrap().playing = false;
    f.run(1.0);
    assert!(f.audio().source_voice(e).is_none());
    assert!(f.levels().peak < 1e-4);
    f.app.world.component_mut::<AudioSource>(e).unwrap().playing = true;
    f.run(0.2);
    assert!(f.audio().source_voice(e).is_some());
}

fn loop_source(volume: f32) -> AudioSource {
    AudioSource {
        fade_in: 0.0,
        fade_out: 0.0,
        volume,
        ..AudioSource::ambience("audio/loop.wav")
    }
}

fn voice(f: &Fixture, e: Entity) -> VoiceInfo {
    f.audio().source_voice(e).expect("playing").clone()
}

/// RMS of the loop clip (a 0.5 sine) as played.
const LOOP_RMS: f32 = 0.5 * std::f32::consts::FRAC_1_SQRT_2;

#[test]
fn each_voice_is_metered_at_the_listener_after_bus_and_master() {
    let mut f = Fixture::capture("meters");
    let loud = f.app.world.spawn((Transform::default(), loop_source(1.0)));
    let soft = f.app.world.spawn((Transform::default(), loop_source(0.25)));
    f.run(0.3);
    let (a, b) = (voice(&f, loud), voice(&f, soft));
    let (la, lb) = (a.level.unwrap(), b.level.unwrap());
    eprintln!("meters: loud {la:?}, soft {lb:?}, mix {:?}", f.levels());
    assert!(la.frames > 0, "measured this frame");
    assert!(near(la.rms(), LOOP_RMS, 0.05), "loud {la:?}");
    assert!(near(lb.rms(), LOOP_RMS * 0.25, 0.05), "soft {lb:?}");
    assert!(near(la.peak, 0.5, 0.05));
    assert!(a.audible && !a.muted && !a.soloed);
    // Both are the same sine in phase: the mix is their sum.
    assert!(near(f.levels().rms(), la.rms() + lb.rms(), 0.05));
    // Bus and master volumes are part of the level.
    f.app.world.insert_resource(AudioSettings {
        master: 0.5,
        ambience: 0.5,
        ..Default::default()
    });
    f.run(0.3);
    let la = voice(&f, loud).level.unwrap();
    assert!(
        near(la.rms(), LOOP_RMS * 0.25, 0.05),
        "bus x master: {la:?}"
    );
}

#[test]
fn positional_voices_report_distance_attenuation_and_pan() {
    let mut f = Fixture::capture("meter_spatial");
    let e = f.app.world.spawn((
        Transform::from_xyz(6.0, 0.0, -2.0),
        AudioSource {
            fade_in: 0.0,
            ..AudioSource::ambience("audio/loop.wav").spatial(2.0, 20.0)
        },
    ));
    f.run(0.3);
    let v = voice(&f, e);
    let s = v.spatial(f.audio().listener()).unwrap();
    let level = v.level.unwrap();
    eprintln!("spatial meter: {s:?}, level {level:?}");
    assert!(near(s.distance, 40f32.sqrt(), 1e-4));
    assert!(s.attenuation > 0.05 && s.attenuation < 1.0);
    assert!(s.pan > 0.2, "on the right");
    // The meter hears what kira mixed: the computed gains match it.
    assert!(near(level.left, LOOP_RMS * s.attenuation * s.left, 0.05));
    assert!(near(level.right, LOOP_RMS * s.attenuation * s.right, 0.05));
    // Non-positional voices have no spatial data.
    let flat = f.app.world.spawn((Transform::default(), loop_source(1.0)));
    f.run(0.1);
    assert!(voice(&f, flat).spatial(f.audio().listener()).is_none());
}

#[test]
fn solo_and_mute_silence_voices_but_keep_their_levels() {
    let mut f = Fixture::capture("solo_mute");
    let group = f.app.world.spawn((Transform::default(),));
    let pond = f.app.world.spawn((Transform::default(), loop_source(1.0)));
    f.app.world.set_parent(pond, Some(group)).unwrap();
    let orchard = f.app.world.spawn((Transform::default(), loop_source(0.25)));
    f.run(0.3);
    let both = f.levels().rms();
    let mute = |f: &mut Fixture, t: MixTarget, on: bool| {
        f.app.world.resource_mut::<Audio>().mute(t, on);
    };
    let solo = |f: &mut Fixture, t: MixTarget, on: bool| {
        f.app.world.resource_mut::<Audio>().solo(t, on);
    };

    // Mute the group: the pond (below it) is silent, its level kept.
    mute(&mut f, MixTarget::Entity(group), true);
    f.run(0.3);
    let p = voice(&f, pond);
    assert!(p.muted && !p.audible);
    assert!(near(p.level.unwrap().rms(), LOOP_RMS, 0.05), "level kept");
    assert!(
        near(f.levels().rms(), LOOP_RMS * 0.25, 0.05),
        "only the orchard"
    );
    mute(&mut f, MixTarget::Entity(group), false);
    f.run(0.3);
    assert!(near(f.levels().rms(), both, 0.05));

    // Solo the orchard: only it plays.
    solo(&mut f, MixTarget::Entity(orchard), true);
    f.run(0.3);
    assert!(!voice(&f, pond).audible && voice(&f, orchard).soloed);
    assert!(near(f.levels().rms(), LOOP_RMS * 0.25, 0.05));
    // Muting a soloed voice silences it: nothing is heard.
    mute(&mut f, MixTarget::Entity(orchard), true);
    f.run(0.3);
    assert!(f.levels().peak < 1e-4);
    f.app.world.resource_mut::<Audio>().clear_mix();
    f.run(0.3);
    assert!(near(f.levels().rms(), both, 0.05));

    // A bus: the effects bus solo silences the ambience loops; a one-shot
    // on it plays, and a clip target catches it.
    solo(&mut f, MixTarget::Bus(AudioBus::Effects), true);
    f.app.world.send(PlaySound::new("audio/tone.wav"));
    f.run(0.1);
    assert!(near(f.levels().rms(), LOOP_RMS, 0.1), "{:?}", f.levels());
    mute(
        &mut f,
        MixTarget::Clip(AssetRef::path("audio/tone.wav")),
        true,
    );
    f.run(0.1);
    assert!(f.levels().peak < 1e-4);
    let mix = f.audio().mix().clone();
    assert_eq!(mix.soloed, vec![MixTarget::Bus(AudioBus::Effects)]);
    assert_eq!(mix.muted.len(), 1);
}

#[test]
fn positional_sounds_pan_and_attenuate_around_the_listener() {
    let mut f = Fixture::capture("spatial");
    // No listener entity: the listener is at the origin facing -Z.
    let at = |x: f32, z: f32| {
        PlaySound::new("audio/loop.wav")
            .at(Vec3::new(x, 0.0, z))
            .distances(1.0, 20.0)
    };
    f.app.world.send(at(3.0, 0.0));
    f.run(0.2);
    let right = f.levels();
    assert!(
        right.right > right.left * 1.5,
        "+X is on the right: {right:?}"
    );
    assert_eq!(
        f.audio().voices().next().unwrap().position,
        Some(Vec3::new(3.0, 0.0, 0.0))
    );
    f.run(1.0); // the one-shot ends

    f.app.world.send(at(-3.0, 0.0));
    f.run(0.2);
    let left = f.levels();
    assert!(left.left > left.right * 1.5, "-X is on the left: {left:?}");
    f.run(1.0);

    f.app.world.send(at(0.0, -15.0));
    f.run(0.2);
    let far = f.levels().rms();
    eprintln!("spatial: right {right:?}, left {left:?}, 15 m rms {far:.4}");
    assert!(
        far < right.rms() * 0.6 && far > 0.0,
        "farther is quieter: {far}"
    );
    f.run(1.0);

    f.app.world.send(at(0.0, -25.0));
    f.run(0.2);
    assert!(f.levels().peak < 1e-4, "silent beyond max_distance");
    f.run(1.0);

    // A listener entity turned around hears +X on its left.
    f.app.world.spawn((
        Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        AudioListener,
    ));
    f.app.world.send(at(3.0, 0.0));
    f.run(0.2);
    let turned = f.levels();
    assert!(
        turned.left > turned.right * 1.5,
        "turned listener: {turned:?}"
    );
}

#[test]
fn a_spatial_source_follows_its_entity() {
    let mut f = Fixture::capture("follow");
    let e = f.app.world.spawn((
        Transform::from_xyz(4.0, 0.0, 0.0),
        AudioSource {
            fade_in: 0.0,
            ..AudioSource::ambience("audio/loop.wav").spatial(1.0, 20.0)
        },
    ));
    f.run(0.2);
    let a = f.levels();
    assert!(a.right > a.left * 1.5, "{a:?}");
    f.app
        .world
        .component_mut::<Transform>(e)
        .unwrap()
        .translation = Vec3::new(-4.0, 0.0, 0.0);
    f.run(0.2);
    let b = f.levels();
    assert!(b.left > b.right * 1.5, "moved to the left: {b:?}");
    assert_eq!(
        f.audio().source_voice(e).unwrap().position,
        Some(Vec3::new(-4.0, 0.0, 0.0))
    );
}

#[test]
fn pausing_silences_and_resuming_continues() {
    let mut f = Fixture::capture("pause");
    let e = f.app.world.spawn((
        Transform::default(),
        AudioSource {
            fade_in: 0.0,
            ..AudioSource::ambience("audio/loop.wav")
        },
    ));
    f.run(0.2);
    let full = f.levels().rms();
    // The game pauses time.
    f.app.world.resource_mut::<Time>().paused = true;
    f.run(0.3);
    assert!(f.audio().is_paused());
    assert!(f.levels().peak < 1e-4, "paused");
    assert!(f.audio().source_voice(e).is_some(), "kept, not stopped");
    f.app.world.resource_mut::<Time>().paused = false;
    f.run(0.3);
    assert!(!f.audio().is_paused());
    assert!(near(f.levels().rms(), full, 0.1));
    // A host pause (editor Play pause) works without the systems running.
    f.app.world.resource_mut::<Audio>().set_paused(true);
    f.run(0.3);
    assert!(f.levels().peak < 1e-4);
    f.app.world.resource_mut::<Audio>().set_paused(false);
    f.run(0.3);
    assert!(f.levels().rms() > full * 0.9);
}

#[test]
fn a_hot_reloaded_clip_restarts_loops_and_reaches_new_one_shots() {
    let mut f = Fixture::capture("reload");
    let e = f.app.world.spawn((
        Transform::default(),
        AudioSource {
            fade_in: 0.0,
            ..AudioSource::ambience("audio/loop.wav")
        },
    ));
    f.run(0.3);
    let before = f.levels().rms();
    let id = f.server.id_of("audio/loop.wav").unwrap();
    f.server.write(id, &sine(1.0, 0.1)).unwrap();
    f.settle();
    f.run(0.3);
    let after = f.levels().rms();
    eprintln!("hot reload: rms {before:.4} -> {after:.4}");
    assert!(
        near(after, before * 0.2, 0.1),
        "reloaded loop: {after} vs {before}"
    );
    assert!(f.audio().source_voice(e).is_some());
    // A one-shot of the reloaded clip plays the new content.
    f.app.world.despawn(e);
    f.run(1.0);
    f.app.world.send(PlaySound::new("audio/loop.wav"));
    f.run(0.2);
    assert!(near(f.levels().rms(), after, 0.1));
}

#[test]
fn a_one_shot_waits_for_its_clip_to_load() {
    let mut f = Fixture::capture("pending");
    std::fs::write(f.dir.join("audio/late.wav"), sine(0.5, 0.5)).unwrap();
    f.server.rescan();
    f.app.world.send(PlaySound::new("audio/late.wav"));
    f.run(DT);
    // Requested on this frame; plays once the import lands.
    f.settle();
    f.run(DT * 2.0);
    assert_eq!(f.audio().pending(), 0);
    assert_eq!(f.audio().voices().count(), 1);
    // A clip that never loads is dropped after the timeout.
    f.app.world.send(PlaySound::new("audio/missing.wav"));
    f.run(box_audio::PENDING_TIMEOUT + 0.5);
    assert_eq!(f.audio().pending(), 0);
}

#[test]
fn without_a_device_choice_nothing_opens_and_voices_are_tracked() {
    // No `AudioOutput` resource and no `enable_device_output()` in this
    // process: silent, no device.
    let mut f = Fixture::new("silent", None);
    assert_eq!(f.audio().output(), OutputKind::NotStarted);
    let e = f.app.world.spawn((
        Transform::default(),
        AudioSource::ambience("audio/loop.wav"),
    ));
    f.app.world.send(PlaySound::new("audio/tone.wav"));
    f.run(DT);
    assert_eq!(f.audio().output(), OutputKind::Silent);
    assert!(f.audio().levels().is_none());
    assert_eq!(f.audio().voices().count(), 2);
    // The one-shot lasts its clip's length; the loop stays.
    f.run(0.6);
    assert_eq!(f.audio().voices().count(), 1);
    assert!(f.audio().source_voice(e).is_some());
    f.app.world.despawn(e);
    f.run(0.6);
    assert_eq!(f.audio().voices().count(), 0);
}

/// Opens the real default device (inaudibly: master volume 0) and reports
/// how long that took. Ignored: CI machines have no device.
/// `cargo test -p box_audio --test audio -- --ignored --nocapture`
#[test]
#[ignore = "needs an audio device"]
fn the_default_device_opens() {
    let mut f = Fixture::new("device", Some(AudioOutput::Device));
    f.app.world.insert_resource(AudioSettings {
        master: 0.0,
        ..Default::default()
    });
    f.app.world.send(PlaySound::new("audio/tone.wav"));
    f.run(DT);
    assert_eq!(
        f.audio().output(),
        OutputKind::Opening,
        "opened off the main thread"
    );
    let started = std::time::Instant::now();
    while f.audio().output() == OutputKind::Opening && started.elapsed().as_secs() < 5 {
        std::thread::sleep(Duration::from_millis(5));
        f.run(DT);
    }
    let audio = f.audio();
    eprintln!(
        "output {:?}, opened in {:?}, voices {}",
        audio.output(),
        audio.startup_time(),
        audio.voices().count()
    );
    assert_eq!(audio.output(), OutputKind::Device);
    assert_eq!(audio.voices().count(), 1);
}

/// Main-thread cost of the audio system with 200 moving positional sources
/// (silent output: kira's mixing runs on its own thread with a device).
/// Prints the mean per frame; run with `--release --nocapture`.
#[test]
fn frame_cost_with_many_moving_sources() {
    let mut f = Fixture::new("cost", Some(AudioOutput::Silent));
    let sources: Vec<Entity> = (0..200)
        .map(|i| {
            f.app.world.spawn((
                Transform::from_xyz(i as f32, 0.0, 0.0),
                AudioSource::ambience("audio/loop.wav").spatial(1.0, 30.0),
            ))
        })
        .collect();
    f.run(DT * 2.0);
    assert_eq!(f.audio().voices().count(), 200);
    let frames = 300;
    let mut spent = Duration::ZERO;
    for frame in 0..frames {
        for (i, e) in sources.iter().enumerate() {
            f.app
                .world
                .component_mut::<Transform>(*e)
                .unwrap()
                .translation
                .z = (frame as f32 * 0.1 + i as f32).sin();
        }
        box_core::transform::propagate_transforms(&mut f.app.world);
        let t = std::time::Instant::now();
        box_audio::update_audio(&mut f.app.world);
        spent += t.elapsed();
    }
    eprintln!(
        "update_audio, 200 moving positional sources: {:.1} µs per frame",
        spent.as_secs_f64() * 1e6 / frames as f64
    );
}
