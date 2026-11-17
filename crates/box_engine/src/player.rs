//! The player: run a project's start scene (or another scene) as a game.
//!
//! `box-player`, `box run` and a native project's own player binary all go
//! through [`run_game`]:
//!
//! ```no_run
//! fn main() -> anyhow::Result<()> {
//!     let options = box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?;
//!     box_engine::player::run_game(options, box_engine::Game::new())?;
//!     Ok(())
//! }
//! ```
//!
//! Scene component types the game's registry does not know are kept and
//! reported once per type at load ([`crate::scenes::warn_unknown_types`]).
//!
//! The project's `assets/` folder is served by an [`AssetServer`] resource
//! (hot reload on, except with `--no-watch` and in headless runs, which load
//! every asset before their first frame).
//!
//! **Exported games.** Given a `.pak` file instead of a project directory,
//! or with no project argument when a `game.pak` sits next to the
//! executable (or in `Contents/Resources` of a macOS app), the player mounts
//! the pak and plays the project inside it ([`crate::packed`]): read-only,
//! no hot reload, GPU validation per the export's profile, the window titled
//! with the exported name.
//!
//! Command line: `[project_dir] [--scene <name|path>] [--no-watch] [runner flags]`
//! (`--capture`, `--frames`, `--size`, `--report`, `--headless`,
//! `--control`; see [`RunOptions`]; `--help` prints [`USAGE`], an unknown
//! flag is an error with a suggestion). With `--control` the running player
//! serves the live control socket for its project (`box mcp` live tools).
//! Without a project directory the current directory (or the nearest parent
//! holding a `box.project.json`) is used.
//!
//! A native project ([`NativeCrate`]) plays with its own player binary,
//! which adds its game plugins; `box-player` refuses it and `box run` runs
//! that binary through cargo.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use box_assets::AssetServer;
use box_core::app::App;
use box_core::scene;

use crate::project::Project;
use crate::runner::{RunOptions, RunSummary, run_with};
use crate::scenes::Scenes;
use crate::{Game, ProjectError};

/// What to play and how.
#[derive(Clone, Debug)]
pub struct PlayerOptions {
    /// Project directory or any path inside it. `None`: the current directory.
    pub project: Option<PathBuf>,
    /// Scene to start with instead of the project's `start_scene`: a file
    /// (relative to the current directory or the project), or a scene name.
    pub scene: Option<String>,
    /// Window, capture and report options. The title defaults to the project name.
    pub run: RunOptions,
    /// Hot reload: watch `assets/` for changes (default on; `--no-watch`
    /// turns it off). Headless and capture runs never watch.
    pub watch: bool,
    /// `--write-registry`: write the game's registry to
    /// `<project>/.box/registry.json` (for `box` and `box mcp`) and exit
    /// without playing.
    pub write_registry: bool,
    /// `--upgrade`: migrate the project's files to this engine and the
    /// game's type versions (after a backup, see [`crate::upgrade`]) and
    /// exit without playing. A native project's game steps run only here,
    /// since `box upgrade` does not have the game's code.
    pub upgrade: bool,
    /// `--dry-run` (with `--upgrade`): print what would change, write nothing.
    pub dry_run: bool,
}

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            project: None,
            scene: None,
            run: RunOptions::new(""),
            watch: true,
            write_registry: false,
            upgrade: false,
            dry_run: false,
        }
    }
}

/// Runner flags that take a value, so their values are not read as the
/// project directory.
const VALUE_FLAGS: [&str; 4] = ["--capture", "--frames", "--report", "--size"];
/// Runner flags without a value.
const SWITCH_FLAGS: [&str; 2] = ["--headless", "--control"];
/// The player's own flags.
const PLAYER_FLAGS: [&str; 6] = [
    "--scene",
    "--version",
    "--no-watch",
    "--write-registry",
    "--upgrade",
    "--dry-run",
];

/// Command-line help of a player: printed by `-h`/`--help`
/// ([`PlayerOptions::from_args`]), after `Usage: <program> `.
pub const USAGE: &str = "[project_dir | game.pak] [options]

Plays a box project's start scene, or an exported game's pak. Without an
argument: the game.pak next to this program (an exported game), else the
current directory (or the nearest parent with a box.project.json).

Options:
  --scene <name|path>   Start with this scene instead of the start scene
                        (a scene name such as `level2`, or a file)
  --no-watch            Turn hot reload of assets off
  --capture <png>       Render headlessly, save the last frame and exit
                        (`{frame}` in the path saves every frame)
  --frames <n>          Exit after n frames (default 3 with --capture)
  --size <W>x<H>        Window size in logical pixels, or capture size
  --report <json>       Write frame timings and render statistics on exit
  --headless            No window: run --frames frames offscreen
  --control             Serve the live control socket for `box mcp`
                        (with --headless and no --frames: until `stop`)
  --write-registry      Write the game's types to <project>/.box/registry.json
                        (what `box` and `box mcp` read) and exit
  --upgrade             Migrate the project's files from an older engine or
                        older game types (backed up to .box/backups/ first)
                        and exit; with --dry-run, only print the changes
  -V, --version         Print the engine version and exit
  -h, --help            Print this help";

/// What `--version` prints: `box-player 0.1.0` like `box` and `box-editor`;
/// another player (a native game's, an exported game) names the engine,
/// since the number is not the game's version: `my-game (box-engine 0.1.0)`.
pub fn version_line(program: &str) -> String {
    let version = crate::project::ENGINE_VERSION;
    if program == "box-player" {
        format!("box-player {version}")
    } else {
        format!("{program} (box-engine {version})")
    }
}

/// The running program's file name (`box-player`, `player`).
pub fn program_name() -> String {
    std::env::args()
        .next()
        .and_then(|p| {
            Path::new(&p)
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "player".into())
}

impl PlayerOptions {
    /// Plays `dir` when no project was given and no `game.pak` is bundled
    /// with the executable. A native game's player calls this with its own
    /// folder, so `cargo player` plays the project while the same binary in
    /// an exported bundle plays its pak.
    pub fn default_project(&mut self, dir: impl Into<PathBuf>) {
        if self.project.is_none() && crate::packed::find_bundled_pak().is_none() {
            self.project = Some(dir.into());
        }
    }

    /// Parse `[project_dir] [--scene <name|path>] [runner flags]`. `-h` or
    /// `--help` prints [`USAGE`] and `-V` or `--version` prints
    /// [`version_line`], then exit the process (like a command-line parser);
    /// an unknown flag is an error that suggests the closest known flag.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let args: Vec<String> = args.into_iter().collect();
        if args.iter().any(|a| a == "-h" || a == "--help") {
            println!("Usage: {} {USAGE}", program_name());
            std::process::exit(0);
        }
        if args.iter().any(|a| a == "-V" || a == "--version") {
            println!("{}", version_line(&program_name()));
            std::process::exit(0);
        }
        let mut options = Self::default();
        let mut runner_args = Vec::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--scene" {
                options.scene = Some(args.next().context("--scene needs a value")?);
            } else if arg == "--no-watch" {
                options.watch = false;
            } else if arg == "--write-registry" {
                options.write_registry = true;
            } else if arg == "--upgrade" {
                options.upgrade = true;
            } else if arg == "--dry-run" {
                options.dry_run = true;
            } else if VALUE_FLAGS.contains(&arg.as_str()) {
                let value = args
                    .next()
                    .with_context(|| format!("{arg} needs a value"))?;
                runner_args.extend([arg, value]);
            } else if SWITCH_FLAGS.contains(&arg.as_str()) {
                runner_args.push(arg);
            } else if arg.starts_with('-') {
                let known = PLAYER_FLAGS
                    .iter()
                    .chain(&VALUE_FLAGS)
                    .chain(&SWITCH_FLAGS)
                    .copied();
                let hint = match box_core::reflect::did_you_mean(&arg, known) {
                    Some(flag) => format!("; did you mean `{flag}`?"),
                    None => String::new(),
                };
                bail!("unknown flag `{arg}`{hint} (--help lists the flags)");
            } else if let Some(project) = &options.project {
                bail!(
                    "unexpected argument `{arg}`: the project directory is already `{}` (--help lists the flags)",
                    project.display()
                );
            } else {
                options.project = Some(arg.into());
            }
        }
        if options.dry_run && !options.upgrade {
            bail!("--dry-run only applies to --upgrade");
        }
        options.run = options.run.with_arg_list(runner_args)?;
        Ok(options)
    }
}

/// The Cargo crate of a native project (a `Cargo.toml` at the project root):
/// its game code runs only in its own binaries, not in `box-player`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCrate {
    /// The crate's `Cargo.toml`.
    pub manifest: PathBuf,
    /// Binary targets: `[[bin]]` names, `src/bin/*.rs` and `src/main.rs`.
    pub bins: Vec<String>,
    /// `required-features` of `[[bin]]` targets that have them, by name.
    pub required_features: Vec<(String, Vec<String>)>,
}

impl NativeCrate {
    /// The crate at `project_root`, if it has a `Cargo.toml`.
    pub fn find(project_root: &Path) -> Option<Self> {
        let manifest = project_root.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).ok()?;
        // `[[bin]]` tables as (name, required features).
        let mut tables: Vec<(Option<String>, Vec<String>)> = Vec::new();
        let mut package = None;
        let mut section = String::new();
        let mut autobins = true;
        let strings = |value: &str| -> Vec<String> {
            value
                .split('"')
                .skip(1)
                .step_by(2)
                .map(str::to_string)
                .collect()
        };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.to_string();
                if section == "[[bin]]" {
                    tables.push((None, Vec::new()));
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.split('#').next().unwrap_or("").trim();
            let string = strings(value).into_iter().next();
            match (section.as_str(), key.trim(), tables.last_mut()) {
                ("[[bin]]", "name", Some(table)) => table.0 = string,
                ("[[bin]]", "required-features", Some(table)) => table.1 = strings(value),
                ("[package]", "name", _) => package = string,
                ("[package]", "autobins", _) => autobins = value != "false",
                _ => {}
            }
        }
        let mut bins: Vec<String> = tables.iter().filter_map(|t| t.0.clone()).collect();
        let required_features = tables
            .into_iter()
            .filter_map(|(name, features)| Some((name?, features)))
            .filter(|(_, features)| !features.is_empty())
            .collect();
        let src = project_root.join("src");
        if autobins {
            if let Ok(dir) = std::fs::read_dir(src.join("bin")) {
                let mut found: Vec<String> = dir
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "rs"))
                    .filter_map(|p| Some(p.file_stem()?.to_string_lossy().into_owned()))
                    .collect();
                found.sort();
                bins.extend(found);
            }
            if src.join("main.rs").is_file() {
                bins.extend(package);
            }
        }
        let mut seen = std::collections::HashSet::new();
        bins.retain(|b| seen.insert(b.clone()));
        Some(Self {
            manifest,
            bins,
            required_features,
        })
    }

    /// The binary that plays the game: `player`, else the only binary whose
    /// name contains "player".
    pub fn player_bin(&self) -> Option<&str> {
        self.bin_named("player")
    }

    /// The game's editor binary: `editor`, else the only binary whose name
    /// contains "editor".
    pub fn editor_bin(&self) -> Option<&str> {
        self.bin_named("editor")
    }

    fn bin_named(&self, role: &str) -> Option<&str> {
        if let Some(exact) = self.bins.iter().find(|b| *b == role) {
            return Some(exact);
        }
        let mut found = self.bins.iter().filter(|b| b.contains(role));
        match (found.next(), found.next()) {
            (Some(b), None) => Some(b),
            _ => None,
        }
    }

    /// The crate's directory (the project root).
    pub fn dir(&self) -> &Path {
        self.manifest.parent().unwrap_or(Path::new("."))
    }

    /// `cargo run [--features <required-features>] --bin <bin> -- <args>`,
    /// to run in [`dir`](Self::dir): from there rustup picks the crate's
    /// `rust-toolchain.toml`.
    pub fn cargo_run(&self, bin: &str, args: &[String]) -> Vec<String> {
        let mut command: Vec<String> = vec!["cargo".into(), "run".into()];
        if let Some((_, features)) = self.required_features.iter().find(|(b, _)| b == bin) {
            command.extend(["--features".to_string(), features.join(",")]);
        }
        command.extend(["--bin".to_string(), bin.to_string()]);
        if !args.is_empty() {
            command.push("--".into());
            command.extend(args.iter().cloned());
        }
        command
    }

    /// `cd <dir> && <command>` as one shell line.
    pub fn shell_line_in_dir(&self, command: &[String]) -> String {
        format!(
            "cd {} && {}",
            shell_line(&[self.dir().display().to_string()]),
            shell_line(command)
        )
    }
}

/// An error and its causes on one line (`a: b: c`), leaving out a cause
/// whose text is already there: contexts and wrapped errors often repeat
/// their source's message.
pub fn error_text(e: &anyhow::Error) -> String {
    let mut text = String::new();
    for cause in e.chain() {
        let part = cause.to_string();
        if text.contains(&part) {
            continue;
        }
        if !text.is_empty() {
            text.push_str(": ");
        }
        text.push_str(&part);
    }
    text
}

/// `words` as one shell command line, quoting words that need it.
pub fn shell_line(words: &[String]) -> String {
    words
        .iter()
        .map(|w| {
            let plain = !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
            if plain {
                w.clone()
            } else {
                format!("'{}'", w.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Open the project and load the scene into an app built by `game`: the
/// scene loads with the game's registry before the plugins are built, and
/// its entities become the main [scene instance](crate::scenes) with the
/// project root set for later scene requests. Returns the app and the
/// project.
pub fn build_app(game: &Game, options: &PlayerOptions) -> Result<(App, Project)> {
    let project = open_project(options)?;
    check_formats(game, &project)?;
    let scenes = Scenes::new().with_project_root(project.root());
    let path = match &options.scene {
        Some(s) if Path::new(s).is_file() => std::path::absolute(s)?,
        Some(s) => scenes
            .resolve(s)
            .map_err(|e| anyhow::anyhow!("{e}{}", available_scenes(&project)))?,
        None => project.start_scene_path(),
    };
    if options.scene.is_none() && !box_assets::vfs::is_file(&path) {
        bail!(
            "{}",
            missing_start_scene_text(&project, &available_scenes(&project))
        );
    }
    let bytes = box_assets::vfs::read(&path)
        .with_context(|| format!("could not read the scene {}", path.display()))?;
    let mut world = scene::load(&bytes, game.registry())
        .with_context(|| format!("could not load the scene {}", path.display()))?;
    tracing::info!(
        "project `{}`: playing {}",
        project.name(),
        project.relative(&path)
    );
    crate::scenes::warn_unknown_types(&world, &format!("`{}`", project.relative(&path)));
    world.insert_resource(scenes.with_start_scene(path));
    if let Some(server) = asset_server(&project, options)? {
        world.insert_resource(server);
    }
    Ok((game.app(world), project))
}

/// The message for a project whose `start_scene` file does not exist;
/// `scenes` lists the project's scenes (as from `available_scenes`).
pub fn missing_start_scene_text(project: &Project, scenes: &str) -> String {
    format!(
        "the start scene `{}` does not exist (set by `start_scene` in {}); create that scene, or point `start_scene` at an existing one{scenes}",
        project.file().start_scene,
        project.root().join(crate::project::PROJECT_FILE).display()
    )
}

/// The pak `options` play: a `.pak` file given as the project, or without
/// a project argument the `game.pak` bundled with this executable
/// ([`crate::packed::find_bundled_pak`]). `None` plays a project folder.
pub fn pak_to_play(options: &PlayerOptions) -> Option<PathBuf> {
    match &options.project {
        Some(p) => crate::packed::is_pak_path(p).then(|| p.clone()),
        None => crate::packed::find_bundled_pak(),
    }
}

/// Refuses a project with files from a newer engine or game; warns once
/// when files are older (they are migrated in memory on every load until
/// `--upgrade` or `box upgrade` writes them). Never writes.
fn check_formats(game: &Game, project: &Project) -> Result<()> {
    let plan = match crate::upgrade::plan(project.root(), &game.registry()) {
        Ok(plan) => plan,
        Err(e) => {
            tracing::warn!("could not check the project's file versions: {e}");
            return Ok(());
        }
    };
    if !plan.newer.is_empty() {
        bail!("{}", plan.summary());
    }
    for p in &plan.errors {
        tracing::warn!("{}: {}", p.path, p.message);
    }
    if plan.needed() && box_assets::vfs::is_read_only(project.root()) {
        tracing::warn!(
            "{} file(s) of this game are from an older engine or game version and are migrated in memory on every load; upgrade the project (`box upgrade`) and export it again",
            plan.files.len()
        );
    } else if plan.needed() {
        tracing::warn!(
            "{} file(s) of this project are from an older engine or game version and are migrated in memory on every load; upgrade them with `{} --upgrade` (or `box upgrade`)",
            plan.files.len(),
            program_name()
        );
    }
    Ok(())
}

/// `--upgrade [--dry-run]`: plans the project's upgrade with the game's
/// registry, prints the summary and, without `--dry-run`, applies it.
pub fn upgrade_project(
    game: &Game,
    options: &PlayerOptions,
) -> Result<crate::upgrade::UpgradePlan> {
    let project = open_project(options)?;
    let plan = crate::upgrade::plan(project.root(), &game.registry())?;
    println!("{}", plan.summary());
    if plan.blocked() {
        bail!("the project cannot be upgraded");
    }
    if options.dry_run {
        if plan.needed() {
            println!("(dry run: nothing was written)");
        }
        return Ok(plan);
    }
    if plan.needed() {
        let report = crate::upgrade::apply(&plan)?;
        println!(
            "upgraded {} file(s); the originals are in {}",
            report.written.len(),
            project.relative(&report.backup)
        );
    }
    Ok(plan)
}

/// The project of `options` (the current directory's without one), or the
/// mounted pak's ([`pak_to_play`]).
fn open_project(options: &PlayerOptions) -> Result<Project> {
    if let Some(pak) = pak_to_play(options) {
        let packed = crate::packed::mount(&pak)?;
        return Project::open(&packed.root)
            .with_context(|| format!("could not open the game data {}", pak.display()));
    }
    let dir = match &options.project {
        Some(p) => p.clone(),
        None => std::env::current_dir().context("could not read the current directory")?,
    };
    match Project::open(&dir) {
        Ok(p) => Ok(p),
        Err(ProjectError::NotFound(_)) if options.project.is_none() => bail!(
            "{} is not inside a box project (no box.project.json here or in a parent); pass a project directory",
            dir.display()
        ),
        Err(e) => Err(e).context("could not open the project"),
    }
}

/// `--write-registry`: writes the game's registry to the project's
/// `.box/registry.json` (see [`crate::registry_file`]), prints what it did
/// and returns the path.
pub fn write_registry(game: &Game, options: &PlayerOptions) -> Result<PathBuf> {
    let project = open_project(options)?;
    if project.is_packed() {
        bail!("an exported game has no project folder to write a registry into");
    }
    let path = crate::registry_file::registry_path(&project);
    let written = crate::registry_file::write_registry(&project, &game.registry())
        .with_context(|| format!("could not write {}", path.display()))?;
    let verb = match written {
        crate::registry_file::Written::Written => "wrote",
        crate::registry_file::Written::Unchanged => "up to date:",
    };
    println!("{verb} {}", path.display());
    Ok(path)
}

/// `"; available scenes: main (assets/scenes/main.scene.json), ..."`, or
/// that the project has none.
fn available_scenes(project: &Project) -> String {
    let files = project.scene_files().unwrap_or_default();
    if files.is_empty() {
        return "; the project has no scene files".into();
    }
    let list: Vec<String> = files
        .iter()
        .map(|f| {
            let rel = project.relative(f);
            let file = rel.rsplit('/').next().unwrap_or(&rel);
            let name = file
                .strip_suffix(crate::project::SCENE_EXTENSION)
                .unwrap_or(file);
            format!("{name} ({rel})")
        })
        .collect();
    format!("; available scenes: {}", list.join(", "))
}

/// The project's asset server (none when it has no `assets/` folder),
/// watching for changes unless `options` turn hot reload off, the run is
/// headless or the assets come from a pak.
fn asset_server(project: &Project, options: &PlayerOptions) -> Result<Option<AssetServer>> {
    let dir = project.assets_dir();
    if !box_assets::vfs::is_dir(&dir) {
        return Ok(None);
    }
    let server = crate::asset_server(&dir)
        .with_context(|| format!("could not open the assets folder {}", dir.display()))?;
    let headless = options.run.headless || options.run.capture.is_some();
    if options.watch
        && !headless
        && !server.is_read_only()
        && let Err(e) = server.watch(true)
    {
        tracing::warn!("hot reload is off: {e}");
    }
    Ok(Some(server))
}

/// [`build_app`], then run it with the runner (in a window, or headless with
/// `--capture` / `--headless`). Command-line flags are not re-read: parse
/// them into `options` with [`PlayerOptions::from_args`].
///
/// Same argument order as `box_editor::run_game(options, game)`, so a native
/// project's player and editor binaries read alike.
pub fn run_game(options: PlayerOptions, game: Game) -> Result<RunSummary> {
    // Before loading, so load-time warnings (unknown components) are shown.
    crate::init_logging();
    crate::crash::install("player");
    if options.write_registry {
        write_registry(&game, &options)?;
        return Ok(RunSummary::default());
    }
    if options.upgrade {
        upgrade_project(&game, &options)?;
        return Ok(RunSummary::default());
    }
    let (app, project) = build_app(&game, &options)?;
    let mut run = options.run;
    if project.is_packed() {
        let manifest = crate::packed::read_manifest(project.root())?;
        if run.control {
            bail!("--control is not available in an exported game (it has no project folder)");
        }
        box_render::set_gpu_validation(Some(
            manifest.profile == crate::packed::Profile::Development,
        ));
        // Nothing outside the pak: no engine shaders from a source checkout.
        box_render::set_shader_hot_reload(false);
        if run.title.is_empty() {
            run.title = manifest.name;
        }
    } else {
        // A native game keeps its registry file current for `box` and `box mcp`.
        crate::registry_file::write_at_startup(&game, &project);
    }
    if run.title.is_empty() {
        run.title = project.name().to_string();
    }
    run_with(app, run)
}

/// [`run_game`] with the arguments the other way round.
#[deprecated(note = "use run_game(options, game)")]
pub fn run(game: Game, options: PlayerOptions) -> Result<RunSummary> {
    run_game(options, game)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenes::SceneInstance;
    use box_core::identity::Uid;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_project_scene_and_runner_flags() {
        let o = PlayerOptions::from_args(args(&[
            "--frames",
            "5",
            "my-game",
            "--scene",
            "level2",
            "--headless",
        ]))
        .unwrap();
        assert_eq!(o.project.as_deref(), Some(Path::new("my-game")));
        assert_eq!(o.scene.as_deref(), Some("level2"));
        assert_eq!(o.run.frames, Some(5));
        assert!(o.run.headless);
        assert!(PlayerOptions::from_args(args(&["a", "b"])).is_err());
        assert!(PlayerOptions::from_args(args(&["--scene"])).is_err());
        assert!(PlayerOptions::from_args(args(&["--frames", "x"])).is_err());
        let typo = PlayerOptions::from_args(args(&["--framez", "3"]))
            .unwrap_err()
            .to_string();
        assert!(
            typo.contains("unknown flag `--framez`; did you mean `--frames`?"),
            "{typo}"
        );
        let unknown = PlayerOptions::from_args(args(&["--bogus"]))
            .unwrap_err()
            .to_string();
        assert!(
            unknown.contains("unknown flag `--bogus`") && !unknown.contains("did you mean"),
            "{unknown}"
        );
    }

    #[test]
    fn version_lines() {
        let v = crate::project::ENGINE_VERSION;
        assert_eq!(version_line("box-player"), format!("box-player {v}"));
        assert_eq!(version_line("my-game"), format!("my-game (box-engine {v})"));
    }

    #[test]
    fn parses_upgrade_flags() {
        let o = PlayerOptions::from_args(args(&["game", "--upgrade", "--dry-run"])).unwrap();
        assert!(o.upgrade && o.dry_run);
        let e = PlayerOptions::from_args(args(&["--dry-run"]))
            .unwrap_err()
            .to_string();
        assert!(e.contains("--dry-run only applies to --upgrade"), "{e}");
    }

    #[test]
    fn error_text_drops_repeated_causes() {
        let e = anyhow::Error::msg("key must be a string")
            .context("invalid JSON: key must be a string")
            .context("could not load the scene x");
        assert_eq!(
            error_text(&e),
            "could not load the scene x: invalid JSON: key must be a string"
        );
    }

    #[test]
    fn finds_a_native_crates_binaries() {
        let root = std::env::temp_dir().join(format!("box_native_{}", Uid::new()));
        std::fs::create_dir_all(root.join("src/bin")).unwrap();
        assert!(NativeCrate::find(&root).is_none());
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"my-game\" # comment\n\n[[bin]]\npath = \"src/bin/editor.rs\"\nrequired-features = [\"editor\"] # egui\nname = \"editor\"\n\n[[bin]]\nname = \"game-player\"\npath = \"src/bin/p.rs\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/bin/editor.rs"), "").unwrap();
        std::fs::write(root.join("src/bin/tool.rs"), "").unwrap();
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        let native = NativeCrate::find(&root).unwrap();
        assert_eq!(native.bins, ["editor", "game-player", "tool", "my-game"]);
        assert_eq!(native.player_bin(), Some("game-player"));
        assert_eq!(native.editor_bin(), Some("editor"));
        assert_eq!(
            native.cargo_run("editor", &[".".into()]),
            [
                "cargo",
                "run",
                "--features",
                "editor",
                "--bin",
                "editor",
                "--",
                "."
            ]
        );
        assert_eq!(native.dir(), root.as_path());
        let command = native.cargo_run("game-player", &[".".into(), "--frames".into()]);
        assert_eq!(
            command,
            [
                "cargo",
                "run",
                "--bin",
                "game-player",
                "--",
                ".",
                "--frames"
            ]
        );
        assert_eq!(
            shell_line(&["a b".into(), "it's".into(), "--x=1".into()]),
            "'a b' 'it'\\''s' --x=1"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_folder_or_start_scene_says_what_to_do() {
        let root = std::env::temp_dir().join(format!("box_player_missing_{}", Uid::new()));
        let gone = PlayerOptions {
            project: Some(root.clone()),
            ..Default::default()
        };
        let err = error_text(&build_app(&Game::new(), &gone).err().unwrap());
        assert!(
            err.contains("does not exist; check the path (was the project moved or renamed?)"),
            "{err}"
        );
        let project = Project::create(&root, "Missing").unwrap();
        std::fs::rename(
            project.start_scene_path(),
            root.join("assets/scenes/level.scene.json"),
        )
        .unwrap();
        let err = error_text(&build_app(&Game::new(), &gone).err().unwrap());
        assert!(
            err.starts_with(
                "the start scene `assets/scenes/main.scene.json` does not exist (set by `start_scene` in "
            ),
            "{err}"
        );
        assert!(
            err.ends_with(
                "point `start_scene` at an existing one; available scenes: level (assets/scenes/level.scene.json)"
            ),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn builds_an_app_from_the_start_scene_or_another_scene() {
        let root = std::env::temp_dir().join(format!("box_player_{}", Uid::new()));
        let project = Project::create(&root, "Player Test").unwrap();
        let options = PlayerOptions {
            project: Some(root.join("assets")),
            ..Default::default()
        };
        let (mut app, opened) = build_app(&Game::new(), &options).unwrap();
        assert_eq!(opened.root(), project.root());
        app.update();
        let scenes = app.world.resource::<Scenes>();
        assert_eq!(scenes.project_root(), Some(project.root()));
        let main = scenes.main().unwrap();
        assert_eq!(
            scenes.path(main),
            Some(project.start_scene_path().as_path())
        );
        assert_eq!(app.world.query::<&SceneInstance>().iter().count(), 4);

        let missing = PlayerOptions {
            project: Some(root.clone()),
            scene: Some("nope".into()),
            ..Default::default()
        };
        let err = build_app(&Game::new(), &missing).err().unwrap().to_string();
        assert!(err.contains("not found"), "{err}");
        assert!(
            err.contains("available scenes: main (assets/scenes/main.scene.json)"),
            "{err}"
        );
        let by_name = PlayerOptions {
            project: Some(root.clone()),
            scene: Some("main".into()),
            ..Default::default()
        };
        assert!(build_app(&Game::new(), &by_name).is_ok());
        std::fs::remove_dir_all(&root).unwrap();

        let not_a_project = PlayerOptions {
            project: Some(std::env::temp_dir()),
            ..Default::default()
        };
        assert!(build_app(&Game::new(), &not_a_project).is_err());
    }

    #[test]
    fn unknown_components_load_and_are_reported() {
        let root = std::env::temp_dir().join(format!("box_player_unknown_{}", Uid::new()));
        let project = Project::create(&root, "Unknown Test").unwrap();
        let path = project.start_scene_path();
        let mut tree: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        tree["entities"][0]["components"]["Primitve"] = serde_json::json!({ "x": 1 });
        std::fs::write(&path, serde_json::to_vec(&tree).unwrap()).unwrap();
        let options = PlayerOptions {
            project: Some(root.clone()),
            ..Default::default()
        };
        let (app, _) = build_app(&Game::new(), &options).unwrap();
        let unknown = crate::scenes::warn_unknown_types(&app.world, "`main`");
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].name, "Primitve");
        assert_eq!(unknown[0].entities, 1);
        assert_eq!(unknown[0].did_you_mean, Some("Primitive"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn write_registry_writes_the_file_and_run_game_exits() {
        let root = std::env::temp_dir().join(format!("box_player_registry_{}", Uid::new()));
        Project::create(&root, "Registry").unwrap();
        let options =
            PlayerOptions::from_args([root.display().to_string(), "--write-registry".into()])
                .unwrap();
        assert!(options.write_registry);
        let summary = run_game(options, Game::new()).unwrap();
        assert_eq!(summary.frames, 0, "nothing played");
        let text = std::fs::read_to_string(root.join(".box/registry.json")).unwrap();
        assert!(text.contains("\"engine_version\""), "{text}");
        assert!(text.contains("\"Transform\""), "{text}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn run_game_and_run_take_the_same_inputs() {
        // Both fail the same way on a folder that is not a project.
        let options = PlayerOptions {
            project: Some(std::env::temp_dir()),
            ..Default::default()
        };
        assert!(run_game(options.clone(), Game::new()).is_err());
        assert!(run_game(options, Game::new()).is_err());
    }
}
