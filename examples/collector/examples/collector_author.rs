//! Regenerate `assets/scenes/*.scene.json` from `authoring/*.commands.json`.
//! `cargo run -p collector --example collector_author`

fn main() -> anyhow::Result<()> {
    let game = collector::authoring::game();
    for name in collector::authoring::SCENES {
        let bytes =
            collector::authoring::build_scene(&game, &collector::authoring::commands_path(name))?;
        let out = collector::authoring::scene_path(name);
        std::fs::write(&out, bytes)?;
        println!("wrote {}", out.display());
    }
    Ok(())
}
