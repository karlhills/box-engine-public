# M6 scatter measurements

Apple M3 Ultra, release, 2026-11-15. `cargo test --release -p box_engine --test scatter scatter_timings -- --nocapture`, `cargo test --release -p box_editor --test scatter_brush stroke_frame_cost -- --nocapture`. Flat ground, grass and bushes with tint, density 2/m², min distance 0.3 m.

| Instances (chunks) | Spawn (settle) | Idle frame | Stroke frame mean / max (60 dabs, 60 Hz) | Flush mean / max | Reconcile per instance: spawn / unchanged pass |
|---|---|---|---|---|---|
| 10,097 (100) | 11.3 ms | 0.043 ms | 0.14 / 0.43 ms | 0.12 / 0.28 ms | 8.6 / 4.2 ms |
| 50,079 (400) | 47.3 ms | 0.24 ms | 0.50 / 1.24 ms | 0.32 / 0.58 ms | 44.2 / 29.9 ms |

- The stroke placed 43 chunks over 30 flushes.
- Editor stroke over 1,522 instances (120 frames): frame mean 0.47 ms, p95 0.71, max 0.95; 60 flushes, 0.19 ms mean, 1.6 chunks each.
- Choice: chunk parts plus plain instance entities; one reconcile part per instance is too slow (a full pass per flush).
