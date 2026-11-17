//! The Collector player.
//! `cargo run -p collector --bin collector-player -- [project_dir] [--scene <name|path>] [--capture <png>] [--frames N] [--size WxH] [--report <json>] [--headless]`
//! Without a project folder it plays this sample.

fn main() -> anyhow::Result<()> {
    let mut options = box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?;
    // In an exported bundle the game.pak next to the binary is played.
    options.default_project(collector::authoring::project_dir());
    box_engine::player::run_game(options, collector::authoring::game())?;
    Ok(())
}
