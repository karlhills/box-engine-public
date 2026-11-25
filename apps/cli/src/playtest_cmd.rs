//! `box playtest`: replay playtests headless and report each check
//! (`box_engine::playtest`).

use std::path::{Path, PathBuf};

use box_engine::player::{NativeCrate, PlayerOptions};
use box_engine::playtest::{self, Playtest, PlaytestError, PlaytestReport, ReplayOptions};
use box_engine::{Game, Project};
use serde_json::{Value as Json, json};

use crate::problems::{self, LogCollector};
use crate::{CliError, Output, PlaytestArgs, cwd};

fn playtest_error(e: PlaytestError, file: &Path) -> CliError {
    CliError::new(e.code, e.message).file(file)
}

/// The project and the playtest files to run: the one given (a path, or a
/// name in `playtests/`), else every `playtests/*.playtest.json`.
fn targets(args: &PlaytestArgs) -> Result<(Project, Vec<PathBuf>), CliError> {
    // `box playtest some.playtest.json`: the file names its project.
    let (project_arg, file) = match (&args.project, &args.file) {
        (Some(p), None) if p.to_string_lossy().ends_with(".json") && p.is_file() => (
            p.parent().map(Path::to_path_buf),
            Some(p.display().to_string()),
        ),
        (p, f) => (p.clone(), f.clone()),
    };
    let base = match project_arg {
        Some(p) => p,
        None => cwd()?,
    };
    if !base.exists() {
        let message = box_engine::ProjectError::NotFound(base.clone()).to_string();
        return Err(CliError::new("not_found", message).file(&base));
    }
    let project = Project::open(&base)?;
    if let Some(file) = file {
        let given = Path::new(&file);
        let path = if given.is_file() {
            given.to_path_buf()
        } else {
            playtest::path_in_project(project.root(), &file)
        };
        if !path.is_file() {
            return Err(CliError::new(
                "not_found",
                format!(
                    "no playtest {} ({}){}",
                    path.display(),
                    "a name means playtests/<name>.playtest.json in the project",
                    available(&project)
                ),
            )
            .file(&path));
        }
        return Ok((project, vec![path]));
    }
    let files = list(&project);
    if files.is_empty() {
        return Err(CliError::new(
            "no_playtests",
            format!(
                "{} has no playtests: record one (MCP record_start/record_stop) or write playtests/<name>.playtest.json",
                project.root().display()
            ),
        ));
    }
    Ok((project, files))
}

/// `playtests/*.playtest.json`, sorted.
fn list(project: &Project) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(project.root().join(playtest::DIR))
        .map(|dir| {
            dir.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(playtest::EXTENSION))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

fn available(project: &Project) -> String {
    let names: Vec<String> = list(project).iter().map(|p| playtest::name_of(p)).collect();
    if names.is_empty() {
        "; the project has no playtests".into()
    } else {
        format!("; playtests: {}", names.join(", "))
    }
}

pub(crate) fn playtest(args: &PlaytestArgs) -> Result<Output, CliError> {
    let (project, files) = targets(args)?;
    let collector = LogCollector::start();
    let native = NativeCrate::find(project.root());
    let mut reports: Vec<(String, PlaytestReport)> = Vec::new();
    for file in &files {
        let rel = project.relative(file);
        let report = match &native {
            Some(native) => replay_native(&project, native, file, args)?,
            None => {
                let playtest = Playtest::read(file).map_err(|e| playtest_error(e, file))?;
                let options = ReplayOptions {
                    capture_every: args.capture_every,
                    capture_dir: args.capture_dir.clone(),
                    size: args.size.unwrap_or((1280, 720)),
                };
                let player = PlayerOptions {
                    project: Some(project.root().to_path_buf()),
                    ..Default::default()
                };
                playtest::replay(
                    &Game::new(),
                    &player,
                    &playtest,
                    &playtest::name_of(file),
                    &options,
                )
                .map_err(|e| playtest_error(e, file))?
            }
        };
        reports.push((rel, report));
    }
    let problems = collector.finish();
    let passed = reports.iter().all(|(_, r)| r.passed);
    let mut text: Vec<String> = reports.iter().map(|(_, r)| r.text()).collect();
    if reports.len() > 1 {
        let failed = reports.iter().filter(|(_, r)| !r.passed).count();
        text.push(format!(
            "{} of {} playtest(s) passed",
            reports.len() - failed,
            reports.len()
        ));
    }
    let mut text = text.join("\n");
    text.push_str(&problems::summary(&problems));
    let root = project.root().display().to_string();
    let json = if let [(file, report)] = reports.as_slice() {
        let mut v = playtest::report_json(report, Some(file));
        v["project"] = json!(root);
        v["problems"] = json!(problems);
        v
    } else {
        json!({
            "project": root,
            "passed": passed,
            "playtests": reports
                .iter()
                .map(|(file, r)| playtest::report_json(r, Some(file)))
                .collect::<Vec<_>>(),
            "problems": problems,
        })
    };
    Ok(Output {
        json,
        text,
        success: passed,
    })
}

/// A native project's playtest runs in its own player (`--playtest`), built
/// and run through cargo; the report comes back as a JSON file.
fn replay_native(
    project: &Project,
    native: &NativeCrate,
    file: &Path,
    args: &PlaytestArgs,
) -> Result<PlaytestReport, CliError> {
    let Some(bin) = native.player_bin() else {
        return Err(CliError::new(
            "native_project",
            format!(
                "`{}` is a native project, but its Cargo.toml has no player binary",
                project.name()
            ),
        )
        .file(&native.manifest));
    };
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let out = std::env::temp_dir().join(format!(
        "box-playtest-{}-{}.json",
        std::process::id(),
        playtest::name_of(file)
    ));
    let _ = std::fs::remove_file(&out);
    let mut player_args = vec![
        ".".to_string(),
        "--playtest".into(),
        absolute(file).display().to_string(),
        "--report".into(),
        out.display().to_string(),
    ];
    if let Some(n) = args.capture_every {
        player_args.extend(["--capture-every".into(), n.to_string()]);
    }
    if let Some((w, h)) = args.size {
        player_args.extend(["--size".into(), format!("{w}x{h}")]);
    }
    let command = native.cargo_run(bin, &player_args);
    let line = native.shell_line_in_dir(&command);
    crate::run_cmd::build_native(native, &command)?;
    let status = std::process::Command::new(&command[0])
        .args(&command[1..])
        .current_dir(native.dir())
        // stdout carries only `box`'s own result.
        .stdout(std::io::stderr())
        .status()
        .map_err(|e| CliError::new("io", format!("could not run `{line}`: {e}")))?;
    let report = std::fs::read_to_string(&out)
        .ok()
        .and_then(|t| serde_json::from_str::<Json>(&t).ok());
    let _ = std::fs::remove_file(&out);
    match report.map(serde_json::from_value::<PlaytestReport>) {
        Some(Ok(r)) => Ok(r),
        _ => Err(CliError::new(
            "run_failed",
            format!("`{line}` failed ({status}) without a playtest report; its output is above"),
        )
        .file(file)),
    }
}
