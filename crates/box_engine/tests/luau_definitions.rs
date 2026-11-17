//! The generated luau-lsp definitions load in luau-lsp and type the example
//! projects' scripts without errors.
//!
//! Runs `luau-lsp analyze` (https://github.com/JohnnyMorganz/luau-lsp; checked
//! with 1.70.1) when it is on `PATH`; otherwise the test says so and passes,
//! so machines without the tool still build. `BOX_REQUIRE_LUAU_LSP=1` makes a
//! missing tool a failure.

use std::path::{Path, PathBuf};
use std::process::Command;

use box_engine::Game;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn luau_lsp() -> Option<PathBuf> {
    let found = Command::new("luau-lsp")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if found {
        return Some(PathBuf::from("luau-lsp"));
    }
    assert!(
        std::env::var_os("BOX_REQUIRE_LUAU_LSP").is_none(),
        "BOX_REQUIRE_LUAU_LSP is set but luau-lsp is not on PATH"
    );
    eprintln!("luau-lsp not on PATH: skipping the definitions check");
    None
}

fn scripts(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            scripts(&path, out);
        } else if path.extension().is_some_and(|e| e == "luau") {
            out.push(path);
        }
    }
}

/// Runs `luau-lsp analyze` and returns its diagnostics, minus unused-local
/// lints and info lines. A definitions file that fails to load is reported
/// as a diagnostic ("Failed to read definitions file").
fn analyze(tool: &Path, definitions: &Path, files: &[PathBuf]) -> Vec<String> {
    let output = Command::new(tool)
        .arg("analyze")
        .arg("--platform")
        .arg("standard")
        .arg(format!("--definitions=@box={}", definitions.display()))
        .args(files)
        .output()
        .expect("run luau-lsp");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !l.starts_with("[INFO]"))
        .filter(|l| !l.contains("LocalUnused"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn definitions_load_in_luau_lsp_and_type_the_examples() {
    let Some(tool) = luau_lsp() else { return };
    let dir = std::env::temp_dir().join(format!("box_luau_defs_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let definitions = dir.join("box.d.luau");
    std::fs::write(&definitions, Game::new().script_definitions()).unwrap();

    // The example projects (data projects run with the stock game). Cinder
    // Keep uses rigid-body APIs (`physics.apply_impulse`, ...), which a build
    // without `dynamics` does not declare: luau-lsp rightly reports them there.
    let mut projects = vec!["collector_luau", "script_demo", "script_rules"];
    if cfg!(feature = "dynamics") {
        projects.push("cinder_keep");
    }
    for project in projects {
        let mut files = Vec::new();
        scripts(
            &root().join("examples").join(project).join("assets"),
            &mut files,
        );
        assert!(!files.is_empty(), "{project}: no scripts");
        let diagnostics = analyze(&tool, &definitions, &files);
        assert!(
            diagnostics.is_empty(),
            "{project}: luau-lsp reported:\n{}",
            diagnostics.join("\n")
        );
    }

    // A strict probe: the declared types are visible and checked (exactly one
    // deliberate error).
    let probe = dir.join("probe.luau");
    std::fs::write(
        &probe,
        r#"--!strict
local function f(e: Entity, t: TransformPatch, i: RuleIntent, o: RuleOut, n: UiNode, c: Color)
    local p: vector = e:position()
    local bad: string = e:position()
    e:set("Transform", { translation = p })
    o:part(i, "post", { Primitive = {} })
    local q = Quat.from_rotation_y(1) * p
    print(t, n, c, bad, q)
end
print(f)
"#,
    )
    .unwrap();
    let diagnostics = analyze(&tool, &definitions, &[probe]);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected one deliberate error:\n{}",
        diagnostics.join("\n")
    );
    assert!(
        diagnostics[0].contains("Expected this to be 'string', but got 'vector'"),
        "{}",
        diagnostics[0]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
