//! Audio and Play mode: an `AudioSource` in the edit scene plays only in the
//! play world (its clip reaches the play world through the edit world's
//! asset server), pauses with the session and is gone after Stop. The output
//! is silent (no `enable_device_output` in tests), so no device is needed.

use std::path::PathBuf;
use std::time::Duration;

use box_core::identity::Uid;
use box_core::prelude::*;
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, PlayState, build_editor};
use box_engine::box_audio::{
    Audio, AudioOutput, AudioSettings, AudioSource, MixTarget, OutputKind, wav,
};

const DT: f32 = 1.0 / 60.0;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sound_plays_only_in_play_pauses_with_it_and_stops_on_stop() {
    let dir = std::env::temp_dir().join(format!("box_editor_play_audio_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, "audio").unwrap();
    let _cleanup = TempDir(dir.clone());
    let tone: Vec<f32> = (0..8000)
        .map(|i| (i as f32 * 400.0 * std::f32::consts::TAU / 8000.0).sin() * 0.5)
        .collect();
    std::fs::create_dir_all(dir.join("assets/audio")).unwrap();
    std::fs::write(
        dir.join("assets/audio/loop.wav"),
        wav::encode_pcm16(&tone, 8000, 1),
    )
    .unwrap();

    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    let s = &mut editor.state;
    s.autosave_dir = dir.join("autosave");
    // The asset browser lists the clip as a sound.
    let server = s.asset_server().unwrap().clone();
    assert!(
        server.id_of("audio/loop.wav").is_some(),
        "indexed by the audio importer"
    );
    s.world
        .spawn_saved(
            Uid::new(),
            "Wind",
            (
                Transform::default(),
                AudioSource::ambience("audio/loop.wav"),
            ),
        )
        .unwrap();
    assert!(
        !s.world.contains_resource::<Audio>(),
        "the edit world never plays"
    );

    s.start_play().unwrap();
    // The clip is requested by the play world and imported by the edit
    // world's server, then forwarded.
    let mut playing = false;
    for _ in 0..600 {
        s.update_play(DT).unwrap();
        assert!(s.wait_assets_idle(Duration::from_secs(10)));
        if s.world.resource::<Audio>().voices().count() == 1 {
            playing = true;
            break;
        }
    }
    assert!(playing, "the play world's source started");
    let audio = s.world.resource::<Audio>();
    assert_eq!(audio.output(), OutputKind::Silent);
    assert!(!audio.is_paused());

    s.set_play_paused(true);
    assert_eq!(s.play_state(), PlayState::Paused);
    assert!(
        s.world.resource::<Audio>().is_paused(),
        "paused with the session"
    );
    s.step_play();
    assert!(
        s.world.resource::<Audio>().is_paused(),
        "a step stays paused"
    );
    s.set_play_paused(false);
    assert!(!s.world.resource::<Audio>().is_paused());

    s.stop_play();
    assert_eq!(s.play_state(), PlayState::Editing);
    assert!(
        !s.world.contains_resource::<Audio>(),
        "the play world and its output are gone"
    );
}

/// The editor with an `AudioSource` selected (its inspector section) and
/// the `audio` folder open in the asset browser (sound tiles):
/// `target/captures/audio/editor_audio.png`.
#[test]
fn capture_the_audio_source_inspector_and_sound_assets() {
    use box_editor::headless::{CaptureOptions, Headless};
    use box_editor::layout;
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the capture");
        return;
    }
    let dir = std::env::temp_dir().join(format!("box_editor_audio_capture_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, "audio").unwrap();
    let _cleanup = TempDir(dir.clone());
    let clips = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../box_engine/examples/audio_assets/audio");
    std::fs::create_dir_all(dir.join("assets/audio")).unwrap();
    for name in ["ambience.wav", "pickup.wav"] {
        std::fs::copy(clips.join(name), dir.join("assets/audio").join(name)).unwrap();
    }
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.tree = layout::default_layout();
    let uid = Uid::new();
    editor
        .state
        .world
        .spawn_saved(
            uid,
            "Wind",
            (
                Transform::default(),
                AudioSource::ambience("audio/ambience.wav").spatial(2.0, 40.0),
            ),
        )
        .unwrap();
    editor.state.select(Some(uid));
    editor.state.assets.folder = "audio".into();
    assert!(editor.state.wait_assets_idle(Duration::from_secs(10)));
    let capture = CaptureOptions {
        size: [1600, 900],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let mut h = match Headless::new(&mut editor, &capture) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no headless editor ({e:#}); skipping the capture");
            return;
        }
    };
    for _ in 0..capture.frames {
        h.frame(&mut editor, Vec::new());
    }
    assert!(
        !editor.state.world.contains_resource::<Audio>(),
        "editing plays nothing"
    );
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/audio");
    std::fs::create_dir_all(&out).unwrap();
    h.image()
        .unwrap()
        .save(out.join("editor_audio.png"))
        .unwrap();
}

/// A village project: `assets/audio/{pond,leaves,music}.wav` (sines at 0.5,
/// 0.2 and 0.1) and an edit scene with a spatial `Pond`, an `Orchard`
/// group with two spatial trees below it, `Music`, and a `Player` with the
/// `AudioListener`.
fn village(name: &str) -> (TempDir, box_editor::Editor) {
    let dir = std::env::temp_dir().join(format!("box_editor_mixer_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, "village").unwrap();
    std::fs::create_dir_all(dir.join("assets/audio")).unwrap();
    for (file, amp) in [("pond", 0.5f32), ("leaves", 0.2), ("music", 0.1)] {
        let tone: Vec<f32> = (0..8000)
            .map(|i| (i as f32 * 400.0 * std::f32::consts::TAU / 8000.0).sin() * amp)
            .collect();
        std::fs::write(
            dir.join(format!("assets/audio/{file}.wav")),
            wav::encode_pcm16(&tone, 8000, 1),
        )
        .unwrap();
    }
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.state.autosave_dir = dir.join("autosave");
    let w = &mut editor.state.world;
    let src = |clip: &str| AudioSource {
        fade_in: 0.0,
        ..AudioSource::ambience(clip)
    };
    w.spawn_saved(
        Uid::new(),
        "Pond",
        (
            Transform::from_xyz(5.0, 0.2, -4.0),
            src("audio/pond.wav").spatial(2.0, 20.0),
        ),
    )
    .unwrap();
    let orchard = w
        .spawn_saved(
            Uid::new(),
            "Orchard",
            (Transform::from_xyz(-5.0, 0.0, -2.0),),
        )
        .unwrap();
    for (i, (x, z)) in [(-1.5f32, 0.5f32), (1.0, -1.5)].into_iter().enumerate() {
        let tree = w
            .spawn_saved(
                Uid::new(),
                &format!("Tree {}", i + 1),
                (
                    Transform::from_xyz(x, 1.0, z),
                    src("audio/leaves.wav").spatial(1.0, 14.0),
                ),
            )
            .unwrap();
        w.set_parent(tree, Some(orchard)).unwrap();
    }
    w.spawn_saved(
        Uid::new(),
        "Music",
        (
            Transform::default(),
            src("audio/music.wav").with_bus(box_engine::box_audio::AudioBus::Music),
        ),
    )
    .unwrap();
    // The player: hears instead of the camera.
    w.spawn_saved(
        Uid::new(),
        "Player",
        (
            Transform::from_xyz(-0.5, 1.2, -1.5),
            box_engine::box_audio::AudioListener,
        ),
    )
    .unwrap();
    (TempDir(dir), editor)
}

/// Starts Play on the capture output and runs until every source plays.
fn play_village(s: &mut box_editor::EditorState) {
    s.start_play().unwrap();
    s.world
        .insert_resource(AudioOutput::Capture { sample_rate: 8000 });
    for _ in 0..600 {
        s.update_play(DT).unwrap();
        assert!(s.wait_assets_idle(Duration::from_secs(10)));
        if s.world.resource::<Audio>().voices().count() == 4 {
            break;
        }
    }
    assert_eq!(s.world.resource::<Audio>().voices().count(), 4);
    for _ in 0..12 {
        s.update_play(DT).unwrap();
    }
}

fn run(s: &mut box_editor::EditorState, frames: usize) {
    for _ in 0..frames {
        s.update_play(DT).unwrap();
        assert!(s.wait_assets_idle(Duration::from_secs(10)));
    }
}

fn level_of(s: &box_editor::EditorState, name: &str) -> f32 {
    let e = s.world.find_by_name(name).unwrap();
    s.world
        .resource::<Audio>()
        .source_voice(e)
        .and_then(|v| v.level)
        .map_or(0.0, |l| l.rms())
}

/// Tuning levels while replaying: inspector edits of `AudioSource.volume`,
/// the Mixer's faders (written to the settings file) and outside edits of
/// `AudioSettings.json` (hot reload) all apply to the playing sound at
/// once; solo and mute are runtime only and gone after Stop.
#[test]
fn levels_change_live_during_play_and_the_mix_ends_with_stop() {
    let (dir, mut editor) = village("live");
    let s = &mut editor.state;
    play_village(s);
    let music = level_of(s, "Music");
    assert!((music - 0.1 / 2f32.sqrt()).abs() < 0.005, "music {music}");

    // An inspector edit of the playing source's volume.
    let uid = s.world.uid(s.world.find_by_name("Music").unwrap()).unwrap();
    assert!(s.set_field(uid, "AudioSource.volume", &0.5f32));
    run(s, 12);
    let half = level_of(s, "Music");
    assert!(
        (half - music * 0.5).abs() < music * 0.05,
        "{half} vs {music}"
    );

    // The Mixer's music fader: live, and written to the settings file.
    assert!(s.set_settings_field("AudioSettings", "AudioSettings.music", &0.5f32, 0.0));
    s.flush_settings_edit(f64::INFINITY, true);
    run(s, 12);
    let quarter = level_of(s, "Music");
    assert!((quarter - music * 0.25).abs() < music * 0.05, "{quarter}");
    let file = dir.0.join("assets/settings/AudioSettings.json");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved["music"], 0.5);

    // The settings file edited outside the editor hot-reloads into Play.
    let mut edited = saved.clone();
    edited["music"] = serde_json::json!(1.0);
    edited["ambience"] = serde_json::json!(0.25);
    let pond_before = level_of(s, "Pond");
    std::fs::write(&file, serde_json::to_string_pretty(&edited).unwrap()).unwrap();
    let mut reloaded = false;
    for _ in 0..600 {
        run(s, 1);
        if s.world.resource::<AudioSettings>().ambience == 0.25 {
            reloaded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        reloaded,
        "the settings file hot-reloaded into the play world"
    );
    run(s, 12);
    assert!((level_of(s, "Music") - music * 0.5).abs() < music * 0.05);
    let pond = level_of(s, "Pond");
    assert!(
        (pond - pond_before * 0.25).abs() < pond_before * 0.05,
        "{pond} vs {pond_before}"
    );

    // Solo the orchard group from the selection: only the trees are heard.
    let orchard = s.world.find_by_name("Orchard").unwrap();
    assert!(s.audio_solo(MixTarget::Entity(orchard), true));
    run(s, 6);
    let audio = s.world.resource::<Audio>();
    let heard: Vec<bool> = ["Pond", "Tree 1", "Tree 2", "Music"]
        .iter()
        .map(|n| {
            audio
                .source_voice(s.world.find_by_name(n).unwrap())
                .unwrap()
                .audible
        })
        .collect();
    assert_eq!(heard, [false, true, true, false]);
    assert!(s.audio_mute(MixTarget::Bus(box_engine::box_audio::AudioBus::Music), true));

    s.stop_play();
    assert!(!s.world.contains_resource::<Audio>());
    assert!(
        !s.audio_solo(MixTarget::Entity(orchard), true),
        "no mix outside Play"
    );
    // The volume edit was a Play edit: the scene still has volume 1.
    let e = s.world.find_by_name("Music").unwrap();
    assert_eq!(s.world.get::<AudioSource>(e).unwrap().volume, 1.0);
    // A new session starts with an empty mix.
    s.start_play().unwrap();
    run(s, 2);
    assert!(s.world.resource::<Audio>().mix().is_empty());
    s.stop_play();
}

/// The Mixer during Play (bus faders, source rows with levels, the
/// orchard soloed) over the free-camera view with Audio Ranges on:
/// `target/captures/audio/mixer.png`; and the ranges while editing:
/// `target/captures/audio/audio_ranges_edit.png`.
#[test]
fn capture_the_mixer_and_audio_ranges() {
    use box_editor::headless::{CaptureOptions, Headless};
    use box_editor::layout;
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the capture");
        return;
    }
    let (_dir, mut editor) = village("capture");
    editor.tree = layout::default_layout();
    editor.state.camera.orbit.target = glam::Vec3::new(0.0, 0.0, -2.0);
    editor.state.camera.orbit.distance = 26.0;
    editor.state.camera.orbit.yaw = 0.35;
    editor.state.camera.orbit.pitch = 0.75;
    editor.state.viewport.view.audio = true;
    assert!(editor.state.wait_assets_idle(Duration::from_secs(10)));
    let capture = CaptureOptions {
        size: [1920, 1080],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let mut h = match Headless::new(&mut editor, &capture) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("no headless editor ({e:#}); skipping the capture");
            return;
        }
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/audio");
    std::fs::create_dir_all(&out).unwrap();
    for _ in 0..capture.frames {
        h.frame(&mut editor, Vec::new());
    }
    h.image()
        .unwrap()
        .save(out.join("audio_ranges_edit.png"))
        .unwrap();

    editor.state.start_play().unwrap();
    editor
        .state
        .world
        .insert_resource(AudioOutput::Capture { sample_rate: 8000 });
    for _ in 0..600 {
        h.frame(&mut editor, Vec::new());
        assert!(editor.state.wait_assets_idle(Duration::from_secs(10)));
        if editor.state.world.resource::<Audio>().voices().count() == 4 {
            break;
        }
    }
    let s = &mut editor.state;
    s.set_free_camera(true);
    let orchard = s.world.find_by_name("Orchard").unwrap();
    s.audio_solo(MixTarget::Entity(orchard), true);
    s.set_mixer(true);
    h.enable_accesskit();
    for _ in 0..6 {
        h.frame(&mut editor, Vec::new());
    }
    let image = h.image().unwrap();
    image.save(out.join("mixer.png")).unwrap();
    // The Mixer lists every source by name.
    let labels: Vec<String> = h
        .accesskit_update()
        .map(|u| {
            u.nodes
                .iter()
                .filter_map(|(_, n)| n.label().or(n.value()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for name in ["Mixer", "Pond", "Tree 1", "Tree 2", "Music", "Ambience"] {
        assert!(
            labels.iter().any(|l| l == name),
            "{name} not shown: {labels:?}"
        );
    }
    editor.state.stop_play();
}
