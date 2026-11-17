# M4 import timings

Apple Silicon (M3 Ultra), release, 2026-11-15. Source: `cargo test --release -p box_assets --test timings -- --nocapture`, three runs (range).

| File | Time |
|---|---|
| `checker.png` | 0.18–0.38 ms |
| `photo.jpg` | 0.21–0.64 ms |
| `tower.glb` | 0.47–0.95 ms |
| `crate.gltf` (+ `.bin`, two PNGs) | 0.47–0.88 ms |
| `stone.material.json` | 0.19–0.40 ms |
| `level.scene.json` | 0.06–0.12 ms |
| Generated grid GLB, 100,352 triangles / 50,625 vertices | 105–114 ms |
| Same grid with `generate_tangents: false` | 2.6–2.9 ms |

About 97% of the large-model import is MikkTSpace tangent generation. Imports run on worker threads (cores − 1, clamped to 1–4), so this is load latency, not a frame hitch: 13 load calls returned in under 100 ms while twelve 50k-triangle imports ran.
