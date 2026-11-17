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
use box_engine::box_audio::{Audio, AudioSource, OutputKind, wav};

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
