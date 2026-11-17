//! Editor usability fixes from Milestone 7: during Play the editor's
//! status leaves the game view (the game's own UI stays fully visible),
//! small numbers are never shown as 0.00, Add has Spot Light, Ambient
//! Volume and Particle Emitter (each with its own icon), and particle
//! emitters are previewed while editing without touching the scene.
//!
//! Headless and driven by egui events; widgets are found in egui's
//! accessibility tree. The last test writes captures to
//! `target/captures/m7_editor/` and needs a GPU.

use std::path::{Path, PathBuf};
use std::time::Duration;

use box_core::identity::{Name, Uid};
use box_core::prelude::{Entity, Transform, World};
use box_editor::icons::{IconKind, icon_entities};
use box_editor::log::LogBuffer;
use box_editor::panes::hierarchy::kind_of;
use box_editor::particle_preview::{ParticlePreview, RESTART_PAUSE};
use box_editor::widgets::{EntityKind, decimals_for, number_text};
use box_editor::{Editor, EditorOptions, NewEntity, build_editor};
use box_engine::box_ui::Ui;
use box_render::{AmbientVolume, ParticleEmitter, ParticleState, SpotLight};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use glam::Vec3;

// ----- an egui driver that reads the accessibility tree -----

#[derive(Clone, Debug)]
struct Node {
    role: Role,
    text: String,
    rect: Rect,
}

fn nodes_of(update: Option<&TreeUpdate>) -> Vec<Node> {
    let Some(u) = update else {
        return Vec::new();
    };
    u.nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            let text = n.label().or(n.value()).unwrap_or_default().to_string();
            Some(Node {
                role: n.role(),
                text,
                rect: Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ),
            })
        })
        .collect()
}

struct Driver {
    ctx: egui::Context,
    time: f64,
    nodes: Vec<Node>,
    screen: egui::Vec2,
}

impl Driver {
    fn new(editor: &Editor, screen: egui::Vec2) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        ctx.enable_accesskit();
        Self {
            ctx,
            time: 0.0,
            nodes: Vec::new(),
            screen,
        }
    }

    fn frame(&mut self, editor: &mut Editor, events: Vec<Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.screen)),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let out = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        editor.prepare_render();
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    fn find(&self, text: &str) -> Option<Node> {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.text == text && n.role != Role::TextInput)
            .cloned()
    }

    /// Runs frames until a widget with `text` exists (or 10 s pass), for UI
    /// that waits on a background import, such as a script's properties.
    fn wait_for(&mut self, editor: &mut Editor, text: &str) -> Node {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.find(text).is_none() && std::time::Instant::now() < deadline {
            editor.state.update_assets();
            self.frame(editor, Vec::new());
            std::thread::sleep(Duration::from_millis(2));
        }
        self.get(text)
    }

    fn get(&self, text: &str) -> Node {
        self.find(text).unwrap_or_else(|| {
            let all: Vec<&str> = self.nodes.iter().map(|n| n.text.as_str()).collect();
            panic!("no widget `{text}` among {all:?}")
        })
    }

    fn press(&mut self, editor: &mut Editor, pos: Pos2, button: PointerButton) {
        let b = |pressed| Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![b(true)]);
        self.frame(editor, vec![b(false)]);
        self.frames(editor, 2);
    }

    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self.get(text).rect.center();
        self.press(editor, pos, PointerButton::Primary);
    }

    fn key(&mut self, editor: &mut Editor, key: Key, modifiers: Modifiers) {
        let e = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        self.frame(editor, vec![e(true)]);
        self.frame(editor, vec![e(false)]);
        self.frame(editor, Vec::new());
    }
}

// ----- projects -----

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_project(name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("box_m7ux_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    box_engine::Project::create(&dir, name).unwrap();
    TempDir(dir)
}

/// Copies a project (without its local `.box` state).
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

/// A copy of the Luau collector (`examples/collector_luau`).
fn collector(name: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("box_m7ux_{name}_{}", Uid::new()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/collector_luau"),
        &dir,
    );
    TempDir(dir)
}

fn open(dir: &Path) -> Editor {
    let options = EditorOptions {
        project: Some(dir.to_path_buf()),
        no_restore: true,
        ..Default::default()
    };
    let mut editor = build_editor(&options, LogBuffer::default()).unwrap();
    editor.state.autosave_dir = dir.join(".box/autosave");
    editor.tree = box_editor::layout::default_layout();
    editor
}

/// The default window size (`box-editor` opens at 1440 × 900 points).
const DEFAULT: egui::Vec2 = vec2(1440.0, 900.0);

fn by_name(world: &World, name: &str) -> Uid {
    world
        .query::<(&Uid, &Name)>()
        .iter()
        .find(|(_, n)| n.0 == name)
        .map(|(u, _)| *u)
        .unwrap_or_else(|| panic!("no entity `{name}`"))
}

fn entity(editor: &Editor, uid: Uid) -> Entity {
    editor.state.world.entity_by_uid(uid).unwrap()
}

/// Imports the project's assets (scripts declare their properties).
fn settle_assets(editor: &mut Editor) {
    for _ in 0..200 {
        editor.state.update_assets();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Runs Play frames until the collector's HUD exists.
fn play_until_hud(editor: &mut Editor) {
    editor.state.start_play().unwrap();
    for _ in 0..600 {
        editor.state.update_assets();
        editor.state.update_play(1.0 / 60.0).unwrap();
        if editor
            .state
            .world
            .get_resource::<Ui>()
            .is_some_and(|ui| ui.find(None, "score").is_some())
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the collector's HUD never appeared");
}

// ----- the game's UI is not covered in Play -----

#[test]
fn play_status_moves_from_the_game_view_to_the_tab_bar() {
    let dir = collector("play");
    let mut ed = open(&dir.0);
    settle_assets(&mut ed);
    let mut d = Driver::new(&ed, DEFAULT);
    let camera = by_name(&ed.state.world, "Camera");
    ed.state.select(Some(camera));
    d.frames(&mut ed, 3);
    let image = ed.state.viewport.rect.expect("the viewport is shown");
    // Editing: the view and the selection are chips over the image.
    let selection = d.get("Camera  ·  1 selected");
    assert!(image.contains_rect(selection.rect), "{selection:?}");
    assert!(
        d.nodes
            .iter()
            .any(|n| n.text.starts_with("Perspective") && image.contains_rect(n.rect))
    );

    play_until_hud(&mut ed);
    ed.state.select(Some(camera));
    d.frames(&mut ed, 3);
    assert!(ed.state.shows_game_camera());
    let image = ed.state.viewport.rect.unwrap();
    // No editor chip over the game's image: nothing accessible inside it.
    let over: Vec<&Node> = d
        .nodes
        .iter()
        .filter(|n| n.rect.intersects(image.shrink(2.0)) && n.role != Role::Window)
        .filter(|n| !n.text.is_empty())
        .collect();
    assert!(over.is_empty(), "covering the game: {over:#?}");
    // The status is in the Viewport tab bar, above the image.
    // Play starts with the game holding input.
    for text in [
        "Game Camera",
        "Playing  ·  Esc gives input back to the editor",
        "Camera  ·  1 selected",
    ] {
        let n = d.get(text);
        assert!(
            n.rect.bottom() <= image.top() + 0.5,
            "{text}: {n:?} vs {image:?}"
        );
        assert!(n.rect.left() >= image.left(), "{text}: {n:?}");
    }
    // Focus and pause change the hint there.
    ed.state.set_game_focus(false);
    d.frames(&mut ed, 2);
    d.get("Playing  ·  Click to give the game input");
    ed.state.toggle_pause();
    d.frames(&mut ed, 2);
    d.get("Paused");
    // Free camera: the editor's view again, with its chips; the hint stays
    // in the tab bar.
    ed.state.toggle_pause();
    ed.state.set_free_camera(true);
    d.frames(&mut ed, 3);
    let hint = d.get("Playing  ·  Free camera");
    assert!(hint.rect.bottom() <= image.top() + 0.5);
    assert!(image.contains_rect(d.get("Camera  ·  1 selected").rect));
    ed.state.stop_play();
}

// ----- small numbers -----

#[test]
fn small_numbers_keep_their_digits() {
    // Two significant digits below 1, never fewer decimals than the base.
    assert_eq!(number_text(0.004, 2), "0.004");
    assert_eq!(number_text(0.004, 4), "0.0040");
    assert_eq!(number_text(0.0045, 2), "0.0045");
    assert_eq!(number_text(0.25, 2), "0.25");
    assert_eq!(number_text(1.5, 2), "1.50");
    assert_eq!(number_text(0.0, 2), "0.00");
    assert_eq!(number_text(-0.0123, 2), "-0.012");
    assert_eq!(number_text(4e-7, 2), "4.0e-7");
    assert_eq!(number_text(12.0, 0), "12");
    assert_eq!(decimals_for(0.004, 2), 4);
    assert_eq!(decimals_for(3.0, 2), 2);
    // No non-zero value reads as zero.
    for v in [1e-9, 3e-7, 2e-6, 0.00049, 0.004, 0.049, 0.5] {
        let s = number_text(v, 2);
        assert!(s.parse::<f64>().unwrap() != 0.0, "{v} -> {s}");
    }
}

#[test]
fn script_properties_and_fields_show_small_values() {
    let dir = collector("small");
    let mut ed = open(&dir.0);
    settle_assets(&mut ed);
    let mut d = Driver::new(&ed, DEFAULT);
    // The camera script's mouse_sensitivity: 0.004 with step 0.0005
    // (4 decimals from the step).
    ed.state.select(Some(by_name(&ed.state.world, "Camera")));
    // Showing the Script component requests the script's import.
    let n = d.wait_for(&mut ed, "0.0040");
    assert_eq!(n.role, Role::Slider);

    // A reflected field with a small range step: an emitter's soft
    // distance (step 0.01) of 0.004 m.
    let uid = ed
        .state
        .add_entity(NewEntity::ParticleEmitter, None)
        .unwrap();
    let soft = 0.004f32;
    assert!(
        ed.state
            .set_field(uid, "ParticleEmitter.soft_distance", &soft)
    );
    d.frames(&mut ed, 4);
    d.get("0.004 m");
    // A drag field (intensity, 0..1000): typing keeps every digit.
    let intensity = 0.00037f32;
    assert!(
        ed.state
            .set_field(uid, "ParticleEmitter.intensity", &intensity)
    );
    d.frames(&mut ed, 3);
    let field = d
        .nodes
        .iter()
        .find(|n| n.role == Role::SpinButton && n.text.starts_with("0.00037"))
        .cloned()
        .unwrap_or_else(|| {
            let spins: Vec<&Node> = d
                .nodes
                .iter()
                .filter(|n| n.role == Role::SpinButton)
                .collect();
            panic!("no 0.00037 drag field among {spins:#?}")
        });
    d.press(&mut ed, field.rect.center(), PointerButton::Primary);
    d.key(&mut ed, Key::A, Modifiers::COMMAND);
    d.frame(&mut ed, vec![Event::Text("0.000123456".into())]);
    d.key(&mut ed, Key::Enter, Modifiers::NONE);
    let e = entity(&ed, uid);
    let typed = ed.state.world.get::<ParticleEmitter>(e).unwrap().intensity;
    assert_eq!(typed, 0.000123456f32);
}

// ----- Add ▸ Spot Light, Ambient Volume, Particle Emitter -----

#[test]
fn add_menu_has_spot_light_ambient_volume_and_particle_emitter() {
    let dir = temp_project("add");
    let mut ed = open(&dir.0);
    let mut d = Driver::new(&ed, DEFAULT);
    d.frames(&mut ed, 3);
    for (label, kind) in [
        ("Spot Light", EntityKind::SpotLight),
        ("Ambient Volume", EntityKind::AmbientVolume),
        ("Particle Emitter", EntityKind::Particles),
    ] {
        let steps = ed.state.edits.undo_len();
        d.click(&mut ed, "+ Add");
        d.click(&mut ed, label);
        assert_eq!(
            ed.state.edits.undo_len(),
            steps + 1,
            "{label}: one undo step"
        );
        let uid = *ed
            .state
            .selection
            .last()
            .expect("the new entity is selected");
        let e = entity(&ed, uid);
        let w = &ed.state.world;
        assert_eq!(w.get::<Name>(e).unwrap().0, label);
        assert_eq!(kind_of(w, e), kind, "{label}");
        let t = *w.get::<Transform>(e).unwrap();
        match kind {
            EntityKind::SpotLight => {
                assert!(w.has::<SpotLight>(e));
                // Shines straight down from 3 m above the ground.
                assert!((t.translation.y - 3.0).abs() < 1e-3, "{t:?}");
                assert!((t.rotation * Vec3::NEG_Z).abs_diff_eq(Vec3::NEG_Y, 1e-5));
            }
            EntityKind::AmbientVolume => {
                let v = *w.get::<AmbientVolume>(e).unwrap();
                // Standing on the ground.
                assert!((t.translation.y - v.half_extents.y).abs() < 1e-3, "{t:?}");
            }
            _ => {
                assert!(w.has::<ParticleEmitter>(e));
                assert!(t.translation.y.abs() < 1e-3, "{t:?}");
            }
        }
        let icon = icon_entities(w)
            .into_iter()
            .find(|(_, u, ..)| *u == uid)
            .map(|(_, _, k, _)| k);
        let expected = match kind {
            EntityKind::SpotLight => IconKind::SpotLight,
            EntityKind::AmbientVolume => IconKind::AmbientVolume,
            _ => IconKind::Particles,
        };
        assert_eq!(icon, Some(expected), "{label}");
        // One undo removes it.
        ed.state.undo();
        assert!(ed.state.world.entity_by_uid(uid).is_none());
        ed.state.redo();
    }
    // The palette knows them too.
    let ids: Vec<String> = box_editor::actions::context_actions(&mut ed.state)
        .iter()
        .map(|a| a.id().to_owned())
        .collect();
    for id in [
        "add.spot_light",
        "add.ambient_volume",
        "add.particle_emitter",
    ] {
        assert!(ids.iter().any(|i| i == id), "{id} in {ids:?}");
    }
}

// ----- particle preview -----

fn particles(ed: &Editor, uid: Uid) -> Option<usize> {
    let e = entity(ed, uid);
    ed.state.world.get::<ParticleState>(e).map(|s| s.len())
}

#[test]
fn particles_preview_while_editing_without_touching_the_scene() {
    let dir = temp_project("preview");
    let mut ed = open(&dir.0);
    let mut d = Driver::new(&ed, DEFAULT);
    d.frames(&mut ed, 2);
    let uid = ed
        .state
        .add_entity(NewEntity::ParticleEmitter, None)
        .unwrap();
    let saved = box_core::scene::save(&ed.state.world).unwrap();
    let (steps, dirty, generation) = (
        ed.state.edits.undo_len(),
        ed.state.is_dirty(),
        ed.state.generation,
    );
    // Selected (the default): it simulates.
    assert_eq!(ed.state.viewport.view.particles, ParticlePreview::Selected);
    d.frames(&mut ed, 60);
    let n = particles(&ed, uid).expect("previewed");
    assert!(n >= 15, "{n} particles after 1 s at 20/s");
    // Nothing saved, no undo step, not dirtier, no world change recorded.
    assert_eq!(box_core::scene::save(&ed.state.world).unwrap(), saved);
    assert_eq!(ed.state.edits.undo_len(), steps);
    assert_eq!(ed.state.is_dirty(), dirty);
    assert_eq!(ed.state.generation, generation);
    // Deselected: its particles go.
    ed.state.select(None);
    d.frames(&mut ed, 2);
    assert_eq!(particles(&ed, uid), None);
    // All: unselected emitters too; None: no preview.
    // View ▸ Particle preview ▸ All (the lower of the two All rows; the
    // upper is Collider wireframes).
    let pill = d
        .nodes
        .iter()
        .find(|n| n.text.starts_with("Perspective"))
        .expect("the view pill")
        .rect
        .center();
    d.press(&mut ed, pill, PointerButton::Primary);
    let all = d
        .nodes
        .iter()
        .filter(|n| n.text == "All")
        .map(|n| n.rect.center())
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .expect("Particle preview: All");
    d.press(&mut ed, all, PointerButton::Primary);
    assert_eq!(ed.state.viewport.view.particles, ParticlePreview::All);
    d.key(&mut ed, Key::Escape, Modifiers::NONE);
    d.frames(&mut ed, 10);
    assert!(particles(&ed, uid).is_some_and(|n| n > 0));
    ed.state.viewport.view.particles = ParticlePreview::None;
    d.frames(&mut ed, 2);
    assert_eq!(particles(&ed, uid), None);

    // A one-shot burst that despawns when finished: previewed again and
    // again, never despawned in the edit world.
    ed.state.viewport.view.particles = ParticlePreview::Selected;
    let burst = ParticleEmitter {
        lifetime: glam::Vec2::splat(0.2),
        ..ParticleEmitter::burst(12)
    };
    assert!(ed.state.set_field(uid, "ParticleEmitter", &burst));
    ed.state.select(Some(uid));
    d.frames(&mut ed, 2);
    assert_eq!(particles(&ed, uid), Some(12));
    // Dies after 0.2 s, rests, then bursts again.
    let rest_frames = ((0.25 + RESTART_PAUSE) * 60.0) as usize + 2;
    let mut empty = false;
    let mut again = false;
    for _ in 0..rest_frames {
        d.frame(&mut ed, Vec::new());
        match particles(&ed, uid) {
            Some(0) => empty = true,
            Some(12) if empty => again = true,
            _ => {}
        }
    }
    assert!(empty && again, "empty {empty}, burst again {again}");
    assert!(ed.state.world.entity_by_uid(uid).is_some());

    // Hidden emitters are not previewed.
    ed.state.toggle_hide_selection();
    d.frames(&mut ed, 2);
    assert_eq!(particles(&ed, uid), None);
    ed.state.toggle_hide_selection();

    // Play runs its own simulation; the edit world is left alone, and the
    // play world starts from the saved scene (no preview state copied).
    d.frames(&mut ed, 5);
    let before = particles(&ed, uid);
    assert!(before.is_some());
    ed.state.start_play().unwrap();
    let e = ed.state.world.entity_by_uid(uid).unwrap();
    assert!(ed.state.world.get::<ParticleState>(e).is_none());
    ed.state.stop_play();
    assert!(particles(&ed, uid).is_some());
}

// ----- captures -----

/// Captures for review: the Luau collector in Play with its HUD clear, the
/// camera script's small mouse sensitivity, the Add menu, an ambient
/// volume and a particle emitter with their icons and helpers, and the
/// particle preview.
#[test]
fn m7_usability_captures() {
    use box_editor::headless::{CaptureOptions, Headless};
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping captures");
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/m7_editor");
    std::fs::create_dir_all(&out).unwrap();
    let options = CaptureOptions {
        size: [2880, 1800],
        pixels_per_point: 2.0,
        frames: 1,
    };
    let headless = |ed: &mut Editor| -> Option<Headless> {
        match Headless::new(ed, &options) {
            Ok(h) => Some(h),
            Err(e)
                if matches!(
                    e.downcast_ref::<box_render::RenderError>(),
                    Some(box_render::RenderError::NoAdapter)
                ) =>
            {
                eprintln!("no GPU adapter; skipping captures");
                None
            }
            Err(e) => panic!("{e:#}"),
        }
    };
    let run = |h: &mut Headless, ed: &mut Editor, events: Vec<Event>| {
        h.frame(ed, events);
        nodes_of(h.accesskit_update())
    };
    let settle = |h: &mut Headless, ed: &mut Editor, n: usize| {
        let mut nodes = Vec::new();
        for _ in 0..n {
            nodes = run(h, ed, Vec::new());
        }
        nodes
    };
    let find = |nodes: &[Node], text: &str| -> Rect {
        nodes
            .iter()
            .rev()
            .find(|n| n.text == text)
            .unwrap_or_else(|| panic!("no `{text}`"))
            .rect
    };
    let click = |h: &mut Headless, ed: &mut Editor, at: Pos2| {
        let b = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(h, ed, vec![Event::PointerMoved(at)]);
        run(h, ed, vec![b(true)]);
        run(h, ed, vec![b(false)]);
    };
    let save = |h: &Headless, name: &str| {
        let path = out.join(name);
        h.image().unwrap().save(&path).unwrap();
        eprintln!("{}", path.display());
    };

    // The Luau collector: the camera's script properties while editing
    // (mouse sensitivity 0.0040), then Play with the HUD clear.
    let dir = collector("captures");
    let mut ed = open(&dir.0);
    settle_assets(&mut ed);
    let Some(mut h) = headless(&mut ed) else {
        return;
    };
    h.enable_accesskit();
    ed.state.select(Some(by_name(&ed.state.world, "Camera")));
    let nodes = settle(&mut h, &mut ed, 4);
    find(&nodes, "0.0040");
    save(&h, "inspector_small_value.png");
    play_until_hud(&mut ed);
    ed.state.select(Some(by_name(&ed.state.world, "Camera")));
    settle(&mut h, &mut ed, 4);
    save(&h, "collector_luau_play.png");
    ed.state.stop_play();
    drop(h);

    // A new project: the Add menu, then a spot light, an ambient volume
    // and an emitter.
    let dir = temp_project("captures_add");
    let mut ed = open(&dir.0);
    let Some(mut h) = headless(&mut ed) else {
        return;
    };
    h.enable_accesskit();
    let nodes = settle(&mut h, &mut ed, 3);
    click(&mut h, &mut ed, find(&nodes, "+ Add").center());
    let nodes = settle(&mut h, &mut ed, 2);
    let spot = find(&nodes, "Spot Light");
    // Hover the new entry so the menu shows as in use.
    run(&mut h, &mut ed, vec![Event::PointerMoved(spot.center())]);
    // Past the menu's fade-in.
    settle(&mut h, &mut ed, 12);
    save(&h, "add_menu.png");
    run(
        &mut h,
        &mut ed,
        vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    let volume = ed.state.add_entity(NewEntity::AmbientVolume, None).unwrap();
    let emitter = ed
        .state
        .add_entity(NewEntity::ParticleEmitter, None)
        .unwrap();
    ed.state.add_entity(NewEntity::SpotLight, None).unwrap();
    // Away from the viewport, so no icon is drawn hovered.
    run(&mut h, &mut ed, vec![Event::PointerMoved(pos2(5.0, 5.0))]);
    ed.state.select(Some(volume));
    settle(&mut h, &mut ed, 3);
    save(&h, "ambient_volume_icon.png");
    // The emitter selected: its emission cone and its particles (the
    // preview), a second later.
    ed.state.select(Some(emitter));
    // Beside the cube, sparks that show against the ground.
    let beside = Transform::from_xyz(-2.5, 0.0, 1.0);
    assert!(ed.state.set_field(emitter, "Transform", &beside));
    let sparks = ParticleEmitter {
        rate: 80.0,
        speed: glam::Vec2::new(1.0, 2.0),
        cone_angle: 0.5,
        spawn_radius: 0.2,
        size: vec![
            box_render::CurveKey {
                t: 0.0,
                value: 0.18,
            },
            box_render::CurveKey {
                t: 1.0,
                value: 0.06,
            },
        ],
        color: vec![
            box_render::GradientKey {
                t: 0.0,
                color: box_core::color::Color::linear(1.0, 0.55, 0.15),
            },
            box_render::GradientKey {
                t: 1.0,
                color: box_core::color::Color::linear(1.0, 0.2, 0.05).with_alpha(0.0),
            },
        ],
        intensity: 3.0,
        blend: box_render::ParticleBlend::Additive,
        ..ParticleEmitter::default()
    };
    assert!(ed.state.set_field(emitter, "ParticleEmitter", &sparks));
    settle(&mut h, &mut ed, 90);
    assert!(particles(&ed, emitter).is_some_and(|n| n > 40));
    save(&h, "particle_preview.png");
    assert_eq!(h.renderer().validation_errors(), 0);
}
