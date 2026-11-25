//! M6 procedural authoring tools in the walls editor, headless and driven
//! by egui events: Draw Wall and Draw Fence registered by a game's
//! `EditorPlugin`, the ghost preview while drawing, snapping with
//! hysteresis, committing as one undo step, editing points with handles,
//! overrides of a derived part from the inspector, the rule list and its
//! enable switch, and the cost of the ghost preview in a 100-wall scene.
//!
//! The last test needs a GPU: captures in `target/captures/m6_tools/`.
//! `cargo test --release -p walls --test tools -- --nocapture` prints the
//! timings.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, EditorPlugin, build_game_editor_with};
use box_engine::box_core::derived::{Derived, PartKey, Preview};
use box_engine::box_core::scene;
use box_engine::box_core::world::{Component, Placement};
use box_engine::box_proc::overrides::Overrides;
use box_engine::box_proc::rule::RuleRegistry;
use box_engine::box_proc::{ProcClock, ProcSettings, ProcState, mesh_hash};
use box_engine::prelude::*;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use walls::editor::{FENCE_TOOL, WALL_TOOL, WallsEditor};
use walls::{FenceGen, FencePath, WallEnd, WallGen, WallPath, authoring, fence, plan};

// ----- an egui driver (as in box_editor's tests) -----

struct Driver {
    ctx: egui::Context,
    time: f64,
    modifiers: Modifiers,
}

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        Self {
            ctx,
            time: 0.0,
            modifiers: Modifiers::NONE,
        }
    }

    /// One 60 Hz frame; the manual regeneration clock advances 1/60 s.
    fn frame(&mut self, editor: &mut Editor, mut events: Vec<Event>) -> Duration {
        let start = Instant::now();
        advance_clock(editor);
        events.insert(0, Event::ModifiersChanged(self.modifiers));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let _ = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        editor.prepare_render();
        self.time += 1.0 / 60.0;
        start.elapsed()
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    fn click_at(&mut self, editor: &mut Editor, pos: Pos2) {
        for e in click_events(pos, self.modifiers) {
            self.frame(editor, e);
        }
    }

    fn key(&mut self, editor: &mut Editor, key: Key) {
        for e in key_events(key, self.modifiers) {
            self.frame(editor, e);
        }
    }
}

fn advance_clock(editor: &mut Editor) {
    if let Some(s) = editor.state.world.get_resource_mut::<ProcSettings>()
        && let ProcClock::Manual(t) = &mut s.clock
    {
        *t += Duration::from_secs_f64(1.0 / 60.0);
    }
}

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    }
}

/// Move, press, release, one more frame: a click without a drag.
fn click_events(pos: Pos2, m: Modifiers) -> Vec<Vec<Event>> {
    vec![
        vec![Event::PointerMoved(pos)],
        vec![button(pos, true, m)],
        vec![button(pos, false, m)],
        Vec::new(),
    ]
}

fn key_events(key: Key, m: Modifiers) -> Vec<Vec<Event>> {
    [true, false]
        .into_iter()
        .map(|pressed| {
            vec![Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: m,
            }]
        })
        .collect()
}

// ----- projects and helpers -----

fn temp_project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("box_walls_tools_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    dir
}

fn open(dir: &Path) -> Editor {
    let options = EditorOptions {
        project: Some(dir.to_path_buf()),
        ..Default::default()
    };
    let plugins: [&dyn EditorPlugin; 1] = [&WallsEditor];
    let mut ed =
        build_game_editor_with(&options, LogBuffer::default(), authoring::game(), &plugins)
            .unwrap();
    ed.tree = box_editor::layout::default_layout();
    // A manual throttle clock far past the real-time epoch.
    ed.state.world.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::from_secs(3600)),
        ..ProcSettings::default()
    });
    ed
}

/// Looks down at the yard area around `target`.
fn look_at(ed: &mut Editor, target: Vec3, distance: f32) {
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = target;
    orbit.distance = distance;
    orbit.yaw = 0.0;
    orbit.pitch = 1.0;
}

/// Screen position (points) of a world point in the viewport.
fn screen(ed: &Editor, p: Vec3) -> Pos2 {
    let s = ed.state.gizmo_view().unwrap().project(p).unwrap();
    ed.state.viewport.rect.unwrap().min + vec2(s.x, s.y)
}

fn xz(x: f32, z: f32) -> Vec3 {
    Vec3::new(x, 0.0, z)
}

fn by_name(ed: &Editor, name: &str) -> Uid {
    let e = ed.state.world.find_by_name(name).unwrap();
    ed.state.world.uid(e).unwrap()
}

fn entity(ed: &Editor, uid: Uid) -> Entity {
    ed.state.world.entity_by_uid(uid).unwrap()
}

fn world_points(ed: &Editor, uid: Uid) -> Vec<Vec3> {
    let e = entity(ed, uid);
    let g = ed.state.world.get::<GlobalTransform>(e).unwrap().0;
    ed.state
        .world
        .get::<WallPath>(e)
        .map(|p| p.points.clone())
        .or_else(|| ed.state.world.get::<FencePath>(e).map(|p| p.points.clone()))
        .unwrap()
        .iter()
        .map(|p| g.transform_point3(*p))
        .collect()
}

fn part_of<T: Component + Clone>(ed: &Editor, owner: Uid) -> Option<T> {
    ed.state
        .world
        .query::<(&Derived, &T)>()
        .iter()
        .find(|(d, _)| d.key.owner() == Some(owner))
        .map(|(_, g)| g.clone())
}

fn wall_gen(ed: &Editor, owner: Uid) -> WallGen {
    part_of::<WallGen>(ed, owner).expect("a wall part")
}

fn part_hash(ed: &Editor, owner: Uid) -> Option<u64> {
    let meshes = ed.state.world.get_resource::<Assets<MeshData>>()?;
    ed.state
        .world
        .query::<(&Derived, &MeshRenderer)>()
        .iter()
        .find(|(d, _)| d.key.owner() == Some(owner))
        .and_then(|(_, r)| meshes.get(r.mesh).map(mesh_hash))
}

fn previews(ed: &Editor) -> usize {
    ed.state.world.query::<&Preview>().iter().count()
}

fn rule_runs(ed: &Editor, name: &str) -> u64 {
    ed.state
        .world
        .resource::<RuleRegistry>()
        .get(name)
        .map_or(0, |r| r.runs)
}

fn generations(ed: &Editor) -> u64 {
    ed.state
        .world
        .query::<&ProcState>()
        .iter()
        .map(|s| s.generations())
        .sum()
}

/// Draws a path with the active tool: a click per point, then Enter.
fn draw(ed: &mut Editor, d: &mut Driver, points: &[Vec3]) {
    for p in points {
        let at = screen(ed, *p);
        d.click_at(ed, at);
    }
    d.key(ed, Key::Enter);
    d.frames(ed, 2);
}

fn ready(dir: &Path) -> (Editor, Driver) {
    let mut ed = open(dir);
    let mut d = Driver::new(&ed);
    look_at(&mut ed, xz(2.0, -1.5), 11.0);
    d.frames(&mut ed, 3);
    (ed, d)
}

// ----- tests -----

#[test]
fn a_game_plugin_registers_the_wall_and_fence_tools_and_actions() {
    let dir = temp_project("register");
    let (mut ed, _) = ready(&dir);
    let tools: Vec<&str> = ed.state.tools().iter().map(|t| t.id).collect();
    assert_eq!(
        tools,
        [
            "place_on_surface",
            "edit_spline",
            "paint_scatter",
            WALL_TOOL,
            FENCE_TOOL
        ]
    );
    for (id, label) in [
        ("tool.draw_wall", "Draw Wall Tool"),
        ("tool.draw_fence", "Draw Fence Tool"),
        ("view.rules", "Rules"),
        ("edit.hide_part", "Hide Generated Part"),
    ] {
        let a = ed.state.actions().get(id).cloned().unwrap();
        assert_eq!(a.label(), label);
    }
    assert!(ed.state.run_action("tool.draw_wall").is_ok());
    assert_eq!(ed.state.active_tool(), Some(WALL_TOOL));
    // The tools survive opening another scene (a new world).
    ed.state.replace_world(authoring::game().new_world());
    assert!(ed.state.tools().iter().any(|t| t.id == FENCE_TOOL));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn drawing_a_wall_onto_another_ones_end_mitres_it_in_one_undo_step() {
    let dir = temp_project("mitre");
    let (mut ed, mut d) = ready(&dir);
    assert!(ed.state.activate_tool(WALL_TOOL));
    draw(&mut ed, &mut d, &[xz(0.0, 0.0), xz(4.0, 0.0)]);
    let a = by_name(&ed, "Wall");
    assert_eq!(ed.state.selection, vec![a]);
    let pa = world_points(&ed, a);
    assert_eq!(pa.len(), 2);
    assert!(pa[1].distance(xz(4.0, 0.0)) < 0.02, "{pa:?}");
    assert_eq!(wall_gen(&ed, a).end, WallEnd::Cap);
    let undo_before = ed.state.edit_undo_len();
    let scene_before = scene::save_string(&ed.state.world).unwrap();
    let hash_before = part_hash(&ed, a).unwrap();

    // The second wall starts 7 cm from the first one's end: it snaps.
    let near_end = pa[1] + Vec3::new(0.05, 0.0, 0.05);
    let at = screen(&ed, near_end);
    d.click_at(&mut ed, at);
    let corner = xz(4.0, -3.0);
    let at = screen(&ed, corner);
    d.frame(&mut ed, vec![Event::PointerMoved(at)]);
    d.frames(&mut ed, 2);
    // Mid-draw: one preview, never saved, not in undo; its ghost already
    // mitres the first wall.
    assert_eq!(previews(&ed), 1);
    assert_eq!(ed.state.edit_undo_len(), undo_before);
    assert_eq!(scene::save_string(&ed.state.world).unwrap(), scene_before);
    assert!(
        matches!(wall_gen(&ed, a).end, WallEnd::Mitre { .. }),
        "{:?}",
        wall_gen(&ed, a).end
    );
    // Esc cancels the drawing (the tool stays); the first wall caps again.
    d.key(&mut ed, Key::Escape);
    d.frames(&mut ed, 2);
    assert_eq!(previews(&ed), 0);
    assert_eq!(ed.state.active_tool(), Some(WALL_TOOL));
    assert_eq!(wall_gen(&ed, a).end, WallEnd::Cap);
    assert_eq!(part_hash(&ed, a), Some(hash_before));

    // Draw it for real: Backspace removes a stray point first.
    let at = screen(&ed, near_end);
    d.click_at(&mut ed, at);
    let at = screen(&ed, xz(7.0, 2.0));
    d.click_at(&mut ed, at);
    d.key(&mut ed, Key::Backspace);
    draw(&mut ed, &mut d, &[corner]);
    assert_eq!(previews(&ed), 0);
    assert_eq!(ed.state.edit_undo_len(), undo_before + 1, "one undo step");
    let b = ed.state.selection[0];
    assert_ne!(a, b);
    let pb = world_points(&ed, b);
    assert_eq!(pb.len(), 2, "{pb:?}");
    assert!(pb[0].distance(pa[1]) < 1e-5, "snapped onto the end: {pb:?}");
    assert!(matches!(wall_gen(&ed, a).end, WallEnd::Mitre { .. }));
    assert!(matches!(wall_gen(&ed, b).start, WallEnd::Mitre { .. }));
    let saved = scene::save_string(&ed.state.world).unwrap();
    assert_eq!(saved.matches("\"WallPath\"").count(), 2);
    assert!(!saved.contains("Preview") && !saved.contains("WallGen"));

    // Undo: back to the single capped wall, byte for byte.
    ed.state.undo();
    d.frames(&mut ed, 3);
    assert_eq!(scene::save_string(&ed.state.world).unwrap(), scene_before);
    assert_eq!(wall_gen(&ed, a).end, WallEnd::Cap);
    assert_eq!(part_hash(&ed, a), Some(hash_before));
    ed.state.redo();
    d.frames(&mut ed, 3);
    assert!(matches!(wall_gen(&ed, a).end, WallEnd::Mitre { .. }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clicking_the_first_point_closes_a_room() {
    let dir = temp_project("close");
    let (mut ed, mut d) = ready(&dir);
    ed.state.activate_tool(WALL_TOOL);
    for p in [xz(0.0, 0.0), xz(4.0, 0.0), xz(4.0, -3.0)] {
        let at = screen(&ed, p);
        d.click_at(&mut ed, at);
    }
    let at = screen(&ed, xz(0.0, 0.0));
    d.click_at(&mut ed, at);
    d.frames(&mut ed, 2);
    let uid = by_name(&ed, "Wall");
    let pts = world_points(&ed, uid);
    assert_eq!(pts.len(), 4, "{pts:?}");
    assert_eq!(pts[0], pts[3], "closed: the first point repeated exactly");
    let g = wall_gen(&ed, uid);
    assert!(matches!(g.start, WallEnd::Mitre { .. }), "{:?}", g.start);
    assert!(matches!(g.end, WallEnd::Mitre { .. }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dragging_a_selected_walls_point_snaps_and_is_one_undo_step() {
    let dir = temp_project("handles");
    let (mut ed, mut d) = ready(&dir);
    ed.state.activate_tool(WALL_TOOL);
    draw(&mut ed, &mut d, &[xz(0.0, 0.0), xz(4.0, 0.0)]);
    let a = by_name(&ed, "Wall");
    draw(&mut ed, &mut d, &[xz(0.0, -3.0), xz(3.5, -2.5)]);
    let b = ed.state.selection[0];
    let undo = ed.state.edit_undo_len();
    let a_end = world_points(&ed, a)[1];
    // Drag b's end towards a's end: within 10 cm it snaps exactly.
    let from = screen(&ed, world_points(&ed, b)[1]);
    let to = screen(&ed, a_end + Vec3::new(0.06, 0.0, -0.05));
    d.frame(&mut ed, vec![Event::PointerMoved(from)]);
    d.frame(&mut ed, vec![button(from, true, Modifiers::NONE)]);
    for k in 1..=12 {
        let p = from + (to - from) * (k as f32 / 12.0);
        d.frame(&mut ed, vec![Event::PointerMoved(p)]);
    }
    d.frame(&mut ed, vec![button(to, false, Modifiers::NONE)]);
    d.frames(&mut ed, 2);
    assert_eq!(
        ed.state.edit_undo_len(),
        undo + 1,
        "the drag is one undo step"
    );
    let pb = world_points(&ed, b);
    assert!(pb[1].distance(a_end) < 1e-5, "{pb:?} vs {a_end}");
    assert!(matches!(wall_gen(&ed, a).end, WallEnd::Mitre { .. }));
    ed.state.undo();
    d.frames(&mut ed, 2);
    assert_eq!(wall_gen(&ed, a).end, WallEnd::Cap);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fence_drawn_up_to_a_wall_butts_its_face_without_a_post() {
    let dir = temp_project("fence");
    let (mut ed, mut d) = ready(&dir);
    ed.state.activate_tool(WALL_TOOL);
    draw(&mut ed, &mut d, &[xz(0.0, 1.0), xz(0.0, -4.0)]);
    ed.state.activate_tool(FENCE_TOOL);
    // 12 cm off the wall's centreline (inside its 0.15 + 0.10 reach).
    draw(&mut ed, &mut d, &[xz(0.12, -1.5), xz(5.0, -1.5)]);
    let f = ed.state.selection[0];
    assert!(ed.state.world.has::<FencePath>(entity(&ed, f)));
    let pts = world_points(&ed, f);
    assert!(pts[0].x.abs() < 1e-4, "snapped onto the wall: {pts:?}");
    let g = part_of::<FenceGen>(&ed, f).unwrap();
    assert!(!g.start_post && g.end_post);
    let g_start = entity(&ed, f);
    let gt = ed.state.world.get::<GlobalTransform>(g_start).unwrap().0;
    let start = gt.transform_point3(g.points[0]);
    assert!(
        (start.x - 0.15).abs() < 1e-3,
        "butts the face at x = 0.15: {start}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn overriding_a_part_field_is_one_undo_step_regenerates_only_that_part_and_saves() {
    let dir = temp_project("override");
    let (mut ed, mut d) = ready(&dir);
    ed.state.activate_tool(WALL_TOOL);
    draw(&mut ed, &mut d, &[xz(0.0, 0.0), xz(4.0, 0.0)]);
    let a = by_name(&ed, "Wall");
    draw(&mut ed, &mut d, &[xz(0.0, -3.0), xz(4.0, -3.0)]);
    let b = ed.state.selection[0];
    ed.state.cancel_tool();
    d.frames(&mut ed, 2);
    let (ha, hb) = (part_hash(&ed, a).unwrap(), part_hash(&ed, b).unwrap());
    let scene_before = scene::save_string(&ed.state.world).unwrap();
    let part = PartKey::new(a, plan::ROLE).uid();
    assert!(ed.state.inspect_part(part));
    let undo = ed.state.edit_undo_len();
    assert!(ed.state.set_part_override(part, "WallGen.height", &4.0f32));
    assert_eq!(ed.state.edit_undo_len(), undo + 1);
    assert_eq!(
        ed.state.part_override(part, "WallGen.height").as_deref(),
        Some("4.0")
    );
    d.frames(&mut ed, 3);
    assert_eq!(wall_gen(&ed, a).height, 4.0);
    assert_ne!(part_hash(&ed, a), Some(ha));
    assert_eq!(part_hash(&ed, b), Some(hb), "the other wall did not change");
    // The intent is untouched; the override is saved on the owner.
    assert_eq!(
        ed.state
            .world
            .get::<WallPath>(entity(&ed, a))
            .unwrap()
            .height,
        2.5
    );
    let saved = scene::save_string(&ed.state.world).unwrap();
    assert!(saved.contains("\"Overrides\"") && saved.contains("WallGen.height"));
    assert!(!saved.contains("\"WallGen\""), "no derived data saved");
    // A reload reproduces the overridden part.
    let reloaded = scene::load(saved.as_bytes(), ed.state.game().registry()).unwrap();
    let le = reloaded.entity_by_uid(a).unwrap();
    assert_eq!(
        reloaded
            .get::<Overrides>(le)
            .unwrap()
            .field(plan::RULE, plan::ROLE, "WallGen.height"),
        Some("4.0")
    );
    // Undo restores the scene and the hash; redo the override.
    ed.state.undo();
    d.frames(&mut ed, 3);
    assert_eq!(scene::save_string(&ed.state.world).unwrap(), scene_before);
    assert_eq!(part_hash(&ed, a), Some(ha));
    ed.state.redo();
    d.frames(&mut ed, 3);
    assert_eq!(wall_gen(&ed, a).height, 4.0);
    // Reset (the row's ↺) removes it: back to the rule's value.
    assert!(ed.state.reset_part_override(part, "WallGen.height"));
    d.frames(&mut ed, 3);
    assert_eq!(part_hash(&ed, a), Some(ha));
    assert!(!ed.state.world.has::<Overrides>(entity(&ed, a)));

    // Hide Part (the palette action): gone, owner selected; Show brings it back.
    assert!(ed.state.inspect_part(part));
    assert!(ed.state.run_action("edit.hide_part").is_ok());
    d.frames(&mut ed, 2);
    assert_eq!(ed.state.selection, vec![a]);
    assert!(part_of::<WallGen>(&ed, a).is_none());
    assert!(ed.state.show_part(a, plan::RULE, plan::ROLE));
    d.frames(&mut ed, 2);
    assert_eq!(part_hash(&ed, a), Some(ha));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_rule_list_shows_both_rules_and_switching_one_off_removes_its_parts() {
    let dir = temp_project("rules");
    let (mut ed, mut d) = ready(&dir);
    ed.state.activate_tool(WALL_TOOL);
    draw(&mut ed, &mut d, &[xz(0.0, 0.0), xz(4.0, 0.0)]);
    let a = by_name(&ed, "Wall");
    ed.state.cancel_tool();
    assert!(ed.state.run_action("view.rules").is_ok());
    assert!(ed.state.rules_panel_open());
    d.frames(&mut ed, 2);
    let engine_rules = Game::new().rules();
    let rows: Vec<_> = ed
        .state
        .rule_rows()
        .into_iter()
        .filter(|r| engine_rules.get(r.name).is_none())
        .collect();
    let names: Vec<(&str, &str, Vec<&str>)> = rows
        .iter()
        .map(|r| (r.name, r.intent, r.reads.clone()))
        .collect();
    assert_eq!(
        names,
        [
            (plan::RULE, "WallPath", vec![]),
            (fence::RULE, "FencePath", vec!["WallPath"])
        ]
    );
    assert_eq!((rows[0].intents, rows[0].parts), (1, 1));
    let h = part_hash(&ed, a).unwrap();
    ed.state.set_rule_enabled(plan::RULE, false);
    d.frames(&mut ed, 2);
    assert!(part_of::<WallGen>(&ed, a).is_none());
    let walls_row = |ed: &Editor| {
        ed.state
            .rule_rows()
            .into_iter()
            .find(|r| r.name == plan::RULE)
            .unwrap()
    };
    assert!(!walls_row(&ed).enabled);
    // Off survives a new world (a reopened scene), for this session.
    let saved = scene::save_string(&ed.state.world).unwrap();
    let world = scene::load(saved.as_bytes(), ed.state.game().registry()).unwrap();
    ed.state.replace_world(world);
    ed.state.world.insert_resource(ProcSettings {
        clock: ProcClock::Manual(Duration::from_secs(3600)),
        ..ProcSettings::default()
    });
    d.frames(&mut ed, 2);
    assert!(part_of::<WallGen>(&ed, a).is_none());
    ed.state.set_rule_enabled(plan::RULE, true);
    d.frames(&mut ed, 3);
    assert_eq!(part_hash(&ed, a), Some(h));

    // The panel's per-frame cost while open.
    let n = 120;
    let open: Duration = (0..n).map(|_| d.frame(&mut ed, Vec::new())).sum();
    ed.state.set_rules_panel(false);
    let closed: Duration = (0..n).map(|_| d.frame(&mut ed, Vec::new())).sum();
    println!(
        "rule list: editor frame {:.3} ms open vs {:.3} ms closed (mean of {n})",
        open.as_secs_f64() * 1000.0 / n as f64,
        closed.as_secs_f64() * 1000.0 / n as f64
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// 100 walls in a 10 × 10 grid of rooms' sides, spawned through the edit
/// stack.
fn hundred_walls(ed: &mut Editor) {
    let registry = ed.state.game().registry();
    let mut k = 0;
    for i in 0..10 {
        for j in 0..10 {
            let (x, z) = (i as f32 * 5.0, -(j as f32) * 5.0 - 6.0);
            let points = if (i + j) % 2 == 0 {
                vec![xz(x, z), xz(x + 4.0, z)]
            } else {
                vec![xz(x, z), xz(x, z - 4.0)]
            };
            let path = WallPath {
                points,
                ..WallPath::default()
            };
            let t = Transform::IDENTITY;
            ed.state
                .edits
                .spawn(
                    &mut ed.state.world,
                    Uid::new(),
                    &format!("Grid {k}"),
                    Placement::last(None),
                    &[("Transform", &t), ("WallPath", &path)],
                )
                .unwrap();
            k += 1;
        }
    }
    let _ = registry;
    ed.state.changed(true);
}

#[test]
fn ghost_preview_cost_per_mouse_move_in_a_100_wall_scene() {
    let dir = temp_project("cost");
    let (mut ed, mut d) = ready(&dir);
    hundred_walls(&mut ed);
    look_at(&mut ed, xz(22.0, -27.0), 60.0);
    d.frames(&mut ed, 4);
    assert_eq!(ed.state.world.count::<&WallPath>(), 100);
    ed.state.activate_tool(WALL_TOOL);
    let start = xz(1.0, -2.0);
    let at = screen(&ed, start);
    d.click_at(&mut ed, at);
    d.frames(&mut ed, 2);
    assert_eq!(
        previews(&ed),
        1,
        "drawing: at {at:?} in {:?}",
        ed.state.viewport.rect
    );
    // Sweep the cursor across the grid, one move per frame.
    let mut times = Vec::new();
    let runs0 = rule_runs(&ed, plan::RULE);
    let moves = 120;
    for k in 0..moves {
        let p = xz(2.0 + k as f32 * 0.3, -3.0 - k as f32 * 0.2);
        let at = screen(&ed, p);
        times.push(d.frame(&mut ed, vec![Event::PointerMoved(at)]));
    }
    let runs = rule_runs(&ed, plan::RULE) - runs0;
    // Still: no rule run, no regeneration, no preview write.
    d.frames(&mut ed, 3);
    let (r, g) = (rule_runs(&ed, plan::RULE), generations(&ed));
    let mut idle = Vec::new();
    for _ in 0..60 {
        idle.push(d.frame(&mut ed, Vec::new()));
    }
    assert_eq!(rule_runs(&ed, plan::RULE), r, "no rule run while still");
    assert_eq!(generations(&ed), g, "no regeneration while still");
    let ms = |v: &[Duration]| {
        let mean = v.iter().sum::<Duration>().as_secs_f64() * 1000.0 / v.len() as f64;
        let max = v.iter().max().unwrap().as_secs_f64() * 1000.0;
        (mean, max)
    };
    let (mean, max) = ms(&times);
    let (idle_mean, _) = ms(&idle);
    println!(
        "ghost preview, 100 walls: {moves} moves, {runs} rule runs; editor frame (UI + rules + \
         flush, no GPU) {mean:.2} ms mean / {max:.2} ms max per move; {idle_mean:.2} ms still"
    );
    assert!(runs >= moves as u64 / 2, "the preview followed the cursor");
    if !cfg!(debug_assertions) {
        assert!(mean < 16.7, "keeps 60 Hz: {mean:.2} ms");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn override_application_cost() {
    // A reconcile with an override (patched, compared with the rule's own
    // value) against one without, for 1,000 walls.
    let game = authoring::game();
    let mut w = game.new_world();
    w.insert_resource(game.rules());
    let mut uids = Vec::new();
    for i in 0..1000 {
        let path = WallPath {
            points: vec![xz(i as f32 * 5.0, 0.0), xz(i as f32 * 5.0 + 4.0, 0.0)],
            ..WallPath::default()
        };
        let e = w
            .spawn_saved(Uid::new(), "w", (Transform::IDENTITY, path))
            .unwrap();
        uids.push(w.uid(e).unwrap());
    }
    box_engine::box_core::transform::propagate_transforms(&mut w);
    box_engine::box_proc::rule::run_rules(&mut w);
    let run = |w: &mut World| {
        // Re-run the rule: one intent nudged and back (two passes).
        let e = w.entity_by_uid(uids[0]).unwrap();
        w.get_mut::<WallPath>(e).unwrap().height = 2.6;
        let t = Instant::now();
        box_engine::box_proc::rule::run_rules(w);
        let first = t.elapsed();
        w.get_mut::<WallPath>(e).unwrap().height = 2.5;
        box_engine::box_proc::rule::run_rules(w);
        first
    };
    let plain: Duration = (0..10).map(|_| run(&mut w)).sum::<Duration>() / 10;
    for u in &uids {
        let e = w.entity_by_uid(*u).unwrap();
        let mut o = Overrides::default();
        o.set_field(plan::RULE, plan::ROLE, "WallGen.height", "3.0");
        w.insert_one(e, o).unwrap();
    }
    let t = Instant::now();
    box_engine::box_proc::rule::run_rules(&mut w);
    let apply_all = t.elapsed();
    let patched: Duration = (0..10).map(|_| run(&mut w)).sum::<Duration>() / 10;
    let e = w.entity_by_uid(uids[500]).unwrap();
    let part = w
        .children(e)
        .into_iter()
        .find(|c| w.has::<WallGen>(*c))
        .unwrap();
    assert_eq!(w.get::<WallGen>(part).unwrap().height, 3.0);
    println!(
        "overrides, 1,000 walls: rule pass {:.2} ms without, {:.2} ms with one override on \
         every wall; applying all 1,000 at once {:.2} ms",
        plain.as_secs_f64() * 1000.0,
        patched.as_secs_f64() * 1000.0,
        apply_all.as_secs_f64() * 1000.0
    );
}

// ----- GPU: captures -----

/// Captures for review in `target/captures/m6_tools/`: the wall tool
/// mid-draw (ghost and snap indicator), the committed wall connected to its
/// neighbour, the inspector with an override (marked, with Reset), the rule
/// list, and a fence meeting a wall. Every frame is checked for wgpu
/// validation errors.
#[test]
fn m6_tool_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = authoring::project_dir().join("../../target/captures/m6_tools");
    std::fs::create_dir_all(&out).unwrap();
    let dir = temp_project("captures");
    let mut ed = open(&dir);
    let options = CaptureOptions {
        size: [2560, 1600],
        pixels_per_point: 2.0,
        frames: 1,
    };
    let mut h = match Headless::new(&mut ed, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<box_engine::box_render::RenderError>(),
                Some(box_engine::box_render::RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    let frame = |h: &mut Headless, ed: &mut Editor, events: Vec<Event>| {
        advance_clock(ed);
        h.frame(ed, events);
    };
    let save = |h: &Headless, ed: &Editor, name: &str| {
        assert_eq!(
            h.renderer().validation_errors(),
            0,
            "{name}: wgpu validation errors"
        );
        let path = out.join(name);
        h.image().unwrap().save(&path).unwrap();
        eprintln!("{}", path.display());
        let _ = ed;
    };
    look_at(&mut ed, xz(0.5, -1.5), 15.0);
    ed.state.camera.orbit.pitch = 0.8;
    for _ in 0..3 {
        frame(&mut h, &mut ed, Vec::new());
    }
    let click = |h: &mut Headless, ed: &mut Editor, p: Vec3| {
        let at = screen(ed, p);
        for e in click_events(at, Modifiers::NONE) {
            advance_clock(ed);
            h.frame(ed, e);
        }
    };
    let enter = |h: &mut Headless, ed: &mut Editor| {
        for e in key_events(Key::Enter, Modifiers::NONE) {
            advance_clock(ed);
            h.frame(ed, e);
        }
    };
    ed.state.activate_tool(WALL_TOOL);
    // A wall along x, then the second one being drawn from its end.
    click(&mut h, &mut ed, xz(-2.0, 0.0));
    click(&mut h, &mut ed, xz(3.0, 0.0));
    enter(&mut h, &mut ed);
    let a = by_name(&ed, "Wall");
    let a_end = world_points(&ed, a)[1];
    click(&mut h, &mut ed, a_end + Vec3::new(0.04, 0.0, -0.06));
    click(&mut h, &mut ed, xz(3.0, -3.0));
    // Hover near the first wall's start: the snap indicator shows.
    let a_start = world_points(&ed, a)[0];
    let at = screen(&ed, a_start + Vec3::new(0.07, 0.0, -0.05));
    for _ in 0..3 {
        frame(&mut h, &mut ed, vec![Event::PointerMoved(at)]);
    }
    save(&h, &ed, "path_tool_mid_draw.png");
    // Commit (Enter: the placed points): an L, mitred at the shared corner.
    enter(&mut h, &mut ed);
    ed.state.cancel_tool();
    look_at(&mut ed, xz(1.5, -1.0), 10.0);
    ed.state.camera.orbit.yaw = 0.7;
    ed.state.camera.orbit.pitch = 0.5;
    for _ in 0..3 {
        frame(&mut h, &mut ed, Vec::new());
    }
    let b = ed.state.selection[0];
    assert!(matches!(wall_gen(&ed, a).end, WallEnd::Mitre { .. }));
    save(&h, &ed, "wall_connected.png");

    // Inspect the first wall's part and override its height and colour.
    let part = PartKey::new(a, plan::ROLE).uid();
    assert!(ed.state.inspect_part(part));
    assert!(ed.state.set_part_override(part, "WallGen.height", &3.4f32));
    assert!(ed.state.set_part_override(
        part,
        "StandardMaterial.base_color",
        &box_engine::box_core::color::Color::hex(0xb5643c)
    ));
    for _ in 0..4 {
        frame(&mut h, &mut ed, Vec::new());
    }
    assert_eq!(wall_gen(&ed, a).height, 3.4);
    save(&h, &ed, "inspector_override.png");

    // The rule list.
    ed.state.select(Some(b));
    ed.state.set_rules_panel(true);
    for _ in 0..3 {
        frame(&mut h, &mut ed, Vec::new());
    }
    save(&h, &ed, "rule_list.png");
    ed.state.set_rules_panel(false);

    // A fence drawn up to the second wall's face.
    ed.state.activate_tool(FENCE_TOOL);
    let wb = world_points(&ed, b);
    let mid = (wb[0] + wb[1]) * 0.5;
    click(&mut h, &mut ed, mid + Vec3::new(0.1, 0.0, 0.0));
    click(&mut h, &mut ed, mid + Vec3::new(2.8, 0.0, -0.6));
    enter(&mut h, &mut ed);
    ed.state.cancel_tool();
    let f = ed.state.selection[0];
    look_at(&mut ed, mid + Vec3::new(2.0, 0.4, -0.3), 6.0);
    ed.state.camera.orbit.yaw = 0.9;
    ed.state.camera.orbit.pitch = 0.35;
    ed.state.select(None);
    for _ in 0..4 {
        frame(&mut h, &mut ed, Vec::new());
    }
    let g = part_of::<FenceGen>(&ed, f).unwrap();
    assert!(!g.start_post, "the fence butts the wall");
    assert!(
        world_points(&ed, f).iter().all(|p| p.y.abs() < 1e-4),
        "on the ground"
    );
    save(&h, &ed, "fence_meets_wall.png");
    let _ = std::fs::remove_dir_all(dir);
}
