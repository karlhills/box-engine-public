//! Engine shader sources: embedding, a tiny `#include` preprocessor, naga
//! validation and (in debug builds with the `hot-reload` feature) loading from
//! disk with a file watcher.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Every engine shader file, embedded at compile time.
const EMBEDDED: &[(&str, &str)] = &[
    ("common.wgsl", include_str!("../shaders/common.wgsl")),
    ("object.wgsl", include_str!("../shaders/object.wgsl")),
    ("lighting.wgsl", include_str!("../shaders/lighting.wgsl")),
    ("shadows.wgsl", include_str!("../shaders/shadows.wgsl")),
    ("fog.wgsl", include_str!("../shaders/fog.wgsl")),
    ("forward.wgsl", include_str!("../shaders/forward.wgsl")),
    ("wind.wgsl", include_str!("../shaders/wind.wgsl")),
    (
        "material_standard.wgsl",
        include_str!("../shaders/material_standard.wgsl"),
    ),
    (
        "material_api.wgsl",
        include_str!("../shaders/material_api.wgsl"),
    ),
    (
        "custom_material.wgsl",
        include_str!("../shaders/custom_material.wgsl"),
    ),
    ("mesh.wgsl", include_str!("../shaders/mesh.wgsl")),
    ("error.wgsl", include_str!("../shaders/error.wgsl")),
    (
        "shadow_depth.wgsl",
        include_str!("../shaders/shadow_depth.wgsl"),
    ),
    ("sky.wgsl", include_str!("../shaders/sky.wgsl")),
    ("tonemap.wgsl", include_str!("../shaders/tonemap.wgsl")),
    (
        "debug_lines.wgsl",
        include_str!("../shaders/debug_lines.wgsl"),
    ),
    (
        "display_lines.wgsl",
        include_str!("../shaders/display_lines.wgsl"),
    ),
    (
        "outline_mask.wgsl",
        include_str!("../shaders/outline_mask.wgsl"),
    ),
    ("outline.wgsl", include_str!("../shaders/outline.wgsl")),
    ("prepass.wgsl", include_str!("../shaders/prepass.wgsl")),
    ("ao_common.wgsl", include_str!("../shaders/ao_common.wgsl")),
    (
        "ao_resolve.wgsl",
        include_str!("../shaders/ao_resolve.wgsl"),
    ),
    ("ao.wgsl", include_str!("../shaders/ao.wgsl")),
    ("bloom.wgsl", include_str!("../shaders/bloom.wgsl")),
    ("particles.wgsl", include_str!("../shaders/particles.wgsl")),
    ("skin.wgsl", include_str!("../shaders/skin.wgsl")),
    (
        "mesh_skinned.wgsl",
        include_str!("../shaders/mesh_skinned.wgsl"),
    ),
    (
        "shadow_skinned.wgsl",
        include_str!("../shaders/shadow_skinned.wgsl"),
    ),
    (
        "error_skinned.wgsl",
        include_str!("../shaders/error_skinned.wgsl"),
    ),
    (
        "custom_material_skinned.wgsl",
        include_str!("../shaders/custom_material_skinned.wgsl"),
    ),
];

/// Shaders that become shader modules (the rest are include-only).
pub(crate) const TOP_LEVEL: &[&str] = &[
    "mesh.wgsl",
    "error.wgsl",
    "shadow_depth.wgsl",
    "sky.wgsl",
    "tonemap.wgsl",
    "debug_lines.wgsl",
    "display_lines.wgsl",
    "outline_mask.wgsl",
    "outline.wgsl",
    "ao_resolve.wgsl",
    "ao.wgsl",
    "bloom.wgsl",
    "particles.wgsl",
    "mesh_skinned.wgsl",
    "shadow_skinned.wgsl",
    "error_skinned.wgsl",
];

/// Raw shader files by name.
#[derive(Clone, Debug)]
pub(crate) struct ShaderSources {
    files: HashMap<String, String>,
}

/// Engine library modules a material shader may import as
/// `import box::<name>;` (`#import` also accepted), and the file each maps
/// to. The library is always included, so imports only document intent.
const IMPORTS: &[(&str, &str)] = &[
    ("globals", "common.wgsl"),
    ("object", "object.wgsl"),
    ("lighting", "lighting.wgsl"),
    ("shadows", "shadows.wgsl"),
    ("fog", "fog.wgsl"),
    ("forward", "forward.wgsl"),
    ("wind", "wind.wgsl"),
    ("material", "material_api.wgsl"),
];

/// Preprocessed and validated top-level shaders, ready for module creation,
/// plus the raw library files material shaders are composed with.
#[derive(Clone, Debug)]
pub(crate) struct ProcessedShaders {
    modules: HashMap<&'static str, String>,
    pub(crate) sources: ShaderSources,
}

impl ProcessedShaders {
    pub(crate) fn get(&self, name: &str) -> &str {
        self.modules
            .get(name)
            .map(String::as_str)
            .unwrap_or_else(|| panic!("unknown shader {name}"))
    }
}

impl ShaderSources {
    pub(crate) fn embedded() -> Self {
        Self {
            files: EMBEDDED
                .iter()
                .map(|(n, s)| (n.to_string(), s.to_string()))
                .collect(),
        }
    }

    /// Reads every engine shader file from `dir`.
    pub(crate) fn load_dir(dir: &Path) -> Result<Self, String> {
        let mut files = HashMap::new();
        for (name, _) in EMBEDDED {
            let path = dir.join(name);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read shader {}: {e}", path.display()))?;
            files.insert(name.to_string(), text);
        }
        Ok(Self { files })
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    pub(crate) fn set(&mut self, name: &str, text: &str) {
        self.files.insert(name.to_string(), text.to_string());
    }

    #[cfg(test)]
    /// Resolves `#include "file.wgsl"` lines recursively. Each file is included
    /// at most once per top-level shader.
    pub(crate) fn preprocess(&self, name: &str) -> Result<String, String> {
        self.preprocess_mapped(name).map(|(text, _)| text)
    }

    /// Resolves `#include "file.wgsl"` lines recursively (each file at most
    /// once per top-level shader), also returning the source file and line of
    /// every output line for error messages.
    pub(crate) fn preprocess_mapped(&self, name: &str) -> Result<(String, LineMap), String> {
        let mut out = String::new();
        let mut map = Vec::new();
        let mut seen = HashSet::new();
        self.expand(name, &mut out, &mut map, &mut seen, &mut Vec::new())?;
        Ok((out, map))
    }

    fn expand(
        &self,
        name: &str,
        out: &mut String,
        map: &mut LineMap,
        seen: &mut HashSet<String>,
        stack: &mut Vec<String>,
    ) -> Result<(), String> {
        if !seen.insert(name.to_string()) {
            return Ok(());
        }
        let text = self.files.get(name).ok_or_else(|| {
            if let Some(parent) = stack.last() {
                format!("{parent}: included file \"{name}\" not found")
            } else {
                format!("shader \"{name}\" not found")
            }
        })?;
        stack.push(name.to_string());
        for (line_no, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("#include") {
                let include = parse_include(rest)
                    .ok_or_else(|| format!("{name}:{}: malformed #include", line_no + 1))?;
                self.expand(include, out, map, seen, stack)?;
            } else if let Some(rest) = trimmed
                .strip_prefix("#import")
                .or_else(|| trimmed.strip_prefix("import "))
            {
                let file =
                    parse_import(rest).map_err(|e| format!("{name}:{}: {e}", line_no + 1))?;
                self.expand(file, out, map, seen, stack)?;
                // Keep a line for it so later lines still map to their source.
                out.push('\n');
                map.push((name.to_string(), line_no as u32 + 1));
            } else {
                out.push_str(line);
                out.push('\n');
                map.push((name.to_string(), line_no as u32 + 1));
            }
        }
        stack.pop();
        Ok(())
    }

    /// Preprocesses and validates every top-level shader.
    pub(crate) fn build(&self) -> Result<ProcessedShaders, String> {
        let mut modules = HashMap::new();
        let mut errors = Vec::new();
        for &name in TOP_LEVEL {
            match self
                .preprocess_mapped(name)
                .and_then(|(src, map)| validate_wgsl(name, &src, &map).map(|_| src))
            {
                Ok(src) => {
                    modules.insert(name, src);
                }
                Err(e) => errors.push(e),
            }
        }
        if errors.is_empty() {
            Ok(ProcessedShaders {
                modules,
                sources: self.clone(),
            })
        } else {
            Err(errors.join("\n"))
        }
    }
}

/// `box::lighting;` or `box::lighting::{shade_standard, Surface};` (after
/// `#import` / `import`): the library file it names.
fn parse_import(rest: &str) -> Result<&'static str, String> {
    let rest = rest.trim().trim_end_matches(';').trim();
    let path = rest.split("::{").next().unwrap_or(rest).trim();
    let module = path
        .strip_prefix("box::")
        .ok_or_else(|| format!("unknown import `{rest}` (expected box::<module>)"))?;
    IMPORTS
        .iter()
        .find(|(m, _)| *m == module)
        .map(|(_, file)| *file)
        .ok_or_else(|| {
            let known: Vec<_> = IMPORTS.iter().map(|(m, _)| format!("box::{m}")).collect();
            format!("unknown import `{path}` (available: {})", known.join(", "))
        })
}

fn parse_include(rest: &str) -> Option<&str> {
    let rest = rest.trim();
    let inner = rest.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.is_empty()).then_some(inner)
}

/// Source file and 1-based line of each preprocessed line.
pub(crate) type LineMap = Vec<(String, u32)>;

/// Parses and validates WGSL with naga, returning the module, or a readable
/// report on failure that names the original file and line.
pub(crate) fn validate_wgsl(
    name: &str,
    source: &str,
    map: &LineMap,
) -> Result<naga::Module, String> {
    let origin = |loc: Option<naga::SourceLocation>| {
        loc.and_then(|l| map.get(l.line_number as usize - 1))
            .map(|(file, line)| format!(" (at {file}:{line})"))
            .unwrap_or_default()
    };
    let preprocessed = format!("{name} (preprocessed)");
    let module = naga::front::wgsl::parse_str(source).map_err(|e| {
        format!(
            "shader {name} failed to parse{}:\n{}",
            origin(e.location(source)),
            e.emit_to_string_with_path(source, &preprocessed)
        )
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| {
        format!(
            "shader {name} failed validation{}:\n{}",
            origin(e.location(source)),
            e.emit_to_string_with_path(source, &preprocessed)
        )
    })?;
    Ok(module)
}

static HOT_RELOAD_OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns engine shader hot reload off (or back on) for renderers created
/// after the call. It is only ever on in debug builds with the `hot-reload`
/// feature, where it reads `crates/box_render/shaders` (or `BOX_SHADER_DIR`);
/// exported games turn it off so they read nothing outside their pak.
pub fn set_shader_hot_reload(enabled: bool) {
    HOT_RELOAD_OFF.store(!enabled, std::sync::atomic::Ordering::Relaxed);
}

/// The directory to load shaders from at runtime, if hot reload is enabled
/// and the directory exists.
pub(crate) fn hot_reload_dir() -> Option<PathBuf> {
    if !cfg!(all(feature = "hot-reload", debug_assertions))
        || HOT_RELOAD_OFF.load(std::sync::atomic::Ordering::Relaxed)
    {
        return None;
    }
    let dir = std::env::var_os("BOX_SHADER_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders"));
    dir.is_dir().then_some(dir)
}

/// Shader sources plus an optional on-disk directory and file watcher.
pub(crate) struct ShaderLibrary {
    dir: Option<PathBuf>,
    watcher: Option<watch::Watcher>,
}

impl ShaderLibrary {
    /// Loads the initial shaders: from `dir` when given and valid, otherwise
    /// the embedded copies.
    pub(crate) fn new(dir: Option<PathBuf>) -> (Self, ProcessedShaders) {
        let embedded = || {
            ShaderSources::embedded()
                .build()
                .expect("embedded engine shaders are validated by unit tests")
        };
        let processed = match &dir {
            Some(d) => match ShaderSources::load_dir(d).and_then(|s| s.build()) {
                Ok(p) => {
                    tracing::info!("shaders loaded from {} (hot reload on)", d.display());
                    p
                }
                Err(e) => {
                    tracing::error!("{e}\nfalling back to embedded shaders");
                    embedded()
                }
            },
            None => embedded(),
        };
        let watcher = dir.as_deref().and_then(watch::Watcher::new);
        (Self { dir, watcher }, processed)
    }

    /// Returns true once a burst of file changes has settled.
    pub(crate) fn poll_changed(&mut self) -> bool {
        self.watcher.as_mut().is_some_and(|w| w.poll())
    }

    /// Re-reads, preprocesses and validates every shader from disk.
    pub(crate) fn reload(&self) -> Result<ProcessedShaders, String> {
        let dir = self
            .dir
            .as_deref()
            .ok_or_else(|| "shader hot reload is not enabled".to_string())?;
        ShaderSources::load_dir(dir)?.build()
    }

    #[cfg(test)]
    pub(crate) fn set_dir(&mut self, dir: PathBuf) {
        self.dir = Some(dir);
    }
}

#[cfg(all(feature = "hot-reload", debug_assertions))]
mod watch {
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use notify::Watcher as _;

    const SETTLE: Duration = Duration::from_millis(120);

    pub(crate) struct Watcher {
        _watcher: notify::RecommendedWatcher,
        rx: mpsc::Receiver<notify::Result<notify::Event>>,
        last_event: Option<Instant>,
    }

    impl Watcher {
        pub(crate) fn new(dir: &Path) -> Option<Self> {
            let (tx, rx) = mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("shader watcher unavailable: {e}");
                    return None;
                }
            };
            if let Err(e) = watcher.watch(dir, notify::RecursiveMode::NonRecursive) {
                tracing::warn!("cannot watch {}: {e}", dir.display());
                return None;
            }
            Some(Self {
                _watcher: watcher,
                rx,
                last_event: None,
            })
        }

        pub(crate) fn poll(&mut self) -> bool {
            while let Ok(event) = self.rx.try_recv() {
                let relevant = event.is_ok_and(|e| {
                    !e.kind.is_access()
                        && e.paths
                            .iter()
                            .any(|p| p.extension().is_some_and(|x| x == "wgsl"))
                });
                if relevant {
                    self.last_event = Some(Instant::now());
                }
            }
            match self.last_event {
                Some(t) if t.elapsed() >= SETTLE => {
                    self.last_event = None;
                    true
                }
                _ => false,
            }
        }
    }
}

#[cfg(not(all(feature = "hot-reload", debug_assertions)))]
mod watch {
    pub(crate) struct Watcher;

    impl Watcher {
        pub(crate) fn new(_dir: &std::path::Path) -> Option<Self> {
            None
        }

        pub(crate) fn poll(&mut self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_shader_preprocesses_and_validates() {
        let processed = ShaderSources::embedded().build().unwrap_or_else(|e| {
            panic!("{e}");
        });
        for name in TOP_LEVEL {
            assert!(!processed.get(name).contains("#include"));
        }
    }

    #[test]
    fn on_disk_shaders_match_embedded() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");
        let disk = ShaderSources::load_dir(&dir).unwrap();
        let embedded = ShaderSources::embedded();
        assert_eq!(disk.files, embedded.files);
    }

    #[test]
    fn includes_are_resolved_once() {
        let mut s = ShaderSources {
            files: HashMap::new(),
        };
        s.set(
            "a.wgsl",
            "#include \"b.wgsl\"\n#include \"c.wgsl\"\nfn a() {}",
        );
        s.set("b.wgsl", "#include \"c.wgsl\"\nfn b() {}");
        s.set("c.wgsl", "#include \"a.wgsl\"\nfn c() {}");
        let out = s.preprocess("a.wgsl").unwrap();
        assert_eq!(out.matches("fn c()").count(), 1);
        assert_eq!(out, "fn c() {}\nfn b() {}\nfn a() {}\n");
    }

    #[test]
    fn missing_include_is_reported() {
        let mut s = ShaderSources {
            files: HashMap::new(),
        };
        s.set("a.wgsl", "#include \"nope.wgsl\"\n");
        let err = s.preprocess("a.wgsl").unwrap_err();
        assert!(err.contains("a.wgsl") && err.contains("nope.wgsl"), "{err}");
        s.set("a.wgsl", "#include nope.wgsl\n");
        assert!(s.preprocess("a.wgsl").unwrap_err().contains("malformed"));
    }

    #[test]
    fn broken_shader_gives_readable_error() {
        let mut s = ShaderSources::embedded();
        let broken = ShaderSources::embedded().files["sky.wgsl"].replace(
            "return vec4<f32>(sky_atmosphere(sky_radiance(rd), rd), 1.0);",
            "return vec4<f32>(sky_atmosphere(sky_radiance(rd), rd), 1.0)",
        );
        s.set("sky.wgsl", &broken);
        let err = s.build().unwrap_err();
        assert!(err.contains("sky.wgsl"), "{err}");
        assert!(err.contains("expected"), "{err}");
        // Type errors are caught by validation, not only parsing.
        let mut s = ShaderSources::embedded();
        let bad_type = ShaderSources::embedded().files["mesh.wgsl"]
            .replace("var shadow = 1.0;", "var shadow = vec2<f32>(1.0);");
        s.set("mesh.wgsl", &bad_type);
        let err = s.build().unwrap_err();
        assert!(err.contains("mesh.wgsl"), "{err}");
    }

    #[test]
    fn reload_failure_keeps_nothing_partial() {
        let dir = std::env::temp_dir().join(format!("box_render_reload_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in EMBEDDED {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let (lib, _) = ShaderLibrary::new(Some(dir.clone()));
        assert!(lib.reload().is_ok());
        std::fs::write(dir.join("fog.wgsl"), "fn broken( {").unwrap();
        let err = lib.reload().unwrap_err();
        assert!(err.contains("failed to parse"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
