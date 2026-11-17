# M5 geometry building blocks

Apple M3 Ultra, release build, median of 300 runs (1,000 for the wall), CPU generation only, 2026-11-15. Source: `cargo run --release -p box_proc --example proc_bench`.

| Block | µs | Output |
|---|---:|---|
| lathe, 32-point profile × 64 segments | 97.8 | 2,080 verts, 3,968 tris |
| sweep, 20-point spline × 16-point circle (path + mesh) | 151.2 | 149 stations, 4,764 tris |
| wall sweep, 20-point polyline × rect | 15.2 | 156 tris |
| union / difference / intersection, 2 × 200 verts | 109 / 107 / 101 | |
| offset +0.2 round / −0.2 mitre, 200 verts | 255 / 235 | |
| triangulate, 1,000 verts, 4 holes | 149.5 | 1,006 tris |
| extrude, 248 verts, 3-step round bevel | 182.0 | 3,968 tris |

All blocks are well under one 60 Hz frame at these sizes, so the M5 synchronous flush is sufficient for them. Offsets are the slowest 2D operation (about 0.25 ms per 200 vertices).

Generator runtime (release): 1,000 idle generators cost 2.75 µs per frame in `flush`; one parameter edit is visible in 1 frame (0.03–0.06 ms); editing all 1,000 at once takes 2 frames (6 ms budget per flush), 7.6 ms in total; a 60 Hz drag regenerates 30 times per second (30 ms throttle) and the last value is visible 1 frame after release. Source: `cargo test --release -p box_engine --test procedural latency -- --nocapture`.

Derived entities (release, 1,000 intents / 2,000 parts): idle rule pass 3.8 µs; one intent edit with rule + reconcile of every owner 1.37 ms (rule alone 0.31 ms).

Spline drag (Edit Spline, 216-sample tube, 16-segment circle, 6,912 triangles, 60 Hz, 120 frames, release, M3 Ultra): editor frame mean 0.37 ms, p95 0.59 ms, max 0.84 ms; 61 regenerations; generate mean 0.24 ms, max 0.35 ms (excludes GPU render and upload). Source: `cargo test --release -p box_editor --test splines -- --nocapture`.

Built-in generators (release, 2026-11-15): a 60 Hz drag for 1 s on 1 of 200 objects regenerates 30 times, last value visible 1 frame after release; worst frame Lathe vase 0.13–0.22 ms (2.7k tris), Lathe 256 segments 0.84–0.99 ms (21k tris), Extrude hole+bevel 0.13–0.21 ms (1.2k tris). Source: `cargo test --release -p box_engine --test procedural lathe_and_extrude -- --nocapture`. Editor, real mouse drag on a Lathe's Height field, 2560×1600 @2x, paced at 60 Hz: 29 regenerations; full frame including GPU wait mean ≈4 ms, p95 ≈9 ms, worst ≤12.7 ms; no frame over 16.7 ms (`cargo test --release -p box_editor --test shapes -- --nocapture`).
