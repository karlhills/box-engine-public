# W3 ground-aware placement: costs

Recorded 2026-11-21 on Apple Silicon (release build), with other work running on the machine.

`cargo test --release -p box_engine --test world_building ground_cost -- --ignored --nocapture`

| Case | Cost |
|---|---|
| Ground system (`update_ground` + `follow_ground`), 1,000 `FollowGround` entities on a 256 m terrain, nothing changed | 0.056 ms per frame |
| The same after a ridge edit that moves about a third of them | 0.057 ms |

Idle work is one compare per terrain and a cache compare per follower (its translation, offset, parent transform and the ground version). A terrain edit re-samples only followers under the changed area. Generators that follow the ground (`Sweep.follow_ground`) regenerate through the generator runtime's throttled flush, and script rules that sample the ground re-run only for intents in the changed area (`crates/box_script/tests/ground.rs` checks that an edit away from every intent runs nothing).
