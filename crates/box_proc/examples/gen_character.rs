//! Writes the procedural test character (`box_proc::humanoid`) as a skinned
//! `.glb` with idle, walk, run and jump clips, and creates its `.meta`.
//!
//! `cargo run -p box_proc --example gen_character [-- <out.glb>]`
//!
//! The default output is the animation example's asset,
//! `crates/box_engine/examples/animation_assets/models/character.glb`
//! (committed; regenerate it after changing the character and review the
//! animation example's capture). The bytes are deterministic, and
//! `box_engine`'s animation tests check that the committed file matches.

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../box_engine/examples/animation_assets/models/character.glb")
        });
    let glb = box_proc::humanoid::character_glb()?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, &glb.bytes)?;
    println!("wrote {} ({} bytes)", out.display(), glb.bytes.len());
    // Opening an asset server on the folder creates the missing `.meta`.
    if let Some(assets) = out.parent().and_then(|d| d.parent()) {
        box_engine::asset_server(assets)?;
    }
    Ok(())
}
