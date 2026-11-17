//! `box import`: copy a file into a project's `assets/`, give it a `.meta`
//! and import it once (docs/ARCHITECTURE.md section 12).

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use box_engine::box_assets::{AssetServer, LoadState};
use box_engine::box_core::asset_ref::AssetRef;
use box_engine::box_core::world::World;
use serde_json::{Value as Json, json};

use crate::problems::{self, LogCollector};
use crate::{CliError, ImportArgs, Output, open_project};

/// How long the import may take.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(120);

/// The folder under `assets/` a file goes to without `--to`.
fn default_folder(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".glb") || lower.ends_with(".gltf") {
        "models"
    } else if [".png", ".jpg", ".jpeg"].iter().any(|e| lower.ends_with(e)) {
        "textures"
    } else if lower.ends_with(".material.json") {
        "materials"
    } else if lower.ends_with(".wgsl") {
        "shaders"
    } else if lower.ends_with(".scene.json") {
        "scenes"
    } else {
        ""
    }
}

/// `%XX` escapes in a glTF URI.
fn percent_decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&uri[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Relative files a `.gltf` names (buffers and images that are not data
/// URIs), refusing paths that leave its folder.
fn gltf_dependencies(src: &Path) -> Result<Vec<PathBuf>, CliError> {
    let text = std::fs::read(src).map_err(|e| CliError::io(src, e))?;
    let json: Json = serde_json::from_slice(&text)
        .map_err(|e| CliError::new("import_failed", format!("invalid glTF JSON: {e}")).file(src))?;
    let mut out = Vec::new();
    for list in ["buffers", "images"] {
        for item in json[list].as_array().into_iter().flatten() {
            let Some(uri) = item["uri"].as_str() else {
                continue;
            };
            if uri.starts_with("data:") {
                continue;
            }
            let rel = PathBuf::from(percent_decode(uri));
            if !rel
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
            {
                return Err(CliError::new(
                    "import_failed",
                    format!("`{uri}` points outside the glTF's folder; copy the files yourself"),
                )
                .file(src));
            }
            if !out.contains(&rel) {
                out.push(rel);
            }
        }
    }
    Ok(out)
}

fn short_type(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

fn state_json(state: &LoadState) -> (&'static str, Option<String>) {
    match state {
        LoadState::Loading => ("loading", None),
        LoadState::Loaded => ("loaded", None),
        LoadState::Failed(m) => ("failed", Some(m.clone())),
    }
}

/// Formats people try to import that have no importer, with what to do.
fn unsupported_format(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit('.').next()?;
    Some(match ext {
        "fbx" => {
            "FBX is not supported: convert it to glTF (e.g. Blender ▸ File ▸ Export ▸ glTF 2.0) and import the .glb"
        }
        "obj" | "dae" | "3ds" | "blend" | "usd" | "usdz" | "stl" | "ply" => {
            "this model format is not supported: convert it to glTF (e.g. Blender ▸ File ▸ Export ▸ glTF 2.0) and import the .glb"
        }
        "tga" | "bmp" | "tif" | "tiff" | "psd" | "exr" | "hdr" | "webp" | "gif" | "ktx2"
        | "dds" => "this image format is not supported: convert it to PNG or JPEG",
        _ => return None,
    })
}

pub(crate) fn import(args: &ImportArgs) -> Result<Output, CliError> {
    if !args.file.is_file() {
        return Err(CliError::new(
            "not_found",
            format!("{} is not a file", args.file.display()),
        )
        .file(&args.file));
    }
    let file_name = args
        .file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(message) = unsupported_format(&file_name) {
        return Err(CliError::new("unsupported_format", message).file(&args.file));
    }
    let project = open_project(args.project.as_deref())?;
    let collector = LogCollector::start();
    let assets = project.assets_dir();
    std::fs::create_dir_all(&assets).map_err(|e| CliError::io(&assets, e))?;
    let src = std::fs::canonicalize(&args.file).map_err(|e| CliError::io(&args.file, e))?;
    let assets_abs = std::fs::canonicalize(&assets).map_err(|e| CliError::io(&assets, e))?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Already inside assets/: import in place. Otherwise copy it (and a
    // .gltf's buffers and images) into `--to` or the default folder.
    let mut copied = Vec::new();
    let (rel, server) = match src.strip_prefix(&assets_abs) {
        Ok(inside) => {
            let rel = inside.to_string_lossy().replace('\\', "/");
            let server = open_server(&assets)?;
            (rel, server)
        }
        Err(_) => {
            let folder = match &args.to {
                Some(f) => f.trim_matches('/').replace('\\', "/"),
                None => default_folder(&name).to_string(),
            };
            let rel = if folder.is_empty() {
                name.clone()
            } else {
                format!("{folder}/{name}")
            };
            let dest = assets.join(&rel);
            if dest.exists() {
                return Err(CliError::new(
                    "already_exists",
                    format!("assets/{rel} already exists; refusing to overwrite it"),
                )
                .file(&dest)
                .with(
                    "hint",
                    json!(format!(
                        "import it into another folder with --to <folder under assets/>, or rename the file; to update the existing asset, copy the new file over assets/{rel} (hot reload re-imports it)"
                    )),
                ));
            }
            let src_dir = src.parent().unwrap_or(Path::new("."));
            let dest_dir = dest.parent().unwrap_or(&assets).to_path_buf();
            let deps = if name.to_ascii_lowercase().ends_with(".gltf") {
                gltf_dependencies(&src)?
            } else {
                Vec::new()
            };
            for dep in deps {
                let (from, to) = (src_dir.join(&dep), dest_dir.join(&dep));
                if to.exists() {
                    let same = std::fs::read(&from).ok() == std::fs::read(&to).ok();
                    if !same {
                        return Err(CliError::new(
                            "already_exists",
                            format!(
                                "{} already exists with other content; refusing to overwrite it",
                                to.display()
                            ),
                        )
                        .file(&to)
                        .with(
                            "hint",
                            json!("import the model into another folder with --to <folder under assets/>"),
                        ));
                    }
                    continue;
                }
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| CliError::io(parent, e))?;
                }
                std::fs::copy(&from, &to).map_err(|e| CliError::io(&from, e))?;
                copied.push(project.relative(&to));
            }
            // Dependencies are copied first, so the server's scan indexes
            // them (their .meta files) before the main file is imported.
            let server = open_server(&assets)?;
            let bytes = std::fs::read(&src).map_err(|e| CliError::io(&src, e))?;
            server
                .create(&rel, &bytes)
                .map_err(|e| CliError::new("import_failed", e.to_string()).file(&dest))?;
            copied.insert(0, project.relative(&dest));
            (rel, server)
        }
    };

    let id = server.id_of(&rel).ok_or_else(|| {
        CliError::new(
            "no_importer",
            format!("assets/{rel}: no importer is registered for this file type"),
        )
        .file(assets.join(&rel))
    })?;
    server
        .load_file(&AssetRef::id(id))
        .map_err(|e| CliError::new("import_failed", e.to_string()))?;
    let mut world = World::new();
    if !server.update_until_idle(&mut world, IMPORT_TIMEOUT) {
        return Err(CliError::new(
            "timeout",
            format!(
                "assets/{rel} did not finish importing in {} s",
                IMPORT_TIMEOUT.as_secs()
            ),
        ));
    }
    let file_state = server.file_state(id).unwrap_or(LoadState::Loading);
    let (state, error) = state_json(&file_state);
    let importer = server.meta(id).ok().map(|m| m.importer);
    let subs: Vec<Json> = server
        .sub_assets(id)
        .iter()
        .map(|s| {
            let (state, error) = state_json(&s.state);
            json!({
                "label": s.label,
                "type": short_type(s.type_name),
                "state": state,
                "error": error,
            })
        })
        .collect();
    let file = project.relative(&assets.join(&rel));
    let mut text = match &error {
        None => format!("Imported {file} (id {id})"),
        Some(e) if !copied.is_empty() => format!(
            "Copied to {}; import failed: {e}\nThe file was kept: fix it (hot reload re-imports it) or delete it.",
            copied.join(", ")
        ),
        Some(e) => format!("Import of {file} failed: {e}"),
    };
    for s in &subs {
        let label = s["label"]
            .as_str()
            .map_or(String::new(), |l| format!("#{l}"));
        text.push_str(&format!(
            "\n  {rel}{label}: {} ({})",
            s["type"].as_str().unwrap_or("?"),
            s["state"].as_str().unwrap_or("?")
        ));
        if let Some(e) = s["error"].as_str().filter(|e| Some(*e) != error.as_deref()) {
            text.push_str(&format!(": {e}"));
        }
    }
    // The import's own failure is `error`; problems are everything else
    // logged meanwhile (importer warnings).
    let main = error.as_ref().map(|e| {
        json!({ "severity": "error", "code": "asset_import_failed", "asset": rel, "message": e })
    });
    let mut problems = problems::merge(main.into_iter().collect(), collector.finish());
    if error.is_some() {
        problems.remove(0);
    }
    text.push_str(&problems::summary(&problems));
    let json = json!({
        "project": project.root().display().to_string(),
        "file": file,
        "path": rel,
        "id": id.to_string(),
        "importer": importer,
        "state": state,
        "error": error,
        "copied": copied,
        "kept": error.is_some() && !copied.is_empty(),
        "sub_assets": subs,
        "problems": problems,
    });
    Ok(Output {
        json,
        text,
        success: file_state == LoadState::Loaded,
    })
}

fn open_server(assets: &Path) -> Result<AssetServer, CliError> {
    box_engine::asset_server(assets)
        .map_err(|e| CliError::new("assets", e.to_string()).file(assets))
}
