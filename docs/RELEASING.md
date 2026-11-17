# Releasing

How an engine release is built and tested (M9: "a user can install
box-engine, create a project, write a script, and export a runnable game"
without Rust). The scripts build releases **locally**: nothing in this
repository publishes, tags, uploads, or signs with a real identity. Those
steps belong to whoever publishes a release.

## 1. Version

All crates share the workspace version (`[workspace.package] version` in
`Cargo.toml`). `box --version`, `box-editor --version` and
`box-player --version` print it, the player templates carry it in
`templates/VERSION` (an export refuses templates of another version), and
projects record it as `engine_version`. Bump it before a release; what each
version promises is in [STABILITY.md](STABILITY.md).

## 2. Build a macOS release

```bash
scripts/release/package-macos.sh
```

On a Mac (Apple Silicon), about 40 s on top of a warm release build. It
builds `box`, `box-editor` and `box-player` in release mode, wraps the
editor with `apps/editor/macos/bundle.sh`, and writes:

```
target/release-dist/box-engine-<version>-macos/
  Box Editor.app/Contents/
    Info.plist                       dev.box-engine.editor, the version
    MacOS/box-editor                 the editor (the app's executable)
    MacOS/box                        the CLI
    Resources/AppIcon.icns
    Resources/templates/VERSION      the engine version
    Resources/templates/macos/box-player          the macOS player template
    Resources/templates/windows/box-player.exe    only when CI provided it
    Resources/templates/projects/    only when templates/projects/ exists
  bin/box, bin/box-editor, bin/box-player         relative links into the app
  install.sh                         installer (no sudo), --uninstall
  README.txt                         install, first steps, where the docs are
  docs/guide/, docs/reference/, docs/TROUBLESHOOTING.md, docs/STABILITY.md
  examples/                          the script-only examples, as tracked in git
  LICENSE.txt
target/release-dist/box-engine-<version>-macos.zip   ditto, links kept, no xattrs
target/release-dist/box-engine-<version>-macos.dmg   when hdiutil works
```

Each program is in the release once. `box-player` is the macOS player
template itself; the commands in `bin/` (and the ones `install.sh` makes) are
links into the app. Programs find their templates from their real location
(`box_export::template` resolves symlinks), so `box export` from
`~/.local/bin/box` uses `Resources/templates` of the installed app. The
script checks that all three programs report the workspace version.

Signing: each program and then the app are signed ad hoc
(`codesign --force --sign - --timestamp=none`) and verified with
`codesign --verify --strict --deep`. The release is **not notarised**;
`install.sh` removes the download quarantine from the installed app, and
the README says to right-click ▸ Open an app copied by hand. A publisher
who has a Developer ID re-signs the programs and the app with it
(`codesign --force --options runtime --timestamp --sign "Developer ID
Application: …"`, programs first), notarises the zip or dmg with
`xcrun notarytool submit --wait`, and staples the ticket
(`xcrun stapler staple`). The engine never does these steps itself.

### Slots filled from outside

| Slot | Source | When missing |
|---|---|---|
| `Resources/templates/windows/box-player.exe` | `target/templates/windows/box-player.exe`, built on Windows by CI (docs/EXPORT.md §6), copied by `bundle.sh` | Reported; the release exports macOS games only, and `box export --target windows` says where templates come from |
| `Resources/templates/projects/` | the repository's `templates/projects/` folder (the variable `project_templates` in `package-macos.sh`) | Reported; `box new` makes its built-in default project |

The Windows template must be built from the same commit (same version):
`templates/VERSION` covers both platforms. How CI builds it and how a
Windows release is packaged is documented separately with the Windows work.

## 3. What `install.sh` does

- Copies `Box Editor.app` to `~/Applications` (`--app-dir <dir>`, for
  example `/Applications` when writable), replacing an earlier install.
- Removes the quarantine attribute from the installed copy.
- Links `box`, `box-editor` and `box-player` into `~/.local/bin`
  (`--bin-dir <dir>`); refuses to replace a file that is not a link.
- Prints `box --version`, and the line to add to `~/.zprofile` when the
  folder is not on `PATH` (or which `box` comes first when another one is).
- `--uninstall` removes the links that point into the app, then the app
  (only if its bundle id is `dev.box-engine.editor`). Projects, the editor's
  recent projects (`~/Library/Application Support/box-editor`) and logs
  (`~/Library/Logs`) are kept.

## 4. The clean-machine test

```bash
scripts/release/clean-machine-test.sh [target/release-dist/box-engine-<version>-macos.zip]
cargo test -p box_export --test clean_machine -- --ignored --nocapture   # packages first, then the same script
```

It simulates a Mac with nothing but macOS and the release: every step runs
under `env -i` with a new, empty `HOME` and `PATH` limited to the system
folders and the install's `~/.local/bin`, after checking that `cargo`,
`rustc` and `rustup` are not found. Then, with the installed programs only:

1. unpack the zip (`ditto -x -k`, as Archive Utility), run `install.sh`;
2. the three `--version`s agree and are the installed links; the programs
   link only macOS libraries (`otool -L`); the app's signature verifies;
3. `box new my-game` (`BOX_TEST_TEMPLATE=<name>` uses
   `box new --template <name>` once templates exist);
4. the getting-started guide's first script and its `box scene apply`
   commands, **copied out of the guide shipped in the release**, so the test
   breaks when the guide does;
5. `box check`, `box run --frames 120`, `box render` (non-blank PNG);
6. `box export --target macos --profile release`; the `.app` copied to
   another folder, its signature verified, run headless twice (frames 5
   and 35): both captures non-blank and different (the spinner turned the
   cube, so the script is in the pak), exit 0, which also means no wgpu
   validation errors;
7. `box export --target windows` when the release has the Windows
   template (layout check only; running it needs Windows), otherwise
   reported as skipped;
8. `box-editor my-game --capture` (non-blank);
9. `install.sh --uninstall` removes the app and the links and leaves the
   project.

Captures are copied to `target/captures/clean-machine/`; look at them. On
failure the test folder is kept and its path printed. The PNG and library
checks run outside the clean environment with the host's `python3`
(`scripts/release/png_check.py`, standard library only) and `otool`.

Measured (Apple M3 Ultra, macOS 26.6, warm caches, three runs): the script
takes 7-12 s: 0.2-0.3 s to unpack, 0.7-0.8 s to install, about 50 ms for
`box new`, 20-30 ms for `box check`, 0.4-2.4 s for `box run --frames 120`
(the first GPU start after a build is the slow one), 0.5-0.6 s for
`box render`, 0.23-0.27 s for the export, 2.7-4.9 s for the two exported runs
including the pure-Python PNG checks, 0.7-2.0 s for the editor capture
(2560x1440). The ignored Rust test takes about 36 s when the release build
is up to date (packaging included). Sizes for 0.1.0: folder 124 MB, zip 44 MB, dmg 48 MB.

It does not replace a test on a second Mac that never had the engine or
Xcode (Gatekeeper on a really downloaded, quarantined zip; a GPU other than
the build machine's). Do that by hand before publishing: download the
artefact, follow [the getting-started guide](guide/getting-started.md)
from "Install a release" to "Export your game".

## 5. Checklist

1. Version bumped; release notes written (unstable Rust API
   changes listed, per STABILITY.md).
2. `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` pass.
3. CI's Windows template in `target/templates/windows/` (same commit).
4. `scripts/release/package-macos.sh`: both slots reported as expected.
5. `scripts/release/clean-machine-test.sh` passes; captures reviewed.
6. Manual test on a clean Mac (above) and the Windows clean-machine test.
7. The publisher signs, notarises and uploads; nothing here does.
