//! The Walls editor: box-editor with the walls and fences rules and the
//! Draw Wall and Draw Fence tools (`walls::editor`).
//! `cargo run -p walls --features editor --bin walls-editor -- [project_dir] [--play] [--capture <png>] [--size WxH] [--frames N]`
//! Without a project folder it opens this example.

fn main() -> anyhow::Result<()> {
    let mut options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {
        options.project = Some(walls::authoring::project_dir());
    }
    box_editor::run_game_with(
        options,
        walls::authoring::game(),
        &[&walls::editor::WallsEditor],
    )
}
