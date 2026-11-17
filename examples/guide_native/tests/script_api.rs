//! The `score` script API: declared for luau-lsp and documented in the
//! game's own script reference. (Scripts calling it are tested by
//! `gameplay.rs`, which plays `assets/scripts/bonus.luau`.)

use box_engine::box_script::docs;
use guide_native::{game, project_dir};

#[test]
fn the_score_api_is_documented_in_the_games_reference() {
    let game = game();
    let apis: Vec<_> = game.script_apis().iter().cloned().collect();
    let reference = docs::reference(&game.registry(), &apis);
    let missing: Vec<_> = reference
        .undocumented()
        .iter()
        .filter(|m| m.contains("score"))
        .collect();
    assert!(missing.is_empty(), "{missing:?}");

    let dir = project_dir().join("../../target/guide_native/reference");
    reference.write(&dir).unwrap();
    let page = std::fs::read_to_string(dir.join("score.md")).unwrap();
    assert!(page.contains("Adds `points` to the score."), "{page}");
    assert!(page.contains("`score.add(5)` gives five points"), "{page}");
}
