# Spike S7: export pipeline (R16)

Question: can box-engine turn a project into a runnable macOS `.app` and a
Windows folder with no toolchain on the user's machine (a prebuilt player plus
a data pack), and which pack format should it use?

Run on an Apple M3 Ultra, macOS 26.6.2, Rust 1.98.1, release build:

```
cargo build --release                          # in spikes/s7_export
target/release/s7_export pak ../../examples/{collector_luau,cinder_keep,script_demo,collector} --synthetic out/synthetic
target/release/s7_export app "$PWD/out" ../../apps/editor/macos/AppIcon.png
pe_sample/build.sh                             # needs `rustup target add x86_64-pc-windows-msvc`
target/release/s7_export pe pe_sample/tiny.exe ../../apps/editor/macos/AppIcon.png out/patched.exe
```

## (d) Pack format

Candidates: a custom indexed archive (header, files back to back 16-byte
aligned, index at the end with xxh3-128 per file and a hash of the index in
the header), zip stored (no compression), zip deflate, and loose files.
Contents: `box.project.json` plus `assets/**` with `.meta` files (what a
cooked game needs). Times are medians of 31 runs (5 for the synthetic
project), warm file cache, "read all" = open the archive and read every file
into memory (the custom pak with and without verifying each file's hash).

| project | files | loose bytes | pak | zip stored | zip deflate | read: loose | pak (verify) | pak (no verify) | zip stored | zip deflate | open pak index |
|---|---|---|---|---|---|---|---|---|---|---|---|
| collector_luau | 39 | 266,387 | 269,254 | 271,787 | 106,544 | 0.41 ms | 0.068 ms | 0.058 ms | 0.16 ms | 0.51 ms | 0.014 ms |
| cinder_keep | 47 | 653,038 | 656,357 | 659,402 | 108,286 | 0.56 ms | 0.097 ms | 0.079 ms | 0.20 ms | 0.68 ms | 0.020 ms |
| script_demo | 23 | 40,649 | 42,304 | 43,745 | 24,535 | 0.26 ms | 0.032 ms | 0.032 ms | 0.091 ms | 0.19 ms | 0.012 ms |
| collector | 15 | 154,053 | 155,213 | 156,185 | 84,822 | 0.18 ms | 0.035 ms | 0.032 ms | 0.073 ms | 0.34 ms | 0.011 ms |
| synthetic (2,000 x 1-64 KiB + 8 x 8 MiB) | 2,009 | 133.6 MB | 133.7 MB | 133.8 MB | 83.5 MB | 40 ms | 17 ms | 13.5 ms | 21 ms | 663 ms | 0.24 ms |

Writing: the synthetic project takes 103 ms as a pak, 430 ms as a stored zip
and 1.9 s as a deflated zip. (For the small projects the pak was written first
and paid the cold file reads, so its write times there are not comparable.)

Hashes over 64 MiB: xxh3-128 42 GB/s, blake3 (pure Rust, one thread)
1.1 GB/s, sha256 0.5 GB/s. Verifying every file with xxh3 costs 10-25% of a
read; blake3 or sha256 would cost more than the read itself.

Corruption: flipping one data byte is reported by the file's hash when that
file is read ("content hash mismatch"); flipping one index byte is reported
when the pak is opened ("index hash mismatch").

Determinism: writing the same project twice gives identical bytes (sorted
paths, no timestamps; zip needs `last_modified_time` pinned for the same).

**Decision: a custom pak** (`BOXPAK\r\n`, version 1): uncompressed, sorted
index at the end, xxh3-128 per file (verified on every read) and over the
index (verified at open). It reads 2.3x faster than a stored zip and 4-6x
faster than loose files here, opens in microseconds, needs no dependency
beyond a hash, and is trivially deterministic. Zip deflate reads 7-40x slower
for a 1.2-6x smaller file; download size is the installer's job (a `.dmg` or
`.zip` of the bundle compresses anyway). No memory mapping: `unsafe` is denied
workspace-wide and positioned reads are already fast; mapping stays possible
since files are 16-byte aligned. Zip stays the documented fallback.

## (a) A prebuilt player against a pak

The player reads the project from disk in more places than the asset server:
`Project::open` (`box.project.json`), the start scene and runtime scene
switches (`scenes.rs`), settings files and `input.actions.json`
(`settings.rs`), the asset index scan and `.meta` files, importer dependency
reads (glTF buffers, external images), Luau `require` (`box_script`), and the
registry file a native player writes at startup.

**Decision:** a small virtual file system in `box_assets` (`vfs`): a pak is
*mounted* at its own path (`/x/Game.app/Contents/Resources/game.pak`), and
`vfs::read`, `is_file`, `is_dir`, `read_dir` serve paths below that root from
the pak and everything else from the disk. Paths stay `PathBuf`s, so the
project root of a packed game is simply `<pak path>`, scene paths are
`<pak>/assets/scenes/x.scene.json`, and every call site above only swaps
`std::fs::read` for `vfs::read`. The asset server treats a mounted root as
read-only: no `.meta` writes, no file watching. Proven end to end by the M9
export tests (the player runs the examples from a pak with no project folder).

## (b) macOS `.app` without Xcode

- An `.app` is a folder: `Contents/MacOS/<exe>`, `Contents/Info.plist`,
  `Contents/Resources/{AppIcon.icns, game.pak}`. The plist is plain XML we
  generate (escape `&`, `<`, `>`: the test name "S7 Spike & Test" works).
- The icon: `sips` + `iconutil` (both ship with macOS) turn a 1024 px PNG
  into `AppIcon.icns`. Without them (or for a non-Mac host) the bundle simply
  has no icon; an `.icns` can also be written directly (it is a list of PNGs).
- Signing: `codesign --force --sign - --timestamp=none <app>` (ships with
  macOS, no Xcode) takes ~25 ms; `codesign --verify --strict` then reports
  "valid on disk / satisfies its Designated Requirement". The binary cargo
  builds is already *linker-signed* ad-hoc, but the bundle signature seals
  Info.plist and Resources, so signing must be the last step: editing
  Info.plist afterwards fails verification ("invalid Info.plist (plist or
  signature have been modified)"). The pak in Resources is sealed too.
- Launch: `open -W -n <app> --args <file>` (LaunchServices, as Finder does)
  runs it; the launched process saw its arguments, its executable path inside
  the bundle and `Contents/Resources/game.pak`. **Its working directory is
  `/`**, so the player must locate the pak from `current_exe()`, never the
  working directory.
- Gatekeeper: `spctl -a -t exec` rejects the ad-hoc bundle, as expected. A
  locally built bundle has no quarantine attribute and opens normally. A
  downloaded one (quarantined) is blocked; the user must right-click > Open
  (or `xattr -dr com.apple.quarantine <app>`), or the developer must sign with
  a Developer ID and notarize with their own credentials (out of scope;
  `rcodesign` can do both from non-Mac hosts).

## (c) Windows bundle

- Layout: `<Name>/<Name>.exe` (the prebuilt `box-player.exe` renamed) next to
  `game.pak`. The player finds `game.pak` next to its executable.
- **`editpe` 0.2.4** (BSD-2-Clause, pure Rust, deps zerocopy/indexmap/image)
  patches PE resources from macOS: on a sample PE built here with rustc's
  Windows target and `rust-lld` (`pe_sample/`, 2 KB, no resources), it set
  the main icon from a PNG (resized to the icon sizes), the version info
  (FileVersion, ProductName, ...) and the subsystem CONSOLE -> GUI in 40 ms,
  and re-reading the output returns all three; `file` reports
  "PE32+ executable (GUI) x86-64". The subsystem switch is how the release
  profile hides the console window with one prebuilt template.
- Not verified here (needs Windows): that the patched real `box-player.exe`
  starts, shows the icon in Explorer and the taskbar, and shows the version
  in Properties. Patching invalidates an Authenticode signature, so
  templates ship unsigned and the user signs the final exe (`signtool`,
  or `osslsigncode` from macOS) if they want to.
- The Windows template must be built on Windows (CI): cross-compiling the
  full player (wgpu, Luau through `cc`, audio) from macOS needs the MSVC SDK
  (`cargo-xwin`), which ASSETS.md section 7 already rules out.

## Texture compression

`texture-compressor` 0.2.4 (MIT OR Apache-2.0, pure Rust, deterministic BC1-7
encoder) is suitable. Using it needs a compressed format in `ImageData`
(today RGBA8 only), BC uploads in the renderer and a `TEXTURE_COMPRESSION_BC`
check with an RGBA fallback. See docs/EXPORT.md for the outcome.
