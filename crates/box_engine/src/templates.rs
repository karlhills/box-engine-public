//! Project templates and example projects: where they are found, what they
//! hold, and making a new project from one (`box new --template`, the
//! editor's New Project and Open Example dialogs).
//!
//! **Lookup.** The templates root is the first of these folders that holds
//! `projects/` (templates) or `examples/examples.json` (the gallery), the
//! two searched separately, in order:
//!
//! 1. `$BOX_TEMPLATES` (the variable the player templates of `box export`
//!    use too)
//! 2. `<folder of the running program>/../Resources/templates` (inside
//!    `Box Editor.app`: `Contents/Resources/templates`; release packaging
//!    copies the repository's `templates/projects/` to
//!    `Contents/Resources/templates/projects/`)
//! 3. `<folder of the running program>/templates` (a release zip, `box`
//!    next to `templates/`)
//! 4. the source checkout this engine was built from (`<repo>/templates`),
//!    when it still exists
//!
//! "The running program" is the real executable with symlinks resolved, so
//! a `box` symlinked from `~/.local/bin` into the app bundle finds the
//! bundle's templates.
//!
//! Templates are merged by name across those roots (the first wins), so a
//! release that ships fewer templates still finds the rest in a checkout.
//!
//! **Layout** (`<root>` is one of the folders above):
//!
//! ```text
//! <root>/projects/<name>/template.json   {"title", "description", "order", "native", "base"}
//! <root>/projects/<name>/preview.png     shown by the editor (optional)
//! <root>/projects/<name>/...             the project's files (box.project.json, assets/, README.md, .gitignore)
//! <root>/examples/examples.json          {"examples": [{"name", "title", "description"}]}
//! <root>/examples/<name>.png             an example's preview (optional)
//! <root>/examples/<name>/                the example project (a release copies it here) ...
//! <root>/../examples/<name>/             ... or next to the root (the source checkout's examples/)
//! ```
//!
//! A new project is a copy of the template folder without `template.json`
//! and `preview.png`, with `box.project.json` rewritten to the new name and
//! this engine's version. A template with `"base": "<other>"` copies the
//! other template first and its own files over it. A `"native": true`
//! template is a data project plus a Rust crate that only the `box` CLI
//! writes (`box new --native`); [`create_project`] refuses it.
//!
//! The `empty` template is also built in ([`Project::create`], the same
//! scene), so `box new` works even where no templates folder is found.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Project;
use crate::project::{ENGINE_VERSION, PROJECT_FILE, PROJECT_VERSION, ProjectError, ProjectFile};

/// Environment variable naming a templates folder (shared with the player
/// templates of `box export`).
pub const TEMPLATES_ENV: &str = "BOX_TEMPLATES";
/// The template `box new` and the editor use when none is chosen.
pub const DEFAULT_TEMPLATE: &str = "empty";
/// The template that adds a Rust crate (`box new --native`).
pub const NATIVE_TEMPLATE: &str = "native";
/// A template's description file, inside its folder.
pub const TEMPLATE_FILE: &str = "template.json";
/// A template's preview image, inside its folder.
pub const PREVIEW_FILE: &str = "preview.png";
/// The gallery's list of example projects, inside `<root>/examples/`.
pub const EXAMPLES_FILE: &str = "examples.json";

/// Files of a template folder that are not copied into the project.
const NOT_COPIED: [&str; 2] = [TEMPLATE_FILE, PREVIEW_FILE];

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown template `{name}`; available: {}", .available.join(", "))]
    UnknownTemplate {
        name: String,
        available: Vec<String>,
    },
    #[error("unknown example `{name}`; available: {}", .available.join(", "))]
    UnknownExample {
        name: String,
        available: Vec<String>,
    },
    #[error(
        "template `{0}` is a native project (a Rust crate): create it with `box new --native <folder>`"
    )]
    Native(String),
    #[error("{}: already exists; choose a new or empty folder", .0.display())]
    AlreadyExists(PathBuf),
    #[error("{}: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Project(#[from] ProjectError),
}

impl TemplateError {
    /// The file or folder the error is about, if any.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::AlreadyExists(p) | Self::Invalid { path: p, .. } | Self::Io { path: p, .. } => {
                Some(p)
            }
            Self::Project(e) => e.path(),
            _ => None,
        }
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> TemplateError + '_ {
    move |source| TemplateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A project template.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// The folder name, what `box new --template` takes (`third-person`).
    pub name: String,
    /// Shown in the editor (`Third-person game`).
    pub title: String,
    /// One or two sentences: what the project holds.
    pub description: String,
    /// Its folder; `None` for the built-in `empty` template.
    pub dir: Option<PathBuf>,
    /// A data project plus a Rust crate, written by `box new --native`.
    pub native: bool,
    /// Another template copied first.
    pub base: Option<String>,
    order: i32,
}

impl Template {
    /// The preview image, when the template has one.
    pub fn preview(&self) -> Option<PathBuf> {
        let p = self.dir.as_ref()?.join(PREVIEW_FILE);
        p.is_file().then_some(p)
    }

    fn builtin_empty() -> Self {
        Self {
            name: DEFAULT_TEMPLATE.into(),
            title: "Empty".into(),
            description: "A ground plane, a cube, a sun and a camera.".into(),
            dir: None,
            native: false,
            base: None,
            order: 0,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateJson {
    title: String,
    description: String,
    #[serde(default)]
    order: i32,
    #[serde(default)]
    native: bool,
    #[serde(default)]
    base: Option<String>,
}

/// An example project of the gallery.
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    /// The folder name (`collector_luau`), the default name of a copy.
    pub name: String,
    pub title: String,
    pub description: String,
    /// The example project (never edited: [`copy_example`] copies it).
    pub dir: PathBuf,
    /// Its preview image, when there is one.
    pub preview: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExamplesJson {
    examples: Vec<ExampleJson>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExampleJson {
    name: String,
    title: String,
    description: String,
}

/// The source checkout's templates folder (`<repo>/templates`).
fn source_root() -> PathBuf {
    normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates"))
}

/// The folders searched for templates and examples, in order (see the
/// module docs), for a `BOX_TEMPLATES` value, the running program's folder
/// and the source checkout's templates folder.
pub fn search_dirs_in(
    env: Option<&Path>,
    exe_dir: Option<&Path>,
    source: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = env.filter(|d| !d.as_os_str().is_empty()) {
        dirs.push(dir.to_path_buf());
    }
    if let Some(exe) = exe_dir {
        dirs.push(normalize(&exe.join("../Resources/templates")));
        dirs.push(exe.join("templates"));
    }
    if let Some(source) = source {
        dirs.push(source.to_path_buf());
    }
    dirs
}

/// [`search_dirs_in`] for this process.
pub fn search_dirs() -> Vec<PathBuf> {
    let env = std::env::var_os(TEMPLATES_ENV).map(PathBuf::from);
    // Symlinks resolved: `~/.local/bin/box` -> `Box Editor.app/Contents/MacOS/box`.
    let exe_dir = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    search_dirs_in(env.as_deref(), exe_dir.as_deref(), Some(&source_root()))
}

/// Every template found in `dirs`, sorted for display (`order`, then name);
/// always includes `empty`. Broken `template.json` files are logged and
/// skipped.
pub fn templates_in(dirs: &[PathBuf]) -> Vec<Template> {
    let mut out: Vec<Template> = Vec::new();
    for root in dirs {
        let Ok(entries) = std::fs::read_dir(root.join("projects")) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        found.sort();
        for dir in found {
            let file = dir.join(TEMPLATE_FILE);
            if !file.is_file() {
                continue;
            }
            let Some(name) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            if out.iter().any(|t| t.name == name) {
                continue;
            }
            let parsed = std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    serde_json::from_str::<TemplateJson>(&text).map_err(|e| e.to_string())
                });
            match parsed {
                Ok(t) => out.push(Template {
                    name,
                    title: t.title,
                    description: t.description,
                    dir: Some(dir),
                    native: t.native,
                    base: t.base,
                    order: t.order,
                }),
                Err(e) => {
                    tracing::warn!(target: "box_engine::templates", "{}: {e}; template skipped", file.display())
                }
            }
        }
    }
    if !out.iter().any(|t| t.name == DEFAULT_TEMPLATE) {
        out.push(Template::builtin_empty());
    }
    out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Every template this process finds ([`search_dirs`]).
pub fn templates() -> Vec<Template> {
    templates_in(&search_dirs())
}

/// The template called `name` among `list`.
pub fn find_template<'a>(list: &'a [Template], name: &str) -> Result<&'a Template, TemplateError> {
    list.iter()
        .find(|t| t.name == name)
        .ok_or_else(|| TemplateError::UnknownTemplate {
            name: name.to_string(),
            available: list.iter().map(|t| t.name.clone()).collect(),
        })
}

/// The gallery's examples found in `dirs`: from the first root with an
/// `examples/examples.json`, each found at `<root>/examples/<name>` or
/// `<root>/../examples/<name>`. Missing ones are skipped.
pub fn examples_in(dirs: &[PathBuf]) -> Vec<Example> {
    for root in dirs {
        let gallery = root.join("examples");
        let file = gallery.join(EXAMPLES_FILE);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let list = match serde_json::from_str::<ExamplesJson>(&text) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(target: "box_engine::templates", "{}: {e}", file.display());
                continue;
            }
        };
        return list
            .examples
            .into_iter()
            .filter_map(|e| {
                let dir = [
                    gallery.join(&e.name),
                    root.join("../examples").join(&e.name),
                ]
                .into_iter()
                .find(|d| d.join(PROJECT_FILE).is_file())?;
                let preview = gallery.join(format!("{}.png", e.name));
                Some(Example {
                    dir: normalize(&dir),
                    preview: preview.is_file().then_some(preview),
                    name: e.name,
                    title: e.title,
                    description: e.description,
                })
            })
            .collect();
    }
    Vec::new()
}

/// The gallery's examples for this process ([`search_dirs`]).
pub fn examples() -> Vec<Example> {
    examples_in(&search_dirs())
}

/// The example called `name` among `list`.
pub fn find_example<'a>(list: &'a [Example], name: &str) -> Result<&'a Example, TemplateError> {
    list.iter()
        .find(|e| e.name == name)
        .ok_or_else(|| TemplateError::UnknownExample {
            name: name.to_string(),
            available: list.iter().map(|e| e.name.clone()).collect(),
        })
}

/// `..` segments resolved lexically (for display: `<repo>/examples/x`).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether a path below a project folder is copied into a new project or a
/// copy of an example: not `.box/` (editor layout, saves, the registry;
/// `.box/types/`, the committed luau-lsp definitions, is kept), `target/`,
/// `.DS_Store` or a template's own files.
fn copied(relative: &Path) -> bool {
    let mut parts = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy());
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if first == ".box" {
        return second.as_deref() == Some("types") || second.is_none();
    }
    let name = relative
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    first != "target" && name != ".DS_Store"
}

/// Every file to copy from `src`, relative to it, sorted.
fn files_to_copy(src: &Path, skip_template_files: bool) -> Result<Vec<PathBuf>, TemplateError> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), TemplateError> {
        for entry in std::fs::read_dir(dir).map_err(io(dir))? {
            let entry = entry.map_err(io(dir))?;
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            if !copied(&rel) {
                continue;
            }
            let kind = entry.file_type().map_err(io(&path))?;
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                out.push(rel);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(src, src, &mut out)?;
    if skip_template_files {
        out.retain(|r| !NOT_COPIED.iter().any(|n| r == Path::new(n)));
    }
    out.sort();
    Ok(out)
}

/// Copies `files` (relative paths) from their sources into `dest`, refusing
/// before writing anything when one already exists there. On failure the
/// files and folders it created are removed again.
fn copy_files(dest: &Path, files: &[(PathBuf, PathBuf)]) -> Result<(), TemplateError> {
    for (rel, _) in files {
        let to = dest.join(rel);
        if to.exists() {
            return Err(TemplateError::AlreadyExists(to));
        }
    }
    let mut created_dirs: Vec<PathBuf> = Vec::new();
    let mut created_files: Vec<PathBuf> = Vec::new();
    let result = (|| {
        for (rel, from) in files {
            let to = dest.join(rel);
            if let Some(parent) = to.parent() {
                let missing: Vec<PathBuf> = parent
                    .ancestors()
                    .take_while(|d| !d.exists())
                    .map(Path::to_path_buf)
                    .collect();
                std::fs::create_dir_all(parent).map_err(io(parent))?;
                created_dirs.extend(missing);
            }
            std::fs::copy(from, &to).map_err(io(&to))?;
            created_files.push(to);
        }
        Ok(())
    })();
    if result.is_err() {
        for f in &created_files {
            let _ = std::fs::remove_file(f);
        }
        // Deepest first; fails (as it should) where something else is in there.
        created_dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
        for d in &created_dirs {
            let _ = std::fs::remove_dir(d);
        }
    }
    result
}

/// The files a template copies: its base's first, then its own (which
/// replace the base's), as (relative path, source file).
fn template_files(
    list: &[Template],
    template: &Template,
    depth: usize,
) -> Result<Vec<(PathBuf, PathBuf)>, TemplateError> {
    let Some(dir) = &template.dir else {
        return Ok(Vec::new());
    };
    let mut files = match &template.base {
        Some(base) if depth < 4 => template_files(list, find_template(list, base)?, depth + 1)?,
        Some(_) => {
            return Err(TemplateError::Invalid {
                path: dir.join(TEMPLATE_FILE),
                message: "`base` templates nest too deeply".into(),
            });
        }
        None => Vec::new(),
    };
    for rel in files_to_copy(dir, true)? {
        files.retain(|(r, _)| *r != rel);
        let from = dir.join(&rel);
        files.push((rel, from));
    }
    Ok(files)
}

/// Rewrites the copied `box.project.json` with `name` and this engine's
/// version.
fn finish_project_file(root: &Path, name: &str) -> Result<Project, TemplateError> {
    let path = root.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    let mut file: ProjectFile = crate::project::parse_project_file(&path, &text)?;
    file.version = PROJECT_VERSION;
    file.name = name.to_string();
    file.engine_version = ENGINE_VERSION.to_string();
    let mut text = serde_json::to_string_pretty(&file).map_err(|e| TemplateError::Invalid {
        path: path.clone(),
        message: e.to_string(),
    })?;
    text.push('\n');
    crate::project::write_atomic(&path, text.as_bytes()).map_err(io(&path))?;
    Ok(Project::open(root)?)
}

/// Creates a project called `name` in `dir` (created if missing) from
/// `template` (one of `list`, which resolves its `base`). Refuses a folder
/// that already holds a project or any file the template would write;
/// other files are left alone. Native templates are refused
/// ([`TemplateError::Native`]); `box new --native` writes those.
pub fn create_project(
    dir: &Path,
    name: &str,
    list: &[Template],
    template: &Template,
) -> Result<Project, TemplateError> {
    if template.native {
        return Err(TemplateError::Native(template.name.clone()));
    }
    create_data_project(dir, name, list, template)
}

/// [`create_project`] without the native check: a native template's data
/// project, which the CLI then adds the crate to.
pub fn create_data_project(
    dir: &Path,
    name: &str,
    list: &[Template],
    template: &Template,
) -> Result<Project, TemplateError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ProjectError::EmptyName.into());
    }
    let root = std::path::absolute(dir).map_err(io(dir))?;
    let root = normalize(&root);
    if root.join(PROJECT_FILE).exists() {
        return Err(ProjectError::AlreadyExists(root).into());
    }
    let files = template_files(list, template, 0)?;
    if files.is_empty() {
        // The built-in `empty` template.
        return Ok(Project::create(&root, name)?);
    }
    if !files.iter().any(|(r, _)| r == Path::new(PROJECT_FILE)) {
        return Err(TemplateError::Invalid {
            path: template.dir.clone().unwrap_or_default(),
            message: format!("template has no {PROJECT_FILE}"),
        });
    }
    copy_files(&root, &files)?;
    for (path, bytes) in crate::project::agent_guide_files(&root) {
        crate::project::write_atomic(&path, &bytes).map_err(io(&path))?;
    }
    match finish_project_file(&root, name) {
        Ok(p) => Ok(p),
        Err(e) => {
            for (rel, _) in &files {
                let _ = std::fs::remove_file(root.join(rel));
            }
            Err(e)
        }
    }
}

/// Copies `example` into `dest` (a new or empty folder) and returns the
/// copy. The original is only read. `.box/` (local editor state) is left
/// out, except `.box/types/`.
pub fn copy_example(example: &Example, dest: &Path) -> Result<Project, TemplateError> {
    let root = normalize(&std::path::absolute(dest).map_err(io(dest))?);
    if root.exists() {
        let mut entries = std::fs::read_dir(&root).map_err(io(&root))?;
        if entries.next().is_some() {
            return Err(TemplateError::AlreadyExists(root));
        }
    }
    let src = example.dir.canonicalize().map_err(io(&example.dir))?;
    // The copy's longest existing prefix with symlinks resolved.
    let real = root
        .ancestors()
        .find(|a| a.exists())
        .and_then(|a| Some(a.canonicalize().ok()?.join(root.strip_prefix(a).ok()?)));
    if real.is_some_and(|r| r.starts_with(&src)) {
        return Err(TemplateError::Invalid {
            path: root,
            message: "the copy cannot go inside the example".into(),
        });
    }
    let files: Vec<(PathBuf, PathBuf)> = files_to_copy(&src, false)?
        .into_iter()
        .map(|rel| {
            let from = src.join(&rel);
            (rel, from)
        })
        .collect();
    copy_files(&root, &files)?;
    Ok(Project::open(&root)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "box_templates_{name}_{}",
            box_core::identity::Uid::new()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn search_order_matches_the_docs() {
        let dirs = search_dirs_in(
            Some(Path::new("/env")),
            Some(Path::new("/app/bin")),
            Some(Path::new("/src/templates")),
        );
        assert_eq!(
            dirs,
            [
                PathBuf::from("/env"),
                PathBuf::from("/app/Resources/templates"),
                PathBuf::from("/app/bin/templates"),
                PathBuf::from("/src/templates"),
            ]
        );
        assert_eq!(
            search_dirs_in(Some(Path::new("")), None, None),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn copies_skip_local_state() {
        assert!(copied(Path::new("assets/scenes/main.scene.json")));
        assert!(copied(Path::new(".box")));
        assert!(copied(Path::new(".box/types/box.d.luau")));
        assert!(!copied(Path::new(".box/layout.json")));
        assert!(!copied(Path::new(".box/saves")));
        assert!(!copied(Path::new("target/debug/x")));
        assert!(!copied(Path::new("assets/.DS_Store")));
    }

    #[test]
    fn source_checkout_has_the_templates_and_examples() {
        let list = templates_in(&[source_root()]);
        let names: Vec<&str> = list.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["empty", "third-person", "procedural", "native"]);
        for t in &list {
            assert!(t.preview().is_some(), "{} has no preview", t.name);
            assert!(
                t.dir.as_ref().unwrap().join("README.md").is_file(),
                "{}",
                t.name
            );
        }
        let examples = examples_in(&[source_root()]);
        assert!(examples.len() >= 3, "{examples:?}");
        for e in &examples {
            assert!(e.preview.is_some(), "{} has no preview", e.name);
        }
    }

    /// The `empty` template's scene is the built-in starter scene (only the
    /// uids differ), so `box new` without a templates folder gives the same.
    #[test]
    fn empty_template_is_the_starter_scene() {
        fn without_uids(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(map) => {
                    map.remove("uid");
                    map.values_mut().for_each(without_uids);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(without_uids),
                _ => {}
            }
        }
        let file = source_root().join("projects/empty/assets/scenes/main.scene.json");
        let mut committed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let built = box_core::scene::save(&crate::project::starter_scene()).unwrap();
        let mut built: serde_json::Value = serde_json::from_slice(&built).unwrap();
        without_uids(&mut committed);
        without_uids(&mut built);
        // The ground's collider exists only with the physics module.
        if !cfg!(feature = "physics") {
            for e in committed["entities"].as_array_mut().unwrap() {
                if let Some(c) = e["components"].as_object_mut() {
                    c.remove("Collider");
                }
            }
        }
        assert_eq!(committed, built, "regenerate {}", file.display());
    }

    #[test]
    fn creates_from_a_template_and_refuses_collisions() {
        let src = temp("src");
        let t = src.join("projects/demo");
        std::fs::create_dir_all(t.join("assets/scenes")).unwrap();
        std::fs::write(
            t.join(TEMPLATE_FILE),
            r#"{"title": "Demo", "description": "d"}"#,
        )
        .unwrap();
        std::fs::write(t.join(PREVIEW_FILE), b"png").unwrap();
        std::fs::write(
            t.join(PROJECT_FILE),
            r#"{"version": 1, "name": "Demo", "engine_version": "0.0.1", "start_scene": "assets/scenes/main.scene.json"}"#,
        )
        .unwrap();
        std::fs::write(
            t.join("assets/scenes/main.scene.json"),
            r#"{ "version": 1, "resources": {}, "entities": [] }"#,
        )
        .unwrap();
        std::fs::create_dir_all(t.join(".box")).unwrap();
        std::fs::write(t.join(".box/layout.json"), b"{}").unwrap();
        let list = templates_in(std::slice::from_ref(&src));
        let demo = find_template(&list, "demo").unwrap();
        assert!(find_template(&list, "empty").unwrap().dir.is_none());
        let err = find_template(&list, "nope").unwrap_err().to_string();
        assert!(err.contains("available: demo, empty"), "{err}");

        let dest = temp("dest");
        let project = create_project(&dest, "My Game", &list, demo).unwrap();
        assert_eq!(project.name(), "My Game");
        assert_eq!(project.file().engine_version, ENGINE_VERSION);
        assert!(dest.join("assets/scenes/main.scene.json").is_file());
        assert!(!dest.join(TEMPLATE_FILE).exists());
        assert!(!dest.join(PREVIEW_FILE).exists());
        assert!(!dest.join(".box/layout.json").exists());
        // A second project in the same folder is refused.
        assert!(create_project(&dest, "Again", &list, demo).is_err());

        // A file the template would write: refused, nothing written.
        let other = temp("other");
        std::fs::create_dir_all(other.join("assets/scenes")).unwrap();
        std::fs::write(other.join("assets/scenes/main.scene.json"), b"mine").unwrap();
        let err = create_project(&other, "X", &list, demo).unwrap_err();
        assert!(matches!(err, TemplateError::AlreadyExists(_)), "{err}");
        assert!(!other.join(PROJECT_FILE).exists());
        assert_eq!(
            std::fs::read(other.join("assets/scenes/main.scene.json")).unwrap(),
            b"mine"
        );

        for d in [src, dest, other] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// The packaged layout the release bundles (see the module docs): the
    /// editor app's `Contents/Resources/templates` with `projects/` and
    /// `examples/` (the example projects copied in), found from the
    /// executable's folder alone.
    #[test]
    fn finds_templates_and_examples_inside_the_app_bundle() {
        fn copy_tree(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for e in std::fs::read_dir(from).unwrap().flatten() {
                let target = to.join(e.file_name());
                if e.file_type().unwrap().is_dir() {
                    copy_tree(&e.path(), &target);
                } else {
                    std::fs::copy(e.path(), target).unwrap();
                }
            }
        }
        let root = temp("bundle");
        let contents = root.join("Box Editor.app/Contents");
        let exe_dir = contents.join("MacOS");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let bundled = contents.join("Resources/templates");
        copy_tree(&source_root().join("projects"), &bundled.join("projects"));
        let gallery = bundled.join("examples");
        std::fs::create_dir_all(&gallery).unwrap();
        std::fs::copy(
            source_root().join("examples").join(EXAMPLES_FILE),
            gallery.join(EXAMPLES_FILE),
        )
        .unwrap();
        let script_rules = source_root().join("../examples/script_rules");
        copy_tree(&script_rules, &gallery.join("script_rules"));
        std::fs::copy(
            source_root().join("examples/script_rules.png"),
            gallery.join("script_rules.png"),
        )
        .unwrap();

        let dirs = search_dirs_in(None, Some(&exe_dir), None);
        let list = templates_in(&dirs);
        let third = find_template(&list, "third-person").unwrap();
        assert!(
            third.dir.as_ref().unwrap().starts_with(&bundled),
            "{third:?}"
        );
        assert!(third.preview().is_some());
        // Only the examples the bundle holds are offered.
        let examples = examples_in(&dirs);
        assert_eq!(examples.len(), 1, "{examples:?}");
        assert!(examples[0].dir.starts_with(&bundled), "{examples:?}");
        assert!(examples[0].preview.is_some());
        let copy = copy_example(&examples[0], &root.join("mine")).unwrap();
        assert_eq!(copy.name(), "Script Rules");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn native_templates_are_refused_here() {
        let t = Template {
            native: true,
            name: "native".into(),
            ..Template::builtin_empty()
        };
        let err = create_project(&temp("native"), "N", &[], &t).unwrap_err();
        assert!(err.to_string().contains("box new --native"), "{err}");
    }
}
