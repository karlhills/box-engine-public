//! The guide sample's player.
//! `cargo run -p guide_native --bin guide-player -- [project_dir] [--scene <name|path>] [--capture <png>] [--frames N] [--headless]`
//! Without a project folder it plays this crate's project.

use std::process::ExitCode;

fn main() -> ExitCode {
    match play() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", box_engine::player::error_text(&e));
            ExitCode::FAILURE
        }
    }
}

fn play() -> anyhow::Result<()> {
    let mut options = box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {
        options.project = Some(guide_native::project_dir());
    }
    box_engine::player::run_game(options, guide_native::game())?;
    Ok(())
}
