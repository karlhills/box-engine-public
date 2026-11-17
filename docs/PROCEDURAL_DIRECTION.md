# Procedural Direction: intent in, coherent editable results out

Status: direction review, 2026-11-15. No code changed. This document assesses a proposed direction (procedural authoring inspired by Oskar Stålberg's work) against the engine as it exists, and says what to add, what to leave alone and where it belongs in the roadmap. The mechanics stay in [PROCEDURAL.md](PROCEDURAL.md); decision D7 in [DECISIONS.md](DECISIONS.md) is unchanged.

## 1. Executive summary

- **The philosophy is already the plan.** "Developers define intent and relationships; the engine generates coherent, editable results" is what D7 and spike S6 established: saved semantic components, a rule system that writes neighbour facts into generator parameters, `reconcile` producing unsaved derived entities, and a parameter hash that decides what regenerates.
- **A generalized "Procedural Rules System" with pluggable resolvers and many spatial representations should not be built now.** It has no near-term user (VISION principle 2), and S6 showed that plain rule functions are enough for the hardest case we have. The right framework is the smallest one that makes rule functions observable and safe: about four small additions to the M5 design.
- **Five gaps are real** and cheap to close in design before M5 code exists: provenance of generated output, manual overrides, selection of generated objects, rule authoring without Rust, and drawing tools.
- **The first experiment (neighbour-aware walls) fits the architecture without disruption** if it is built as the M5 acceptance example rather than a separate spike, with a fence as a second, non-architectural user of the same rule code.
- **Wave function collapse stays deferred.** The valuable Stålberg lessons are about input design and feel, not about the solver.

Not done for this review: the Stålberg material is summarized from his published talks and articles as recalled, not re-read; no subagent reviews were run because the codebase evidence (S6 findings, existing docs, existing `Instanced` mechanism) was sufficient to answer the questions.

## 2. Lessons from Oskar Stålberg's work

| Technique | What it is | Value to box-engine |
|---|---|---|
| Tiny input, everything else derived | Townscaper saves little more than which cells are filled and their colour. Geometry, detailing and decoration are all derived. | **Core principle, already adopted** (parameters are truth, meshes are cache). |
| Adjacency-driven module selection | A marching-cubes-style lookup: the filled/empty state of a cell's corners selects a module. Deterministic, no search. | **Useful later, cheap.** A bitmask lookup over grid neighbours that selects a mesh covers modular kits (sci-fi corridors, dungeon tiles, fences) without a solver. Candidate for M8, only if the second game wants it. |
| Constraint solving (WFC) | Picks among several valid modules so neighbours agree; Bad North generates whole islands this way. | **Deferred.** Needs hundreds of hand-authored, mutually compatible modules (an art cost), fails non-locally and is hard to debug. No box-engine milestone needs it. |
| Irregular quad grid | A relaxed, organic grid; modules are deformed to fit each cell. | **Deferred.** It is a look, not an architecture. It could arrive later as one spatial source feeding the same rule/reconcile path. |
| Contextual detailing | Bespoke pattern rules add stairs, arches, props where the surroundings allow. | **Adopt as a pattern**: decoration rules are ordinary rule functions reading the same intent. Belongs to games and `box_arch`, not the core. |
| Incremental regeneration | Only the neighbourhood of an edit changes. | **Already adopted** through the parameter hash; S6 measured 2 to 4 of 6 parts regenerating per edit. |
| Predictable at large scale, surprising in detail | The player controls shape; the system chooses trim. | **Adopt as a design test** for every rule: the same intent must give the same large-scale result; only seeded detail may vary. |
| Feel | Every edit responds within a frame, with a small animation and sound. | Matches the ease-of-use goal (immediate feedback). A latency budget, not an algorithm. |

The specific algorithms (WFC, irregular grids, marching cubes) are answers to Townscaper's problem: one input type, one art style. The principles transfer; the algorithms should be adopted one at a time, when a milestone needs one.

Different problems need different tools, and none of these need a shared solver:

| Problem | Right tool here |
|---|---|
| Wall connections | Deterministic adjacency rules writing junction parameters (S6 rules J1 to J5) |
| Curved walls, rails, roads | Parametric sweep along a spline |
| Openings | 2D booleans in elevation space |
| Roofs | Closed-form generators; straight skeleton later |
| Vegetation | Seeded scatter with masks and exclusion rules |
| Modular kits | Adjacency bitmask lookup; constraint solving only if a kit has real ambiguity |

## 3. Procedural authoring philosophy

1. **Intent is what is saved.** A wall path, a room, a road spline, a scatter region. Small, reflected, editable through `box_core::edit`.
2. **Rules are pure functions from intent to generator parameters.** Every relationship (this end meets that wall, this corner is concave) becomes a parameter. Nothing reads its neighbours during generation.
3. **Generated output is disposable.** Derived entities and meshes are never saved and can always be rebuilt from intent, so undo, save and reload are correct by construction.
4. **Opt-in.** A scene with only imported models and ordinary entities never touches this layer. Procedural authoring sits above the scene system, not inside it.
5. **Explainable.** Every generated object can say which rule made it and from which intent; every failure is a visible warning on the object the user edits.
6. **The user keeps control.** Generated details can be overridden or baked to an ordinary mesh.
7. **No algorithm vocabulary in the interface.** Users see "Corner", "End cap", "Seed", "Density"; never entropy or propagation.

## 4. Applicability to the existing codebase

What exists today (M4 complete, `box_proc` not started):

| Need | Existing piece | State |
|---|---|---|
| Saved intent with inspector, undo, CLI and MCP access | `Reflect` components, `box_core::edit` (`EditStack`, JSON `Command`), `Uid` | Built |
| Parameters to mesh on change | `Primitive` + `sync_scene_meshes` (change-driven) | Built; generalizes to `Generator` in M5 |
| Unsaved, non-editable generated entities | `prefab::Instanced`: skipped by `scene::save`, excluded from editor queries, picking maps to the instance root (`prefab::editable_ancestor`) | Built for prefabs; **the same mechanism serves `Derived`** |
| Material groups per mesh | `MeshRenderer.materials` by group slot | Built in M4 (the S6 engine gap is closed) |
| Colliders following meshes | Change-driven `sync_physics` | Built |
| Junction geometry and the rule/reconcile pattern | Spike S6 (walls, corners, openings, roofs, `plan`, reconcile; not published) | Proven as throwaway code |
| `Generator`, `Procedural`, `Derived`, `reconcile`, throttled flush | [PROCEDURAL.md](PROCEDURAL.md) sections 2 and 5 | Designed, scheduled for M5 |
| Viewport tools | Extension point scheduled for M5 | Not built |

Assessment: the architecture supports this direction well. Nothing needs refactoring. The five conceptual layers map onto existing or planned pieces without new crates:

| Layer | Where it lives |
|---|---|
| Intent | Reflected components in the scene (game or plugin types) |
| Rules | Rule functions in the game or a plugin such as `box_arch` |
| Resolution | The rule function's return value: keyed generator parameter sets. A solver, if one is ever needed, is an implementation detail of one rule |
| Generation | `Generator::generate` in `box_proc`, run by the throttled flush |
| Presentation | Ordinary `MeshRenderer`, materials, colliders |

## 5. Architecture recommendations

The smallest coherent framework is the M5 design plus four additions. All live in `box_proc` (and a few lines in `box_editor`); none changes what scenes store.

**5.1 Named rule registration.** PROCEDURAL.md describes rule systems as "ordinary game code in `Update`". Keep the function plain, but register it:

```rust
// Illustrative.
app.add_rule("walls", walls::plan);   // fn(&RuleInput<WallPath>) -> Vec<Part>
```

The engine then owns the boring parts once: run the rule only when its intent components changed (the cached-copy comparison transforms already use), call `reconcile`, record provenance, collect warnings, and list rules for debugging. A game that prefers a hand-written system can still call `reconcile` directly. Whether this wrapper earns its place is a question for the prototype (section 7).

**As built (M7, script rules).** One rule per `.luau` file; scope defaults to every intent naming it. Measured at 1,000 intents, a whole-file run cost 38–54 ms per edit, too slow to drag. Rule files now declare a narrower scope: `scope = "intent"` or `{ neighbours = r }` (the intent plus intents whose world bounds come within r). An edit rebuilds the changed intents and those within r of the change's old and new bounds, from a context that adds their own neighbours; the result matches a whole run (seeded random-edit tests compare both, and a fresh load). A linear bounds scan was enough; no spatial-hash index. The fence declares `{ neighbours = SNAP }` with pairwise end-post ownership. Results: 0.19 ms per edit at 1,000 fences and 0.34 ms at 5,000. Whole-file rules cache converted components by content and skip reconcile for unchanged owners (25 ms at 1,000). Typed Rust rules can opt into the same skip through `RuleParts::skipping_unchanged`.

**5.2 Provenance.** `Derived { owner, key }` becomes `Derived { owner, key, rule }`. `GenOutput.warnings` and rule-level warnings are attached to the owner, so the inspector on the thing the user edits shows "window dropped: too close to corner". This answers "which rule made this" and "why did it fail" with one extra field.

**5.3 Selection.** Clicking generated geometry selects the owner, through the same path `Instanced` uses today (`prefab::editable_ancestor`). Generalize that function to cover both markers rather than adding a second one. A modifier click selects the derived part itself, read-only, to show its key, rule and parameters.

**5.4 Manual overrides.** An optional saved `Overrides` component on the owner: a map from part key to a small patch (reflected field path and value, or `hidden`). `reconcile` applies patches after the rule output and before hashing, so an override is just another parameter. Two rules follow:

- Part keys must be stable and semantic (`<owner uid>/end0`, `room2/wall1`), never positional indices that shift when a neighbour is added.
- An override whose key no longer exists is kept and reported as a warning, never dropped silently.

Design the key convention in M5. Build `Overrides` when the first user needs it (expected in M6).

**As built (M6).** `box_proc::overrides::Overrides { parts: [PartOverride { rule, part, hidden, fields: [FieldOverride { path, value }] }] }` on the owner, registered by the engine. `reconcile` applies the owner's overrides for its rule after the rule output and before hashing (a patched component is compared with the rule's last unpatched value, so it regenerates only when that or the patch changes); a hidden part is not spawned; unknown parts, fields or components are kept and reported on the owner. `add_rule`'s gate includes each intent's `Overrides`. Measured: about 0.9 µs per overridden part per rule pass (1,000 walls: 2.5 → 3.4 ms). Overrides apply after the rule, so neighbours do not react to them (a wall made taller by an override keeps its neighbour's junction height): change the intent for that.

**What not to add now**

- No resolver trait, no constraint engine, no rule description language. Rules are Rust functions; from M7, script functions.
- No spatial-representation abstraction. Splines and polygons are scheduled (M5); grids and graphs get a helper when a second user appears.
- No new crate for rules. If shared adjacency helpers (endpoint snapping, spatial hash, connected components) are needed by two rule sets, they go in `box_proc` then.

**Propagation beyond immediate neighbours.** A rule is a pure function over a *scope*, not over one entity, so non-local effects (raising one room splits a roof run two rooms away) are handled by recomputing the scope and letting the hash skip unchanged parts. Scope starts as "all intent of this type"; S6 measured 2.7 µs for three rooms. When that stops being cheap, the scope narrows to connected components found with a spatial hash. This is an open performance question, to be measured in the prototype at 10, 100 and 1,000 walls, not assumed.

## 6. Editor UX recommendations

| Workflow | Recommendation | When |
|---|---|---|
| Draw walls and paths | A viewport tool: click to place points, snap to existing ends and bodies, ghost preview of the generated result while drawing, Enter to commit as one undo step | M5 (tool extension point, spline handles); wall tool in M6 |
| Adjust parameters | The inspector, already generated from reflection, with ranges, units and tooltips | Works today for any reflected intent |
| Immediate preview | Synchronous generation in the throttled flush; S6 timings (under 0.2 ms per part) leave ample room | M5 |
| Select generated objects | Click selects the owner; modifier click inspects the part | M5 |
| See what produced an object | Read-only "Generated by" section: rule name, key, owner link | M5 |
| Understand failures | Warnings on the owner in the inspector and as a viewport badge; routed to the Console with the toast and badge from [DX_REVIEW.md](DX_REVIEW.md) so they are never silent | M5 |
| Override a detail | Edit a field on the inspected part; it becomes an override with a reset button, shown like a prefab override | M6 |
| Paint regions | A brush tool writing a mask or polygon into a scatter intent | With scatter, M6 |
| Edit rule sets | First: a rule list panel (name, intent type, part count, last run time, warnings, enable toggle). No node graph. A visual rule editor only after script rules exist and real rule sets show what needs editing | List in M5 or M6; editor after M9 |

**As built (M6).** Draw walls and paths: `box_editor::path_tool` (generic, game-configured; walls registers Draw Wall and Draw Fence). Ghost = real rule output on a `Preview` owner; 0.5 ms per pointer move for the whole editor frame at 100 walls, nothing while the mouse is still. Snapping hysteresis is `box_proc::snap::Snapper` (moved from examples/walls). Override a detail: as §5.4. Rule list: View ▸ Rules, switches per session (not saved). A rule reads other intent types with `app.rule_reads::<T>(rule)` (`RuleInput::read::<T>()`, in the change gate); the walls fence uses it to butt wall faces. Paint regions: the Paint Scatter brush (EDITOR.md).


- **Snapping needs hysteresis.** A wall end that connects at 10 cm must not disconnect until, say, 15 cm, or junctions flicker during a drag.
- **Progressive disclosure.** A beginner draws a wall and it connects. Junction kind, ownership and thresholds are visible only when the part or the rule is inspected.

## 7. First prototype specification: neighbour-aware walls

**Form.** Not a separate spike. S6 already answered the geometry questions with throwaway code; what is untested is the path through the real engine: the edit stack, save and load, the editor and the flush. So the prototype is an example crate (`examples/walls`, shaped like `examples/collector`) built during M5 on the real `box_proc`, and it doubles as the M5 acceptance scenario for derived entities. It does not need `box_arch` (M6): a wall here is a rectangular profile swept along a polyline with joint-line parameters at each end, which uses only M5 building blocks. Openings and roofs stay out.

**Intent (saved).** `WallPath { points, height, thickness }` on an entity. Nothing else.

**Rule (`walls::plan`, pure).** For each wall end:

| Situation | Result |
|---|---|
| Free end | `Open` cap |
| Meets another wall's end, same thickness | Mitre: both get the shared joint line |
| Meets another wall's end, different thickness, or meets a wall's body | Butt: the owner (lower `Uid`) runs through, the other is cut on its face line |
| Three or more ends at one point | Butt against the longest pair; warning if unresolved |
| Corner sharper than the mitre limit | Bevel, with a warning |

Output: one part per wall, key `<uid>/wall`, parameters including both end joints.

**Second user of the same rule.** `FencePath` (posts and rails) uses the same adjacency function with a different generator: shared corner posts instead of mitres. This is the generality check: if the fence needs changes to engine code rather than to its own rule and generator, the abstraction is wrong.

**Behaviour to demonstrate.** Place a wall; place a second touching it and see the connection; drag one to a new position and angle; undo; save, close and reopen.

**Measurements and pass criteria.**

| Measure | Criterion |
|---|---|
| Edit to visible mesh | Within one frame at display refresh while dragging, for a 100-wall scene |
| Parts regenerated per edit | Only the moved wall and walls it touches or stopped touching |
| Rule pass time | Recorded at 10, 100 and 1,000 walls; decides whether scoping is needed |
| Undo | After undo, scene serialization is byte-identical and the hash of every derived mesh matches the original |
| Save and reload | Derived mesh hashes match before and after |
| Scene file | Contains only `WallPath` / `FencePath`; no derived entities, no meshes |
| Reuse | A second scene works with no code change; the fence needs no engine change |
| Per-frame cost when idle | Flag checks only; no rule run, no generation |

**Instability cases to probe and record.** Snap threshold flicker, three or more walls at a point, acute angles, mixed thickness, T junction mid-side, a wall crossing another, zero-length segments, a wall dragged through a junction, two walls exactly collinear and overlapping. Each gets a capture and either a rule or a warning.

**Plan (during M5, after the `Generator` trait and flush exist).**

1. `Derived` (with `rule`), `reconcile` and scene-save exclusion in `box_proc`, with unit tests.
2. Sweep with joint-line ends; golden hash and capture tests.
3. `walls::plan` with property tests (pure function, no engine needed).
4. Example crate with headless tests for the behaviours and measurements above.
5. Editor: selection mapping, "Generated by" section, warnings on the owner.
6. Fence as the second user; write down what had to change.
7. Record numbers in `docs/measurements/` and decide on 5.1 (`add_rule`) and on scoping.

**Outcome (examples/walls, 2026-11-15).** Built as planned on M5 `box_proc`; the fence needed no engine change (its own rule and generator over the shared `adjacency::analyse`). Every pass criterion has a headless test (`examples/walls/tests/acceptance.rs`); numbers in [measurements/m5-walls.md](measurements/m5-walls.md).
- Hysteresis is per gesture: the rule tests a fixed 10 cm, a `Snapper` held by whatever drags a point keeps a snap until 15 cm and writes the snapped position. A remembered connection set cannot pass undo or reload (12 cm apart, drag to 5, back to 12, undo: remembered "connected", original not).
- Ownership of a butt: the taller wall, then the lower uid (a lower owner leaves the taller wall's end open; S6 J3).
- Instability cases: snap flicker (raw writes flip 7 times in 10 jittered steps, the Snapper once); 3+ ends (longest pair joins, others butt, "unresolved" warning on overlap); acute (< 29°: capped, bevel fill, warning); mixed thickness (butt at any angle, in line: the larger section caps); T mid-side (cut on the near face; grazing angles capped with a warning); crossing, overlap, zero-length and collapsed walls warn on both owners; dragging through a junction goes cap → T → crossing warning with finite geometry throughout.
- Snapping carries a move one wall further: moving a wall that owns its junction points (lowest uid) moves its partners' snapped ends, so their other junctions regenerate (4 parts per nudge in a room). Coincident data, as the Snapper writes it, regenerates only the wall and the walls it touches.
- Since M6 a rule can read other intent types (`rule_reads`): a fence end in a wall's footprint butts its face without a post.
- The prototype found one engine bug: the rule gate compared its cached intents by query position, so an owner changing archetype re-ran the rule once with no change. Fixed in `box_proc::rule` (intents matched by entity, test `add_rule_ignores_query_order_changes`).

## 8. Roadmap integration

Architectural preparation (design decisions, no feature work) is separated from implementation.

| Milestone | Preparation | Implementation |
|---|---|---|
| Now | This document; consistency edits to PROCEDURAL.md, ROADMAP.md and EDITOR.md | None |
| M5 | Part-key convention; `Derived.rule`; decide `add_rule` after the prototype | `Derived`, `reconcile`, selection mapping, "Generated by" and warnings in the inspector, `examples/walls` with the fence |
| M6 | | Wall and path drawing tools, `Overrides`, rule list panel, `box_arch`, scatter with painted regions |
| M7 | Script API shape for rules | Rules and `reconcile` callable from Luau, so script projects can author rules |
| M8 | Owner decision: should the second game include one non-architectural rule set (roads, fences or a modular kit with adjacency lookup)? Recommended, since it is the generality gate | Whatever that game needs |
| After M9 | | Visual rule editing, irregular grids, constraint solving, terrain-aware roads (needs terrain), community rule sets |

## 9. Risks and trade-offs

| Risk | Assessment | Mitigation |
|---|---|---|
| Excessive abstraction | High if a general rules framework is built first | Four small additions only; every further piece needs a second user |
| Unstable part keys break overrides | Medium | Semantic keys by convention, tested in the prototype; orphaned overrides are reported |
| Regeneration instability (flicker, popping) | Medium | Snap hysteresis; deterministic ownership (by `Uid`); instability cases captured in the prototype |
| Non-local rule effects surprise users | Medium | Rules scoped to connected intent; the rule list shows what changed |
| Rule pass cost at scale | Unknown until measured | Change-gated; scope to connected components when the measurement says so |
| Difficult debugging | Medium | Provenance, warnings on the owner, pure functions that are unit-testable without the engine |
| Serialization complexity | Low | Only intent and overrides are saved; both are ordinary reflected components |
| Inappropriate use of WFC | Low while deferred | Adopt only with a concrete module set and a case deterministic lookup cannot handle |
| Becoming a one-game engine | Medium | Fence in the prototype; M6 criterion "no game-specific type in engine crates"; M8 gate |
| Rules only authorable in Rust | Real for script-project users until M7 | Script access in M7; stated as a known limit until then |
| Scope added to M5 | Small but real | The additions replace work M6 would need anyway; the example replaces part of the M5 test scenes |

## 10. Decisions: now versus later

| Decide now | Recommendation |
|---|---|
| Is a general procedural rules framework built now? | No. Rule functions plus `reconcile`, with provenance |
| What marks generated entities? | `Derived { owner, key, rule }`, sharing the unsaved and pick-to-owner behaviour of `Instanced` |
| Part-key convention | Stable, semantic, derived from `Uid` and role |
| Form of the first experiment | An M5 example crate with a second, non-architectural user |
| WFC, irregular grids | Stay deferred |

| Decided after the prototype | Decision |
|---|---|
| Keep the `add_rule` wrapper or leave rules as plain systems | **Keep `add_rule`.** The same walls rule as a plain system gives identical meshes in an App but needs its own change gate, owner grouping, owner clean-up and explicit ordering before `sync_scene_meshes`, and it does not run in the editor's edit world (only `Game::rules()` does). Plain systems stay fine for Play-only code. |
| Rule scoping strategy | **No connected-component scoping.** A spatial grid inside the adjacency function made the rule linear (1,000 walls: 5.2 → 1.2 ms); a whole edit costs 2.0 ms at 1,000 walls (rule 1.2, reconcile 0.7). Revisit if reconcile across all owners grows. |
| `Overrides` patch format | Per part: rule name + role (key without owner uid), `hidden`, and field overrides as reflected path from the component's short name (`WallGen.height`) + value as JSON text in scene-file form (f32 in shortest form). Stale entries are kept and warned. Rule name included so staleness is exact per rule. |

| Decide later | When |
|---|---|
| Script rule API | M7 |
| Whether M8 includes a procedural rule set | Owner, with Q5 |
| Adjacency lookup helper, visual rule editor, constraint solving | When a milestone needs each |
