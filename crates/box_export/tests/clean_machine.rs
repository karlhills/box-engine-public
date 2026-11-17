//! The clean-machine release test (docs/RELEASING.md), for CI: packages a
//! macOS release (`scripts/release/package-macos.sh`, a release build), then
//! runs `scripts/release/clean-machine-test.sh` on its zip: install into a
//! fresh HOME with no Rust on PATH, follow the getting-started guide, export
//! and run the game, capture the editor.
//!
//! `cargo test -p box_export --test clean_machine -- --ignored --nocapture`

#[cfg(target_os = "macos")]
#[test]
#[ignore = "builds and packages a release (minutes on a clean target dir); run with --ignored"]
fn a_release_installs_and_exports_a_game_without_rust() {
    use std::path::Path;
    use std::process::Command;

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = |script: &str| {
        let status = Command::new("bash")
            .arg(root.join("scripts/release").join(script))
            .current_dir(&root)
            .status()
            .unwrap_or_else(|e| panic!("could not run {script}: {e}"));
        assert!(status.success(), "{script} failed ({status})");
    };
    run("package-macos.sh");
    run("clean-machine-test.sh");

    // The script checked the captures; check the copies it left, too.
    let captures = root.join("target/captures/clean-machine");
    for name in [
        "render.png",
        "exported_f5.png",
        "exported_f35.png",
        "editor.png",
    ] {
        let image = image::open(captures.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .to_rgb8();
        let first = image.get_pixel(0, 0);
        let differing = image.pixels().filter(|p| *p != first).count();
        assert!(
            differing * 10 > image.pixels().len(),
            "{name} is nearly one colour"
        );
    }
}
