# M6 renderer, RS3 on demand

2026-11-16, Apple M3 Ultra, Metal, 2560x1440, `cargo test --release -p box_engine --test look_cost -- --ignored --nocapture` (blocks of 150 frames after 10 warm-up; minimum / p10; times in ms).

| scene, config | render min | GPU frame min / p10 | GPU delta min / p10 |
|---|---|---|---|
| pillars, all off (AO off) | 2.084 | 0.883 / 0.930 | |
| + haze, mist, quieting | 2.097 | 0.904 / 0.962 | +0.021 / +0.033 |
| + layered sky | 2.090 | 0.874 / 0.921 | noise |
| + stars | 2.098 | 0.888 / 0.914 | noise |
| + grade SMH + vignette | 2.076 | 0.880 / 0.942 | noise |
| + depth fade (prepass) | 2.267 | 0.975 / 1.082 | +0.092 / +0.152 |
| all on, AO off | 2.282 | 0.993 / 1.106 | +0.111 / +0.176 |
| all on + AO | 2.820 | 1.428 / 1.519 | +0.435 / +0.413 vs AO off |
| time-of-day sweep every frame | 2.795 | 1.399 / 1.532 | noise; 0 pipelines, 0 reallocations |
| foliage, cards opaque | 2.286 | 0.963 / 1.179 | |
| foliage, alpha-tested | 2.266 | 0.971 / 1.186 | +0.008 / +0.007 |
| wind scene, no sway | 2.361 | 0.936 / 1.189 | |
| wind scene, sway in all passes | 2.366 | 0.955 / 1.194 | +0.019 / +0.005 |
| night, stars off → on | 2.309 → 2.315 | 0.951 → 0.904 | noise |
