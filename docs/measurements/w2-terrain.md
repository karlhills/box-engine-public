# W2 terrain measurements

2026-11-22, Apple M3 Ultra, release build. `cargo test --release -p box_engine --test world_building terrain_cost -- --ignored --nocapture`: a 256 m terrain at 1 m cells (64 chunks of 32 × 32 cells) with eight stamps (noise, six 18 m hills, a walkable strip).

| What | Time |
|---|---|
| First build: the rule's 64 chunk parts and every chunk mesh | 22 ms (0.34 ms per chunk) |
| First frame after it, 64 heightfield colliders | 0.8–1.2 ms |
| Moving one 18 m hill by 0.5 m: rule plus the 4–6 chunks it touches | 1.2–1.5 ms median, under 3 ms max |
| Next frame, those chunks' colliders | 0.07 ms median |

The generator runtime regenerates at its throttle while a stamp is dragged, so the per-frame average during a drag is below these per-step numbers.

Colliders: the first version gave each chunk a triangle-mesh collider (`ColliderShape::Mesh` on its generated mesh); rebuilding those after a hill step took 3.5 ms median and 29 ms at worst. Chunks now hand physics their height grid (`GeneratedMesh::heights`), and the same `Mesh` collider becomes a parry heightfield split along the drawn mesh's diagonals: 0.07 ms.
