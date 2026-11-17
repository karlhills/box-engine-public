//! The script rule sample of the procedural guide (`docs/guide/procedural.md`):
//! `examples/guide_samples/assets/scenes/procedural.scene.json` played
//! headless with the stock engine. The garden walks are made by
//! `assets/rules/walk.luau`; the guide's claims about their parts (count,
//! keys, transforms, overrides, generated meshes) are checked here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_core::derived::{Derived, GeneratedWarnings};
use box_engine::box_script::ScriptIntent;
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;

const GARDEN_WALK: &str = "9a0c0000-0000-4000-8000-000000000002";
const SIDE_WALK: &str = "9a0c0000-0000-4000-8000-000000000003";
const RULE: &str = "rules/walk.luau";

fn project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/guide_samples")
}

fn play() -> App {
    let options = PlayerOptions::from_args([
        project().display().to_string(),
        "--headless".into(),
        "--scene".into(),
        "procedural".into(),
    ])
    .unwrap();
    let (mut app, _project) = build_app(&Game::new(), &options).unwrap();
    for _ in 0..30 {
        if let Some(s) = app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut app.world, Duration::from_millis(20));
        }
        app.run_frames(1, 1.0 / 60.0);
    }
    app
}

/// The roles of the parts of `owner` (sorted) with their entities.
fn parts(app: &App, owner: &str) -> Vec<(String, Entity)> {
    let owner = Uid::parse(owner).unwrap();
    let mut out: Vec<(String, Entity)> = app
        .world
        .query::<(Entity, &Derived)>()
        .iter()
        .filter(|(_, d)| d.key.owner() == Some(owner))
        .map(|(e, d)| {
            assert_eq!(d.rule, RULE);
            (d.key.role().to_owned(), e)
        })
        .collect();
    out.sort();
    out
}

fn role<'a>(parts: &'a [(String, Entity)], role: &str) -> Option<&'a Entity> {
    parts.iter().find(|(r, _)| r == role).map(|(_, e)| e)
}

#[test]
fn the_walk_rule_makes_the_parts_the_guide_describes() {
    let app = play();

    // Garden Walk: 4 points, 3 stretches of 3.16, 3.54 and 2.55 m at 0.8 m
    // spacing (3 + 4 + 3 stones), the gravel, and lanterns 1, 3 and 4
    // (lantern2 is hidden by an override).
    let garden = parts(&app, GARDEN_WALK);
    let roles: Vec<&str> = garden.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        roles,
        [
            "gravel",
            "lantern1",
            "lantern3",
            "lantern4",
            "leg1/stone0",
            "leg1/stone1",
            "leg1/stone2",
            "leg2/stone0",
            "leg2/stone1",
            "leg2/stone2",
            "leg2/stone3",
            "leg3/stone0",
            "leg3/stone1",
            "leg3/stone2",
        ]
    );

    // Side Walk: one 3.2 m stretch, 4 stones, no lanterns (property
    // `lanterns = false`), a narrower strip.
    let side = parts(&app, SIDE_WALK);
    assert_eq!(side.len(), 5, "{side:?}");
    assert!(role(&side, "lantern1").is_none());

    // The first stone sits half a step along the first stretch (0,0,0) to
    // (3,0,-1), in the owner's space, 2 cm up.
    let stone = *role(&garden, "leg1/stone0").unwrap();
    let t = *app.world.get::<Transform>(stone).unwrap();
    let expected = Vec3::new(3.0, 0.0, -1.0) * (0.5 / 3.0) + Vec3::new(0.0, 0.02, 0.0);
    assert!(
        t.translation.distance(expected) < 1e-4,
        "{:?}",
        t.translation
    );
    // Its parent is the owner, so its world position is offset by the
    // owner's (-4, 0, 3).
    let world = app
        .world
        .get::<GlobalTransform>(stone)
        .unwrap()
        .0
        .translation;
    assert!(
        (Vec3::from(world) - (expected + Vec3::new(-4.0, 0.0, 3.0))).length() < 1e-4,
        "{world:?}"
    );

    // The field override recolours that stone; the others keep the
    // rule's colour.
    let red = app.world.get::<StandardMaterial>(stone).unwrap().base_color;
    assert_eq!(red, Color::linear(0.6, 0.25, 0.2));
    let other = *role(&garden, "leg1/stone1").unwrap();
    assert_ne!(
        app.world.get::<StandardMaterial>(other).unwrap().base_color,
        red
    );

    // Every part's generator (Sweep, Extrude, Lathe) produced a mesh,
    // without warnings; no owner has a warning either.
    for (r, e) in garden.iter().chain(&side) {
        let state = app
            .world
            .get::<ProcState>(*e)
            .unwrap_or_else(|| panic!("{r}: no generator state"));
        assert!(state.mesh().is_some(), "{r}: no mesh");
        assert!(state.warnings().is_empty(), "{r}: {:?}", state.warnings());
    }
    for (e, w) in app.world.query::<(Entity, &GeneratedWarnings)>().iter() {
        assert!(w.0.is_empty(), "{e:?}: {:?}", w.0);
    }
}

#[test]
fn a_walk_with_one_point_warns_on_its_owner() {
    let mut app = play();
    let owner = app
        .world
        .entity_by_uid(Uid::parse(SIDE_WALK).unwrap())
        .unwrap();
    app.world
        .component_mut::<ScriptIntent>(owner)
        .unwrap()
        .points
        .truncate(1);
    app.run_frames(2, 1.0 / 60.0);
    // The warning names the rule file and line; the walk has no parts left.
    let warnings = app.world.get::<GeneratedWarnings>(owner).unwrap();
    let text: Vec<String> = warnings.0.iter().map(|w| w.message.clone()).collect();
    assert_eq!(
        text,
        ["rules/walk.luau:25: a walk needs at least two points"],
        "{text:?}"
    );
    assert!(parts(&app, SIDE_WALK).is_empty());
}
