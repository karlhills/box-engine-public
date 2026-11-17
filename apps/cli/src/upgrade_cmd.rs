//! `box upgrade`: migrate a project's files to this engine's formats after
//! a backup (`box_engine::upgrade`).

use std::path::Path;

use box_engine::upgrade::{self, UpgradeError};
use serde_json::json;

use crate::{CliError, Output, game_registry, open_project};

fn upgrade_error(e: UpgradeError) -> CliError {
    match e {
        UpgradeError::Io { path, source } => CliError::io(&path, source),
        UpgradeError::Blocked(m) => CliError::new("upgrade_blocked", m),
        UpgradeError::Project { path, message } => {
            CliError::new("project_invalid", format!("{}: {message}", path.display())).file(path)
        }
        e @ UpgradeError::Partial { .. } => CliError::new("upgrade_partial", e.to_string()),
    }
}

/// Plans the upgrade with the project's registry (the engine's types plus a
/// native project's game types from `.box/registry.json`) and, without
/// `dry_run`, applies it. A project that needs nothing is a success; a
/// blocked one (newer files, failed steps) is a failure with exit code 1.
pub(crate) fn upgrade(project: Option<&Path>, dry_run: bool) -> Result<Output, CliError> {
    let project = open_project(project)?;
    let loaded = game_registry::for_project(&project);
    let plan = upgrade::plan(project.root(), &loaded.registry).map_err(upgrade_error)?;
    let mut json = plan.to_json();
    json["dry_run"] = json!(dry_run);
    json["problems"] = json!(loaded.problems);
    let mut text = plan.summary();
    let needs_game = plan
        .errors
        .iter()
        .any(|p| p.message.contains("only in the game's code"));
    if needs_game {
        let hint = match box_engine::player::NativeCrate::find(project.root())
            .and_then(|n| n.player_bin().map(str::to_string).zip(Some(n)))
        {
            Some((bin, native)) => {
                native.shell_line_in_dir(&native.cargo_run(&bin, &["--upgrade".to_string()]))
            }
            None => "run the game's player with --upgrade".to_string(),
        };
        json["hint"] = json!(hint);
        text.push_str(&format!("\nhint: the game's own steps are needed: {hint}"));
    }
    if plan.blocked() {
        json["applied"] = json!(false);
        return Ok(Output {
            json,
            text,
            success: false,
        });
    }
    if dry_run || !plan.needed() {
        json["applied"] = json!(false);
        if dry_run && plan.needed() {
            text.push_str("\n(dry run: nothing was written; run without --dry-run to upgrade)");
        }
        return Ok(Output::ok(json, text));
    }
    let report = upgrade::apply(&plan).map_err(upgrade_error)?;
    json["applied"] = json!(true);
    json["backup"] = json!(project.relative(&report.backup));
    json["written"] = json!(report.written);
    text.push_str(&format!(
        "\nupgraded {} file(s); the originals are in {}",
        report.written.len(),
        project.relative(&report.backup)
    ));
    Ok(Output::ok(json, text))
}
