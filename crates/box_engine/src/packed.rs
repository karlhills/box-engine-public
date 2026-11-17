//! Exported games: the data pack a player runs from (docs/EXPORT.md).
//!
//! `box export` writes the project into one [`PAK_FILE`] (a
//! [`box_assets::Pak`]): `box.project.json`, `assets/**` with their `.meta`
//! files, and an [`ExportManifest`] at [`MANIFEST_FILE`]. The player finds it
//! with [`find_bundled_pak`] (next to the executable, or in
//! `Contents/Resources` of a macOS app), mounts it ([`mount`],
//! [`box_assets::vfs`]) and opens the project at the pak's own path, so
//! scenes, settings, scripts and assets read from the pak through the same
//! paths a project folder uses. A mounted project is read-only: no hot
//! reload, no `.meta` writes, no registry file; save games go to
//! [`data_dir`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// File name of an exported game's data pack.
pub const PAK_FILE: &str = "game.pak";
/// The manifest inside the pak (at its root).
pub const MANIFEST_FILE: &str = "export.json";
/// Format version of [`ExportManifest`].
pub const MANIFEST_FORMAT: u32 = 1;

/// Build profile of an export.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Players for people: no GPU API validation, no console window on
    /// Windows, nothing written next to the game.
    #[default]
    Release,
    /// Players for the developer: GPU API validation (wgpu backend
    /// validation and debug labels) and a console window on Windows.
    Development,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::Release => "release",
            Profile::Development => "development",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "release" => Some(Profile::Release),
            "development" | "dev" => Some(Profile::Development),
            _ => None,
        }
    }
}

/// `export.json` inside a pak: what the export was made of and how the
/// player should behave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportManifest {
    pub format: u32,
    /// The application's display name.
    pub name: String,
    /// Reverse-DNS identifier (`com.example.my-game`): the macOS bundle id
    /// and the folder name of [`data_dir`].
    pub bundle_id: String,
    /// The game's version (`1.0.0`).
    pub version: String,
    pub profile: Profile,
    /// The engine version that cooked the pak.
    pub engine_version: String,
    /// `macos` or `windows`.
    pub target: String,
}

/// A mounted exported game.
#[derive(Clone, Debug)]
pub struct Packed {
    /// The pak file, which is also the project root (see the module docs).
    pub root: PathBuf,
    pub manifest: ExportManifest,
}

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// `game.pak` next to the running executable, or in `../Resources/` when the
/// executable is in a macOS app's `Contents/MacOS/`. Located from the
/// executable, never the working directory (an app opened from Finder runs
/// in `/`).
pub fn find_bundled_pak() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_pak_for(&exe)
}

/// [`find_bundled_pak`] for a given executable path.
pub fn bundled_pak_for(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join(PAK_FILE)];
    if dir.file_name().is_some_and(|n| n == "MacOS") {
        candidates.push(dir.parent()?.join("Resources").join(PAK_FILE));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// True for a path naming a pak file (`*.pak` on disk).
pub fn is_pak_path(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "pak") && path.is_file()
}

/// Opens and mounts the pak at `path` (checking its header and index),
/// reads its manifest and records [`data_dir`]. Returns the mounted game.
pub fn mount(path: &Path) -> anyhow::Result<Packed> {
    use anyhow::Context;
    let root = box_assets::vfs::mount_file(path)
        .with_context(|| format!("could not open the game data {}", path.display()))?;
    let manifest = read_manifest(&root)?;
    let _ = DATA_DIR.set(user_data_dir(&manifest.bundle_id));
    tracing::info!(
        "playing `{}` {} ({} build) from {}",
        manifest.name,
        manifest.version,
        manifest.profile.name(),
        path.display()
    );
    Ok(Packed { root, manifest })
}

/// The manifest of the pak mounted at `root`.
pub fn read_manifest(root: &Path) -> anyhow::Result<ExportManifest> {
    use anyhow::Context;
    let path = root.join(MANIFEST_FILE);
    let bytes = box_assets::vfs::read(&path).with_context(|| {
        format!(
            "{}: not an exported game (no {MANIFEST_FILE})",
            root.display()
        )
    })?;
    let manifest: ExportManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("{}: invalid {MANIFEST_FILE}", root.display()))?;
    if manifest.format > MANIFEST_FORMAT {
        anyhow::bail!(
            "{}: exported by a newer box-engine (manifest format {}); update the player",
            root.display(),
            manifest.format
        );
    }
    Ok(manifest)
}

/// Where a mounted game writes (save games): the per-user application data
/// folder named after its bundle id. `None` until a pak is mounted.
pub fn data_dir() -> Option<&'static Path> {
    DATA_DIR.get().map(PathBuf::as_path)
}

/// The per-user data folder for `id`: `~/Library/Application Support/<id>`
/// on macOS, `%APPDATA%\<id>` on Windows, `$XDG_DATA_HOME/<id>` (or
/// `~/.local/share/<id>`) elsewhere; `./<id>` when none is known.
pub fn user_data_dir(id: &str) -> PathBuf {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local/share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_pak_next_to_the_exe_or_in_an_app_bundle() {
        let dir =
            std::env::temp_dir().join(format!("box_packed_{}", box_core::identity::Uid::new()));
        let macos = dir.join("Game.app/Contents/MacOS");
        let res = dir.join("Game.app/Contents/Resources");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(&res).unwrap();
        assert_eq!(bundled_pak_for(&macos.join("game")), None);
        std::fs::write(res.join(PAK_FILE), b"x").unwrap();
        assert_eq!(
            bundled_pak_for(&macos.join("game")),
            Some(res.join(PAK_FILE))
        );
        std::fs::write(dir.join(PAK_FILE), b"x").unwrap();
        assert_eq!(
            bundled_pak_for(&dir.join("game.exe")),
            Some(dir.join(PAK_FILE))
        );
        assert!(is_pak_path(&dir.join(PAK_FILE)));
        assert!(!is_pak_path(&dir.join("nope.pak")));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(Profile::parse("dev"), Some(Profile::Development));
        assert_eq!(Profile::parse("release").unwrap().name(), "release");
        assert!(user_data_dir("com.example.x").ends_with("com.example.x"));
    }
}
