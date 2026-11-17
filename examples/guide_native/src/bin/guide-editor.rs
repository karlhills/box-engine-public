//! The guide sample's editor: box-editor with the game's types, systems and
//! its editor plugin (a palette action).
//! `cargo run -p guide_native --features editor --bin guide-editor -- [project_dir] [--play] [--capture <png>]`
//! Without a project folder it opens this crate's project.

use guide_native::editor::GuideEditor;

fn main() -> anyhow::Result<()> {
    let mut options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {
        options.project = Some(guide_native::project_dir());
    }
    box_editor::run_game_with(options, guide_native::game(), &[&GuideEditor])
}
