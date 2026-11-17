# M3 physics measurements

Apple M3 Ultra, release build, 2026-11-15. Source: `cargo run --release -p box_physics --example overlap_bench` (ROADMAP M3 test strategy: 5,000 moving entities with overlap queries). The machine was shared with other builds (load average about 14), so p50 is the representative figure.

5,000 moving colliders (mixed shapes, rotating) plus 50 static triggers; each mover runs one sphere overlap per frame; 100 raycasts per frame; 600 frames.

| Per frame (ms) | mean | p50 | p95 | max |
|---|---|---|---|---|
| Frame tick: move 5,000 transforms, propagation, physics sync | 0.97 | 0.92 | 1.28 | 1.62 |
| of which physics sync (5,000 pose updates, BVH refit, triggers) | 0.53 | 0.50 | 0.76 | 1.17 |
| 5,000 overlap queries + 100 raycasts | 1.64 | 1.53 | 2.15 | 3.03 |

Steady state builds no shapes (`0 shapes built`): moves only update poses and bounds, and the BVH leaf changes only when a collider leaves its 5 cm margin. An unchanged frame costs one compare pass and no tree or trigger work. The physics agent's first run, under load average 64, measured p50 0.96 / 0.52 / 1.62 ms for the same rows.
