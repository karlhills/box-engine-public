# M8 lean native build

Date: 2026-11-16. Machine: Mac Studio, Apple M3 Ultra (28 cores). rustc 1.98.1. Release profile, fresh target directory, dependency sources already downloaded. Two runs each. Lean is `cargo build --release -p minimal_native --bin minimal-player` (box_engine features physics, audio). Full is the same command with `--features box_engine/dynamics,box_engine/script`.

| | lean (physics, audio) | full (all four modules) | saving |
|---|---|---|---|
| Clean release build | 28 s, 25 s | 31 s, 30 s | about 4 s (≈13%) |
| Binary | 29,502,896 B | 36,568,512 B | −7.1 MB (−19%) |
| Binary, stripped | 21,813,624 B | 27,336,984 B | −5.5 MB (−20%) |
| Crates (`cargo tree -e normal`) | 242 | 256 | −14 (rapier3d, nalgebra, mlua, mlua-sys, box_script …) |
| Crates incl. build deps (`-e normal,build`) | 247 | 270 | −23 (adds the Luau C++ source build) |
| Compiled units (fingerprints) | 310 | 341 | −31 |

Leaving out dynamics and script removes rapier3d (with nalgebra, matrixmultiply, typenum, ordered-float …), mlua with the vendored Luau build, and box_script. The binary is a fifth smaller. The clean build is only about 4 s shorter because a 28-core build is bound by its slowest chain of crates (wgpu, naga), not by the total amount of code.
