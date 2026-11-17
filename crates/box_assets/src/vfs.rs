//! A small virtual file system: [`Pak`] files mounted at their own path,
//! everything else on disk.
//!
//! An exported game is one `game.pak`. Mounting it (`vfs::mount`) makes the
//! paths below the pak file's own path readable: with the pak at
//! `/Games/Keep.app/Contents/Resources/game.pak`, the project file is
//! `/Games/Keep.app/Contents/Resources/game.pak/box.project.json` and the
//! start scene `.../game.pak/assets/scenes/main.scene.json`. Code that reads
//! project files uses [`read`], [`is_file`], [`is_dir`] and [`read_dir`]
//! instead of `std::fs`, so the same paths work for a project folder and a
//! pak. Mounted paths are read-only: nothing writes into a pak, and
//! [`is_read_only`] tells writers (the asset server's `.meta` creation,
//! settings file creation) to skip.
//!
//! With nothing mounted, every call is a plain `std::fs` call after one
//! atomic load.

use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::pak::{Pak, PakError};

static ANY_MOUNTED: AtomicBool = AtomicBool::new(false);
static MOUNTS: RwLock<Vec<(PathBuf, Arc<Pak>)>> = RwLock::new(Vec::new());

/// `path` made absolute (against the working directory) with `.` and `..`
/// resolved lexically.
fn absolute(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Mounts `pak` at its own path (made absolute) and returns that root.
/// Mounting the same path again replaces the earlier pak.
pub fn mount(pak: Pak) -> PathBuf {
    let root = absolute(pak.path());
    let mut mounts = MOUNTS.write().unwrap_or_else(PoisonError::into_inner);
    mounts.retain(|(r, _)| *r != root);
    mounts.push((root.clone(), Arc::new(pak)));
    ANY_MOUNTED.store(true, Ordering::Release);
    root
}

/// Opens the pak at `path` and mounts it; returns the mount root.
pub fn mount_file(path: impl AsRef<Path>) -> Result<PathBuf, PakError> {
    Ok(mount(Pak::open(path)?))
}

/// Unmounts the pak mounted at `root`. True if one was.
pub fn unmount(root: &Path) -> bool {
    let root = absolute(root);
    let mut mounts = MOUNTS.write().unwrap_or_else(PoisonError::into_inner);
    let before = mounts.len();
    mounts.retain(|(r, _)| *r != root);
    ANY_MOUNTED.store(!mounts.is_empty(), Ordering::Release);
    mounts.len() != before
}

/// The pak a path is inside, and the path relative to the pak (`""` for
/// the mount root itself). `None` for paths on disk.
pub fn resolve(path: &Path) -> Option<(Arc<Pak>, String)> {
    if !ANY_MOUNTED.load(Ordering::Acquire) {
        return None;
    }
    let abs = absolute(path);
    let mounts = MOUNTS.read().unwrap_or_else(PoisonError::into_inner);
    mounts.iter().find_map(|(root, pak)| {
        let rest = abs.strip_prefix(root).ok()?;
        let rel: Vec<String> = rest
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some((pak.clone(), rel.join("/")))
    })
}

/// True when `path` is inside a mounted pak (or is its root).
pub fn is_mounted(path: &Path) -> bool {
    resolve(path).is_some()
}

/// True when writing to `path` cannot work: it is inside a mounted pak.
pub fn is_read_only(path: &Path) -> bool {
    is_mounted(path)
}

/// The mount root a path is inside.
pub fn mount_root(path: &Path) -> Option<PathBuf> {
    if !ANY_MOUNTED.load(Ordering::Acquire) {
        return None;
    }
    let abs = absolute(path);
    let mounts = MOUNTS.read().unwrap_or_else(PoisonError::into_inner);
    mounts
        .iter()
        .find(|(root, _)| abs.starts_with(root))
        .map(|(root, _)| root.clone())
}

fn pak_error(e: PakError) -> io::Error {
    match e {
        PakError::NotFound(p) => {
            io::Error::new(io::ErrorKind::NotFound, format!("{p}: not in the pak"))
        }
        PakError::Io { source, .. } => source,
        other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
    }
}

/// [`std::fs::read`] that also reads files inside mounted paks.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    match resolve(path) {
        Some((pak, rel)) => pak.read(&rel).map_err(pak_error),
        None => std::fs::read(path),
    }
}

/// [`std::fs::read_to_string`] that also reads files inside mounted paks.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// [`Path::is_file`] that also sees files inside mounted paks.
pub fn is_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match resolve(path) {
        Some((pak, rel)) => pak.contains(&rel),
        None => path.is_file(),
    }
}

/// [`Path::is_dir`] that also sees folders inside mounted paks (the mount
/// root is a folder).
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match resolve(path) {
        Some((pak, rel)) => pak.is_dir(&rel),
        None => path.is_dir(),
    }
}

/// True for a file or a folder.
pub fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    is_file(path) || is_dir(path)
}

/// Size of a file, if it exists.
pub fn file_len(path: impl AsRef<Path>) -> Option<u64> {
    let path = path.as_ref();
    match resolve(path) {
        Some((pak, rel)) => pak.entry(&rel).map(|e| e.len),
        None => std::fs::metadata(path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len()),
    }
}

/// One entry of [`read_dir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// The entries of a folder, on disk or in a mounted pak, sorted by name.
/// Entries whose type cannot be read are skipped.
pub fn read_dir(path: impl AsRef<Path>) -> io::Result<Vec<DirEntry>> {
    let path = path.as_ref();
    if let Some((pak, rel)) = resolve(path) {
        if !pak.is_dir(&rel) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}: no such folder in the pak", path.display()),
            ));
        }
        return Ok(pak
            .list(&rel)
            .into_iter()
            .map(|(name, is_dir)| DirEntry {
                path: path.join(&name),
                name,
                is_dir,
            })
            .collect());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let Ok(ty) = entry.file_type() else { continue };
        out.push(DirEntry {
            path: entry.path(),
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir: ty.is_dir(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pak::PakWriter;

    #[test]
    fn mounted_paths_read_from_the_pak() {
        let dir = std::env::temp_dir().join(format!("box_vfs_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("disk.txt"), b"disk").unwrap();
        let mut w = PakWriter::new();
        w.add_bytes("assets/a.txt", b"pak".to_vec()).unwrap();
        w.add_bytes("box.project.json", b"{}".to_vec()).unwrap();
        let pak_path = dir.join("game.pak");
        w.write(&pak_path).unwrap();
        let root = mount_file(&pak_path).unwrap();
        assert_eq!(root, absolute(&pak_path));
        assert_eq!(read(root.join("assets/a.txt")).unwrap(), b"pak");
        assert_eq!(read(root.join("assets/./x/../a.txt")).unwrap(), b"pak");
        assert_eq!(read_to_string(dir.join("disk.txt")).unwrap(), "disk");
        assert!(is_dir(&root) && is_dir(root.join("assets")));
        assert!(is_file(root.join("box.project.json")));
        assert!(!is_file(root.join("assets")) && !exists(root.join("nope")));
        assert_eq!(
            read(root.join("nope")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(is_read_only(&root.join("assets/new.meta")));
        assert!(!is_read_only(&dir.join("disk.txt")));
        assert_eq!(file_len(root.join("assets/a.txt")), Some(3));
        let names: Vec<_> = read_dir(&root)
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.is_dir))
            .collect();
        assert_eq!(
            names,
            [
                ("assets".to_string(), true),
                ("box.project.json".into(), false)
            ]
        );
        let disk: Vec<_> = read_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(disk, ["disk.txt", "game.pak"]);
        assert_eq!(mount_root(&root.join("assets/a.txt")), Some(root.clone()));
        assert!(unmount(&root));
        assert!(!is_file(root.join("box.project.json")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
