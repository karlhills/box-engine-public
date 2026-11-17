//! Writes `assets/scenes/main.scene.json` from `minimal_native::main_scene`.
//! `cargo run -p minimal_native --example minimal_native_author`

fn main() -> anyhow::Result<()> {
    let bytes = minimal_native::main_scene(&minimal_native::game())?;
    let path = minimal_native::main_scene_path();
    std::fs::create_dir_all(path.parent().expect("scene folder"))?;
    std::fs::write(&path, bytes)?;
    println!("wrote {}", path.display());
    Ok(())
}
