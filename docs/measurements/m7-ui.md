# M7 in-game UI measurements

Apple M3 Ultra, Metal, release, 2026-11-16, shared machine. Source: `cargo test --release -p box_ui --test cost -- --nocapture`. Scene: HUD (health bar, counter with a colour tag, 8 world labels) plus an open modal menu (title, slider, 2 toggles, 6 buttons) at 1920×1080, reference scale 1, Inter. CPU covers the UI systems only, mean of 200 frames.

| Per frame | CPU (ms) |
|---|---|
| Nothing changed | 0.002 |
| HUD value changed every frame (relayout, paint, tessellation, overlay rewrite) | 0.074–0.078 |
| Camera moving (8 labels re-projected) | 0.048 |

GPU: the overlay pass's own timestamps read 0 at p50/p95 and about 0.05 ms in the first frames (atlas upload). Whole-frame GPU p50 is 1.12–1.14 ms with the UI versus 1.01–1.02 ms without, about 0.1 ms. Layout and paint run only on change, and the glyph atlas uploads as incremental patches. Not yet measured: hundreds of nodes, text-heavy pages, Windows/DX12.
