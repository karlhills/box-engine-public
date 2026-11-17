# M7 script boundary measurements

Machine: Apple M3 Ultra, macOS, release build. Command: `cargo test --release -p box_script --test bench -- --nocapture` (best of 3 runs, 2026-11-15). The script stage is timed alone (`script_update`), 20 frames after 3 warm-up frames.

| Case | Time |
|---|---|
| 10,000 `on_update` callbacks (`self.t += dt`) | 0.57–0.61 ms per frame |
| … without the watchdog interrupt | 0.57–0.59 ms |
| 10,000 callbacks, each `self.entity:translate(v)` | 1.34–1.38 ms |
| Batch: `world.query("Transform","Name")` over 5,000 + `e:translate` | 0.52–0.54 ms |
| Batch: `world.each("Transform", fn)` over 5,000 (copies) | 2.97–3.00 ms |
| `world.query("Transform","Name")` alone (5,000) | 0.16 ms |
| `e:position()` / `e:set_position(v)` | 0.07–0.08 µs |
| `e:get("Transform")` (generic copy) | 0.53 µs |
| `e:get("Transform.translation")` | 0.31 µs |
| `e:set("Transform", { translation = v })` | 0.52–0.55 µs |
| `e:get("Name")` | 0.21 µs |
| `Quat.from_rotation_y(a) * v` | 0.19 µs |
| `vector` arithmetic `(v + v) * 0.5` | 0.011 µs |

Spike S4 numbers (binding options) are in `spikes/s4_luau/FINDINGS.md`.

Reading: a frame budget of 16.7 ms affords 10,000 lightly scripted entities (0.6 ms) or several 5,000-entity typed batch passes. Generic component copies cost about 30× a typed accessor; hot loops use typed accessors and `world.query`.

## Script rules

`cargo test --release -p box_script --test script_rules -- --nocapture` (debug builds run 100 intents only). Fence rule over rows of 32 fences, each fence's end shared with the next (about 6 parts per fence). Measured twice (load average about 7.7, then about 6): the runs agree within a few percent. Before: commit 2feb907 with the same benchmark.

| Intents (parts) | Idle pass | One edit, before (whole file) | One edit, `scope = { neighbours = SNAP }` | 60 Hz drag, scoped (slowest) | One edit, no scope | 60 Hz drag, no scope |
|---|---|---|---|---|---|---|
| 100 (604) | 1.5 µs (was 4.0) | 4.5 ms | 0.19 ms | 0.17 ms (0.20) | 2.5 ms | 2.5 ms |
| 1,000 (6,032) | 9.3 µs (was 22) | 37.7 ms | 0.19 ms | 0.16 ms (0.21) | 25.2 ms | 25.4 ms |
| 5,000 (30,157) | 47 µs (was 115) | 194 ms | 0.34 ms | 0.33 ms (0.40) | 128 ms | 128 ms |

Profile before (1,000 fences, one edit): `out:part` 50% (reflection conversion 30%), the rule's own Luau 30%, reconcile of every owner 10%. Idle: `ScriptIntent` compares 36%, SipHash entity lookups 31%, the asset-server state lookup 1%. After: positional matching removed the hashing; the asset state is read only after a script asset event. The conversion cache (content-encoded component patches) makes one whole-file edit at 1,000 fences 30 ms instead of 38 ms; skipping unchanged owners makes reconcile negligible. At 5,000 scoped intents about 0.2 ms of a pass is the engine-wide transform propagation after a changed part. A spatial hash for neighbour search was not needed: a linear bounds scan costs about as much per intent as the change gate.

Earlier layout (fences 6 m apart, 7 parts each, no shared ends), before: 5.3 ms per edit at 100 intents and 54 ms at 1,000. The first fence rule's O(n²) end search cost 205 ms at 1,000 intents; a spatial hash in Luau made it 45 ms.
