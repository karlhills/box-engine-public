//! Player executables to bundle: prebuilt templates for script projects,
//! the project's own player (built with cargo) for native projects.
//!
//! Templates are `box-player` binaries of this engine version, looked up in
//! order:
//!
//! 1. `$BOX_TEMPLATES/<target>/box-player[.exe]`
//! 2. `<dir of this program>/templates/<target>/box-player[.exe]` (the
//!    program's real location: symlinks to it are resolved)
//! 3. `<dir of this program>/../Resources/templates/<target>/...` (inside
//!    the editor's `.app`)
//! 4. for the host platform only: `box-player[.exe]` next to this program
//!    (`cargo build -p box_player_app`, `cargo install` of the engine's apps)
//!
//! `<target>` is `macos` or `windows`. A templates folder may hold a
//! `VERSION` file; a version other than this engine's is refused. Windows
//! templates are built on Windows (CI), not cross-compiled.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use box_engine::Project;
use box_engine::player::NativeCrate;
use box_engine::project::ENGINE_VERSION;

use crate::{ExportError, Progress, Target};

/// Environment variable naming the templates folder.
pub const TEMPLATES_ENV: &str = "BOX_TEMPLATES";

/// File name of the player template for `target`.
pub fn template_file(target: Target) -> &'static str {
    match target {
        Target::MacOs => "box-player",
        Target::Windows => "box-player.exe",
    }
}

/// The folders searched for templates, in order (see the module docs), for
/// a `BOX_TEMPLATES` value and the running program's folder.
pub fn template_dirs(env: Option<&Path>, exe_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = env.filter(|d| !d.as_os_str().is_empty()) {
        dirs.push(dir.to_path_buf());
    }
    if let Some(exe_dir) = exe_dir {
        dirs.push(exe_dir.join("templates"));
        dirs.push(exe_dir.join("../Resources/templates"));
    }
    dirs
}

/// The prebuilt player for `target` (see the module docs).
pub fn find_template(target: Target) -> Result<PathBuf, ExportError> {
    let env = std::env::var_os(TEMPLATES_ENV).map(PathBuf::from);
    // Resolved: `box` installed as a symlink (`~/.local/bin/box` into
    // Box Editor.app) reports the link's path on macOS.
    let exe_dir = std::env::current_exe()
        .ok()
        .map(|e| e.canonicalize().unwrap_or(e))
        .and_then(|e| e.parent().map(Path::to_path_buf));
    find_template_in(target, env.as_deref(), exe_dir.as_deref())
}

/// [`find_template`] for a given `BOX_TEMPLATES` value and program folder.
pub fn find_template_in(
    target: Target,
    env: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Result<PathBuf, ExportError> {
    let file = template_file(target);
    let mut tried = Vec::new();
    for dir in template_dirs(env, exe_dir) {
        let candidate = dir.join(target.name()).join(file);
        if candidate.is_file() {
            check_version(&dir)?;
            return Ok(candidate);
        }
        tried.push(candidate);
    }
    if target == Target::host()
        && let Some(sibling) = exe_dir.map(|d| d.join(file))
    {
        if sibling.is_file() {
            return Ok(sibling);
        }
        tried.push(sibling);
    }
    let hint = match target {
        Target::Windows => {
            "Windows player templates are built on Windows (CI) and ship with engine releases"
        }
        Target::MacOs => "build one with `cargo build --release -p box_player_app`",
    };
    Err(ExportError::Template(format!(
        "no {} player template found ({hint}; or set {TEMPLATES_ENV} to a folder holding {}/{file}). Looked for: {}",
        target.label(),
        target.name(),
        tried
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

fn check_version(dir: &Path) -> Result<(), ExportError> {
    let Ok(text) = std::fs::read_to_string(dir.join("VERSION")) else {
        return Ok(());
    };
    let version = text.trim();
    if version != ENGINE_VERSION {
        return Err(ExportError::Template(format!(
            "the player templates in {} are for box-engine {version}, this is {ENGINE_VERSION}",
            dir.display()
        )));
    }
    Ok(())
}

/// Builds a native project's player binary in release mode (`cargo build
/// --release --bin <player>` in the project folder) and returns its path.
/// Only for the host platform.
pub fn build_native_player(
    project: &Project,
    native: &NativeCrate,
    target: Target,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ExportError> {
    if target != Target::host() {
        return Err(ExportError::Cargo(format!(
            "`{}` is a native project: its player must be built on {} (cross-compiling is not supported); export it there, or pass the built player with --player",
            project.name(),
            target.label()
        )));
    }
    let bin = native.player_bin().ok_or_else(|| {
        ExportError::Cargo(format!(
            "{}: no player binary found (binaries: {})",
            native.manifest.display(),
            native.bins.join(", ")
        ))
    })?;
    progress(Progress::new(
        "build",
        format!("Building {bin} (cargo build --release)"),
        0.02,
    ));
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(native.dir()).args([
        "build",
        "--release",
        "--message-format=json-render-diagnostics",
    ]);
    if let Some((_, features)) = native.required_features.iter().find(|(b, _)| b == bin) {
        command.args(["--features", &features.join(",")]);
    }
    command.args(["--bin", bin]);
    let out = command
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| ExportError::Cargo(format!("could not run cargo: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(20).collect();
        return Err(ExportError::Cargo(format!(
            "cargo build --release --bin {bin} failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if msg["reason"] == "compiler-artifact"
            && msg["target"]["name"] == bin
            && let Some(exe) = msg["executable"].as_str()
        {
            return Ok(PathBuf::from(exe));
        }
    }
    Err(ExportError::Cargo(format!(
        "cargo built {bin} but did not report its executable"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_found_in_order() {
        let root = std::env::temp_dir().join(format!("box_templates_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // An editor .app: Contents/MacOS/box-editor, templates in Resources.
        let exe_dir = root.join("Box Editor.app/Contents/MacOS");
        let bundled = root.join("Box Editor.app/Contents/Resources/templates");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::create_dir_all(bundled.join("macos")).unwrap();
        std::fs::write(bundled.join("macos/box-player"), b"x").unwrap();
        std::fs::write(bundled.join("VERSION"), ENGINE_VERSION).unwrap();
        let found = find_template_in(Target::MacOs, None, Some(&exe_dir)).unwrap();
        assert!(
            found.ends_with("Resources/templates/macos/box-player"),
            "{}",
            found.display()
        );
        // BOX_TEMPLATES first.
        let env = root.join("env");
        std::fs::create_dir_all(env.join("windows")).unwrap();
        std::fs::write(env.join("windows/box-player.exe"), b"x").unwrap();
        let found = find_template_in(Target::Windows, Some(&env), Some(&exe_dir)).unwrap();
        assert_eq!(found, env.join("windows/box-player.exe"));
        // Another platform's template is never taken from next to the program.
        if Target::host() == Target::MacOs {
            std::fs::write(exe_dir.join("box-player.exe"), b"x").unwrap();
            let err = find_template_in(Target::Windows, None, Some(&exe_dir)).unwrap_err();
            assert!(err.to_string().contains("built on Windows"), "{err}");
        }
        // A version mismatch is refused.
        std::fs::write(bundled.join("VERSION"), "9.9.9").unwrap();
        let err = find_template_in(Target::MacOs, None, Some(&exe_dir)).unwrap_err();
        assert!(err.to_string().contains("box-engine 9.9.9"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
