//! `box script types`: the luau-lsp definitions file for a project's
//! scripts, and the VS Code setting that points luau-lsp at it.
//! `box script docs`: the script API reference as Markdown, generated from
//! the same declarations.

use std::path::Path;

use box_engine::box_script::types::{DEFINITIONS_PATH, definitions, write_definitions};
use box_engine::{Game, Project};
use serde_json::{Value as Json, json};

use crate::{CliError, Output, cwd, game_registry, open_project};

/// Where `box script docs` writes inside a project by default.
const REFERENCE_DIR: &str = ".box/reference";

pub(crate) fn docs(project: Option<&Path>, out: Option<&Path>) -> Result<Output, CliError> {
    let project = match project {
        Some(p) => Some(open_project(Some(p))?),
        None => Project::find_root(&cwd()?).and_then(|r| Project::open(r).ok()),
    };
    let game = Game::new();
    let apis: Vec<_> = game.script_apis().iter().cloned().collect();
    let (registry, mut problems) = match &project {
        Some(p) => {
            let loaded = game_registry::for_project(p);
            (loaded.registry.clone(), loaded.problems.clone())
        }
        None => (game.registry(), Vec::new()),
    };
    let dir = match (out, &project) {
        (Some(o), _) => o.to_path_buf(),
        (None, Some(p)) => p.root().join(REFERENCE_DIR),
        (None, None) => {
            return Err(CliError::new(
                "usage",
                "not inside a project: give --out <dir> (or --project <dir>)",
            ));
        }
    };
    let reference = box_engine::box_script::docs::reference(&registry, &apis);
    let written = reference.write(&dir).map_err(|e| CliError::io(&dir, e))?;
    for item in reference.undocumented() {
        problems.push(json!({
            "severity": "warning",
            "code": "undocumented",
            "message": format!("no description: {item}"),
        }));
    }
    if project
        .as_ref()
        .is_some_and(|p| box_engine::player::NativeCrate::find(p.root()).is_some())
    {
        problems.push(json!({
            "severity": "info",
            "code": "native_reference",
            "message": "this native project's game types come from .box/registry.json; script APIs the game adds in Rust are not included",
        }));
    }
    let json = json!({
        "dir": dir.display().to_string(),
        "pages": reference.pages.len(),
        "items": reference.item_count(),
        "written": written,
        "undocumented": reference.undocumented().len(),
        "problems": problems,
    });
    let mut text = format!(
        "wrote {} ({} pages, {} items, {written} changed)",
        dir.display(),
        reference.pages.len(),
        reference.item_count()
    );
    for p in &problems {
        text.push_str(&format!(
            "\n{}: {}",
            p["severity"].as_str().unwrap_or("note"),
            p["message"].as_str().unwrap_or("")
        ));
    }
    Ok(Output::ok(json, text))
}

/// Where VS Code reads workspace settings.
const VSCODE_SETTINGS: &str = ".vscode/settings.json";

pub(crate) fn types(project: Option<&Path>) -> Result<Output, CliError> {
    let project = open_project(project)?;
    let loaded = game_registry::for_project(&project);
    let apis: Vec<_> = Game::new().script_apis().iter().cloned().collect();
    let text = definitions(&loaded.registry, &apis);
    let path =
        write_definitions(project.root(), &text).map_err(|e| CliError::io(project.root(), e))?;
    let mut problems: Vec<Json> = loaded.problems.clone();
    if box_engine::player::NativeCrate::find(project.root()).is_some() {
        problems.push(json!({
            "severity": "info",
            "code": "native_definitions",
            "file": DEFINITIONS_PATH,
            "message": "this native project's game types come from .box/registry.json; script APIs the game adds in Rust are written by its own editor or player at startup",
            "hint": box_engine::registry_file::refresh_command(&project),
        }));
    }
    // Point luau-lsp at the file, unless the project has its own settings.
    let settings = project.root().join(VSCODE_SETTINGS);
    let vscode = if settings.exists() {
        let has = std::fs::read_to_string(&settings).is_ok_and(|s| s.contains(DEFINITIONS_PATH));
        if !has {
            problems.push(json!({
                "severity": "info",
                "code": "vscode_settings",
                "file": VSCODE_SETTINGS,
                "message": "add the definitions file to luau-lsp in your VS Code settings",
                "hint": format!("\"luau-lsp.types.definitionFiles\": [\"{DEFINITIONS_PATH}\"]"),
            }));
        }
        "kept"
    } else {
        let body = json!({
            "luau-lsp.types.definitionFiles": [DEFINITIONS_PATH],
            "luau-lsp.platform.type": "standard",
        });
        std::fs::create_dir_all(settings.parent().unwrap_or(project.root()))
            .and_then(|_| {
                std::fs::write(
                    &settings,
                    serde_json::to_string_pretty(&body).unwrap_or_default() + "\n",
                )
            })
            .map_err(|e| CliError::io(&settings, e))?;
        "written"
    };
    let count = |prefix: &str| text.lines().filter(|l| l.starts_with(prefix)).count();
    let file = project.relative(&path);
    let json = json!({
        "project": project.root().display().to_string(),
        "file": file,
        "types": count("export type ") / 2,
        "declarations": count("declare "),
        "vscode_settings": vscode,
        "problems": problems,
    });
    let mut out = format!(
        "wrote {file} ({} types, {} globals)",
        count("export type ") / 2,
        count("declare ")
    );
    if vscode == "written" {
        out.push_str(&format!(
            "\nwrote {VSCODE_SETTINGS} (luau-lsp reads the definitions)"
        ));
    }
    for p in &problems {
        out.push_str(&format!(
            "\n{}: {}",
            p["severity"].as_str().unwrap_or("note"),
            p["message"].as_str().unwrap_or("")
        ));
    }
    Ok(Output::ok(json, out))
}
