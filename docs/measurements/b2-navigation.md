# B2 navigation (N1, N2): bake, query and agent costs

Recorded 2026-11-24 on an Apple M3 Ultra (release build), with other work running on the machine.

`cargo test --release -p box_engine --test navigation cost -- --ignored --nocapture`

Scene: a 200 × 200 m floor with 500 boxes (0.5–3.5 m) at seeded random spots; default `NavSettings` (0.4 m agent, 0.15 m cells, 32 m tiles).

| Case | Cost | Target (docs/NAVIGATION.md) |
|---|---|---|
| Full bake, inline on one thread (64 tiles, 3,178 polygons; the frame including the collider sync) | 328 ms | under 400 ms of worker time: met. In Play and the editor it runs on a worker thread; the main thread only gathers triangles and swaps tiles in |
| One box moved: its 2 tiles rebaked (the frame, inline) | 15 ms (about 7.5 ms a tile) | under 25 ms a tile: met |
| Idle frame of the whole app, navmesh built, nothing changing | 0.032 ms | an empty-list check for navigation |
| Path query across the level (200 random pairs, all complete) | median 0.039 ms, p95 0.091 ms, max 0.179 ms | p95 under 0.1 ms: met |
| 50 agents walking across the level, goals changed every second (`nav_agents`, one fixed step) | median 0.012 ms, p95 0.087 ms, max 0.42 ms | under 0.3 ms a frame: met except the step where all 50 goals change at once (replans are capped at 8 a step, so that burst lasts 7 steps) |

N1 first measured the path query at p95 0.218 ms. The time went to hashing polygon refs with SipHash in A*'s node map and closed set; an FxHash-style hasher for those maps (`box_nav::mesh::FxHasher`) brought it to 0.09 ms.
