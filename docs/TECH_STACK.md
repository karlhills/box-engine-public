# Tech Stack

Dependencies selected for box-engine, with versions checked on **2026-11-14**. Each row says whether the claim was confirmed against a primary source in the research pass or is an assumption still to be verified.

Legend: **C** = confirmed from crates.io, the project repository or its documentation on 2026-11-14. **A** = assumption or from general knowledge; verify before relying on it.

## 1. Toolchain

| Item | Choice | Notes |
|---|---|---|
| Rust | 1.98.1, pinned in `rust-toolchain.toml`; MSRV 1.95 | Raised from 1.94 on 2026-11-15: bevy_reflect 0.20.1, egui 0.37 and egui_tiles 0.18 need 1.95 (spikes S1, S2). |
| Edition | 2024 | A |
| macOS linker | system default | Faster than LLD on macOS per Bevy's fast-build config (C). |
| Windows linker | `rust-lld` | C (Bevy fast-build config) |
| Dev profile | `opt-level = 1`, dependencies at `opt-level = 3`, `debug = "line-tables-only"` | wgpu CI uses line-tables-only (C). Build times unmeasured (A). |
| CI | GitHub Actions: `macos-26` (arm64) and `windows-2022` | Both are what wgpu itself uses (C). |

## 2. Core dependencies

| Area | Crate | Version | Status | Why |
|---|---|---|---|---|
| GPU | `wgpu` (+ `naga`) | 30.0.2 | C | First-class Metal, Vulkan, DX12. Breaking major roughly every three months. |
| Window and input | `winit` | 0.30.13 | C | 0.31 is still in beta with large breaking changes; egui-wgpu targets 0.30. Confined to `box_engine` and `box_editor`. |
| Math | `glam` | 0.33.12 | C | Pinned to the version bevy_reflect's `glam` feature uses (`^0.33.2`) so math types are reflectable without wrappers (spike S2). Upgrade glam and bevy_reflect together. parry3d 0.31.1 uses `glamx` 0.3.1, which re-exports glam `^0.33.7`; it unifies with 0.33.12 (checked in M3), so parry math types are our glam types. |
| Entity storage | `hecs` | 0.11.2 | C | Small, stable, "a library, not a framework". Used through `box_core::World`; query types are re-exported, not wrapped. |
| Reflection | `bevy_reflect` | 0.20.1 | C | Standalone: depends only on bevy_platform, bevy_ptr, bevy_utils and its derive (C). Re-exported as `box_core::reflect`. Breaks each Bevy cycle (A). |
| Serialization | `serde`, `serde_json` | current | A | Scene and project files. |
| Jobs | `rayon` | current | A | Mesh generation, imports. |
| IDs | `uuid` | current | A | Entity `Uid` and `AssetId`. |
| Hashing | `blake3` | 1.8.7 | C | Import cache and generator cache keys. |
| Shaders | plain WGSL with our own include step plus `import box::<module>;` | — | C | `wesl` 0.6.0 evaluated at M4 and not adopted: rustc 1.97.1 minimum, about 12 dependencies including a second WGSL parser, identifier mangling hurts error mapping, pre-1.0 (D1). |
| Errors | `thiserror` 2, `anyhow` 1 | C | Libraries versus applications. |
| Logging | `tracing` | 0.1.44 | C | |
| Profiling | `profiling` 1.0.18 with `tracy-client` | C | CPU scopes. `wgpu-profiler` 0.28.0 (C) is deferred: it would add a second constraint on wgpu upgrades and pins tracy-client 0.18 (C). |
| Gamepad | `gilrs` | 0.11.2 | C | winit has no gamepad support. 0.11.2 released 2026-05-30 (0.11.1 2026-01-12); actively maintained on GitLab (gilrs-project/gilrs); MSRV 1.84. Default feature `wgi` (Windows Gaming Input). Wrapped by `box_engine::GamepadPoller`; gilrs types stay in box_engine. |

## 3. Editor

| Area | Crate | Version | Status | Notes |
|---|---|---|---|---|
| UI | `egui`, `egui-winit`, `egui-wgpu` | 0.37.0 | C | egui-wgpu 0.37 requires wgpu ^30 and winit ^0.30.13 (C). We do **not** use `eframe`; the engine owns the device and loop. |
| Docking | `egui_tiles` | 0.18.0 | C | Maintained by Rerun, tracks egui 0.37, serde layouts. `egui_dock` 0.21.1 is still on egui 0.36 (C). |
| Tree view | `egui_ltreeview` | 0.10 | C | Tracks egui 0.37. Or a custom flattened virtual list. |
| Gizmos | own (`box_editor::gizmo`) | — | — | Built in M2 on the display-overlay line layers. |
| File dialogs | `rfd` | 0.18.0 | C | Native folder dialogs for New/Open Project (owner approved 2026-11-15). Pulls objc2 0.6 next to winit's objc2 0.5 on macOS (duplicate, accepted). |

## 4. Optional modules

| Area | Crate | Version | Status | Notes |
|---|---|---|---|---|
| Scripting | `mlua` with the Luau backend (`luau`, `vendored`, `serialize`) | 0.12.2 (Luau 0.740) | C | Used only by `box_script`. `Lua::sandbox`, `set_interrupt` (watchdog), native `vector`, `Table::set_safeenv`. Spike S4. |
| Spatial queries | `parry3d` | 0.31.1 | C | Raycast, shapecast, overlap, BVH. |
| Dynamics | `rapier3d` | =0.36.1 | C | `box_physics` `dynamics` feature, default-on in box_engine; initialized only while a `RigidBody` exists. Depends on parry3d 0.31.1 and glamx 0.3.1 (shared with the query layer; one parry, one glam). Adds nalgebra 0.35. Fast bodies are always swept against static colliders; `ccd` adds moving bodies. Clean dev build of rapier3d: about 25 s. Pinned: frequent breaking releases. The built-in character controller is not used (it needs rapier's query pipeline). |
| Audio | `kira` | 0.12.6 | C | In `box_audio` since M6, with `default-features = false` and features `cpal`, `ogg`, `vorbis`, `mp3`, `flac`, `wav`, `pcm` (no `cpal-realtime`: avoids dbus on Linux). Brings its own glam 0.34 (duplicate; positions cross through `mint` 0.5.9). The device opens in about 160 ms on macOS (done on a background thread). Latest on crates.io 2026-11-15. |
| 2D booleans | `i_overlay` | 9.1.0 | C | In `box_proc::poly` since M5: booleans, offsets, simplify; i32 grid, NonZero fill. 9.2.0 released 2026-11-15, not adopted yet. |
| Triangulation | `earcut` | 0.4.12 | C | In `box_proc::poly::triangulate` since M5 (f64, holes). |
| Mesh optimization | `meshopt` | 0.7.0 | C | Import-time. |
| Tangents | `bevy_mikktspace` | 1.0.0 | C | Standalone crate. |

## 5. Assets

| Area | Crate | Version | Status | Notes |
|---|---|---|---|---|
| glTF | `gltf` | 1.4.1 | C | In use since M4 with `default-features = false` and features `utils`, `names`, `KHR_materials_emissive_strength`; buffers, data URIs and images are handled by box_assets. No release since 2024-05 but still receives fixes. No Draco, meshopt or BasisU support (C). |
| Images | `image` | 0.25.10 | C | PNG, JPEG, and HDR sources. |
| EXR | `exr` | 1.74.3 | C | Environment maps, when needed. |
| Compressed textures | `ktx2` 0.5.0 (parse), `ctt` 0.6.1 or `intel_tex_2` 0.5.0 (encode) | C | BC formats work on Apple Silicon and NVIDIA through wgpu (C, from wgpu-hal source). Verify on hardware. |
| Audio decode | `symphonia` | 0.6.1 | C | Through kira 0.12.6 (resolved 0.6.1); never used directly. |
| File watching | `notify` 8.2.2, `notify-debouncer-full` 0.7.0 | C | FSEvents can emit spurious metadata events (C). |
| Packaging | own uncompressed pak, or `zip` 9.5.1 | C/A | Decide at M9. |

## 6. Testing

| Area | Crate | Version | Status |
|---|---|---|---|
| Image comparison | own thresholded pixel difference on `image` | — | `nv-flip` 0.1.2 (C, used by wgpu) is the upgrade if a perceptual metric is needed; it brings a C++ build. |
| Snapshots | `insta` | 1.49.0 | C |
| Property tests | `proptest` | 1.12.0 | C |
| Benchmarks | `criterion` | 0.8.2 | C |

## 7. Things evaluated and rejected

| Candidate | Reason |
|---|---|
| `bevy_ecs` | Capable and usable standalone (C), but high churn (resources became components in 0.19, C), heavy compile cost (A), and Bevy concepts would leak into our scripting and plugin API. |
| `bevy_render` | Hard dependency on `bevy_ecs` (C). |
| `rend3` | Archived June 2025 (C). |
| `flecs_ecs` | C dependency, slow releases, small Rust user base (C/A). |
| `evenio`, `legion` | Unmaintained (C). |
| Iced | `iced_wgpu` pins wgpu 27, no official docking, no release in eleven months (C). |
| Slint | GPL / royalty-free / commercial licensing (C) is awkward for an open engine; no docking. |
| gpui | Best native look, but Metal-only on macOS with no shared wgpu device, and driven by Zed's needs (C). Runner-up if egui cannot meet the visual bar. |
| Tauri / webview | Cannot share a wgpu device; Graphite moved away from it (C). |
| Rhai | Slow tree-walking interpreter (A) and stale LSP (C). |
| WebAssembly scripting | Strong sandbox and multi-language story, but per-language toolchains and a weaker debugging loop. Deferred, not rejected. |
| C# via `netcorehost` | Would allow reusing existing C# code, but adds a large runtime and binding surface. Deferred. |
| `rapier` for the construction slice | It needs only queries; `parry3d` alone covers it. |
| Dylib hot reload of Rust (`hot-lib-reloader`, `subsecond`) | Experimental and fragile (C for subsecond's own caveats). Rust changes restart the editor instead. |
| `cross` | No release since 2023 (C). Windows builds happen on CI. |

## 8. Version management policy

- Pin every version in the workspace `Cargo.toml` and commit `Cargo.lock`.
- **wgpu upgrade rule:** upgrade only when `egui-wgpu` supports the new major. Treat each upgrade as its own change with golden-image tests run before and after.
- `bevy_reflect`, `mlua`, `rapier3d` and `egui` are all fast-moving. Each is used through one of our crates so that most application code does not name them directly.
- Before M1 begins, re-check the versions in this file; several will have moved.

## 9. Open verification items

1. ~~Exact `glam` version shared by `parry3d` 0.31 and our workspace.~~ Resolved in M3: parry3d 0.31.1's `glamx` 0.3.1 re-exports glam `^0.33.7`, which unifies with 0.33.12.
2. BC7, BC5 and BC6H (including sRGB variants) actually sampling correctly on M-series through wgpu.
3. `static-dxc` build on the Windows CI runner and its binary-size cost.
4. Whether Metal on the standard `macos-26` runner is deterministic enough for golden images.
5. ~~`wesl` behaviour under hot reload and its error reporting quality, evaluated at M4.~~ Answered in M4: not adopted (D1).
6. Clean and incremental build times of the M1 workspace on the Mac Studio.

## 10. References

Primary sources consulted on 2026-11-14:

- wgpu: <https://github.com/gfx-rs/wgpu>, <https://github.com/gfx-rs/wgpu/releases>, <https://docs.rs/wgpu/30.0.2/wgpu/struct.Features.html>
- winit releases: <https://github.com/rust-windowing/winit/releases>
- WESL: <https://wesl-lang.dev>; Bevy 0.20 notes: <https://bevy.org/news/bevy-0-20/>; Bevy 0.19 notes: <https://bevy.org/news/bevy-0-19/>
- egui: <https://github.com/emilk/egui/releases>, <https://docs.rs/egui-wgpu/latest/egui_wgpu/struct.Renderer.html>
- rend3 archive: <https://github.com/BVE-Reborn/rend3>
- hecs: <https://github.com/Ralith/hecs>; bevy_ecs: <https://docs.rs/bevy_ecs/latest/bevy_ecs/>; bevy_reflect: <https://docs.rs/bevy_reflect/>
- mlua: <https://docs.rs/crate/mlua/0.12.2>; LuaJIT install notes: <https://luajit.org/install.html>
- subsecond: <https://docs.rs/subsecond/>; Fyrox hot reload: <https://fyrox-book.github.io/beginning/hot_reloading.html>
- Rapier changelog: <https://github.com/dimforge/rapier/blob/master/CHANGELOG.md>
- kira: <https://github.com/tesselode/kira>
- gltf: <https://github.com/gltf-rs/gltf>; ctt: <https://github.com/cwfitzgerald/ctt>
- Godot export and macOS signing: <https://docs.godotengine.org/en/stable/tutorials/export/exporting_projects.html>, <https://docs.godotengine.org/en/stable/tutorials/export/exporting_for_macos.html>
- Godot scene format: <https://docs.godotengine.org/en/stable/engine_details/file_formats/tscn.html>
- Godot embedded game window on macOS: <https://godotengine.org/article/dev-snapshot-godot-4-5-dev-4/>
- Unity play mode configuration: <https://docs.unity3d.com/Manual/ConfigurableEnterPlayModeDetails.html>
- Townscaper: <https://www.gamedeveloper.com/game-platforms/how-townscaper-works-a-story-four-games-in-the-making>
- Tiny Glade interview: <https://80.lv/articles/exclusive-tiny-glade-developers-discuss-bevy-proceduralism-publishers-cozy-games>
- Unreal Geometry Script: <https://dev.epicgames.com/documentation/unreal-engine/geometry-scripting-users-guide-in-unreal-engine>
- Crate versions: `https://crates.io/api/v1/crates/<name>`
