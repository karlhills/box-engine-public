//! box-engine editor.
//!
//! A library: `apps/editor` is a thin binary around [`run`], and a native
//! project builds its own editor binary around [`run_game`] with its plugins:
//!
//! ```no_run
//! # struct MyGame;
//! # impl box_core::app::Plugin for MyGame { fn build(&self, _: &mut box_core::app::App) {} }
//! use box_engine::Game;
//!
//! fn main() -> anyhow::Result<()> {
//!     let options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
//!     box_editor::run_game(options, Game::new().with_plugin(MyGame))
//! }
//! ```
//!
//! Editor-only extensions of a game (viewport tools, palette actions) come
//! from an [`EditorPlugin`] given to [`run_game_with`]; box_engine's `Game`
//! cannot hold them (box_engine never depends on the editor):
//!
//! ```no_run
//! # struct MyGame;
//! # impl box_core::app::Plugin for MyGame { fn build(&self, _: &mut box_core::app::App) {} }
//! use box_editor::{EditorPlugin, EditorState};
//! use box_editor::path_tool::{PathTool, PathToolConfig};
//! use box_engine::Game;
//!
//! struct MyTools;
//! impl EditorPlugin for MyTools {
//!     fn build(&self, editor: &mut EditorState) {
//!         editor.register_tool(PathTool::new(PathToolConfig::new(
//!             "draw_road", "Road", "RoadPath", "points",
//!         )));
//!     }
//! }
//!
//! fn main() -> anyhow::Result<()> {
//!     let options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
//!     box_editor::run_game_with(options, Game::new().with_plugin(MyGame), &[&MyTools])
//! }
//! ```
//!
//! - [`Editor`] owns the [`EditorState`] and the dock tree; [`Editor::ui`]
//!   draws one frame.
//! - Panes are functions of `(&mut egui::Ui, &mut EditorState)` in [`panes`].
//! - The viewport is drawn by box_render into an offscreen
//!   [`ViewportTarget`](box_render::ViewportTarget) that egui shows as a
//!   native texture; egui then paints into the window surface (or, headless,
//!   an offscreen texture) on the same wgpu device.

pub mod actions;
mod asset_undo;
pub mod assets;
pub mod assistant;
mod behaviour_gizmos;
mod camera;
pub mod control;
mod dialogs;
mod dock;
mod document;
mod editor;
pub mod export;
pub mod feedback;
pub mod generated;
mod gfx;
pub mod gizmo;
pub mod headless;
pub mod history;
pub mod icons;
pub mod inspect;
pub mod layout;
pub mod log;
pub mod marquee;
pub mod mixer_panel;
mod new_project;
pub mod palette;
pub mod panes;
pub mod particle_preview;
pub mod path_tool;
pub mod picking;
pub mod play;
pub mod play_input;
pub mod prefs;
pub mod presets;
pub mod procedural;
mod project_settings;
pub mod rebuild;
pub mod rules_panel;
pub mod scatter_brush;
pub mod scratch;
pub mod script_editor;
mod script_props;
pub mod shortcuts;
pub mod spline_tool;
pub mod start;
mod state;
pub mod theme;
mod toolbar;
pub mod tools;
pub mod values;
pub mod visibility;
pub mod widgets;
mod windowed;

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use box_engine::Game;

pub use camera::{EditorCamera, FlyKeys, NAVIGATION, ViewportPointer};
pub use dialogs::{Dialog, DocAction, UnsavedChoice, UpgradeChoice};
pub use document::{
    RECOVERY_INTERVAL_S, normalize_scene_path, recovery_is_newer, recovery_path_for, scene_name,
};
pub use editor::{Editor, GENERATED_OUTLINE_LIMIT};
pub use play::{GamePanic, PlayError, PlayPhase, PlaySession, PlayState};
pub use state::{
    ConsoleState, EditorState, HierarchyRow, HierarchyState, NewEntity, Pivot, SceneDoc,
    SelectMode, Snap, Space, Status, TransformTool, ViewportState, entity_bounds, scene_bounds,
};

/// What `box-editor --help` prints (and what [`EditorOptions::from_args`]
/// accepts).
pub const USAGE: &str = "\
Usage: box-editor [project_dir] [options]

Opens the box-engine editor on a project folder (the one holding
box.project.json, or any folder inside it). Without one it opens the project
the working folder is in, else the last project, else the sample scene with
a Welcome card (New Project, Open Project, recent projects). A project opens
where its last session left off (scene, camera, selection). If the project or
its start scene cannot be opened, the editor still starts and shows the error.

Options:
  --play             Start in Play mode
  --control          Allow agent control (box mcp) from the start; needs a project
  --no-restore       Do not reopen the last project or the project's last session
  --restore          With --capture: restore the project's last session
  --capture <png>    Render the editor headlessly to a PNG and exit
  --size <W>x<H>     Window size in points, or capture size in pixels
  --frames <n>       Frames to run before capturing; in a window, quit after n frames
  --write-registry   Write the game's types to <project>/.box/registry.json (what
                     `box` and `box mcp` read) and exit
  --after-rebuild    Set by Game ▸ Rebuild and Restart: restore the scene's
                     unsaved changes without asking
  -V, --version      Print the engine version and exit
  -h, --help         Show this help
";

/// Command line: see [`USAGE`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorOptions {
    /// Project folder; the layout is kept in `<project>/.box/layout.json`.
    pub project: Option<PathBuf>,
    /// Render the editor headlessly (default layout) to this PNG and exit.
    pub capture: Option<PathBuf>,
    /// Window size in logical pixels, or capture size in physical pixels.
    pub size: Option<[u32; 2]>,
    /// Frames to run before capturing; in a window, quit after this many
    /// presented frames.
    pub frames: Option<u32>,
    /// Start in Play mode (a capture then shows a Play frame).
    pub play: bool,
    /// Start with Allow Agent Control on (needs a project): `box mcp` can
    /// then query and drive this editor.
    pub control: bool,
    /// `-h` / `--help`: [`run_game`] prints [`USAGE`] and exits.
    pub help: bool,
    /// `-V` / `--version`: [`run_game`] prints `box-editor <version>` and
    /// exits.
    pub version: bool,
    /// `--no-restore`: do not reopen the last project, nor a project's
    /// last session.
    pub no_restore: bool,
    /// `--restore`: restore the project's last session in a `--capture` run
    /// too (windowed runs do unless `--no-restore`).
    pub restore: bool,
    /// The user config folder (recent projects; see [`start`]). [`run_game`]
    /// sets it for windowed runs. With it, a launch without a project folder
    /// opens the working folder's project, else the last project, else the
    /// Welcome card, and opened projects restore their last session. `None`
    /// (tests, `--capture`) reads and writes no user state.
    pub config_dir: Option<PathBuf>,
    /// `--write-registry`: write the game's registry to
    /// `<project>/.box/registry.json` and exit (see
    /// `box_engine::registry_file`).
    pub write_registry: bool,
    /// `--after-rebuild`: started by Rebuild and Restart; the session and
    /// the scene's recovery file are restored without asking.
    pub after_rebuild: bool,
}

impl EditorOptions {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut o = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "-h" | "--help" => o.help = true,
                "-V" | "--version" => o.version = true,
                "--capture" => o.capture = Some(value()?.into()),
                "--play" => o.play = true,
                "--control" => o.control = true,
                "--no-restore" => o.no_restore = true,
                "--restore" => o.restore = true,
                "--write-registry" => o.write_registry = true,
                rebuild::AFTER_REBUILD_FLAG => o.after_rebuild = true,
                "--frames" => {
                    o.frames = Some(value()?.parse().context("--frames expects a number")?)
                }
                "--size" => {
                    let v = value()?;
                    let (w, h) = v.split_once('x').context("--size expects WxH")?;
                    o.size = Some([
                        w.parse().context("bad width")?,
                        h.parse().context("bad height")?,
                    ]);
                }
                flag if flag.starts_with('-') => {
                    bail!("unknown option {flag}; run box-editor --help for the options")
                }
                path => {
                    if o.project.is_some() {
                        bail!("more than one project folder given");
                    }
                    o.project = Some(path.into());
                }
            }
        }
        Ok(o)
    }
}

/// Editor-only extensions of a game: viewport tools
/// ([`EditorState::register_tool`]), palette actions
/// ([`EditorState::register_action`]) and anything else that needs the
/// editor's types. A native project's editor binary passes them to
/// [`run_game_with`]; tests pass them to [`build_game_editor_with`] (or
/// call [`EditorPlugin::build`] on an editor's state). Built once, after
/// the editor opens its project and before `--play` starts; the registered
/// tools and actions stay for the whole session (projects opened later
/// keep them).
pub trait EditorPlugin {
    /// Registers the plugin's tools and actions.
    fn build(&self, editor: &mut EditorState);
}

/// Builds the editor for `options` with the engine alone ([`Game::new`]).
pub fn build_editor(options: &EditorOptions, log: log::LogBuffer) -> Result<Editor> {
    build_game_editor(options, log, Game::new())
}

/// Builds the editor for `options` and `game`: the project's start scene and
/// saved layout, or the scratch scene without a project. Every world the
/// editor creates or loads uses the game's registry, so the game's component
/// types are in the inspector and in saved scenes; Play runs its plugins.
/// With `options.play` the editor starts in Play mode.
///
/// A project that cannot be opened leaves the scratch scene open, and a
/// start scene that cannot be loaded opens the project with an empty,
/// unsaved scene; either way the editor starts with an error dialog saying
/// what failed and what to do.
pub fn build_game_editor(
    options: &EditorOptions,
    log: log::LogBuffer,
    game: Game,
) -> Result<Editor> {
    build_game_editor_with(options, log, game, &[])
}

/// [`build_game_editor`] with the game's editor plugins (tools, actions).
pub fn build_game_editor_with(
    options: &EditorOptions,
    log: log::LogBuffer,
    game: Game,
    plugins: &[&dyn EditorPlugin],
) -> Result<Editor> {
    // Shown at once (a toast and the Console), not only at the first Play.
    game.report_registration_errors();
    let world = scratch::scratch_world_with(game.registry());
    let mut state = EditorState::with_game(world, log, game);
    let interactive = options.config_dir.is_some() && options.capture.is_none();
    state.recent = start::RecentProjects::load(options.config_dir.as_deref());
    state.restore_session =
        options.restore || options.after_rebuild || (interactive && !options.no_restore);
    state.after_rebuild = options.after_rebuild;
    let mut startup_error = None;
    let mut pending_upgrade = None;
    let mut project = options.project.clone();
    if project.is_none() && interactive {
        let cwd = std::env::current_dir().ok();
        match start::choose_launch(cwd.as_deref(), &state.recent, options.no_restore) {
            start::Launch::WorkingFolder(p) => {
                tracing::info!("opening the working folder's project {}", p.display());
                project = Some(p);
            }
            start::Launch::LastProject(p) => {
                tracing::info!("reopening the last project {}", p.display());
                project = Some(p);
            }
            start::Launch::Welcome => state.welcome = true,
        }
    }
    if let Some(p) = &project {
        match state.upgrade_plan(p) {
            // Asked first; the scratch scene stays open meanwhile.
            Ok(Some(plan)) => pending_upgrade = Some(plan),
            Ok(None) => {}
            // A project that does not open is reported below, as before.
            Err(_) if box_engine::Project::open(p).is_err() => {}
            Err(e) => {
                startup_error = Some((
                    "Could not open the project",
                    format!(
                        "{e:#}\n\nThe scratch scene is open instead: use File ▸ Open Project or New \
                         Project."
                    ),
                ));
            }
        }
    }
    if let Some(p) = project
        .as_ref()
        .filter(|_| pending_upgrade.is_none() && startup_error.is_none())
    {
        match state.open_project_at_start(p) {
            Ok(None) => {}
            Ok(Some(e)) => {
                let missing = state
                    .project
                    .as_ref()
                    .filter(|p| !p.start_scene_path().exists());
                let message = match missing {
                    Some(p) => format!(
                        "{}.\n\nThe project is open with an empty, unsaved scene. Save it as \
                         {} (File ▸ Save Scene), or open another scene and set it as the start \
                         scene.",
                        upper_first(&box_engine::player::missing_start_scene_text(p, "")),
                        p.file().start_scene
                    ),
                    None => format!(
                        "{}\n\nThe project is open with an empty, unsaved scene; the file was \
                         not changed. Fix it (`box check` or a text editor) and open it again \
                         with File ▸ Open Scene, or save this scene under another name.",
                        box_engine::player::error_text(&e)
                    ),
                };
                startup_error = Some(("Could not load the start scene", message));
            }
            Err(e) => {
                startup_error = Some((
                    "Could not open the project",
                    format!(
                        "{}\n\nA project is a folder with a box.project.json. The scratch scene \
                         is open instead: use File ▸ Open Project or New Project, or create one \
                         with `box new <folder>`.",
                        box_engine::player::error_text(&e)
                    ),
                ));
            }
        }
    }
    if state.project.is_none() {
        tracing::info!("scene: scratch ({} entities)", state.world.entity_count());
        state.frame_on_open();
        if options.project.is_none() && interactive && pending_upgrade.is_none() {
            // The last project failed to open: offer the others.
            state.welcome = true;
        }
    }
    let mut editor = Editor::new(state);
    for p in plugins {
        p.build(&mut editor.state);
    }
    if let Some((title, message)) = startup_error {
        editor.show_error(title, message);
    }
    if let Some(plan) = &pending_upgrade {
        editor.show_upgrade(plan, false);
    }
    if options.control {
        editor.state.set_agent_control(true);
    }
    if options.play {
        editor.state.start_play()?;
    }
    Ok(editor)
}

/// `text` with its first letter in upper case (a message as a sentence).
fn upper_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Runs the editor for the engine alone; see [`run_game`].
pub fn run(options: EditorOptions) -> Result<()> {
    run_game(options, Game::new())
}

/// Runs the editor for `game`: a window, or with `--capture` a headless
/// render of the default layout written to a PNG (exits with an error on wgpu
/// validation errors). [`run_game_with`] adds the game's editor plugins.
pub fn run_game(options: EditorOptions, game: Game) -> Result<()> {
    run_game_with(options, game, &[])
}

/// [`run_game`] with the game's [`EditorPlugin`]s: their viewport tools and
/// palette actions are registered before the first frame.
pub fn run_game_with(
    mut options: EditorOptions,
    game: Game,
    plugins: &[&dyn EditorPlugin],
) -> Result<()> {
    if options.help {
        print!("{USAGE}");
        return Ok(());
    }
    if options.version {
        println!("box-editor {}", box_engine::project::ENGINE_VERSION);
        return Ok(());
    }
    let log = log::LogBuffer::default();
    let windowed = options.capture.is_none();
    // A windowed editor started from Finder has no terminal: keep its log
    // in a file so startup errors can be read.
    let file = if windowed { log::log_file_path() } else { None };
    log::init_with_file(&log, file.as_deref());
    // Crash reports carry the Console's last records.
    box_engine::crash::install(start::CRASH_KIND);
    let recent = log.clone();
    box_engine::crash::set_log_source(move |n| recent.try_recent(n));
    if windowed && options.config_dir.is_none() {
        options.config_dir = start::user_config_dir();
    }
    if options.write_registry {
        return write_registry(&options, &game).map(|_| ());
    }
    let result = run_editor(&options, log, game, plugins);
    if let Err(e) = &result {
        tracing::error!("{e:#}");
    }
    result
}

/// `--write-registry`: writes the game's registry to the project's
/// `.box/registry.json` (the project folder given, else the working
/// folder's project) and returns its path.
pub fn write_registry(options: &EditorOptions, game: &Game) -> Result<PathBuf> {
    let dir = match &options.project {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let project = box_engine::Project::open(&dir).context("could not open the project")?;
    let path = box_engine::registry_file::registry_path(&project);
    let written = box_engine::registry_file::write_registry(&project, &game.registry())
        .with_context(|| format!("could not write {}", path.display()))?;
    let verb = match written {
        box_engine::registry_file::Written::Written => "wrote",
        box_engine::registry_file::Written::Unchanged => "up to date:",
    };
    println!("{verb} {}", path.display());
    Ok(path)
}

fn run_editor(
    options: &EditorOptions,
    log: log::LogBuffer,
    game: Game,
    plugins: &[&dyn EditorPlugin],
) -> Result<()> {
    match &options.capture {
        Some(path) => {
            // Reproducible console timestamps: the capture's frame clock.
            log.set_manual_time(0.0);
            let mut editor = build_game_editor_with(options, log, game, plugins)?;
            // A capture is for automation: a project that failed to open is
            // an error, not a picture of the error dialog.
            if let Some(Dialog::Error { title, message }) = editor.dialog() {
                // Without the hint about what the window shows instead.
                let cause = message.split("\n\n").next().unwrap_or(message);
                bail!("{title}: {cause}");
            }
            editor.tree = layout::default_layout_for(editor.state.project.is_some());
            if !options.restore {
                headless::select_sample_entity(&mut editor);
            }
            let defaults = headless::CaptureOptions::default();
            let capture = headless::CaptureOptions {
                size: options.size.unwrap_or(defaults.size),
                frames: options.frames.unwrap_or(defaults.frames),
                ..defaults
            };
            let image = headless::capture(&mut editor, &capture)?;
            // End Play like Stop would (drops a project-less autosave).
            editor.state.stop_play();
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            image
                .save(path)
                .with_context(|| format!("could not write {}", path.display()))?;
            tracing::info!(
                "captured {} ({}x{} @ {}x, {} frames)",
                path.display(),
                capture.size[0],
                capture.size[1],
                capture.pixels_per_point,
                capture.frames
            );
            Ok(())
        }
        None => {
            let mut editor = build_game_editor_with(options, log, game, plugins)?
                .with_native_dialogs()
                .with_gamepads(box_engine::gamepad::GamepadPoller::new());
            editor.offer_crash_report();
            // A native game's editor keeps its registry file current for
            // `box` and `box mcp`.
            if let Some(project) = &editor.state.project {
                box_engine::registry_file::write_at_startup(editor.state.game(), project);
            }
            windowed::run(
                editor,
                options.size.unwrap_or([1440, 900]),
                options.frames.map(u64::from),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_arguments() {
        let o = EditorOptions::from_args(
            [
                "proj",
                "--capture",
                "a.png",
                "--size",
                "800x600",
                "--frames",
                "2",
                "--play",
                "--control",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(
            o,
            EditorOptions {
                project: Some("proj".into()),
                capture: Some("a.png".into()),
                size: Some([800, 600]),
                frames: Some(2),
                play: true,
                control: true,
                ..Default::default()
            }
        );
        let o = EditorOptions::from_args(["--no-restore", "--restore"].map(String::from)).unwrap();
        assert!(o.no_restore && o.restore && o.config_dir.is_none());
        let o = EditorOptions::from_args(["--write-registry", "--after-rebuild"].map(String::from))
            .unwrap();
        assert!(o.write_registry && o.after_rebuild);
        assert!(EditorOptions::from_args(["--bogus".to_string()]).is_err());
        let e = EditorOptions::from_args(["-x".to_string()]).unwrap_err();
        assert!(e.to_string().contains("--help"), "{e}");
        for h in ["-h", "--help"] {
            assert!(EditorOptions::from_args([h.to_string()]).unwrap().help);
        }
        for v in ["-V", "--version"] {
            assert!(EditorOptions::from_args([v.to_string()]).unwrap().version);
        }
        assert!(EditorOptions::from_args(["a", "b"].map(String::from)).is_err());
        assert!(EditorOptions::from_args(["--size".to_string()]).is_err());
    }
}
