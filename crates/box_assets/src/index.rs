//! The id ↔ path index of the assets folder, built from `.meta` sidecars.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use box_core::asset_ref::AssetId;

use crate::importer::ImporterSet;
use crate::meta::{self, META_SUFFIX, Meta};

/// Normalizes a path relative to the assets folder: `/` separators, `.` and
/// `..` resolved. `None` for an empty path, an absolute path, a drive prefix
/// or a `..` that leaves the folder.
pub fn normalize_rel(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    if path.starts_with('/') || path.as_bytes().get(1) == Some(&b':') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// True for names the server never treats as assets: hidden files and
/// folders (`.git`, `.DS_Store`, temporary `.x.tmp` files).
pub(crate) fn is_hidden(rel: &str) -> bool {
    rel.split('/').any(|p| p.starts_with('.'))
}

/// What a refresh of one path found.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Change {
    /// Known file at its known path (its content or settings may have changed).
    Unchanged { id: AssetId },
    /// A file that was not indexed.
    New { id: AssetId },
    /// A known id found at a new path; the old path is gone.
    Moved { id: AssetId, from: String },
    /// The indexed file is gone (or its `.meta` became unreadable).
    Removed { id: AssetId, path: String },
}

#[derive(Default)]
pub(crate) struct Index {
    root: PathBuf,
    by_id: HashMap<AssetId, String>,
    by_path: HashMap<String, AssetId>,
    /// Files whose `.meta` cannot be used: path → message.
    broken: HashMap<String, String>,
    /// Bumped whenever the set of indexed (or broken) files or their paths
    /// change, so UIs can rebuild listings only when needed.
    generation: u64,
    /// Files whose `.meta` this index created (default settings nobody chose).
    created_metas: HashSet<String>,
}

impl Index {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            ..Default::default()
        }
    }

    /// True when this index created `rel`'s `.meta` (it had none), so its
    /// settings are the importer's defaults rather than someone's choice.
    pub fn created_meta(&self, rel: &str) -> bool {
        self.created_metas.contains(rel)
    }

    pub fn path_of(&self, id: AssetId) -> Option<&str> {
        self.by_id.get(&id).map(String::as_str)
    }

    pub fn id_of(&self, path: &str) -> Option<AssetId> {
        self.by_path.get(path).copied()
    }

    pub fn broken(&self, path: &str) -> Option<&str> {
        self.broken.get(path).map(String::as_str)
    }

    /// Files whose `.meta` cannot be used, as `(path, message)`.
    pub fn broken_entries(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .broken
            .iter()
            .map(|(p, m)| (p.clone(), m.clone()))
            .collect();
        v.sort();
        v
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Every indexed file as `(id, path)`, sorted by path.
    pub fn entries(&self) -> Vec<(AssetId, String)> {
        let mut v: Vec<_> = self.by_path.iter().map(|(p, i)| (*i, p.clone())).collect();
        v.sort_by(|a, b| a.1.cmp(&b.1));
        v
    }

    fn insert(&mut self, id: AssetId, path: String) {
        self.generation += 1;
        if let Some(old) = self.by_id.insert(id, path.clone()) {
            self.by_path.remove(&old);
        }
        if let Some(old_id) = self.by_path.insert(path, id)
            && old_id != id
        {
            self.by_id.remove(&old_id);
        }
    }

    fn remove_path(&mut self, path: &str) -> Option<AssetId> {
        let id = self.by_path.remove(path)?;
        self.generation += 1;
        self.by_id.remove(&id);
        Some(id)
    }

    /// Records a rename done by the server.
    pub fn moved(&mut self, id: AssetId, to: &str) {
        self.insert(id, to.to_string());
    }

    /// Rebuilds the index from disk: every file with an importer, its
    /// `.meta` created when missing; duplicate ids (copied files) resolved by
    /// giving the newer files fresh ids. Returns the changes against the
    /// previous index.
    pub fn scan(&mut self, importers: &ImporterSet) -> Vec<Change> {
        let mut files = Vec::new();
        walk(&self.root, &self.root, &mut files);
        files.sort();
        let mut found: Vec<(AssetId, String, SystemTime)> = Vec::new();
        let mut broken = HashMap::new();
        for rel in files {
            let Some(importer) = importers.for_path(&rel) else {
                continue;
            };
            let abs = self.root.join(&rel);
            match read_or_create_meta(&abs, &rel, importer.name(), importer.default_settings()) {
                Ok((meta, mtime, created)) => {
                    if created {
                        self.created_metas.insert(rel.clone());
                    }
                    found.push((meta.id, rel, mtime));
                }
                Err(message) => {
                    tracing::error!("{rel}.meta: invalid .meta file, asset skipped: {message}");
                    broken.insert(rel, message);
                }
            }
        }
        // Duplicate ids: the oldest .meta keeps the id (ties: smallest path).
        found.sort_by(|a, b| (a.0, a.2, &a.1).cmp(&(b.0, b.2, &b.1)));
        let mut new_by_id: HashMap<AssetId, String> = HashMap::new();
        for (id, rel, _) in found {
            if let Some(kept) = new_by_id.get(&id) {
                match give_fresh_id(&self.root.join(&rel)) {
                    Ok(fresh) => {
                        tracing::warn!(
                            "{rel}: has the same asset id as {kept} (a copied file?); gave it the new id {fresh}"
                        );
                        new_by_id.insert(fresh, rel);
                    }
                    Err(e) => {
                        tracing::error!(
                            "{rel}: duplicate asset id {id}; could not write a new one: {e}"
                        );
                        broken.insert(rel, format!("duplicate asset id {id}: {e}"));
                    }
                }
            } else {
                new_by_id.insert(id, rel);
            }
        }
        let mut changes = Vec::new();
        for (id, path) in &new_by_id {
            match self.by_id.get(id) {
                Some(old) if old == path => changes.push(Change::Unchanged { id: *id }),
                Some(old) => changes.push(Change::Moved {
                    id: *id,
                    from: old.clone(),
                }),
                None => changes.push(Change::New { id: *id }),
            }
        }
        for (id, path) in &self.by_id {
            if !new_by_id.contains_key(id) {
                changes.push(Change::Removed {
                    id: *id,
                    path: path.clone(),
                });
            }
        }
        if broken != self.broken
            || changes
                .iter()
                .any(|c| !matches!(c, Change::Unchanged { .. }))
        {
            self.generation += 1;
        }
        self.by_path = new_by_id.iter().map(|(i, p)| (p.clone(), *i)).collect();
        self.by_id = new_by_id;
        self.broken = broken;
        changes
    }

    /// Re-examines one file (after a file system event or a load of an
    /// unknown path).
    pub fn refresh(&mut self, rel: &str, importers: &ImporterSet) -> Vec<Change> {
        let abs = self.root.join(rel);
        let importer = importers
            .for_path(rel)
            .filter(|_| crate::vfs::is_file(&abs) && !is_hidden(rel));
        let Some(importer) = importer else {
            if self.broken.remove(rel).is_some() {
                self.generation += 1;
            }
            return self
                .remove_path(rel)
                .map(|id| Change::Removed {
                    id,
                    path: rel.to_string(),
                })
                .into_iter()
                .collect();
        };
        let meta =
            match read_or_create_meta(&abs, rel, importer.name(), importer.default_settings()) {
                Ok((meta, _, created)) => {
                    if created {
                        self.created_metas.insert(rel.to_string());
                    }
                    meta
                }
                Err(message) => {
                    tracing::error!("{rel}.meta: invalid .meta file, asset skipped: {message}");
                    if self.broken.insert(rel.to_string(), message.clone()) != Some(message) {
                        self.generation += 1;
                    }
                    return self
                        .remove_path(rel)
                        .map(|id| Change::Removed {
                            id,
                            path: rel.to_string(),
                        })
                        .into_iter()
                        .collect();
                }
            };
        if self.broken.remove(rel).is_some() {
            self.generation += 1;
        }
        let mut changes = Vec::new();
        // The file at this path used to have another id (its .meta was replaced).
        if let Some(old) = self.id_of(rel)
            && old != meta.id
        {
            self.remove_path(rel);
            changes.push(Change::Removed {
                id: old,
                path: rel.to_string(),
            });
        }
        match self.by_id.get(&meta.id).cloned() {
            Some(p) if p == rel => changes.push(Change::Unchanged { id: meta.id }),
            Some(p) => {
                let other = self.root.join(&p);
                let still_there = crate::vfs::is_file(&other)
                    && matches!(meta::read_meta(&other), Ok(Some(m)) if m.id == meta.id);
                if still_there {
                    match give_fresh_id(&abs) {
                        Ok(fresh) => {
                            tracing::warn!(
                                "{rel}: has the same asset id as {p} (a copied file?); gave it the new id {fresh}"
                            );
                            self.insert(fresh, rel.to_string());
                            changes.push(Change::New { id: fresh });
                        }
                        Err(e) => {
                            tracing::error!(
                                "{rel}: duplicate asset id; could not write a new one: {e}"
                            );
                            self.broken
                                .insert(rel.to_string(), format!("duplicate asset id: {e}"));
                        }
                    }
                } else {
                    self.insert(meta.id, rel.to_string());
                    changes.push(Change::Moved {
                        id: meta.id,
                        from: p,
                    });
                }
            }
            None => {
                self.insert(meta.id, rel.to_string());
                changes.push(Change::New { id: meta.id });
            }
        }
        changes
    }

    /// Indexed paths equal to `rel` or below it (a removed folder).
    pub fn paths_under(&self, rel: &str) -> Vec<String> {
        let prefix = format!("{rel}/");
        self.by_path
            .keys()
            .filter(|p| *p == rel || p.starts_with(&prefix))
            .cloned()
            .collect()
    }
}

/// Files below `dir` as paths relative to `root`, skipping hidden entries
/// and `.meta` sidecars.
pub(crate) fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = crate::vfs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        if entry.name.starts_with('.') {
            continue;
        }
        if entry.is_dir {
            walk(root, &entry.path, out);
        } else if !entry.name.ends_with(META_SUFFIX)
            && let Some(rel) = rel_of(root, &entry.path)
        {
            out.push(rel);
        }
    }
}

/// `abs` relative to `root` with `/` separators.
pub(crate) fn rel_of(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn read_or_create_meta(
    abs: &Path,
    rel: &str,
    importer: &str,
    settings: serde_json::Value,
) -> Result<(Meta, SystemTime, bool), String> {
    let mut created = false;
    let meta = match meta::read_meta(abs)? {
        Some(m) => m,
        None if crate::vfs::is_read_only(abs) => {
            return Err("missing (the pak has no .meta for this file)".into());
        }
        None => {
            created = true;
            let m = Meta::new(importer, settings);
            meta::write_meta(abs, &m).map_err(|e| format!("could not create it: {e}"))?;
            tracing::info!("{rel}: created {rel}.meta (id {})", m.id);
            m
        }
    };
    let mtime = std::fs::metadata(meta::meta_path(abs))
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok((meta, mtime, created))
}

fn give_fresh_id(abs: &Path) -> Result<AssetId, String> {
    let mut m = meta::read_meta(abs)?.ok_or("its .meta disappeared")?;
    m.id = AssetId::new();
    meta::write_meta(abs, &m).map_err(|e| e.to_string())?;
    Ok(m.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_paths() {
        assert_eq!(normalize_rel("a/./b/../c.png").as_deref(), Some("a/c.png"));
        assert_eq!(normalize_rel("a\\b.png").as_deref(), Some("a/b.png"));
        assert_eq!(normalize_rel("../x.png"), None);
        assert_eq!(normalize_rel("/etc/passwd"), None);
        assert_eq!(normalize_rel("C:/x.png"), None);
        assert_eq!(normalize_rel(""), None);
        assert!(is_hidden("a/.x.tmp"));
        assert!(!is_hidden("a/x.png"));
    }
}
