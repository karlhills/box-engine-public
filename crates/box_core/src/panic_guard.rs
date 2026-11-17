//! Panics the engine expects and handles: a generator, importer or game
//! system that panics is caught and reported as an error, not a crash.
//!
//! [`catch`] is `catch_unwind` that also marks the thread while the closure
//! runs, so a process panic hook (the crash reporter in
//! `box_engine::crash`) can tell a caught panic ([`is_catching`]) from one
//! that ends the program.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f`, returning `Err(payload)` if it panicked. While it runs,
/// [`is_catching`] is true on this thread.
pub fn catch<T>(f: impl FnOnce() -> T) -> std::thread::Result<T> {
    DEPTH.with(|d| d.set(d.get() + 1));
    let result = catch_unwind(AssertUnwindSafe(f));
    DEPTH.with(|d| d.set(d.get() - 1));
    result
}

/// True inside [`catch`] on this thread: a panic now will be caught.
pub fn is_catching() -> bool {
    DEPTH.with(Cell::get) > 0
}

/// The text of a panic payload (`&str` or `String`), or a placeholder.
pub fn payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-string panic payload)".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_the_thread_while_catching() {
        assert!(!is_catching());
        let r = catch(|| {
            assert!(is_catching());
            let inner = catch(|| panic!("inner {}", 1));
            assert!(is_catching(), "still inside the outer catch");
            payload_message(inner.unwrap_err().as_ref())
        });
        assert_eq!(r.unwrap(), "inner 1");
        assert!(!is_catching());
        let e = catch(|| panic!("plain")).unwrap_err();
        assert_eq!(payload_message(e.as_ref()), "plain");
    }
}
