# M6 renderer (RS3) measurements

2026-11-16, Apple M3 Ultra, Metal, 2560x1440. The machine was shared with other GPU jobs: minimums are reported (other work only adds time); medians were 3–10x higher. Worth re-running `post_cost` on an idle machine.

## Effects

`cargo test --release -p box_engine --test post_cost -- --ignored --nocapture` (showcase, configurations interleaved, 300 frames).

| config | render wall min | GPU frame min |
|---|---|---|
| none | 1.32 ms | 0.26 ms |
| AO | 2.01 ms | 0.89 ms |
| bloom (5 mips) | 1.99 ms | 0.56 ms |
| grade | 1.60 ms | 0.08 ms |
| all | 2.58 ms | 1.02 ms |

AO pass interval minimums: prepass 0.032 ms, downsample 0.025, SSAO 0.10, blur 0.08. The upsample is inside the main pass. With 7 bloom mips, bloom cost +1.1 ms wall (13 passes, mostly per-pass overhead); 5 mips cost +0.66 ms.

## S5 (thin poles and wall corners)

Prepass + resolve: 4x MSAA 0.18–0.22 ms, 1x 0.056 ms. Pixels differing by more than 10% from the 1x result in the AO view: MSAA sample 0: 40, farthest: 215, nearest: 443; plain bilinear upsample: 750–818.

## Instancing

`instancing_stress --mode identical|mixed [--move 0.1] --no-post --headless --frames 300 --size 2560x1440`

| scene | draws | shadow draws | CPU to submit p50 | GPU (pass interval sum) | upload / frame |
|---|---|---|---|---|---|
| identical, before | 15,080 | 6,621 | 14.0 ms | 0.80 ms | 2.50 MB |
| identical, after | 9 | 5 | 0.94 ms | 0.37 ms | ~1.7 KB idle (no instance data) |
| mixed, before | 15,099 | 6,636 | 17.5 ms | 1.65 ms | 2.50 MB |
| mixed, after | 127 | 24 | 1.29 ms | 0.74 ms | idle as above |

10% of objects moving every frame (scattered): 1.44 MB per frame (all 64-slot chunks dirty), about +0.2 ms CPU.
