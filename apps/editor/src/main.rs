//! `box-editor [project_dir] [--play] [--control] [--capture <png>] [--size WxH] [--frames N]`
//!
//! Without a project folder the editor opens a scratch scene. `--play`
//! starts in Play mode. `--control` turns on Allow Agent Control (needs a
//! project) so `box mcp` can drive the editor. `--capture` renders the whole editor UI headlessly
//! (default layout, 2560x1440 at 2x unless `--size` is given) and exits
//! non-zero on wgpu validation errors.

fn main() -> anyhow::Result<()> {
    // Launched from Finder or the Dock (the macOS app bundle) the working
    // folder is `/`, where nothing can be saved; start in the home folder.
    if std::env::current_dir().is_ok_and(|d| d == std::path::Path::new("/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        let _ = std::env::set_current_dir(home);
    }
    // Older macOS passes a `-psn_…` process serial number to bundled apps.
    let args = std::env::args().skip(1).filter(|a| !a.starts_with("-psn_"));
    let options = box_editor::EditorOptions::from_args(args)?;
    box_editor::run(options)
}
