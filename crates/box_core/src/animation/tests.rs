use glam::{Quat, Vec3};

use super::*;
use crate::app::App;
use crate::asset_ref::{AssetId, AssetRef};
use crate::assets::{AssetLookup, Assets};
use crate::identity::{Name, Uid};
use crate::reflect::Registry;
use crate::transform::Transform;
use crate::world::{Entity, World};

const DT: f32 = 1.0 / 60.0;

/// A clip moving `Bone` along x from 0 to `to` over one second.
fn slide(to: f32) -> AnimationClip {
    AnimationClip::new(vec![Channel {
        target: "Bone".into(),
        interpolation: Interpolation::Linear,
        times: vec![0.0, 1.0],
        values: Keyframes::Translation(vec![Vec3::ZERO, Vec3::new(to, 0.0, 0.0)]),
    }])
}

fn spin() -> AnimationClip {
    AnimationClip::new(vec![Channel {
        target: "Bone".into(),
        interpolation: Interpolation::Step,
        times: vec![0.0, 0.5],
        values: Keyframes::Rotation(vec![Quat::IDENTITY, Quat::from_rotation_y(1.0)]),
    }])
}

/// Adds `clip` as `anim.glb#Animation/<name>` and returns its reference.
fn add_clip(world: &mut World, name: &str, clip: AnimationClip) -> AssetRef {
    let handle = world
        .resource_or_default::<Assets<AnimationClip>>()
        .add(clip);
    let label = format!("Animation/{name}");
    world.resource_or_default::<AssetLookup>().insert(
        AssetId::new(),
        Some(&label),
        Some("anim.glb"),
        handle,
    );
    AssetRef::path(&format!("anim.glb#{label}"))
}

/// An app with the animation systems and a player with a `Bone` child.
fn rig() -> (App, Entity, Entity) {
    let mut app = App::new();
    app.add_plugin(&AnimationPlugin);
    app.check_order().unwrap();
    let root = app.world.spawn((Transform::IDENTITY, Name::new("Hero")));
    let bone = app.world.spawn((Transform::IDENTITY, Name::new("Bone")));
    app.world.set_parent(bone, Some(root)).unwrap();
    (app, root, bone)
}

fn x(app: &App, e: Entity) -> f32 {
    app.world.get::<Transform>(e).unwrap().translation.x
}

#[test]
fn players_pose_their_targets_at_known_times() {
    let (mut app, root, bone) = rig();
    let walk = add_clip(&mut app.world, "Walk", slide(2.0));
    app.world
        .insert_one(root, AnimationPlayer::new(walk))
        .unwrap();
    app.run_frames(30, DT);
    assert!((x(&app, bone) - 1.0).abs() < 1e-4, "{}", x(&app, bone));
    // The world transform follows in the same frame.
    let gx = app
        .world
        .get::<crate::transform::GlobalTransform>(bone)
        .unwrap()
        .translation()
        .x;
    assert!((gx - 1.0).abs() < 1e-4);
    // Looping wraps; one-shot holds the last frame.
    app.run_frames(45, DT);
    assert!((x(&app, bone) - 0.5).abs() < 1e-3, "{}", x(&app, bone));
    app.world
        .component_mut::<AnimationPlayer>(root)
        .unwrap()
        .looping = false;
    app.run_frames(60, DT);
    assert_eq!(x(&app, bone), 2.0);
    assert!(
        app.world
            .get::<AnimationPlayerState>(root)
            .unwrap()
            .finished()
    );
    // Paused: time stands still.
    app.world
        .component_mut::<AnimationPlayer>(root)
        .unwrap()
        .paused = true;
    let t = app.world.get::<AnimationPlayerState>(root).unwrap().time();
    app.run_frames(10, DT);
    assert_eq!(
        app.world.get::<AnimationPlayerState>(root).unwrap().time(),
        t
    );
}

#[test]
fn crossfades_blend_by_elapsed_fraction() {
    let (mut app, root, bone) = rig();
    let a = add_clip(&mut app.world, "A", slide(0.0));
    let b = add_clip(&mut app.world, "B", slide(4.0));
    app.world
        .insert_one(root, AnimationPlayer::new(a.clone()))
        .unwrap();
    app.run_frames(1, DT);
    app.world
        .component_mut::<AnimationPlayer>(root)
        .unwrap()
        .crossfade_to(b.clone(), 0.5);
    app.run_frames(15, DT); // a quarter second into a half-second fade
    let layers = app
        .world
        .get::<AnimationPlayerState>(root)
        .unwrap()
        .layers();
    assert_eq!(layers.len(), 2);
    assert_eq!((&layers[0].clip, &layers[1].clip), (&a, &b));
    assert!((layers[0].weight - 0.5).abs() < 1e-4, "{layers:?}");
    assert!((layers[1].weight - 0.5).abs() < 1e-4);
    // B is at 0.25 s (x = 1), A is at rest (x = 0): halfway is 0.5.
    assert!((x(&app, bone) - 0.5).abs() < 1e-3, "{}", x(&app, bone));
    app.run_frames(15, DT);
    let layers = app
        .world
        .get::<AnimationPlayerState>(root)
        .unwrap()
        .layers();
    assert_eq!(layers.len(), 1, "a finished fade drops the old clip");
    assert_eq!(layers[0].weight, 1.0);
    assert!((x(&app, bone) - 2.0).abs() < 1e-3);
}

#[test]
fn a_zero_crossfade_cuts_and_rotations_blend() {
    let (mut app, root, bone) = rig();
    let a = add_clip(&mut app.world, "A", slide(1.0));
    let s = add_clip(&mut app.world, "Spin", spin());
    let mut player = AnimationPlayer::new(a);
    player.crossfade = 0.0;
    player.looping = false;
    app.world.insert_one(root, player).unwrap();
    app.run_frames(2, DT);
    app.world
        .component_mut::<AnimationPlayer>(root)
        .unwrap()
        .clip = s;
    app.run_frames(40, DT);
    let t = *app.world.get::<Transform>(bone).unwrap();
    assert!(t.rotation.angle_between(Quat::from_rotation_y(1.0)) < 1e-5);
    // Translation is not animated by the new clip: it keeps its last value.
    assert!(t.translation.x > 0.0);
}

#[test]
fn targets_appearing_later_are_bound() {
    let mut app = App::new();
    app.add_plugin(&AnimationPlugin);
    let root = app.world.spawn((Transform::IDENTITY,));
    let walk = add_clip(&mut app.world, "Walk", slide(2.0));
    app.world
        .insert_one(root, AnimationPlayer::new(walk))
        .unwrap();
    app.run_frames(3, DT);
    // A prefab instanced late: the bone appears under the player.
    let bone = app.world.spawn((Transform::IDENTITY, Name::new("Bone")));
    app.world.set_parent(bone, Some(root)).unwrap();
    app.run_frames(3, DT);
    assert!(x(&app, bone) > 0.0);
    // Respawned (a prefab reload): bound again.
    app.world.despawn(bone);
    let again = app.world.spawn((Transform::IDENTITY, Name::new("Bone")));
    app.world.set_parent(again, Some(root)).unwrap();
    app.run_frames(2, DT);
    assert!(x(&app, again) > 0.0);
}

#[test]
fn playback_is_deterministic() {
    let run = || {
        let (mut app, root, bone) = rig();
        let a = add_clip(&mut app.world, "A", slide(3.0));
        let b = add_clip(&mut app.world, "B", spin());
        app.world.insert_one(root, AnimationPlayer::new(a)).unwrap();
        let mut out = Vec::new();
        for i in 0..90 {
            if i == 40 {
                app.world
                    .component_mut::<AnimationPlayer>(root)
                    .unwrap()
                    .clip = b.clone();
            }
            app.run_frames(1, DT);
            out.push(*app.world.get::<Transform>(bone).unwrap());
        }
        out
    };
    assert_eq!(run(), run());
}

fn machine_graph(idle: AssetRef, walk: AssetRef, jump: AssetRef) -> AnimationGraph {
    AnimationGraph {
        states: vec![
            AnimationGraphState::new("Idle", idle),
            AnimationGraphState::new("Walk", walk).with_speed(2.0),
            AnimationGraphState::new("Jump", jump).once(),
        ],
        transitions: vec![
            AnimationTransition::new("Idle", "Walk", 0.2)
                .when("speed", ConditionTest::Greater(0.5)),
            AnimationTransition::new("Walk", "Idle", 0.2).when("speed", ConditionTest::Less(0.5)),
            AnimationTransition::new("", "Jump", 0.1).when("jump", ConditionTest::Triggered),
            AnimationTransition::new("Jump", "Idle", 0.2)
                .when("grounded", ConditionTest::IsTrue)
                .after(1.0),
        ],
        start: "Idle".into(),
    }
}

fn current(app: &App, e: Entity) -> String {
    app.world
        .get::<AnimationStateMachineState>(e)
        .unwrap()
        .current()
        .to_string()
}

#[test]
fn state_machines_follow_parameters_and_exit_times() {
    let (mut app, root, _) = rig();
    let idle = add_clip(&mut app.world, "Idle", slide(0.0));
    let walk = add_clip(&mut app.world, "Walk", slide(1.0));
    let jump = add_clip(&mut app.world, "Jump", slide(5.0));
    let graph = machine_graph(idle.clone(), walk.clone(), jump.clone());
    let mut machine = AnimationStateMachine::new(graph);
    machine.set_bool("grounded", true);
    app.world.insert_one(root, machine).unwrap();
    app.run_frames(1, DT);
    assert_eq!(current(&app, root), "Idle");
    assert!(app.world.has::<AnimationPlayer>(root), "a player is added");
    assert_eq!(app.world.get::<AnimationPlayer>(root).unwrap().clip, idle);

    let set = |app: &mut App, f: &dyn Fn(&mut AnimationStateMachine)| {
        f(app
            .world
            .component_mut::<AnimationStateMachine>(root)
            .unwrap());
    };
    set(&mut app, &|m| m.set_float("speed", 1.0));
    app.run_frames(1, DT);
    assert_eq!(current(&app, root), "Walk");
    let player = AnimationPlayer::clone(&app.world.get::<AnimationPlayer>(root).unwrap());
    assert_eq!(
        (player.clip, player.speed, player.crossfade),
        (walk, 2.0, 0.2)
    );

    // A trigger fires from any state once, and is reset.
    set(&mut app, &|m| m.trigger("jump"));
    app.run_frames(1, DT);
    assert_eq!(current(&app, root), "Jump");
    let m = AnimationStateMachine::clone(&app.world.get::<AnimationStateMachine>(root).unwrap());
    assert_eq!(m.parameter("jump"), Some(ParameterValue::Trigger(false)));
    assert!(!app.world.get::<AnimationPlayer>(root).unwrap().looping);
    // Jump -> Idle waits for the clip's end although `grounded` holds.
    app.run_frames(30, DT);
    assert_eq!(current(&app, root), "Jump");
    app.run_frames(40, DT);
    assert_eq!(
        current(&app, root),
        "Walk",
        "Idle, then Walk at once (speed > 0.5)"
    );
    let transitions = app
        .world
        .get::<AnimationStateMachineState>(root)
        .unwrap()
        .transitions();
    assert_eq!(transitions, 4);
}

#[test]
fn players_and_machines_round_trip_through_scene_files() {
    let registry = std::sync::Arc::new(Registry::with_core_types());
    let mut world = World::with_registry(registry.clone());
    let walk = AssetRef::path("models/hero.glb#Animation/Walk");
    let mut machine = AnimationStateMachine::new(machine_graph(
        AssetRef::path("models/hero.glb#Animation/Idle"),
        walk.clone(),
        AssetRef::path("models/hero.glb#Animation/Jump"),
    ));
    machine.set_float("speed", 0.25);
    machine.trigger("jump");
    let player = AnimationPlayer {
        clip: walk,
        speed: 1.5,
        looping: false,
        paused: true,
        crossfade: 0.4,
    };
    let joint = Uid::new();
    let binding = SkinBinding {
        skin: AssetRef::path("models/hero.glb#Skin/0"),
        joints: vec![joint],
    };
    let e = world
        .spawn_saved(
            Uid::new(),
            "Hero",
            (player.clone(), machine.clone(), binding.clone()),
        )
        .unwrap();
    let text = crate::scene::save_string(&world).unwrap();
    let back = crate::scene::load(text.as_bytes(), registry).unwrap();
    let uid = world.uid(e).unwrap();
    let e2 = back.entity_by_uid(uid).unwrap();
    assert_eq!(*back.get::<AnimationPlayer>(e2).unwrap(), player);
    assert_eq!(*back.get::<AnimationStateMachine>(e2).unwrap(), machine);
    assert_eq!(*back.get::<SkinBinding>(e2).unwrap(), binding);
    // Joint uids are plain strings in the file, so prefab instancing remaps them.
    assert!(text.contains(&format!("\"{joint}\"")), "{text}");
}
