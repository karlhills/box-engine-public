//! `box export`: a runnable macOS `.app` or Windows folder (box_export).

use std::io::Write;

use box_engine::packed::Profile;
use box_export::{ExportError, ExportOptions, Target};
use serde_json::json;

use crate::{CliError, ExportArgs, Output, cwd};

fn error(e: ExportError) -> CliError {
    let mut err = CliError::new(e.code(), e.to_string());
    match &e {
        ExportError::Cook { problems } => err = err.with("problems", json!(problems)),
        ExportError::Io { path, .. } | ExportError::OutputExists(path) => err = err.file(path),
        ExportError::Project(p) => {
            if let Some(path) = p.path() {
                err = err.file(path);
            }
        }
        _ => {}
    }
    err
}

pub(crate) fn export(args: &ExportArgs) -> Result<Output, CliError> {
    let target = match &args.target {
        Some(t) => Target::parse(t).ok_or_else(|| {
            CliError::new(
                "usage",
                format!("unknown --target `{t}`; use macos or windows"),
            )
        })?,
        None => Target::host(),
    };
    let profile = Profile::parse(&args.profile).ok_or_else(|| {
        CliError::new(
            "usage",
            format!(
                "unknown --profile `{}`; use release or development",
                args.profile
            ),
        )
    })?;
    let project = match &args.project {
        Some(p) => p.clone(),
        None => cwd()?,
    };
    let mut options = ExportOptions::new(project, target, profile, &args.out);
    options.player = args.player.clone();
    options.sign = !args.no_sign;
    // Progress on stderr; stdout carries the result.
    let mut last = "";
    let report = box_export::export(&options, &mut |p| {
        if p.step != last || p.step != "import" {
            let _ = writeln!(std::io::stderr(), "{}", p.message);
        }
        last = p.step;
    })
    .map_err(error)?;
    let mut text = format!(
        "exported {} ({} {}, {} build)\n  run: {}\n  game.pak: {} files, {:.1} KiB (project assets {:.1} KiB)\n  time: {:.0} ms (import {:.0} ms, pak {:.0} ms, bundle {:.0} ms, sign {:.0} ms{})",
        report.output.display(),
        report.name,
        report.version,
        report.profile.name(),
        report.executable.display(),
        report.cook.files,
        report.cook.pak_bytes as f64 / 1024.0,
        report.cook.source_bytes as f64 / 1024.0,
        report.total_ms,
        report.cook.import_ms,
        report.cook.write_ms,
        report.bundle_ms,
        report.sign_ms,
        if report.player_source == "cargo" {
            format!(", cargo build {:.0} ms", report.build_ms)
        } else {
            String::new()
        }
    );
    for w in &report.warnings {
        text.push_str(&format!("\nwarning: {w}"));
    }
    let mut value =
        serde_json::to_value(&report).map_err(|e| CliError::new("internal", e.to_string()))?;
    value["ok"] = json!(true);
    Ok(Output::ok(value, text))
}
