# M1 build times

Date: 2026-11-15. Machine: Mac Studio, Apple M3 Ultra. Rust 1.94.0. Dev profile from the workspace `Cargo.toml` (crate code at `opt-level = 1`, dependencies at `opt-level = 3`, line tables only). Fresh target directory, dependency sources already downloaded.

| Build | Command | Time |
|---|---|---|
| Clean, whole workspace with examples | `cargo build --workspace --examples` | 27 s |
| Incremental after touching `box_render/src/renderer.rs` | same | 1.0 s |
| Incremental after touching `box_core/src/world.rs` | same | 1.2 s |

Incremental times were measured by touching a file; a real edit that changes code generation will take somewhat longer. Re-measure at the end of each milestone (risk R19).
