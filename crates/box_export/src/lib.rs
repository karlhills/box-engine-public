//! Exporting games (docs/EXPORT.md, docs/ASSETS.md section 7): a
//! project becomes a macOS `.app` or a Windows folder that runs without
//! box-engine or Rust installed.
//!
//! 1. **Player.** A script project uses the prebuilt `box-player` template
//!    for the target ([`template`]); a native project (with a `Cargo.toml`)
//!    builds its own player with `cargo build --release`.
//! 2. **Cook** ([`cook`]): every asset is imported to check it, then
//!    `box.project.json`, `assets/**` (with `.meta` files) and an
//!    [`ExportManifest`] are written into one `game.pak`
//!    ([`box_engine::box_assets::pak`]).
//! 3. **Bundle**: [`macos`] writes `<Name>.app` (Info.plist, icon, the pak
//!    in `Contents/Resources`) and signs it ad-hoc; [`windows`] writes
//!    `<Name>/<Name>.exe` with icon, version and subsystem patched, and
//!    `game.pak` beside it.
//!
//! The player finds the pak next to itself and plays it
//! ([`box_engine::packed`]). `box export` and the editor's File ▸ Export…
//! both call [`export`].
//!
//! ```no_run
//! use box_export::{ExportOptions, Target};
//! use box_engine::packed::Profile;
//! let options = ExportOptions::new("my-game", Target::MacOs, Profile::Release, "my-game/export");
//! let report = box_export::export(&options, &mut |p| println!("{}", p.message))?;
//! println!("{}", report.output.display());
//! # Ok::<(), box_export::ExportError>(())
//! ```

pub mod cook;
pub mod macos;
pub mod template;
pub mod windows;

use std::path::{Path, PathBuf};
use std::time::Instant;

use box_engine::box_assets::PakError;
use box_engine::packed::{ExportManifest, MANIFEST_FORMAT, PAK_FILE, Profile};
use box_engine::player::NativeCrate;
use box_engine::project::{ENGINE_VERSION, ExportSettings};
use box_engine::{Project, ProjectError};
use image::DynamicImage;
use serde::Serialize;

pub use cook::CookReport;

/// The engine's own icon, used when a project sets none.
pub const DEFAULT_ICON_PNG: &[u8] = include_bytes!("../../../apps/editor/macos/AppIcon.png");

/// The default version of an exported game.
pub const DEFAULT_VERSION: &str = "1.0.0";

/// Platform an export runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    #[serde(rename = "macos")]
    MacOs,
    Windows,
}

impl Target {
    /// `macos` or `windows`.
    pub fn name(self) -> &'static str {
        match self {
            Target::MacOs => "macos",
            Target::Windows => "windows",
        }
    }

    /// `macOS` or `Windows`.
    pub fn label(self) -> &'static str {
        match self {
            Target::MacOs => "macOS",
            Target::Windows => "Windows",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "macos" | "mac" | "osx" => Some(Target::MacOs),
            "windows" | "win" => Some(Target::Windows),
            _ => None,
        }
    }

    /// The platform this program runs on (macOS for anything but Windows).
    pub fn host() -> Self {
        if cfg!(windows) {
            Target::Windows
        } else {
            Target::MacOs
        }
    }

    pub const ALL: [Target; 2] = [Target::MacOs, Target::Windows];
}

/// What to export and where.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// The project folder (or any path inside it).
    pub project: PathBuf,
    pub target: Target,
    pub profile: Profile,
    /// Folder the bundle is written into (created if missing).
    pub out: PathBuf,
    /// The player executable to bundle instead of the template (script
    /// project) or the cargo build (native project).
    pub player: Option<PathBuf>,
    /// Sign a macOS bundle ad-hoc (default on).
    pub sign: bool,
}

impl ExportOptions {
    pub fn new(
        project: impl Into<PathBuf>,
        target: Target,
        profile: Profile,
        out: impl Into<PathBuf>,
    ) -> Self {
        Self {
            project: project.into(),
            target,
            profile,
            out: out.into(),
            player: None,
            sign: true,
        }
    }
}

/// One progress report of [`export`].
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// `build`, `import`, `pak`, `bundle`, `sign` or `done`.
    pub step: &'static str,
    pub message: String,
    /// 0 to 1.
    pub fraction: f32,
}

impl Progress {
    pub fn new(step: &'static str, message: impl Into<String>, fraction: f32) -> Self {
        Self {
            step,
            message: message.into(),
            fraction,
        }
    }
}

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The project's export settings are invalid.
    #[error("export settings: {0}")]
    Settings(String),
    /// Assets that do not import, a missing start scene.
    #[error("the project has {} problem(s):\n  {}", problems.len(), problems.join("\n  "))]
    Cook { problems: Vec<String> },
    #[error(transparent)]
    Pak(#[from] PakError),
    /// No usable player template.
    #[error("{0}")]
    Template(String),
    /// Building a native project's player failed.
    #[error("{0}")]
    Cargo(String),
    #[error("{0}")]
    Bundle(String),
    #[error("signing: {0}")]
    Sign(String),
    /// The output exists and is not an earlier export.
    #[error("{}: already exists and is not an exported game; choose another output folder", .0.display())]
    OutputExists(PathBuf),
}

impl ExportError {
    /// A short machine-readable code for `--json` results.
    pub fn code(&self) -> &'static str {
        match self {
            ExportError::Project(_) => "project",
            ExportError::Io { .. } => "io",
            ExportError::Settings(_) => "export_settings",
            ExportError::Cook { .. } => "cook_failed",
            ExportError::Pak(_) => "pak",
            ExportError::Template(_) => "no_template",
            ExportError::Cargo(_) => "cargo_failed",
            ExportError::Bundle(_) => "bundle",
            ExportError::Sign(_) => "sign_failed",
            ExportError::OutputExists(_) => "output_exists",
        }
    }
}

/// The exported application's identity, from the project's
/// [`ExportSettings`] with defaults filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppIdentity {
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    /// The icon PNG; `None` is the engine's icon.
    pub icon: Option<PathBuf>,
}

/// `name` as a file name every platform accepts (`Cinder Keep` stays,
/// `a/b: c` becomes `a-b- c`).
pub fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"];
    match trimmed {
        "" => "Game".to_string(),
        t if reserved.contains(&t.to_ascii_uppercase().as_str()) => format!("{t}-game"),
        t => t.to_string(),
    }
}

/// `com.example.<name>` with the name lowercased to letters, digits and `-`.
pub fn default_bundle_id(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    format!(
        "com.example.{}",
        if slug.is_empty() { "game" } else { slug }
    )
}

/// Problems with export settings as `(field, message)`; empty when valid.
/// The editor shows them next to the fields.
pub fn settings_problems(project_root: &Path, s: &ExportSettings) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(name) = &s.app_name
        && (name.trim().is_empty() || name.chars().count() > 64)
    {
        out.push(("app_name", "must be 1 to 64 characters".to_string()));
    }
    if let Some(id) = &s.bundle_id {
        let parts: Vec<&str> = id.split('.').collect();
        let ok = parts.len() >= 2
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        if !ok {
            out.push((
                "bundle_id",
                format!(
                    "`{id}`: use reverse-DNS form, letters, digits and `-` (com.example.my-game)"
                ),
            ));
        }
    }
    if let Some(v) = &s.version {
        let parts: Vec<&str> = v.split('.').collect();
        let ok = (1..=3).contains(&parts.len())
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 5 && p.parse::<u16>().is_ok());
        if !ok {
            out.push((
                "version",
                format!("`{v}`: use one to three numbers 0-65535 separated by dots (1.2.0)"),
            ));
        }
    }
    if let Some(icon) = &s.icon {
        let path = project_root.join(icon);
        match std::fs::read(&path) {
            Err(e) => out.push(("icon", format!("{icon}: {e}"))),
            Ok(bytes) => match image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            {
                Err(e) => out.push(("icon", format!("{icon}: not a PNG image ({e})"))),
                Ok(img) if img.width() != img.height() => out.push((
                    "icon",
                    format!(
                        "{icon}: must be square (it is {}x{})",
                        img.width(),
                        img.height()
                    ),
                )),
                Ok(_) => {}
            },
        }
    }
    out
}

/// The identity of `project`'s exported app (defaults filled in), or the
/// settings problems.
pub fn identity(project: &Project) -> Result<AppIdentity, ExportError> {
    let s = &project.file().export;
    let problems = settings_problems(project.root(), s);
    if !problems.is_empty() {
        return Err(ExportError::Settings(
            problems
                .iter()
                .map(|(f, m)| format!("{f}: {m}"))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let name = s
        .app_name
        .clone()
        .unwrap_or_else(|| project.name().to_string())
        .trim()
        .to_string();
    Ok(AppIdentity {
        bundle_id: s
            .bundle_id
            .clone()
            .unwrap_or_else(|| default_bundle_id(&name)),
        version: s.version.clone().unwrap_or_else(|| DEFAULT_VERSION.into()),
        icon: s.icon.as_ref().map(|i| project.root().join(i)),
        name,
    })
}

fn load_icon(id: &AppIdentity) -> Result<DynamicImage, ExportError> {
    let bytes = match &id.icon {
        Some(path) => std::fs::read(path).map_err(|source| ExportError::Io {
            path: path.clone(),
            source,
        })?,
        None => DEFAULT_ICON_PNG.to_vec(),
    };
    image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|e| ExportError::Settings(format!("icon: {e}")))
}

/// What [`export`] made.
#[derive(Clone, Debug, Serialize)]
pub struct ExportReport {
    pub project: PathBuf,
    pub target: Target,
    pub profile: Profile,
    /// The `.app` (macOS) or the folder (Windows).
    pub output: PathBuf,
    /// The bundled player executable.
    pub executable: PathBuf,
    pub pak: PathBuf,
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    /// The game shows the startup splash (`export.splash` is not `false`).
    pub splash: bool,
    /// Where the player came from: `template` or `cargo` (or `given`).
    pub player_source: &'static str,
    pub player: PathBuf,
    /// The macOS bundle is ad-hoc signed.
    pub signed: bool,
    pub cook: CookReport,
    /// Milliseconds: building a native player, bundling, signing, total.
    pub build_ms: f64,
    pub bundle_ms: f64,
    pub sign_ms: f64,
    pub total_ms: f64,
    pub warnings: Vec<String>,
}

/// The bundle path `export` writes for `name` in `out`.
pub fn output_path(out: &Path, target: Target, name: &str) -> PathBuf {
    match target {
        Target::MacOs => out.join(format!("{}.app", file_name(name))),
        Target::Windows => out.join(file_name(name)),
    }
}

/// The pak inside an export's output.
pub fn pak_path(output: &Path, target: Target) -> PathBuf {
    match target {
        Target::MacOs => output.join("Contents/Resources").join(PAK_FILE),
        Target::Windows => output.join(PAK_FILE),
    }
}

/// Removes an earlier export at `output` so it can be replaced; refuses
/// anything else.
fn clear_output(output: &Path, target: Target) -> Result<(), ExportError> {
    if !output.exists() {
        return Ok(());
    }
    let ours = pak_path(output, target).is_file()
        || (output.is_dir() && std::fs::read_dir(output).is_ok_and(|mut d| d.next().is_none()));
    if !ours {
        return Err(ExportError::OutputExists(output.to_path_buf()));
    }
    std::fs::remove_dir_all(output).map_err(|source| ExportError::Io {
        path: output.to_path_buf(),
        source,
    })
}

/// Exports a project (see the crate docs). `progress` is called from this
/// thread as the export goes.
pub fn export(
    options: &ExportOptions,
    progress: &mut dyn FnMut(Progress),
) -> Result<ExportReport, ExportError> {
    let started = Instant::now();
    let project = Project::open(&options.project)?;
    if project.is_packed() {
        return Err(ExportError::Settings(
            "cannot export an exported game; export its project folder".into(),
        ));
    }
    let id = identity(&project)?;
    let icon = load_icon(&id)?;
    let mut warnings = Vec::new();

    // 1. The player.
    let build_start = Instant::now();
    let native = NativeCrate::find(project.root());
    let (player, player_source) = match (&options.player, &native) {
        (Some(p), _) => (p.clone(), "given"),
        (None, Some(native)) => (
            template::build_native_player(&project, native, options.target, progress)?,
            "cargo",
        ),
        (None, None) => (template::find_template(options.target)?, "template"),
    };
    let build_ms = build_start.elapsed().as_secs_f64() * 1000.0;
    if !player.is_file() {
        return Err(ExportError::Template(format!(
            "{}: player executable not found",
            player.display()
        )));
    }

    // 2. Cook into a staging folder next to the output.
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ExportError::Io { path, source }
    };
    std::fs::create_dir_all(&options.out).map_err(io(&options.out))?;
    let output = output_path(&options.out, options.target, &id.name);
    let staging = options.out.join(format!(
        ".{}.export-{}",
        file_name(&id.name),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(io(&staging))?;
    let result = (|| {
        let manifest = ExportManifest {
            format: MANIFEST_FORMAT,
            name: id.name.clone(),
            bundle_id: id.bundle_id.clone(),
            version: id.version.clone(),
            profile: options.profile,
            engine_version: ENGINE_VERSION.to_string(),
            target: options.target.name().to_string(),
        };
        let staged_pak = staging.join(PAK_FILE);
        let cook = cook::cook(&project, &manifest, &staged_pak, progress)?;

        // 3. Bundle.
        progress(Progress::new(
            "bundle",
            format!("Bundling for {}", options.target.label()),
            0.8,
        ));
        let bundle_start = Instant::now();
        let staged_output = staging.join(output.file_name().unwrap_or_default());
        let exe_name;
        match options.target {
            Target::MacOs => {
                exe_name = file_name(&id.name);
                macos::write_bundle(&staged_output, &id, &player, &exe_name, &icon)?;
            }
            Target::Windows => {
                exe_name = format!("{}.exe", file_name(&id.name));
                let bytes = std::fs::read(&player).map_err(io(&player))?;
                let patched =
                    windows::patch_executable(&bytes, &id, &icon, &exe_name, options.profile)?;
                std::fs::create_dir_all(&staged_output).map_err(io(&staged_output))?;
                let exe = staged_output.join(&exe_name);
                std::fs::write(&exe, patched).map_err(io(&exe))?;
            }
        }
        let pak_in_bundle = pak_path(&staged_output, options.target);
        std::fs::rename(&staged_pak, &pak_in_bundle).map_err(io(&pak_in_bundle))?;
        let bundle_ms = bundle_start.elapsed().as_secs_f64() * 1000.0;

        // 4. Sign (macOS), last: the signature seals the bundle.
        let sign_start = Instant::now();
        let mut signed = false;
        if options.target == Target::MacOs {
            if options.sign {
                progress(Progress::new("sign", "Signing (ad-hoc)", 0.9));
                match macos::sign(&staged_output)? {
                    macos::Signed::Yes => signed = true,
                    macos::Signed::No(reason) => warnings.push(reason),
                }
            } else {
                warnings
                    .push("not signed (--no-sign): Apple Silicon Macs run only signed apps".into());
            }
        }
        let sign_ms = sign_start.elapsed().as_secs_f64() * 1000.0;

        // 5. Replace the previous export.
        clear_output(&output, options.target)?;
        std::fs::rename(&staged_output, &output).map_err(io(&output))?;
        let executable = match options.target {
            Target::MacOs => output.join("Contents/MacOS").join(&exe_name),
            Target::Windows => output.join(&exe_name),
        };
        Ok::<_, ExportError>((cook, bundle_ms, sign_ms, signed, executable))
    })();
    let _ = std::fs::remove_dir_all(&staging);
    let (cook, bundle_ms, sign_ms, signed, executable) = result?;
    let report = ExportReport {
        project: project.root().to_path_buf(),
        target: options.target,
        profile: options.profile,
        pak: pak_path(&output, options.target),
        output,
        executable,
        name: id.name,
        bundle_id: id.bundle_id,
        version: id.version,
        splash: project.file().export.splash_enabled(),
        player_source,
        player,
        signed,
        cook,
        build_ms,
        bundle_ms,
        sign_ms,
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        warnings,
    };
    progress(Progress::new(
        "done",
        format!("Exported {}", report.output.display()),
        1.0,
    ));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_ids_and_settings() {
        assert_eq!(file_name("Cinder Keep"), "Cinder Keep");
        assert_eq!(file_name("a/b: c?"), "a-b- c-");
        assert_eq!(file_name(" .. "), "Game");
        assert_eq!(file_name("con"), "con-game");
        assert_eq!(
            default_bundle_id("Collector (Luau)"),
            "com.example.collector-luau"
        );
        assert_eq!(default_bundle_id("!!"), "com.example.game");
        let root = std::env::temp_dir();
        let bad = ExportSettings {
            app_name: Some(" ".into()),
            bundle_id: Some("nodots".into()),
            version: Some("1.2.3.4".into()),
            icon: Some("missing-icon-file.png".into()),
            splash: Some(false),
        };
        let fields: Vec<&str> = settings_problems(&root, &bad).iter().map(|p| p.0).collect();
        assert_eq!(fields, ["app_name", "bundle_id", "version", "icon"]);
        let good = ExportSettings {
            app_name: Some("Keep".into()),
            bundle_id: Some("com.example.keep-2".into()),
            version: Some("1.20.300".into()),
            icon: None,
            splash: None,
        };
        assert!(settings_problems(&root, &good).is_empty());
        assert_eq!(Target::parse("Windows"), Some(Target::Windows));
        assert_eq!(Target::parse("mac"), Some(Target::MacOs));
        assert!(image::load_from_memory(DEFAULT_ICON_PNG).is_ok());
    }
}
