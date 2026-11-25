//! `box nav`: the navmesh path between two points of a scene
//! (`box_engine::nav_glue`, docs/NAVIGATION.md).

use std::path::Path;

use box_engine::box_core::glam::Vec3;
use box_engine::nav_glue::{Navigation, settle_navigation};
use box_engine::{Project, app_with_world, settle_assets};
use serde_json::json;

use crate::problems::{self, LogCollector};
use crate::{CliError, NavArgs, Output, load_scene, locate, resolve_scene};

const FIXED_DT: f32 = 1.0 / 60.0;
const ASSET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// The scene at `path` loaded with its assets, a couple of frames run, and
/// its navmesh built; with the asset problems found.
pub(crate) fn settled_app(
    path: &Path,
) -> Result<(box_engine::box_core::app::App, Vec<serde_json::Value>), CliError> {
    let world = load_scene(path)?;
    let mut app = app_with_world(world);
    app.world.insert_resource(box_engine::Game::new().rules());
    // The actions behaviours read, as in a game (no unknown-action warnings).
    box_engine::behaviours::add_default_actions(&mut app.world);
    let mut asset_problems = Vec::new();
    if let Ok(project) = Project::open(path)
        && project.assets_dir().is_dir()
    {
        let server = box_engine::asset_server(project.assets_dir())
            .map_err(|e| CliError::new("assets", e.to_string()).file(path))?;
        app.world.insert_resource(server.clone());
        settle_assets(&mut app.world, ASSET_TIMEOUT);
        asset_problems = problems::asset_ref_problems(&app.world, &server, true, "error");
    }
    // Generated meshes, transforms and the colliders (synced in PostUpdate).
    for _ in 0..2 {
        app.tick(FIXED_DT);
    }
    settle_navigation(&mut app.world);
    Ok((app, asset_problems))
}

pub(crate) fn nav(args: &NavArgs) -> Result<Output, CliError> {
    nav_between(&args.scene, args.from, args.to)
}

/// The path from `from` to `to` (feet positions) in the scene `scene` (a
/// file or a project directory).
pub(crate) fn nav_between(scene: &Path, from: Vec3, to: Vec3) -> Result<Output, CliError> {
    let path = resolve_scene(scene)?;
    let collector = LogCollector::start();
    let (app, asset_problems) = settled_app(&path)?;
    let nav = app.world.resource::<Navigation>();
    let mesh = nav.mesh();
    let polys = mesh.poly_count();
    let tiles = mesh.tiles().count();
    let result = mesh.find_path(from, to);
    let closest = mesh.closest_point(to);
    let problems = problems::merge(asset_problems, collector.finish());
    let (project, file) = locate(&path);
    let status = match result.status {
        box_engine::nav_glue::box_nav::PathStatus::Complete => "complete",
        box_engine::nav_glue::box_nav::PathStatus::Partial => "unreachable",
        box_engine::nav_glue::box_nav::PathStatus::NoStart => "no_start",
    };
    let fmt = |p: Vec3| format!("({:.1}, {:.1}, {:.1})", p.x, p.y, p.z);
    let text = match result.status {
        box_engine::nav_glue::box_nav::PathStatus::Complete => format!(
            "Path from {} to {} in {}: {:.1} m, {} corners\n  {}{}",
            fmt(from),
            fmt(to),
            path.display(),
            result.length(),
            result.points.len(),
            result
                .points
                .iter()
                .map(|p| fmt(*p))
                .collect::<Vec<_>>()
                .join(" -> "),
            problems::summary(&problems),
        ),
        box_engine::nav_glue::box_nav::PathStatus::Partial => format!(
            "{} cannot be reached from {} in {}; the nearest reachable point is {}{}",
            fmt(to),
            fmt(from),
            path.display(),
            result.points.last().map_or("none".into(), |p| fmt(*p)),
            problems::summary(&problems),
        ),
        box_engine::nav_glue::box_nav::PathStatus::NoStart => format!(
            "{} is not on the navmesh of {} ({} polygons in {} tiles): no walkable ground within 2 m sideways and 3 m up or down{}",
            fmt(from),
            path.display(),
            polys,
            tiles,
            problems::summary(&problems),
        ),
    };
    let points: Vec<[f32; 3]> = result.points.iter().map(|p| p.to_array()).collect();
    let json = json!({
        "project": project,
        "file": file,
        "status": status,
        "length": result.length(),
        "points": points,
        "nearest_to_goal": closest.map(|p| p.to_array()),
        "polygons": polys,
        "tiles": tiles,
        "problems": problems,
    });
    Ok(Output::ok(json, text))
}
