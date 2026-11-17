//! Crash reports: what happened, where, and the recent log, written to a
//! file on this computer when an app panics or loses its GPU device.
//!
//! [`install`] (called by the editor, the player and `box` at startup, on the
//! main thread only, so test binaries never install it) wraps the process
//! panic hook. A panic that the engine catches and reports itself (a game
//! system in Play mode, a generator, an importer: see
//! [`box_core::panic_guard`]) is left to the previous hook. Any other panic
//! prints as usual, then writes a report to [`crash_dir`] and prints its
//! path. A report holds the message and location, a backtrace, the engine
//! version, OS, GPU adapter ([`set_gpu`]), the command line and the last
//! [`RECENT_LINES`] log records (the log ring of [`crate::log_capture`], or
//! the source given to [`set_log_source`], such as the editor's Console).
//!
//! A panic on the main thread ends the app: it also records the report as
//! the app kind's last crash ([`last_crash`]), so the next editor start can
//! offer to open it. Nothing is ever uploaded: reports stay in the folder
//! until the user deletes them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Log lines kept in a report.
pub const RECENT_LINES: usize = 200;

/// Environment variable that replaces [`crash_dir`] (tests, sandboxes).
pub const CRASH_DIR_ENV: &str = "BOX_CRASH_DIR";

/// The engine version in reports.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

type LogSource = Box<dyn Fn(usize) -> Vec<String> + Send + Sync>;

#[derive(Default)]
struct State {
    /// `editor`, `player` or `cli`; set by [`install`].
    kind: Option<&'static str>,
    gpu: Option<String>,
    log: Option<LogSource>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

/// Runs `f` on the state unless it is locked (never blocks a panic hook).
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    match state().try_lock() {
        Ok(mut s) => Some(f(&mut s)),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(f(&mut p.into_inner())),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Installs the crash reporter for an app of `kind` (`"editor"`,
/// `"player"`, `"cli"`). Does nothing when called off the main thread (a
/// test calling an app's entry point) or a second time. Turns on the log
/// ring ([`crate::log_capture::enable`]) so reports carry the recent log.
pub fn install(kind: &'static str) {
    if std::thread::current().name() != Some("main") {
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        with_state(|s| s.kind = Some(kind));
        crate::log_capture::enable();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            if box_core::panic_guard::is_catching() {
                return;
            }
            let thread = std::thread::current();
            let report = Report {
                title: "panic".into(),
                message: box_core::panic_guard::payload_message(info.payload()),
                location: info.location().map(|l| l.to_string()),
                thread: thread.name().map(str::to_owned),
                backtrace: Some(std::backtrace::Backtrace::force_capture().to_string()),
            };
            let fatal = thread.name() == Some("main");
            match write_report(&report, fatal) {
                Ok(path) => eprintln!("\n{}", crashed_text(&path, fatal)),
                Err(e) => eprintln!("\ncould not write a crash report: {e}"),
            }
        }));
    });
}

/// If `error` (or a cause) is [`RenderError::DeviceLost`](box_render::RenderError::DeviceLost),
/// writes a crash report for it and returns the report's path (logged as an
/// error). The app then ends with the error instead of drawing nothing.
pub fn report_device_lost(error: &anyhow::Error) -> Option<PathBuf> {
    let reason = error.chain().find_map(|c| match c.downcast_ref() {
        Some(box_render::RenderError::DeviceLost(r)) => Some(r.clone()),
        _ => None,
    })?;
    let report = Report {
        title: "GPU device lost".into(),
        message: format!("{error:#}"),
        location: None,
        thread: std::thread::current().name().map(str::to_owned),
        backtrace: None,
    };
    match write_report(&report, true) {
        Ok(path) => {
            tracing::error!(
                target: "box_engine::crash",
                "the GPU device was lost ({reason}); a crash report was written to {}",
                path.display()
            );
            Some(path)
        }
        Err(e) => {
            tracing::error!(target: "box_engine::crash", "the GPU device was lost ({reason}); could not write a crash report: {e}");
            None
        }
    }
}

/// What the app prints after writing the report at `path`.
pub fn crashed_text(path: &Path, fatal: bool) -> String {
    let what = if fatal {
        format!("{} crashed", app_name())
    } else {
        format!("{} hit an internal error", app_name())
    };
    format!(
        "{what}. A crash report was written to:\n  {}\nIt stays on this computer; attach it when you report the problem.",
        path.display()
    )
}

/// Records the GPU adapter for reports (name, backend, type, driver).
pub fn set_gpu(info: &box_render::wgpu::AdapterInfo) {
    let mut text = format!("{} ({:?}, {:?}", info.name, info.backend, info.device_type);
    if !info.driver.is_empty() {
        let _ = write!(text, ", driver {} {}", info.driver, info.driver_info);
    }
    text.push(')');
    with_state(|s| s.gpu = Some(text.trim_end().to_owned()));
}

/// Where reports get their recent log lines (the last `n`, oldest first)
/// instead of [`crate::log_capture`]. It must not block (use `try_lock`).
pub fn set_log_source(f: impl Fn(usize) -> Vec<String> + Send + Sync + 'static) {
    with_state(|s| s.log = Some(Box::new(f)));
}

/// The folder reports go to: [`CRASH_DIR_ENV`] if set, else
/// `~/Library/Logs/box-engine` (macOS), `%LOCALAPPDATA%\box-engine\logs`
/// (Windows), `$XDG_STATE_HOME/box-engine/logs` or
/// `~/.local/state/box-engine/logs` (elsewhere).
pub fn crash_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(d) = var(CRASH_DIR_ENV) {
        return Some(d);
    }
    if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Logs/box-engine"))
    } else if cfg!(windows) {
        var("LOCALAPPDATA").map(|d| d.join("box-engine").join("logs"))
    } else {
        var("XDG_STATE_HOME")
            .or_else(|| var("HOME").map(|h| h.join(".local/state")))
            .map(|d| d.join("box-engine").join("logs"))
    }
}

/// One report.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// What kind of failure: `panic`, `GPU device lost`.
    pub title: String,
    pub message: String,
    /// `file:line:column` of a panic.
    pub location: Option<String>,
    pub thread: Option<String>,
    pub backtrace: Option<String>,
}

/// Writes `report` to a new file in [`crash_dir`] and returns its path.
/// `fatal` (the app is ending) also records it as the app kind's
/// [`last_crash`].
pub fn write_report(report: &Report, fatal: bool) -> std::io::Result<PathBuf> {
    let dir = crash_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no folder for crash reports (HOME/LOCALAPPDATA is not set)",
        )
    })?;
    std::fs::create_dir_all(&dir)?;
    let (stamp, readable) = utc_now();
    let app = app_name();
    let path = dir.join(format!("{app}-crash-{stamp}-{}.txt", std::process::id()));
    std::fs::write(&path, report_text(report, &readable))?;
    let kind = with_state(|s| s.kind).flatten();
    if fatal && let Some(kind) = kind {
        std::fs::write(marker(&dir, kind), path.display().to_string())?;
    }
    Ok(path)
}

/// The text of a report.
fn report_text(report: &Report, time: &str) -> String {
    let (kind, gpu, log) = with_state(|s| {
        let lines = s
            .log
            .as_ref()
            .map(|f| f(RECENT_LINES))
            .unwrap_or_else(|| crate::log_capture::try_recent(RECENT_LINES));
        (s.kind, s.gpu.clone(), lines)
    })
    .unwrap_or_default();
    let mut t = String::new();
    let _ = writeln!(t, "box-engine crash report ({})", report.title);
    let _ = writeln!(t, "==========================================");
    let _ = writeln!(t, "App:       {} ({})", app_name(), kind.unwrap_or("app"));
    let _ = writeln!(t, "Engine:    box-engine {ENGINE_VERSION}");
    let _ = writeln!(t, "Time:      {time}");
    let _ = writeln!(t, "OS:        {}", os_text());
    let _ = writeln!(
        t,
        "GPU:       {}",
        gpu.as_deref().unwrap_or("(not created yet)")
    );
    let args: Vec<String> = std::env::args().collect();
    let _ = writeln!(t, "Command:   {}", crate::player::shell_line(&args));
    if let Ok(dir) = std::env::current_dir() {
        let _ = writeln!(t, "Directory: {}", dir.display());
    }
    if let Some(thread) = &report.thread {
        let _ = writeln!(t, "Thread:    {thread}");
    }
    let _ = writeln!(t, "\nWhat happened\n-------------");
    if let Some(l) = &report.location {
        let _ = writeln!(t, "at {l}:");
    }
    let _ = writeln!(t, "{}", report.message);
    if let Some(bt) = &report.backtrace {
        let _ = writeln!(t, "\nBacktrace\n---------\n{}", bt.trim_end());
    }
    let _ = writeln!(t, "\nRecent log (oldest first)\n-------------------------");
    if log.is_empty() {
        let _ = writeln!(t, "(none)");
    }
    for line in log {
        let _ = writeln!(t, "{line}");
    }
    let _ = writeln!(
        t,
        "\nThis file stays on this computer: box-engine never uploads crash reports.\nAttach it when you report the problem; delete it when you no longer need it."
    );
    t
}

/// The file naming the last fatal report of `kind`.
fn marker(dir: &Path, kind: &str) -> PathBuf {
    dir.join(format!("{kind}.last-crash"))
}

/// The report of the last crash of an app of `kind` that was not
/// [dismissed](dismiss_last_crash), if its file still exists.
pub fn last_crash(kind: &str) -> Option<PathBuf> {
    let dir = crash_dir()?;
    let text = std::fs::read_to_string(marker(&dir, kind)).ok()?;
    let path = PathBuf::from(text.trim());
    path.is_file().then_some(path)
}

/// Forgets the last crash of `kind` (the report file is kept).
pub fn dismiss_last_crash(kind: &str) {
    if let Some(dir) = crash_dir() {
        let _ = std::fs::remove_file(marker(&dir, kind));
    }
}

/// The executable's name (`box-editor`, `collector-player`, ...).
fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "box".into())
}

fn os_text() -> String {
    let version = if cfg!(target_os = "macos") {
        std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
            .ok()
            .and_then(|plist| {
                let key = plist.find("<key>ProductVersion</key>")?;
                let rest = &plist[key..];
                let start = rest.find("<string>")? + "<string>".len();
                let end = rest[start..].find("</string>")?;
                Some(rest[start..start + end].to_owned())
            })
    } else if cfg!(target_os = "linux") {
        std::fs::read_to_string("/etc/os-release")
            .ok()
            .and_then(|t| {
                t.lines()
                    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                    .map(|v| v.trim_matches('"').to_owned())
            })
    } else {
        None
    };
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match version {
        Some(v) => format!("{os} {v} ({arch})"),
        None => format!("{os} ({arch})"),
    }
}

/// (`20261116-205122`, `2026-11-16 20:51:22 UTC`) for now.
fn utc_now() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, mo, d) = civil_from_days((secs / 86_400) as i64);
    let (h, mi, s) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    (
        format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}"),
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC"),
    )
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `install` relies on this: the test harness runs tests on threads named
    /// after the test (also with `--test-threads=1`), so a test that calls an
    /// app entry point never installs the reporter.
    #[test]
    fn tests_do_not_run_on_the_main_thread() {
        assert_ne!(std::thread::current().name(), Some("main"));
        install("player");
        assert_eq!(with_state(|s| s.kind).flatten(), None);
    }

    #[test]
    fn dates_convert() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_408), (2025, 11, 16));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn report_text_has_the_sections() {
        let text = report_text(
            &Report {
                title: "panic".into(),
                message: "boom".into(),
                location: Some("src/x.rs:1:2".into()),
                thread: Some("main".into()),
                backtrace: Some("0: frame".into()),
            },
            "2026-01-01 00:00:00 UTC",
        );
        for part in [
            "box-engine crash report (panic)",
            &format!("Engine:    box-engine {ENGINE_VERSION}"),
            "OS:        ",
            "GPU:       ",
            "at src/x.rs:1:2:\nboom",
            "Backtrace\n---------\n0: frame",
            "Recent log (oldest first)",
            "never uploads",
        ] {
            assert!(text.contains(part), "{part:?} missing in:\n{text}");
        }
    }
}
