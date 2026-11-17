//! `box-player [project_dir | game.pak] [--scene <name|path>] [--capture <png>] [--frames N] [--size WxH] [--report <json>] [--headless] [--control] [--force-engine-only]`
//!
//! Runs a project's start scene (or `--scene`) with the engine's types only.
//! Given a `.pak` (or with a `game.pak` next to it, as in an exported game)
//! it plays the pak instead; this binary is the player template `box export`
//! copies into macOS and Windows bundles.
//! A native project (one with a `Cargo.toml` at its root) is refused with
//! the command that runs its own player binary, which includes its game
//! plugins; `--force-engine-only` plays it anyway, without its game code.
//! Without a project folder the current directory (or the nearest parent
//! with a `box.project.json`) is used. `--capture` or `--headless` runs
//! without a window. `--control` serves the live control socket (`box mcp`
//! live tools) for the project; `--headless --control` without `--frames`
//! runs until the `stop` method. `--help` lists the flags.

use std::process::ExitCode;

use anyhow::bail;
use box_engine::Project;
use box_engine::player::{NativeCrate, PlayerOptions, USAGE, error_text, program_name, shell_line};

const FORCE: &str = "--force-engine-only";

fn main() -> ExitCode {
    match play() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", error_text(&e));
            ExitCode::FAILURE
        }
    }
}

fn play() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        let force_help = format!(
            "  {FORCE}   Play a native project (one with a Cargo.toml) without\n                        its game code; normally refused\n  -h, --help"
        );
        println!(
            "Usage: {} {}",
            program_name(),
            USAGE.replacen("  -h, --help", &force_help, 1)
        );
        return Ok(());
    }
    let force = args.iter().any(|a| a == FORCE);
    args.retain(|a| a != FORCE);
    let options = PlayerOptions::from_args(args.clone())?;
    // An exported game (a pak) carries no game code to miss.
    if !force && box_engine::player::pak_to_play(&options).is_none() {
        refuse_native(&options, &args)?;
    }
    box_engine::player::run_game(options, box_engine::Game::new())?;
    Ok(())
}

/// A native project's components and systems are not in this binary: say
/// which command plays it with them.
fn refuse_native(options: &PlayerOptions, args: &[String]) -> anyhow::Result<()> {
    let dir = match &options.project {
        Some(p) => p.clone(),
        None => std::env::current_dir()?,
    };
    let Ok(project) = Project::open(&dir) else {
        return Ok(());
    };
    let Some(native) = NativeCrate::find(project.root()) else {
        return Ok(());
    };
    // The same arguments, run in the project directory.
    // Output paths made absolute, since the command runs elsewhere.
    let mut forwarded = vec![".".to_string()];
    let given = options.project.as_ref().map(|p| p.as_os_str());
    let mut path_value = false;
    for a in args {
        if path_value {
            path_value = false;
            let abs = std::path::absolute(a).unwrap_or_else(|_| a.into());
            forwarded.push(abs.display().to_string());
        } else if given != Some(std::ffi::OsStr::new(a.as_str())) {
            path_value = a == "--capture" || a == "--report";
            forwarded.push(a.clone());
        }
    }
    let run = match native.player_bin() {
        Some(bin) => format!(
            "run its own player instead:\n  {}\n(or `box run {}`)",
            native.shell_line_in_dir(&native.cargo_run(bin, &forwarded)),
            shell_line(&[project.root().display().to_string()])
        ),
        None => format!(
            "build and run its player binary (found binaries: {})",
            if native.bins.is_empty() {
                "none".to_string()
            } else {
                native.bins.join(", ")
            }
        ),
    };
    bail!(
        "`{}` is a native project ({} exists): its game code (components, systems) is not in box-player, so it would play without it. To play it, {run}\nTo play it with the engine's types only, add {FORCE}.",
        project.name(),
        native.manifest.display()
    )
}
