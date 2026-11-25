//! `box render`: headless render of a scene file.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use box_engine::box_core::edit::resolve_entity;
use box_engine::box_core::math::Aabb;
use box_engine::box_core::prelude::*;
use box_engine::box_render::{Camera, RenderError, Renderer};
use box_engine::project::write_atomic;
use box_engine::scene_glue::{content_bounds, framing_transform, scene_bounds};
use box_engine::{
    CameraPose, Project, WindowSize, app_with_world, overview_transform, settle_assets,
};
use serde_json::json;

use crate::problems::{self, LogCollector};
use crate::{CliError, Output, RenderArgs, hierarchy_order, load_scene, locate, resolve_scene};

const FIXED_DT: f32 = 1.0 / 60.0;
/// How long `box render` waits for a scene's assets.
const ASSET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Which camera renders, decided before the first frame.
enum Chosen {
    Scene(Entity),
    Framing,
}

/// Make exactly one camera active (the renderer draws the first active one it
/// finds, in storage order): the requested one, else the first active camera
/// in hierarchy order.
fn choose_camera(
    world: &mut World,
    requested: Option<&str>,
    file: &Path,
) -> Result<Chosen, CliError> {
    let order = hierarchy_order(world);
    let chosen = match requested {
        Some(reference) => {
            let uid = resolve_entity(world, reference).map_err(|e| CliError::edit(e).file(file))?;
            let e = world.entity_by_uid(uid).ok_or_else(|| {
                CliError::new("entity_not_found", format!("no entity {uid}"))
                    .file(file)
                    .entity(uid.to_string())
            })?;
            if !world.has::<Camera>(e) {
                return Err(CliError::new(
                    "not_a_camera",
                    format!("entity `{reference}` has no Camera component"),
                )
                .file(file)
                .entity(uid.to_string()));
            }
            Some(e)
        }
        None => order
            .iter()
            .copied()
            .find(|&e| world.get::<Camera>(e).is_some_and(|c| c.active)),
    };
    for (e, camera) in world.query_mut::<(Entity, &mut Camera)>() {
        camera.active = Some(e) == chosen;
    }
    Ok(chosen.map_or(Chosen::Framing, Chosen::Scene))
}

/// Refuse an output path that is not a PNG, or that would replace a file
/// that is not one (a scene, a source file, a directory).
fn check_out(out: &Path) -> Result<(), CliError> {
    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let is_png = out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err(
            CliError::new("usage", format!("--out {} must end in .png", out.display())).file(out),
        );
    }
    let mut existing = match std::fs::File::open(out) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(CliError::io(out, e)),
    };
    let mut signature = [0u8; 8];
    if existing.read_exact(&mut signature).is_err() || signature != PNG_SIGNATURE {
        return Err(CliError::new(
            "already_exists",
            format!(
                "{} exists and is not a PNG image; refusing to overwrite it",
                out.display()
            ),
        )
        .file(out));
    }
    Ok(())
}

/// What one image is rendered from.
enum View {
    /// A camera entity of the scene.
    Scene(Entity),
    /// An automatic camera framing the scene.
    Framing,
    /// An automatic camera over the scene, from above at an angle.
    Overview,
    /// A temporary camera at a pose.
    Pose(CameraPose),
}

/// One image to render: its view and output path (`name` for `--views`).
struct Shot {
    name: Option<String>,
    view: View,
    out: PathBuf,
}

/// The shots of a `--views` file, written next to `out` as
/// `<stem>_<name>.png`.
fn views_shots(
    world: &mut World,
    file: &Path,
    out: &Path,
    scene: &Path,
) -> Result<Vec<Shot>, CliError> {
    let text = std::fs::read_to_string(file).map_err(|e| CliError::io(file, e))?;
    let list: Vec<serde_json::Value> = serde_json::from_str(&text).map_err(|e| {
        CliError::new(
            "usage",
            format!("--views {}: not a JSON list: {e}", file.display()),
        )
        .file(file)
    })?;
    if list.is_empty() {
        return Err(CliError::new("usage", "--views lists no views").file(file));
    }
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "view".into());
    let mut shots = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for (i, item) in list.iter().enumerate() {
        let bad = |message: String| {
            CliError::new("usage", format!("--views item {i}: {message}")).file(file)
        };
        let name = item
            .get("name")
            .and_then(|n| n.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| (i + 1).to_string());
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(bad(format!(
                "name `{name}` may only use letters, digits, - and _"
            )));
        }
        if !names.insert(name.clone()) {
            return Err(bad(format!("name `{name}` is used twice")));
        }
        let view = if let Some(camera) = item.get("camera") {
            let reference = camera
                .as_str()
                .ok_or_else(|| bad("camera must be a uid or a name".into()))?;
            match choose_camera(world, Some(reference), scene)? {
                Chosen::Scene(e) => View::Scene(e),
                Chosen::Framing => View::Framing,
            }
        } else if item.get("overview").and_then(|o| o.as_bool()) == Some(true) {
            View::Overview
        } else if item.get("eye").is_some() {
            let pose: CameraPose = serde_json::from_value(item.clone())
                .map_err(|e| bad(format!("expected eye, look_at and optional fov: {e}")))?;
            pose.transform().map_err(&bad)?;
            pose.camera().map_err(&bad)?;
            View::Pose(pose)
        } else {
            return Err(bad(
                "give eye and look_at, \"overview\": true, or a camera".into()
            ));
        };
        let out = out.with_file_name(format!("{stem}_{name}.png"));
        check_out(&out)?;
        shots.push(Shot {
            name: Some(name),
            view,
            out,
        });
    }
    Ok(shots)
}

/// Makes `view` the only active camera: a scene camera, or the temporary
/// camera (spawned on first use) placed for it. `bounds` frames the
/// automatic views.
fn activate(
    world: &mut World,
    view: &View,
    temp: &mut Option<Entity>,
    bounds: &Aabb,
    fov: Option<f32>,
    aspect: f32,
) -> Result<serde_json::Value, CliError> {
    let scene_camera = match view {
        View::Scene(e) => Some(*e),
        _ => None,
    };
    for (e, camera) in world.query_mut::<(Entity, &mut Camera)>() {
        camera.active = Some(e) == scene_camera;
    }
    if let Some(e) = scene_camera {
        return Ok(json!({
            "uid": world.uid(e).map(|u| u.to_string()),
            "name": world.get::<Name>(e).map(|n| n.to_string()),
            "automatic": false,
        }));
    }
    let auto_pose = CameraPose {
        fov,
        ..CameraPose::new(Vec3::ZERO, Vec3::NEG_Z)
    };
    let camera = match view {
        View::Pose(pose) => pose.camera(),
        _ => auto_pose.camera(),
    }
    .map_err(|e| CliError::new("usage", e))?;
    let (transform, json) = match view {
        View::Pose(pose) => (
            pose.transform().map_err(|e| CliError::new("usage", e))?,
            json!({ "uid": null, "name": null, "automatic": false, "pose": pose }),
        ),
        View::Overview => (
            overview_transform(bounds, camera.fov_y, aspect),
            json!({ "uid": null, "name": null, "automatic": true, "overview": true }),
        ),
        _ => (
            framing_transform(bounds, camera.fov_y, aspect),
            json!({ "uid": null, "name": null, "automatic": true }),
        ),
    };
    match *temp {
        Some(e) => {
            let _ = world.insert(e, (transform, camera));
        }
        None => *temp = Some(world.spawn((transform, camera))),
    }
    Ok(json)
}

pub(crate) fn render(args: &RenderArgs) -> Result<Output, CliError> {
    check_out(&args.out)?;
    let path = resolve_scene(&args.scene)?;
    let mut world = load_scene(&path)?;
    let (width, height) = args.size;
    let aspect = width as f32 / height as f32;
    let shots = if let Some(file) = &args.views {
        views_shots(&mut world, file, &args.out, &path)?
    } else {
        let view = if let (Some(eye), Some(look_at)) = (args.eye, args.look_at) {
            let pose = CameraPose {
                fov: args.fov,
                ..CameraPose::new(eye, look_at)
            };
            pose.transform().map_err(|e| CliError::new("usage", e))?;
            pose.camera().map_err(|e| CliError::new("usage", e))?;
            View::Pose(pose)
        } else if args.overview {
            View::Overview
        } else {
            match choose_camera(&mut world, args.camera.as_deref(), &path)? {
                Chosen::Scene(e) => View::Scene(e),
                Chosen::Framing => View::Framing,
            }
        };
        vec![Shot {
            name: None,
            view,
            out: args.out.clone(),
        }]
    };

    let mut renderer = Renderer::new_headless(width, height).map_err(|e| match e {
        RenderError::NoAdapter => {
            CliError::new("no_gpu", "no GPU adapter available for headless rendering")
        }
        other => CliError::new(
            "render",
            format!("could not create a headless renderer: {other}"),
        ),
    })?;

    let collector = LogCollector::start();
    let mut app = app_with_world(world);
    // The actions behaviours read, as in a game (no unknown-action warnings).
    box_engine::behaviours::add_default_actions(&mut app.world);
    // The engine's rules (script rules), so a data project's rule parts
    // render as they do in the editor and the player.
    app.world.insert_resource(box_engine::Game::new().rules());
    app.world.insert_resource(WindowSize {
        width,
        height,
        scale_factor_milli: 1000,
    });
    // A scene inside a project renders with its assets (models, prefabs,
    // textures, shaders), all loaded before the first frame.
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

    // Automatic views frame the scene's bounds: one update builds meshes
    // and global transforms to measure.
    let automatic = shots
        .iter()
        .any(|s| matches!(s.view, View::Framing | View::Overview));
    let fallback = Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0));
    let (bounds, content) = if automatic {
        app.tick(FIXED_DT);
        (
            scene_bounds(&app.world).unwrap_or(fallback),
            content_bounds(&app.world).unwrap_or(fallback),
        )
    } else {
        (fallback, fallback)
    };

    let mut temp = None;
    let mut rendered = Vec::new();
    for (i, shot) in shots.iter().enumerate() {
        let framed = if matches!(shot.view, View::Overview) {
            &content
        } else {
            &bounds
        };
        let camera_json = activate(
            &mut app.world,
            &shot.view,
            &mut temp,
            framed,
            args.fov,
            aspect,
        )?;
        // The first view simulates --frames; later ones only settle the
        // camera move (transforms, shadows) before their capture.
        let frames = if i == 0 { args.frames } else { 2 };
        let mut image = None;
        for frame in 0..frames {
            app.tick(FIXED_DT);
            if args.nav && frame + 1 == frames {
                draw_nav(&mut app.world);
            }
            let result = if frame + 1 == frames {
                renderer
                    .capture(&mut app.world)
                    .map(|img| image = Some(img))
            } else {
                renderer.render(&mut app.world)
            };
            result.map_err(|e| CliError::new("render", format!("frame {frame}: {e}")))?;
        }
        let image = image.ok_or_else(|| CliError::new("render", "no frame was captured"))?;
        rendered.push((shot, image, camera_json));
    }

    // An image rendered with validation errors is not trustworthy: report
    // the failure and write nothing.
    let stats = renderer.stats();
    if stats.validation_errors > 0 {
        return Err(CliError::new(
            "gpu_validation",
            format!(
                "{} GPU validation error(s) while rendering; no image was written",
                stats.validation_errors
            ),
        )
        .file(&path));
    }

    let mut views = Vec::new();
    for (shot, image, camera_json) in &rendered {
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| CliError::new("render", format!("could not encode the image: {e}")))?;
        if let Some(parent) = shot.out.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| CliError::io(parent, e))?;
        }
        write_atomic(&shot.out, &png).map_err(|e| CliError::io(&shot.out, e))?;
        views.push(json!({
            "name": shot.name,
            "out": shot.out.display().to_string(),
            "camera": camera_json,
        }));
    }

    let problems = problems::merge(asset_problems, collector.finish());
    let (project, file) = locate(&path);
    let first = &views[0];
    let mut json = json!({
        "project": project,
        "file": file,
        "out": first["out"],
        "width": width,
        "height": height,
        "frames": args.frames,
        "camera": first["camera"],
        "objects": stats.objects,
        "visible": stats.visible,
        "triangles": stats.triangles,
        "validation_errors": stats.validation_errors,
        "problems": problems,
    });
    if args.views.is_some() {
        json["views"] = json!(views);
    }
    let camera_text = |c: &serde_json::Value| {
        if let Some(name) = c["name"].as_str() {
            name.to_owned()
        } else if c.get("pose").is_some() {
            "pose".to_owned()
        } else if c.get("overview").is_some() {
            "overview".to_owned()
        } else {
            "automatic framing".to_owned()
        }
    };
    let text = if args.views.is_some() {
        let list: Vec<String> = views
            .iter()
            .map(|v| {
                format!(
                    "  {} ({})",
                    v["out"].as_str().unwrap_or(""),
                    camera_text(&v["camera"])
                )
            })
            .collect();
        format!(
            "Rendered {} views of {} ({width}x{height}):\n{}{}",
            views.len(),
            path.display(),
            list.join("\n"),
            problems::summary(&problems),
        )
    } else {
        format!(
            "Rendered {} to {} ({width}x{height}, {} objects, camera: {}){}",
            path.display(),
            args.out.display(),
            stats.objects,
            camera_text(&first["camera"]),
            problems::summary(&problems),
        )
    };
    Ok(Output::ok(json, text))
}

/// `--nav`: the navmesh as it is now, drawn into this frame's debug lines.
fn draw_nav(world: &mut box_engine::box_core::world::World) {
    box_engine::nav_glue::settle_navigation(world);
    let mut dd = world
        .remove_resource::<box_engine::box_render::DebugDraw>()
        .unwrap_or_default();
    if let Some(mesh) = box_engine::nav_glue::nav_mesh(world) {
        box_engine::nav_glue::draw_navmesh(mesh, &mut dd);
    }
    world.insert_resource(dd);
}
