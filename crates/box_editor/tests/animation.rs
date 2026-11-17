//! Skinned animation in the editor (M7): a dropped skinned model shows in
//! bind pose in the edit world, an `AnimationPlayer` added through the edit
//! layer animates it only during Play, and Stop leaves the edit scene
//! byte-identical with the joints back at rest.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use box_core::animation::{AnimationPlayerState, SkinnedMesh};
use box_core::asset_ref::AssetRef;
use box_core::identity::Name;
use box_core::scene;
use box_core::transform::Transform;
use box_core::world::{Entity, World};
use box_editor::assets::AssetItem;
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor};
use glam::{Quat, Vec3};

const IDLE: Duration = Duration::from_secs(30);

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn joint(world: &World, root: Entity, name: &str) -> Entity {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if world.get::<Name>(e).is_some_and(|n| n.as_str() == name) {
            return e;
        }
        stack.extend(world.children(e));
    }
    panic!("no {name}");
}

fn thigh(world: &World, root: Entity) -> Quat {
    world
        .get::<Transform>(joint(world, root, "UpperLeg.L"))
        .unwrap()
        .rotation
}

#[test]
fn skinned_models_animate_in_play_and_stop_restores_the_edit_scene() {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "box_editor_animation_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, "animation").unwrap();
    let _guard = TempDir(dir.clone());
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../box_engine/examples/animation_assets/models/character.glb");
    std::fs::create_dir_all(dir.join("assets/models")).unwrap();
    std::fs::copy(&source, dir.join("assets/models/character.glb")).unwrap();
    let options = EditorOptions {
        project: Some(dir.clone()),
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    let server = editor.state.asset_server().unwrap().clone();
    let file = server.id_of("models/character.glb").unwrap();
    let uid = editor
        .state
        .drop_asset_at(&AssetItem::file(file), Vec3::ZERO)
        .unwrap();
    editor.state.add_component(uid, "AnimationPlayer");
    let walk = AssetRef::path("models/character.glb#Animation/Walk");
    assert!(editor.state.set_field(uid, "AnimationPlayer.clip", &walk));
    assert!(editor.state.wait_assets_idle(IDLE));
    assert!(box_engine::settle_assets(&mut editor.state.world, IDLE));

    // Edit world: the skeleton and skin exist, in bind pose; nothing plays.
    let root = editor.state.world.entity_by_uid(uid).unwrap();
    assert_eq!(editor.state.world.count::<&SkinnedMesh>(), 1);
    assert_eq!(thigh(&editor.state.world, root), Quat::IDENTITY);
    assert!(!editor.state.world.has::<AnimationPlayerState>(root));
    let saved = scene::save(&editor.state.world).unwrap();

    // Play: the copy animates.
    editor.state.start_play().unwrap();
    for _ in 0..20 {
        editor.state.update_assets();
        assert!(editor.state.update_play(1.0 / 60.0).unwrap());
    }
    let play = &editor.state.world;
    let root = play.entity_by_uid(uid).unwrap();
    assert_eq!(play.count::<&SkinnedMesh>(), 1);
    assert!(play.get::<AnimationPlayerState>(root).unwrap().time() > 0.3);
    assert!(thigh(play, root).angle_between(Quat::IDENTITY) > 0.1);

    // Stop: byte-identical edit scene, joints at rest.
    editor.state.stop_play();
    assert_eq!(scene::save(&editor.state.world).unwrap(), saved);
    let root = editor.state.world.entity_by_uid(uid).unwrap();
    assert_eq!(thigh(&editor.state.world, root), Quat::IDENTITY);
}
