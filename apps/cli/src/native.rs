//! `box new --native`: the native project tier, a Cargo crate next to the
//! project's assets with the game plugin and its own editor and player.

use std::path::{Path, PathBuf};

use crate::CliError;

/// The engine workspace manifest this CLI was built from, for dependency
/// versions that a game crate must share with the engine.
const ENGINE_MANIFEST: &str = include_str!("../../../Cargo.toml");

/// The engine repository this CLI was built from.
fn default_engine_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `version = "..."` of a `[workspace.dependencies]` entry.
fn engine_dependency_version(name: &str) -> Option<&'static str> {
    let prefix = format!("{name} = ");
    let line = ENGINE_MANIFEST
        .lines()
        .find(|l| l.starts_with(&prefix))?
        .strip_prefix(&prefix)?;
    if let Some(v) = line.strip_prefix('"') {
        return v.split('"').next();
    }
    let rest = &line[line.find("version = \"")? + "version = \"".len()..];
    rest.split('"').next()
}

/// Check the engine checkout and return its absolute path. Called before
/// anything is written.
pub(crate) fn engine_path(given: Option<&Path>) -> Result<PathBuf, CliError> {
    let path = given.map_or_else(default_engine_path, Path::to_path_buf);
    let marker = path.join("crates/box_engine/Cargo.toml");
    if !marker.is_file() {
        return Err(CliError::new(
            "not_found",
            format!(
                "{} is not a box-engine checkout (no crates/box_engine/Cargo.toml); pass --engine-path <path to box-engine>",
                path.display()
            ),
        )
        .file(&path));
    }
    let canonical = std::fs::canonicalize(&path).map_err(|e| CliError::io(&path, e))?;
    Ok(strip_verbatim(canonical))
}

/// Windows `canonicalize` returns verbatim paths (`\\?\C:\...`), which
/// Cargo rejects in a `path` dependency; drop the prefix for drive paths.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// Files the native tier adds, relative to the project root.
pub(crate) const FILES: [&str; 4] = [
    "Cargo.toml",
    "src/lib.rs",
    "src/bin/editor.rs",
    "src/bin/player.rs",
];

/// `.cargo/config.toml` of a native project (written unless one exists).
const CARGO_ALIASES: &str = r#"# Shortcuts: `cargo player [-- <flags>]` and `cargo editor [-- <flags>]`.
[alias]
player = "run --bin player"
editor = "run --features editor --bin editor"
"#;

/// Refuse a directory that already has a crate, before anything is written.
pub(crate) fn check_free(dir: &Path) -> Result<(), CliError> {
    for f in FILES {
        let p = dir.join(f);
        if p.exists() {
            return Err(
                CliError::new("already_exists", format!("{} already exists", p.display())).file(&p),
            );
        }
    }
    Ok(())
}

/// A Cargo package name from a project name: lowercase ASCII letters,
/// digits and `-`, not starting with a digit, not a Rust keyword or a
/// dependency's name.
pub(crate) fn crate_name(project: &str) -> String {
    let mut name = String::new();
    for c in project.chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    let mut name = name.trim_matches('-').to_string();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name = format!("game-{name}").trim_end_matches('-').to_string();
    }
    const RESERVED: [&str; 18] = [
        "box-engine",
        "box-editor",
        "box-core",
        "box-render",
        "anyhow",
        "bevy-reflect",
        "test",
        "core",
        "std",
        "alloc",
        "self",
        "super",
        "crate",
        "type",
        "fn",
        "mod",
        "use",
        "impl",
    ];
    if RESERVED.contains(&name.as_str()) {
        name.push_str("-game");
    }
    name
}

/// The game plugin's type name from a project name: `"Space Race"` gives
/// `SpaceRacePlugin` (never `Game`, which would shadow `box_engine::Game`).
pub(crate) fn plugin_name(project: &str) -> String {
    let mut name = String::new();
    for word in project.split(|c: char| !c.is_ascii_alphanumeric()) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            name.push(first.to_ascii_uppercase());
            name.extend(chars);
        }
    }
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert_str(0, "Game");
    }
    name.push_str("Plugin");
    name
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Write the crate into the (already created) project at `root`. Returns
/// the files written, relative to the root.
pub(crate) fn write(
    root: &Path,
    project_name: &str,
    engine: &Path,
) -> Result<Vec<String>, CliError> {
    let package = crate_name(project_name);
    let lib = package.replace('-', "_");
    let plugin = plugin_name(project_name);
    let engine_dir = engine.to_string_lossy().replace('\\', "/");
    let dep = |name: &str| toml_string(&format!("{engine_dir}/crates/{name}"));
    let anyhow = engine_dependency_version("anyhow").unwrap_or("1.0");

    let cargo_toml = format!(
        r#"# {project_name}: a box-engine native project. The game plugin is in
# src/lib.rs; `cargo player` plays the start scene and `cargo editor` opens
# the project in the editor with the game's types (aliases in
# .cargo/config.toml for `cargo run --bin player` and
# `cargo run --features editor --bin editor`).
[package]
name = "{package}"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
path = "src/lib.rs"

[features]
# The engine modules the game builds with (box_engine's features). To compile
# less, keep only the ones the game uses, for example
# `default = ["physics", "audio"]` without rigid bodies (`dynamics`, rapier)
# and Luau scripts (`script`). A scene that uses a left-out module's types
# still loads, with a warning naming the feature. The editor binary always
# has every module.
default = ["physics", "dynamics", "audio", "script"]
physics = ["box_engine/physics"]
dynamics = ["box_engine/dynamics"]
audio = ["box_engine/audio"]
script = ["box_engine/script"]
# The editor binary. Off by default so the player builds without the editor
# (egui and its dependencies).
editor = ["dep:box_editor"]

[[bin]]
name = "editor"
path = "src/bin/editor.rs"
required-features = ["editor"]

[[bin]]
name = "player"
path = "src/bin/player.rs"

# The engine's prelude re-exports what a game needs (`bevy_reflect` for
# `#[derive(Reflect)]`, the `tracing` log macros), so these are all.
[dependencies]
box_engine = {{ path = {engine_dep}, default-features = false }}
box_editor = {{ path = {editor_dep}, optional = true }}
anyhow = "{anyhow}"

# A workspace of its own, so a workspace in a parent folder does not claim it.
[workspace]

# The engine's build settings: optimized dependencies in debug builds.
[profile.dev]
opt-level = 1
debug = "line-tables-only"

[profile.dev.package."*"]
opt-level = 3
debug-assertions = false
"#,
        engine_dep = dep("box_engine"),
        editor_dep = dep("box_editor"),
    );

    let lib_rs = format!(
        r#"//! {project_name}: the game plugin. `register` adds the saved types the
//! editor shows and scenes store; `build` adds systems. The player
//! (`cargo player`) and the editor (`cargo editor`) both run [`game`].

use box_engine::prelude::*;

/// Spins its entity around the Y axis. Add it to an entity in the editor.
#[derive(Reflect, Clone)]
#[reflect(Component, Default, Clone)]
pub struct Spin {{
    /// Turns per second.
    #[reflect(@Range {{ min: -4.0, max: 4.0 }}, @Unit("turn/s"))]
    pub speed: f32,
}}

impl Default for Spin {{
    fn default() -> Self {{
        Self {{ speed: 0.25 }}
    }}
}}

fn spin(world: &mut World) {{
    let dt = world.resource::<Time>().delta;
    for (transform, spin) in world.query_mut::<(&mut Transform, &Spin)>() {{
        transform.rotation *= Quat::from_rotation_y(spin.speed * std::f32::consts::TAU * dt);
    }}
}}

/// The game's types and systems.
pub struct {plugin};

impl Plugin for {plugin} {{
    fn register(&self, registry: &mut Registry) {{
        registry.register_all::<(Spin,)>();
    }}

    fn build(&self, app: &mut App) {{
        app.add_system(Stage::Update, spin);
        // Once per entity that gets a `Spin`: spawned, added or loaded by a
        // scene. The place to add runtime state. (`RUST_LOG=info` shows it.)
        app.on_add::<Spin>(|world, entity| {{
            let name = world.get::<Name>(entity).map(|n| n.as_str().to_owned());
            info!("{{}} spins", name.unwrap_or_default());
        }});
        // Once per event, with `&mut World`; send with `world.send(..)`.
        app.on_event(Stage::Update, |_world: &mut World, event: &SceneEvent| {{
            info!("scene: {{event:?}}");
        }});
    }}
}}

/// The game as the player, the editor and tests run it
/// (`game().new_app()` for a headless test app).
pub fn game() -> Game {{
    Game::new().with_plugin({plugin})
}}
"#
    );

    let editor_rs = format!(
        r#"//! The {project_name} editor: box-editor with this game's types and systems.
//! `cargo editor [-- <flags>]` (`cargo run --features editor --bin editor`).
//! Without a project folder it opens this project.

fn main() -> anyhow::Result<()> {{
    let mut options = box_editor::EditorOptions::from_args(std::env::args().skip(1))?;
    if options.project.is_none() {{
        options.project = Some(env!("CARGO_MANIFEST_DIR").into());
    }}
    box_editor::run_game(options, {lib}::game())
}}
"#
    );

    let player_rs = format!(
        r#"//! The {project_name} player.
//! `cargo player [-- [project_dir] [--scene <name|path>] [--capture <png>] [--frames N] [--size WxH] [--report <json>] [--headless] [--control]]`
//! Without a project folder it plays this project.

use std::process::ExitCode;

fn main() -> ExitCode {{
    match play() {{
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {{
            // One line, with repeated causes dropped.
            eprintln!("error: {{}}", box_engine::player::error_text(&e));
            ExitCode::FAILURE
        }}
    }}
}}

fn play() -> anyhow::Result<()> {{
    let mut options = box_engine::player::PlayerOptions::from_args(std::env::args().skip(1))?;
    // In an exported bundle (`box export`) the game.pak next to the binary is played.
    options.default_project(env!("CARGO_MANIFEST_DIR"));
    box_engine::player::run_game(options, {lib}::game())?;
    Ok(())
}}
"#
    );

    let mut written = Vec::new();
    for (rel, text) in [
        (FILES[0], cargo_toml),
        (FILES[1], lib_rs),
        (FILES[2], editor_rs),
        (FILES[3], player_rs),
    ] {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CliError::io(dir, e))?;
        }
        std::fs::write(&path, text).map_err(|e| CliError::io(&path, e))?;
        written.push(rel.to_string());
    }

    // Build with the engine's toolchain and dependency versions.
    for name in ["rust-toolchain.toml", "Cargo.lock"] {
        let from = engine.join(name);
        let to = root.join(name);
        if from.is_file() && !to.exists() {
            std::fs::copy(&from, &to).map_err(|e| CliError::io(&to, e))?;
            written.push(name.to_string());
        }
    }

    // `cargo player` / `cargo editor`, so nobody has to remember the feature.
    let aliases = root.join(".cargo/config.toml");
    if !aliases.exists() {
        if let Some(dir) = aliases.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CliError::io(dir, e))?;
        }
        std::fs::write(&aliases, CARGO_ALIASES).map_err(|e| CliError::io(&aliases, e))?;
        written.push(".cargo/config.toml".to_string());
    }

    let gitignore = root.join(".gitignore");
    let mut text = std::fs::read_to_string(&gitignore).unwrap_or_default();
    if !text
        .lines()
        .any(|l| l.trim() == "target/" || l.trim() == "/target")
    {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("target/\n");
        std::fs::write(&gitignore, text).map_err(|e| CliError::io(&gitignore, e))?;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_names_are_valid() {
        assert_eq!(crate_name("My Game"), "my-game");
        assert_eq!(crate_name("  Space -- Race 2! "), "space-race-2");
        assert_eq!(crate_name("3D"), "game-3d");
        assert_eq!(crate_name("ü"), "game");
        assert_eq!(crate_name("Test"), "test-game");
        assert_eq!(crate_name("box_engine"), "box-engine-game");
    }

    #[test]
    fn reads_engine_dependency_versions() {
        assert_eq!(engine_dependency_version("anyhow"), Some("1.0"));
        let bevy = engine_dependency_version("bevy_reflect").unwrap();
        assert!(bevy.starts_with('='), "{bevy}");
        assert!(engine_path(None).is_ok());
    }

    #[test]
    fn plugin_names_are_camel_case_and_never_game() {
        assert_eq!(plugin_name("Space Race"), "SpaceRacePlugin");
        assert_eq!(plugin_name("my-game"), "MyGamePlugin");
        assert_eq!(plugin_name("3D"), "Game3DPlugin");
        assert_eq!(plugin_name("ü"), "GamePlugin");
        assert_eq!(plugin_name("game"), "GamePlugin");
    }

    /// The generated crate builds with nothing but the engine crates and
    /// `anyhow`: the prelude supplies `bevy_reflect` for the derive. Checks
    /// both binaries (the editor with its feature) in the engine's target
    /// directory, offline; skipped when the dependencies are not cached or
    /// `BOX_SKIP_NATIVE_CHECK` is set.
    #[test]
    fn generated_project_builds_with_only_engine_dependencies() {
        if std::env::var_os("BOX_SKIP_NATIVE_CHECK").is_some() {
            eprintln!("BOX_SKIP_NATIVE_CHECK set; skipping");
            return;
        }
        let engine = engine_path(None).unwrap();
        let root = std::env::temp_dir().join(format!("box_native_check_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let files = write(&root, "Check Game", &engine).unwrap();
        assert!(
            files.contains(&".cargo/config.toml".to_string()),
            "{files:?}"
        );
        let cargo_toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(!cargo_toml.contains("bevy_reflect ="), "{cargo_toml}");
        let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
        assert!(lib.contains("impl Plugin for CheckGamePlugin"), "{lib}");
        assert!(!lib.contains("type_path"), "{lib}");
        assert!(
            lib.contains("app.on_add::<Spin>") && lib.contains("app.on_event"),
            "{lib}"
        );
        let player = std::fs::read_to_string(root.join("src/bin/player.rs")).unwrap();
        assert!(player.contains("error_text(&e)"), "{player}");

        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| engine.join("target"));
        let out = std::process::Command::new(env!("CARGO"))
            .current_dir(&root)
            .args([
                "check",
                "--offline",
                "--quiet",
                "--bins",
                "--features",
                "editor",
            ])
            .env("CARGO_TARGET_DIR", &target)
            .output()
            .expect("run cargo");
        let stderr = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_dir_all(&root);
        if !out.status.success()
            && [
                "offline",
                "failed to download",
                "no matching package",
                "network",
            ]
            .iter()
            .any(|s| stderr.contains(s))
        {
            eprintln!("dependencies unavailable offline; skipping:\n{stderr}");
            return;
        }
        assert!(
            out.status.success(),
            "the generated project does not build:\n{stderr}"
        );
    }
}
