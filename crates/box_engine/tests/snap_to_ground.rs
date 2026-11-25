//! Snap to Ground (docs/WORLD_BUILDING.md W3): the `snap_to_ground` command
//! moves an entity's origin onto the terrain under it, as one undoable edit,
//! in a world that never ran a frame (what `box scene apply` loads).

use box_engine::box_core::edit::{EditStack, apply_commands, parse_commands};
use box_engine::box_core::transform::compute_global;
use box_engine::prelude::*;

/// A hill of height 3 at the origin on a 40 m terrain lifted 1 m.
fn world() -> World {
    let mut w = Game::new().new_world();
    w.spawn((
        Uid::new(),
        Name::new("Land"),
        Transform::from_xyz(0.0, 1.0, 0.0),
        Terrain {
            size: Vec2::splat(40.0),
            cell_size: 0.5,
            stamps: vec![TerrainStamp::Hill {
                center: Vec2::ZERO,
                radius: 8.0,
                height: 3.0,
            }],
            ..Terrain::default()
        },
    ));
    w
}

fn spawn(w: &mut World, name: &str, t: Transform) -> Entity {
    w.spawn((Uid::new(), Name::new(name), t))
}

fn apply(w: &mut World, stack: &mut EditStack, json: &str) -> Result<(), String> {
    let commands = parse_commands(json).map_err(|e| e.to_string())?;
    apply_commands(w, stack, &commands, "test")
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn ground(w: &World, x: f32, z: f32) -> (f32, Vec3) {
    let s = w.resource::<Ground>().sample(x, z).unwrap();
    (s.height, s.normal)
}

#[test]
fn snapping_puts_the_origin_on_the_ground_and_undoes() {
    let mut w = world();
    let rock = spawn(&mut w, "Rock", Transform::from_xyz(2.0, 20.0, 1.0));
    let mut stack = EditStack::new();
    apply(
        &mut w,
        &mut stack,
        r#"{ "op": "snap_to_ground", "entity": "Rock", "offset": 0.25 }"#,
    )
    .unwrap();
    let (h, _) = ground(&w, 2.0, 1.0);
    assert!(h > 3.0, "on the lifted hill: {h}");
    let t = *w.get::<Transform>(rock).unwrap();
    assert_eq!((t.translation.x, t.translation.z), (2.0, 1.0));
    assert!((t.translation.y - (h + 0.25)).abs() < 1e-5, "{t:?}");
    assert_eq!(t.rotation, Quat::IDENTITY, "rotation kept without align");
    stack.undo(&mut w).unwrap();
    assert_eq!(w.get::<Transform>(rock).unwrap().translation.y, 20.0);
}

#[test]
fn a_child_lands_in_world_space_and_align_tilts_it_to_the_slope() {
    let mut w = world();
    let parent = spawn(
        &mut w,
        "Group",
        Transform::from_xyz(-1.0, 5.0, 0.0)
            .with_rotation(Quat::from_rotation_y(0.5))
            .with_scale(Vec3::splat(2.0)),
    );
    let lamp = spawn(
        &mut w,
        "Lamp",
        Transform::from_xyz(2.0, 3.0, 0.0).with_rotation(Quat::from_rotation_y(1.0)),
    );
    w.set_parent(lamp, Some(parent)).unwrap();
    let before = compute_global(&w, lamp).translation;
    let mut stack = EditStack::new();
    apply(
        &mut w,
        &mut stack,
        r#"{ "op": "snap_to_ground", "entity": "Lamp", "align": true }"#,
    )
    .unwrap();
    let g = compute_global(&w, lamp);
    let (scale, rotation, at) = g.to_scale_rotation_translation();
    let (h, n) = ground(&w, before.x, before.z);
    assert!(
        (at - Vec3::new(before.x, h, before.z)).length() < 1e-4,
        "{at}"
    );
    assert!(n.y < 0.99, "on the hill's side: {n}");
    assert!((rotation * Vec3::Y - n).length() < 1e-4, "up is the normal");
    assert!(
        (scale - Vec3::splat(2.0)).length() < 1e-4,
        "scale kept: {scale}"
    );
}

#[test]
fn no_ground_fails_the_list_and_changes_nothing() {
    let mut w = world();
    let a = spawn(&mut w, "A", Transform::from_xyz(0.0, 9.0, 0.0));
    spawn(&mut w, "Far", Transform::from_xyz(100.0, 9.0, 0.0));
    let mut stack = EditStack::new();
    let err = apply(
        &mut w,
        &mut stack,
        r#"[{ "op": "snap_to_ground", "entity": "A" },
            { "op": "snap_to_ground", "entity": "Far" }]"#,
    )
    .unwrap_err();
    assert!(
        err.contains("command 1 (snap_to_ground), entity `Far`"),
        "{err}"
    );
    assert!(err.contains("no ground under it"), "{err}");
    assert_eq!(
        w.get::<Transform>(a).unwrap().translation.y,
        9.0,
        "rolled back"
    );
}

#[test]
fn snapping_sees_a_terrain_made_earlier_in_the_same_list() {
    let mut w = Game::new().new_world();
    let cube = spawn(&mut w, "Cube", Transform::from_xyz(0.0, 0.5, 0.0));
    let mut stack = EditStack::new();
    apply(
        &mut w,
        &mut stack,
        r#"[{ "op": "spawn", "name": "Land", "components": {
                "Terrain": { "size": [20.0, 20.0], "stamps": [
                    { "Hill": { "center": [0.0, 0.0], "radius": 6.0, "height": 2.0 } } ] } } },
            { "op": "snap_to_ground", "entity": "Cube", "offset": 0.5 }]"#,
    )
    .unwrap();
    let y = w.get::<Transform>(cube).unwrap().translation.y;
    assert!((y - 2.5).abs() < 1e-3, "on the hill top: {y}");
}
