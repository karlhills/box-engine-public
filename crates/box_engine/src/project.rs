//! Projects on disk: `box.project.json`, the `assets/` tree and `.box/`.
//!
//! ```json
//! {
//!   "version": 1,
//!   "name": "My Game",
//!   "engine_version": "0.1.0",
//!   "start_scene": "assets/scenes/main.scene.json"
//! }
//! ```
//!
//! Paths in the project file are relative to the project root and use `/`.
//! A project written by a newer engine, or with a newer project format, is
//! refused rather than half-read.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use box_core::color::Color;
use box_core::identity::Uid;
use box_core::primitive::{Primitive, Shape};
use box_core::scene::{self, SceneError};
use box_core::transform::Transform;
use box_core::world::World;
use box_render::{Camera, DirectionalLight, Environment, StandardMaterial};
use glam::{UVec2, Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::scene_glue::shared_engine_registry;

/// File name of the project file at the project root.
pub const PROJECT_FILE: &str = "box.project.json";
/// The project file format version this engine writes and reads.
pub const PROJECT_VERSION: u32 = 1;
/// The engine version written into new projects (the box_engine crate version).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The start scene of a new project, relative to the project root.
pub const DEFAULT_START_SCENE: &str = "assets/scenes/main.scene.json";
/// Extension of scene files.
pub const SCENE_EXTENSION: &str = ".scene.json";

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}", not_found_text(.0))]
    NotFound(PathBuf),
    #[error("{}: already contains a project ({})", .0.display(), PROJECT_FILE)]
    AlreadyExists(PathBuf),
    #[error("{}: invalid project file: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    #[error(
        "{}: project format version {found} is newer than this engine supports ({supported}); update box-engine",
        path.display()
    )]
    NewerFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error(
        "{}: project was saved by box-engine {project}, newer than this engine ({engine}); update box-engine",
        path.display()
    )]
    NewerEngine {
        path: PathBuf,
        project: String,
        engine: &'static str,
    },
    #[error("project name must not be empty")]
    EmptyName,
    #[error(
        "{}: `{relative}` must be a relative path to a file inside the project (no leading `/`, no drive prefix, no `..` that leaves the project)",
        path.display()
    )]
    OutsideProject { path: PathBuf, relative: String },
    // Boxed: with Windows' larger `PathBuf` an inline `SceneError` makes
    // every `Result<_, ProjectError>` trip clippy's `result_large_err`.
    #[error("{}: {source}", path.display())]
    Scene {
        path: PathBuf,
        source: Box<SceneError>,
    },
}

impl ProjectError {
    /// The file the error is about, if any.
    pub fn path(&self) -> Option<&Path> {
        match self {
            ProjectError::Io { path, .. }
            | ProjectError::Parse { path, .. }
            | ProjectError::NewerFormat { path, .. }
            | ProjectError::NewerEngine { path, .. }
            | ProjectError::OutsideProject { path, .. }
            | ProjectError::Scene { path, .. } => Some(path),
            ProjectError::NotFound(p) | ProjectError::AlreadyExists(p) => Some(p),
            ProjectError::EmptyName => None,
        }
    }
}

/// [`ProjectError::NotFound`]: a folder that does not exist (a typo, a
/// moved or renamed project) is said so; otherwise where to look.
fn not_found_text(path: &Path) -> String {
    if !path.exists() {
        format!(
            "{} does not exist; check the path (was the project moved or renamed?), or create a project there with `box new {}`",
            path.display(),
            path.display()
        )
    } else {
        format!(
            "no {PROJECT_FILE} in {} or any parent directory; open the folder that holds {PROJECT_FILE}, or create a project with `box new <dir>`",
            path.display()
        )
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ProjectError + '_ {
    move |source| ProjectError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// `path` without `.` segments and with `..` segments resolved lexically
/// (symlinks are not followed; `..` at the filesystem root is dropped).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` made absolute and normalised, without touching the filesystem.
fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    Ok(normalize(&std::path::absolute(path)?))
}

/// `path` with symlinks resolved in its longest existing prefix, so a file
/// that does not exist yet can still be compared with a canonical root.
fn canonicalize_existing(path: &Path) -> Option<PathBuf> {
    let existing = path.ancestors().find(|a| a.exists())?;
    let rest = path.strip_prefix(existing).ok()?;
    Some(existing.canonicalize().ok()?.join(rest))
}

/// Whether a path from a project file names something below the project root.
/// Lexical, so it works for files that do not exist and gives the same answer
/// on every platform: `/` and `\` both separate, a leading separator or a
/// drive prefix (`C:`) is absolute, and `..` may not climb out of the root.
fn is_inside_project(relative: &str) -> bool {
    let bytes = relative.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if drive || relative.starts_with(['/', '\\']) {
        return false;
    }
    let mut depth = 0usize;
    for segment in relative.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => match depth.checked_sub(1) {
                Some(d) => depth = d,
                None => return false,
            },
            _ => depth += 1,
        }
    }
    // Depth 0 is the root itself, not a file in it.
    depth > 0
}

/// Write `path` through a temporary file in the same directory that is
/// flushed to disk and then renamed into place, so a crash or a failed write
/// never leaves a partial file. The temporary file is removed on failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

fn write_atomic_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = std::fs::File::create(&tmp)
        .and_then(|mut file| {
            write(&mut file)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The contents of `box.project.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub version: u32,
    pub name: String,
    pub engine_version: String,
    /// Relative to the project root.
    pub start_scene: String,
    /// How exports name and identify the game (`box export`, File ▸
    /// Export…). Left out of the file while empty.
    #[serde(default, skip_serializing_if = "ExportSettings::is_empty")]
    pub export: ExportSettings,
}

/// `"export"` in `box.project.json`: the exported application's identity
/// and startup splash. Every field is optional; `box_export` documents the
/// defaults (the project name, `com.example.<name>`, `1.0.0`, the engine's
/// icon, the splash on).
///
/// ```json
/// "export": { "app_name": "Cinder Keep", "bundle_id": "com.example.cinder-keep",
///             "version": "1.2.0", "icon": "assets/icon.png", "splash": false }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportSettings {
    /// The application's display name (default: the project name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    /// Reverse-DNS identifier: the macOS bundle id and the save folder name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// Version shown by the operating system: up to three numbers (`1.2.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// A square PNG (1024 px is best), relative to the project root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Show the "made with box engine" startup splash (default: on;
    /// `"splash": false` turns it off). See `box_engine::splash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub splash: Option<bool>,
}

impl ExportSettings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether exports show the startup splash (`splash` absent or true).
    pub fn splash_enabled(&self) -> bool {
        self.splash != Some(false)
    }
}

/// An open project.
#[derive(Clone, Debug)]
pub struct Project {
    root: PathBuf,
    file: ProjectFile,
}

impl Project {
    /// Create a project in `dir` (created if missing): `box.project.json`, a
    /// starter scene at [`DEFAULT_START_SCENE`] and a `.gitignore` ignoring
    /// `.box/`. Refuses a directory that already holds a project file or a
    /// start scene; other existing files are left alone. If it fails, the
    /// files and directories it created are removed again, so it can be
    /// retried.
    pub fn create(dir: impl AsRef<Path>, name: &str) -> Result<Project, ProjectError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProjectError::EmptyName);
        }
        let root = absolute(dir.as_ref()).map_err(io(dir.as_ref()))?;
        if root.join(PROJECT_FILE).exists() {
            return Err(ProjectError::AlreadyExists(root));
        }
        let scene_path = root.join(DEFAULT_START_SCENE);
        if scene_path.exists() {
            return Err(ProjectError::AlreadyExists(scene_path));
        }
        let file = ProjectFile {
            version: PROJECT_VERSION,
            name: name.to_string(),
            engine_version: ENGINE_VERSION.to_string(),
            start_scene: DEFAULT_START_SCENE.to_string(),
            export: ExportSettings::default(),
        };
        // The directories that do not exist yet, innermost first.
        let scenes_dir = root.join("assets/scenes");
        let new_dirs: Vec<PathBuf> = scenes_dir
            .ancestors()
            .take_while(|d| !d.exists())
            .map(Path::to_path_buf)
            .collect();
        let mut new_files = Vec::new();
        if let Err(e) = write_new_project(&root, &file, &mut new_files) {
            for path in &new_files {
                let _ = std::fs::remove_file(path);
            }
            for dir in &new_dirs {
                // Fails, as it should, when something else is in there.
                let _ = std::fs::remove_dir(dir);
            }
            return Err(e);
        }
        Ok(Project { root, file })
    }

    /// Open the project whose root is `path`, or which contains `path` (the
    /// nearest ancestor directory with a `box.project.json`). `path` may also
    /// be the project file itself.
    pub fn open(path: impl AsRef<Path>) -> Result<Project, ProjectError> {
        let path = path.as_ref();
        let root = Self::find_root(path).ok_or_else(|| ProjectError::NotFound(path.into()))?;
        let file_path = root.join(PROJECT_FILE);
        let text = box_assets::vfs::read_to_string(&file_path).map_err(io(&file_path))?;
        let file = parse_project_file(&file_path, &text)?;
        Ok(Project { root, file })
    }

    /// The nearest directory at or above `path` holding a `box.project.json`.
    /// Inside a mounted pak ([`box_assets::vfs`], an exported game) the
    /// search stays inside the pak, whose root is the pak file's path.
    pub fn find_root(path: &Path) -> Option<PathBuf> {
        use box_assets::vfs;
        if let Some(mount) = vfs::mount_root(path) {
            let start = absolute(path).ok()?;
            return start
                .ancestors()
                .take_while(|d| d.starts_with(&mount))
                .find(|d| vfs::is_file(d.join(PROJECT_FILE)))
                .map(Path::to_path_buf);
        }
        let start = if path.is_file() { path.parent()? } else { path };
        let start = absolute(start).ok()?;
        start
            .ancestors()
            .find(|d| d.join(PROJECT_FILE).is_file())
            .map(Path::to_path_buf)
    }

    /// True when the project is read from a mounted pak (an exported game):
    /// nothing in it can be written.
    pub fn is_packed(&self) -> bool {
        box_assets::vfs::is_mounted(&self.root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> &str {
        &self.file.name
    }

    pub fn file(&self) -> &ProjectFile {
        &self.file
    }

    /// Replaces the project file's export settings and writes
    /// `box.project.json` (atomically; other fields are kept as read).
    pub fn set_export_settings(&mut self, export: ExportSettings) -> Result<(), ProjectError> {
        let path = self.project_file_path();
        let mut file = self.file.clone();
        file.export = export;
        let mut text = serde_json::to_string_pretty(&file).map_err(|e| ProjectError::Parse {
            path: path.clone(),
            message: e.to_string(),
        })?;
        text.push('\n');
        write_atomic(&path, text.as_bytes()).map_err(io(&path))?;
        self.file = file;
        Ok(())
    }

    pub fn project_file_path(&self) -> PathBuf {
        self.root.join(PROJECT_FILE)
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    /// `.box/`: local editor state and caches, ignored by version control.
    pub fn local_dir(&self) -> PathBuf {
        self.root.join(".box")
    }

    pub fn start_scene_path(&self) -> PathBuf {
        self.resolve(&self.file.start_scene)
    }

    /// A path from the project file (relative, `/`-separated) as a filesystem
    /// path. A path that would leave the project resolves to the project root
    /// itself, never to a file outside it; [`Project::try_resolve`] reports
    /// that as an error instead.
    pub fn resolve(&self, relative: &str) -> PathBuf {
        self.try_resolve(relative)
            .unwrap_or_else(|_| self.root.clone())
    }

    /// [`Project::resolve`], refusing absolute paths and paths that leave the
    /// project root (checked lexically; the file need not exist).
    pub fn try_resolve(&self, relative: &str) -> Result<PathBuf, ProjectError> {
        if !is_inside_project(relative) {
            return Err(ProjectError::OutsideProject {
                path: self.project_file_path(),
                relative: relative.to_string(),
            });
        }
        Ok(normalize(&self.root.join(relative)))
    }

    /// `path` relative to the project root with `/` separators, when it is
    /// inside the project; otherwise `path` as given. `..` segments are
    /// resolved first, and a root or path reached through a symlink still
    /// counts as inside (that fallback reads the filesystem; a path that is
    /// lexically inside the root costs no system call).
    pub fn relative(&self, path: &Path) -> String {
        let abs = absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let inside = match abs.strip_prefix(&self.root) {
            Ok(rel) => Some(rel.to_path_buf()),
            Err(_) => self.root.canonicalize().ok().and_then(|root| {
                let rel = canonicalize_existing(&abs)?;
                rel.strip_prefix(root).ok().map(Path::to_path_buf)
            }),
        };
        match inside {
            Some(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
            None => path.display().to_string(),
        }
    }

    /// Every `*.scene.json` under `assets/`, sorted by path.
    pub fn scene_files(&self) -> Result<Vec<PathBuf>, ProjectError> {
        let mut out = Vec::new();
        let assets = self.assets_dir();
        if box_assets::vfs::is_dir(&assets) {
            collect_scenes(&assets, &mut out)?;
        }
        out.sort();
        Ok(out)
    }
}

/// Write a new project's files into `root`, the project file last and
/// atomically, recording each created file in `created` so a failure can be
/// undone.
fn write_new_project(
    root: &Path,
    file: &ProjectFile,
    created: &mut Vec<PathBuf>,
) -> Result<(), ProjectError> {
    let scene_path = root.join(DEFAULT_START_SCENE);
    let project_path = root.join(PROJECT_FILE);
    let scene = scene::save(&starter_scene()).map_err(|source| ProjectError::Scene {
        path: scene_path.clone(),
        source: Box::new(source),
    })?;
    let mut text = serde_json::to_string_pretty(file).map_err(|e| ProjectError::Parse {
        path: project_path.clone(),
        message: e.to_string(),
    })?;
    text.push('\n');

    let scenes_dir = root.join("assets/scenes");
    std::fs::create_dir_all(&scenes_dir).map_err(io(&scenes_dir))?;
    let gitignore = root.join(".gitignore");
    let mut files = vec![(scene_path, scene)];
    if !gitignore.exists() {
        files.push((gitignore, b".box/\n".to_vec()));
    }
    files.push((project_path, text.into_bytes()));
    for (path, bytes) in files {
        write_atomic(&path, &bytes).map_err(io(&path))?;
        created.push(path);
    }
    Ok(())
}

fn collect_scenes(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ProjectError> {
    for entry in box_assets::vfs::read_dir(dir).map_err(io(dir))? {
        if entry.is_dir {
            collect_scenes(&entry.path, out)?;
        } else if entry.name.ends_with(SCENE_EXTENSION) {
            out.push(entry.path);
        }
    }
    Ok(())
}

/// Parse and check a project file's text.
pub fn parse_project_file(path: &Path, text: &str) -> Result<ProjectFile, ProjectError> {
    let tree: serde_json::Value = serde_json::from_str(text).map_err(|e| ProjectError::Parse {
        path: path.into(),
        message: e.to_string(),
    })?;
    // Check the version before the shape, so a newer file is reported as such
    // even when it has fields this engine does not know.
    let version = tree
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| ProjectError::Parse {
            path: path.into(),
            message: "missing or non-numeric \"version\"".into(),
        })?;
    if version > u64::from(PROJECT_VERSION) {
        return Err(ProjectError::NewerFormat {
            path: path.into(),
            found: u32::try_from(version).unwrap_or(u32::MAX),
            supported: PROJECT_VERSION,
        });
    }
    let mut tree = tree;
    upgrade_project_tree(&mut tree).map_err(|e| ProjectError::Parse {
        path: path.into(),
        message: e.to_string(),
    })?;
    let file: ProjectFile = serde_json::from_value(tree).map_err(|e| ProjectError::Parse {
        path: path.into(),
        message: e.to_string(),
    })?;
    if !is_inside_project(&file.start_scene) {
        return Err(ProjectError::OutsideProject {
            path: path.into(),
            relative: file.start_scene,
        });
    }
    let parse = |v: &str| parse_version(v);
    match (parse(&file.engine_version), parse(ENGINE_VERSION)) {
        (Some(p), Some(e)) if p > e => Err(ProjectError::NewerEngine {
            path: path.into(),
            project: file.engine_version.clone(),
            engine: ENGINE_VERSION,
        }),
        (None, _) => Err(ProjectError::Parse {
            path: path.into(),
            message: format!(
                "engine_version `{}` is not a version like \"0.1.0\"",
                file.engine_version
            ),
        }),
        _ => Ok(file),
    }
}

/// The project file's migration chain (step *i* upgrades `"version"` *i + 1*
/// to *i + 2*). Empty: the project file format has not changed yet.
pub fn format_chain() -> box_core::migrate::Chain {
    box_core::migrate::Chain::new(Vec::new())
}

/// Upgrades a project file tree (version 1 or later) to [`PROJECT_VERSION`]
/// in place. Returns the steps that ran (empty when it was current).
pub fn upgrade_project_tree(
    tree: &mut serde_json::Value,
) -> Result<Vec<String>, box_core::migrate::MigrateError> {
    let version = tree
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .filter(|v| *v >= 1)
        .ok_or_else(|| {
            box_core::migrate::MigrateError::Format("missing or invalid \"version\"".into())
        })?;
    if version == u64::from(PROJECT_VERSION) {
        return Ok(Vec::new());
    }
    let steps = format_chain().upgrade("project file", version as u32, tree)?;
    tree["version"] = serde_json::Value::from(PROJECT_VERSION);
    Ok(steps)
}

/// `"1.2.3"` (a pre-release or build suffix is ignored) as a comparable tuple.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// The sky, fog and exposure of a new project: [`Environment::day`] with a
/// brighter, bluer sky and exposure 0.8. AgX (kept) compresses and desaturates
/// bright colours, so the day preset's sky renders grey-blue; these sky
/// colours are chosen for the rendered result (a clear blue zenith fading to
/// a pale horizon), and `ambient_intensity` is lowered to keep the hemisphere
/// light, which uses the same colours, near the day preset's.
pub fn starter_environment() -> Environment {
    Environment {
        sky_zenith: Color::linear(0.02, 0.24, 0.9),
        sky_horizon: Color::linear(0.36, 0.62, 1.0),
        ambient_intensity: 0.55,
        exposure: 0.8,
        ..Environment::day()
    }
}

/// The scene a new project starts with: a 200 m ground plane, a cube, a
/// sun, a camera framing the cube and the [`starter_environment`].
pub fn starter_scene() -> World {
    let mut w = World::with_registry(shared_engine_registry());
    w.insert_resource(starter_environment());
    let spawn = |w: &mut World, name: &str| {
        w.spawn_saved(Uid::new(), name, ())
            .expect("fresh uids are unique")
    };

    let ground = spawn(&mut w, "Ground");
    let _ = w.insert(
        ground,
        (
            Transform::default(),
            Primitive {
                shape: Shape::Plane {
                    size: Vec2::new(200.0, 200.0),
                    cells: UVec2::ONE,
                },
                cast_shadows: false,
            },
            StandardMaterial {
                base_color: Color::hex(0x6f8f4f),
                roughness: 0.95,
                ..Default::default()
            },
        ),
    );

    let cube = spawn(&mut w, "Cube");
    let _ = w.insert(
        cube,
        (
            Transform::from_xyz(0.0, 0.5, 0.0),
            Primitive::new(Shape::Cuboid { size: Vec3::ONE }),
            StandardMaterial {
                base_color: Color::hex(0xc8643c),
                roughness: 0.6,
                ..Default::default()
            },
        ),
    );

    let sun = spawn(&mut w, "Sun");
    let _ = w.insert(
        sun,
        (
            Transform::from_xyz(0.0, 10.0, 0.0).looking_to(Vec3::new(0.35, -0.7, -0.6), Vec3::Y),
            DirectionalLight {
                shadow_distance: 40.0,
                ..Default::default()
            },
        ),
    );

    let camera = spawn(&mut w, "Camera");
    let _ = w.insert(
        camera,
        (
            Transform::from_xyz(2.6, 1.5, 3.6).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
            Camera::default(),
        ),
    );
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("box_project_{name}_{}", Uid::new()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn create_then_open() {
        let dir = temp_dir("create");
        let p = Project::create(&dir, "Demo").unwrap();
        assert_eq!(p.name(), "Demo");
        assert!(dir.join(".gitignore").is_file());
        let opened = Project::open(dir.join("assets/scenes")).unwrap();
        assert_eq!(opened.file(), p.file());
        assert_eq!(
            opened.scene_files().unwrap(),
            vec![opened.start_scene_path()]
        );
        assert_eq!(
            opened.relative(&opened.start_scene_path()),
            DEFAULT_START_SCENE
        );
        let bytes = std::fs::read(opened.start_scene_path()).unwrap();
        let world = scene::load(&bytes, shared_engine_registry()).unwrap();
        assert_eq!(world.roots().len(), 4);
        assert!(world.contains_resource::<Environment>());
        assert!(matches!(
            Project::create(&dir, "Again"),
            Err(ProjectError::AlreadyExists(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_newer_projects() {
        let path = Path::new("box.project.json");
        let newer_format =
            r#"{ "version": 2, "name": "x", "engine_version": "0.1.0", "start_scene": "a" }"#;
        assert!(matches!(
            parse_project_file(path, newer_format),
            Err(ProjectError::NewerFormat { found: 2, .. })
        ));
        let newer_engine =
            r#"{ "version": 1, "name": "x", "engine_version": "99.0.0", "start_scene": "a" }"#;
        let err = parse_project_file(path, newer_engine).unwrap_err();
        assert!(matches!(err, ProjectError::NewerEngine { .. }), "{err}");
        assert!(err.to_string().contains("update box-engine"));
        let older =
            r#"{ "version": 1, "name": "x", "engine_version": "0.0.1", "start_scene": "a" }"#;
        assert!(parse_project_file(path, older).is_ok());
        assert!(parse_project_file(path, "{").is_err());
    }

    #[test]
    fn not_found_names_the_directory() {
        let dir = temp_dir("missing");
        std::fs::create_dir_all(&dir).unwrap();
        let err = Project::open(&dir).unwrap_err();
        assert!(matches!(err, ProjectError::NotFound(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_start_scene_outside_the_project() {
        let path = Path::new("box.project.json");
        let with = |start: &str| {
            let file = serde_json::json!({
                "version": 1, "name": "x", "engine_version": "0.0.1", "start_scene": start,
            });
            parse_project_file(path, &file.to_string())
        };
        assert!(matches!(
            with("../x.scene.json"),
            Err(ProjectError::OutsideProject { .. })
        ));
        for bad in [
            "../outside/x.scene.json",
            "assets/../../x.scene.json",
            "/etc/x.scene.json",
            "\\x.scene.json",
            "..\\x.scene.json",
            "C:\\x.scene.json",
            "c:/x.scene.json",
            "C:x.scene.json",
            "\\\\server\\share\\x.scene.json",
            "",
            "assets/..",
        ] {
            assert!(with(bad).is_err(), "`{bad}` must be rejected");
        }
        for good in [
            "a.scene.json",
            "./assets/a.scene.json",
            "assets/x/../a.scene.json",
        ] {
            assert!(with(good).is_ok(), "`{good}` must be accepted");
        }
    }

    #[test]
    fn resolve_never_leaves_the_project() {
        let dir = temp_dir("resolve");
        let p = Project::create(&dir, "Demo").unwrap();
        assert_eq!(
            p.try_resolve("assets/x/../a.scene.json").unwrap(),
            p.root().join("assets").join("a.scene.json")
        );
        for bad in ["../x", "/x", "C:\\x", "a/../../x"] {
            assert!(matches!(
                p.try_resolve(bad),
                Err(ProjectError::OutsideProject { .. })
            ));
            assert_eq!(p.resolve(bad), p.root());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failed_atomic_write_removes_its_temporary_file() {
        let dir = temp_dir("atomic");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("a.json");
        write_atomic(&target, b"old").unwrap();
        let failed = write_atomic_with(&target, |f| {
            f.write_all(b"half")?;
            Err(std::io::Error::other("disk full"))
        });
        assert!(failed.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_create_leaves_nothing_that_blocks_a_retry() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("create_fails");
        // `assets/` is writable, the root is not: the scene can be written,
        // `.gitignore` and the project file cannot.
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let failed = Project::create(&dir, "Demo");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(failed, Err(ProjectError::Io { .. })), "{failed:?}");
        assert!(!dir.join(DEFAULT_START_SCENE).exists(), "scene left behind");
        assert!(
            !dir.join("assets/scenes").exists(),
            "created directory left behind"
        );
        assert!(
            dir.join("assets").is_dir(),
            "a directory that existed before is kept"
        );
        Project::create(&dir, "Demo").expect("retry succeeds");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn relative_ignores_dot_dot_segments() {
        let dir = temp_dir("dotdot");
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let created = Project::create(dir.join("other/../proj"), "Demo").unwrap();
        let scene = dir.join("proj").join(DEFAULT_START_SCENE);
        assert_eq!(created.relative(&scene), DEFAULT_START_SCENE);
        let opened = Project::open(dir.join("other/../proj")).unwrap();
        assert_eq!(opened.relative(&scene), DEFAULT_START_SCENE);
        assert_eq!(
            opened.relative(&dir.join("proj/assets/../assets/new.scene.json")),
            "assets/new.scene.json"
        );
        assert_eq!(
            opened.relative(&opened.start_scene_path()),
            DEFAULT_START_SCENE
        );
        let outside = dir.join("other/x.scene.json");
        assert_eq!(opened.relative(&outside), outside.display().to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn relative_sees_through_a_symlinked_root() {
        let dir = temp_dir("symlink");
        Project::create(dir.join("real"), "Demo").unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("link")).unwrap();
        let via_link = Project::open(dir.join("link")).unwrap();
        assert_eq!(
            via_link.relative(&dir.join("real").join(DEFAULT_START_SCENE)),
            DEFAULT_START_SCENE
        );
        assert_eq!(
            via_link.relative(&dir.join("real/assets/not_yet.scene.json")),
            "assets/not_yet.scene.json"
        );
        let real = Project::open(dir.join("real")).unwrap();
        assert_eq!(
            real.relative(&dir.join("link").join(DEFAULT_START_SCENE)),
            DEFAULT_START_SCENE
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_splash_setting_defaults_to_on() {
        let path = Path::new("box.project.json");
        let parse = |export: &str| {
            let text = format!(
                r#"{{ "version": 1, "name": "x", "engine_version": "0.0.1", "start_scene": "a"{export} }}"#
            );
            parse_project_file(path, &text).unwrap().export
        };
        let absent = parse("");
        assert_eq!(absent.splash, None);
        assert!(absent.splash_enabled() && absent.is_empty());
        let off = parse(r#", "export": { "splash": false }"#);
        assert_eq!(off.splash, Some(false));
        assert!(!off.splash_enabled());
        assert!(parse(r#", "export": { "splash": true }"#).splash_enabled());
        // Written back only when set.
        let json = serde_json::to_value(&off).unwrap();
        assert_eq!(json, serde_json::json!({ "splash": false }));
        assert_eq!(
            serde_json::to_value(&absent).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_version("x"), None);
    }
}
