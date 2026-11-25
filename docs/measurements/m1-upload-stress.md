# M1 upload stress measurement (spike S3)

Date: 2026-11-15. Machine: Mac Studio, Apple M3 Ultra, macOS 26.6, Metal. Release build, wgpu validation on (default instance flags).

## Method

`upload_stress` rewrites every vertex (position and normal) of N grid meshes every frame. Each mesh starts at 33×33 vertices (1,089 vertices, 52 KB at 48 bytes per vertex). Every 120 frames one mesh in eight grows by 8 cells per side, which forces a vertex and index buffer reallocation. Meshes do not cast shadows.

Runs were headless at 1280×720 for 600 frames. Headless frames wait for their GPU work to finish before the next frame starts, so `render_cpu_ms` is CPU submission plus GPU execution, serialized. A windowed run overlaps CPU and GPU and would be somewhat faster. The first 10 % of frames are excluded from the summaries as warm-up.

```bash
cargo run --release --example upload_stress -- --meshes 200 --frames 600 \
  --capture target/captures/upload_200.png --report target/upload_200.json
```

## Results

| Meshes | Upload per frame | Update (CPU mesh edit) p50 | Render p50 | Render p95 | Render max | Reallocations (600 frames) |
|---|---|---|---|---|---|---|
| 100 | 8.8 MB | 0.85 ms | 11.9 ms | 15.7 ms | 19.7 ms | 127 |
| 200 | 17.5 MB | 1.59 ms | 19.8 ms | 25.4 ms | 28.1 ms | 251 |
| 400 | 35.1 MB | 3.37 ms | 34.0 ms | 38.2 ms | 41.9 ms | 501 |

Baseline without uploads (the `showcase` scene, 29,550 triangles, 162 draws including all passes): render p50 6.7 ms, p95 11.2 ms, max 14.5 ms.

## Findings

- Render cost grows linearly with bytes uploaded: roughly **0.6 ms per MB per frame** on top of the scene cost. That covers interleaving vertices on the CPU plus `Queue::write_buffer`.
- **Reallocation frames are not visible as spikes.** The slowest frames are spread through the run and are at most about 1.5× the median, while reallocations happen every 120 frames.
- The only large outlier is frame 0 (pipeline and resource creation, 30–75 ms).

## Decision

- Keep **one vertex and one index buffer per mesh** (no pooled allocator). The measurement shows no reallocation spikes for a pool to fix.
- Real procedural editing changes a few meshes per frame, not hundreds. At 0.6 ms per MB, a **per-frame upload budget of about 4 MB** keeps uploads near 2.5 ms. That budget arrives with background generation (M5 or M6), as planned.
- Re-measure when the vertex format or the generation pipeline changes, and once in a window with presentation.

## Not measured

- Windowed runs: the display was not available during this session (macOS reported the window as occluded). Frame pacing with a real swapchain is still unverified.
- GPU time on its own (needs timestamp queries; deferred).
- Windows/DX12.

## 2026-11-17: 104-byte vertices (custom data sets)

The vertex grew from 72 to 104 bytes with the two custom vec4 sets (`MeshData::custom`), so every upload is 44% larger in its vertex part. One run after the change, Apple M3 Ultra (Metal), release, default mesh count, 600 headless frames: 31.4 MB uploaded per frame, update p50 1.5 ms, render (CPU and submit) p50 12.7 ms, p95 13.7 ms, 251 reallocations. No run of the 72-byte format on this machine was recorded, so there is no before number; the tables above are from other hardware and a 48-byte vertex and are not comparable.
