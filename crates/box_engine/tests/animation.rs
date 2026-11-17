//! M7 animation acceptance: the procedural test character
//! (`examples/animation_assets/models/character.glb`) through the glTF
//! importer, prefab instancing, skin binding, clip sampling, the state
//! machine, scene files and GPU skinning.
//!
//! Rendering tests are skipped when `BOX_SKIP_GPU_TESTS` is set or no GPU
//! adapter exists; their captures are written to `target/captures/m7/`.
//! `cargo test --release -p box_engine --test animation -- --ignored --nocapture`
//! prints the sampling benchmark (100 and 500 characters).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_engine::box_assets::AssetServer;
use box_engine::box_core::animation::{
    AnimationClip, Interpolation, Keyframes, Sample, Skin, animate_players, update_state_machines,
};
use box_engine::box_core::prefab::Instanced;
use box_engine::box_core::scene;
use box_engine::box_core::transform::propagate_transforms;
use box_engine::box_proc::humanoid::{self, JOINTS};
use box_engine::prelude::*;
use box_engine::{WindowSize, app_with_world, settle_assets, shared_engine_registry};
use box_render::{RenderError, SelectionOutline};

const W: u32 = 480;
const H: u32 = 360;
const DT: f32 = 1.0 / 60.0;
const MODEL: &str = "models/character.glb";

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/animation_assets")
}

fn server() -> AssetServer {
    box_engine::asset_server(assets()).unwrap()
}

fn clip_ref(name: &str) -> AssetRef {
    AssetRef::path(&format!("{MODEL}#Animation/{name}"))
}

fn character(world: &mut World, at: Vec3, player: AnimationPlayer) -> Entity {
    world
        .spawn_saved(
            Uid::new(),
            "Character",
            (
                Transform::from_translation(at),
                PrefabInstance {
                    scene: AssetRef::path(&format!("{MODEL}#Scene")),
                },
                player,
            ),
        )
        .unwrap()
}

fn settled_app(world: World) -> App {
    let mut world = world;
    world.insert_resource(server());
    let mut app = app_with_world(world);
    app.world.insert_resource(WindowSize {
        width: W,
        height: H,
        scale_factor_milli: 1000,
    });
    assert!(settle_assets(&mut app.world, Duration::from_secs(30)));
    app
}

/// The descendant of `root` named `name`.
fn joint(world: &World, root: Entity, name: &str) -> Entity {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if world.get::<Name>(e).is_some_and(|n| n.as_str() == name) {
            return e;
        }
        stack.extend(world.children(e));
    }
    panic!("no {name} under {root:?}");
}

fn rotation(world: &World, e: Entity) -> Quat {
    world.get::<Transform>(e).unwrap().rotation
}

#[test]
fn the_committed_character_is_the_generator_output() {
    let committed = std::fs::read(assets().join(MODEL)).unwrap();
    let generated = humanoid::character_glb().unwrap().bytes;
    assert!(
        committed == generated,
        "examples/animation_assets/{MODEL} is stale: run `cargo run -p box_proc --example gen_character`"
    );
}

#[test]
fn imports_the_skin_and_clips() {
    let server = server();
    let mut world = World::new();
    let skin = server
        .load::<Skin>(&AssetRef::path(&format!("{MODEL}#Skin/0")))
        .unwrap();
    let clips: Vec<_> = humanoid::CLIPS
        .iter()
        .map(|(n, _)| server.load::<AnimationClip>(&clip_ref(n)).unwrap())
        .collect();
    let mesh = server
        .load::<MeshData>(&AssetRef::path(&format!("{MODEL}#Mesh/Body")))
        .unwrap();
    assert!(server.update_until_idle(&mut world, Duration::from_secs(30)));

    let skin = world.resource::<Assets<Skin>>().get(skin).unwrap().clone();
    assert_eq!(skin.joint_count(), 14);
    assert_eq!(skin.joint_names, JOINTS);
    assert_eq!(skin.parents[0], None);
    assert_eq!(skin.parents[5], Some(4), "LowerArm.L under UpperArm.L");
    assert_eq!(skin.parents[10], Some(9), "Foot.L under LowerLeg.L");
    // Bind rotations are identity: an inverse bind matrix is a translation
    // by minus the joint's bind position (Head at 0.95 + 0.10 + 0.20 + 0.25).
    let head = skin.inverse_bind_matrices[3];
    assert!(
        head.abs_diff_eq(Mat4::from_translation(Vec3::new(0.0, -1.5, 0.0)), 1e-5),
        "{head}"
    );
    let foot = skin.inverse_bind_matrices[13];
    assert!(foot.abs_diff_eq(Mat4::from_translation(-Vec3::new(-0.1, 0.08, 0.0)), 1e-5));

    let store = world.resource::<Assets<AnimationClip>>();
    for (h, (name, duration)) in clips.iter().zip(humanoid::CLIPS) {
        let clip = store.get(*h).unwrap();
        assert!((clip.duration - duration).abs() < 1e-6, "{name}");
        assert!(clip.targets().iter().all(|t| JOINTS.contains(t)), "{name}");
    }
    let idle = store.get(clips[0]).unwrap();
    let mode = |target: &str| {
        idle.channels
            .iter()
            .find(|c| c.target == target && matches!(c.values, Keyframes::Rotation(_)))
            .unwrap()
            .interpolation
    };
    assert_eq!(mode("Chest"), Interpolation::CubicSpline);
    assert_eq!(mode("Head"), Interpolation::Step);
    assert_eq!(mode("UpperArm.L"), Interpolation::Linear);
    // A step channel holds its key until the next.
    let head = idle.channels.iter().find(|c| c.target == "Head").unwrap();
    assert_eq!(
        head.sample(1.0),
        Sample::Rotation(Quat::from_rotation_y(0.35))
    );

    let mesh = world.resource::<Assets<MeshData>>().get(mesh).unwrap();
    assert_eq!(mesh.joints.len(), mesh.positions.len());
    assert_eq!(mesh.weights.len(), mesh.positions.len());
    assert!(
        mesh.weights
            .iter()
            .all(|w| (w.iter().sum::<f32>() - 1.0).abs() < 1e-5)
    );
}

#[test]
fn prefab_instances_spawn_joints_and_a_skinned_mesh() {
    let mut world = World::with_registry(shared_engine_registry());
    let root = character(&mut world, Vec3::ZERO, AnimationPlayer::default());
    let mut app = settled_app(world);
    app.tick(DT);
    let skinned: Vec<(Entity, SkinnedMesh)> = app
        .world
        .query::<(Entity, &SkinnedMesh)>()
        .iter()
        .map(|(e, s)| (e, s.clone()))
        .collect();
    assert_eq!(skinned.len(), 1);
    let (body, skin) = &skinned[0];
    assert!(app.world.has::<MeshRenderer>(*body));
    assert_eq!(skin.joints.len(), 14);
    for (e, name) in skin.joints.iter().zip(JOINTS) {
        assert_eq!(app.world.get::<Name>(*e).unwrap().as_str(), name);
        assert!(
            app.world.has::<Instanced>(*e),
            "joints are runtime prefab contents"
        );
        assert_eq!(*e, joint(&app.world, root, name));
    }
    // A second instance gets its own joints.
    let other = character(&mut app.world, Vec3::X * 2.0, AnimationPlayer::default());
    app.tick(DT);
    assert_eq!(app.world.count::<&SkinnedMesh>(), 2);
    assert_ne!(joint(&app.world, other, "Hips"), skin.joints[0]);
}

#[test]
fn clips_pose_the_joints_at_known_times() {
    let mut world = World::with_registry(shared_engine_registry());
    let root = character(
        &mut world,
        Vec3::ZERO,
        AnimationPlayer::new(clip_ref("Walk")),
    );
    let mut app = settled_app(world);
    // Each frame advances the clip by its delta (the first included): 15
    // frames reach t = 0.25, a keyframe of the 16-key walk: thigh forward 0.5 rad.
    app.run_frames(15, DT);
    let state_time = app.world.get::<AnimationPlayerState>(root).unwrap().time();
    assert!((state_time - 0.25).abs() < 1e-4, "{state_time}");
    let thigh = rotation(&app.world, joint(&app.world, root, "UpperLeg.L"));
    assert!(
        thigh.angle_between(Quat::from_rotation_x(0.5)) < 1e-3,
        "{thigh}"
    );
    let other = rotation(&app.world, joint(&app.world, root, "UpperLeg.R"));
    assert!(
        other.angle_between(Quat::from_rotation_x(-0.5)) < 1e-3,
        "{other}"
    );
    // Hips translation: bind height minus the bob at a quarter cycle.
    let hips = app
        .world
        .get::<Transform>(joint(&app.world, root, "Hips"))
        .unwrap()
        .translation;
    assert!((hips.y - (0.95 - 0.025 - 0.025)).abs() < 1e-3, "{hips}");
    // The skinned mesh follows: the left foot's world position moved forward (−Z).
    let foot = app
        .world
        .get::<GlobalTransform>(joint(&app.world, root, "Foot.L"))
        .unwrap()
        .translation();
    assert!(foot.z < -0.2, "{foot}");
}

/// Speed and jump schedule used by the state machine tests.
fn locomotion() -> AnimationGraph {
    AnimationGraph {
        states: vec![
            AnimationGraphState::new("Idle", clip_ref("Idle")),
            AnimationGraphState::new("Walk", clip_ref("Walk")),
            AnimationGraphState::new("Run", clip_ref("Run")),
            AnimationGraphState::new("Jump", clip_ref("Jump")).once(),
        ],
        transitions: vec![
            AnimationTransition::new("", "Jump", 0.1).when("jump", ConditionTest::Triggered),
            AnimationTransition::new("Jump", "Idle", 0.2).after(1.0),
            AnimationTransition::new("Idle", "Walk", 0.25)
                .when("speed", ConditionTest::Greater(0.5)),
            AnimationTransition::new("Walk", "Run", 0.25)
                .when("speed", ConditionTest::Greater(3.0)),
            AnimationTransition::new("Run", "Walk", 0.25).when("speed", ConditionTest::Less(3.0)),
            AnimationTransition::new("Walk", "Idle", 0.25).when("speed", ConditionTest::Less(0.5)),
        ],
        start: "Idle".into(),
    }
}

fn machine(app: &mut App, root: Entity) -> &mut AnimationStateMachine {
    app.world
        .component_mut::<AnimationStateMachine>(root)
        .unwrap()
}

fn state(app: &App, root: Entity) -> String {
    app.world
        .get::<AnimationStateMachineState>(root)
        .unwrap()
        .current()
        .to_string()
}

/// Runs idle → walk → run → jump → idle and returns every frame's thigh
/// rotation.
fn run_schedule() -> (Vec<Quat>, Vec<String>) {
    let mut world = World::with_registry(shared_engine_registry());
    let root = character(&mut world, Vec3::ZERO, AnimationPlayer::default());
    world
        .insert_one(root, AnimationStateMachine::new(locomotion()))
        .unwrap();
    let mut app = settled_app(world);
    let thigh = joint(&app.world, root, "UpperLeg.L");
    let mut poses = Vec::new();
    let mut states = Vec::new();
    for frame in 0..300 {
        match frame {
            30 => machine(&mut app, root).set_float("speed", 1.5),
            90 => machine(&mut app, root).set_float("speed", 4.0),
            150 => {
                machine(&mut app, root).set_float("speed", 0.0);
                machine(&mut app, root).trigger("jump");
            }
            _ => {}
        }
        app.tick(DT);
        poses.push(rotation(&app.world, thigh));
        let s = state(&app, root);
        if states.last() != Some(&s) {
            states.push(s);
        }
    }
    (poses, states)
}

#[test]
fn the_state_machine_walks_runs_and_jumps_deterministically() {
    let (poses, states) = run_schedule();
    assert_eq!(states, ["Idle", "Walk", "Run", "Jump", "Idle"]);
    // Crossfades are continuous. The largest step is the 0.1 s fade from
    // Run (thigh back up to 0.85 rad) into Jump's crouch (0.8 rad forward):
    // about 1.65 rad over 6 frames. A cut would move up to 1.65 at once.
    let steps: Vec<f32> = poses.windows(2).map(|w| w[0].angle_between(w[1])).collect();
    let (frame, max_step) = steps
        .iter()
        .copied()
        .enumerate()
        .fold((0, 0.0f32), |a, (i, s)| if s > a.1 { (i, s) } else { a });
    assert!(
        max_step < 0.35,
        "{max_step} at frame {frame}: {:?}",
        &steps[frame.saturating_sub(3)..frame + 3]
    );
    let (again, _) = run_schedule();
    assert_eq!(poses, again, "same inputs, same poses");
}

#[test]
fn players_and_machines_on_prefabs_round_trip_through_scene_files() {
    let mut world = World::with_registry(shared_engine_registry());
    let root = character(
        &mut world,
        Vec3::ZERO,
        AnimationPlayer {
            crossfade: 0.4,
            ..AnimationPlayer::new(clip_ref("Run"))
        },
    );
    let mut m = AnimationStateMachine::new(locomotion());
    m.set_float("speed", 2.0);
    world.insert_one(root, m.clone()).unwrap();
    let mut app = settled_app(world);
    app.run_frames(10, DT);
    let text = scene::save_string(&app.world).unwrap();
    // Only the instance root is saved; the parameters' saved values are what the game set.
    assert!(
        !text.contains("UpperLeg.L"),
        "instanced joints are not saved"
    );
    assert!(!text.contains("SkinBinding"));
    let back = scene::load(text.as_bytes(), shared_engine_registry()).unwrap();
    let e = back.entity_by_uid(app.world.uid(root).unwrap()).unwrap();
    assert_eq!(*back.get::<AnimationStateMachine>(e).unwrap(), m);
    let player = AnimationPlayer::clone(&back.get::<AnimationPlayer>(e).unwrap());
    // The machine entered Walk (speed 2), which set the player's fields.
    assert_eq!(player.clip, clip_ref("Walk"));
    assert_eq!(player.crossfade, 0.25);
}

#[test]
fn graph_files_drive_state_machines() {
    let dir = std::env::temp_dir().join(format!("box_animgraph_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::copy(assets().join(MODEL), dir.join(MODEL)).unwrap();
    let graph = serde_json::json!({
        "states": [
            { "name": "Idle", "clip": format!("{MODEL}#Animation/Idle"), "speed": 1.0, "looping": true },
            { "name": "Run", "clip": format!("{MODEL}#Animation/Run"), "speed": 1.5, "looping": true }
        ],
        "transitions": [
            { "from": "Idle", "to": "Run", "duration": 0.2, "exit_time": 0.0,
              "conditions": [{ "parameter": "fast", "test": "IsTrue" }] }
        ],
        "start": "Idle"
    });
    std::fs::write(
        dir.join("hero.animgraph.json"),
        serde_json::to_vec_pretty(&graph).unwrap(),
    )
    .unwrap();
    let mut world = World::with_registry(shared_engine_registry());
    let root = character(&mut world, Vec3::ZERO, AnimationPlayer::default());
    world
        .insert_one(
            root,
            AnimationStateMachine::from_asset(AssetRef::path("hero.animgraph.json")),
        )
        .unwrap();
    world.insert_resource(box_engine::asset_server(&dir).unwrap());
    let mut app = app_with_world(world);
    assert!(settle_assets(&mut app.world, Duration::from_secs(30)));
    // The graph loads, then the machine starts.
    for _ in 0..200 {
        app.tick(DT);
        if app.world.has::<AnimationStateMachineState>(root) && !state(&app, root).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(state(&app, root), "Idle");
    machine(&mut app, root).set_bool("fast", true);
    app.tick(DT);
    assert_eq!(state(&app, root), "Run");
    let player = AnimationPlayer::clone(&app.world.get::<AnimationPlayer>(root).unwrap());
    assert_eq!(player.clip.label.as_deref(), Some("Animation/Run"));
    assert_eq!(player.speed, 1.5);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------- GPU ----

fn renderer() -> Option<Renderer> {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping");
        return None;
    }
    match Renderer::new_headless(W, H) {
        Ok(r) => Some(r),
        Err(RenderError::NoAdapter) => {
            eprintln!("no GPU adapter; skipping");
            None
        }
        Err(e) => panic!("renderer creation failed: {e}"),
    }
}

fn captures_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m7");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A lit ground, a camera in front of the characters (who face −Z) and a
/// shadowing sun.
fn stage() -> World {
    let mut w = World::with_registry(shared_engine_registry());
    w.spawn((
        Transform::default(),
        Primitive::plane(Vec2::splat(20.0)),
        StandardMaterial::color(Color::hex(0x8a9a78)),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 1.3, -4.5).looking_at(Vec3::new(0.0, 1.1, 0.0), Vec3::Y),
        Camera::default(),
    ));
    w.spawn((
        Transform::default().looking_at(Vec3::new(-0.3, -1.0, 0.5), Vec3::Y),
        DirectionalLight {
            shadows: true,
            ..Default::default()
        },
    ));
    w
}

/// Screen pixel of world point `p` seen by the world's camera.
fn project(world: &World, p: Vec3) -> (u32, u32) {
    let (cam, g) = world
        .query::<(&Camera, &GlobalTransform)>()
        .iter()
        .map(|(c, g)| (*c, *g))
        .next()
        .unwrap();
    let px = cam
        .world_to_viewport(&g, Vec2::new(W as f32, H as f32), p)
        .unwrap()
        .pixels;
    (px.x as u32, px.y as u32)
}

/// Where a hand is: 0.3 m down the lower arm from its joint.
fn hand(world: &World, root: Entity, side: &str) -> Vec3 {
    let g = world
        .get::<GlobalTransform>(joint(world, root, &format!("LowerArm.{side}")))
        .unwrap();
    g.0.transform_point3(Vec3::new(0.0, -0.3, 0.0))
}

fn warm(rgb: [u8; 4]) -> bool {
    rgb[0] as i32 - rgb[2] as i32 > 20
}

#[test]
fn skinned_characters_render_posed_in_every_pass_and_batch() {
    let Some(mut r) = renderer() else { return };
    let mut world = stage();
    let idle = character(
        &mut world,
        Vec3::new(0.7, 0.0, 0.0),
        AnimationPlayer::new(clip_ref("Idle")),
    );
    let jump = character(
        &mut world,
        Vec3::new(-0.7, 0.0, 0.0),
        AnimationPlayer {
            looping: false,
            ..AnimationPlayer::new(clip_ref("Jump"))
        },
    );
    let mut app = settled_app(world);
    // Jump at 0.3 s: arms straight up.
    app.run_frames(19, DT);
    // Outline the jumping character (its skinned mesh) and run the prepass
    // (ambient occlusion is on by default): every pass draws skinned.
    let body = joint(&app.world, jump, "Body");
    app.world.insert_resource(SelectionOutline {
        primary: vec![body],
        ..Default::default()
    });
    r.render(&mut app.world).unwrap();
    let img = r.capture(&mut app.world).unwrap();
    img.save(captures_dir().join("skinned_poses.png")).unwrap();
    let stats = r.stats().clone();
    assert_eq!(stats.validation_errors, 0);
    assert_eq!(stats.skinned, 2);
    assert_eq!(stats.joint_matrices, 28);

    // The jumping character's hands are up beside its head; the idle one's
    // hang by its hips. Skin-coloured pixels prove the vertices moved.
    let px = |p: Vec3| {
        let (x, y) = project(&app.world, p);
        img.get_pixel(x.min(W - 1), y.min(H - 1)).0
    };
    for side in ["L", "R"] {
        let up = hand(&app.world, jump, side);
        assert!(up.y > 1.8, "{up}");
        assert!(warm(px(up)), "jump hand {side} at {up}: {:?}", px(up));
        let down = hand(&app.world, idle, side);
        assert!(down.y < 1.0, "{down}");
        assert!(warm(px(down)), "idle hand {side} at {down}: {:?}", px(down));
        // Where the jumping character's hand would be at rest there is no hand.
        let rest = down - Vec3::new(1.4, 0.0, 0.0);
        assert!(!warm(px(rest)), "nothing at {rest}: {:?}", px(rest));
    }

    // Both characters share mesh, material and pipeline: one instanced draw
    // per material group and pass whatever their poses.
    let one = {
        let mut world = stage();
        character(
            &mut world,
            Vec3::ZERO,
            AnimationPlayer::new(clip_ref("Idle")),
        );
        let mut app = settled_app(world);
        app.run_frames(2, DT);
        r.render(&mut app.world).unwrap();
        r.stats().clone()
    };
    app.world.remove_resource::<SelectionOutline>();
    r.render(&mut app.world).unwrap();
    let two = r.stats().clone();
    assert_eq!(two.draws, one.draws, "{} vs {}", two.draws, one.draws);
    assert!(two.instances > one.instances);
}

// ------------------------------------------------------------ bench ----

/// CPU cost per frame of `n` characters running the locomotion machine at
/// staggered schedule phases: state machines, sampling and transform
/// propagation, in milliseconds (mean over 300 frames after 60 warm-up).
fn sampling_cost(n: usize) -> (f64, f64, f64) {
    let mut world = World::with_registry(shared_engine_registry());
    let columns = (n as f32).sqrt().ceil() as usize;
    let mut roots = Vec::new();
    for i in 0..n {
        let at = Vec3::new((i % columns) as f32 * 1.5, 0.0, (i / columns) as f32 * 1.5);
        let root = character(&mut world, at, AnimationPlayer::default());
        world
            .insert_one(root, AnimationStateMachine::new(locomotion()))
            .unwrap();
        roots.push(root);
    }
    let mut app = settled_app(world);
    app.run_frames(2, DT);
    let (mut machines, mut sampling, mut propagation) = (0.0, 0.0, 0.0);
    for frame in 0..360u32 {
        for (i, &root) in roots.iter().enumerate() {
            let t = (frame as usize + i * 37) % 480;
            let speed = match t {
                0..120 => 0.0,
                120..240 => 1.5,
                _ => 4.0,
            };
            let m = machine(&mut app, root);
            m.set_float("speed", speed);
            if t == 470 {
                m.trigger("jump");
            }
        }
        let time = app.world.resource_mut::<Time>();
        time.delta = DT;
        time.elapsed += DT;
        let a = Instant::now();
        update_state_machines(&mut app.world);
        let b = Instant::now();
        animate_players(&mut app.world);
        let c = Instant::now();
        propagate_transforms(&mut app.world);
        let d = Instant::now();
        if frame >= 60 {
            machines += (b - a).as_secs_f64();
            sampling += (c - b).as_secs_f64();
            propagation += (d - c).as_secs_f64();
        }
    }
    let ms = |s: f64| s * 1000.0 / 300.0;
    (ms(machines), ms(sampling), ms(propagation))
}

#[test]
#[ignore = "benchmark: run in release with --ignored --nocapture"]
fn bench_animation_sampling() {
    for n in [100, 500] {
        let (machines, sampling, propagation) = sampling_cost(n);
        println!(
            "{n} characters: state machines {machines:.3} ms, sampling {sampling:.3} ms, \
             propagation {propagation:.3} ms per frame"
        );
    }
}
