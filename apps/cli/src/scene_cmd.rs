//! `box scene show` and `box scene apply`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use box_engine::Project;
use box_engine::asset_glue::canonicalize_scene_refs;
use box_engine::box_core::edit::{
    EditStack, apply_commands_with_warnings, parse_commands, resolve_entity,
};
use box_engine::box_core::scene;
use box_engine::project::write_atomic;
use serde_json::{Value as Json, json};

use crate::problems::{self, LogCollector};
use crate::{
    CliError, Output, game_registry, load_scene_with, locate, resolve_input, resolve_scene,
};

/// The edit command format: `box scene apply --help` and the MCP
/// `scene_apply`/`apply_commands` tools both show this text.
pub const COMMANDS_HELP: &str = r#"Commands: one command object or an array of them, applied in order as one all-or-nothing transaction. Each has an "op":
  spawn             {name, parent?, index?, uid?, components?}
  set               {entity, path, value}
  set_resource      {path, value}
  add_component     {entity, component, value?}
  remove_component  {entity, component}
  reparent          {entity, parent?, index?, keep?: "world" | "local"}
  despawn           {entity}
  rename            {entity, name}
  duplicate         {entity}
`entity` and `parent` take a uid or a unique name; `path` is Component.field.sub (e.g. "Transform.translation.y"); values use the scene-file JSON form that `box scene show` (MCP: scene_show, world_query) prints; `box registry` (MCP: registry) lists components, fields, ranges and units.

Example:
  [{"op": "spawn", "name": "Ball", "components": {"Primitive": {"shape": {"Sphere": {"radius": 0.5, "sectors": 32, "stacks": 16}}, "cast_shadows": true}}},
   {"op": "set", "entity": "Ball", "path": "Transform.translation", "value": [0.0, 1.0, 0.0]}]"#;

/// Entity records as in the scene file, each with a `"children"` list of uids.
fn records_with_children(
    file: &Path,
    world: &box_engine::box_core::world::World,
) -> Result<Json, CliError> {
    let bytes = scene::save(world).map_err(|e| CliError::scene(file, e))?;
    let mut tree: Json =
        serde_json::from_slice(&bytes).map_err(|e| CliError::new("internal", e.to_string()))?;
    let mut children: HashMap<String, Vec<Json>> = HashMap::new();
    if let Some(entities) = tree["entities"].as_array() {
        for e in entities {
            if let (Some(parent), Some(uid)) = (e["parent"].as_str(), e["uid"].as_str()) {
                children
                    .entry(parent.to_string())
                    .or_default()
                    .push(json!(uid));
            }
        }
    }
    if let Some(entities) = tree["entities"].as_array_mut() {
        for e in entities {
            let kids = e["uid"]
                .as_str()
                .and_then(|u| children.remove(u))
                .unwrap_or_default();
            if let Some(obj) = e.as_object_mut() {
                obj.insert("children".into(), Json::Array(kids));
            }
        }
    }
    Ok(tree)
}

fn entity_line(e: &Json, depth: usize) -> String {
    let components: Vec<&str> = e["components"]
        .as_object()
        .map(|c| c.keys().map(String::as_str).collect())
        .unwrap_or_default();
    format!(
        "{}{} ({}) [{}]",
        "  ".repeat(depth),
        e["name"].as_str().unwrap_or("<unnamed>"),
        e["uid"].as_str().unwrap_or("?"),
        components.join(", ")
    )
}

pub(crate) fn show(scene_arg: &Path, entity: Option<&str>) -> Result<Output, CliError> {
    let path = resolve_scene(scene_arg)?;
    let loaded = game_registry::for_path(&path);
    let world = load_scene_with(&path, loaded.registry.clone())?;
    let tree = records_with_children(&path, &world)?;
    let (project, file) = locate(&path);
    let entities = tree["entities"].as_array().cloned().unwrap_or_default();

    if let Some(reference) = entity {
        let uid = resolve_entity(&world, reference)
            .map_err(|e| CliError::edit(e).file(&path))?
            .to_string();
        let record = entities
            .into_iter()
            .find(|e| e["uid"].as_str() == Some(uid.as_str()))
            .ok_or_else(|| {
                CliError::new("entity_not_found", format!("entity {uid} is not saved"))
                    .file(&path)
                    .entity(uid.clone())
            })?;
        let text = serde_json::to_string_pretty(&record).unwrap_or_default();
        return Ok(Output::ok(
            json!({ "project": project, "file": file, "entity": record, "problems": loaded.problems }),
            text,
        ));
    }

    // Depth of each entity for the text tree (parents precede children).
    let mut depth: HashMap<String, usize> = HashMap::new();
    let mut text = String::new();
    for e in &entities {
        let d = e["parent"]
            .as_str()
            .and_then(|p| depth.get(p))
            .map_or(0, |d| d + 1);
        if let Some(uid) = e["uid"].as_str() {
            depth.insert(uid.to_string(), d);
        }
        text.push_str(&entity_line(e, d));
        text.push('\n');
    }
    let resources: Vec<&str> = tree["resources"]
        .as_object()
        .map(|r| r.keys().map(String::as_str).collect())
        .unwrap_or_default();
    text.push_str(&format!(
        "{} entities; resources: {}",
        entities.len(),
        if resources.is_empty() {
            "none".to_string()
        } else {
            resources.join(", ")
        }
    ));
    text.push_str(&problems::summary(&loaded.problems));
    let json = json!({
        "project": project,
        "file": file,
        "version": tree["version"],
        "resources": tree["resources"],
        "entities": entities,
        "problems": loaded.problems,
    });
    Ok(Output::ok(json, text))
}

fn read_commands(source: &str) -> Result<String, CliError> {
    if source == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| CliError::io(Path::new("<stdin>"), e))?;
        return Ok(text);
    }
    let path = resolve_input(Path::new(source))?;
    std::fs::read_to_string(&path).map_err(|e| CliError::io(&path, e))
}

pub(crate) fn apply(
    scene_arg: &Path,
    commands_arg: &str,
    dry_run: bool,
) -> Result<Output, CliError> {
    let text = read_commands(commands_arg)?;
    let source = if commands_arg == "-" {
        "<stdin>"
    } else {
        commands_arg
    };
    apply_text(scene_arg, &text, source, dry_run)
}

/// [`apply`] with the commands JSON given as text; `source` names it in errors.
pub(crate) fn apply_text(
    scene_arg: &Path,
    text: &str,
    source: &str,
    dry_run: bool,
) -> Result<Output, CliError> {
    let path = resolve_scene(scene_arg)?;
    let commands = parse_commands(text).map_err(|e| CliError::commands_parse(source, e))?;
    let collector = LogCollector::start();
    let loaded = game_registry::for_path(&path);
    let mut world = load_scene_with(&path, loaded.registry.clone())?;
    // Writing an older file would upgrade it silently, without a backup.
    if !dry_run && !commands.is_empty() {
        let mut tree: Json = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Json::Null);
        if let Ok(report) = scene::upgrade_tree(&loaded.registry, &mut tree)
            && !report.is_empty()
        {
            return Err(CliError::new(
                "upgrade_needed",
                format!(
                    "{} is in an older format ({}); upgrade the project first",
                    path.display(),
                    report.lines().join("; ")
                ),
            )
            .file(&path)
            .with("hint", json!("box upgrade (backs the files up first)")));
        }
    }
    let mut stack = EditStack::new();
    let applied =
        apply_commands_with_warnings(&mut world, &mut stack, &commands, "box scene apply")
            .map_err(|e| CliError::command(&path, e).name_entity(&world))?;
    // Applied differently from what was asked (a value clamped to its range).
    let mut command_problems: Vec<Json> = applied
        .warnings
        .iter()
        .map(|w| json!({ "severity": "warning", "code": "value_clamped", "message": w }))
        .collect();
    // Save repair: asset references written in canonical form (ids filled
    // in for path-only references, path hints current). References that
    // name no file are written as given, with a warning.
    let mut refs_repaired = 0;
    let mut asset_problems = Vec::new();
    if let Ok(project) = Project::open(&path)
        && project.assets_dir().is_dir()
    {
        let server = box_engine::asset_server(project.assets_dir())
            .map_err(|e| CliError::new("assets", e.to_string()).file(&path))?;
        if !dry_run && !commands.is_empty() {
            refs_repaired = canonicalize_scene_refs(&mut world, &mut stack, &server)
                .map_err(|e| CliError::new("assets", e.to_string()).file(&path))?;
        }
        asset_problems = problems::asset_ref_problems(&world, &server, false, "warning");
    }

    // Validate: the edited world must save and load back cleanly.
    let bytes = scene::save(&world).map_err(|e| CliError::scene(&path, e))?;
    scene::load(&bytes, loaded.registry.clone()).map_err(|e| CliError::scene(&path, e))?;
    // No commands, no changes: leave the file (and its formatting) alone.
    let written = !dry_run && !commands.is_empty();
    if written {
        write_atomic(&path, &bytes).map_err(|e| CliError::io(&path, e))?;
    }

    let created: Vec<String> = applied.created.iter().map(ToString::to_string).collect();
    let mut text = format!(
        "{} {} command(s) to {}",
        if dry_run { "Validated" } else { "Applied" },
        commands.len(),
        path.display()
    );
    for uid in &created {
        text.push_str(&format!(
            "\ncreated {}",
            problems::entity_label(&world, uid)
        ));
    }
    command_problems.extend(problems::merge(asset_problems, collector.finish()));
    let mut problems = loaded.problems.clone();
    problems.extend(command_problems);
    text.push_str(&problems::summary(&problems));
    let (project, file) = locate(&path);
    let json = json!({
        "project": project,
        "file": file,
        "commands": commands.len(),
        "created": created,
        "dry_run": dry_run,
        "written": written,
        "refs_repaired": refs_repaired,
        "problems": problems,
    });
    Ok(Output::ok(json, text))
}
