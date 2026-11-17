//! The walls project as first committed (M5, 5320096), vendored in
//! `tests/fixtures/upgrade/walls_m5` (project file, both scenes and their
//! `.meta` files, byte for byte): it opens without an upgrade, and the
//! game's rules generate exactly today's walls and fences from it (the same
//! part keys and mesh hashes as today's scenes).

mod common;

use std::path::PathBuf;

use box_engine::upgrade;
use common::*;
use walls::authoring;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrade/walls_m5")
}

#[test]
fn the_m5_scenes_generate_todays_meshes() {
    let game = game();
    let plan = upgrade::plan(&fixture(), &game.registry()).unwrap();
    assert!(plan.is_current(), "{}", plan.summary());
    for name in ["court", "yard"] {
        let old_bytes =
            std::fs::read(fixture().join(format!("assets/scenes/{name}.scene.json"))).unwrap();
        let old = part_hashes(&load_bytes(&game, &old_bytes));
        let now = part_hashes(&load_scene(&game, name));
        assert!(!old.is_empty(), "{name}: the rules generated parts");
        assert_eq!(old, now, "{name}: generated meshes differ");
    }
    // The vendored copy is the M5 file, not today's.
    assert_ne!(
        std::fs::read(fixture().join("assets/scenes/court.scene.json")).unwrap(),
        std::fs::read(authoring::scene_path("court")).unwrap()
    );
}
