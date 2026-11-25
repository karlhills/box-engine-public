//! Log capture for the console pane: a `tracing` layer that copies records
//! into a shared ring buffer.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// Records kept before the oldest are dropped.
pub const DEFAULT_CAPACITY: usize = 10_000;

/// The console's three severity groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// INFO, DEBUG and TRACE.
    Info = 0,
    Warn = 1,
    Error = 2,
}

impl Severity {
    pub fn of(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            _ => Self::Info,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogRecord {
    /// Increases by one per record; survives eviction of older records.
    pub seq: u64,
    /// Seconds since the buffer was created (or the manual clock).
    pub time: f64,
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
}

enum Clock {
    Real(Instant),
    /// Headless runs stamp records with the fixed-step frame time so captures
    /// are reproducible.
    Manual(f64),
}

pub struct LogInner {
    records: VecDeque<LogRecord>,
    capacity: usize,
    next_seq: u64,
    counts: [usize; 3],
    generation: u64,
    clock: Clock,
}

impl LogInner {
    pub fn records(&self) -> &VecDeque<LogRecord> {
        &self.records
    }

    /// The record with sequence number `seq`, if it has not been evicted.
    pub fn get(&self, seq: u64) -> Option<&LogRecord> {
        let first = self.records.front()?.seq;
        self.records.get(seq.checked_sub(first)? as usize)
    }

    /// Record counts by [`Severity`].
    pub fn counts(&self) -> [usize; 3] {
        self.counts
    }

    /// Changes whenever records are added or cleared.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The sequence number the next record will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Records with `seq >= since`, oldest first (evicted ones skipped).
    /// Sequence numbers are contiguous, so this skips straight to the first
    /// one instead of scanning the buffer.
    pub fn since(&self, since: u64) -> impl Iterator<Item = &LogRecord> {
        let first = self.records.front().map_or(self.next_seq, |r| r.seq);
        let skip = since.saturating_sub(first) as usize;
        self.records.range(skip.min(self.records.len())..)
    }
}

/// A shared, bounded log. Cloning shares the same buffer.
#[derive(Clone)]
pub struct LogBuffer(Arc<Mutex<LogInner>>);

impl std::fmt::Debug for LogBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.lock();
        f.debug_struct("LogBuffer")
            .field("records", &inner.records.len())
            .field("capacity", &inner.capacity)
            .finish()
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(LogInner {
            records: VecDeque::new(),
            capacity: capacity.max(1),
            next_seq: 0,
            counts: [0; 3],
            generation: 0,
            clock: Clock::Real(Instant::now()),
        })))
    }

    pub fn lock(&self) -> MutexGuard<'_, LogInner> {
        // A panic while holding the lock leaves the buffer consistent enough
        // to keep displaying.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The last `n` records as text lines, oldest first, for a crash report.
    /// Never blocks: empty while another thread holds the lock.
    pub fn try_recent(&self, n: usize) -> Vec<String> {
        let inner = match self.0.try_lock() {
            Ok(i) => i,
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Vec::new(),
        };
        let skip = inner.records.len().saturating_sub(n);
        inner
            .records
            .iter()
            .skip(skip)
            .map(|r| {
                format!(
                    "[{:>9.3}s {:<5} {}] {}",
                    r.time, r.level, r.target, r.message
                )
            })
            .collect()
    }

    /// Stamps new records with `seconds` instead of wall-clock time.
    pub fn set_manual_time(&self, seconds: f64) {
        self.lock().clock = Clock::Manual(seconds);
    }

    pub fn push(&self, level: tracing::Level, target: &str, message: String) {
        let mut inner = self.lock();
        let time = match inner.clock {
            Clock::Real(start) => start.elapsed().as_secs_f64(),
            Clock::Manual(t) => t,
        };
        if inner.records.len() == inner.capacity
            && let Some(old) = inner.records.pop_front()
        {
            inner.counts[Severity::of(old.level) as usize] -= 1;
        }
        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.counts[Severity::of(level) as usize] += 1;
        inner.generation += 1;
        inner.records.push_back(LogRecord {
            seq,
            time,
            level,
            target: target.to_owned(),
            message,
        });
    }

    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.records.clear();
        inner.counts = [0; 3];
        inner.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Up to `limit` records with `seq >= since` (oldest first, evicted ones
    /// skipped), and the `seq` to pass next time for only newer records.
    pub fn page(&self, since: u64, limit: usize) -> (Vec<LogRecord>, u64) {
        let inner = self.lock();
        let records: Vec<LogRecord> = inner.since(since).take(limit).cloned().collect();
        let next = records
            .last()
            .map_or(since.max(inner.next_seq), |r| r.seq + 1);
        (records, next)
    }

    /// The sequence number the next record will get.
    pub fn next_seq(&self) -> u64 {
        self.lock().next_seq
    }

    /// A layer that copies every event it sees into this buffer.
    pub fn layer(&self) -> ConsoleLayer {
        ConsoleLayer {
            buffer: self.clone(),
        }
    }
}

/// `tracing` layer feeding a [`LogBuffer`].
pub struct ConsoleLayer {
    buffer: LogBuffer,
}

impl<S: tracing::Subscriber> Layer<S> for ConsoleLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let target = visitor.log_target.take();
        let target = target.as_deref().unwrap_or(meta.target());
        self.buffer
            .push(*meta.level(), target, visitor.into_message());
    }
}

/// Collects `message` plus other fields as ` key=value`. Records bridged from
/// the `log` crate carry their real target in `log.target`.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
    log_target: Option<String>,
}

impl MessageVisitor {
    fn into_message(self) -> String {
        let mut m = self.message;
        if !self.fields.is_empty() {
            if !m.is_empty() {
                m.push(' ');
            }
            m.push_str(self.fields.trim_start());
        }
        m
    }
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "message" => self.message.push_str(value),
            "log.target" => self.log_target = Some(value.to_owned()),
            name if name.starts_with("log.") => {}
            name => {
                let _ = write!(self.fields, " {name}={value}");
            }
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        match field.name() {
            "message" => {
                let _ = write!(self.message, "{value:?}");
            }
            "log.target" => self.log_target = Some(format!("{value:?}")),
            name if name.starts_with("log.") => {}
            name => {
                let _ = write!(self.fields, " {name}={value:?}");
            }
        }
    }
}

/// The default filter of the console: `RUST_LOG`, or info with wgpu and
/// naga internals at warn.
pub fn default_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")
    })
}

/// Installs the global subscriber: the engine's quiet terminal output on
/// stderr ([`box_engine::terminal_log_layer`]) plus `buffer`, filtered by
/// [`default_filter`]. `log` records (wgpu) are bridged.
/// Returns false if a global subscriber was already set.
pub fn init(buffer: &LogBuffer) -> bool {
    init_with_file(buffer, None)
}

/// [`init`], also writing every record [`default_filter`] passes to `file`
/// (replaced at each start, so it holds the last session). A file that
/// cannot be created is reported on stderr and skipped.
pub fn init_with_file(buffer: &LogBuffer, file: Option<&std::path::Path>) -> bool {
    let file_layer = file.and_then(|path| {
        let opened = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::File::create(path));
        match opened {
            Ok(f) => Some(
                tracing_subscriber::fmt::layer()
                    .with_writer(Mutex::new(f))
                    .with_ansi(false)
                    .with_filter(default_filter()),
            ),
            Err(e) => {
                eprintln!(
                    "box-editor: could not write the log to {}: {e}",
                    path.display()
                );
                None
            }
        }
    });
    tracing_subscriber::registry()
        .with(box_engine::terminal_log_layer())
        .with(buffer.layer().with_filter(default_filter()))
        .with(file_layer)
        // Records only once enabled (crash reports, playtests' log_contains).
        .with(box_engine::log_capture::CaptureLayer.with_filter(
            tracing_subscriber::EnvFilter::new(box_engine::CAPTURE_LOG_FILTER),
        ))
        .try_init()
        .is_ok()
}

/// Where a windowed editor without a terminal writes its log: on macOS
/// `~/Library/Logs/box-editor.log` when stderr is not a terminal (the app
/// bundle started from Finder or the Dock). `None` elsewhere: stderr is
/// visible there.
pub fn log_file_path() -> Option<std::path::PathBuf> {
    use std::io::IsTerminal;
    if !cfg!(target_os = "macos") || std::io::stderr().is_terminal() {
        return None;
    }
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(std::path::PathBuf::from(home).join("Library/Logs/box-editor.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_evicts_oldest_and_keeps_counts() {
        let b = LogBuffer::new(3);
        b.push(tracing::Level::INFO, "a", "one".into());
        b.push(tracing::Level::WARN, "a", "two".into());
        b.push(tracing::Level::ERROR, "a", "three".into());
        b.push(tracing::Level::DEBUG, "a", "four".into());
        let inner = b.lock();
        assert_eq!(inner.records().len(), 3);
        assert_eq!(inner.counts(), [1, 1, 1]);
        assert!(inner.get(0).is_none(), "evicted");
        assert_eq!(inner.get(3).unwrap().message, "four");
        drop(inner);
        let generation = b.generation();
        b.clear();
        assert_eq!(b.lock().counts(), [0, 0, 0]);
        assert!(b.generation() > generation);
    }

    #[test]
    fn pages_continue_from_next_and_skip_evicted_records() {
        let b = LogBuffer::new(3);
        for i in 0..5 {
            b.push(tracing::Level::INFO, "a", format!("m{i}"));
        }
        let (page, next) = b.page(0, 2);
        let seqs: Vec<u64> = page.iter().map(|r| r.seq).collect();
        assert_eq!((seqs, next), (vec![2, 3], 4));
        let (page, next) = b.page(next, 10);
        assert_eq!((page.len(), page[0].message.as_str(), next), (1, "m4", 5));
        let (page, next) = b.page(next, 10);
        assert!(page.is_empty());
        assert_eq!(next, 5);
    }

    #[test]
    fn layer_captures_message_fields_and_target() {
        let b = LogBuffer::new(16);
        b.set_manual_time(1.5);
        let subscriber = tracing_subscriber::registry().with(b.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(target: "box_render", count = 3, "shader reloaded");
        });
        let inner = b.lock();
        let r = inner.records().back().unwrap();
        assert_eq!(r.level, tracing::Level::WARN);
        assert_eq!(r.target, "box_render");
        assert_eq!(r.message, "shader reloaded count=3");
        assert_eq!(r.time, 1.5);
    }
}
