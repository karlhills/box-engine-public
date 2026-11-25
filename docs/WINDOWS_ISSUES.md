# Windows issues

Findings from the first Windows run (D9: Windows CI and a manual smoke run on an NVIDIA PC), and the fixes applied. Machine: Windows 11 Pro, NVIDIA GeForce RTX 5070 Ti (driver 621.14), Rust 1.98.1 (`rust-toolchain.toml`), Visual Studio 2022 Build Tools 14.44. Branch `ci/enable-github-actions`.

Status key: **Fixed** (committed on this branch), **Open** (not fixed yet), **Expected** (known and accepted).

## Summary

| # | Issue | Area | Status |
|---|---|---|---|
| 1 | GitHub Actions workflow was parked | CI | Fixed |
| 2 | Linking fails without the MSVC ATL component (`atls.lib`) | Toolchain | Fixed (docs + local install) |
| 3 | Four example crates had an example named `author`; links collide on Windows | Build | Fixed |
| 4 | Git checks out text files with CRLF; byte-for-byte file tests fail | Repo | Fixed |
| 5 | DX12: shadows on the wrong objects, skinned meshes invisible, shader-material instancing broken | Renderer | Fixed |
| 6 | Golden images differ from the Metal references | Tests | Expected (M9 parity) |
| 7 | Remaining test failures, triaged (7a–7g) | Tests | Fixed (7f not reproduced) |
| 8 | `box new --native` writes `//?/C:/...` engine paths that Cargo rejects | CLI | Fixed |
| 9 | Clippy `result_large_err` on Windows only (`ProjectError`) | Build/CI | Fixed |
| 10 | macOS CI: drag tests' regeneration counts depended on runner timing | CI | Fixed (not Windows) |

## 1. CI workflow was parked (Fixed)

`ci/github-actions-ci.yml` moved to `.github/workflows/ci.yml` (already targets `windows-2022` and `macos-26`, `BOX_SKIP_GPU_TESTS=1`). It runs on pushes to `main` and on pull requests. AGENTS.md's CI section updated.

## 2. MSVC ATL component required (Fixed: documented)

Symptom: every binary fails to link with `LINK : fatal error LNK1104: cannot open file 'atls.lib'`.

Cause: wgpu's DX12 backend links `mach-dxcompiler-rs` (a statically built DXC), which needs the ATL libraries. The Build Tools "Desktop development with C++" workload does not include ATL by default.

Fix: install the `Microsoft.VisualStudio.Component.VC.ATL` component (Visual Studio Installer ▸ Modify ▸ Individual components ▸ "C++ ATL for latest build tools"), or from a shell:

```
"C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe" modify --installPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" --add Microsoft.VisualStudio.Component.VC.ATL --passive --norestart
```

The GitHub `windows-2022` image has full Visual Studio with ATL, so CI is not affected. Added to AGENTS.md Gotchas.

## 3. `author` example name collision (Fixed)

Symptom: `cargo test --workspace` fails with `LNK1104: cannot open file 'target\debug\examples\author.exe'`, preceded by cargo's `warning: output filename collision`.

Cause: several example crates (`collector`, `minimal_native`, `walls` and one more) each had `examples/author.rs`. Cargo writes all of them to `target/debug/examples/author.exe`; on macOS the last link wins, on Windows parallel links hit the locked file (cargo calls this a future hard error, rust-lang/cargo#6313). CI's Windows job would have failed the same way.

Fix: renamed to `<crate>_author` (`collector_author`, `minimal_native_author`, `walls_author`, …) and updated every reference (AGENTS.md, READMEs, source comments, test failure messages). New commands, e.g. `cargo run -p walls --example walls_author`.

## 4. CRLF checkouts break byte-for-byte tests (Fixed)

Symptom: about a dozen tests fail with "out of date" or "differs from the generator": `scenes_match_authoring` (collector, walls), `the_scene_file_is_current` (minimal_native), `committed_fixtures_match_the_generator` (box_assets), `*_definitions_are_current` (collector_luau, cinder_keep `.d.luau`), `the_committed_character_is_the_generator_output`.

Cause: Git for Windows defaults to `core.autocrlf=true` (system gitconfig), and the repo had no `.gitattributes`, so every text file was checked out with CRLF. The generators write LF.

Fix: `.gitattributes` with `* text=auto eol=lf`, so every platform checks out LF. Renormalising changed only three OFL licence `.txt` files that had been committed with CRLF. Existing clones need a one-time re-checkout: `git rm -r --cached . -q && git reset --hard` (with a clean tree).

## 5. DX12: `instance_index` read in helper functions lacks the first instance (Fixed)

Symptoms (DX12 only; Vulkan on the same GPU renders correctly):
- Showcase: fence posts, shrubs, spheres, the capsule and two houses cast no shadow; a dark vertical band on the pillar (another object's caster drawn there).
- Cinder Keep: the player and the NPC (skinned) are invisible, and a large stray shadow covers the left field.
- Tests: `look_tests::alpha_masked_casters_cast_their_cut_out_and_batch`, `look_tests::wind_moves_the_surface_its_shadow_and_its_outline_alike`, `rs4_tests::the_shadowed_local_light_is_blocked_by_an_occluder` (401 shadowed vs 401 lit), `skinned_characters_render_posed_in_every_pass_and_batch`, the shader-material golden.

Cause: objects are drawn instanced; a vertex shader finds its object through `draw_slots[instance_index]`, and `instance_index` must include the draw's `first_instance`. D3D12's `SV_InstanceID` does not include it, so naga's HLSL backend adds a root constant: `(_NagaConstants.first_instance + SV_InstanceID)`. It does that only where the **entry point itself** reads its input (`FunctionCtx::is_fixed_function_input` in naga 30 returns `None` for any non-entry function). The engine passed the whole input struct to helpers that read `in.instance` there:
- `shadow_depth.wgsl` `static_clip` / `wind_clip`: every static shadow caster
- `skin.wgsl` `skin_vertex`: every skinned pass (main, prepass, outline, shadow, error, shader materials)
- `shadow_skinned.wgsl` `skinned_shadow`
- `custom_material.wgsl` `material_varyings`: every shader-material pass

Those helpers got the raw `SV_InstanceID` and loaded another object's transform (or slot 0's). `mesh.wgsl`, `error.wgsl` and `outline_mask.wgsl` already called `load_object(in.instance)` in the entry point, which is why plain standard-material meshes rendered correctly in the main pass. Metal and Vulkan include the base instance natively.

How it was found: DX12 vs `WGPU_BACKEND=vulkan` captures (Vulkan correct); identical draw and caster counts in `--report` (so GPU-side); FXC vs DXC (`WGPU_DX12_COMPILER`) gave the same result (so not the DXC compiler); drawing all casters at the origin filled the shadow map correctly (so writing and sampling work); splitting instanced draws into one draw per instance changed the wrong shadows (so the per-draw base instance is involved); naga's `is_fixed_function_input` source confirmed it.

Fix: the helpers take the instance as an explicit `instance: u32` argument and every `@vertex` function passes `in.instance` (`skin_vertex(in, in.instance)`, `static_clip(in, in.instance)`, `material_varyings(in, in.instance, view_proj)`). Behaviour on Metal and Vulkan is unchanged. The rule is documented at the top of `object.wgsl`: read `in.instance` only in the `@vertex` function itself.

Verified on DX12: `target/captures/showcase_fixed_dx12.png` (matches the Vulkan render and the Metal golden), `target/captures/cinder_keep_fixed_dx12.png` (both characters and their shadows), and the `box_render` tests above.

Worth reporting upstream to naga/wgpu: the HLSL backend's first-instance offset should follow a builtin into helper functions (or the struct should be adjusted at entry), since WGSL code that reads `instance_index` in a helper is valid and correct on every other backend.

## 6. Golden images (Expected)

References were recorded on Apple Silicon (Metal), and `golden::DEFAULT_TOLERANCE` is tight (0.05% of pixels). Before the item 5 fix: `showcase_day` 2.7%, `builtin_shapes` 2.7%, `procedural_steps` 1.9%, `textured` 1.5%, `interior` 10.8%, `editor_default` 0.8% of pixels differed.

After the item 5 fix most of that was gone: `showcase_day`, `builtin_shapes`, `procedural_steps`, `interior` and `golden_detects_a_missing_shrub` pass on DX12, and all 10 `m7_acceptance` tests pass (including `interior_is_lit_by_local_lights_without_sky_leak`). Remaining: `debug_draw` 4.9% (1-px grid-line rasterisation; identical by eye), `textured` 0.54%, `particles` 0.09%, `editor_default` 0.8% (not rerun after the fix). These are the expected cross-GPU differences until the M9 parity pass; compare the `*.actual.png` files by eye.

## 7. Remaining test failures, triaged

Rerun on DX12 after items 5, 8 and 9. Each failure from the two full runs, by cause:

### 7a. Generated files depended on the platform's math library (Fixed: D17)

- A generated-scene test of an example game: two generated material files differed in one colour channel by one ULP (`0.89000547` committed, `0.8900055` on Windows). `Color::srgb(0.95, ..)` called `f32::powf`, which is the platform C library's `powf` (UCRT on Windows, Apple libm on macOS). The exact result, `0.890005499151...`, lies 3e-11 from the midpoint between the two f32 values; the UCRT rounds it up, Apple's libm down.
- `the_committed_character_is_the_generator_output` (box_engine): regenerating `crates/box_engine/examples/animation_assets/models/character.glb` changed 110 of 81,216 bytes, all in aligned f32 values (animation data; `sin`/`cos`).
- `wall_golden_hashes` (box_arch): the `Wall` case `arc_pointed` hashed `0x0f969c814fb27996` instead of `0xaae53bc14db21086` (arcs use `sin`/`cos`).

Fix (decision D17): generated content is bit-identical on every platform.
- `box_core::pmath`: `sin`, `cos`, `sin_cos`, `tan`, `asin`, `acos`, `atan`, `atan2`, `powf`, `exp`, `ln` (and `pmath::f64`), backed by the pure-Rust `libm` crate. 108 call sites in `box_core` (colour, mesh primitives, image, tween), `box_proc` and `box_arch` use it.
- glam is built with its `libm` feature, so `Quat::from_rotation_*`, `from_axis_angle`, `slerp` and the other glam trigonometry are portable too.
- `clippy.toml` in `box_core`, `box_proc` and `box_arch` disallows the platform `f32`/`f64` methods, so new code cannot reintroduce them.
- `Wall::VERSION` 1 -> 2 and its golden hashes re-recorded; the other generators' hashes did not change. Re-recorded outputs (one-ULP colour and vertex/animation differences): `character.glb`, the four Cinder Keep `.glb` models, and an example game's generated material files and scene. A `pmath` unit test pins results whose platform values differ.
- Not covered: Luau scripts (`math.sin` in script rules) and game/example crates still use the C library.

### 7b. Knock-on effects of the DX12 bug (Fixed by item 5)

These pass on DX12 after item 5: `dropped_models_render_pick_and_play_with_assets` (edit and Play worlds draw objects in different slot orders, which the bug scrambled differently), `cinder_keep_is_completable_in_editor_play_mode`, `the_luau_collector_plays_in_the_editor_and_reloads_scripts_during_play`, `editor_default_layout` (editor golden), `rule_run_cost`.

### 7g. Two tests that raced background loading (Fixed: tests)

Both passed alone but failed in full workspace runs (machine under load):
- `script_properties_and_fields_show_small_values` (box_editor `m7_usability`): selecting the camera makes the inspector request `scripts/camera.luau`, and the test looked for its property after 4 frames, before the worker-thread import landed (the inspector showed `scripts/camera.luau (loading)`). It failed 2 of 6 runs even alone. The test now runs frames until the widget appears (`Driver::wait_for`, 10 s cap): 10 of 10 runs pass.
- `the_arena_plays_its_wind_loop_and_a_pickup_sound` (collector): when the wind clip finished loading during `settle_assets`, no frame ran `update_audio` before the voice count was checked. The test runs one frame after settling.

### 7c. `mcp_live_tools_end_to_end`: stale control file reported as `timeout` (Fixed)

With a stale `.box/control.json` (the host exited without removing it), `box mcp` live tools said `timeout` instead of `not_running` with the "stale control file" hint. Windows retries a refused loopback connection for about two seconds, so `TcpStream::connect_timeout` ends in `TimedOut` rather than `ConnectionRefused`. `ControlClient::connect_project` now treats a connect timeout like a refusal (`ClientError::Stale`): a live host accepts loopback connections from the kernel backlog at once. Timeouts on responses are unchanged. `box_cli --test mcp` and `box_engine --test control` pass.

### 7d. `recent_projects_dedupe_cap_persist_and_clear` (Fixed: test)

`start::config_dir_from` reads `APPDATA` on Windows; the test's fake environment only had `HOME`. The test now provides `APPDATA` too.

### 7e. `catch_errors_catches_backend_shader_failures` takes 30–43 minutes on DX12 (Fixed: test)

The test compiles a vertex shader with a 64 MiB local array (`array<f32, 16777216>`) to check that Metal's rejection is returned as an error. Only the Metal assertion was meaningful, and DX12's compiler accepts the array but needs 30–43 minutes (the `box_render` lib binary took 1,852 s and 2,574 s in the two full runs). The test now runs on Metal only; the other 77 `box_render` lib tests take 2.3 s on DX12.

### 7f. `hot_reload_keeps_previous_pipelines_on_error` (Not reproduced)

Failed once in the first full run, while another test process was running; passes alone and in later runs. Probably file-watcher timing under load. Watch for it in CI.

## 8. Verbatim paths in generated native projects (Fixed)

Symptom: `native::tests::generated_project_builds_with_only_engine_dependencies` and `lean_native_project_builds_and_runs` fail: `failed to parse manifest ... invalid path url '//?/C:\src\box-engine\crates\box_editor'`. Users running `box new --native` on Windows would get a project that does not build.

Cause: `native::engine_path` returns `std::fs::canonicalize(..)`, which on Windows is a verbatim path (`\\?\C:\...`); the generator turns `\` into `/` and writes `//?/C:/...` into `Cargo.toml`.

Fix: `engine_path` strips the `\\?\` prefix for drive paths (`strip_verbatim` in `apps/cli/src/native.rs`; UNC paths are left alone). The `run_native` tests compare against the same stripped form, and `help_shows_the_command_schema_and_mcp_setup` now accepts the shell-quoted exe path the help prints on Windows (`'C:\...\box.exe'`). `box_cli` native, `feedback` and `run_native` tests pass.

## 9. Clippy `result_large_err` on Windows (Fixed)

Symptom: `cargo clippy --workspace --all-targets -- -D warnings` fails on Windows only (8 errors in `crates/box_engine/src/project.rs`); the first CI run's `windows-2022` job failed on this.

Cause: `PathBuf` is larger on Windows, which pushes `ProjectError` (its `Scene` variant holds a `PathBuf` and an inline `SceneError`) over clippy's 128-byte limit.

Fix: `ProjectError::Scene { source: Box<SceneError> }`. Clippy is clean on Windows.

## 10. macOS CI: drag test regeneration counts (Fixed, not Windows)

The `macos-26` job failed `dragging_a_lathe_parameter_updates_every_frame_budget` (`box_editor` `shapes`, 19 regenerations) and later `dragging_a_wall_height_keeps_frames_in_budget` (`box_arch` `editor_drag`, 17), both asserting 20..=40 regenerations in 60 frames. The regeneration throttle (30 ms) ran on wall-clock time, and on a shared runner `thread::sleep` overshoot bunched the paced frames, so fewer of them cleared it; frame times were within budget. Both tests now drive the throttle with `ProcClock::Manual`, one 60 Hz step per frame: 29 regenerations on every run. The frame-time measurements stay real.

The failure also showed that `cargo test` stops at the first failing test binary, so the macOS job never ran the tests after `box_arch`'s `editor_drag`. The workflow's test steps now pass `--no-fail-fast`.

## Smoke run

| Step | Result |
|---|---|
| `cargo build --workspace` | Pass (after item 2) |
| `cargo run --example showcase -- --capture target/captures/showcase.png` | Runs, no validation errors; shadows wrong before item 5, correct after |
| `cargo run -p box_editor_app -- --capture target/captures/editor.png` | Runs, 2560x1440 @ 2x, no errors (R26 editor surface path: the headless capture does not exercise the window surface) |
| `cargo run -p box_player_app -- examples/cinder_keep` (headless capture) | Runs; characters invisible before item 5, correct after |
| `WGPU_BACKEND=vulkan` | Renders correctly; used to split backend bugs from engine bugs |

### Manual run (IMPLEMENTATION_PLAN step 16), driven by an agent through desktop control

Showcase in a window (`cargo run --example showcase`, debug build, DX12, 240 Hz display):

| Check | Result |
|---|---|
| Window opens, renders lit, shadowed primitives under sky and fog | Pass; ~4.2 ms (240 fps, Fifo at the display rate) |
| Orbit (left drag), zoom (wheel) | Pass |
| Keys 1/2 (day/evening), C (cascades), O (AO view), G (grid), T (tone mapper) | Pass |
| Maximise to the full ultrawide screen and restore | Pass; aspect correct |
| Minimise and restore | Pass; no crash, rendering resumes |
| Shader hot reload in a window (`tonemap.wgsl` edit) | Pass; change visible within 2 s |
| Broken shader | Pass; `shader reload failed; keeping previous pipelines` with file:line, image unchanged, recovers when fixed |
| Esc quits | Pass |
| Validation errors / panics in the log | None |

Colours on screen could not be judged from desktop screenshots: the desktop capture washes out all SDR content on this machine (probably an HDR desktop). The headless PNG opened in Paint looked exactly as washed out as the live DX12 window and a live `WGPU_BACKEND=vulkan` window. So the window's colours match the reference *relative to each other*, which is evidence for R26 but not a by-eye check.

Editor in a window (`box-editor examples/collector_luau --no-restore`):

| Check | Result |
|---|---|
| Window opens; surface `Bgra8Unorm` with sRGB view (R26 path) | Pass; egui panels and viewport render, 240 fps |
| Hierarchy selection, viewport marquee, inspector, gizmo | Pass |
| Play: skinned character, HUD, scripts | Pass |
| Game input in Play (click viewport, WASD, Space) | Pass; walked, jumped onto the platform, collected a gem (Score 10) |
| Esc returns input; Stop restores the edit scene | Pass; collected gem back |
| Maximise | Pass |
| Close; warnings/errors/validation in the log | Clean exit; none |

`input_demo` in a window: W moves and Space jumps the cube; click locks the cursor (pointer pinned while mouse moves turn the camera); Esc releases it (pointer moves freely); a second Esc quits. Log clean.

`audio_demo` on the real device: the device opens in 35 ms. Output was measured with the Windows audio session peak meter for the process (not by ear): wind loop 0.01–0.03 peak, varying as the emitter circles; each Space adds a chime spike; M (mute) and P (pause) drop it to 0.000; resuming restores it. Log clean.

Still to do by hand: look at the showcase and the editor on the monitor itself (R26 by eye; desktop screenshots cannot judge colour here), listen to the audio, and the gamepad (no controller was connected).
