//! The file watcher behind hot reload: notify with notify-debouncer-full.
//!
//! The watcher thread only forwards changed paths through a channel; the
//! asset server handles them in `update` on the main thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::error::AssetError;

/// Debounce window: editors and exporters often write a file in several
/// steps (temporary file, rename, metadata).
pub const DEBOUNCE: Duration = Duration::from_millis(150);

pub(crate) struct Watcher {
    // Dropping the debouncer stops its threads.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    rx: Receiver<Vec<PathBuf>>,
}

impl Watcher {
    /// Watches `root` recursively (the folder, not single files, so
    /// save-by-rename is seen).
    pub fn start(root: &Path) -> Result<Self, AssetError> {
        let (tx, rx) = channel();
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            match result {
                Ok(events) => {
                    let paths: Vec<PathBuf> = events
                        .into_iter()
                        .filter(|e| {
                            // Metadata-only events (FSEvents emits them
                            // spuriously) and reads change nothing.
                            !matches!(
                                e.kind,
                                EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_))
                            )
                        })
                        .flat_map(|e| e.event.paths)
                        .collect();
                    if !paths.is_empty() {
                        let _ = tx.send(paths);
                    }
                }
                Err(errors) => {
                    for e in errors {
                        tracing::warn!("asset watcher: {e}");
                    }
                }
            }
        })
        .map_err(|e| AssetError::Watch(e.to_string()))?;
        debouncer
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| AssetError::Watch(e.to_string()))?;
        Ok(Self {
            _debouncer: debouncer,
            rx,
        })
    }

    /// Paths reported since the last call.
    pub fn drain(&self) -> Vec<PathBuf> {
        self.rx.try_iter().flatten().collect()
    }
}
