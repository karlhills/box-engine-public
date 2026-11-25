//! `box walk`: walk a probe character through a scene's colliders
//! (`box_engine::route`).

use std::path::Path;

use box_engine::route::{RouteSpec, walk_route};
use box_engine::{Project, app_with_world, settle_assets};
use serde_json::json;

use crate::problems::{self, LogCollector};
use crate::{CliError, Output, WalkArgs, load_scene, locate, resolve_scene};

const FIXED_DT: f32 = 1.0 / 60.0;
const ASSET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) fn walk(args: &WalkArgs) -> Result<Output, CliError> {
    let text = std::fs::read_to_string(&args.route).map_err(|e| CliError::io(&args.route, e))?;
    let spec: RouteSpec = serde_json::from_str(&text).map_err(|e| {
        CliError::new("usage", format!("--route {}: {e}", args.route.display())).file(&args.route)
    })?;
    walk_spec(&args.scene, &spec)
}

/// Walk `spec` through the scene `scene` (a file or a project directory).
pub(crate) fn walk_spec(scene: &Path, spec: &RouteSpec) -> Result<Output, CliError> {
    let path = resolve_scene(scene)?;
    let world = load_scene(&path)?;
    let collector = LogCollector::start();
    let mut app = app_with_world(world);
    // The actions behaviours read, as in a game (no unknown-action warnings).
    box_engine::behaviours::add_default_actions(&mut app.world);
    // Rule parts (script rules) collide as they do in the game.
    app.world.insert_resource(box_engine::Game::new().rules());
    let mut asset_problems = Vec::new();
    if let Ok(project) = Project::open(&path)
        && project.assets_dir().is_dir()
    {
        let server = box_engine::asset_server(project.assets_dir())
            .map_err(|e| CliError::new("assets", e.to_string()).file(&path))?;
        app.world.insert_resource(server.clone());
        settle_assets(&mut app.world, ASSET_TIMEOUT);
        asset_problems = problems::asset_ref_problems(&app.world, &server, true, "error");
    }
    // Generated meshes, transforms and the physics world (synced in
    // PostUpdate).
    for _ in 0..2 {
        app.tick(FIXED_DT);
    }
    let report = walk_route(&app.world, spec).map_err(|e| CliError::new("route", e).file(&path))?;
    let problems = problems::merge(asset_problems, collector.finish());
    let (project, file) = locate(&path);
    let mut lines = Vec::new();
    for leg in &report.legs {
        let to = spec.waypoints[leg.to];
        let what = if leg.reached {
            format!("reached in {:.1} s", leg.time)
        } else {
            let at = leg.stuck_at.unwrap_or(leg.end);
            format!("STUCK at {:.1},{:.1},{:.1}", at[0], at[1], at[2])
        };
        lines.push(format!(
            "  leg {} -> ({:.1}, {:.1}, {:.1}): {what}; step up {:.2} m, falls {}, steepest {:.0}°",
            leg.to, to[0], to[1], to[2], leg.max_step_up, leg.falls, leg.steepest_slope
        ));
    }
    let text = format!(
        "{} {}:\n{}{}",
        if report.reached_all {
            "Walked every leg of"
        } else {
            "Could not walk all of"
        },
        path.display(),
        lines.join("\n"),
        problems::summary(&problems),
    );
    let mut json = json!({
        "project": project,
        "file": file,
        "problems": problems,
    });
    let report_json =
        serde_json::to_value(&report).map_err(|e| CliError::new("route", e.to_string()))?;
    if let (Some(obj), Some(r)) = (json.as_object_mut(), report_json.as_object()) {
        for (k, v) in r {
            obj.insert(k.clone(), v.clone());
        }
    }
    Ok(Output::ok(json, text))
}
