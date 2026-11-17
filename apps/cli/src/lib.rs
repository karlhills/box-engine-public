//! The `box` command-line tool (decision D13): create, check, inspect, edit and
//! render projects without a window.
//!
//! The binary is a thin wrapper around [`run`], so tests and the MCP server
//! ([`mcp`], `box mcp`) call the same functions. Every command returns an [`Output`]
//! with a JSON value (printed with `--json`) and a human-readable text. Scene
//! edits go through `box_core::edit`, the same command layer the editor uses.

mod error;
mod export_cmd;
mod game_registry;
mod import_cmd;
pub mod mcp;
mod native;
mod problems;
mod render;
mod run_cmd;
mod scene_cmd;
mod script_cmd;
mod upgrade_cmd;

use std::path::{Path, PathBuf};

use box_engine::Project;
use box_engine::box_assets::LoadState;
use box_engine::box_core::asset_ref::AssetRef;
use box_engine::box_core::reflect::Registry;
use box_engine::box_core::scene::{self, UnknownComponents, unknown_types};
use box_engine::box_core::world::{Entity, World};
use box_engine::project::ENGINE_VERSION;
use box_engine::templates::{self, DEFAULT_TEMPLATE, NATIVE_TEMPLATE, Template, TemplateError};
use clap::{Args, Parser, Subcommand};
use serde_json::{Value as Json, json};

pub use error::{CliError, ErrorBody};
pub use problems::collecting;
pub use scene_cmd::COMMANDS_HELP;

/// Command-line interface of `box`.
#[derive(Debug, Parser)]
#[command(name = "box", version, about = "box-engine command-line tool", long_about = None)]
pub struct Cli {
    /// Print a machine-readable JSON result on stdout (errors as `{ "error": { ... } }`).
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a project from a template (default `empty`: a ground, a cube, a sun and a camera). `--list-templates` lists them.
    New {
        /// Directory to create the project in (created if missing).
        #[arg(required_unless_present = "list_templates")]
        dir: Option<PathBuf>,
        /// Project name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
        /// The template: empty, third-person, procedural or native (see --list-templates).
        #[arg(long, short = 't')]
        template: Option<String>,
        /// List the templates (name, title, description, folder) instead of creating a project.
        #[arg(long, conflicts_with_all = ["dir", "name", "template", "native", "engine_path"])]
        list_templates: bool,
        /// Same as `--template native`: also create a Cargo crate, the game plugin (src/lib.rs) with its own editor and player binaries. On an existing project, adds only the crate.
        #[arg(long)]
        native: bool,
        /// box-engine checkout the native crate depends on (default: the one this CLI was built from).
        #[arg(long)]
        engine_path: Option<PathBuf>,
    },
    /// Validate a project: parse every *.scene.json under assets/, import every asset file and resolve every asset reference; report problems (exit code 1 on errors).
    Check {
        /// Project directory, or any path inside it (default: the current directory).
        project: Option<PathBuf>,
    },
    /// Upgrade a project made with an older engine (or older game types): back up every file that changes to .box/backups/<time>/, migrate scenes, materials, animation graphs, settings and .meta files, and record this engine's version. Files from a newer engine are refused.
    Upgrade {
        /// Project directory, or any path inside it (default: the current directory).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Report what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// List component and resource types with fields, ranges, units, docs and defaults. In a native project, includes the game's types from <project>/.box/registry.json (written by the game's editor and player).
    Registry {
        /// Project whose game types to include (default: the project containing the current directory, if any).
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Inspect and edit scene files.
    #[command(subcommand)]
    Scene(SceneCommand),
    /// Luau script tools.
    #[command(subcommand)]
    Script(ScriptCommand),
    /// Render a scene headlessly to a PNG.
    Render(RenderArgs),
    /// Copy a file into the project's assets/, write its .meta and import it once; prints its id and sub-assets.
    Import(ImportArgs),
    /// Play a project (or one of its scene files): in a window, or headless with --frames or --capture. A native project (with a Cargo.toml) runs its own player through `cargo run`.
    Run(RunArgs),
    /// Export a runnable game: cook the project into game.pak and bundle it with the player as a macOS .app (ad-hoc signed) or a Windows folder (<Name>.exe + game.pak). A native project builds its own player with `cargo build --release`.
    Export(ExportArgs),
    /// Serve the Model Context Protocol on stdio, for AI agents (MCP clients launch this).
    #[command(after_help = mcp::registration_help())]
    Mcp {
        /// Default project for tools (default: the project containing the current directory).
        #[arg(long)]
        project: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SceneCommand {
    /// List the scene files of a project.
    List {
        /// Project directory, or any path inside it (default: the current directory).
        project: Option<PathBuf>,
    },
    /// Show entities, hierarchy and component values.
    Show {
        /// Scene file (relative to the current directory or the project root), or a project directory for its start scene.
        scene: PathBuf,
        /// Show only this entity (a uid or a unique name).
        #[arg(long)]
        entity: Option<String>,
    },
    /// Apply editor commands (JSON) as one undoable transaction and save the scene.
    #[command(after_help = COMMANDS_HELP)]
    Apply {
        /// Scene file (relative to the current directory or the project root).
        scene: PathBuf,
        /// JSON file with one command or an array of commands; `-` reads stdin.
        commands: String,
        /// Validate and report without writing the scene.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ScriptCommand {
    /// Write <project>/.box/types/box.d.luau, the luau-lsp definitions of the script API (every component, resource and event type, entity methods, world, input, physics, ...), and point VS Code's luau-lsp at it (.vscode/settings.json, when the project has none).
    Types {
        /// Project directory, or any path inside it (default: the current directory).
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Write the script API reference as Markdown: every global, function, method, callback, rule type and registered component, resource and event, with descriptions. Without a project (outside one), the engine's own reference.
    Docs {
        /// Project whose game types to include (default: the project containing the current directory, if any).
        #[arg(long)]
        project: Option<PathBuf>,
        /// Output folder (default: <project>/.box/reference; required outside a project).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct RenderArgs {
    /// Scene file (relative to the current directory or the project root), or a project directory for its start scene.
    pub scene: PathBuf,
    /// Output PNG (relative to the current directory; parent directories are created).
    #[arg(long)]
    pub out: PathBuf,
    /// Image size in pixels.
    #[arg(long, default_value = "1280x720", value_parser = parse_size)]
    pub size: (u32, u32),
    /// Camera entity (uid or unique name). Default: the scene's first active camera, else an automatic camera framing the scene.
    #[arg(long)]
    pub camera: Option<String>,
    /// Frames to simulate and render before the capture.
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u32).range(1..=600))]
    pub frames: u32,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// The file to import (a .glb/.gltf with its buffers and images, .png, .jpg, .material.json, .wgsl, .scene.json). A file already inside assets/ is imported in place.
    pub file: PathBuf,
    /// Project directory, or any path inside it (default: the current directory).
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// Folder under assets/ to copy into (default by type: models, textures, materials, shaders, scenes).
    #[arg(long)]
    pub to: Option<String>,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Project directory, or any path inside it (default: the current directory).
    #[arg(long)]
    pub project: Option<PathBuf>,
    /// Platform: macos or windows (default: this one).
    #[arg(long)]
    pub target: Option<String>,
    /// release (no GPU validation, no console window) or development (GPU validation, console on Windows).
    #[arg(long, default_value = "release")]
    pub profile: String,
    /// Folder to write the bundle into (created if missing; an earlier export there is replaced).
    #[arg(long)]
    pub out: PathBuf,
    /// Bundle this player executable instead of the template (script projects) or the cargo build (native projects).
    #[arg(long)]
    pub player: Option<PathBuf>,
    /// Do not sign the macOS bundle (Apple Silicon Macs then refuse to run it).
    #[arg(long)]
    pub no_sign: bool,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Project directory, any path inside it, or a scene file (default: the current directory).
    pub target: Option<PathBuf>,
    /// Run this many frames, then exit. Without --capture the run is headless (no window).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub frames: Option<u64>,
    /// Write frame timings and render statistics as JSON.
    #[arg(long)]
    pub report: Option<PathBuf>,
    /// Render headlessly and save the last frame as a PNG (`{frame}` in the path saves every frame).
    #[arg(long)]
    pub capture: Option<PathBuf>,
    /// Window size in logical pixels, or headless size in pixels (default 1280x720).
    #[arg(long, value_parser = parse_size)]
    pub size: Option<(u32, u32)>,
    /// Serve the live control socket for `box mcp` live tools while the game runs (until it exits or the `stop` method ends it).
    #[arg(long)]
    pub control: bool,
    /// Turn hot reload of assets off (it is on in windowed runs).
    #[arg(long)]
    pub no_watch: bool,
    /// For a native project (one with a Cargo.toml): print the cargo command that runs its player instead of running it.
    #[arg(long)]
    pub no_cargo: bool,
}

fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("expected WxH, got `{s}`"))?;
    let parse = |v: &str| {
        v.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=16384).contains(n))
            .ok_or_else(|| format!("`{v}` is not a size between 1 and 16384"))
    };
    Ok((parse(w)?, parse(h)?))
}

/// The result of a command.
#[derive(Debug)]
pub struct Output {
    /// Printed with `--json`.
    pub json: Json,
    /// Printed without `--json`.
    pub text: String,
    /// False when the command ran but found problems (`box check`): exit code 1.
    pub success: bool,
}

impl Output {
    fn ok(json: Json, text: impl Into<String>) -> Self {
        Self {
            json,
            text: text.into(),
            success: true,
        }
    }
}

/// Run one parsed command.
pub fn run(cli: &Cli) -> Result<Output, CliError> {
    dispatch(cli).map_err(|mut e| {
        // Report the file the way results do: relative to its project.
        if let Some(file) = e.file.clone()
            && Path::new(&file).exists()
            && let (Json::String(project), rel) = locate(Path::new(&file))
            && !rel.is_empty()
        {
            e.file = Some(rel);
            e = e.with("project", json!(project));
        }
        e
    })
}

fn dispatch(cli: &Cli) -> Result<Output, CliError> {
    match &cli.command {
        Command::New {
            list_templates: true,
            ..
        } => Ok(list_templates()),
        Command::New {
            dir,
            name,
            template,
            native,
            engine_path,
            ..
        } => {
            let dir = dir
                .as_deref()
                .ok_or_else(|| CliError::new("usage", "box new needs a directory"))?;
            let template = match (template.as_deref(), *native) {
                (Some(t), true) if t != NATIVE_TEMPLATE => {
                    return Err(CliError::new(
                        "usage",
                        format!(
                            "--native means `--template native`; it cannot be combined with `--template {t}`"
                        ),
                    ));
                }
                (_, true) => NATIVE_TEMPLATE,
                (Some(t), false) => t,
                (None, false) => DEFAULT_TEMPLATE,
            };
            new_project(dir, name.as_deref(), template, engine_path.as_deref())
        }
        Command::Check { project } => check(project.as_deref()),
        Command::Upgrade { project, dry_run } => upgrade_cmd::upgrade(project.as_deref(), *dry_run),
        Command::Registry { project } => registry(project.as_deref()),
        Command::Scene(SceneCommand::List { project }) => scene_list(project.as_deref()),
        Command::Scene(SceneCommand::Show { scene, entity }) => {
            scene_cmd::show(scene, entity.as_deref())
        }
        Command::Scene(SceneCommand::Apply {
            scene,
            commands,
            dry_run,
        }) => scene_cmd::apply(scene, commands, *dry_run),
        Command::Script(ScriptCommand::Types { project }) => script_cmd::types(project.as_deref()),
        Command::Script(ScriptCommand::Docs { project, out }) => {
            script_cmd::docs(project.as_deref(), out.as_deref())
        }
        Command::Render(args) => render::render(args),
        Command::Import(args) => import_cmd::import(args),
        Command::Run(args) => run_cmd::run(args, cli.json),
        Command::Export(args) => export_cmd::export(args),
        Command::Mcp { project } => {
            // stdout carries only protocol messages.
            if cli.json {
                return Err(CliError::new(
                    "usage",
                    "`box mcp` speaks MCP on stdout; --json does not apply",
                ));
            }
            mcp::serve_stdio(project.as_deref())?;
            Ok(Output::ok(Json::Null, ""))
        }
    }
}

// ------------------------------------------------------------ helpers ----

pub(crate) fn cwd() -> Result<PathBuf, CliError> {
    std::env::current_dir().map_err(|e| CliError::io(Path::new("."), e))
}

fn open_project(path: Option<&Path>) -> Result<Project, CliError> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => cwd()?,
    };
    if !path.exists() {
        // The message says to check the path or create a project there.
        let message = box_engine::ProjectError::NotFound(path.clone()).to_string();
        return Err(CliError::new("not_found", message).file(&path));
    }
    Ok(Project::open(path)?)
}

/// An input path as given if it exists (relative to the current directory),
/// otherwise relative to the root of the project containing the current
/// directory.
pub(crate) fn resolve_input(path: &Path) -> Result<PathBuf, CliError> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    if path.is_relative()
        && let Some(root) = Project::find_root(&cwd()?)
    {
        let candidate = root.join(path);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(CliError::new(
        "not_found",
        format!(
            "{} does not exist (looked in the current folder and the project's folder)",
            path.display()
        ),
    )
    .file(path))
}

/// A scene argument: a scene file, or a project directory (its start scene).
pub(crate) fn resolve_scene(path: &Path) -> Result<PathBuf, CliError> {
    let resolved = resolve_input(path)?;
    if resolved.is_dir() {
        let project = Project::open(&resolved)?;
        return Ok(project.start_scene_path());
    }
    Ok(resolved)
}

/// Load a scene file with its project's registry (the engine's types plus a
/// native project's game types from `.box/registry.json`).
pub(crate) fn load_scene(path: &Path) -> Result<World, CliError> {
    load_scene_with(path, game_registry::for_path(path).registry)
}

/// Load a scene file with `registry`.
pub(crate) fn load_scene_with(
    path: &Path,
    registry: std::sync::Arc<Registry>,
) -> Result<World, CliError> {
    let bytes = std::fs::read(path).map_err(|e| CliError::io(path, e))?;
    scene::load(&bytes, registry).map_err(|e| CliError::scene(path, e))
}

/// Saved entities in hierarchy pre-order (the scene-file order).
pub(crate) fn hierarchy_order(world: &World) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = world.roots().iter().rev().copied().collect();
    while let Some(e) = stack.pop() {
        out.push(e);
        stack.extend(world.children(e).into_iter().rev());
    }
    out
}

/// How results name a file: the root of the project containing it (JSON
/// `null` outside any project) and the path relative to that root with `/`
/// separators, or the path as given outside a project.
pub(crate) fn locate(path: &Path) -> (Json, String) {
    match Project::open(path) {
        Ok(project) => (
            json!(project.root().display().to_string()),
            project.relative(path),
        ),
        Err(_) => (Json::Null, path.display().to_string()),
    }
}

/// A path for output: relative to the project when inside it.
fn display_path(project: Option<&Project>, path: &Path) -> String {
    match project {
        Some(p) => p.relative(path),
        None => path.display().to_string(),
    }
}

// ----------------------------------------------------------- commands ----

impl From<TemplateError> for CliError {
    fn from(e: TemplateError) -> Self {
        match e {
            TemplateError::Project(e) => e.into(),
            TemplateError::UnknownTemplate { ref available, .. }
            | TemplateError::UnknownExample { ref available, .. } => {
                let list = json!(available);
                CliError::new("usage", e.to_string()).with("available", list)
            }
            TemplateError::AlreadyExists(ref p) => {
                CliError::new("already_exists", e.to_string()).file(p)
            }
            TemplateError::Io { ref path, .. } => CliError::new("io", e.to_string()).file(path),
            TemplateError::Native(_) => CliError::new("usage", e.to_string()),
            TemplateError::Invalid { ref path, .. } => {
                CliError::new("template_invalid", e.to_string()).file(path)
            }
        }
    }
}

fn template_json(t: &Template) -> Json {
    json!({
        "name": t.name,
        "title": t.title,
        "description": t.description,
        "native": t.native,
        "folder": t.dir.as_ref().map(|d| d.display().to_string()),
        "preview": t.preview().map(|p| p.display().to_string()),
    })
}

/// `box new --list-templates`.
fn list_templates() -> Output {
    let list = templates::templates();
    let width = list.iter().map(|t| t.name.len()).max().unwrap_or(0);
    let mut text = String::from("Templates (box new <dir> --template <name>):");
    for t in &list {
        let default = if t.name == DEFAULT_TEMPLATE {
            " (default)"
        } else {
            ""
        };
        text.push_str(&format!(
            "\n  {:width$}  {}{default}: {}",
            t.name, t.title, t.description
        ));
    }
    Output::ok(
        json!({
            "templates": list.iter().map(template_json).collect::<Vec<_>>(),
            "default": DEFAULT_TEMPLATE,
        }),
        text,
    )
}

fn new_project(
    dir: &Path,
    name: Option<&str>,
    template: &str,
    engine_path: Option<&Path>,
) -> Result<Output, CliError> {
    let name = match name {
        Some(n) => n.to_string(),
        None => std::path::absolute(dir)
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "New Project".to_string()),
    };
    let native = template == NATIVE_TEMPLATE;
    if engine_path.is_some() && !native {
        return Err(CliError::new(
            "usage",
            "--engine-path is for native projects (`--native` or `--template native`)",
        ));
    }
    let list = templates::templates();
    // `native` works without a templates folder: the empty project plus the crate.
    let chosen = match templates::find_template(&list, template) {
        Ok(t) => t.clone(),
        Err(_) if native => templates::find_template(&list, DEFAULT_TEMPLATE)?.clone(),
        Err(e) => return Err(e.into()),
    };
    // `--native` on an existing project adds only the crate.
    let existing = native && dir.join(box_engine::project::PROJECT_FILE).is_file();
    // Everything that can refuse runs before the first file is written.
    let engine = if native {
        native::check_free(dir)?;
        Some(native::engine_path(engine_path)?)
    } else {
        None
    };
    let project = if existing {
        Project::open(dir)?
    } else {
        templates::create_data_project(dir, &name, &list, &chosen).map_err(|e| {
            let err = CliError::from(e);
            if err.code == "already_exists" {
                err.with(
                    "hint",
                    json!(
                        "choose another directory; `box new --native <dir>` adds a native crate to an existing project"
                    ),
                )
            } else {
                err
            }
        })?
    };
    let shown = shell_word(&dir.display().to_string());
    let mut json = json!({
        "project": project.root().display().to_string(),
        "name": project.name(),
        "template": template,
        "engine_version": ENGINE_VERSION,
        "project_file": project.project_file_path().display().to_string(),
        "start_scene": project.file().start_scene,
        "created": !existing,
    });
    let mut text = if existing {
        format!(
            "Added a native crate to project `{}` in {}",
            project.name(),
            project.root().display()
        )
    } else {
        format!(
            "Created project `{}` from template `{template}` in {}\nstart scene: {}",
            project.name(),
            project.root().display(),
            project.file().start_scene
        )
    };
    if !existing && project.root().join("README.md").is_file() {
        text.push_str("\nREADME.md says what is in it");
    }
    let next: Vec<String> = match engine {
        Some(engine) => {
            let files = native::write(project.root(), project.name(), &engine)?;
            json["native"] = json!({
                "crate": native::crate_name(project.name()),
                "engine_path": engine.display().to_string(),
                "files": files,
            });
            text.push_str(&format!(
                "\nnative crate `{}` (engine: {}); files: {}",
                native::crate_name(project.name()),
                engine.display(),
                files.join(", ")
            ));
            // Commands for the crate's own binaries, as its Cargo.toml has them.
            let mut next = Vec::new();
            if let Some(krate) = box_engine::player::NativeCrate::find(project.root()) {
                for bin in [krate.editor_bin(), krate.player_bin()]
                    .into_iter()
                    .flatten()
                {
                    let command =
                        box_engine::player::shell_line(&krate.cargo_run(bin, &[".".into()]));
                    next.push(format!("cd {shown} && {command}"));
                }
            }
            next.push(format!("box run {shown}"));
            next
        }
        None => vec![
            format!("box-editor {shown}"),
            format!("box run {shown}"),
            format!("box render {shown} --out shot.png"),
        ],
    };
    text.push_str(&format!("\nNext: {}", next.join(" · ")));
    json["next"] = json!(next);
    Ok(Output::ok(json, text))
}

/// `word` quoted for a shell when it needs it.
fn shell_word(word: &str) -> String {
    box_engine::player::shell_line(&[word.to_string()])
}

/// The first sentence of a doc comment, on one line.
fn first_sentence(doc: &str) -> String {
    let text = doc.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.find(". ") {
        Some(end) => text[..=end].to_string(),
        None => text,
    }
}

/// A field type as people write it in scene JSON: `number`, `vec3 [x, y, z]`,
/// an enum's variants, ... instead of a Rust type path.
fn friendly_type(type_path: &str, registry: &box_engine::box_core::reflect::Registry) -> String {
    use box_engine::box_core::reflect::TypeInfo;
    use box_engine::box_core::reflect::bevy_reflect::enums::VariantInfo;
    if let Some(inner) = type_path
        .strip_prefix("alloc::vec::Vec<")
        .and_then(|t| t.strip_suffix('>'))
    {
        return format!("list of {}", friendly_type(inner, registry));
    }
    if let Some(inner) = type_path
        .strip_prefix("core::option::Option<")
        .and_then(|t| t.strip_suffix('>'))
    {
        return format!("{} or null", friendly_type(inner, registry));
    }
    let known = match type_path {
        "f32" | "f64" => "number",
        "u8" | "u16" | "u32" | "u64" | "usize" => "integer >= 0",
        "i8" | "i16" | "i32" | "i64" | "isize" => "integer",
        "bool" => "true or false",
        "alloc::string::String" => "string",
        "uuid::Uuid" => "uuid string",
        "glam::Vec2" => "vec2 [x, y]",
        "glam::Vec3" => "vec3 [x, y, z]",
        "glam::Vec4" => "vec4 [x, y, z, w]",
        "glam::UVec2" => "uvec2 [x, y]",
        "glam::UVec3" => "uvec3 [x, y, z]",
        "glam::Quat" => "quaternion [x, y, z, w]",
        "box_engine::Color" => "linear colour [r, g, b, a]",
        "box_engine::AssetRef" => r#"asset: null or {"path": "models/a.glb", "label"?: "Mesh/A"}"#,
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let short = type_path.rsplit("::").next().unwrap_or(type_path);
    let info = registry
        .type_registry()
        .get_with_type_path(type_path)
        .map(|r| r.type_info());
    match info {
        Some(TypeInfo::Enum(e)) => {
            let variants: Vec<String> = e
                .iter()
                .map(|v| match v {
                    VariantInfo::Unit(u) => format!("\"{}\"", u.name()),
                    VariantInfo::Struct(s) => {
                        let fields: Vec<&str> = s.iter().map(|f| f.name()).collect();
                        format!("{{\"{}\": {{{}}}}}", s.name(), fields.join(", "))
                    }
                    VariantInfo::Tuple(t) => format!("{{\"{}\": [..{}]}}", t.name(), t.field_len()),
                })
                .collect();
            format!("{short}: {}", variants.join(" | "))
        }
        Some(TypeInfo::Struct(s)) => {
            let fields: Vec<&str> = s.iter().map(|f| f.name()).collect();
            format!("{short} {{{}}}", fields.join(", "))
        }
        _ => short.to_string(),
    }
}

fn registry(project: Option<&Path>) -> Result<Output, CliError> {
    let project = match project {
        Some(p) => Some(open_project(Some(p))?),
        None => Project::find_root(&cwd()?).and_then(|r| Project::open(r).ok()),
    };
    let loaded = project.as_ref().map(game_registry::for_project);
    let registry = loaded
        .as_ref()
        .map_or_else(box_engine::scene_glue::shared_engine_registry, |l| {
            l.registry.clone()
        });
    let mut json = registry.to_json();
    // The game's types (schema-only here), marked, in name order.
    for (list, resource) in [("components", false), ("resources", true)] {
        let entries = json[list].as_array_mut().expect("to_json has both lists");
        for schema in registry.schemas().filter(|s| s.is_resource() == resource) {
            let mut entry = schema.entry().clone();
            entry["game"] = json!(true);
            entries.push(entry);
        }
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    }
    let mut problems = Vec::new();
    if let (Some(project), Some(loaded)) = (&project, &loaded) {
        json["project"] = json!(project.root().display().to_string());
        json["game_registry"] = loaded.summary(project);
        problems.extend(loaded.problems.iter().cloned());
    }
    json["problems"] = json!(problems);
    let mut text = String::new();
    for (kind, list) in [("component", "components"), ("resource", "resources")] {
        for t in json[list].as_array().into_iter().flatten() {
            let fields: Vec<String> = t["fields"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|f| {
                    let mut s = format!(
                        "{}: {}",
                        f["name"].as_str().unwrap_or("?"),
                        friendly_type(f["type_path"].as_str().unwrap_or("?"), &registry)
                    );
                    if let Some(u) = f["unit"].as_str() {
                        s.push_str(&format!(" [{u}]"));
                    }
                    if let (Some(min), Some(max)) =
                        (f["range"]["min"].as_f64(), f["range"]["max"].as_f64())
                    {
                        s.push_str(&format!(" ({min}..{max})"));
                    }
                    if let Some(doc) = f["doc"].as_str() {
                        s.push_str(&format!(": {}", first_sentence(doc)));
                    }
                    s
                })
                .collect();
            let mut notes = Vec::new();
            if t["game"] == true {
                notes.push("game");
            }
            if t["settings"] == true {
                notes.push("settings file");
            } else if t["saved"] == json!(false) {
                notes.push("not saved");
            }
            text.push_str(&format!(
                "{kind} {}{}\n",
                t["name"].as_str().unwrap_or("?"),
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", notes.join(", "))
                }
            ));
            if let Some(doc) = t["doc"].as_str() {
                text.push_str(&format!("    {}\n", first_sentence(doc)));
            }
            for f in fields {
                text.push_str(&format!("    - {f}\n"));
            }
        }
    }
    text.push_str("Field types are as written in scene JSON; `box registry --json` has the full docs, defaults and steps.");
    text.push_str(&problems::summary(&problems));
    Ok(Output::ok(json, text))
}

fn scene_list(project: Option<&Path>) -> Result<Output, CliError> {
    let project = open_project(project)?;
    let mut scenes = Vec::new();
    let mut text = String::new();
    for path in project.scene_files()? {
        let rel = project.relative(&path);
        let entry = match load_scene(&path) {
            Ok(world) => {
                let n = hierarchy_order(&world).len();
                text.push_str(&format!("{rel} ({n} entities)\n"));
                json!({ "path": rel, "entities": n })
            }
            Err(e) => {
                text.push_str(&format!("{rel} (error: {})\n", e.message));
                json!({ "path": rel, "error": e.to_json()["error"] })
            }
        };
        scenes.push(entry);
    }
    let json = json!({
        "project": project.root().display().to_string(),
        "start_scene": project.file().start_scene,
        "scenes": scenes,
    });
    Ok(Output::ok(json, text.trim_end().to_string()))
}

/// How long `box check` waits for the project's assets to import.
const CHECK_IMPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// One problem per unknown component or resource type of a loaded scene
/// ([`unknown_types`]): `unknown_component` names the entities carrying it
/// (`entity`/`entity_name`: the first; `entities`: all uids), and both carry
/// the registry's did-you-mean as `suggestion`. A type of an engine module
/// the native project's game leaves out (`.box/registry.json` `modules`) is
/// `module_disabled` instead, with the `module` (cargo feature) and a
/// `hint`.
fn unknown_type_problems(world: &World, order: &[Entity], rel: &str) -> Vec<Json> {
    unknown_types(world)
        .into_iter()
        .map(|t| {
            let module = box_engine::modules::module_of(&t.name);
            let mut p = json!({
                "severity": "warning",
                "code": match (module, t.resource) {
                    (Some(_), _) => "module_disabled",
                    (None, true) => "unknown_resource",
                    (None, false) => "unknown_component",
                },
                "file": rel,
                "message": t.message(rel),
            });
            if let Some(m) = module {
                p["message"] = json!(format!(
                    "{rel}: `{}` belongs to box_engine's `{}` feature, which this project's game leaves out (data kept, not run)",
                    t.name, m.feature
                ));
                p["module"] = json!(m.feature);
                p["hint"] = json!(format!(
                    "add \"{}\" to the game's box_engine features in Cargo.toml, then refresh .box/registry.json",
                    m.feature
                ));
            }
            if t.resource {
                p["resource"] = json!(t.name);
            } else {
                let carriers: Vec<Entity> = order
                    .iter()
                    .copied()
                    .filter(|&e| {
                        world
                            .get::<UnknownComponents>(e)
                            .is_some_and(|u| u.0.contains_key(&t.name))
                    })
                    .collect();
                if let Some(&first) = carriers.first() {
                    p["entity"] = json!(world.uid(first).map(|u| u.to_string()));
                    p["entity_name"] = json!(
                        world
                            .get::<box_engine::box_core::prelude::Name>(first)
                            .map(|n| n.to_string())
                    );
                }
                p["entities"] = json!(
                    carriers
                        .iter()
                        .filter_map(|&e| world.uid(e).map(|u| u.to_string()))
                        .collect::<Vec<_>>()
                );
                p["component"] = json!(t.name);
            }
            if let Some(s) = t.did_you_mean {
                p["suggestion"] = json!(s);
            }
            p
        })
        .collect()
}

/// Errors for raw values of schema-only (game) types that do not fit their
/// schema, in a loaded scene.
fn schema_problems(world: &World, order: &[Entity], rel: &str) -> Vec<Json> {
    let registry = world.registry();
    let mut out = Vec::new();
    for &e in order {
        let Some(raw) = world.get::<UnknownComponents>(e) else {
            continue;
        };
        for (name, value) in &raw.0 {
            let Some(schema) = registry.schema(name) else {
                continue;
            };
            if let Err(message) = schema.validate(registry, value) {
                out.push(json!({
                    "severity": "error",
                    "code": "invalid_component",
                    "file": rel,
                    "entity": world.uid(e).map(|u| u.to_string()),
                    "entity_name": world.get::<box_engine::box_core::prelude::Name>(e).map(|n| n.to_string()),
                    "component": name,
                    "message": message,
                }));
            }
        }
    }
    if let Some(raw) = world.get_resource::<box_engine::box_core::scene::UnknownResources>() {
        for (name, value) in &raw.0 {
            if let Some(schema) = registry.schema(name)
                && let Err(message) = schema.validate(registry, value)
            {
                out.push(json!({
                    "severity": "error",
                    "code": "invalid_resource",
                    "file": rel,
                    "resource": name,
                    "message": message,
                }));
            }
        }
    }
    out
}

/// Problems with the project's settings files (`assets/settings/<Type>.json`)
/// for the settings types the registry knows: not an object or fields that
/// do not fit (game types are checked against their schema).
fn settings_problems(project: &Project, registry: &Registry) -> Vec<Json> {
    use box_engine::settings::settings_file;
    let mut out = Vec::new();
    let mut check = |name: &str, validate: &dyn Fn(&Json) -> Result<(), String>| {
        let rel = settings_file(name);
        let path = project.assets_dir().join(&rel);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let file = format!("assets/{rel}");
        // Syntax errors are the json importer's (asset_import_failed).
        let Ok(json) = serde_json::from_slice::<Json>(&bytes) else {
            return;
        };
        if let Err(message) = validate(&json) {
            out.push(json!({
                "severity": "error",
                "code": "invalid_settings",
                "file": file,
                "resource": name,
                "message": message,
            }));
        }
    };
    for ty in registry.settings() {
        let type_id = ty.type_id();
        check(ty.name(), &|json: &Json| {
            let mut merged = ty
                .default_value()
                .and_then(|d| registry.value_to_json(d.as_ref()).ok())
                .unwrap_or(Json::Null);
            if let (Some(m), Some(f)) = (merged.as_object_mut(), json.as_object()) {
                for (k, v) in f {
                    if m.contains_key(k) {
                        m.insert(k.clone(), v.clone());
                    }
                }
            }
            registry
                .value_from_json(type_id, &merged)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
    }
    for schema in registry.schemas().filter(|s| s.is_settings()) {
        check(schema.name(), &|json: &Json| {
            schema.validate(registry, json).map(|_| ())
        });
    }
    out
}

/// `module_disabled` warnings for settings files of engine modules the
/// native project's game leaves out (its `.box/registry.json` `modules`):
/// the game ignores them.
fn left_out_settings_problems(
    project: &Project,
    loaded: &game_registry::ProjectRegistry,
) -> Vec<Json> {
    use box_engine::modules::{MODULES, left_out_settings_files, left_out_settings_message};
    let Some(features) = loaded.game.as_ref().and_then(|g| g.modules.as_ref()) else {
        return Vec::new();
    };
    let mut modules = MODULES;
    for m in &mut modules {
        m.enabled = features.iter().any(|f| f == m.feature);
    }
    left_out_settings_files(&project.assets_dir(), &modules)
        .into_iter()
        .map(|(rel, m)| {
            json!({
                "severity": "warning",
                "code": "module_disabled",
                "file": format!("assets/{rel}"),
                "module": m.feature,
                "message": left_out_settings_message(&rel, m),
                "hint": format!(
                    "add \"{}\" to the game's box_engine features in Cargo.toml, or delete the file",
                    m.feature
                ),
            })
        })
        .collect()
}

/// `upgrade_needed` (a warning: files are migrated in memory on every load
/// until `box upgrade` writes them), `newer_format` (an error: a file from a
/// newer engine or game) and `upgrade_failed` problems.
fn upgrade_problems(project: &Project, registry: &std::sync::Arc<Registry>) -> Vec<Json> {
    let plan = match box_engine::upgrade::plan(project.root(), registry) {
        Ok(plan) => plan,
        Err(e) => {
            return vec![json!({
                "severity": "error",
                "code": "upgrade_failed",
                "message": e.to_string(),
            })];
        }
    };
    let mut out = Vec::new();
    for f in &plan.files {
        out.push(json!({
            "severity": "warning",
            "code": "upgrade_needed",
            "file": f.path,
            "message": format!("older format, migrated in memory on load: {}", f.changes.join("; ")),
            "hint": "box upgrade (backs the files up first; --dry-run shows the changes)",
        }));
    }
    for p in &plan.newer {
        out.push(json!({
            "severity": "error",
            "code": "newer_format",
            "file": p.path,
            "message": p.message,
        }));
    }
    for p in &plan.errors {
        out.push(json!({
            "severity": "error",
            "code": "upgrade_failed",
            "file": p.path,
            "message": p.message,
        }));
    }
    out
}

fn check(project: Option<&Path>) -> Result<Output, CliError> {
    let project = open_project(project)?;
    let collector = problems::LogCollector::start();
    let loaded = game_registry::for_project(&project);
    let mut problems = loaded.problems.clone();
    let mut scenes = Vec::new();
    let mut worlds = Vec::new();
    let start = project.start_scene_path();
    if !start.is_file() {
        problems.push(json!({
            "severity": "error",
            "code": "start_scene_missing",
            "file": project.relative(&project.project_file_path()),
            "message": format!(
                "start_scene `{}` does not exist; create that scene or point `start_scene` at an existing one",
                project.file().start_scene
            ),
        }));
    }
    for path in project.scene_files()? {
        let rel = display_path(Some(&project), &path);
        match load_scene_with(&path, loaded.registry.clone()) {
            Ok(world) => {
                let order = hierarchy_order(&world);
                problems.extend(unknown_type_problems(&world, &order, &rel));
                problems.extend(schema_problems(&world, &order, &rel));
                scenes.push(json!({ "file": rel, "ok": true, "entities": order.len() }));
                worlds.push((rel, world));
            }
            Err(e) => {
                let mut p = json!({
                    "severity": "error",
                    "code": e.code,
                    "file": rel,
                    "message": e.message,
                });
                if let Some(entity) = &e.entity {
                    p["entity"] = json!(entity);
                }
                if let Some(c) = e.extra.get("component") {
                    p["component"] = c.clone();
                }
                problems.push(p);
                scenes.push(json!({ "file": rel, "ok": false }));
            }
        }
    }
    problems.extend(settings_problems(&project, &loaded.registry));
    problems.extend(left_out_settings_problems(&project, &loaded));
    problems.extend(upgrade_problems(&project, &loaded.registry));
    let mut assets_checked = 0;
    let mut asset_problems = Vec::new();
    if project.assets_dir().is_dir() {
        let server = box_engine::asset_server(project.assets_dir())
            .map_err(|e| CliError::new("assets", e.to_string()).file(project.assets_dir()))?;
        let file_of = |asset: &str| format!("assets/{asset}");
        for (path, message) in server.broken_files() {
            asset_problems.push(json!({
                "severity": "error",
                "code": "asset_import_failed",
                "file": file_of(&path),
                "asset": path,
                "message": format!("invalid .meta file: {message}"),
            }));
        }
        let files = server.files();
        for (id, _) in &files {
            let _ = server.load_file(&AssetRef::id(*id));
        }
        let mut world = World::new();
        if !server.update_until_idle(&mut world, CHECK_IMPORT_TIMEOUT) {
            asset_problems.push(json!({
                "severity": "error",
                "code": "timeout",
                "message": format!("assets did not finish importing in {} s", CHECK_IMPORT_TIMEOUT.as_secs()),
            }));
        }
        assets_checked = files.len();
        let failed_scenes: Vec<String> = scenes
            .iter()
            .filter(|s| s["ok"] == false)
            .filter_map(|s| s["file"].as_str().map(str::to_string))
            .collect();
        for (id, path) in &files {
            if let Some(LoadState::Failed(message)) = server.file_state(*id) {
                asset_problems.push(json!({
                    "severity": "error",
                    "code": problems::failure_code(&message),
                    "file": file_of(path),
                    "asset": path,
                    "message": message,
                    // A scene that failed to load is already reported.
                    "repeat": failed_scenes.contains(&file_of(path)),
                }));
            }
        }
        // Material shaders compile only on the GPU at runtime; validate
        // them here (naga, no GPU) so errors show up before Play.
        for (id, path) in &files {
            if !path.ends_with(".wgsl") || server.file_state(*id) != Some(LoadState::Loaded) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(project.assets_dir().join(path)) else {
                continue;
            };
            if !text.contains("fn surface") {
                continue;
            }
            if let Err(message) = box_engine::box_render::check_material_shader(path, &text) {
                asset_problems.push(json!({
                    "severity": "error",
                    "code": "shader_invalid",
                    "file": file_of(path),
                    "asset": path,
                    "message": message,
                }));
            }
        }
        for (rel, world) in &worlds {
            for mut p in problems::asset_ref_problems(world, &server, false, "error") {
                p["file"] = json!(rel);
                asset_problems.push(p);
            }
        }
    }
    // Merged before dropping the repeats, so their log records go too.
    problems.extend(
        problems::merge(asset_problems, collector.finish())
            .into_iter()
            .filter(|p| p["repeat"] != true)
            .map(|mut p| {
                if let Some(o) = p.as_object_mut() {
                    o.remove("repeat");
                }
                p
            }),
    );
    let (errors, warnings) = problems::counts(&problems);
    let mut text = String::new();
    for p in &problems {
        let entity = match (p["entity_name"].as_str(), p["entity"].as_str()) {
            (Some(name), Some(uid)) => format!(", entity `{name}` ({uid})"),
            (None, Some(uid)) => format!(", entity {uid}"),
            _ => String::new(),
        };
        text.push_str(&format!(
            "{}[{}]: {}{entity}: {}\n",
            p["severity"].as_str().unwrap_or("?"),
            p["code"].as_str().unwrap_or("?"),
            p["file"].as_str().unwrap_or("-"),
            p["message"].as_str().unwrap_or("?"),
        ));
    }
    text.push_str(&format!(
        "{}: {} scene(s), {assets_checked} asset file(s), {errors} error(s), {warnings} warning(s)",
        if errors == 0 { "ok" } else { "failed" },
        scenes.len()
    ));
    let json = json!({
        "project": project.root().display().to_string(),
        "ok": errors == 0,
        "errors": errors,
        "warnings": warnings,
        "scenes": scenes,
        "assets": assets_checked,
        "game_registry": loaded.summary(&project),
        "problems": problems,
    });
    Ok(Output {
        json,
        text,
        success: errors == 0,
    })
}
