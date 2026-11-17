# M7 RS4: lights and particles

`cargo test --release -p box_engine --test rs4_cost -- --ignored --nocapture`, 2560x1440, Apple M3 Ultra (Metal), 2026-11-17. Minimum / 10th percentile over 120 frames after 10 warm-up; GPU = first-to-last pass timestamp span. The machine was shared, so treat it as ±0.1 ms.

| Configuration (interior scene) | CPU submit ms | GPU ms | Δ GPU |
|---|---|---|---|
| no local lights | 0.567 | 1.069 | — |
| 1 spot (unshadowed) | 0.563 | 1.082 | +0.014 |
| 16 spots | 0.564 | 1.364 | +0.296 |
| 64 spots | 0.562 | 2.159 | +1.091 |
| 256 spots | 0.579 | 5.708 | +4.639 |
| shadowed spot 1024² (1 view) vs unshadowed | +0.023 | 1.136 | −0.04 (noise) |
| shadowed spot 2048² | +0.026 | 1.247 | +0.119 |
| shadowed point 1024² (6 views) vs unshadowed | +0.164 | 1.707 | +0.549 |
| shadowed point 2048² (6 views) | +0.166 | 1.701 | +0.534 |
| ambient volume vs none | +0.07 | 1.183 | +0.040 |
| full interior (shadowed spot + 2 points + volume) | 0.667 | 1.183 | — |

| Particles (particles scene, 10,000 alive, 0.12 m) | sim ms | Δ CPU submit | Δ GPU |
|---|---|---|---|
| alpha, sorted, lit, soft | 0.051 | +0.271 | +0.091 |
| alpha, sorted | 0.050 | +0.162 | −0.03 (noise) |
| additive | 0.051 | +0.184 | −0.04 (noise) |

Spots each reach 4 m in an 8x6 m room, so every fragment loops over most lights: the per-fragment loop, not a light count, sets the cost (clustering is RS5). The submit cost of particles is instance building, the per-emitter back-to-front sort and a 640 KB upload. Large, overdrawn particles cost GPU fill rate not measured here.
