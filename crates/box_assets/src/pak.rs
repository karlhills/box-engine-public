//! `.pak` files: the data pack of an exported game (docs/ASSETS.md section 7,
//! spike S7).
//!
//! Layout, all integers little-endian:
//!
//! | bytes | field |
//! |---|---|
//! | 0..8 | magic `BOXPAK\r\n` |
//! | 8..12 | format version ([`PAK_VERSION`]) |
//! | 12..16 | flags (0) |
//! | 16..20 | entry count |
//! | 20..24 | reserved (0) |
//! | 24..32 | index offset |
//! | 32..40 | index length |
//! | 40..56 | xxh3-128 of the index |
//! | 56..64 | xxh3-64 of bytes 0..56 |
//!
//! File contents follow the 64-byte header back to back, each 16-byte
//! aligned (zero padding), uncompressed. The index sits at the end: per entry,
//! sorted by path, a `u16` path length, the UTF-8 path (relative, `/`
//! separators), the `u64` offset and length and the xxh3-128 of the content.
//!
//! The header hash and the index hash are checked by [`Pak::open`]; each
//! file's hash by [`Pak::read`]. A writer given the same files writes the
//! same bytes (no timestamps), so a cook is reproducible.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::index::normalize_rel;

/// The first eight bytes of every pak (`\r\n` catches text-mode mangling).
pub const PAK_MAGIC: &[u8; 8] = b"BOXPAK\r\n";
/// The pak format version this engine writes and reads.
pub const PAK_VERSION: u32 = 1;
const HEADER_LEN: u64 = 64;
const ALIGN: u64 = 16;

/// A content hash: xxh3-128, little-endian bytes.
pub type ContentHash = [u8; 16];

/// The xxh3-128 hash a pak stores for `bytes`.
pub fn content_hash(bytes: &[u8]) -> ContentHash {
    twox_hash::XxHash3_128::oneshot(bytes).to_le_bytes()
}

/// `hash` as 32 lowercase hex digits.
pub fn hash_hex(hash: &ContentHash) -> String {
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why a pak could not be written, opened or read.
#[derive(Debug, thiserror::Error)]
pub enum PakError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: not a box pak file", .0.display())]
    NotAPak(PathBuf),
    #[error(
        "{}: pak format version {found} is newer than this engine supports ({PAK_VERSION}); update box-engine",
        path.display()
    )]
    NewerVersion { path: PathBuf, found: u32 },
    /// A hash or a size does not match: the file is damaged or truncated.
    #[error("{}: damaged pak ({message})", path.display())]
    Corrupt { path: PathBuf, message: String },
    #[error("`{0}` is not a relative path (no leading `/`, drive prefix or `..`)")]
    BadPath(String),
    #[error("{0}: not in the pak")]
    NotFound(String),
}

/// One file in a pak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PakEntry {
    /// Relative path with `/` separators.
    pub path: String,
    pub offset: u64,
    pub len: u64,
    pub hash: ContentHash,
}

enum Source {
    Bytes(Vec<u8>),
    File(PathBuf),
}

/// Collects files and writes a pak. Entries are written sorted by path, so
/// the order of `add` calls does not matter.
#[derive(Default)]
pub struct PakWriter {
    files: BTreeMap<String, Source>,
}

/// What [`PakWriter::write`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PakSummary {
    pub entries: usize,
    /// Size of the pak file.
    pub bytes: u64,
    /// Sum of the file sizes stored.
    pub content_bytes: u64,
    /// xxh3-128 of the whole pak file.
    pub hash: ContentHash,
}

impl PakWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `bytes` at `path` (replacing an earlier entry with that path).
    pub fn add_bytes(&mut self, path: &str, bytes: Vec<u8>) -> Result<(), PakError> {
        let rel = normalize_rel(path).ok_or_else(|| PakError::BadPath(path.into()))?;
        self.files.insert(rel, Source::Bytes(bytes));
        Ok(())
    }

    /// Adds the file `source` at `path`; it is read when the pak is written.
    pub fn add_file(&mut self, path: &str, source: impl Into<PathBuf>) -> Result<(), PakError> {
        let rel = normalize_rel(path).ok_or_else(|| PakError::BadPath(path.into()))?;
        self.files.insert(rel, Source::File(source.into()));
        Ok(())
    }

    /// Paths added so far, sorted.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Writes the pak to `out` through a temporary file in the same folder
    /// renamed into place, so a failed write never leaves a partial pak.
    pub fn write(&self, out: &Path) -> Result<PakSummary, PakError> {
        let io = |path: &Path| {
            let path = path.to_path_buf();
            move |source| PakError::Io { path, source }
        };
        let name = out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "game.pak".into());
        let tmp = out.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
        let result = self.write_to(&tmp).and_then(|summary| {
            std::fs::rename(&tmp, out).map_err(io(out))?;
            Ok(summary)
        });
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    fn write_to(&self, path: &Path) -> Result<PakSummary, PakError> {
        let io = |p: &Path| {
            let p = p.to_path_buf();
            move |source| PakError::Io { path: p, source }
        };
        let file = File::create(path).map_err(io(path))?;
        let mut out = BufWriter::new(file);
        out.write_all(&[0u8; HEADER_LEN as usize])
            .map_err(io(path))?;
        let mut offset = HEADER_LEN;
        let mut index = Vec::new();
        let mut content_bytes = 0;
        for (rel, source) in &self.files {
            let owned;
            let bytes: &[u8] = match source {
                Source::Bytes(b) => b,
                Source::File(f) => {
                    owned = std::fs::read(f).map_err(io(f))?;
                    &owned
                }
            };
            let pad = (ALIGN - offset % ALIGN) % ALIGN;
            out.write_all(&[0u8; ALIGN as usize][..pad as usize])
                .map_err(io(path))?;
            offset += pad;
            out.write_all(bytes).map_err(io(path))?;
            let len = u16::try_from(rel.len()).map_err(|_| PakError::BadPath(rel.clone()))?;
            index.extend_from_slice(&len.to_le_bytes());
            index.extend_from_slice(rel.as_bytes());
            index.extend_from_slice(&offset.to_le_bytes());
            index.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            index.extend_from_slice(&content_hash(bytes));
            offset += bytes.len() as u64;
            content_bytes += bytes.len() as u64;
        }
        out.write_all(&index).map_err(io(path))?;
        let count = u32::try_from(self.files.len()).map_err(|_| PakError::Corrupt {
            path: path.to_path_buf(),
            message: "more than 4 billion files".into(),
        })?;
        let mut header = Vec::with_capacity(HEADER_LEN as usize);
        header.extend_from_slice(PAK_MAGIC);
        header.extend_from_slice(&PAK_VERSION.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&count.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&offset.to_le_bytes());
        header.extend_from_slice(&(index.len() as u64).to_le_bytes());
        header.extend_from_slice(&content_hash(&index));
        let header_hash = twox_hash::XxHash3_64::oneshot(&header);
        header.extend_from_slice(&header_hash.to_le_bytes());
        out.seek(SeekFrom::Start(0)).map_err(io(path))?;
        out.write_all(&header).map_err(io(path))?;
        let file = out.into_inner().map_err(|e| PakError::Io {
            path: path.to_path_buf(),
            source: e.into_error(),
        })?;
        file.sync_all().map_err(io(path))?;
        drop(file);
        let bytes = std::fs::read(path).map_err(io(path))?;
        Ok(PakSummary {
            entries: self.files.len(),
            bytes: bytes.len() as u64,
            content_bytes,
            hash: content_hash(&bytes),
        })
    }
}

/// An open pak: the index in memory, contents read on demand (positioned
/// reads, each checked against its hash).
pub struct Pak {
    path: PathBuf,
    file: Mutex<File>,
    entries: Vec<PakEntry>,
    by_path: BTreeMap<String, usize>,
    dirs: BTreeSet<String>,
}

impl std::fmt::Debug for Pak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pak")
            .field("path", &self.path)
            .field("entries", &self.entries.len())
            .finish()
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(a)
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

impl Pak {
    /// Opens `path`, checking the header and the index (not the contents:
    /// see [`verify`](Self::verify)).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PakError> {
        let path = path.as_ref().to_path_buf();
        let io = |source| PakError::Io {
            path: path.clone(),
            source,
        };
        let corrupt = |message: String| PakError::Corrupt {
            path: path.clone(),
            message,
        };
        let mut file = File::open(&path).map_err(io)?;
        let size = file.metadata().map_err(io)?.len();
        let mut header = [0u8; HEADER_LEN as usize];
        if size < HEADER_LEN {
            return Err(PakError::NotAPak(path));
        }
        file.read_exact(&mut header).map_err(io)?;
        if &header[0..8] != PAK_MAGIC {
            return Err(PakError::NotAPak(path));
        }
        if twox_hash::XxHash3_64::oneshot(&header[..56]) != u64_at(&header, 56) {
            return Err(corrupt("header hash mismatch".into()));
        }
        let version = u32_at(&header, 8);
        if version > PAK_VERSION {
            return Err(PakError::NewerVersion {
                path,
                found: version,
            });
        }
        let count = u32_at(&header, 16) as usize;
        let index_offset = u64_at(&header, 24);
        let index_len = u64_at(&header, 32);
        if index_offset
            .checked_add(index_len)
            .is_none_or(|end| end != size)
        {
            return Err(corrupt(format!(
                "index at {index_offset}+{index_len} does not end the {size}-byte file (truncated?)"
            )));
        }
        let mut index = vec![0u8; index_len as usize];
        file.seek(SeekFrom::Start(index_offset)).map_err(io)?;
        file.read_exact(&mut index).map_err(io)?;
        if content_hash(&index)[..] != header[40..56] {
            return Err(corrupt("index hash mismatch".into()));
        }
        let mut entries = Vec::with_capacity(count);
        let mut at = 0usize;
        let short = || corrupt("index entry cut short".into());
        for _ in 0..count {
            let n = index.get(at..at + 2).ok_or_else(short)?;
            let n = u16::from_le_bytes([n[0], n[1]]) as usize;
            let name = index.get(at + 2..at + 2 + n).ok_or_else(short)?;
            let name = String::from_utf8(name.to_vec())
                .map_err(|_| corrupt("index path is not UTF-8".into()))?;
            at += 2 + n;
            let rest = index.get(at..at + 32).ok_or_else(short)?;
            let offset = u64_at(rest, 0);
            let len = u64_at(rest, 8);
            let mut hash = [0u8; 16];
            hash.copy_from_slice(&rest[16..32]);
            at += 32;
            if offset < HEADER_LEN || offset.checked_add(len).is_none_or(|e| e > index_offset) {
                return Err(corrupt(format!("{name}: data outside the file")));
            }
            if normalize_rel(&name).as_deref() != Some(name.as_str()) {
                return Err(corrupt(format!("`{name}`: bad path in index")));
            }
            entries.push(PakEntry {
                path: name,
                offset,
                len,
                hash,
            });
        }
        if at != index.len() {
            return Err(corrupt("trailing bytes after the index".into()));
        }
        let by_path: BTreeMap<String, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.path.clone(), i))
            .collect();
        if by_path.len() != entries.len() {
            return Err(corrupt("duplicate paths in index".into()));
        }
        let mut dirs = BTreeSet::new();
        for e in &entries {
            let mut p = e.path.as_str();
            while let Some((parent, _)) = p.rsplit_once('/') {
                if !dirs.insert(parent.to_string()) {
                    break;
                }
                p = parent;
            }
        }
        Ok(Self {
            path,
            file: Mutex::new(file),
            entries,
            by_path,
            dirs,
        })
    }

    /// The pak file's path, as given to [`open`](Self::open).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry, sorted by path.
    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }

    pub fn entry(&self, rel: &str) -> Option<&PakEntry> {
        self.by_path.get(rel).map(|&i| &self.entries[i])
    }

    pub fn contains(&self, rel: &str) -> bool {
        self.by_path.contains_key(rel)
    }

    /// True for a folder that holds entries (`""` is the root).
    pub fn is_dir(&self, rel: &str) -> bool {
        rel.is_empty() || self.dirs.contains(rel)
    }

    /// The direct children of folder `rel` (`""` is the root) as
    /// `(name, is_dir)`, sorted by name.
    pub fn list(&self, rel: &str) -> Vec<(String, bool)> {
        let prefix = if rel.is_empty() {
            String::new()
        } else {
            format!("{rel}/")
        };
        let mut out: BTreeMap<String, bool> = BTreeMap::new();
        for path in self.by_path.range(prefix.clone()..).map(|(p, _)| p) {
            let Some(rest) = path.strip_prefix(&prefix) else {
                break;
            };
            match rest.split_once('/') {
                Some((dir, _)) => {
                    out.insert(dir.to_string(), true);
                }
                None => {
                    out.insert(rest.to_string(), false);
                }
            }
        }
        out.into_iter().collect()
    }

    /// The content of `rel`, checked against its hash.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>, PakError> {
        let entry = self
            .entry(rel)
            .ok_or_else(|| PakError::NotFound(rel.to_string()))?;
        let mut buf = vec![0u8; entry.len as usize];
        {
            let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
            let io = |source| PakError::Io {
                path: self.path.clone(),
                source,
            };
            file.seek(SeekFrom::Start(entry.offset)).map_err(io)?;
            file.read_exact(&mut buf).map_err(io)?;
        }
        if content_hash(&buf) != entry.hash {
            return Err(PakError::Corrupt {
                path: self.path.clone(),
                message: format!("{rel}: content hash mismatch"),
            });
        }
        Ok(buf)
    }

    /// Reads every entry and checks its hash.
    pub fn verify(&self) -> Result<(), PakError> {
        for e in &self.entries {
            self.read(&e.path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("box_pak_{name}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(dir: &Path) -> PathBuf {
        std::fs::write(dir.join("src.bin"), b"from disk").unwrap();
        let mut w = PakWriter::new();
        w.add_bytes("assets/scenes/main.scene.json", b"{}".to_vec())
            .unwrap();
        w.add_bytes("box.project.json", b"{\"name\":\"x\"}".to_vec())
            .unwrap();
        w.add_file("assets/a/b/c.bin", dir.join("src.bin")).unwrap();
        w.add_bytes("./assets/empty.txt", Vec::new()).unwrap();
        assert!(w.add_bytes("../x", Vec::new()).is_err());
        assert!(w.add_bytes("/x", Vec::new()).is_err());
        let out = dir.join("game.pak");
        let summary = w.write(&out).unwrap();
        assert_eq!(summary.entries, 4);
        assert_eq!(summary.bytes, std::fs::metadata(&out).unwrap().len());
        out
    }

    #[test]
    fn round_trip_and_listing() {
        let dir = temp("rt");
        let pak = Pak::open(sample(&dir)).unwrap();
        assert_eq!(pak.read("assets/a/b/c.bin").unwrap(), b"from disk");
        assert_eq!(pak.read("box.project.json").unwrap(), b"{\"name\":\"x\"}");
        assert_eq!(pak.read("assets/empty.txt").unwrap(), b"");
        assert!(matches!(pak.read("nope"), Err(PakError::NotFound(_))));
        assert!(pak.is_dir("") && pak.is_dir("assets") && pak.is_dir("assets/a/b"));
        assert!(!pak.is_dir("assets/a/b/c.bin"));
        assert_eq!(
            pak.list(""),
            [
                ("assets".to_string(), true),
                ("box.project.json".into(), false)
            ]
        );
        assert_eq!(
            pak.list("assets"),
            [
                ("a".to_string(), true),
                ("empty.txt".into(), false),
                ("scenes".into(), true)
            ]
        );
        pak.verify().unwrap();
        for e in pak.entries() {
            assert_eq!(e.offset % ALIGN, 0, "{} aligned", e.path);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn same_input_same_bytes() {
        let dir = temp("det");
        let a = std::fs::read(sample(&dir)).unwrap();
        let b = std::fs::read(sample(&dir)).unwrap();
        assert_eq!(a, b);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detects_damage() {
        let dir = temp("bad");
        let path = sample(&dir);
        let good = std::fs::read(&path).unwrap();
        let bad = dir.join("bad.pak");
        let open_with = |bytes: &[u8]| {
            std::fs::write(&bad, bytes).unwrap();
            Pak::open(&bad)
        };
        // A flipped content byte: found when that file is read.
        let pak = Pak::open(&path).unwrap();
        let e = pak.entry("assets/a/b/c.bin").unwrap().clone();
        let mut b = good.clone();
        b[e.offset as usize] ^= 1;
        let pak = open_with(&b).unwrap();
        let err = pak.read("assets/a/b/c.bin").unwrap_err().to_string();
        assert!(err.contains("content hash mismatch"), "{err}");
        assert!(pak.verify().is_err());
        // A flipped index byte, header byte, truncation, wrong magic.
        let mut b = good.clone();
        let n = b.len();
        b[n - 3] ^= 1;
        assert!(
            open_with(&b)
                .unwrap_err()
                .to_string()
                .contains("index hash")
        );
        let mut b = good.clone();
        b[17] ^= 1;
        assert!(
            open_with(&b)
                .unwrap_err()
                .to_string()
                .contains("header hash")
        );
        let b = &good[..good.len() - 1];
        assert!(matches!(open_with(b), Err(PakError::Corrupt { .. })));
        assert!(matches!(
            open_with(b"PK\x03\x04 not a pak at all, long enough to have a header....."),
            Err(PakError::NotAPak(_))
        ));
        assert!(matches!(open_with(b"tiny"), Err(PakError::NotAPak(_))));
        // A newer version (header hash recomputed).
        let mut b = good.clone();
        b[8] = 9;
        let h = twox_hash::XxHash3_64::oneshot(&b[..56]).to_le_bytes();
        b[56..64].copy_from_slice(&h);
        assert!(matches!(
            open_with(&b),
            Err(PakError::NewerVersion { found: 9, .. })
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
