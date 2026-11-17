# Contributing to box-engine

Thank you for helping. box-engine is a Rust 3D game engine with an integrated editor, licensed under the [MIT licence](LICENSE). By contributing you agree that your contribution is licensed under the MIT licence too.

[AGENTS.md](AGENTS.md) is the working guide for this repository: layout, commands, conventions and gotchas. It applies to human and AI contributors alike. Read it before your first change. This file sums up what a contribution needs.

## Build

- **Toolchain.** `rust-toolchain.toml` pins the Rust toolchain (1.98.1; the minimum supported version is 1.95). On macOS the GPU backend is Metal, and on Windows DX12 or Vulkan.

```bash
cargo build --workspace
cargo run -p box_editor_app -- [project_dir]     # the editor
cargo run -p box_player_app -- examples/cinder_keep   # play a project
cargo run -p box_cli -- --help                   # the agent CLI (`box`)
```

## Test

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                       # unit tests + golden images (needs a GPU)
BOX_SKIP_GPU_TESTS=1 cargo test --workspace  # what CI runs (no GPU)
```

Focused suites (the full list is in AGENTS.md "Commands"):

```bash
cargo test -p box_engine --test m8_game      # Cinder Keep played start to finish
cargo test -p box_engine --test luau_definitions   # luau-lsp over every example (when luau-lsp is on PATH)
cargo test -p box_script                     # script runtime
cargo test -p box_editor --test play         # editor Play mode
cargo test -p box_cli --test mcp             # the MCP server
BOX_UPDATE_GOLDEN=1 cargo test -p box_engine --test golden   # re-record golden images: review every PNG
```

**Script API changes** have extra steps:

- **Regenerate the committed type definitions:**

  ```bash
  cargo run -p box_cli -- script types --project examples/cinder_keep
  cargo run -p box_cli -- script types --project examples/collector_luau
  ```

- **Run `luau_definitions` with `luau-lsp` (1.70.1 or later) on `PATH`.**

## Definition of done

A change is done when every item below holds (AGENTS.md "Definition of done"):

1. `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass.
2. **Visual changes have a capture.** Render with `--capture`, open the PNG and look at it. Code reading alone is not verification. Name the capture in the pull request.
3. Golden images still match, or were re-recorded on purpose and the new PNGs reviewed.
4. There are no wgpu validation errors (`FrameStats::validation_errors == 0`).
5. There is no unused scaffolding, stub or unwired option.
6. The docs are updated when behaviour, commands or decisions change: AGENTS.md, the docs/ page for the area, and SCRIPTING.md for script API changes.

**Bug fixes.** Prove the cause before you fix it: a failing test, a logged value or a rendered frame. The fix comes with the test that failed without it.

## Code style

- **Formatting.** `rustfmt` with the repository's `rustfmt.toml`. `unsafe` is denied workspace-wide.
- **Errors.** Library crates return `Result` with `thiserror` errors. Binaries and examples use `anyhow`. Never panic on bad input or bad assets.
- **Logging.** Through `tracing` under a `box_*` target. The editor turns warnings and errors into toasts. Never drop an error silently.
- **Per-frame work.** Expensive work (mesh rebuilds, buffer reallocation, shader compilation) is change-driven or throttled, never run every frame. State the per-frame cost of new systems in the pull request.
- **Data and edits.**
  - Scene data is reflected: AGENTS.md "Conventions" has the derive pattern.
  - Scene edits go through `box_core::edit`.
  - Games are plugins.
- **Dependency direction** (AGENTS.md "Layout"). Keep winit types in `box_engine` and `box_editor`, and wgpu types in `box_render` and `box_editor`.
- **Doc comments.** Write them for every public item. In a `#[derive(Reflect)]` type the doc comment is the inspector tooltip. Script API items carry a `---` doc line in their `definitions()`.
- **Conventions.** Right-handed, +Y up, −Z forward, metres, radians, linear colours.

## Compatibility

[docs/STABILITY.md](docs/STABILITY.md) says what users can rely on. In short:

- **Stable script API items.** Never rename or remove one directly. Add the new name, keep the old one through `box_script::deprecated_alias` (it warns once with the caller's line), and record both in docs/API_REVIEW.md and the release notes. The old name may go two minor releases later.
- **Scene, project, `.meta` or save formats.** Bump the version and add a migration, so older files open. Never drop unknown data.
- **Native Rust API.** It is unstable in 0.x. Breaking changes are allowed but go in the release notes.

## Proposing changes

- **Small fixes** (bugs, docs, tests): open a pull request directly.
- **Larger changes**: open an issue first describing the problem and the proposed shape. This covers new subsystems, script API additions, format changes and new dependencies. Decisions with trade-offs are recorded in [docs/DECISIONS.md](docs/DECISIONS.md), and the plan lives in [docs/ROADMAP.md](docs/ROADMAP.md).
- **Scope.** Keep each pull request to one concern and explain what it changes and how it was verified: tests, captures, measurements. Build the simplest thing the current milestone needs; later-milestone ideas belong in the docs.
- **Commits.** Use imperative subjects with a scope, such as `fix(script): on_load runs after on_start returns`, and a body that says why.
- **Shared branches.** Another contributor (or agent) may be editing the same branch. Commit only the files you changed, and never revert work you did not do.
