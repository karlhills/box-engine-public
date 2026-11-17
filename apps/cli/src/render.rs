//! `box render`: headless render of a scene file.

use std::io::{Cursor, Read};
use std::path::Path;

use box_engine::box_core::edit::resolve_entity;
use box_engine::box_core::math::Aabb;
use box_engine::box_core::prelude::*;
use box_engine::box_render::{Camera, RenderError, Renderer};
use box_engine::project::write_atomic;
use box_engine::scene_glue::{framing_transform, scene_bounds};
use box_engine::{Project, WindowSize, app_with_world, settle_assets};
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

pub(crate) fn render(args: &RenderArgs) -> Result<Output, CliError> {
    check_out(&args.out)?;
    let path = resolve_scene(&args.scene)?;
    let mut world = load_scene(&path)?;
    let (width, height) = args.size;
    let chosen = choose_camera(&mut world, args.camera.as_deref(), &path)?;

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

    let camera_json = match chosen {
        Chosen::Scene(e) => json!({
            "uid": app.world.uid(e).map(|u| u.to_string()),
            "name": app.world.get::<Name>(e).map(|n| n.to_string()),
            "automatic": false,
        }),
        Chosen::Framing => {
            // One update builds meshes and global transforms to measure.
            app.tick(FIXED_DT);
            let bounds = scene_bounds(&app.world)
                .unwrap_or_else(|| Aabb::new(Vec3::splat(-1.0), Vec3::splat(1.0)));
            let camera = Camera::default();
            let transform = framing_transform(&bounds, camera.fov_y, width as f32 / height as f32);
            app.world.spawn((transform, camera));
            json!({ "uid": null, "name": null, "automatic": true })
        }
    };

    let mut image = None;
    for frame in 0..args.frames {
        app.tick(FIXED_DT);
        let result = if frame + 1 == args.frames {
            renderer
                .capture(&mut app.world)
                .map(|img| image = Some(img))
        } else {
            renderer.render(&mut app.world)
        };
        result.map_err(|e| CliError::new("render", format!("frame {frame}: {e}")))?;
    }
    let image = image.ok_or_else(|| CliError::new("render", "no frame was captured"))?;

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

    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| CliError::new("render", format!("could not encode the image: {e}")))?;
    if let Some(parent) = args.out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| CliError::io(parent, e))?;
    }
    write_atomic(&args.out, &png).map_err(|e| CliError::io(&args.out, e))?;

    let problems = problems::merge(asset_problems, collector.finish());
    let (project, file) = locate(&path);
    let json = json!({
        "project": project,
        "file": file,
        "out": args.out.display().to_string(),
        "width": width,
        "height": height,
        "frames": args.frames,
        "camera": camera_json,
        "objects": stats.objects,
        "visible": stats.visible,
        "triangles": stats.triangles,
        "validation_errors": stats.validation_errors,
        "problems": problems,
    });
    let text = format!(
        "Rendered {} to {} ({width}x{height}, {} objects, camera: {}){}",
        path.display(),
        args.out.display(),
        stats.objects,
        camera_json["name"].as_str().unwrap_or("automatic framing"),
        problems::summary(&problems),
    );
    Ok(Output::ok(json, text))
}
