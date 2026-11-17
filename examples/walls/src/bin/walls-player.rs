//! The Walls player.
//! `cargo run -p walls --bin walls-player -- [project_dir] [--scene <name|path>] [--capture <png>] [--frames N] [--size WxH] [--report <json>] [--headless]`
//! Without a project folder it plays this example.

fn main() -> anyhow::Result<()> {
    let mut options = box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?;
    // In an exported bundle the game.pak next to the binary is played.
    options.default_project(walls::authoring::project_dir());
    box_engine::player::run_game(options, walls::authoring::game())?;
    Ok(())
}
