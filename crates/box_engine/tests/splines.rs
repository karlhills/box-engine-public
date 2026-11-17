//! M5 splines in the engine: the `Spline` component (saved, reflected,
//! round-trips through scenes), the `Sweep` generator (regenerates when its
//! spline changes, and only its dependents do; a spline on another entity
//! with both transforms; a removed spline warns), and Sweep's golden
//! hashes with spline inputs.

use std::path::PathBuf;
use std::time::Duration;

use box_engine::box_core::edit::EditStack;
use box_engine::box_core::reflect::PartialReflect;
use box_engine::box_core::scene;
use box_engine::box_proc::golden::{GoldenCase, assert_golden};
use box_engine::box_proc::mesh_builder::edge_report;
use box_engine::box_proc::{GeneratorRegistry, ProcClock, mesh_hash};
use box_engine::prelude::*;
use box_engine::{Game, sync_scene_meshes};

/// A world with the engine registry and a manual throttle clock.
fn world() -> World {
    let mut w = Game::new().new_world();
    w.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::ZERO),
        ..ProcSettings::default()
    });
    w
}

/// Propagate and regenerate, as a frame does; the throttle clock moves on
/// far enough that the flush is due.
fn frame(w: &mut World) {
    if !w.contains_resource::<ProcSettings>() {
        w.insert_resource(ProcSettings {
            clock: ProcClock::Manual(Duration::ZERO),
            ..ProcSettings::default()
        });
    }
    if let ProcClock::Manual(t) = &mut w.resource_mut::<ProcSettings>().clock {
        *t += Duration::from_millis(100);
    }
    box_engine::box_core::transform::propagate_transforms(w);
    sync_scene_meshes(w);
}

fn generations(w: &World, e: Entity) -> u64 {
    w.get::<ProcState>(e).expect("ProcState").generations()
}

fn warnings(w: &World, e: Entity) -> Vec<String> {
    w.get::<ProcState>(e)
        .expect("ProcState")
        .warnings()
        .to_vec()
}

fn drawn(w: &World, e: Entity) -> Option<MeshData> {
    let r = w.get::<MeshRenderer>(e)?;
    w.resource::<Assets<MeshData>>().get(r.mesh).cloned()
}

fn drawn_hash(w: &World, e: Entity) -> u64 {
    mesh_hash(&drawn(w, e).expect("a drawn mesh"))
}

fn set(w: &mut World, edits: &mut EditStack, uid: Uid, path: &str, value: &dyn PartialReflect) {
    let p = w.field_path(path).unwrap();
    edits.set_field(w, uid, &p, value).unwrap();
}

fn bezier() -> Spline {
    Spline {
        points: vec![
            ControlPoint::new(Vec3::new(-2.0, 0.3, 0.0)).with_tangent(Vec3::new(2.0, 0.0, -3.0)),
            ControlPoint::new(Vec3::new(0.0, 0.8, 0.5))
                .with_tangents(Vec3::new(3.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0))
                .with_roll(0.3)
                .with_scale(Vec2::new(1.5, 0.75)),
            ControlPoint::new(Vec3::new(2.0, 0.3, 0.0)),
        ],
        mode: SplineMode::Bezier,
        closed: false,
    }
}

#[test]
fn spline_and_sweep_are_registered_saved_and_round_trip() {
    let registry = box_engine::engine_registry();
    assert!(registry.component("Spline").is_some());
    let sweep = registry.generator("Sweep").expect("Sweep is a generator");
    assert_eq!(sweep.category(), "Shapes");
    assert_eq!(sweep.material_slots(), ["Surface", "Caps"]);

    let mut w = world();
    let spline = bezier();
    let uid = Uid::new();
    let e = w
        .spawn_saved(
            uid,
            "Rail",
            (
                Transform::from_xyz(1.0, 0.0, 2.0),
                spline.clone(),
                Sweep {
                    profile: SweepProfile::Rect {
                        width: 0.2,
                        height: 0.1,
                    },
                    ..Sweep::default()
                },
            ),
        )
        .unwrap();
    frame(&mut w);
    let before = drawn_hash(&w, e);
    let text = scene::save_string(&w).unwrap();
    for saved in [
        "\"Spline\"",
        "\"in_tangent\"",
        "\"Bezier\"",
        "\"Sweep\"",
        "\"Rect\"",
    ] {
        assert!(text.contains(saved), "{saved} missing:\n{text}");
    }
    for runtime in ["ProcState", "MeshRenderer", "positions"] {
        assert!(!text.contains(runtime), "{runtime} saved:\n{text}");
    }
    let mut loaded = scene::load(text.as_bytes(), w.registry().clone()).unwrap();
    frame(&mut loaded);
    let e2 = loaded.entity_by_uid(uid).unwrap();
    assert_eq!(
        *loaded.get::<Spline>(e2).unwrap(),
        spline,
        "byte-identical spline"
    );
    assert_eq!(drawn_hash(&loaded, e2), before, "the same geometry");
}

/// Editing a spline regenerates every sweep that reads it, and nothing else
/// (ROADMAP M5: "Editing a spline regenerates every object that depends on it").
#[test]
fn editing_a_spline_regenerates_exactly_its_dependents() {
    let mut w = world();
    let mut edits = EditStack::new();
    let path_uid = Uid::new();
    let other_uid = Uid::new();
    // A spline with its own tube, two more sweeps reading it from other
    // entities, and an unrelated tube on its own spline.
    let own = w
        .spawn_saved(
            path_uid,
            "Path",
            (Transform::default(), Spline::default(), Sweep::default()),
        )
        .unwrap();
    let a = w
        .spawn_saved(
            Uid::new(),
            "Kerb",
            (
                Transform::default(),
                Sweep {
                    spline: Some(path_uid),
                    profile: SweepProfile::Rect {
                        width: 0.3,
                        height: 0.1,
                    },
                    orientation: SweepOrientation::Up,
                    ..Sweep::default()
                },
            ),
        )
        .unwrap();
    let b = w
        .spawn_saved(
            Uid::new(),
            "Cable",
            (
                Transform::from_xyz(0.0, 1.0, 0.0),
                Sweep {
                    spline: Some(path_uid),
                    ..Sweep::default()
                },
            ),
        )
        .unwrap();
    let other = w
        .spawn_saved(
            other_uid,
            "Other",
            (
                Transform::from_xyz(5.0, 0.0, 0.0),
                Spline::default(),
                Sweep::default(),
            ),
        )
        .unwrap();
    frame(&mut w);
    let gens = |w: &World| [own, a, b, other].map(|e| generations(w, e));
    let start = gens(&w);
    assert_eq!(start, [1, 1, 1, 1]);
    let hashes = [own, a, b, other].map(|e| drawn_hash(&w, e));
    // Idle frames regenerate nothing.
    frame(&mut w);
    frame(&mut w);
    assert_eq!(gens(&w), start);

    edits.begin("Move point");
    set(
        &mut w,
        &mut edits,
        path_uid,
        "Spline.points[1].position",
        &Vec3::new(-0.7, 0.6, -1.2),
    );
    edits.commit();
    frame(&mut w);
    assert_eq!(
        gens(&w),
        [2, 2, 2, 1],
        "the three dependents, not the other tube"
    );
    for (i, e) in [own, a, b].into_iter().enumerate() {
        assert_ne!(drawn_hash(&w, e), hashes[i], "dependent {i} changed");
    }
    assert_eq!(drawn_hash(&w, other), hashes[3]);

    // Undo restores the geometry exactly.
    edits.undo(&mut w).unwrap();
    frame(&mut w);
    assert_eq!([own, a, b, other].map(|e| drawn_hash(&w, e)), hashes);
}

/// A sweep reading another entity's spline follows it in world space: the
/// spline's and the sweep's transforms both count, and moving either one
/// regenerates it.
#[test]
fn a_spline_on_another_entity_accounts_for_both_transforms() {
    let mut w = world();
    let mut edits = EditStack::new();
    let spline_uid = Uid::new();
    let sweep_uid = Uid::new();
    let spline = Spline::from_positions(
        &[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
        SplineMode::Polyline,
        false,
    );
    let spline_t = Transform::from_xyz(10.0, 2.0, 0.0)
        .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    let _ = w
        .spawn_saved(spline_uid, "Path", (spline_t, spline))
        .unwrap();
    let sweep = Sweep {
        spline: Some(spline_uid),
        profile: SweepProfile::Circle {
            radius: 0.1,
            segments: 8,
        },
        caps: false,
        ..Sweep::default()
    };
    let e = w
        .spawn_saved(
            sweep_uid,
            "Tube",
            (Transform::from_xyz(1.0, 0.0, 0.0), sweep),
        )
        .unwrap();
    frame(&mut w);
    assert!(warnings(&w, e).is_empty(), "{:?}", warnings(&w, e));
    // The spline runs from (10, 2, 0) to (10, 2, -4) in world space (turned
    // 90° about Y); in the sweep's space (moved by +1 in x) that is
    // x = 9, z from 0 to -4.
    let world_bounds = |w: &World| {
        let g = w.get::<GlobalTransform>(e).unwrap().0;
        drawn(w, e).unwrap().bounds().transformed(&g)
    };
    let b = world_bounds(&w);
    assert!((b.min - Vec3::new(9.9, 1.9, -4.0)).length() < 1e-3, "{b:?}");
    assert!((b.max - Vec3::new(10.1, 2.1, 0.0)).length() < 1e-3, "{b:?}");

    // Moving the sweep's entity keeps the tube on the spline (it regenerates).
    let g0 = generations(&w, e);
    set(
        &mut w,
        &mut edits,
        sweep_uid,
        "Transform.translation",
        &Vec3::new(-3.0, 1.0, 0.0),
    );
    frame(&mut w);
    assert_eq!(generations(&w, e), g0 + 1);
    let b2 = world_bounds(&w);
    assert!(
        (b2.min - b.min).length() < 1e-3 && (b2.max - b.max).length() < 1e-3,
        "{b2:?}"
    );

    // Moving the spline's entity moves the tube.
    set(
        &mut w,
        &mut edits,
        spline_uid,
        "Transform.translation",
        &Vec3::new(10.0, 5.0, 0.0),
    );
    frame(&mut w);
    assert_eq!(generations(&w, e), g0 + 2);
    let b3 = world_bounds(&w);
    assert!((b3.min.y - 4.9).abs() < 1e-3, "{b3:?}");
}

#[test]
fn removing_the_spline_entity_warns_and_draws_nothing() {
    let mut w = world();
    let mut edits = EditStack::new();
    let spline_uid = Uid::new();
    let _ = w
        .spawn_saved(
            spline_uid,
            "Path",
            (Transform::default(), Spline::default()),
        )
        .unwrap();
    let e = w
        .spawn_saved(
            Uid::new(),
            "Tube",
            (
                Transform::default(),
                Sweep {
                    spline: Some(spline_uid),
                    ..Sweep::default()
                },
            ),
        )
        .unwrap();
    frame(&mut w);
    assert!(drawn(&w, e).is_some());
    edits.despawn(&mut w, spline_uid).unwrap();
    frame(&mut w);
    let warn = warnings(&w, e);
    assert!(
        warn.iter()
            .any(|m| m.contains("`Spline`") && m.contains("missing")),
        "{warn:?}"
    );
    assert!(
        w.get::<ProcState>(e).unwrap().mesh().is_none(),
        "draws nothing"
    );
    // Undo brings the spline and the tube back.
    edits.undo(&mut w).unwrap();
    frame(&mut w);
    assert!(warnings(&w, e).is_empty());
    assert!(w.get::<ProcState>(e).unwrap().mesh().is_some());

    // A sweep on an entity without any spline warns too.
    let lone = w
        .spawn_saved(Uid::new(), "Lone", (Transform::default(), Sweep::default()))
        .unwrap();
    frame(&mut w);
    assert!(!warnings(&w, lone).is_empty());
    assert!(w.get::<ProcState>(lone).unwrap().mesh().is_none());
}

// --------------------------------------------------------------- golden ----

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/generators")
}

const OWNER: Uid = Uid::from_u128(0x5ee9);

fn case(name: &str, sweep: Sweep, spline: Spline) -> GoldenCase<Sweep> {
    GoldenCase::new(name, sweep, 0)
        .with_owner(OWNER)
        .with_input(OWNER, spline)
}

fn helix() -> Spline {
    Spline::from_positions(
        &(0..9)
            .map(|i| {
                let a = i as f32 * 0.9;
                Vec3::new(a.cos() * 2.0, i as f32 * 0.35, a.sin() * 2.0)
            })
            .collect::<Vec<_>>(),
        SplineMode::CatmullRom,
        false,
    )
}

fn sweep_cases() -> Vec<GoldenCase<Sweep>> {
    let ring = Spline::from_positions(
        &[
            Vec3::new(3.0, 0.5, 0.0),
            Vec3::new(1.0, 1.2, -2.5),
            Vec3::new(-2.0, 0.4, -2.0),
            Vec3::new(-3.0, 1.0, 1.0),
            Vec3::new(0.0, 0.3, 2.5),
        ],
        SplineMode::CatmullRom,
        true,
    );
    let u_shape = Spline::from_positions(
        &[
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(2.0, 0.0, -3.0),
            Vec3::new(4.0, 0.0, 1.0),
        ],
        SplineMode::Polyline,
        false,
    );
    let bar = SweepProfile::Rect {
        width: 0.5,
        height: 0.2,
    };
    vec![
        case("default", Sweep::default(), Spline::default()),
        case(
            "tube_closed_spline",
            Sweep {
                profile: SweepProfile::Circle {
                    radius: 0.25,
                    segments: 12,
                },
                ..Sweep::default()
            },
            ring,
        ),
        case(
            "rect_polyline_mitred",
            Sweep {
                profile: SweepProfile::Rect {
                    width: 0.3,
                    height: 1.2,
                },
                orientation: SweepOrientation::Up,
                ..Sweep::default()
            },
            u_shape,
        ),
        case(
            "helix_up",
            Sweep {
                profile: bar.clone(),
                orientation: SweepOrientation::Up,
                ..Sweep::default()
            },
            helix(),
        ),
        case(
            "helix_rotation_minimizing",
            Sweep {
                profile: bar,
                orientation: SweepOrientation::RotationMinimizing,
                ..Sweep::default()
            },
            helix(),
        ),
        case(
            "bezier_custom_open_profile",
            Sweep {
                profile: SweepProfile::Custom {
                    points: vec![
                        Vec2::new(-0.2, 0.0),
                        Vec2::new(0.0, 0.08),
                        Vec2::new(0.2, 0.0),
                    ],
                    closed: false,
                },
                tolerance: 0.005,
                caps: false,
                ..Sweep::default()
            },
            bezier(),
        ),
    ]
}

/// Changing `Sweep` output without bumping `Sweep::VERSION` fails here.
#[test]
fn sweep_golden_hashes() {
    let cases = sweep_cases();
    assert_golden::<Sweep>(golden_dir(), &cases);
    // The two orientation modes differ on a 3D curve.
    let helix_hash = |name: &str| {
        let c = cases.iter().find(|c| c.name == name).unwrap();
        mesh_hash(&c.params.sweep_local(&helix()).mesh.unwrap())
    };
    assert_ne!(
        helix_hash("helix_up"),
        helix_hash("helix_rotation_minimizing")
    );
}

#[test]
fn polyline_rect_sweep_is_closed_with_mitred_corners() {
    let cases = sweep_cases();
    let c = cases
        .iter()
        .find(|c| c.name == "rect_polyline_mitred")
        .unwrap();
    let spline = Spline::from_positions(
        &[
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(-3.0, 0.0, -3.0),
            Vec3::new(2.0, 0.0, -3.0),
            Vec3::new(4.0, 0.0, 1.0),
        ],
        SplineMode::Polyline,
        false,
    );
    let out = c.params.sweep_local(&spline);
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let m = out.mesh.unwrap();
    assert!(edge_report(&m, 1e-5).is_closed_manifold());
    // Mitred corner: the outer corner vertex of the first turn is at
    // (-3.15, y, -3.15).
    assert!(
        m.positions
            .iter()
            .any(|p| (*p - Vec3::new(-3.15, 1.2, -3.15)).length() < 1e-4),
        "mitred outer corner"
    );
}
