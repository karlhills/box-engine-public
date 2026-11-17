# M7 animation measurements

2026-11-16, Apple M3 Ultra, Metal, release, 1920x1080, 300 frames.

## CPU (`cargo test --release -p box_engine --test animation -- --ignored --nocapture`)

Characters running the locomotion state machine at staggered phases (14 joints each).

| characters | state machines | sampling | propagation |
|---|---|---|---|
| 100 | 0.004 ms | 0.057 ms | 0.198 ms |
| 500 | 0.026 ms | 0.304 ms | 1.088 ms |

## Rendering (`animation --count N --spread 0.37 --size 1920x1080 --headless --frames 300 --report`, `--bind-pose` draws unskinned)

| characters | mode | update | CPU to submit | GPU frame | main | shadows | draws (shadow) | triangles | upload/frame |
|---|---|---|---|---|---|---|---|---|---|
| 100 | skinned | 0.298 | 0.715 | 1.311 | 0.259 | 0.123 | 30 (10) | 161 k | 92 KB |
| 100 | bind pose | 0.281 | 0.620 | 1.286 | 0.254 | 0.118 | 30 (10) | 160 k | 2.4 KB |
| 500 | skinned | 1.443 | 1.047 | 1.348 | 0.325 | 0.161 | 30 (10) | 656 k | 453 KB |
| 500 | bind pose | 1.357 | 0.781 | 1.371 | 0.354 | 0.176 | 30 (10) | 654 k | 2.7 KB |

Times in ms. GPU skinning is within noise at these counts. The CPU cost of skinning is the palettes and their upload (64 B per joint): +0.1 ms at 100, +0.27 ms at 500 characters. Instancing holds: skinned characters in different poses are one draw per material group and pass.
