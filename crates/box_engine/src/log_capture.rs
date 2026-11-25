//! A bounded, process-wide ring buffer of log records for the control
//! socket's `read_log` ([`crate::control`]) and playtest assertions.
//!
//! [`crate::init_logging`] installs [`CaptureLayer`] next to the terminal
//! output, with a fixed [`crate::CAPTURE_LOG_FILTER`]. It records nothing
//! until [`enable`] is called (for control, playtests, or crash reports),
//! otherwise a log event costs one atomic load. At most [`CAPACITY`]
//! records are kept.
//! Sequence numbers keep increasing when old records are dropped, so a
//! client's `since` stays valid.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

use crate::control::{LogEntry, LogPage};

/// Records kept before the oldest are dropped.
pub const CAPACITY: usize = 4096;

static ENABLED: AtomicBool = AtomicBool::new(false);
static RING: OnceLock<Mutex<Ring>> = OnceLock::new();

struct Ring {
    records: VecDeque<LogEntry>,
    next_seq: u64,
    started: Instant,
}

fn ring() -> MutexGuard<'static, Ring> {
    RING.get_or_init(|| {
        Mutex::new(Ring {
            records: VecDeque::new(),
            next_seq: 0,
            started: Instant::now(),
        })
    })
    .lock()
    // A panic while logging leaves the ring consistent enough to read.
    .unwrap_or_else(|e| e.into_inner())
}

/// Start recording log events (idempotent).
pub fn enable() {
    // Create the ring now, so record times count from here.
    drop(ring());
    ENABLED.store(true, Ordering::Relaxed);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Add a record directly (the layer does this for every event).
pub fn push(level: &tracing::Level, target: &str, message: String) {
    let mut ring = ring();
    let time = ring.started.elapsed().as_secs_f64();
    if ring.records.len() == CAPACITY {
        ring.records.pop_front();
    }
    let seq = ring.next_seq;
    ring.next_seq += 1;
    ring.records.push_back(LogEntry {
        seq,
        time,
        level: level.as_str().to_ascii_lowercase(),
        target: target.to_string(),
        message,
    });
}

/// The sequence number the next record will get: pass it as `since` to
/// [`read`] later to get only what was logged in between.
pub fn next_seq() -> u64 {
    ring().next_seq
}

/// Records with `seq >= since`, oldest first, at most `limit`. `next` is the
/// `since` for the following call.
pub fn read(since: u64, limit: usize) -> LogPage {
    let ring = ring();
    let entries: Vec<LogEntry> = ring
        .records
        .iter()
        .filter(|e| e.seq >= since)
        .take(limit)
        .cloned()
        .collect();
    let next = entries
        .last()
        .map_or(since.max(ring.next_seq), |e| e.seq + 1);
    LogPage { entries, next }
}

/// The last `n` records as text lines (`[  1.234s WARN target] message`),
/// oldest first. Never blocks: empty if the ring is locked (a panic while
/// logging), so a panic hook can call it.
pub fn try_recent(n: usize) -> Vec<String> {
    let Some(ring) = RING.get() else {
        return Vec::new();
    };
    let ring = match ring.try_lock() {
        Ok(r) => r,
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return Vec::new(),
    };
    let skip = ring.records.len().saturating_sub(n);
    ring.records
        .iter()
        .skip(skip)
        .map(|e| {
            format!(
                "[{:>9.3}s {:<5} {}] {}",
                e.time,
                e.level.to_ascii_uppercase(),
                e.target,
                e.message
            )
        })
        .collect()
}

/// `tracing` layer feeding the ring while capture is [enabled](enable).
pub struct CaptureLayer;

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if !is_enabled() {
            return;
        }
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let target = visitor.log_target.take();
        let target = target.as_deref().unwrap_or(meta.target());
        push(meta.level(), target, visitor.into_message());
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

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn layer_records_only_when_enabled_and_since_pages() {
        let subscriber = tracing_subscriber::registry().with(CaptureLayer);
        tracing::subscriber::with_default(subscriber, || {
            // The ring is process-wide: other tests may log too, so look for
            // our own records only.
            enable();
            // No entries with limit 0: `next` is the next sequence number.
            let start = read(0, 0).next;
            tracing::warn!(target: "box::log_capture_test", count = 3, "first");
            tracing::info!(target: "box::log_capture_test", "second");
            let page = read(start, usize::MAX);
            let ours: Vec<&LogEntry> = page
                .entries
                .iter()
                .filter(|e| e.target == "box::log_capture_test")
                .collect();
            assert_eq!(ours.len(), 2, "{page:?}");
            assert_eq!(ours[0].level, "warn");
            assert_eq!(ours[0].message, "first count=3");
            assert_eq!(ours[1].message, "second");
            assert!(ours[1].seq > ours[0].seq);
            let later = read(page.next, usize::MAX);
            assert!(later.entries.iter().all(|e| e.seq >= page.next));
            let one = read(ours[0].seq, 1);
            assert_eq!(one.entries.len(), 1);
            assert_eq!(one.next, ours[0].seq + 1);
        });
    }
}
