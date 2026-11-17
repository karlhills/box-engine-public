//! Regenerate `assets/scenes/*.scene.json` from `authoring/*.commands.json`.
//! `cargo run -p walls --example walls_author`

fn main() -> anyhow::Result<()> {
    let game = walls::authoring::game();
    for name in walls::authoring::SCENES {
        let bytes = walls::authoring::build_scene(&game, &walls::authoring::commands_path(name))?;
        let out = walls::authoring::scene_path(name);
        std::fs::write(&out, bytes)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}
