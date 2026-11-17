//! System ergonomics: `on_add`, `on_remove`, `on_event`, `World::send`,
//! deferred commands applied between systems, ordering by name, time scale
//! and pause.

use box_core::app::OrderError;
use box_core::prelude::*;

#[derive(Default)]
struct Log(Vec<String>);

fn log(w: &mut World, s: impl Into<String>) {
    w.resource_or_default::<Log>().0.push(s.into());
}

fn logged(app: &App) -> Vec<String> {
    app.world
        .get_resource::<Log>()
        .map(|l| l.0.clone())
        .unwrap_or_default()
}

struct Enemy(u32);
struct Brain;

fn hooked_app() -> App {
    let mut app = App::new();
    app.on_add::<Enemy>(|w, e| {
        let id = w.get::<Enemy>(e).unwrap().0;
        log(w, format!("add {id}"));
        let _ = w.insert_one(e, Brain);
    });
    app.on_remove::<Enemy>(|w, e| {
        log(w, format!("remove alive={}", w.contains(e)));
    });
    app
}

#[test]
fn on_add_runs_once_per_entity_however_it_got_the_component() {
    let mut app = hooked_app();
    // Spawned before the first frame: initialised before any system runs.
    let a = app.world.spawn((Enemy(1),));
    app.add_system(Stage::Update, move |w| {
        assert!(w.has::<Brain>(a), "on_add ran before the first system");
    });
    // Spawned by a system: initialised before the next system.
    app.add_system(Stage::Update, |w| {
        if w.resource::<Time>().frame == 1 {
            let e = w.spawn((Enemy(2),));
            w.insert_resource(e);
        }
    });
    app.add_system(Stage::Update, |w| {
        if let Some(&e) = w.get_resource::<Entity>() {
            assert!(w.has::<Brain>(e), "initialised between systems");
        }
    });
    app.run_frames(3, 1.0 / 60.0);
    assert_eq!(logged(&app), ["add 1", "add 2"]);

    // Inserted later, spawned through `defer`, and removed then re-added on
    // separate frames: each counts.
    let plain = app.world.spawn((Transform::default(),));
    app.world.insert_one(plain, Enemy(3)).unwrap();
    app.world.defer().spawn((Enemy(4),));
    app.run_frames(1, 1.0 / 60.0);
    app.world.remove_one::<Enemy>(a).unwrap();
    app.run_frames(1, 1.0 / 60.0);
    app.world.insert_one(a, Enemy(5)).unwrap();
    app.run_frames(2, 1.0 / 60.0);
    // Entities found in one pass run in storage order, not spawn order.
    let mut log = logged(&app);
    log[2..4].sort();
    assert_eq!(
        log,
        [
            "add 1",
            "add 2",
            "add 3",
            "add 4",
            "remove alive=true",
            "add 5"
        ]
    );
    assert_eq!(app.world.count::<&Brain>(), 4);
}

#[test]
fn on_remove_sees_removals_and_recursive_despawns() {
    let mut app = hooked_app();
    let parent = app.world.spawn((Enemy(1),));
    let child = app.world.spawn((Enemy(2),));
    let plain = app.world.spawn((Transform::default(),));
    app.world.set_parent(child, Some(parent)).unwrap();
    app.world.set_parent(plain, Some(child)).unwrap();
    app.update();
    // Despawning the parent takes the child (and its Enemy) with it.
    app.world.despawn(parent);
    app.update();
    let removes = logged(&app)
        .iter()
        .filter(|l| l.starts_with("remove"))
        .count();
    assert_eq!(removes, 2);
    assert!(logged(&app).contains(&"remove alive=false".to_string()));
    // Despawned during a system through defer, while iterating.
    let es: Vec<Entity> = (0..5).map(|i| app.world.spawn((Enemy(10 + i),))).collect();
    app.add_system(Stage::Update, |w| {
        for (e, enemy) in w.query::<(Entity, &Enemy)>().iter() {
            if enemy.0 % 2 == 0 {
                w.defer().despawn(e);
            }
        }
    });
    app.update();
    assert_eq!(app.world.count::<&Enemy>(), 2);
    assert!(!app.world.contains(es[0]) && app.world.contains(es[1]));
    let removes = logged(&app)
        .iter()
        .filter(|l| l.starts_with("remove"))
        .count();
    assert_eq!(removes, 2 + 3);
}

#[test]
fn deferred_commands_apply_before_the_next_system() {
    let mut app = App::new();
    let a = app.world.spawn((Enemy(0),));
    let b = app.world.spawn((Enemy(1),));
    app.world.set_parent(b, Some(a)).unwrap();
    app.add_system(Stage::Update, move |w| {
        for (e, _) in w.query::<(Entity, &Enemy)>().iter() {
            if e == a {
                w.defer().despawn(e);
                let spawned = w.defer().spawn((Brain, Transform::default()));
                w.defer().set_parent(spawned, None);
            }
        }
        assert!(w.contains(a), "not applied while the system runs");
    });
    app.add_system(Stage::Update, move |w| {
        assert!(
            !w.contains(a) && !w.contains(b),
            "recursive despawn applied"
        );
        assert_eq!(w.count::<&Brain>(), 1);
        log(w, "checked");
    });
    app.update();
    assert_eq!(logged(&app), ["checked"]);
}

#[derive(Clone, Debug, PartialEq)]
struct Ping(u64);

#[test]
fn on_event_sees_every_event_once_at_any_frame_rate_and_stage() {
    for hz in [30.0_f32, 60.0, 144.0] {
        let mut app = App::new();
        app.world
            .insert_resource((Vec::<u64>::new(), Vec::<u64>::new()));
        // Sent from both stages; read in both stages.
        app.add_system(Stage::Update, |w| {
            let frame = w.resource::<Time>().frame;
            w.send(Ping(frame * 1000));
        });
        app.add_system(Stage::FixedUpdate, |w| {
            let tick = w.resource::<FixedTime>().tick;
            w.send(Ping(tick));
        });
        app.on_event(Stage::FixedUpdate, |w: &mut World, p: &Ping| {
            w.resource_mut::<(Vec<u64>, Vec<u64>)>().0.push(p.0);
        });
        app.on_event(Stage::Update, |w: &mut World, p: &Ping| {
            w.resource_mut::<(Vec<u64>, Vec<u64>)>().1.push(p.0);
        });
        let frames = (2.0 * hz) as u64;
        app.run_frames(frames as usize, 1.0 / hz);
        let ticks = app.world.resource::<FixedTime>().tick;
        let (fixed, update) = app.world.resource::<(Vec<u64>, Vec<u64>)>().clone();
        let mut sent: Vec<u64> = (1..=frames).map(|f| f * 1000).collect();
        sent.extend(1..=ticks);
        sent.sort_unstable();
        // The Update reader (after the Update sender) saw every event once.
        let mut seen = update.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), update.len(), "{hz} Hz: no duplicates");
        assert_eq!(seen, sent, "{hz} Hz");
        // The fixed reader saw every event sent before its last step, once.
        let mut f = fixed.clone();
        f.sort_unstable();
        f.dedup();
        assert_eq!(f.len(), fixed.len(), "{hz} Hz: no duplicates");
        assert!(f.iter().all(|v| sent.contains(v)));
        assert!(
            f.len() as u64 >= ticks + frames - (hz / 60.0).ceil() as u64 - 1,
            "{hz} Hz"
        );
    }
}

struct Unqueued;

#[test]
fn send_without_a_queue_warns_instead_of_panicking() {
    let mut app = App::new();
    app.world.send(Unqueued);
    app.world.send(Unqueued);
    assert!(app.world.get_resource::<Events<Unqueued>>().is_none());
    app.add_event::<Unqueued>();
    app.world.send(Unqueued);
    assert_eq!(app.world.resource::<Events<Unqueued>>().len(), 1);
}

fn alpha(w: &mut World) {
    log(w, "alpha");
}
fn beta(w: &mut World) {
    log(w, "beta");
}
fn gamma(w: &mut World) {
    log(w, "gamma");
}

#[test]
fn systems_order_by_name_suffix_full_path_and_label() {
    let mut app = App::new();
    app.add_system(Stage::Update, alpha);
    app.system(Stage::Update, gamma).before("beta");
    app.add_system(Stage::Update, beta);
    app.system(Stage::Update, |w| log(w, "first"))
        .label("first")
        .before("system_api::alpha");
    app.system(Stage::Update, |w| log(w, "last"))
        .after("first")
        .after("beta")
        .label("last")
        .add_system(Stage::Update, |w| log(w, "unordered"));
    assert!(app.check_order().is_ok());
    app.update();
    assert_eq!(
        logged(&app),
        ["first", "alpha", "gamma", "beta", "last", "unordered"]
    );
    let names = app.system_names(Stage::Update);
    assert!(
        names[1].ends_with("alpha") && names[3].ends_with("beta"),
        "{names:?}"
    );
    // PostUpdate systems can go before transform propagation.
    app.system(Stage::PostUpdate, beta)
        .before("propagate_transforms");
    assert!(app.system_names(Stage::PostUpdate)[0].ends_with("beta"));
}

#[test]
fn cycles_and_unknown_names_are_errors_and_keep_insertion_order() {
    let mut app = App::new();
    app.system(Stage::Update, alpha).after("gamma");
    app.system(Stage::Update, beta).after("alpha");
    app.system(Stage::Update, gamma).after("beta");
    match app.check_order() {
        Err(OrderError::Cycle { stage, cycle }) => {
            assert_eq!(stage, Stage::Update);
            assert_eq!(cycle.len(), 4, "{cycle:?}");
            assert_eq!(cycle.first(), cycle.last());
            let text = OrderError::Cycle { stage, cycle }.to_string();
            assert!(text.contains("alpha") && text.contains("gamma"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    app.update();
    assert_eq!(logged(&app), ["alpha", "beta", "gamma"]);

    let mut app = App::new();
    app.system(Stage::FixedUpdate, alpha).after("move_playerz");
    let err = app.check_order().unwrap_err();
    assert!(matches!(&err, OrderError::Unknown { reference, .. } if reference == "move_playerz"));
    assert!(err.to_string().contains("move_playerz"), "{err}");
}

#[test]
fn time_scale_and_pause_slow_and_stop_fixed_steps() {
    let mut app = App::new();
    app.add_system(Stage::FixedUpdate, |_| {});
    app.world.resource_mut::<Time>().scale = 0.5;
    app.run_frames(60, 1.0 / 60.0);
    let ticks = app.world.resource::<FixedTime>().tick;
    assert!((29..=30).contains(&ticks), "{ticks}");
    app.world.resource_mut::<Time>().paused = true;
    app.add_event::<Ping>();
    app.add_system(Stage::Update, |w| w.send(Ping(0)));
    app.run_frames(30, 1.0 / 60.0);
    assert_eq!(app.world.resource::<FixedTime>().tick, ticks);
    // Events still advance while paused, so they do not pile up.
    assert_eq!(app.world.resource::<Events<Ping>>().len(), 2);
    let t = app.world.resource::<Time>();
    assert_eq!((t.delta, t.unscaled_delta), (0.0, 1.0 / 60.0));
}

/// `on_add` and deferred-command overhead at 10k entities. Run with
/// `cargo test --release -p box_core --test system_api -- --ignored --nocapture`.
#[test]
#[ignore]
fn measure_hooks_and_defer_at_10k_entities() {
    use std::time::Instant;
    const N: usize = 10_000;
    struct Coin;
    struct Spinner;
    let mut app = App::new();
    app.on_add::<Coin>(|w, e| {
        let _ = w.insert_one(e, Spinner);
    });
    app.add_system(Stage::Update, |_| {});
    for i in 0..N {
        app.world
            .spawn((Transform::from_xyz(i as f32, 0.0, 0.0), Coin));
    }
    let t = Instant::now();
    app.update();
    let first = t.elapsed();
    assert_eq!(app.world.count::<&Spinner>(), N);
    let frames = 1000;
    let t = Instant::now();
    for _ in 0..frames {
        app.update();
    }
    let steady = t.elapsed() / frames;
    let mut bare = App::new();
    bare.add_system(Stage::Update, |_| {});
    for i in 0..N {
        bare.world
            .spawn((Transform::from_xyz(i as f32, 0.0, 0.0), Coin, Spinner));
    }
    bare.update();
    let t = Instant::now();
    for _ in 0..frames {
        bare.update();
    }
    let bare_steady = t.elapsed() / frames;

    // Deferred despawn of every entity while iterating vs collect-then-apply.
    let t = Instant::now();
    for (e, _) in app.world.query::<(Entity, &Coin)>().iter() {
        app.world.defer().despawn(e);
    }
    let queued = t.elapsed();
    let t = Instant::now();
    app.world.apply_deferred();
    let applied = t.elapsed();
    let t = Instant::now();
    let all: Vec<Entity> = bare
        .world
        .query::<(Entity, &Coin)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in all {
        bare.world.despawn(e);
    }
    let direct = t.elapsed();
    println!(
        "on_add: first frame with {N} new entities {first:?}; steady frame {steady:?} \
         (same app without hooks: {bare_steady:?})"
    );
    println!(
        "defer: queue {N} despawns {queued:?}, apply {applied:?}; collect-then-despawn {direct:?}"
    );
}
