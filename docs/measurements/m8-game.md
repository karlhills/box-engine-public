# M8 game measurements (Cinder Keep)

Machine: Apple M3 Ultra, macOS 26.6.2, release build, 1920x1080 headless renderer (GPU waited for each frame). Command: `cargo test --release -p box_engine --test m8_game wave -- --nocapture` (2026-11-16). The hero is put in the courtyard; the wave spawns; frames 120–420 measure the swarm (every cinderling alive around the hero, spitters throwing embers), frames 420–720 the hero sweeping every 0.5 s (kills, bursts of sparks). "update" is every system of the frame (one fixed step included); "scripts" is `ScriptStats::update_time + fixed_time`.

| Wave | Phase | update ms (mean / p95) | scripts ms (mean / p95) | render ms (mean / p95) | draws (mean / max) | live particles (max) |
|---|---|---|---|---|---|---|
| 120 cinderlings (the game) | swarm | 0.32 / 0.35 | 0.27 / 0.30 | 2.62 / 2.82 | 150 / 155 | 54 |
| 120 | sweeping | 0.11 / 0.24 | 0.07 / 0.16 | 2.57 / 2.76 | 157 / 246 | 1,301 |
| 200 (R24 stress) | swarm | 0.52 / 0.61 | 0.46 / 0.52 | 2.62 / 2.82 | 151 / 156 | 57 |
| 200 | sweeping | 0.16 / 0.37 | 0.11 / 0.29 | 2.59 / 2.78 | 168 / 272 | 1,670 |

Reading: the whole wave runs from one batch script (Lua arrays, a 1 m neighbour grid, typed `set_position`/`set_rotation`): about 2.3 µs per cinderling per frame, linear in count; the R24 risk does not materialise at this scale. All copies of an enemy share one mesh asset, so draws stay about 150 whatever the count (instanced); each death burst is one emitter entity and one draw, so draws peak during kills. Render cost is dominated by the scene, not the wave. The full playthrough (2,413 frames, both scenes) runs headless in 0.4 s wall (dev profile, which is optimized).
