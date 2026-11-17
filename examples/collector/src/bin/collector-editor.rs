//! The Collector editor: box-editor with the game's types and systems.
//! `cargo run -p collector --features editor --bin collector-editor -- [project_dir] [--play] [--capture <png>] [--size WxH] [--frames N]`
//! Without a project folder it opens this sample.

fn main() -> anyhow::Result<()> {
    let mut options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {
        options.project = Some(collector::authoring::project_dir());
    }
    box_editor::run_game(options, collector::authoring::game())
}
