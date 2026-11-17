//! The Collector project as first committed (M3, 967840e), vendored in
//! `tests/fixtures/upgrade/collector_m3` (its project file and both arena
//! scenes, byte for byte), played by today's game: it opens without an
//! upgrade (no format changed incompatibly since; fields added since take
//! their defaults) and plays exactly like today's arenas. Then a test-only
//! version 2 of `Collectible` upgrades it with a backup, and the upgraded
//! project still plays the same.

use std::path::{Path, PathBuf};

use box_engine::Game;
use box_engine::box_core::scene;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_engine::upgrade;
use collector::{ArenaExit, Collectible, FollowCamera, PlayerController, Score, authoring};

const DT: f32 = 1.0 / 60.0;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A fresh copy of the M3 project.
fn m3_project(tag: &str) -> PathBuf {
    let from =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade/collector_m3");
    let to = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("collector_m3_{tag}"));
    let _ = std::fs::remove_dir_all(&to);
    copy_dir(&from, &to);
    to
}

fn app(game: &Game, project: &Path) -> App {
    let options = PlayerOptions {
        project: Some(project.to_path_buf()),
        ..Default::default()
    };
    build_app(game, &options).unwrap().0
}

fn gem_positions(world: &World) -> Vec<Vec3> {
    world
        .query::<(&Transform, &Collectible)>()
        .iter()
        .map(|(t, _)| t.translation)
        .collect()
}

fn scene_file(world: &World) -> String {
    let scenes = world.resource::<Scenes>();
    let main = scenes.main().expect("a main scene");
    scenes
        .path(main)
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What the run did: positions after walking, then the score and scene
/// after collecting each arena, as in `gameplay.rs`.
#[derive(Debug, PartialEq)]
struct Run {
    walked_to: Vec3,
    after_walk: Score,
    arenas: Vec<(Score, String, usize)>,
}

/// Walk forward one second through Gem 1, then collect every gem of both
/// arenas by standing on each in turn.
fn play(mut app: App) -> Run {
    app.run_frames(10, DT);
    app.world.resource_mut::<Input>().press_key(KeyCode::W);
    app.run_frames(60, DT);
    app.world.resource_mut::<Input>().release_key(KeyCode::W);
    app.run_frames(2, DT);
    let player = app.world.first_with::<PlayerController>().unwrap();
    let walked_to = app.world.get::<Transform>(player).unwrap().translation;
    let after_walk = *app.world.resource::<Score>();
    let mut arenas = Vec::new();
    for _ in 0..2 {
        for at in gem_positions(&app.world) {
            let p = app.world.first_with::<PlayerController>().unwrap();
            app.world.component_mut::<Transform>(p).unwrap().translation = at;
            if let Some(k) = app.world.component_mut::<KinematicCharacter>(p) {
                k.velocity = Vec3::ZERO;
            }
            app.run_frames(3, DT);
        }
        app.run_frames(3, DT);
        arenas.push((
            *app.world.resource::<Score>(),
            scene_file(&app.world),
            gem_positions(&app.world).len(),
        ));
    }
    Run {
        walked_to: (walked_to * 100.0).round() / 100.0,
        after_walk,
        arenas,
    }
}

/// Gameplay data by uid: transforms, colliders and the game's components
/// (the fields M3 had).
fn gameplay_data(world: &World) -> Vec<String> {
    let mut out: Vec<String> = world
        .query::<(Entity, &Uid)>()
        .iter()
        .map(|(e, uid)| {
            let t = world.get::<Transform>(e).map(|t| *t);
            let c = world.get::<Collider>(e).map(|c| format!("{:?}", c.shape));
            let pc = world.get::<PlayerController>(e).map(|p| format!("{p:?}"));
            let g = world
                .get::<Collectible>(e)
                .map(|g| (g.points, g.spin_speed, g.bob_height));
            let f = world.get::<FollowCamera>(e).map(|f| format!("{f:?}"));
            let x = world.get::<ArenaExit>(e).map(|x| format!("{x:?}"));
            format!("{uid} {t:?} {c:?} {pc:?} {g:?} {f:?} {x:?}")
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_m3_project_opens_without_an_upgrade_and_plays_like_today() {
    let game = authoring::game();
    let root = m3_project("current");
    let plan = upgrade::plan(&root, &game.registry()).unwrap();
    assert!(plan.is_current(), "{}", plan.summary());

    // The gameplay data in the M3 files is today's.
    for name in ["arena", "arena2"] {
        let load =
            |path: PathBuf| scene::load(&std::fs::read(path).unwrap(), game.registry()).unwrap();
        let old = load(root.join(format!("assets/scenes/{name}.scene.json")));
        let now = load(authoring::scene_path(name));
        assert!(scene::unknown_types(&old).is_empty());
        let (old_data, now_data) = (gameplay_data(&old), gameplay_data(&now));
        // Entities added since (the wind) are the only difference.
        for line in &old_data {
            assert!(now_data.contains(line), "{name}: {line} changed");
        }
    }

    let old_run = play(app(&game, &root));
    let now_run = play(app(&game, &authoring::project_dir()));
    assert_eq!(old_run, now_run);
    assert_eq!(
        old_run.arenas,
        [
            (
                Score {
                    points: 75,
                    collected: 6,
                    arenas_completed: 1
                },
                "arena2.scene.json".to_string(),
                7
            ),
            (
                Score {
                    points: 160,
                    collected: 13,
                    arenas_completed: 2
                },
                "arena.scene.json".to_string(),
                6
            ),
        ]
    );
}

/// Test only: version 2 of `Collectible`, without a data change.
struct CollectibleV2;

impl Plugin for CollectibleV2 {
    fn register(&self, registry: &mut Registry) {
        registry.add_migration(
            "Collectible",
            MigrationStep::new("test-only: Collectible v2 (no data change)").custom(|v| {
                v.get("points").ok_or("a Collectible has points")?;
                Ok(())
            }),
        );
    }

    fn build(&self, _app: &mut App) {}
}

#[test]
fn the_m3_project_upgrades_with_a_backup_and_still_plays_the_same() {
    let game = authoring::game().with_plugin(CollectibleV2);
    let root = m3_project("upgraded");
    let before: Vec<Vec<u8>> = ["arena", "arena2"]
        .iter()
        .map(|n| {
            let bytes = std::fs::read(root.join(format!("assets/scenes/{n}.scene.json"))).unwrap();
            scene::save(&scene::load(&bytes, game.registry()).unwrap()).unwrap()
        })
        .collect();
    let in_memory = play(app(&game, &root));

    let plan = upgrade::plan(&root, &game.registry()).unwrap();
    assert_eq!(plan.files.len(), 2, "{}", plan.summary());
    let report = upgrade::apply(&plan).unwrap();
    assert_eq!(
        std::fs::read(report.backup.join("assets/scenes/arena.scene.json")).unwrap(),
        std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/upgrade/collector_m3/assets/scenes/arena.scene.json")
        )
        .unwrap()
    );
    let text = std::fs::read_to_string(root.join("assets/scenes/arena.scene.json")).unwrap();
    assert_eq!(text.matches("\"$version\": 2").count(), 6, "one per gem");

    for (n, bytes) in ["arena", "arena2"].iter().zip(&before) {
        let file = std::fs::read(root.join(format!("assets/scenes/{n}.scene.json"))).unwrap();
        let world = scene::load(&file, game.registry()).unwrap();
        assert_eq!(&scene::save(&world).unwrap(), bytes, "{n}");
    }
    assert_eq!(play(app(&game, &root)), in_memory);
    assert!(upgrade::plan(&root, &game.registry()).unwrap().is_current());
}
