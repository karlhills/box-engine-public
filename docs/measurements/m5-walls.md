# M5 walls measurements

`cargo test --release -p walls --test measure -- --nocapture`, Apple M3 Ultra, 2026-11-15 (machine shared, load average about 7), after the walls, splines and generators merges. Room grid of mitred square rooms; one edit = `set_field(WallPath.points)` nudging a room's first wall 3 cm; default 30 ms throttle, manual 60 Hz clock.

| walls | rule ms | reconcile ms | edit frame ms | parts regenerated | idle frame ms | drag latency |
|---:|---:|---:|---:|---:|---:|---|
| 10 | 0.014 | 0.009 | 0.043 | 4 | 0.002 | ≤ 1 frame (worst frame 0.05 ms) |
| 100 | 0.103 | 0.067 | 0.196 | 4 | 0.009 | ≤ 1 frame (worst 0.33 ms) |
| 1,000 | 1.16 | 0.72 | 2.03 | 4 | 0.074 | ≤ 1 frame (worst 2.30 ms) |

- Without the spatial grid the rule took 0.005 / 0.088 / 5.16 ms (quadratic): the grid was added, component scoping was not needed.
- 4 parts per nudge: the wall, its two partners (their snapped ends follow the junction point it owns) and the opposite wall (its mitres with the partners). In the yard drag (40 steps) 1 to 2 parts regenerate per step, only the dragged wall and walls it touched or touches.
- The first edit after a pause is visible in the same frame; during a drag the throttle delays some edits by one frame.
- Idle, 100 walls, the full edit-world frame (propagate, rule gate, flush stamp, renderers): about 12 µs; no rule run and no generation.
