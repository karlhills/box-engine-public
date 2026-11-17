# S1 findings: egui editor shell

**Question.** Can egui 0.37 (through `egui-winit` and `egui-wgpu`, without eframe) plus `egui_tiles` give us a restrained, professional editor shell that stays fast on a HiDPI Mac?

**Answer.** Yes, if we keep the approach used here: our own theme tokens, a small set of our own widgets, and egui's stock widgets only where their look is neutral. Performance is not a concern. egui's CPU cost is under 1 ms per frame, and its GPU pass takes about 0.3 ms at 2560x1440. **Recommendation: proceed with egui for M2.** Do not open a gpui evaluation unless the owner rejects the look in the captures.

## How to run

```bash
cd spikes/s1_editor_shell
cargo run --release                     # windowed; prints frame-interval stats on quit
cargo run --release -- --seconds 30     # windowed, quits after 30 s
cargo run --release -- --capture        # captures/*.png and the layout JSON round trip
cargo run --release -- --bench          # captures/bench.json (300 frames per scenario)
```

The spike pins its own toolchain (`rust-toolchain.toml`, 1.98.1). egui 0.37 needs Rust >= 1.95, and the repository pins 1.94.0. See "What fought us" below.

## Captures

All captures are 2560x1440 at `pixels_per_point = 2.0`, rendered headless on Metal (Apple M3 Ultra).

| File | Shows |
|---|---|
| `captures/shell_dark.png` | Default layout (EDITOR.md section 3). A prefab is selected, its path is expanded, and the viewport outlines it. Console tab in front. |
| `captures/shell_hierarchy_deep.png` | Search `window 03` over the fully expanded 10,000-entity tree: 583 matches at depth 5. Ancestors appear dimmed for context, the matched text is semibold, and indent guides are drawn. |
| `captures/shell_layout_alt.png` | Alternative docked layout, restored from `captures/layout_alt.json`: hierarchy over assets, viewport over console, inspector on the right. |
| `captures/shell_assets_tab.png` | Default layout with the Assets tab in front and a light selected. The inspector changes its sections for each entity kind. |

Before each capture, the layout is serialised to JSON, written to disk, read back, and the restored tree is the one rendered.

## What was built

- **Theme** (`src/theme.rs`): one `Tokens` struct holds everything visual:
  - 9 neutral surface steps and 4 text steps.
  - One accent (`#4B8BF5`) with a derived selection tint. The accent appears only on the selected row and tile, the active tool, checked boxes, focus rings, the dragging resize line and the viewport selection outline.
  - Functional colours only: X/Y/Z axes, warning and error.
  - Fixed metrics: 22 pt row height, a label column of 40 % of the inspector width (96-140 pt), 3 pt radius.
  - Shadows set to `NONE`, no gradients.
  - Fonts: Inter (variable, `wght` 430 for body, 590 for semibold) at 12 pt and 11 pt, plus Source Code Pro 11 pt for numbers and the console.
- **Own widgets** (`src/widgets.rs`): property row (fixed label column with ellipsis), Vec3 drag fields with a 2 pt axis tick, a Blender-style fill slider, checkbox, collapsible section header with a hairline, segmented control, toolbar buttons, and painted monochrome glyphs (entity kinds, play/pause/stop, snap, search). There are no icon fonts and no emoji.
- **Stock egui widgets, themed:** `DragValue` (monospace through `Style::drag_value_text_style`), `ComboBox`, `TextEdit`, the colour picker and `MenuBar`.
- **Docking:** `egui_tiles` 0.18, all panes in tab containers. A custom `tab_ui` paints the tabs, the gaps are 1 px dividers, and resize handles turn accent while being dragged.
  - Layout is saved to `.box/layout.json` (`{version, tree}`) on quit and restored on start, falling back to the default if the file is missing, has the wrong version, or lacks a pane. Both directions were verified in windowed runs.
  - Project-menu actions: Save, Reset and Alternative layout.
- **Viewport:** our own wgpu pass, a ray-marched stand-in scene with a grid, soft shadows and a selection outline. It renders every frame into an `Rgba8Unorm` texture sized to panel rect x `pixels_per_point`. The texture is registered once with `register_native_texture`, and on resize it is re-pointed with `update_egui_texture_from_wgpu_texture`. The windowed runner debounces resizes by 6 frames; headless runs resize immediately. Dragging in the viewport orbits the camera.
- **Hierarchy:** 10,000 entities up to 6 levels deep, with expand/collapse (chevron or double-click), selection and search.
  - The visible rows are flattened into a `Vec<Row>` only when expansion or the filter changes, then shown with `ScrollArea::show_rows`.
  - Flattening all 10,000 rows takes 0.14-0.2 ms, so even re-flattening every frame would be acceptable.
- **Inspector:** sections depend on the entity kind: Transform, Mesh Renderer, Material Overrides, Window Array (procedural), Collider, Directional Light, and an Add Component menu that is wired up.
- **Console:** 400 lines, virtualised, with level toggle chips showing counts, a text filter, Clear, and monospace columns.
- **Assets:** a grid of tiles with ellipsised names.

## Timings

Release build, Apple M3 Ultra, Metal, headless at 2560x1440 px and 2x, 300 frames per scenario, after a 30-frame warm-up. Every frame waits for the GPU. "ui" is `Context::run_ui`, "tess" is `tessellate`, "prep" is texture and buffer upload plus encoding, and "gpu egui"/"gpu vp" are GPU timestamp queries around each pass. Full data is in `captures/bench.json`.

| Scenario | Rows | ui ms (mean/p95/max) | tess | prep | gpu egui | gpu vp | total (sync) |
|---|---|---|---|---|---|---|---|
| default, hover moving | 39 | 0.29 / 0.43 / 0.94 | 0.11 / 0.17 / 0.49 | 0.19 / 0.33 / 0.68 | 1.26 / 6.06 / 8.12 | 5.88 / 8.54 / 9.26 | 8.85 / 13.39 / 15.31 |
| 10k expanded, jump scroll every frame | 10,000 | 0.39 / 0.62 / 1.05 | 0.11 / 0.17 / 0.41 | 0.21 / 0.31 / 0.60 | 1.74 / 6.91 / 9.41 | 6.07 / 8.55 / 11.23 | 9.18 / 13.62 / 16.11 |
| 10k expanded, smooth scroll | 10,000 | 0.31 / 0.47 / 0.72 | 0.11 / 0.18 / 0.52 | 0.21 / 0.37 / 0.60 | 1.80 / 7.16 / 11.35 | 6.13 / 8.68 / 12.39 | 9.21 / 13.79 / 15.96 |
| search "a", jump scroll | 4,012 | 0.50 / 0.75 / 1.29 | 0.11 / 0.17 / 0.35 | 0.20 / 0.33 / 0.53 | 2.01 / 6.90 / 11.31 | 5.93 / 7.95 / 8.64 | 9.21 / 13.31 / 16.71 |
| 10k, re-flatten every frame | 10,000 | 0.48 / 0.68 / 0.90 | 0.11 / 0.16 / 0.36 | 0.20 / 0.32 / 0.74 | 2.02 / 7.34 / 11.62 | 6.09 / 8.43 / 11.18 | 9.45 / 13.64 / 15.45 |
| default, **viewport pass skipped** | 35 | 0.27 / 0.40 / 1.19 | 0.10 / 0.15 / 0.27 | 0.16 / 0.26 / 0.48 | **0.32 / 1.98 / 3.83** | - | 3.70 / 7.24 / 9.61 |
| 10k jump scroll, **viewport skipped** | 10,000 | 0.39 / 0.68 / 1.43 | 0.11 / 0.20 / 0.35 | 0.19 / 0.33 / 0.52 | **0.31 / 1.83 / 3.40** | - | 3.95 / 7.80 / 9.88 |

How to read the table:

- **egui CPU cost per frame is about 0.6-0.8 ms mean and under 1.2 ms at p95,** even with 10,000 rows and a new scroll position every frame. Virtualisation works: cost does not depend on tree size.
- **egui GPU cost is about 0.3 ms mean** for the whole 2560x1440 UI.
- When the viewport pass runs first, the egui pass's timestamps read 1.3-2 ms. I did not isolate why. Overlap between consecutive passes on Apple's tile-based GPU is a plausible cause, not a proven one. GPU-side per-pass timings on Metal need care in M2.
- **The "total (sync)" column is not a frame-rate prediction.** Each frame submits and then blocks on the GPU. With the viewport skipped, about 3 ms of the remaining time is submit-to-completion latency on an idle GPU, which a pipelined window loop hides. The stand-in ray-marcher takes about 6 ms of GPU time and will be replaced by the real renderer.
- p95 and max values in the GPU columns look like clock ramping between serialised frames: with the viewport skipped, egui's GPU mean is 0.3 ms but p95 is 2 ms.

**Not measured: "no dropped frames at display refresh on a HiDPI screen".** The screen was locked during this session. The windowed binary builds, creates the window and Metal surface, and saves and restores layout on quit, but every surface acquire returned `CurrentSurfaceTexture::Occluded`, so no frame was presented. The owner should run `cargo run --release -- --seconds 30` on the ProMotion or 5K display. On quit it prints mean, p95 and p99 frame intervals and the count of frames over 1.5x of 8.33 ms and of 16.67 ms. The numbers above suggest a 120 Hz frame is well within budget for the UI itself.

## What worked

- **Direct `egui-winit` + `egui-wgpu` is easy.** The shared paint path (`src/gfx.rs`) is about 100 lines plus timestamps, and the same path serves the window and the headless capture. Versions line up exactly: egui-wgpu 0.37 -> wgpu 30.0.2, winit 0.30.13, and `cargo tree -d` shows no duplicate wgpu or winit.
- **Headless UI capture is simple:**
  1. Build a `RawInput` with `screen_rect` and `viewports[ROOT].native_pixels_per_point = 2.0`.
  2. Call `run_ui`, then `tessellate`, then `Renderer::render` into an offscreen `Rgba8Unorm` texture.
  3. Read the texture back.

  The layout settles in 2-4 frames. This is the basis for EDITOR.md section 10's UI golden tests.
- **Native viewport texture:** register once, then `update_egui_texture_from_wgpu_texture` on resize, and the same `TextureId` keeps working.
- **Text quality is good.** egui 0.37 rasterises with skrifa and shapes with harfrust. Inter's variable `wght` axis works through `FontTweak::coords`, so one font file gives both weights. Text is crisp at 2x with no blurring and no baseline jitter in the captures.
- **egui_tiles is enough for docking.** Overriding `tab_ui`, `tab_bar_color`, `resize_stroke`, `gap_width`, the drag preview and `top_bar_right_ui` gave a tab look that matches the theme. `Tree<Pane>` serialises with serde, and setting `all_panes_must_have_tabs` makes every pane draggable.
- **Immediate-mode virtualisation is cheap.** `show_rows` plus a cached flatten gives a flat cost per frame. The inspector is a plain function of the state, which is promising for reflection-driven panels.

## What fought us

1. **egui 0.37 requires Rust 1.95 or newer** (egui_tiles 0.18 too). The repository pins `1.94.0`, so the spike carries its own `rust-toolchain.toml` (1.98.1, already installed). **M2 must bump the workspace toolchain to >= 1.95.**
2. **egui 0.37 ships no fonts unless you opt in.** Default features are empty, so `default_fonts` is off. We supply our own, which is what we want anyway. Fonts added here, both under the SIL Open Font License 1.1:
   - `assets/fonts/Inter-Variable.ttf`, copied from another local project, with `OFL-Inter.txt` alongside.
   - `assets/fonts/SourceCodePro-Regular.ttf`, from macOS's bundled Ruby rdoc assets. Its licence text is bundled as `OFL-SourceCodePro.txt`, taken from the adobe-fonts/source-code-pro repository. For the engine, take the font itself from that repository rather than from the macOS copy.

   egui now shapes text with harfrust, so ligature fonts such as Fira Code would turn `->` into a ligature in the console. Choose monospace fonts without ligatures.
3. **API churn compared with older examples:**
   - Panels take `&mut Ui`: `Panel::top(..).show(ui, ..)` and `Context::run_ui`.
   - `Ui::id` is deprecated in favour of `scope_id`, and `Id::new` in favour of `Id::unique`. `Tree::new` needs an `Id`, not a `&str`.
   - `TexturesDelta::set` is now a map of `SmallVec<[ImageDelta; 1]>`.
   - `Visuals::clip_rect_margin` is deprecated.
   - wgpu 30 adds `RequestAdapterOptions::apply_limit_buckets` and replaces `frame.present()` with `queue.present(frame)`.

   Read the crate sources, not blog examples.
4. **Theming limits:**
   - `Visuals::widgets` has one fill per interaction state, shared by buttons, text fields, combo boxes and drag values. Anything that should look distinct, such as sliders, checkboxes, segmented tools and section headers, needs its own widget. That took about 400 lines here and is the right call.
   - `MenuBar` claims the full available width (`set_min_size(available_width)`), so the rest of the toolbar must live inside its closure.
   - `MenuBar` takes its row height from `interact_size.y` at construction. Set it before calling `MenuBar::ui`, or items end up 3 pt off the toolbar's centre line.
   - Popup and menu chrome comes from `window_fill`, `window_stroke` and `menu_corner_radius`, and was acceptable once shadows were off.
5. **egui_tiles behaviour:**
   - Panes do not paint their own background; `pane_ui` must fill its rect.
   - The serialised layout is keyed by internal `TileId`s plus our `Pane` enum values. EDITOR.md asks for "stable panel IDs": the `Pane` variants serve that role, and the loader must still validate that every pane is present (done here) because tile ids are arbitrary.
   - The tab bar has no built-in vertical tabs and no close buttons per pane unless we implement them.
6. **Colour spaces.** egui-wgpu treats a native `Rgba8Unorm` texture as gamma-encoded, so the viewport's final pass must write sRGB-encoded values into a UNORM texture. Alternatively, a copy or blit can convert the engine's HDR or `*Srgb` target. egui-wgpu also prefers a non-sRGB swapchain format; it switches to a "linear framebuffer" shader for sRGB targets, so either format works. The engine's renderer currently picks an sRGB surface, so M2 must choose one deliberately.
7. **The windowed path could not be watched in this session** because the screen was locked or the window occluded; see "Not measured" above.

## Self-critique of the look

What reads as a professional tool: aligned label columns, a consistent 22 pt row rhythm, monospace numbers, one accent used sparingly, hairline section dividers, a quiet tab bar, a viewport that is the brightest thing on screen, and no shadows or gradients.

Weaker spots to fix in M2:

- The toolbar is crowded at 1280 pt wide. At narrow widths it needs an overflow rule.
- Combo-box arrows and the colour button are stock egui and slightly heavier than our own widgets.
- The Add Component button text is left-aligned.
- Slider value text sits on the fill boundary, which is the Blender convention but not everyone's taste.
- The light theme is untested, though the token set is shaped for it.

## What M2 should reuse

- `src/theme.rs`: the token struct and `apply()`, including the font setup and variable-weight trick, almost verbatim. Make `Tokens` a resource so a light theme can follow.
- `src/widgets.rs`: `property_row`, `vec3`, `slider`, `checkbox`, `section`, `segmented`, `tool_button`, `text_button` and the glyphs. These become the `box_editor` widget kit that the reflection inspector targets.
- `src/gfx.rs`: the shared "viewport pass + egui paint" path and the timestamp helper. In M2 the viewport pass becomes `box_render` drawing into the registered texture.
- `src/headless.rs`: the headless UI capture harness, the basis for editor UI goldens and an `--capture` flag on the editor binary.
- From `src/app.rs`: the `Dock` behaviour (tab look and resize strokes), the layout file `{version, tree}` with validation and fallback, and the hierarchy flatten-and-virtualise pattern (`scene::flatten` + `show_rows`).

Leave behind: the fake scene and log data, the ray-marched viewport, and the all-in-one `app.rs` structure. Panels should become separate modules behind the extension points in EDITOR.md section 8.
