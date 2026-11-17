# Spikes

Throwaway proofs of concept that answer one question each (see `docs/RISKS.md` section 2). Each spike is its own Cargo workspace (its `Cargo.toml` has an empty `[workspace]` table) and is excluded from the main workspace build. Code here is not maintained; findings go into `FINDINGS.md` in each spike and into the planning documents.

| Spike | Question | Gates |
|---|---|---|
| `s1_editor_shell` | Can egui deliver a refined, fast editor shell? | M2 (D2) |
| `s2_reflection` | Does `bevy_reflect` on `hecs` support inspector, serialization, clone and undo? | M2 (D3) |
| `s4_luau` | What shape should the Luau binding take (world access, entities, values, batch, waits, errors, sandbox)? | M7 (D4) |
| S6 (junctions; not published) | How hard are architectural junctions; does a construction game's document map onto generators? | M5 (D7) |
| `s7_export` | Can a prebuilt player plus a data pack become a signed `.app` and a Windows bundle without a toolchain; which pack format? | M9 (R16) |
