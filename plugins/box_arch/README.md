# box_arch

Architectural construction for box-engine (M6, [docs/PROCEDURAL.md](../../docs/PROCEDURAL.md) §9): an optional plugin, not in the engine's default plugins. Add it with `Game::new().with_plugin(box_arch::ArchPlugin)`; its generators appear under Add ▸ Architecture.

| Type | What |
|---|---|
| `Wall` | Solid wall along a polyline with arcs (`WallPoint::bulge`) or a `Spline` (this or another entity). Ends: `WallEnd::{Cap, Open, Extend, Abut, Mitre, Butt}`. Openings (`Opening`: window, door, passage; flat, round or pointed head) cut in 2D elevation with reveals, panels, sills and glazing bars. Plinth, cornice, coping. Top flat or following a `RoofUnderside`. Slots: Wall, Reveal, Top, Trim, Glass, Door |
| `Roof` | Gable, hip, shed over rectangles; cone, dome over circles. Slab, soffit, fascia/barge boards, ridge/hip rolls, `RoofEnd::Abut`. `Roof::underside()` is what walls under it rise to. Slots: Roof, Underside, Trim, Ridge |
| `Platform` | Slab from a footprint (rectangle, circle, polygon) with a lip or trim; top at y = 0. Slots: Top, Sides, Trim |
| `Stair` | Straight flight between two points, solid or open (stringers), optional landing; warns when the slope is impossible. Slots: Tread, Riser, Side |
| `Anchor` | Places a decoration on a wall face, wall top, wall opening or roof ridge (frame: X along, Y up, Z out; offset (u, v, w)); `resolve_anchors` (PostUpdate) keeps it there as the target changes. Games add targets with `AnchorTargets::add::<T: Anchorable>()` |
| `junction` | `mitre`, `butt` (owner/subordinate), `tee`, `can_mitre`: wall end parameters for walls meeting at a point |

Conventions: plan points are `Vec2` (x, z), entity-local. A wall's +w side (outer: plinth, sills, panels set back from it) is right of its direction of travel, the outside of a loop drawn counter-clockwise seen from above. U is arc length from the first point. All generators bake ambient occlusion into vertex colours (`ao` strength, 0 disables), emit UV set 0 in metres and UV set 1 as weathering data (walls: height above the foot, depth below the top). Magic numbers live in `consts.rs`.

Generic only: rules (ownership, which junction, door placement, window bays, roof runs) belong to games. A rule computes junctions with `junction`, transforms `Roof::underside()` into each wall's space and writes them into the parts it reconciles.

```bash
cargo run -p box_arch --example arch_gallery -- --capture target/captures/arch/gallery.png --size 1920x1080   # --view reveals|eaves|junctions|ao|tower|back
cargo run --release -p box_arch --example arch_bench    # generation and anchor timings
cargo test -p box_arch                                   # golden, properties, warnings, runtime, editor (GPU)
BOX_UPDATE_GOLDEN=1 cargo test -p box_arch --test golden # re-record after a VERSION bump
cargo test --release -p box_arch --test editor_drag -- --nocapture   # editor drag frame times
```

Known gaps: roof valleys, L runs and side-on abutment; `ColliderHint::Boxes` from walls is not used by physics yet. Numbers: [docs/measurements/m6-arch.md](../../docs/measurements/m6-arch.md).
