//! Project upgrades: bring every file of a project made with an older
//! engine (or an older version of its game's types) to the current formats,
//! explicitly and after a backup.
//!
//! [`plan`] reads the project and works out, without writing anything, what
//! would change: the project file (`version`, `engine_version`), every scene
//! and prefab, material, animation graph, settings file and `.meta` sidecar
//! under `assets/`, each upgraded on its JSON tree by the format chains and
//! the registry's type steps ([`box_core::migrate`]). Files written by a
//! newer engine or game are listed as refusals, files whose migration fails
//! as errors; either blocks the upgrade. [`apply`] copies every file it is
//! about to change to `.box/backups/<UTC time>/` (same relative paths, plus
//! `upgrade.json` describing the upgrade), then writes the new files
//! atomically, the project file last.
//!
//! Nothing upgrades files silently: loading always migrates in memory only.
//! The editor asks (Upgrade / Open Read-Only / Cancel), `box upgrade
//! [--dry-run]` and a player's `--upgrade [--dry-run]` do it on request.
//!
//! Formats without a version (input action maps, string tables, themes,
//! data tables) are not touched: their readers ignore what they do not know
//! or report it, and they change only additively.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use box_assets::vfs;
use box_core::migrate::{self, MigrateError};
use box_core::reflect::Registry;
use box_core::scene::{self, SceneError};
use serde_json::{Value as Json, json};

use crate::project::{
    ENGINE_VERSION, PROJECT_FILE, PROJECT_VERSION, parse_version, upgrade_project_tree,
    write_atomic,
};

/// Folder of upgrade backups, under the project's `.box/`.
pub const BACKUPS_DIR: &str = "backups";

/// The kind of a file an upgrade reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Project,
    Scene,
    Material,
    AnimationGraph,
    Settings,
    Meta,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Scene => "scene",
            Self::Material => "material",
            Self::AnimationGraph => "animgraph",
            Self::Settings => "settings",
            Self::Meta => "meta",
        }
    }
}

/// One file the upgrade rewrites.
#[derive(Clone, Debug)]
pub struct FileChange {
    /// Relative to the project root, `/`-separated.
    pub path: String,
    pub kind: FileKind,
    /// What changes, one line each.
    pub changes: Vec<String>,
    bytes: Vec<u8>,
}

/// A file the upgrade cannot handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileProblem {
    /// Relative to the project root.
    pub path: String,
    pub message: String,
}

/// What [`plan`] found.
#[derive(Clone, Debug)]
pub struct UpgradePlan {
    root: PathBuf,
    /// The project's name (from its project file).
    pub name: String,
    /// The project file's `engine_version`.
    pub engine_version: String,
    /// Files to rewrite, the project file last.
    pub files: Vec<FileChange>,
    /// Files written by a newer engine or game: the project cannot be
    /// opened for editing by this engine.
    pub newer: Vec<FileProblem>,
    /// Files whose migration failed (a step error, steps that only the
    /// game's code has, or a result that does not load).
    pub errors: Vec<FileProblem>,
}

impl UpgradePlan {
    /// The project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// True when some file is older than this engine.
    pub fn needed(&self) -> bool {
        !self.files.is_empty()
    }

    /// True when the upgrade cannot run (newer files or failed migrations).
    pub fn blocked(&self) -> bool {
        !self.newer.is_empty() || !self.errors.is_empty()
    }

    /// True when the project needs nothing and has no problems.
    pub fn is_current(&self) -> bool {
        !self.needed() && !self.blocked()
    }

    /// A few lines for a prompt or a terminal.
    pub fn summary(&self) -> String {
        let mut out = Vec::new();
        if !self.newer.is_empty() {
            out.push(format!(
                "{} file(s) were written by a newer box-engine or game; update box-engine (or the game) to open this project:",
                self.newer.len()
            ));
            out.extend(
                self.newer
                    .iter()
                    .map(|p| format!("  {}: {}", p.path, p.message)),
            );
        }
        if !self.errors.is_empty() {
            out.push(format!("{} file(s) cannot be upgraded:", self.errors.len()));
            out.extend(
                self.errors
                    .iter()
                    .map(|p| format!("  {}: {}", p.path, p.message)),
            );
        }
        if self.needed() {
            out.push(format!(
                "Upgrading from box-engine {} to {ENGINE_VERSION} changes {} file(s) (backed up to .box/{BACKUPS_DIR}/ first):",
                self.engine_version,
                self.files.len()
            ));
            for f in &self.files {
                out.push(format!("  {}: {}", f.path, f.changes.join("; ")));
            }
        } else if !self.blocked() {
            out.push(format!(
                "The project is up to date (box-engine {ENGINE_VERSION})."
            ));
        }
        out.join("\n")
    }

    /// The plan as JSON (`box upgrade --json`).
    pub fn to_json(&self) -> Json {
        let problems = |list: &[FileProblem]| -> Vec<Json> {
            list.iter()
                .map(|p| json!({ "file": p.path, "message": p.message }))
                .collect()
        };
        json!({
            "project": self.root.display().to_string(),
            "engine_version": self.engine_version,
            "target_engine_version": ENGINE_VERSION,
            "needed": self.needed(),
            "blocked": self.blocked(),
            "files": self.files.iter().map(|f| json!({
                "file": f.path,
                "kind": f.kind.as_str(),
                "changes": f.changes,
            })).collect::<Vec<_>>(),
            "newer": problems(&self.newer),
            "errors": problems(&self.errors),
        })
    }
}

/// What [`apply`] did.
#[derive(Clone, Debug)]
pub struct UpgradeReport {
    /// The backup folder (absolute).
    pub backup: PathBuf,
    /// The files written, relative to the project root.
    pub written: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("the project cannot be upgraded:\n{0}")]
    Blocked(String),
    #[error("{}: invalid project file: {message}", path.display())]
    Project { path: PathBuf, message: String },
    /// A write failed part way; the backup holds every original.
    #[error(
        "writing {file} failed ({source}); {written} file(s) were already upgraded, the originals are in {}",
        backup.display()
    )]
    Partial {
        file: String,
        written: usize,
        backup: PathBuf,
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> UpgradeError + '_ {
    move |source| UpgradeError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Text as the engine writes it: compact numeric arrays (scenes, materials,
/// graphs) or plain pretty JSON, keeping the original's final newline.
fn render(json: &Json, compact: bool, newline: bool) -> Vec<u8> {
    let mut bytes = if compact {
        scene::to_pretty_json(json).unwrap_or_default()
    } else {
        serde_json::to_vec_pretty(json).unwrap_or_default()
    };
    if newline {
        bytes.push(b'\n');
    }
    bytes
}

/// Works out what upgrading the project at `root` (its root folder, or a
/// mounted pak's root) with `registry` (the game's registry, or the
/// engine's) would change. Writes nothing.
pub fn plan(root: &Path, registry: &Arc<Registry>) -> Result<UpgradePlan, UpgradeError> {
    let root = std::path::absolute(root).map_err(io(root))?;
    let project_path = root.join(PROJECT_FILE);
    let text = vfs::read(&project_path).map_err(io(&project_path))?;
    let mut tree: Json = serde_json::from_slice(&text).map_err(|e| UpgradeError::Project {
        path: project_path.clone(),
        message: e.to_string(),
    })?;
    let engine_version = tree["engine_version"].as_str().unwrap_or("").to_string();
    let mut plan = UpgradePlan {
        root: root.clone(),
        name: tree["name"].as_str().unwrap_or_default().to_string(),
        engine_version: engine_version.clone(),
        files: Vec::new(),
        newer: Vec::new(),
        errors: Vec::new(),
    };

    // assets/
    let assets = root.join("assets");
    let mut files = Vec::new();
    if vfs::is_dir(&assets) {
        collect_files(&assets, &mut files).map_err(io(&assets))?;
    }
    files.sort();
    let settings: Vec<String> = registry
        .settings()
        .map(|t| t.name().to_string())
        .chain(
            registry
                .schemas()
                .filter(|s| s.is_settings())
                .map(|s| s.name().to_string()),
        )
        .collect();
    for path in files {
        let rel = relative(&root, &path);
        let Some(kind) = classify(&root, &path, &settings) else {
            continue;
        };
        let Ok(bytes) = vfs::read(&path) else {
            plan.errors.push(FileProblem {
                path: rel,
                message: "could not be read".into(),
            });
            continue;
        };
        // A file that is not valid JSON is not the upgrade's business
        // (`box check` reports it); it is left alone.
        let Ok(mut json) = serde_json::from_slice::<Json>(&bytes) else {
            continue;
        };
        let newline = bytes.ends_with(b"\n");
        match upgrade_file(kind, &path, registry, &mut json) {
            Ok(changes) if changes.is_empty() => {}
            Ok(changes) => {
                let compact = !matches!(kind, FileKind::Settings | FileKind::Meta);
                plan.files.push(FileChange {
                    path: rel,
                    kind,
                    changes,
                    bytes: render(&json, compact, newline),
                });
            }
            Err(Failure::Newer(message)) => plan.newer.push(FileProblem { path: rel, message }),
            Err(Failure::Error(message)) => plan.errors.push(FileProblem { path: rel, message }),
        }
    }

    // The project file, last.
    let project_rel = PROJECT_FILE.to_string();
    let mut changes = Vec::new();
    let format = tree["version"].as_u64().unwrap_or(0);
    if format > u64::from(PROJECT_VERSION) {
        plan.newer.push(FileProblem {
            path: project_rel.clone(),
            message: format!(
                "project format version {format} is newer than this engine supports ({PROJECT_VERSION})"
            ),
        });
    } else {
        match upgrade_project_tree(&mut tree) {
            Ok(steps) if format != u64::from(PROJECT_VERSION) => {
                let mut line = format!("format version {format} -> {PROJECT_VERSION}");
                if !steps.is_empty() {
                    line.push_str(&format!(": {}", steps.join("; ")));
                }
                changes.push(line);
            }
            Ok(_) => {}
            Err(e) => plan.errors.push(FileProblem {
                path: project_rel.clone(),
                message: e.to_string(),
            }),
        }
    }
    match (
        parse_version(&engine_version),
        parse_version(ENGINE_VERSION),
    ) {
        (Some(p), Some(e)) if p > e => plan.newer.push(FileProblem {
            path: project_rel.clone(),
            message: format!(
                "saved by box-engine {engine_version}, newer than this engine ({ENGINE_VERSION})"
            ),
        }),
        (Some(p), Some(e)) if p < e => {
            changes.push(format!(
                "engine_version {engine_version} -> {ENGINE_VERSION}"
            ));
            tree["engine_version"] = json!(ENGINE_VERSION);
        }
        (None, _) => plan.errors.push(FileProblem {
            path: project_rel.clone(),
            message: format!("engine_version `{engine_version}` is not a version like \"0.1.0\""),
        }),
        _ => {}
    }
    if !changes.is_empty() {
        plan.files.push(FileChange {
            path: project_rel,
            kind: FileKind::Project,
            changes,
            bytes: render(&tree, false, text.ends_with(b"\n")),
        });
    }
    Ok(plan)
}

enum Failure {
    Newer(String),
    Error(String),
}

impl From<MigrateError> for Failure {
    fn from(e: MigrateError) -> Self {
        match e {
            MigrateError::Newer { .. } => Failure::Newer(e.to_string()),
            other => Failure::Error(other.to_string()),
        }
    }
}

impl From<SceneError> for Failure {
    fn from(e: SceneError) -> Self {
        match e {
            SceneError::NewerVersion { .. } => Failure::Newer(e.to_string()),
            SceneError::Migrate(m) => (*m).into(),
            SceneError::ComponentMigration { ref source, .. }
                if matches!(**source, MigrateError::Newer { .. }) =>
            {
                Failure::Newer(e.to_string())
            }
            other => Failure::Error(other.to_string()),
        }
    }
}

/// Upgrades one parsed file in place; returns what changed.
fn upgrade_file(
    kind: FileKind,
    path: &Path,
    registry: &Arc<Registry>,
    json: &mut Json,
) -> Result<Vec<String>, Failure> {
    let value_changes = |name: &str, json: &mut Json| -> Result<Vec<String>, Failure> {
        let before = migrate::stored_version(json)?;
        let steps = migrate::upgrade_value(registry, name, json)?;
        if steps.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![format!(
            "`{name}` version {before} -> {}: {}",
            registry.type_version(name),
            steps.join("; ")
        )])
    };
    match kind {
        FileKind::Scene => {
            let report = scene::upgrade_tree(registry, json)?;
            if report.is_empty() {
                return Ok(Vec::new());
            }
            // The result must load with this registry before it is written.
            scene::load_value(json, registry.clone())
                .map_err(|e| Failure::Error(format!("the upgraded scene does not load: {e}")))?;
            Ok(report.lines())
        }
        FileKind::Material => {
            let name = if json.get("shader").is_some() {
                "ShaderMaterial"
            } else {
                "StandardMaterial"
            };
            value_changes(name, json)
        }
        FileKind::AnimationGraph => value_changes("AnimationGraph", json),
        FileKind::Settings => {
            let name = settings_name(path).unwrap_or_default();
            value_changes(&name, json)
        }
        FileKind::Meta => {
            let found = json["version"].as_u64().unwrap_or(0);
            let current = u64::from(box_assets::meta::META_VERSION);
            if found > current {
                return Err(Failure::Newer(format!(
                    ".meta format version {found} is newer than this engine supports ({current})"
                )));
            }
            if found == current || found == 0 {
                return Ok(Vec::new());
            }
            let steps = box_assets::meta::format_chain().upgrade(".meta", found as u32, json)?;
            json["version"] = json!(current);
            let mut line = format!("format version {found} -> {current}");
            if !steps.is_empty() {
                line.push_str(&format!(": {}", steps.join("; ")));
            }
            Ok(vec![line])
        }
        FileKind::Project => Ok(Vec::new()),
    }
}

fn settings_name(path: &Path) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_suffix(".json")
        .map(str::to_string)
}

fn classify(root: &Path, path: &Path, settings: &[String]) -> Option<FileKind> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".meta") {
        return Some(FileKind::Meta);
    }
    if name.ends_with(".scene.json") || name.ends_with(".prefab.json") {
        return Some(FileKind::Scene);
    }
    if name.ends_with(".material.json") {
        return Some(FileKind::Material);
    }
    if name.ends_with(".animgraph.json") {
        return Some(FileKind::AnimationGraph);
    }
    let in_settings =
        path.parent() == Some(&root.join("assets").join(crate::settings::SETTINGS_DIR));
    if in_settings && settings_name(path).is_some_and(|n| settings.contains(&n)) {
        return Some(FileKind::Settings);
    }
    None
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in vfs::read_dir(dir)? {
        if entry.name.starts_with('.') {
            continue;
        }
        if entry.is_dir {
            collect_files(&entry.path, out)?;
        } else {
            out.push(entry.path);
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// `YYYYMMDDTHHMMSSZ` for a Unix time (UTC).
fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// A new backup folder under `<root>/.box/backups/`.
fn backup_dir(root: &Path) -> Result<PathBuf, UpgradeError> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let base = root.join(".box").join(BACKUPS_DIR);
    let stamp = utc_stamp(secs);
    let mut dir = base.join(&stamp);
    let mut n = 2;
    while dir.exists() {
        dir = base.join(format!("{stamp}-{n}"));
        n += 1;
    }
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    Ok(dir)
}

/// Runs a plan: backs up every file it changes, then writes them (the
/// project file last). Refuses a blocked plan; does nothing for a plan
/// that needs nothing (no backup folder either).
pub fn apply(plan: &UpgradePlan) -> Result<UpgradeReport, UpgradeError> {
    if plan.blocked() {
        return Err(UpgradeError::Blocked(plan.summary()));
    }
    if !plan.needed() {
        return Ok(UpgradeReport {
            backup: PathBuf::new(),
            written: Vec::new(),
        });
    }
    let root = &plan.root;
    let backup = backup_dir(root)?;
    for f in &plan.files {
        let from = root.join(&f.path);
        let to = backup.join(&f.path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::copy(&from, &to).map_err(io(&from))?;
    }
    let mut manifest = plan.to_json();
    manifest["backup_of"] = json!(root.display().to_string());
    let manifest_path = backup.join("upgrade.json");
    std::fs::write(&manifest_path, render(&manifest, false, true)).map_err(io(&manifest_path))?;
    let mut written = Vec::new();
    for f in &plan.files {
        let path = root.join(&f.path);
        if let Err(source) = write_atomic(&path, &f.bytes) {
            return Err(UpgradeError::Partial {
                file: f.path.clone(),
                written: written.len(),
                backup,
                source,
            });
        }
        written.push(f.path.clone());
    }
    tracing::info!(
        target: "box_engine::upgrade",
        "upgraded {} file(s) of {}; originals in {}",
        written.len(),
        root.display(),
        backup.display()
    );
    Ok(UpgradeReport { backup, written })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_stamps() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        assert_eq!(utc_stamp(951_782_400), "20000229T000000Z");
        assert_eq!(utc_stamp(1_794_844_800 + 3_723), "20261116T170203Z");
    }
}
