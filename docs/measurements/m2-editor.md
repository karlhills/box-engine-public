# M2 editor measurements

Apple M3 Ultra, Metal, headless, 2026-11-15. Source: `cargo test -p box_editor --test acceptance -- --nocapture` (acceptance test (a)).

## 1,000-entity scene

1,038 entities, hierarchy fully expanded and scrolled every frame, one entity selected with the translate gizmo, 2560x1440 at 2x. Every frame waits for the GPU, so "total" is serialized latency, not a frame-rate limit.

| Profile | UI pass (mean / p95) | Render CPU (mean / p95) | Total incl. GPU wait (mean / p95) |
|---|---|---|---|
| Release | 0.31 / 0.46 ms | 2.11 / 2.56 ms | 4.35 / 5.02 ms |
| Dev | 0.41 / 0.60 ms | 4.59 / 5.27 ms | 6.85 / 7.63 ms |
| Dev, dependencies without debug-assertions | 0.31 / 0.46 ms | 2.16 / 2.68 ms | 4.40 / 5.23 ms |

First recording (before caster culling and change-driven propagation): release 3.9 ms render CPU, dev 12.5 ms.

Transform propagation and `sync_scene_meshes` are not the cost: at 1,000 entities in steady state they take 0.004 ms and 0.005 ms (release), 0.006 ms each (dev); propagation was 0.03–0.04 ms before it became change-driven. Render CPU is dominated by wgpu `CommandEncoder::finish` encoding about 1,000 draws per pass (profiled with `sample`). In dev, the dependencies' debug assertions (objc2 message verification, wgpu-core checks) doubled it; the workspace dev profile now turns them off for dependencies. The release lever is draw count: instancing (RS3).

## Shadow caster culling

Showcase 1280x720: draws 162 → 102, shadow draws 128 → 68, triangles 29,550 → 17,758, upload 51,720 B/frame unchanged, images byte-identical. Editor default capture: 160 → 97 draws.

## Reflection build cost (box_core)

Clean `cargo build -p box_core`: 9.3 s to 20.9 s; incremental after a box_core change: 0.27 s to 1.4 s. Measured with the machine heavily loaded (load average 33), so the clean figure is pessimistic; spike S2 estimated +5–6 s.
