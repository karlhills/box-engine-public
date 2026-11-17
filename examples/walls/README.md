# Walls

The M5 acceptance scenario for derived entities ([docs/PROCEDURAL_DIRECTION.md](../../docs/PROCEDURAL_DIRECTION.md) §7): walls and fences drawn as paths connect to their neighbours automatically. Scenes save only `WallPath` and `FencePath`; every junction, part and mesh is derived, so undo, save and reload reproduce the geometry exactly.

| Path | What |
|---|---|
| `src/intent.rs` | `WallPath { points, height, thickness }`, `FencePath { points, height, post_spacing, post_size, rail_count }`: the only saved data (owner-local polylines) |
| `src/adjacency.rs` | Path-end adjacency shared by both rules: junction nodes (ends within 10 cm), T contacts, crossings, overlaps. Pure, uniform-grid broad phase |
| `src/plan.rs` | The `walls` rule: mitre, butt, bevel, T and 3+ resolution; one `WallGen` part per wall (`<uid>/wall`) |
| `src/wall_gen.rs` | `WallGen`: a rectangular section swept along the polyline (`box_proc::sweep`) with joint-plane ends and bevel fills. Slots: Wall, Top, Ends |
| `src/fence.rs` | The `fences` rule and `FenceGen`: one shared corner post per junction, posts by `divide_size` / `place_bays`, rails as sweeps. Slots: Posts, Rails |
| `src/snap.rs` | Re-exports `box_proc::snap::Snapper` (connect at 10 cm, let go beyond 15 cm; state per gesture, never saved) |
| `src/editor.rs` (feature `editor`) | `WallsEditor`: the Draw Wall and Draw Fence tools (`box_editor::path_tool`), passed to `run_game_with` by `walls-editor` |
| `authoring/`, `examples/walls_author.rs` | Scenes built from JSON edit commands |
| `assets/scenes/` | `yard` (every junction kind, fences meeting walls) and `court` (closed courtyard, rotated owner, garden fence) |

```bash
cargo run -p walls --features editor --bin walls-editor
cargo run -p walls --bin walls-player -- [--scene court] [--capture target/captures/walls/yard.png]
cargo run -p walls --example walls_author        # regenerate assets/scenes from authoring/
cargo test -p walls                        # acceptance, instability, properties, goldens, captures (GPU)
cargo test --release -p walls --test measure -- --nocapture
cargo test --release -p walls --test tools -- --nocapture   # draw tools, overrides, rule list; timings; captures in target/captures/m6_tools/
```

Junctions: free end → cap; two ends with the same thickness and height → mitre; otherwise a butt where the owner (taller, then lower uid) runs to the other's outer face and caps and the other is cut on the owner's inner face; an end on a body → cut on its near face; three or more ends → the longest pair joins and the rest butt against it (warning if one still overlaps); corners under the mitre limit (29°) → capped with a bevel fill and a warning. Crossings, overlaps, zero-length segments and collapsed walls warn on the owner (inspector, hierarchy and viewport badges).

Fences share the adjacency: ends that meet share one post (built by the lower uid), T ends snap onto the other fence's line. They need no engine code. The fence rule also reads walls (`app.rule_reads::<WallPath>("fences")`): a free fence end within a wall's half thickness + 10 cm butts the wall's face without a post; an end running along a wall keeps its post with a warning.

Numbers: [docs/measurements/m5-walls.md](../../docs/measurements/m5-walls.md).
