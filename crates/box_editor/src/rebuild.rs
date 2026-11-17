//! Game ▸ Rebuild and Restart (⇧⌘B): rebuild a native project's editor
//! binary and restart into it, keeping the scene, camera and selection.
//!
//! Available when the editor runs from the project's own editor binary: the
//! open project has a `Cargo.toml` ([`NativeCrate`]) whose editor binary has
//! the running executable's name. The build runs on a background thread
//! (`cargo build --message-format=json [--release] [--features ...] --bin
//! <editor>` in the project folder; `BOX_REBUILD_COMMAND` replaces it, run
//! through the shell) and streams to the Console; compiler errors become
//! toasts with `file:line:column`. On failure the editor keeps running. On
//! success the editor writes the scene's unsaved changes to its recovery
//! file, saves its session (scene, camera, selection in `.box/editor.json`),
//! layout and preferences as on quit, and replaces itself with the new
//! binary (Unix `exec`; on Windows it starts the new process and exits),
//! passing `--after-rebuild`. The new editor restores the session and, with
//! that flag, the recovery file without asking
//! (`Editor::restore_recovery_after_rebuild`).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};

use box_engine::Project;
use box_engine::player::{NativeCrate, shell_line};
use serde_json::Value as Json;

use crate::editor::Editor;
use crate::state::EditorState;

/// Environment variable that replaces the build command (a shell line run
/// in the project folder), for tests and unusual setups.
pub const REBUILD_COMMAND_ENV: &str = "BOX_REBUILD_COMMAND";

/// Log target of build output in the Console.
pub const BUILD_TARGET: &str = "box_editor::build";

/// How to rebuild and restart this editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebuildTarget {
    /// Where the command runs (the project's crate folder).
    pub dir: PathBuf,
    /// The editor binary's name.
    pub bin: String,
    /// The build command (program and arguments).
    pub command: Vec<String>,
    /// Arguments for the restarted editor.
    pub args: Vec<String>,
}

impl RebuildTarget {
    /// The target for the open project when this process is its editor
    /// binary; otherwise why Rebuild and Restart is not available.
    pub fn detect(project: Option<&Project>) -> Result<Self, String> {
        let project =
            project.ok_or("Rebuild and Restart needs a native project (one with a Cargo.toml)")?;
        let krate = NativeCrate::find(project.root()).ok_or(
            "this project has no Rust crate (no Cargo.toml); add one with `box new --native .`",
        )?;
        let bin = krate
            .editor_bin()
            .ok_or("the project's crate has no editor binary")?
            .to_string();
        let exe = std::env::current_exe().map_err(|e| format!("cannot find this editor: {e}"))?;
        let stem = exe.file_stem().map(|s| s.to_string_lossy().into_owned());
        if stem.as_deref() != Some(bin.as_str()) {
            return Err(format!(
                "this editor is not the project's own editor binary (`{bin}`); start that one (`cargo editor` or `cargo run --features editor --bin {bin}`) to rebuild from here"
            ));
        }
        let release = exe.components().any(|c| c.as_os_str() == "release");
        Ok(Self::new(&krate, &bin, release, project.root()))
    }

    /// The target for `krate`'s editor binary `bin` (a `--release` build
    /// when `release`), restarting on `project_root`.
    pub fn new(krate: &NativeCrate, bin: &str, release: bool, project_root: &Path) -> Self {
        let mut command: Vec<String> = [
            "cargo",
            "build",
            "--message-format=json",
            "--color",
            "never",
        ]
        .map(String::from)
        .into();
        if release {
            command.push("--release".into());
        }
        if let Some((_, features)) = krate.required_features.iter().find(|(b, _)| b == bin) {
            command.extend(["--features".to_string(), features.join(",")]);
        }
        command.extend(["--bin".to_string(), bin.to_string()]);
        if let Some(line) = std::env::var_os(REBUILD_COMMAND_ENV).filter(|v| !v.is_empty()) {
            command = shell_command(&line.to_string_lossy());
        }
        Self {
            dir: krate.dir().to_path_buf(),
            bin: bin.to_string(),
            command,
            args: restart_args(std::env::args().skip(1), project_root),
        }
    }
}

/// `line` run through the platform's shell.
pub fn shell_command(line: &str) -> Vec<String> {
    if cfg!(windows) {
        vec!["cmd".into(), "/C".into(), line.into()]
    } else {
        vec!["sh".into(), "-c".into(), line.into()]
    }
}

/// The flag a restarted editor gets: restore the scene's recovery file
/// without asking, and the session even with `--no-restore`.
pub const AFTER_REBUILD_FLAG: &str = "--after-rebuild";

/// The restarted editor's arguments: this editor's, without one-shot flags
/// (`--capture`, `--frames`, `--play`, `--write-registry`, `--no-restore`),
/// with the project folder and [`AFTER_REBUILD_FLAG`].
pub fn restart_args(args: impl IntoIterator<Item = String>, project: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut args = args.into_iter();
    let mut project_given = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--capture" | "--frames" => {
                args.next();
            }
            "--play" | "--write-registry" | "--no-restore" | "--restore" | AFTER_REBUILD_FLAG => {}
            "--size" => {
                out.push(a);
                out.extend(args.next());
            }
            flag if flag.starts_with('-') => out.push(a),
            _ => {
                project_given = true;
                out.push(project.display().to_string());
            }
        }
    }
    if !project_given {
        out.insert(0, project.display().to_string());
    }
    out.push(AFTER_REBUILD_FLAG.into());
    out
}

/// What the build thread reports.
#[derive(Debug)]
enum BuildMsg {
    /// A line for the Console.
    Line(String),
    /// A compiler error or warning: `file:line:col: message` and the
    /// rendered diagnostic.
    Diagnostic {
        error: bool,
        location: Option<String>,
        message: String,
        rendered: Option<String>,
    },
    /// The built executable.
    Artifact(PathBuf),
    /// The command ended: success, and its last `error:` line.
    Done {
        success: bool,
        error: Option<String>,
    },
}

/// One line of `cargo --message-format=json` output.
fn parse_line(line: &str, bin: &str) -> Option<BuildMsg> {
    let Ok(json) = serde_json::from_str::<Json>(line) else {
        return Some(BuildMsg::Line(line.to_string()));
    };
    match json["reason"].as_str()? {
        "compiler-message" => {
            let m = &json["message"];
            let level = m["level"].as_str().unwrap_or_default();
            let error = level.starts_with("error");
            if !error && level != "warning" {
                return None;
            }
            let location = m["spans"].as_array().and_then(|spans| {
                spans.iter().find(|s| s["is_primary"] == true).map(|s| {
                    format!(
                        "{}:{}:{}",
                        s["file_name"].as_str().unwrap_or("?"),
                        s["line_start"],
                        s["column_start"]
                    )
                })
            });
            Some(BuildMsg::Diagnostic {
                error,
                location,
                message: m["message"].as_str().unwrap_or_default().to_string(),
                rendered: m["rendered"].as_str().map(str::to_string),
            })
        }
        "compiler-artifact" if json["target"]["name"] == bin => json["executable"]
            .as_str()
            .map(|e| BuildMsg::Artifact(PathBuf::from(e))),
        _ => None,
    }
}

/// A running build.
pub(crate) struct Build {
    rx: Receiver<BuildMsg>,
    target: RebuildTarget,
    artifact: Option<PathBuf>,
    errors: usize,
    /// UI time it started, for the status label.
    pub(crate) started: std::time::Instant,
}

fn spawn_build(target: &RebuildTarget) -> std::io::Result<Receiver<BuildMsg>> {
    let (program, args) = target
        .command
        .split_first()
        .ok_or_else(|| std::io::Error::other("empty build command"))?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(&target.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = channel();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let bin = target.bin.clone();
    let out_tx = tx.clone();
    let out = std::thread::Builder::new()
        .name("box-rebuild-out".into())
        .spawn(move || {
            if let Some(s) = stdout {
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    if let Some(m) = parse_line(&line, &bin) {
                        let _ = out_tx.send(m);
                    }
                }
            }
        })?;
    std::thread::Builder::new()
        .name("box-rebuild".into())
        .spawn(move || {
            let mut last_error = None;
            if let Some(s) = stderr {
                for line in BufReader::new(s).lines().map_while(Result::ok) {
                    if line.trim_start().starts_with("error") {
                        last_error = Some(line.trim().to_string());
                    }
                    let _ = tx.send(BuildMsg::Line(line));
                }
            }
            let _ = out.join();
            let success = child.wait().is_ok_and(|s| s.success());
            let _ = tx.send(BuildMsg::Done {
                success,
                error: last_error,
            });
        })?;
    Ok(rx)
}

/// What a restart runs: the new editor binary with its arguments, in a
/// folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestartCommand {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub dir: PathBuf,
}

/// Replaces the process with `cmd` (Unix `exec`; on Windows a new process
/// and an exit). Returns only on failure.
pub fn exec_restart(cmd: &RestartCommand) -> std::io::Error {
    let mut command = Command::new(&cmd.exe);
    command.args(&cmd.args).current_dir(&cmd.dir);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.exec()
    }
    #[cfg(not(unix))]
    {
        match command.spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => e,
        }
    }
}

/// The function that performs a restart (replaced in tests).
pub type RestartHook = Box<dyn FnMut(&RestartCommand) -> std::io::Error>;

/// Rebuild state, part of [`EditorState`].
#[derive(Default)]
pub struct Rebuild {
    /// Detection result for a project root (recomputed when it changes).
    cached: Option<(Option<PathBuf>, Result<RebuildTarget, String>)>,
    pub(crate) build: Option<Build>,
}

impl EditorState {
    /// Detects how Rebuild and Restart runs for the open project when the
    /// project changed (once per frame; cheap otherwise).
    pub(crate) fn refresh_rebuild_target(&mut self) {
        let root = self.project.as_ref().map(|p| p.root().to_path_buf());
        if self.rebuild.cached.as_ref().is_none_or(|(r, _)| *r != root) {
            let detected = RebuildTarget::detect(self.project.as_ref());
            self.rebuild.cached = Some((root, detected));
        }
    }

    /// How Rebuild and Restart runs for the open project, or why it is not
    /// available.
    pub fn rebuild_target(&self) -> Result<RebuildTarget, String> {
        let root = self.project.as_ref().map(|p| p.root().to_path_buf());
        match &self.rebuild.cached {
            Some((r, detected)) if *r == root => detected.clone(),
            // Not refreshed since the project changed.
            _ => RebuildTarget::detect(self.project.as_ref()),
        }
    }

    /// Whether Rebuild and Restart can start now, or why not.
    pub fn rebuild_availability(&self) -> Result<(), String> {
        if self.rebuild_running() {
            return Err("Building…".into());
        }
        self.rebuild_target().map(|_| ())
    }

    /// Uses `target` for Rebuild and Restart in this project (tests, or a
    /// launcher that knows better than [`RebuildTarget::detect`]).
    pub fn set_rebuild_target(&mut self, target: RebuildTarget) {
        let root = self.project.as_ref().map(|p| p.root().to_path_buf());
        self.rebuild.cached = Some((root, Ok(target)));
    }

    /// A rebuild is running.
    pub fn rebuild_running(&self) -> bool {
        self.rebuild.build.is_some()
    }

    /// Asks the shell for Rebuild and Restart (menu, shortcut).
    pub fn request_rebuild(&mut self) {
        self.requests.rebuild = true;
    }
}

impl Editor {
    /// Starts Rebuild and Restart. Refused (with an error toast) when it is
    /// not available, while one runs, or when the scene has unsaved changes
    /// but no file to keep them in.
    pub fn begin_rebuild(&mut self) {
        if self.state.rebuild_running() {
            return;
        }
        self.state.refresh_rebuild_target();
        let target = match self.state.rebuild_target() {
            Ok(t) => t,
            Err(reason) => {
                tracing::warn!("Rebuild and Restart: {reason}");
                return;
            }
        };
        if self.state.scene.path.is_none() && self.state.is_dirty() {
            tracing::error!(
                "Rebuild and Restart: save the scene first (it has no file yet, so its changes could not be restored)"
            );
            return;
        }
        tracing::info!(
            target: BUILD_TARGET,
            "rebuilding: {} (in {})",
            shell_line(&target.command),
            target.dir.display()
        );
        match spawn_build(&target) {
            Ok(rx) => {
                self.state.notify("Building…");
                self.state.rebuild.build = Some(Build {
                    rx,
                    target,
                    artifact: None,
                    errors: 0,
                    started: std::time::Instant::now(),
                });
            }
            Err(e) => tracing::error!(
                "Rebuild and Restart: could not run `{}`: {e}",
                shell_line(&target.command)
            ),
        }
    }

    /// Reads the build's output (once per frame): lines to the Console,
    /// errors as toasts; when it ends, restarts on success. Returns true
    /// while a build runs.
    pub fn poll_rebuild(&mut self) -> bool {
        let Some(build) = &mut self.state.rebuild.build else {
            return false;
        };
        let mut done = None;
        while let Ok(msg) = build.rx.try_recv() {
            match msg {
                BuildMsg::Line(line) => {
                    if !line.trim().is_empty() {
                        tracing::info!(target: BUILD_TARGET, "{line}");
                    }
                }
                BuildMsg::Diagnostic {
                    error,
                    location,
                    message,
                    rendered,
                } => {
                    let head = match &location {
                        Some(l) => format!("{l}: {message}"),
                        None => message,
                    };
                    if error {
                        build.errors += 1;
                        let full = rendered.unwrap_or_default();
                        tracing::error!(target: BUILD_TARGET, "{head}\n{}", full.trim_end());
                    } else if let Some(r) = rendered {
                        tracing::info!(target: BUILD_TARGET, "{}", r.trim_end());
                    }
                }
                BuildMsg::Artifact(path) => build.artifact = Some(path),
                BuildMsg::Done { success, error } => done = Some((success, error)),
            }
        }
        let Some((success, error)) = done else {
            return true;
        };
        let Some(build) = self.state.rebuild.build.take() else {
            return false;
        };
        if !success {
            let detail = match (build.errors, error) {
                (0, Some(e)) => e,
                (0, None) => "the build command failed".into(),
                (n, _) => format!("{n} compiler error{}", if n == 1 { "" } else { "s" }),
            };
            tracing::error!(
                "Rebuild failed: {detail}; the editor keeps running (Console has the output)"
            );
            return false;
        }
        let exe = build
            .artifact
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from(&build.target.bin));
        tracing::info!(
            target: BUILD_TARGET,
            "built {} in {:.1} s; restarting",
            exe.display(),
            build.started.elapsed().as_secs_f32()
        );
        self.restart(RestartCommand {
            exe,
            args: build.target.args.clone(),
            dir: build.target.dir.clone(),
        });
        false
    }

    /// Saves the session and replaces this editor with `cmd` (through the
    /// restart hook). If the restart fails the editor keeps running.
    fn restart(&mut self, cmd: RestartCommand) {
        if self.state.scene.path.is_none() && self.state.is_dirty() {
            tracing::error!(
                "Rebuild and Restart: the scene has no file, so it was not restarted; save the scene and try again"
            );
            return;
        }
        self.state.flush_settings_edit(f64::INFINITY, true);
        // Unsaved changes to the recovery file (this also ends Play), then
        // the session, layout and preferences as on quit.
        self.state.write_recovery_if_dirty();
        self.on_quit();
        let error = (self.restart_hook)(&cmd);
        tracing::error!(
            "could not restart into {}: {error}; this editor keeps running",
            cmd.exe.display()
        );
    }

    /// Replaces how a restart is performed (tests).
    pub fn set_restart_hook(&mut self, hook: RestartHook) {
        self.restart_hook = hook;
    }

    /// After a Rebuild and Restart (`--after-rebuild`): restores the
    /// scene's unsaved changes from its recovery file without asking (the
    /// session, restored before, gave the scene and camera; the saved
    /// selection is applied after the restore). Returns whether there was a
    /// recovery file to restore.
    pub(crate) fn restore_recovery_after_rebuild(&mut self) -> bool {
        let s = &mut self.state;
        if s.recovery_candidate().is_none() {
            return false;
        }
        // The saved selection: entities only in the unsaved changes were
        // not there when the session was applied to the scene file.
        let selection = match &s.project {
            Some(p) => {
                crate::prefs::load(&crate::prefs::prefs_path(p))
                    .session
                    .selection
            }
            None => s.selection.clone(),
        };
        if let Err(e) = s.restore_recovery() {
            tracing::error!("could not restore the unsaved changes after the restart: {e:#}");
            return false;
        }
        s.selection = selection
            .into_iter()
            .filter(|u| s.world.entity_by_uid(*u).is_some())
            .collect();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cargo_json_messages() {
        let err = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","spans":[{"file_name":"src/lib.rs","line_start":12,"column_start":5,"is_primary":true}],"rendered":"error[E0308]: mismatched types\n --> src/lib.rs:12:5\n"}}"#;
        match parse_line(err, "editor") {
            Some(BuildMsg::Diagnostic {
                error: true,
                location: Some(l),
                message,
                ..
            }) => {
                assert_eq!(l, "src/lib.rs:12:5");
                assert_eq!(message, "mismatched types");
            }
            other => panic!("{other:?}"),
        }
        let art = r#"{"reason":"compiler-artifact","target":{"name":"editor"},"executable":"/x/target/debug/editor"}"#;
        assert!(
            matches!(parse_line(art, "editor"), Some(BuildMsg::Artifact(p)) if p == Path::new("/x/target/debug/editor"))
        );
        assert!(parse_line(art, "player").is_none(), "another binary");
        assert!(parse_line(r#"{"reason":"build-finished","success":true}"#, "editor").is_none());
        assert!(matches!(
            parse_line("plain text", "editor"),
            Some(BuildMsg::Line(_))
        ));
    }

    #[test]
    fn restart_arguments_keep_the_project_and_drop_one_shot_flags() {
        let p = Path::new("/games/demo");
        let args = |a: &[&str]| restart_args(a.iter().map(|s| s.to_string()), p);
        assert_eq!(args(&[]), ["/games/demo", AFTER_REBUILD_FLAG]);
        assert_eq!(
            args(&[
                "old",
                "--play",
                "--control",
                "--size",
                "800x600",
                "--frames",
                "3",
                "--no-restore"
            ]),
            [
                "/games/demo",
                "--control",
                "--size",
                "800x600",
                AFTER_REBUILD_FLAG
            ]
        );
        assert_eq!(
            args(&["--capture", "a.png", "--control", AFTER_REBUILD_FLAG]),
            ["/games/demo", "--control", AFTER_REBUILD_FLAG]
        );
    }

    #[test]
    fn the_build_command_follows_the_crate() {
        let root =
            std::env::temp_dir().join(format!("box_rebuild_{}", box_core::identity::Uid::new()));
        std::fs::create_dir_all(root.join("src/bin")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\n[[bin]]\nname = \"editor\"\npath = \"src/bin/editor.rs\"\nrequired-features = [\"editor\"]\n",
        )
        .unwrap();
        let krate = NativeCrate::find(&root).unwrap();
        let t = RebuildTarget::new(&krate, "editor", true, &root);
        if std::env::var_os(REBUILD_COMMAND_ENV).is_none() {
            assert_eq!(
                t.command,
                [
                    "cargo",
                    "build",
                    "--message-format=json",
                    "--color",
                    "never",
                    "--release",
                    "--features",
                    "editor",
                    "--bin",
                    "editor"
                ]
            );
        }
        assert_eq!(t.dir, root);
        let project = Project::create(&root, "g").unwrap();
        let e = RebuildTarget::detect(Some(&project)).unwrap_err();
        assert!(e.contains("not the project's own editor"), "{e}");
        assert!(RebuildTarget::detect(None).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
