# M6 box_arch measurements

Apple M3 Ultra, release, 2026-11-15. `cargo run --release -p box_arch --example arch_bench` (median of 300 generations, CPU only) and `cargo test --release -p box_arch --test editor_drag -- --nocapture`.

| generator | vertices | triangles | median µs |
|---|---:|---:|---:|
| Wall: straight 6 m, door + window | 456 | 152 | 43.7 |
| Wall: house loop 8 × 6 m, 8 openings, plinth, cornice | 3,480 | 1,160 | 314.8 |
| Wall: same loop under a hip roof (top follows the roof) | 4,386 | 1,462 | 476.3 |
| Wall (large): 40 m S curve (arcs), 20 openings, plinth, cornice | 6,114 | 2,038 | 574.2 |
| Roof: gable 5 × 4 m | 462 | 154 | 26.4 |
| Roof: hip 8.3 × 6.3 m | 438 | 146 | 27.5 |
| Roof: cone r 1.35 m, 48 segments | 1,440 | 480 | 70.8 |
| Roof (large): dome r 6 m, 128 × 32 | 52,146 | 17,382 | 2,229.4 |
| Platform: 4 × 3 m | 36 | 12 | 5.7 |
| Platform: circle 64 segments with a lip | 2,292 | 764 | 274.6 |
| Platform (large): 200-point outline with a lip | 8,268 | 2,756 | 1,009.0 |
| Stair: 6 risers, solid | 156 | 52 | 10.2 |
| Stair (large): 30 risers, open, landing | 1,224 | 408 | 55.7 |

- Typical parts are well under 1 ms. Over 1 ms: the 128 × 32 dome (size-bound: its faces share no vertices) and the 200-point lip platform (two i_overlay offsets, about 70 µs each for 64 points, grow with the outline).
- Anchors, 1,000 decorations on 100 walls: first resolve 0.88 ms, idle 0.26 ms per frame, one wall edited 0.29 ms (10 decorations moved).
- Editor drag of a house wall's Height (closed loop 8 × 6 m, 8 openings, plinth, cornice, 1,172 triangles; 2560 × 1600 at 2×, 60 frames at 60 Hz, full editor frame incl. GPU wait): 29 regenerations, mean 7.43 ms, p95 10.58 ms, worst 12.00 ms, 0 frames over 16.7 ms; one undo step; 0 wgpu validation errors.
