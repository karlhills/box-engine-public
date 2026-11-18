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
//! `VERSION` file; a version other than this engine's is refused. Release
//! Windows templates are built on Windows (CI).
//!
//! A native project's player is built with cargo for the host, or, for
//! Windows from macOS or Linux, cross-compiled with `cargo xwin`
//! ([`cross_tools`]).

use std::ffi::OsString;
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

/// The target triple of Windows cross builds.
pub const WINDOWS_TRIPLE: &str = "x86_64-pc-windows-msvc";

/// The tools a Windows cross build runs: `cargo-xwin`, and LLVM's
/// `clang-cl`, `lld-link` and `llvm-lib`.
const CROSS_TOOLS: [(&str, &str); 4] = [
    ("cargo-xwin", "cargo install --locked cargo-xwin"),
    ("clang-cl", "brew install llvm"),
    ("llvm-lib", "brew install llvm"),
    ("lld-link", "brew install lld"),
];

/// Folders searched for the cross tools after `PATH`: Homebrew's keg-only
/// LLVM and lld (Apple Silicon and Intel prefixes), and cargo's bin folder.
fn cross_tool_dirs(home: Option<&Path>, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "/opt/homebrew/opt/llvm/bin",
        "/opt/homebrew/opt/lld/bin",
        "/usr/local/opt/llvm/bin",
        "/usr/local/opt/lld/bin",
    ]
    .iter()
    .map(PathBuf::from)
    .collect();
    match (cargo_home, home) {
        (Some(c), _) => dirs.push(c.join("bin")),
        (None, Some(h)) => dirs.push(h.join(".cargo/bin")),
        (None, None) => {}
    }
    dirs
}

/// What a Windows cross build runs with: the search path that finds the
/// cross tools.
#[derive(Debug, Clone)]
pub struct CrossTools {
    /// `PATH` with the tool folders appended.
    pub path: OsString,
}

/// Checks the tools a Windows cross build needs (`cargo-xwin`, LLVM, lld
/// and the Rust target `x86_64-pc-windows-msvc` for the toolchain `dir`
/// selects) and says how to install what is missing.
pub fn cross_tools(dir: &Path) -> Result<CrossTools, ExportError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(cross_tool_dirs(home.as_deref(), cargo_home.as_deref()));
    let target_installed = rustup_has_target(dir);
    let missing = missing_cross_tools(&dirs, target_installed);
    if !missing.is_empty() {
        return Err(ExportError::Cargo(cross_tools_message(&missing)));
    }
    let path = std::env::join_paths(&dirs)
        .map_err(|e| ExportError::Cargo(format!("PATH for the cross build: {e}")))?;
    Ok(CrossTools { path })
}

/// Whether rustup has the Windows target for the toolchain `dir` selects
/// (its `rust-toolchain.toml`). `None` without rustup: cargo reports it.
fn rustup_has_target(dir: &Path) -> Option<bool> {
    let out = Command::new("rustup")
        .current_dir(dir)
        .args(["target", "list", "--installed"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == WINDOWS_TRIPLE),
    )
}

/// The missing tools as `(name, install command)`, given the folders
/// searched and whether the Rust target is installed.
fn missing_cross_tools(
    dirs: &[PathBuf],
    target_installed: Option<bool>,
) -> Vec<(&'static str, String)> {
    let mut missing: Vec<(&'static str, String)> = CROSS_TOOLS
        .iter()
        .filter(|(tool, _)| !dirs.iter().any(|d| d.join(tool).is_file()))
        .map(|(tool, fix)| (*tool, fix.to_string()))
        .collect();
    if target_installed == Some(false) {
        missing.push((
            "the Rust target x86_64-pc-windows-msvc",
            format!("rustup target add {WINDOWS_TRIPLE}"),
        ));
    }
    missing
}

fn cross_tools_message(missing: &[(&'static str, String)]) -> String {
    let names: Vec<&str> = missing.iter().map(|m| m.0).collect();
    let mut fixes: Vec<&str> = Vec::new();
    for (_, fix) in missing {
        if !fixes.contains(&fix.as_str()) {
            fixes.push(fix);
        }
    }
    format!(
        "building a native project for Windows on this computer needs {} (docs/EXPORT.md section 6). Install with: {}",
        names.join(", "),
        fixes
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

/// Builds a native project's player binary in release mode (`cargo build
/// --release --bin <player>` in the project folder) and returns its path.
/// For the host platform, or Windows from macOS or Linux with `cargo xwin`
/// (static CRT, so testers need no Visual C++ Redistributable).
pub fn build_native_player(
    project: &Project,
    native: &NativeCrate,
    target: Target,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ExportError> {
    let cross = target != Target::host();
    if cross && target != Target::Windows {
        return Err(ExportError::Cargo(format!(
            "`{}` is a native project: its player must be built on {}; export it there, or pass the built player with --player",
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
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(native.dir());
    if cross {
        let tools = cross_tools(native.dir())?;
        progress(Progress::new(
            "build",
            format!(
                "Building {bin} for Windows (cargo xwin; the first build downloads Microsoft's CRT and SDK)"
            ),
            0.02,
        ));
        // Static CRT: the exe needs no VCRUNTIME140.dll. Appended to any
        // flags the caller set for this target.
        let flags_var = "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS";
        let mut flags = std::env::var(flags_var).unwrap_or_default();
        flags.push_str(" -C target-feature=+crt-static");
        // ATL: wgpu's statically linked DXC (DX12 shader compiler) links
        // atls.lib.
        command
            .env("PATH", &tools.path)
            .env("XWIN_INCLUDE_ATL", "true")
            .env(flags_var, flags.trim())
            .args(["xwin", "build", "--target", WINDOWS_TRIPLE]);
    } else {
        progress(Progress::new(
            "build",
            format!("Building {bin} (cargo build --release)"),
            0.02,
        ));
        command.arg("build");
    }
    command.args(["--release", "--message-format=json-render-diagnostics"]);
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
            "cargo {} --release --bin {bin} failed:\n{}",
            if cross { "xwin build" } else { "build" },
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
    fn missing_cross_tools_say_how_to_install_them() {
        let root = std::env::temp_dir().join(format!("box_cross_tools_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Nothing found, target missing: every tool, install commands once.
        let missing = missing_cross_tools(std::slice::from_ref(&root), Some(false));
        let names: Vec<&str> = missing.iter().map(|m| m.0).collect();
        assert_eq!(
            names,
            [
                "cargo-xwin",
                "clang-cl",
                "llvm-lib",
                "lld-link",
                "the Rust target x86_64-pc-windows-msvc"
            ]
        );
        let text = cross_tools_message(&missing);
        assert_eq!(text.matches("`brew install llvm`").count(), 1, "{text}");
        assert!(text.contains("`brew install lld`"), "{text}");
        assert!(
            text.contains("`cargo install --locked cargo-xwin`"),
            "{text}"
        );
        assert!(
            text.contains("`rustup target add x86_64-pc-windows-msvc`"),
            "{text}"
        );
        // Everything present; rustup unknown is not reported.
        for (tool, _) in CROSS_TOOLS {
            std::fs::write(root.join(tool), b"").unwrap();
        }
        assert!(missing_cross_tools(std::slice::from_ref(&root), None).is_empty());
        assert!(missing_cross_tools(std::slice::from_ref(&root), Some(true)).is_empty());
        // Homebrew's keg-only folders and cargo's bin are searched.
        let dirs = cross_tool_dirs(Some(Path::new("/h")), None);
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/opt/lld/bin")));
        assert!(dirs.contains(&PathBuf::from("/h/.cargo/bin")));
        let dirs = cross_tool_dirs(Some(Path::new("/h")), Some(Path::new("/c")));
        assert!(dirs.contains(&PathBuf::from("/c/bin")));
        let _ = std::fs::remove_dir_all(&root);
    }

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
