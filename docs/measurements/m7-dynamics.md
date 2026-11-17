# M7 dynamics measurements

Release build, Apple M3 Ultra, 2026-11-16, machine shared (load average 15–17). `cargo run --release -p box_physics --features dynamics --example dynamics_bench`

| Scene (600 frames) | step p50 / p95 (first 2 s) | step p50 / p95 (after) | tick p50 (after) |
|---|---|---|---|
| 1,000 dynamic bodies | 1.30 / 1.80 ms | 1.10 / 1.52 ms | 1.29 ms |
| 5,000 dynamic bodies | 6.53 / 10.57 ms | 6.77 / 9.38 ms | 7.69 ms |
| 200 CCD projectiles | 0.24 / 0.35 ms | 0.24 / 0.25 ms | 0.28 ms |

- Most bodies in the body bench were still awake at the end (949/1000, 4922/5000); piles of balls and capsules keep rolling, so these are near-worst-case active numbers.
- Projectiles: 3,807 relaunches, 0 tunnelled.
- Tick minus step (write-back plus query-world sync) is about 0.2 ms at 1,000 bodies and about 0.9 ms at 5,000.
- Scenes without a `RigidBody` pay one archetype scan per fixed step.
- A clean dev build of rapier3d takes 24.8 s wall time (69.5 s of CPU).
