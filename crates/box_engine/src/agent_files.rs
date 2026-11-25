//! `read_asset` and `write_asset` (`box mcp`): text files under a project's
//! `assets/` folder, for agents that have no other file access (the
//! editor's Assistant panel runs its agent with only `box mcp`).
//!
//! Writes are journalled so the editor can undo them with the assistant
//! turn that made them: before a file is replaced its old bytes are copied
//! to `.box/assistant/backups/<seq>`, and a line `{seq, path, backup}` is
//! appended to `.box/assistant/journal.jsonl` (`backup` is null for a file
//! that did not exist). [`restore`] puts a journal entry back.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde_json::{Value as Json, json};

/// Extensions that may be read and written.
pub const TEXT_EXTENSIONS: &[&str] = &["luau", "json", "wgsl", "md", "txt"];
/// Largest file read or written.
pub const MAX_BYTES: usize = 1024 * 1024;
/// The journal, relative to the project root.
pub const JOURNAL: &str = ".box/assistant/journal.jsonl";
const BACKUPS: &str = ".box/assistant/backups";

/// A failed read or write: an error code and a message for the agent.
#[derive(Debug)]
pub struct FileError {
    pub code: &'static str,
    pub message: String,
}

impl FileError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn to_json(&self) -> Json {
        json!({ "error": { "code": self.code, "message": self.message } })
    }
}

/// `path` (relative to `assets/`, or starting with `assets/`) as a file
/// inside the project's assets folder, with an allowed extension.
pub fn resolve(project: &Path, path: &str) -> Result<(PathBuf, String), FileError> {
    let rel = path.trim().trim_start_matches("./");
    let rel = rel.strip_prefix("assets/").unwrap_or(rel);
    let p = Path::new(rel);
    if rel.is_empty()
        || p.is_absolute()
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(FileError::new(
            "invalid_path",
            format!("`{path}`: give a path inside assets/, such as scripts/door.luau"),
        ));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !TEXT_EXTENSIONS.contains(&ext.as_str()) {
        return Err(FileError::new(
            "invalid_path",
            format!(
                "`{path}`: only text files can be read or written here ({})",
                TEXT_EXTENSIONS.join(", ")
            ),
        ));
    }
    let shown = format!("assets/{}", rel.replace('\\', "/"));
    Ok((project.join("assets").join(rel), shown))
}

/// `read_asset`: the file's text.
pub fn read(project: &Path, path: &str) -> Result<Json, FileError> {
    let (file, shown) = resolve(project, path)?;
    let bytes = std::fs::read(&file).map_err(|e| {
        FileError::new(
            "not_found",
            format!("{shown}: {e} (list the folder with scene_list or check the name)"),
        )
    })?;
    if bytes.len() > MAX_BYTES {
        return Err(FileError::new(
            "too_large",
            format!(
                "{shown} is {} bytes; at most {MAX_BYTES} are read",
                bytes.len()
            ),
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| FileError::new("not_text", format!("{shown} is not UTF-8 text")))?;
    Ok(json!({ "path": shown, "bytes": text.len(), "text": text }))
}

/// `write_asset`: writes `text`, after journalling what it replaces.
pub fn write(project: &Path, path: &str, text: &str) -> Result<Json, FileError> {
    let (file, shown) = resolve(project, path)?;
    if text.len() > MAX_BYTES {
        return Err(FileError::new(
            "too_large",
            format!("at most {MAX_BYTES} bytes are written"),
        ));
    }
    let io = |e: std::io::Error| FileError::new("io", format!("{shown}: {e}"));
    let seq = next_seq(project);
    let backup = match std::fs::read(&file) {
        Ok(old) => {
            let rel = format!("{BACKUPS}/{seq}");
            let dst = project.join(&rel);
            std::fs::create_dir_all(dst.parent().unwrap_or(project)).map_err(io)?;
            std::fs::write(&dst, old).map_err(io)?;
            Some(rel)
        }
        Err(_) => None,
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let created = backup.is_none();
    let entry = json!({ "seq": seq, "path": shown, "backup": backup });
    let journal = project.join(JOURNAL);
    if let Some(dir) = journal.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal)
        .map_err(io)?;
    writeln!(f, "{entry}").map_err(io)?;
    let tmp = file.with_extension("box-write-tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, &file).map_err(io)?;
    Ok(json!({ "path": shown, "bytes": text.len(), "created": created, "seq": seq }))
}

fn next_seq(project: &Path) -> u64 {
    journal_entries(project)
        .last()
        .and_then(|e| e["seq"].as_u64())
        .map_or(1, |s| s + 1)
}

/// Every journal entry, oldest first.
pub fn journal_entries(project: &Path) -> Vec<Json> {
    std::fs::read_to_string(project.join(JOURNAL))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Puts back what a journal entry replaced: the backup's bytes, or no file
/// when the write created it.
pub fn restore(project: &Path, entry: &Json) -> Result<(), String> {
    let path = entry["path"]
        .as_str()
        .ok_or("journal entry without a path")?;
    let file = project.join(path);
    match entry["backup"].as_str() {
        Some(b) => {
            let bytes = std::fs::read(project.join(b)).map_err(|e| format!("{b}: {e}"))?;
            std::fs::write(&file, bytes).map_err(|e| format!("{path}: {e}"))
        }
        None => match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{path}: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("box_asset_files_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/scripts")).unwrap();
        dir
    }

    #[test]
    fn paths_stay_inside_assets_and_are_text() {
        let p = project("paths");
        for bad in [
            "../box.project.json",
            "/etc/passwd",
            "scripts/../../x.luau",
            "models/a.glb",
            "",
        ] {
            assert_eq!(resolve(&p, bad).unwrap_err().code, "invalid_path", "{bad}");
        }
        let (file, shown) = resolve(&p, "assets/scripts/door.luau").unwrap();
        assert_eq!(shown, "assets/scripts/door.luau");
        assert_eq!(file, p.join("assets/scripts/door.luau"));
        assert_eq!(resolve(&p, "scripts/door.luau").unwrap().1, shown);
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn writes_are_journalled_and_restored() {
        let p = project("journal");
        std::fs::write(p.join("assets/scripts/a.luau"), "old").unwrap();
        let r = write(&p, "scripts/a.luau", "new").unwrap();
        assert_eq!(r["created"], false);
        let r = write(&p, "scripts/b.luau", "fresh").unwrap();
        assert_eq!(r["created"], true);
        assert_eq!(read(&p, "scripts/a.luau").unwrap()["text"], "new");
        let entries = journal_entries(&p);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["seq"], 2);
        for e in entries.iter().rev() {
            restore(&p, e).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(p.join("assets/scripts/a.luau")).unwrap(),
            "old"
        );
        assert!(!p.join("assets/scripts/b.luau").exists());
        let _ = std::fs::remove_dir_all(p);
    }
}
