//! The live audio mixer on the capture output: `audio_levels` JSON (levels
//! at the listener, distance, attenuation, pan), solo and mute of entity
//! groups, buses and clips through the control socket, and the per-source
//! audio report of a run (`--audio-report`).

use std::f32::consts::TAU;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use box_engine::Game;
use box_engine::audio_mixer::{self, AudioReport, MixList, resolve_target};
use box_engine::box_audio::{AudioOutput, AudioSettings, AudioSource, MixTarget, PlaySound, wav};
use box_engine::box_core::app::App;
use box_engine::box_core::identity::Uid;
use box_engine::box_core::prelude::*;
use box_engine::control::{ControlClient, ControlHost, ControlServer, HostKind, Mode, Status};
use serde_json::{Value as Json, json};

const RATE: u32 = 8000;
const DT: f32 = 1.0 / 60.0;

fn sine(seconds: f32, amp: f32) -> Vec<u8> {
    let n = (seconds * RATE as f32) as usize;
    let samples: Vec<f32> = (0..n)
        .map(|i| (i as f32 * 400.0 * TAU / RATE as f32).sin() * amp)
        .collect();
    wav::encode_pcm16(&samples, RATE, 1)
}

struct Village {
    dir: PathBuf,
    app: App,
}

impl Drop for Village {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A village: a `Pond` source (spatial, loud), an `Orchard` group with two
/// quieter sources below it, and music; captured at 8 kHz.
fn village(name: &str) -> Village {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("audio_mixer_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(dir.join("audio/pond.wav"), sine(1.0, 0.5)).unwrap();
    std::fs::write(dir.join("audio/leaves.wav"), sine(1.0, 0.2)).unwrap();
    std::fs::write(dir.join("audio/music.wav"), sine(1.0, 0.1)).unwrap();
    std::fs::write(dir.join("audio/chime.wav"), sine(0.5, 0.3)).unwrap();
    let server = box_engine::asset_server(&dir).unwrap();
    let mut world = World::new();
    world.insert_resource(AudioOutput::Capture { sample_rate: RATE });
    world.insert_resource(server.clone());
    let mut app = Game::new().app(world);
    let w = &mut app.world;
    let src = |clip: &str| AudioSource {
        fade_in: 0.0,
        ..AudioSource::ambience(clip)
    };
    w.spawn_saved(
        Uid::new(),
        "Pond",
        (
            Transform::from_xyz(4.0, 0.0, 0.0),
            src("audio/pond.wav").spatial(2.0, 30.0),
        ),
    )
    .unwrap();
    let orchard = w
        .spawn_saved(
            Uid::new(),
            "Orchard",
            (Transform::from_xyz(-6.0, 0.0, -3.0),),
        )
        .unwrap();
    for (i, x) in [-1.0f32, 1.0].into_iter().enumerate() {
        let tree = w
            .spawn_saved(
                Uid::new(),
                &format!("Tree{i}"),
                (
                    Transform::from_xyz(x, 0.0, 0.0),
                    src("audio/leaves.wav").spatial(1.0, 25.0),
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
    for _ in 0..600 {
        app.run_frames(1, DT);
        assert!(server.update_until_idle(&mut app.world, Duration::from_secs(10)));
        if app
            .world
            .resource::<box_engine::box_audio::Audio>()
            .voices()
            .count()
            == 4
        {
            break;
        }
    }
    app.run_frames(12, DT);
    Village { dir, app }
}

fn voice<'a>(levels: &'a Json, name: &str) -> &'a Json {
    levels["voices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap_or_else(|| panic!("no voice {name}: {levels:#}"))
}

fn rms(v: &Json) -> f64 {
    v["level"]["rms"].as_f64().unwrap_or(0.0)
}

#[test]
fn levels_report_each_source_at_the_listener() {
    let v = village("levels");
    let levels = audio_mixer::levels_json(&v.app.world, None).unwrap();
    eprintln!("{levels:#}");
    assert_eq!(levels["output"], "capture");
    assert_eq!(levels["measured"], true);
    assert_eq!(levels["voices"].as_array().unwrap().len(), 4);
    let pond = voice(&levels, "Pond");
    assert_eq!(pond["bus"], "Ambience");
    assert_eq!(pond["kind"], "source");
    assert!((pond["distance"].as_f64().unwrap() - 4.0).abs() < 1e-3);
    assert!(pond["pan"].as_f64().unwrap() > 0.3, "pond is to the right");
    let att = pond["attenuation"].as_f64().unwrap();
    assert!(att > 0.1 && att < 1.0);
    // The measured level is what the attenuation predicts for a 0.5 sine
    // (both ears' gains averaged).
    let expected = 0.5 / 2f64.sqrt()
        * att
        * (pond["gain_left"].as_f64().unwrap() + pond["gain_right"].as_f64().unwrap())
        / 2.0;
    assert!((rms(pond) - expected).abs() < expected * 0.05, "{pond:#}");
    // Loudest first; the orchard trees are to the left.
    assert_eq!(levels["voices"][0]["name"], "Pond");
    assert!(voice(&levels, "Tree0")["pan"].as_f64().unwrap() < 0.0);
    let music = voice(&levels, "Music");
    assert_eq!(music["spatial"], false);
    assert!(music["distance"].is_null());
    assert!((rms(music) - 0.1 / 2f64.sqrt()).abs() < 0.005);
    // Buses: ambience holds three voices.
    assert_eq!(levels["buses"][1]["bus"], "Ambience");
    assert_eq!(levels["buses"][1]["voices"], 3);
    // A filter keeps the orchard's trees only.
    let orchard = resolve_target(&v.app.world, "Orchard").unwrap();
    let only = audio_mixer::levels_json(&v.app.world, Some(&orchard)).unwrap();
    let names: Vec<&str> = only["voices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.iter().all(|n| n.starts_with("Tree")));
}

#[test]
fn targets_resolve_by_name_bus_and_clip() {
    let v = village("targets");
    let w = &v.app.world;
    assert!(matches!(
        resolve_target(w, "Pond"),
        Ok(MixTarget::Entity(_))
    ));
    assert_eq!(
        resolve_target(w, "ambience").unwrap(),
        MixTarget::Bus(box_engine::box_audio::AudioBus::Ambience)
    );
    assert_eq!(
        resolve_target(w, "bus:Music").unwrap(),
        MixTarget::Bus(box_engine::box_audio::AudioBus::Music)
    );
    assert!(matches!(
        resolve_target(w, "audio/pond.wav"),
        Ok(MixTarget::Clip(_))
    ));
    assert!(matches!(
        resolve_target(w, "clip:audio/chime.wav"),
        Ok(MixTarget::Clip(_))
    ));
    let err = resolve_target(w, "pond").unwrap_err();
    assert!(err.contains("did you mean `Pond`"), "{err}");
    let err = resolve_target(w, "Pondd").unwrap_err();
    assert!(err.contains("did you mean `Pond`"), "{err}");
    assert!(err.contains("Orchard"), "{err}");
    assert!(resolve_target(w, "bus:drums").is_err());
}

/// A control host around a playing app (what the player's `--control`
/// host does): `audio_*` act on its world.
struct Host(App);

impl ControlHost for Host {
    fn status(&mut self) -> Status {
        Status {
            mode: Mode::Play,
            scene: None,
            project: None,
            frame: 0,
            undo: None,
        }
    }

    fn play_world(
        &mut self,
        _method: &str,
    ) -> Result<&mut World, box_engine::control::ControlError> {
        Ok(&mut self.0.world)
    }

    fn step_frames(&mut self, frames: u32) -> Result<Json, box_engine::control::ControlError> {
        self.0.run_frames(frames as usize, DT);
        Ok(json!({ "frames": frames }))
    }
}

#[test]
fn solo_and_mute_over_the_control_socket() {
    let mut v = village("control");
    let mut server = ControlServer::bind(HostKind::Player, None).unwrap();
    let app = std::mem::replace(&mut v.app, App::new());
    let mut host = Host(app);
    let discovery = server.discovery();
    let client = thread::spawn(move || {
        let mut c = ControlClient::connect(discovery, Duration::from_secs(10)).unwrap();
        let mut call = |m: &str, p: Json| {
            c.call(m, p)
                .unwrap_or_else(|e| json!({ "error": e.to_control_error().to_json() }))
        };
        let mut r = serde_json::Map::new();
        r.insert("before".into(), call("audio_levels", json!({})));
        r.insert(
            "solo".into(),
            call("audio_solo", json!({ "target": "Orchard" })),
        );
        call("step_frames", json!({ "frames": 12 }));
        r.insert("soloed".into(), call("audio_levels", json!({})));
        r.insert(
            "mute".into(),
            call("audio_mute", json!({ "target": "Tree0" })),
        );
        call("step_frames", json!({ "frames": 12 }));
        r.insert("muted".into(), call("audio_levels", json!({})));
        r.insert("clear".into(), call("audio_solo", json!({ "clear": true })));
        r.insert(
            "unmute".into(),
            call("audio_mute", json!({ "target": "Tree0", "on": false })),
        );
        r.insert(
            "bus".into(),
            call("audio_mute", json!({ "target": "music" })),
        );
        call("step_frames", json!({ "frames": 12 }));
        r.insert(
            "after".into(),
            call("audio_levels", json!({ "target": "bus:music" })),
        );
        r.insert(
            "bad".into(),
            call("audio_solo", json!({ "target": "Nobody" })),
        );
        r.insert(
            "both".into(),
            call("audio_solo", json!({ "target": "Pond", "clear": true })),
        );
        r.insert("none".into(), call("audio_mute", json!({})));
        Json::Object(r)
    });
    let start = Instant::now();
    while !client.is_finished() {
        server.poll(&mut host);
        thread::sleep(Duration::from_millis(1));
        assert!(start.elapsed() < Duration::from_secs(60), "client hung");
    }
    server.poll(&mut host);
    let r = client.join().unwrap();
    v.app = host.0;
    eprintln!("solo: {:#}", r["solo"]);

    let mix = |k: &str| r[k]["output_level"]["rms"].as_f64().unwrap();
    let before = mix("before");
    assert_eq!(r["solo"]["changed"], true);
    assert_eq!(r["solo"]["target"]["name"], "Orchard");
    assert_eq!(r["solo"]["playing"].as_array().unwrap().len(), 2);
    // Only the trees are heard; the pond keeps its level, unheard.
    let pond = voice(&r["soloed"], "Pond");
    assert_eq!(pond["audible"], false);
    assert!(rms(pond) > 0.05);
    assert_eq!(voice(&r["soloed"], "Tree1")["soloed"], true);
    assert!(
        mix("soloed") < before * 0.6,
        "{} vs {before}",
        mix("soloed")
    );
    // Muting one soloed tree leaves the other.
    assert_eq!(voice(&r["muted"], "Tree0")["audible"], false);
    assert_eq!(voice(&r["muted"], "Tree1")["audible"], true);
    assert!(mix("muted") < mix("soloed"));
    assert_eq!(r["clear"]["cleared"], true);
    assert_eq!(r["clear"]["mix"]["soloed"], json!([]));
    // Muting the music bus: the filtered view shows the music voice muted.
    assert_eq!(r["after"]["voices"].as_array().unwrap().len(), 1);
    assert_eq!(r["after"]["voices"][0]["muted"], true);
    assert_eq!(r["after"]["mix"]["muted"][0]["bus"], "Music");
    let code = |k: &str| r[k]["error"]["code"].as_str().unwrap_or("").to_string();
    assert_eq!(code("bad"), "invalid_params");
    assert_eq!(code("both"), "invalid_params");
    assert_eq!(code("none"), "invalid_params");
}

#[test]
fn the_audio_report_sums_each_source_over_the_run() {
    let mut v = village("report");
    let path = v.dir.join("out/audio.json");
    let mut app = std::mem::replace(&mut v.app, App::new());
    audio_mixer::add_audio_report(&mut app, path.clone());
    app.run_frames(30, DT);
    app.world.send(PlaySound::new("audio/chime.wav"));
    // The chime imports on a worker thread in real time while frames run
    // instantly: wait for it, or a busy machine plays it late or not at all.
    app.run_frames(1, DT);
    let server = app
        .world
        .resource::<box_engine::box_assets::AssetServer>()
        .clone();
    assert!(server.update_until_idle(&mut app.world, Duration::from_secs(10)));
    app.run_frames(29, DT);
    // Halfway, the ambience bus is turned down and the orchard muted.
    app.world.insert_resource(AudioSettings {
        ambience: 0.5,
        ..Default::default()
    });
    audio_mixer::set_mix(&mut app.world, MixList::Mute, Some("Orchard"), true, false).unwrap();
    app.run_frames(60, DT);
    assert!(app.world.contains_resource::<AudioReport>());
    drop(app);
    let report: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    eprintln!("{report:#}");
    assert_eq!(report["frames"], 120);
    assert_eq!(report["output"], "capture");
    assert!((report["seconds"].as_f64().unwrap() - 2.0).abs() < 0.01);
    let sources = report["sources"].as_array().unwrap();
    let find = |n: &str| sources.iter().find(|s| s["name"] == n).unwrap();
    let pond = find("Pond");
    assert_eq!(pond["plays"], 1);
    assert!(pond["peak"].as_f64().unwrap() > pond["rms"].as_f64().unwrap());
    assert!(pond["rms_db"].as_f64().unwrap() < 0.0);
    let chime = find("audio/chime.wav");
    assert_eq!(chime["kind"], "one_shot");
    assert!((chime["heard_seconds"].as_f64().unwrap() - 0.5).abs() < 0.05);
    let tree = find("Tree0");
    assert!((tree["muted_seconds"].as_f64().unwrap() - 1.0).abs() < 0.05);
    assert!(report["mix"]["rms"].as_f64().unwrap() > 0.0);
    assert_eq!(report["settings"]["ambience"], 0.5);
}
