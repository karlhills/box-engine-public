//! Cooking: a project folder into one `game.pak`.
//!
//! - Every asset file is imported once with the engine's importers (the
//!   same ones the player runs); a file that fails to import, a broken
//!   `.meta` or a missing start scene stops the export with the list of
//!   problems.
//! - Only what the game reads is packed: `box.project.json` and `assets/**`
//!   with their `.meta` files. Hidden files and folders (`.box/`, `.DS_Store`,
//!   temporary files), `authoring/`, docs, Cargo files and everything else
//!   outside `assets/` stay behind.
//! - [`MANIFEST_FILE`](box_engine::packed::MANIFEST_FILE) records the name,
//!   version, profile and target.
//!
//! The pak is reproducible: the same project gives the same bytes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_engine::Project;
use box_engine::box_assets::{LoadState, PakWriter, pak};
use box_engine::box_core::asset_ref::AssetRef;
use box_engine::box_core::world::World;
use box_engine::packed::{ExportManifest, MANIFEST_FILE};
use box_engine::project::PROJECT_FILE;
use serde::Serialize;

use crate::{ExportError, Progress};

/// How long imports may take before the cook gives up.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(600);

/// What [`cook`] did.
#[derive(Clone, Debug, Serialize)]
pub struct CookReport {
    /// Files in the pak (the manifest included).
    pub files: usize,
    /// Asset files imported to validate them.
    pub assets_imported: usize,
    /// Bytes of the packed files.
    pub content_bytes: u64,
    /// Size of `game.pak`.
    pub pak_bytes: u64,
    /// xxh3-128 of `game.pak`, hex.
    pub pak_hash: String,
    /// Size of `box.project.json` and `assets/` on disk (hidden files
    /// included): what the pak replaces.
    pub source_bytes: u64,
    /// Milliseconds spent importing (validation).
    pub import_ms: f64,
    /// Milliseconds spent writing the pak.
    pub write_ms: f64,
    pub warnings: Vec<String>,
}

/// Every file below `dir` as paths relative to `base` (`/` separators),
/// skipping hidden names.
fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(base, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(base) {
            let rel: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(rel.join("/"));
        }
    }
    Ok(())
}

/// Total size of the files below `dir` (all of them, hidden included).
fn folder_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => folder_bytes(&e.path()),
            Ok(_) => e.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

/// The files of `project` a cooked game contains, relative to its root.
pub fn packed_files(project: &Project) -> Result<Vec<String>, ExportError> {
    let root = project.root();
    let mut files = vec![PROJECT_FILE.to_string()];
    let assets = project.assets_dir();
    if assets.is_dir() {
        walk(root, &assets, &mut files).map_err(|source| ExportError::Io {
            path: assets.clone(),
            source,
        })?;
    }
    Ok(files)
}

/// Imports every asset of `project` and returns the problems found (empty
/// when the game is good to pack), and how many files were imported.
fn validate(
    project: &Project,
    progress: &mut dyn FnMut(Progress),
) -> Result<(Vec<String>, usize), ExportError> {
    let mut problems = Vec::new();
    let start = project.start_scene_path();
    if !start.is_file() {
        problems.push(format!(
            "{PROJECT_FILE}: start_scene `{}` does not exist",
            project.file().start_scene
        ));
    }
    let assets = project.assets_dir();
    if !assets.is_dir() {
        return Ok((problems, 0));
    }
    // Creates missing .meta files in the project (as the editor does), so
    // asset ids are stable and packed.
    let server = box_engine::asset_server(&assets).map_err(|e| ExportError::Cook {
        problems: vec![format!("assets: {e}")],
    })?;
    for (path, message) in server.broken_files() {
        problems.push(format!("assets/{path}.meta: {message}"));
    }
    let files = server.files();
    for (id, _) in &files {
        let _ = server.load_file(&AssetRef::id(*id));
    }
    let total = files.len().max(1);
    let mut world = World::new();
    let started = Instant::now();
    loop {
        server.update(&mut world);
        let done = files
            .iter()
            .filter(|(id, _)| !matches!(server.file_state(*id), Some(LoadState::Loading)))
            .count();
        progress(Progress::new(
            "import",
            format!("Importing assets ({done}/{})", files.len()),
            0.05 + 0.55 * done as f32 / total as f32,
        ));
        if server.is_idle() {
            break;
        }
        if started.elapsed() > IMPORT_TIMEOUT {
            problems.push(format!(
                "assets did not finish importing in {} s",
                IMPORT_TIMEOUT.as_secs()
            ));
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for (id, path) in &files {
        if let Some(LoadState::Failed(message)) = server.file_state(*id) {
            problems.push(format!("assets/{path}: {message}"));
        }
    }
    Ok((problems, files.len()))
}

/// Imports every asset of `project` (stopping on problems), then writes
/// the project's game files and `manifest` into the pak `out`.
pub fn cook(
    project: &Project,
    manifest: &ExportManifest,
    out: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<CookReport, ExportError> {
    progress(Progress::new("import", "Importing assets", 0.05));
    let import_start = Instant::now();
    let (problems, assets_imported) = validate(project, progress)?;
    let import_ms = import_start.elapsed().as_secs_f64() * 1000.0;
    if !problems.is_empty() {
        return Err(ExportError::Cook { problems });
    }
    progress(Progress::new("pak", "Writing game.pak", 0.62));
    let write_start = Instant::now();
    let files = packed_files(project)?;
    let mut writer = PakWriter::new();
    let root: PathBuf = project.root().to_path_buf();
    for rel in &files {
        writer.add_file(rel, root.join(rel))?;
    }
    let mut manifest_json =
        serde_json::to_vec_pretty(manifest).map_err(|e| ExportError::Bundle(e.to_string()))?;
    manifest_json.push(b'\n');
    writer.add_bytes(MANIFEST_FILE, manifest_json)?;
    let summary = writer.write(out)?;
    let write_ms = write_start.elapsed().as_secs_f64() * 1000.0;
    Ok(CookReport {
        files: summary.entries,
        assets_imported,
        content_bytes: summary.content_bytes,
        pak_bytes: summary.bytes,
        pak_hash: pak::hash_hex(&summary.hash),
        source_bytes: folder_bytes(&project.assets_dir())
            + std::fs::metadata(project.project_file_path()).map_or(0, |m| m.len()),
        import_ms,
        write_ms,
        warnings: Vec::new(),
    })
}
