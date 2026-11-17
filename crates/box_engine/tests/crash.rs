//! Crash reports (`box_engine::crash`), tested in child processes: this
//! test binary runs itself with `BOX_CRASH_TEST_CHILD=<case>`, and the child
//! installs the crash reporter on its main thread, as the apps do.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use box_engine::crash;

const CHILD: &str = "BOX_CRASH_TEST_CHILD";

fn main() {
    if let Ok(case) = std::env::var(CHILD) {
        child(&case);
        return;
    }
    let cases: [(&str, fn()); 5] = [
        ("a_main_thread_panic_writes_a_report", main_thread_panic),
        ("a_caught_panic_writes_nothing", caught_panic),
        ("a_worker_panic_is_reported_but_not_a_crash", worker_panic),
        ("a_lost_device_writes_a_report", device_lost),
        ("dismissing_forgets_the_last_crash", dismiss),
    ];
    for (name, f) in cases {
        print!("test {name} ... ");
        f();
        println!("ok");
    }
    println!("\ntest result: ok. {} passed", cases.len());
}

/// What a child process does.
fn child(case: &str) {
    box_engine::init_logging();
    crash::install("player");
    tracing::warn!(target: "box_engine::crash_test", "a warning before the crash");
    match case {
        "panic" => panic!("the game state is impossible"),
        "caught" => {
            let r = box_core::panic_guard::catch(|| panic!("handled by the engine"));
            assert!(r.is_err());
        }
        "worker" => {
            let t = std::thread::Builder::new()
                .name("importer".into())
                .spawn(|| panic!("worker failed"))
                .unwrap();
            assert!(t.join().is_err());
        }
        "device_lost" => {
            let e = anyhow::Error::new(box_engine::box_render::RenderError::DeviceLost(
                "simulated".into(),
            ))
            .context("rendering frame 3");
            let path = crash::report_device_lost(&e).expect("a report");
            println!("REPORT {}", path.display());
        }
        "last_crash" => {
            println!("LAST {:?}", crash::last_crash("player"));
            crash::dismiss_last_crash("player");
            println!("AFTER {:?}", crash::last_crash("player"));
        }
        other => panic!("unknown case {other}"),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "box_crash_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_child(case: &str, dir: &Path) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .env(CHILD, case)
        .env(crash::CRASH_DIR_ENV, dir)
        .env("RUST_BACKTRACE", "0")
        .output()
        .unwrap()
}

fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains("-crash-"))
        })
        .collect();
    v.sort();
    v
}

fn main_thread_panic() {
    let dir = temp_dir("panic");
    let out = run_child("panic", &dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "{stderr}");
    let files = reports(&dir);
    assert_eq!(files.len(), 1, "{files:?}\n{stderr}");
    let report = &files[0];
    // The standard panic message, then where the report is.
    assert!(stderr.contains("the game state is impossible"), "{stderr}");
    assert!(
        stderr.contains("crashed. A crash report was written to:"),
        "{stderr}"
    );
    assert!(stderr.contains(&report.display().to_string()), "{stderr}");
    let text = std::fs::read_to_string(report).unwrap();
    for part in [
        "box-engine crash report (panic)",
        "(player)",
        &format!("Engine:    box-engine {}", crash::ENGINE_VERSION),
        "OS:        ",
        "GPU:       (not created yet)",
        "Thread:    main",
        "tests/crash.rs:",
        "the game state is impossible",
        "Backtrace",
        "a warning before the crash",
        "never uploads",
    ] {
        assert!(text.contains(part), "{part:?} missing:\n{text}");
    }
    // The next editor-like start finds it as the last crash of its kind.
    let marker = std::fs::read_to_string(dir.join("player.last-crash")).unwrap();
    assert_eq!(marker.trim(), report.display().to_string());
    std::fs::remove_dir_all(dir).unwrap();
}

fn caught_panic() {
    let dir = temp_dir("caught");
    let out = run_child("caught", &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(reports(&dir).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

fn worker_panic() {
    let dir = temp_dir("worker");
    let out = run_child("worker", &dir);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let files = reports(&dir);
    assert_eq!(files.len(), 1, "{stderr}");
    assert!(stderr.contains("hit an internal error"), "{stderr}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("Thread:    importer"), "{text}");
    // Not fatal: no "crashed last time" marker.
    assert!(!dir.join("player.last-crash").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

fn device_lost() {
    let dir = temp_dir("lost");
    let out = run_child("device_lost", &dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let path = stdout
        .lines()
        .find_map(|l| l.strip_prefix("REPORT "))
        .expect("the report path");
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("crash report (GPU device lost)"), "{text}");
    assert!(
        text.contains("rendering frame 3: the GPU device was lost (simulated)"),
        "{text}"
    );
    assert!(stderr.contains("a crash report was written to"), "{stderr}");
    assert!(dir.join("player.last-crash").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

fn dismiss() {
    let dir = temp_dir("dismiss");
    assert_eq!(run_child("panic", &dir).status.code(), Some(101));
    let report = reports(&dir).pop().unwrap();
    let out = run_child("last_crash", &dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&format!("LAST Some({:?})", report)),
        "{stdout}"
    );
    assert!(stdout.contains("AFTER None"), "{stdout}");
    assert!(report.is_file(), "dismissing keeps the report");
    std::fs::remove_dir_all(dir).unwrap();
}
