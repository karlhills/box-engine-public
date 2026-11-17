//! The generated script API reference (`docs/reference`, `box script docs`)
//! covers every public script item with a description, the committed pages
//! match the engine, and the luau-lsp definitions carry the same
//! descriptions for hover.
//!
//! When a test here fails after an API change: describe the new item with a
//! `--- text` line above its declaration (`ScriptApi::definitions`), or a
//! doc comment on the new component, resource or event field, then
//! regenerate with `cargo run -p box_cli -- script docs --out docs/reference`.

use std::path::{Path, PathBuf};

use box_engine::Game;
use box_engine::box_script::docs::{GENERATED_MARKER, reference};

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference")
}

const REGENERATE: &str = "cargo run -p box_cli -- script docs --out docs/reference";

#[test]
fn every_script_api_item_has_a_description() {
    let game = Game::new();
    let apis: Vec<_> = game.script_apis().iter().cloned().collect();
    let r = reference(&game.registry(), &apis);
    assert!(
        r.undocumented().is_empty(),
        "script API items without a description (add a `--- text` line above the declaration, \
         or a doc comment on the type or field):\n{}",
        r.undocumented().join("\n")
    );
    assert!(r.item_count() > 500, "{} items", r.item_count());
}

#[test]
fn committed_reference_is_current() {
    let game = Game::new();
    let apis: Vec<_> = game.script_apis().iter().cloned().collect();
    let r = reference(&game.registry(), &apis);
    let dir = reference_dir();
    let mut stale = Vec::new();
    for page in &r.pages {
        let committed = std::fs::read_to_string(dir.join(&page.path)).unwrap_or_default();
        if committed != page.text {
            stale.push(page.path.clone());
        }
    }
    for entry in std::fs::read_dir(&dir)
        .expect("docs/reference exists")
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let generated =
            std::fs::read_to_string(entry.path()).is_ok_and(|t| t.starts_with(GENERATED_MARKER));
        if generated && !r.pages.iter().any(|p| p.path == name) {
            stale.push(format!("{name} (no longer generated)"));
        }
    }
    assert!(
        stale.is_empty(),
        "docs/reference is out of date ({}): run `{REGENERATE}`",
        stale.join(", ")
    );
}

/// Every declaration line of the definitions file (globals, table fields,
/// class members, types, generated overloads) has a `---` description
/// directly above it, which luau-lsp shows on hover.
#[test]
fn definitions_carry_descriptions_for_hover() {
    let defs = Game::new().script_definitions();
    let mut previous = "";
    let mut missing = Vec::new();
    for line in defs.lines() {
        let t = line.trim();
        let structural = t.is_empty() || t.starts_with("--") || matches!(t, "}" | "}," | "end");
        if !structural && !previous.starts_with("---") {
            missing.push(t.to_owned());
        }
        previous = t;
    }
    assert!(
        missing.is_empty(),
        "declarations without a `---` description:\n{}",
        missing.join("\n")
    );
}
