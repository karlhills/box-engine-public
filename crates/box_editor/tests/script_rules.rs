//! Script rules in the stock editor (`examples/script_rules`, a data
//! project): Luau rule files build fences and a colonnade from
//! `ScriptIntent`s in the edit world; property and point edits re-run the
//! rule and undo restores the scene byte for byte; the rule list lists and
//! switches script rules; the Draw Path tool draws a new intent copying the
//! selected one; Play runs the rules in the play world, and editing a rule
//! file during Play re-runs it there and, after Stop, in the edit world.
//!
//! The last test needs a GPU: captures in `target/captures/script_rules/`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_core::derived::Derived;
use box_core::identity::Uid;
use box_core::prelude::*;
use box_core::scene;
use box_editor::log::LogBuffer;
use box_editor::{Editor, EditorOptions, build_editor, layout};
use box_engine::box_proc::derive::parts_of;
use box_engine::box_script::{ScriptIntent, ScriptProperty, ScriptValue};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};

const FENCE: &str = "rules/fence.luau";
const COLONNADE: &str = "rules/colonnade.luau";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".box" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/script_rules")
}

/// A copy of the example (rule files are edited) opened in an editor.
fn open(name: &str) -> (Editor, TempDir) {
    let dir = std::env::temp_dir().join(format!(
        "box_editor_script_rules_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&example(), &dir);
    let options = EditorOptions {
        project: Some(dir.clone()),
        no_restore: true,
        ..Default::default()
    };
    let mut ed = build_editor(&options, LogBuffer::default()).unwrap();
    ed.state.autosave_dir = dir.join(".box/autosave");
    ed.tree = layout::default_layout();
    (ed, TempDir(dir))
}

/// Editor frames (assets, rules, generators) until `done`, at most 5 s.
fn until(ed: &mut Editor, mut done: impl FnMut(&World) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        ed.state.update_assets();
        if ed.state.is_playing() {
            ed.state.update_play(1.0 / 60.0).unwrap();
        }
        ed.prepare_render();
        if done(&ed.state.world) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

fn owner(w: &World, name: &str) -> Entity {
    w.find_by_name(name).unwrap_or_else(|| panic!("{name}"))
}

fn roles(w: &World, name: &str) -> Vec<String> {
    let mut r: Vec<String> = parts_of(w, owner(w, name))
        .into_iter()
        .map(|(_, d)| d.key.role().to_owned())
        .collect();
    r.sort();
    r
}

fn built(w: &World) -> bool {
    ["Fence West", "Fence East", "Colonnade"].iter().all(|n| {
        w.find_by_name(n)
            .is_some_and(|e| !parts_of(w, e).is_empty())
    })
}

fn uid_of(ed: &Editor, name: &str) -> Uid {
    ed.state.world.uid(owner(&ed.state.world, name)).unwrap()
}

#[test]
fn script_rules_build_edit_undo_and_switch_in_the_edit_world() {
    let (mut ed, _dir) = open("edit");
    assert!(
        until(&mut ed, built),
        "the rules built every intent's parts"
    );
    let w = &ed.state.world;
    // Fence West: 3 corners (its last one is shared with Fence East, which
    // has the higher uid), 2 + 1 bay posts, 2 rails.
    assert_eq!(
        roles(w, "Fence West"),
        [
            "bay1/post1",
            "bay1/post2",
            "bay2/post1",
            "corner1",
            "corner2",
            "corner3",
            "rail1",
            "rail2"
        ]
    );
    assert!(
        !roles(w, "Fence East").contains(&"corner1".to_owned()),
        "shared post"
    );
    assert!(
        roles(w, "Fence East").contains(&"rail3".to_owned()),
        "rails = 3"
    );
    // Parts are never saved.
    let saved = scene::save_string(w).unwrap();
    assert!(!saved.contains("corner1") && !saved.contains("Derived"));

    // The rule list lists both script rules.
    let rows = ed.state.rule_rows();
    let row = |name: &str| rows.iter().find(|r| r.name == name).unwrap().clone();
    assert_eq!((row(FENCE).intent, row(FENCE).intents), ("ScriptIntent", 2));
    assert_eq!(row(COLONNADE).intents, 1);
    assert!(row(FENCE).parts > 10 && row(FENCE).last_run.is_some());
    // The Draw Path tool appeared with the first script rule.
    assert!(ed.state.tools().iter().any(|t| t.id == "draw_path"));

    // A point edit re-runs the rule; undo restores the scene byte for byte.
    let west = uid_of(&ed, "Fence West");
    assert!(
        ed.state
            .set_field(west, "ScriptIntent.points[2]", &Vec3::new(8.0, 0.0, 5.0))
    );
    ed.state.finish_edit();
    assert!(until(&mut ed, |w| roles(w, "Fence West")
        .contains(&"bay2/post3".to_owned())));
    ed.state.undo_scene();
    assert!(until(&mut ed, |w| !roles(w, "Fence West")
        .contains(&"bay2/post3".to_owned())));
    assert_eq!(scene::save_string(&ed.state.world).unwrap(), saved);

    // A property edit (what the inspector writes) re-runs it too.
    let props = vec![ScriptProperty {
        name: "rails".into(),
        value: ScriptValue::Integer(4),
    }];
    assert!(ed.state.set_field(west, "ScriptIntent.properties", &props));
    ed.state.finish_edit();
    assert!(until(&mut ed, |w| roles(w, "Fence West")
        .contains(&"rail4".to_owned())));
    ed.state.undo_scene();
    assert!(until(&mut ed, |w| !roles(w, "Fence West")
        .contains(&"rail4".to_owned())));
    assert_eq!(scene::save_string(&ed.state.world).unwrap(), saved);

    // Switching the fence rule off removes its parts only; on brings them
    // back with the same uids.
    let uids = |w: &World| -> Vec<Uid> {
        let mut u: Vec<Uid> = parts_of(w, owner(w, "Fence West"))
            .into_iter()
            .map(|(e, _)| w.uid(e).unwrap())
            .collect();
        u.sort();
        u
    };
    let before = uids(&ed.state.world);
    ed.state.set_rule_enabled(FENCE, false);
    assert!(until(&mut ed, |w| roles(w, "Fence West").is_empty()));
    assert!(!roles(&ed.state.world, "Colonnade").is_empty());
    ed.state.set_rule_enabled(FENCE, true);
    assert!(until(&mut ed, |w| !roles(w, "Fence West").is_empty()));
    assert_eq!(uids(&ed.state.world), before);
}

#[test]
fn editing_a_rule_file_during_play_reruns_it_in_play_and_after_stop() {
    let (mut ed, dir) = open("play");
    assert!(until(&mut ed, built));
    ed.state.start_play().unwrap();
    assert!(until(&mut ed, built), "the play world builds the parts too");
    assert!(ed.state.is_playing());
    let play_parts = ed.state.world.query::<&Derived>().iter().count();
    assert!(play_parts > 20, "{play_parts}");
    assert!(!roles(&ed.state.world, "Fence West").contains(&"rail4".to_owned()));
    // Edit the rule file: Fence West uses the default rail count.
    let path = dir.0.join("assets").join(FENCE);
    let text = std::fs::read_to_string(&path).unwrap();
    let edited = text.replace(
        "rails = { type = \"integer\", default = 2",
        "rails = { type = \"integer\", default = 4",
    );
    assert_ne!(text, edited);
    std::fs::write(&path, edited).unwrap();
    let server = ed.state.asset_server().cloned().unwrap();
    server.reimport(server.id_of(FENCE).unwrap()).unwrap();
    assert!(
        until(&mut ed, |w| roles(w, "Fence West")
            .contains(&"rail4".to_owned())),
        "re-ran in Play"
    );
    assert!(ed.state.is_playing());
    ed.state.stop_play();
    assert!(
        until(&mut ed, |w| roles(w, "Fence West")
            .contains(&"rail4".to_owned())),
        "re-ran in the edit world"
    );
}

// ----- the Draw Path tool, driven by egui events -----

struct Driver {
    ctx: egui::Context,
    time: f64,
}

const SCREEN: egui::Vec2 = vec2(1280.0, 800.0);

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        Self { ctx, time: 0.0 }
    }

    fn frame(&mut self, editor: &mut Editor, mut events: Vec<Event>) {
        events.insert(0, Event::ModifiersChanged(Modifiers::NONE));
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        editor.state.update_assets();
        let _ = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        editor.prepare_render();
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    fn click(&mut self, editor: &mut Editor, p: Vec3) {
        let s = editor.state.gizmo_view().unwrap().project(p).unwrap();
        let pos = editor.state.viewport.rect.unwrap().min + vec2(s.x, s.y);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        for events in [
            vec![Event::PointerMoved(pos)],
            vec![button(true)],
            vec![button(false)],
            Vec::new(),
        ] {
            self.frame(editor, events);
        }
    }

    fn key(&mut self, editor: &mut Editor, key: Key) {
        for pressed in [true, false] {
            self.frame(
                editor,
                vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
            );
        }
    }
}

fn look_down(ed: &mut Editor) {
    let orbit = &mut ed.state.camera.orbit;
    orbit.target = Vec3::new(0.0, 0.0, 0.0);
    orbit.distance = 16.0;
    orbit.yaw = 0.0;
    orbit.pitch = 1.0;
}

#[test]
fn draw_path_copies_the_selected_intent_in_one_undo_step() {
    let (mut ed, _dir) = open("draw");
    let mut d = Driver::new(&ed);
    look_down(&mut ed);
    d.frames(&mut ed, 3);
    assert!(until(&mut ed, built));
    d.frames(&mut ed, 2);
    // Nothing selected: the tool waits for a path to copy.
    ed.state.select(None);
    assert!(ed.state.activate_tool("draw_path"));
    d.frames(&mut ed, 1);
    let undo = ed.state.edit_undo_len();
    let east = uid_of(&ed, "Fence East");
    ed.state.select(Some(east));
    d.frames(&mut ed, 1);
    d.click(&mut ed, Vec3::new(-5.0, 0.0, 5.0));
    d.click(&mut ed, Vec3::new(2.0, 0.0, 6.0));
    d.key(&mut ed, Key::Enter);
    d.frames(&mut ed, 3);
    assert_eq!(ed.state.edit_undo_len(), undo + 1, "one undo step");
    let new = ed.state.selection[0];
    assert_ne!(new, east);
    let w = &ed.state.world;
    let e = w.entity_by_uid(new).unwrap();
    let intent = (*w.get::<ScriptIntent>(e).unwrap()).clone();
    let source = (*w.get::<ScriptIntent>(owner(w, "Fence East")).unwrap()).clone();
    assert_eq!(intent.rule, source.rule);
    assert_eq!(intent.properties, source.properties, "settings copied");
    assert_eq!(intent.points.len(), 2);
    assert!(
        (intent.points[1] - Vec3::new(7.0, 0.0, 1.0)).length() < 0.05,
        "{:?}",
        intent.points
    );
    let name = w.get::<Name>(e).unwrap().as_str().to_owned();
    assert!(until(&mut ed, |w| roles(w, &name).contains(&"rail3".to_owned())));
    ed.state.cancel_tool();
    ed.state.undo_scene();
    d.frames(&mut ed, 2);
    assert!(ed.state.world.entity_by_uid(new).is_none());
}

// ----- GPU: captures -----

/// `target/captures/script_rules/`: the example in the editor with a fence
/// selected (its Script Intent properties in the inspector) and the rule
/// list open; the Draw Path tool mid-draw (the ghost is the rule's real
/// output). Every frame is checked for wgpu validation errors.
#[test]
fn script_rules_editor_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/script_rules");
    std::fs::create_dir_all(&out).unwrap();
    let (mut ed, _dir) = open("captures");
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
    let save = |h: &Headless, name: &str| {
        assert_eq!(
            h.renderer().validation_errors(),
            0,
            "{name}: wgpu validation errors"
        );
        let path = out.join(name);
        h.image().unwrap().save(&path).unwrap();
        eprintln!("{}", path.display());
    };
    ed.state.wait_assets_idle(Duration::from_secs(10));
    for _ in 0..4 {
        h.frame(&mut ed, Vec::new());
        ed.state.wait_assets_idle(Duration::from_secs(5));
    }
    assert!(built(&ed.state.world));
    {
        let orbit = &mut ed.state.camera.orbit;
        orbit.target = Vec3::new(0.0, 0.8, 0.0);
        orbit.distance = 13.0;
        orbit.yaw = 0.55;
        orbit.pitch = 0.55;
    }
    let east = uid_of(&ed, "Fence East");
    ed.state.select(Some(east));
    ed.state.set_rules_panel(true);
    for _ in 0..4 {
        h.frame(&mut ed, Vec::new());
    }
    save(&h, "editor_rules.png");
    ed.state.set_rules_panel(false);

    // Draw Path: a new fence like Fence East, mid-draw.
    assert!(ed.state.activate_tool("draw_path"));
    let click = |h: &mut Headless, ed: &mut Editor, p: Vec3| {
        let s = ed.state.gizmo_view().unwrap().project(p).unwrap();
        let pos = ed.state.viewport.rect.unwrap().min + vec2(s.x, s.y);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        for events in [
            vec![Event::PointerMoved(pos)],
            vec![button(true)],
            vec![button(false)],
            Vec::new(),
        ] {
            h.frame(ed, events);
        }
    };
    click(&mut h, &mut ed, Vec3::new(-3.0, 0.0, 4.6));
    click(&mut h, &mut ed, Vec3::new(1.0, 0.0, 5.2));
    let s = ed
        .state
        .gizmo_view()
        .unwrap()
        .project(Vec3::new(2.5, 0.0, 3.9))
        .unwrap();
    let pos = ed.state.viewport.rect.unwrap().min + vec2(s.x, s.y);
    for _ in 0..3 {
        h.frame(&mut ed, vec![Event::PointerMoved(pos)]);
    }
    // The ghost follows the cursor: a preview with the copied intent.
    let ghost: Vec<Vec<Vec3>> = ed
        .state
        .world
        .query::<(&box_core::derived::Preview, &ScriptIntent)>()
        .iter()
        .map(|(_, i)| i.points.clone())
        .collect();
    assert_eq!(ghost.len(), 1);
    assert_eq!(ghost[0].len(), 3, "{ghost:?}");
    save(&h, "editor_draw_path.png");
}
