//! `box run`: play a project with the engine's types, through the same
//! player library as `box-player`.

use std::path::{Path, PathBuf};

use box_engine::box_render::RenderError;
use box_engine::player::{self, NativeCrate, PlayerOptions};
use box_engine::{Game, Project, RunOptions};
use serde_json::{Value as Json, json};

use crate::problems::{self, LogCollector};
use crate::{CliError, Output, RunArgs, cwd, load_scene, resolve_input};

/// The project and, when the target is a scene file, that scene.
fn target(args: &RunArgs) -> Result<(Project, Option<PathBuf>), CliError> {
    let path = match &args.target {
        Some(t) => resolve_input(t)?,
        None => cwd()?,
    };
    let project = Project::open(&path)?;
    let scene = path
        .is_file()
        .then(|| std::path::absolute(&path).map_err(|e| CliError::io(&path, e)))
        .transpose()?;
    Ok((project, scene))
}

/// Compiler errors kept in a `build_failed` error's `problems`.
const MAX_COMPILE_PROBLEMS: usize = 20;

/// `cargo build` of the player binary (the `cargo run` command
/// `run` without its arguments), with cargo's JSON messages: diagnostics are
/// printed as cargo prints them (stderr), and errors become `compile_error`
/// problems with file, line and column. Fails with `build_failed`.
pub(crate) fn build_native(native: &NativeCrate, run: &[String]) -> Result<(), CliError> {
    use std::io::{BufRead, Write};
    let mut command: Vec<String> = run
        .iter()
        .take_while(|a| *a != "--")
        .map(|a| {
            if a == "run" {
                "build".into()
            } else {
                a.clone()
            }
        })
        .collect();
    command.push("--message-format=json-diagnostic-rendered-ansi".into());
    let line = native.shell_line_in_dir(&command);
    let mut child = std::process::Command::new(&command[0])
        .args(&command[1..])
        .current_dir(native.dir())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                "cargo_not_found"
            } else {
                "io"
            };
            CliError::new(
                code,
                format!("could not run `{line}`: {e}; native projects need the Rust toolchain (https://rustup.rs)"),
            )
        })?;
    let mut errors = Vec::new();
    let mut total = 0;
    let colour = std::io::IsTerminal::is_terminal(&std::io::stderr());
    if let Some(out) = child.stdout.take() {
        for text in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            let Some((rendered, problem)) = compile_message(&text, native.dir()) else {
                continue;
            };
            let rendered = if colour {
                rendered
            } else {
                strip_ansi(&rendered)
            };
            let _ = write!(std::io::stderr(), "{rendered}");
            if let Some(p) = problem {
                total += 1;
                if errors.len() < MAX_COMPILE_PROBLEMS {
                    errors.push(p);
                }
            }
        }
    }
    let status = child.wait().map_err(|e| CliError::io(native.dir(), e))?;
    if status.success() {
        return Ok(());
    }
    let first = errors.first().map(|p: &Json| {
        format!(
            "; first: {}:{}:{}: {}",
            p["file"].as_str().unwrap_or("?"),
            p["line"],
            p["column"],
            p["message"].as_str().unwrap_or("")
        )
    });
    let message = match total {
        0 => format!("`{line}` failed ({status}); its output is above"),
        n => format!(
            "the game's code does not compile ({n} error{}{}); fix it and run again",
            if n == 1 { "" } else { "s" },
            first.unwrap_or_default()
        ),
    };
    let mut err = CliError::new("build_failed", message)
        .with("command", json!(command))
        .with("problems", json!(errors))
        .exit_with(
            status
                .code()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(1),
        );
    err.file = match errors.first().and_then(|p| p["file"].as_str()) {
        Some(f) => Some(f.to_string()),
        None => Some(native.manifest.display().to_string()),
    };
    Err(err)
}

/// One line of cargo's JSON output: a compiler diagnostic's rendered text
/// and, for an error, its `compile_error` problem (`file` relative to
/// `dir`).
fn compile_message(line: &str, dir: &Path) -> Option<(String, Option<Json>)> {
    let msg: Json = serde_json::from_str(line).ok()?;
    if msg["reason"] != "compiler-message" {
        return None;
    }
    let m = &msg["message"];
    let rendered = m["rendered"].as_str()?.to_string();
    if m["level"] != "error" {
        return Some((rendered, None));
    }
    let span = m["spans"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["is_primary"] == true));
    let mut problem = json!({
        "severity": "error",
        "code": "compile_error",
        "message": m["message"],
    });
    if let Some(s) = span {
        let file = s["file_name"].as_str().unwrap_or_default();
        let shown = Path::new(file)
            .strip_prefix(dir)
            .map_or_else(|_| file.to_string(), |p| p.display().to_string());
        problem["file"] = json!(shown);
        problem["line"] = s["line_start"].clone();
        problem["column"] = s["column_start"].clone();
    }
    Some((rendered, Some(problem)))
}

/// `text` without ANSI colour sequences (`ESC [ ... m`).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Runner flags as `box-player` takes them.
fn flags(args: &RunArgs) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(n) = args.frames {
        out.extend(["--frames".to_string(), n.to_string()]);
    }
    if let Some((w, h)) = args.size {
        out.extend(["--size".to_string(), format!("{w}x{h}")]);
    }
    for (flag, path) in [
        ("--capture", &args.capture),
        ("--report", &args.report),
        ("--audio-report", &args.audio_report),
    ] {
        if let Some(p) = path {
            out.extend([flag.to_string(), p.display().to_string()]);
        }
    }
    if args.frames.is_some() && args.capture.is_none() {
        out.push("--headless".into());
    }
    if args.control {
        out.push("--control".into());
    }
    if args.no_watch {
        out.push("--no-watch".into());
    }
    out
}

/// A native project's game code is not in this binary: run its own player
/// through cargo (output streamed; with `--json`, the child's stdout goes
/// to stderr so stdout stays one JSON result), or with `--no-cargo` print
/// the command.
fn run_native(
    project: &Project,
    native: &NativeCrate,
    scene: Option<&Path>,
    args: &RunArgs,
    json_output: bool,
) -> Result<Output, CliError> {
    let Some(bin) = native.player_bin() else {
        return Err(CliError::new(
            "native_project",
            format!(
                "`{}` is a native project, but its Cargo.toml has no player binary (binaries: {}); add a `[[bin]] name = \"player\"` that calls box_engine::player::run with the game",
                project.name(),
                if native.bins.is_empty() { "none".to_string() } else { native.bins.join(", ") }
            ),
        )
        .file(&native.manifest));
    };
    // Runs in the project directory: relative paths are made absolute.
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut player_args = vec![".".to_string()];
    if let Some(s) = scene {
        player_args.extend(["--scene".to_string(), s.display().to_string()]);
    }
    let abs_args = RunArgs {
        target: None,
        frames: args.frames,
        report: args.report.as_deref().map(absolute),
        audio_report: args.audio_report.as_deref().map(absolute),
        capture: args.capture.as_deref().map(absolute),
        size: args.size,
        control: args.control,
        no_watch: args.no_watch,
        no_cargo: false,
    };
    player_args.extend(flags(&abs_args));
    let command = native.cargo_run(bin, &player_args);
    let line = native.shell_line_in_dir(&command);
    let mut json = json!({
        "project": project.root().display().to_string(),
        "native": true,
        "dir": native.dir().display().to_string(),
        "command": command,
        "command_line": line,
    });
    if args.no_cargo {
        return Ok(Output::ok(json, line));
    }
    // Build first, so compiler errors are reported as such (with file and
    // line) instead of as a failed run.
    build_native(native, &command)?;
    let collector = LogCollector::start();
    tracing::info!("running {line}");
    let mut child = std::process::Command::new(&command[0]);
    child.args(&command[1..]).current_dir(native.dir());
    if json_output {
        // stdout carries only the JSON result.
        child.stdout(std::io::stderr());
    }
    let status = child.status().map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            "cargo_not_found"
        } else {
            "io"
        };
        CliError::new(
            code,
            format!("could not run `{line}`: {e}; native projects need the Rust toolchain (https://rustup.rs)"),
        )
    })?;
    let exit_code = status.code();
    json["exit_code"] = json!(exit_code);
    let problems = collector.finish();
    if !status.success() {
        let code = exit_code.and_then(|c| u8::try_from(c).ok()).unwrap_or(1);
        return Err(CliError::new(
            "run_failed",
            format!("`{line}` failed ({status}); its output is above"),
        )
        .file(&native.manifest)
        .with("command", json!(command))
        .exit_with(code));
    }
    if let Some(p) = &args.report
        && let Ok(text) = std::fs::read_to_string(p)
        && let Ok(report) = serde_json::from_str::<Json>(&text)
    {
        json["timings"] = json!({
            "frame_ms": report["frame_ms"],
            "update_ms": report["update_ms"],
            "render_cpu_ms": report["render_cpu_ms"],
        });
    }
    json["captures"] = json!(args.capture.as_ref().map(|p| p.display().to_string()));
    if let Some(p) = &args.audio_report {
        json["audio"] = audio_summary(p);
    }
    json["problems"] = json!(problems);
    let text = format!(
        "Ran `{}` with its own player ({line}){}",
        project.name(),
        problems::summary(&problems)
    );
    Ok(Output::ok(json, text))
}

/// The `audio` part of a run's result: the report file, the mix level and
/// each source's RMS and peak (the whole report is in the file).
fn audio_summary(path: &Path) -> Json {
    let report = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Json>(&t).ok());
    let Some(r) = report else {
        return json!({
            "report": path.display().to_string(),
            "error": "the audio report was not written (see the problems)",
        });
    };
    let sources: Vec<Json> = r["sources"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    json!({
                        "name": s["name"],
                        "bus": s["bus"],
                        "rms_db": s["rms_db"],
                        "peak_db": s["peak_db"],
                        "heard_seconds": s["heard_seconds"],
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "report": path.display().to_string(),
        "output": r["output"],
        "measured": r["measured"],
        "mix": r["mix"],
        "sources": sources,
    })
}

/// Text lines of an audio summary: the mix, then each source.
fn audio_text(a: &Json) -> String {
    let db = |v: &Json| match v.as_f64() {
        Some(d) => format!("{d:.1} dB"),
        None => "silent".into(),
    };
    let mut text = format!("\naudio report {}", a["report"].as_str().unwrap_or(""));
    if a["measured"] != true {
        text.push_str(" (not measured: the output was silent)");
        return text;
    }
    if a["mix"].is_object() {
        text.push_str(&format!(
            "\n  mix: rms {}, peak {}",
            db(&a["mix"]["rms_db"]),
            db(&a["mix"]["peak_db"])
        ));
    }
    for s in a["sources"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "\n  {} ({}): rms {}, peak {}",
            s["name"].as_str().unwrap_or("?"),
            s["bus"].as_str().unwrap_or("?"),
            db(&s["rms_db"]),
            db(&s["peak_db"])
        ));
    }
    text
}

fn run_error(e: anyhow::Error) -> CliError {
    let no_adapter = e.chain().any(|c| {
        matches!(
            c.downcast_ref::<RenderError>(),
            Some(RenderError::NoAdapter)
        )
    });
    let message = player::error_text(&e);
    let code = if no_adapter {
        "no_gpu"
    } else if message.contains("GPU validation error") {
        "gpu_validation"
    } else {
        "run_failed"
    };
    CliError::new(code, message)
}

pub(crate) fn run(args: &RunArgs, json_output: bool) -> Result<Output, CliError> {
    let (project, scene) = target(args)?;
    if let Some(native) = NativeCrate::find(project.root()) {
        return run_native(&project, &native, scene.as_deref(), args, json_output);
    }
    if args.no_cargo {
        return Err(CliError::new(
            "usage",
            format!(
                "--no-cargo is for native projects; `{}` has no Cargo.toml",
                project.name()
            ),
        ));
    }
    let scene_path = scene.clone().unwrap_or_else(|| project.start_scene_path());
    if scene.is_none() && !scene_path.exists() {
        return Err(CliError::new(
            "start_scene_missing",
            player::missing_start_scene_text(&project, ""),
        )
        .file(project.root().join(box_engine::project::PROJECT_FILE)));
    }
    // Fail early with the CLI's scene error codes.
    load_scene(&scene_path)?;
    let collector = LogCollector::start();

    let headless = args.frames.is_some() || args.capture.is_some();
    let mut run = RunOptions::new(project.name());
    if let Some((w, h)) = args.size {
        (run.width, run.height) = (w, h);
    }
    run.frames = args.frames;
    run.capture = args.capture.clone();
    run.report = args.report.clone();
    run.headless = headless;
    run.control = args.control;
    let options = PlayerOptions {
        project: Some(project.root().to_path_buf()),
        scene: scene.map(|s| s.display().to_string()),
        run,
        watch: !args.no_watch,
        audio_report: args.audio_report.clone(),
        ..Default::default()
    };
    let summary = player::run_game(options, Game::new()).map_err(run_error)?;

    let report = summary.report.unwrap_or(Json::Null);
    let timings = if report.is_null() {
        Json::Null
    } else {
        json!({
            "frame_ms": report["frame_ms"],
            "update_ms": report["update_ms"],
            "render_cpu_ms": report["render_cpu_ms"],
        })
    };
    let captures: Vec<String> = summary
        .captures
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let scene_rel = project.relative(&scene_path);
    let problems = collector.finish();
    let audio = args.audio_report.as_deref().map(audio_summary);
    let json = json!({
        "project": project.root().display().to_string(),
        "scene": scene_rel,
        "headless": headless,
        "control": args.control,
        "frames": summary.frames,
        "validation_errors": summary.validation_errors,
        "timings": timings,
        "adapter": report["adapter"],
        "backend": report["backend"],
        "captures": captures,
        "report": args.report.as_ref().map(|p| p.display().to_string()),
        "audio": audio,
        "problems": problems,
    });
    let mut text = format!(
        "Ran `{}` ({scene_rel}) for {} frame(s){}, {} validation error(s)",
        project.name(),
        summary.frames,
        if headless { " headless" } else { "" },
        summary.validation_errors
    );
    if let Some(p50) = report["frame_ms"]["p50"].as_f64() {
        let update = report["update_ms"]["p50"].as_f64().unwrap_or(0.0);
        let render = report["render_cpu_ms"]["p50"].as_f64().unwrap_or(0.0);
        text.push_str(&format!(
            "\nframe p50 {p50:.2} ms, update p50 {update:.2} ms, render CPU p50 {render:.2} ms"
        ));
    }
    for c in &captures {
        text.push_str(&format!("\ncaptured {c}"));
    }
    if let Some(a) = &audio {
        text.push_str(&audio_text(a));
    }
    text.push_str(&problems::summary(&problems));
    Ok(Output::ok(json, text))
}
