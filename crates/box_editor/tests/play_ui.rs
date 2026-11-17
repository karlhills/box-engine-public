//! Game UI (box_ui) in the editor's Play mode: drawn inside the game view by
//! the renderer's overlay pass (no editor code involved), and driven by the
//! keys the editor forwards while the game has focus.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use box_core::prelude::*;
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, PlayState, build_game_editor, layout};
use box_engine::Game;
use box_engine::box_ui::{Ui, UiEvent, UiNode};
use box_render::RenderError;
use egui::{Event, Key, Modifiers, RawInput, Rect, pos2, vec2};

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("box_editor_play_ui_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A modal menu with two buttons whose focused look is magenta; records
/// the clicks it gets.
#[derive(Clone, Default)]
struct Menu(Arc<Mutex<Vec<String>>>);

impl Plugin for Menu {
    fn build(&self, app: &mut App) {
        let ui = app.world.resource_mut::<Ui>();
        let mut theme = box_engine::box_ui::Theme::default();
        theme.styles.insert(
            "button".into(),
            serde_json::from_str(r##"{"focus": {"background": "#ff00ff"}}"##).unwrap(),
        );
        ui.set_theme_value(theme);
        ui.add(
            None,
            UiNode::panel()
                .modal()
                .anchored([0.5, 0.5], [0.0, 0.0])
                .children([
                    UiNode::button("first", "").with_text("First"),
                    UiNode::button("second", "").with_text("Second"),
                ]),
        );
        let seen = self.0.clone();
        let mut cursor = EventCursor::<UiEvent>::default();
        app.add_system(Stage::Update, move |w| {
            for e in cursor.read(w.resource::<Events<UiEvent>>()) {
                if let (box_engine::box_ui::UiEventKind::Click, Some(id)) = (e.kind, &e.id) {
                    seen.lock().unwrap().push(id.clone());
                }
            }
        });
    }
}

fn raw(frame: u32, events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0))),
        time: Some(frame as f64 / 60.0),
        focused: true,
        events,
        ..Default::default()
    }
}

fn key(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

#[test]
fn forwarded_keys_navigate_and_activate_game_menus() {
    let dir = temp_dir("keys");
    let menu = Menu::default();
    let clicks = menu.0.clone();
    let options = EditorOptions::default();
    let mut editor = build_game_editor(
        &options,
        LogBuffer::default(),
        Game::new().with_plugin(menu),
    )
    .unwrap();
    editor.state.autosave_dir = dir;
    editor.tree = layout::default_layout();
    let ctx = egui::Context::default();
    editor.apply_theme(&ctx);
    let mut frame = 0;
    let mut run = |editor: &mut box_editor::Editor, events: Vec<Event>| {
        let _ = ctx.run_ui(raw(frame, events), |ui| editor.ui(ui));
        frame += 1;
    };
    run(&mut editor, vec![]);
    run(&mut editor, vec![key(Key::P, true, Modifiers::COMMAND)]);
    assert_eq!(editor.state.play_state(), PlayState::Playing);
    assert!(editor.state.game_focus());
    run(&mut editor, vec![]);
    run(
        &mut editor,
        vec![key(Key::ArrowDown, true, Modifiers::NONE)],
    );
    run(
        &mut editor,
        vec![key(Key::ArrowDown, false, Modifiers::NONE)],
    );
    run(&mut editor, vec![key(Key::Enter, true, Modifiers::NONE)]);
    run(&mut editor, vec![key(Key::Enter, false, Modifiers::NONE)]);
    assert_eq!(*clicks.lock().unwrap(), ["second"]);
    // The editor did not act on the keys (no tool or selection change).
    assert_eq!(editor.state.play_state(), PlayState::Playing);
}

#[test]
fn capture_game_ui_in_the_editor_play_view() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the Play UI capture");
        return;
    }
    let dir = temp_dir("capture");
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    let options = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let mut editor = build_game_editor(
        &EditorOptions::default(),
        LogBuffer::default(),
        Game::new().with_plugin(Menu::default()),
    )
    .unwrap();
    editor.state.autosave_dir = dir;
    editor.tree = layout::default_layout();
    editor.state.start_play().unwrap();
    let mut h = match Headless::new(&mut editor, &options) {
        Ok(h) => h,
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping the Play UI capture");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    for _ in 0..options.frames {
        h.frame(&mut editor, Vec::new());
    }
    let image = h.image().unwrap();
    image.save(out.join("editor_play_ui.png")).unwrap();
    // The focused button (magenta) is drawn inside the game view, centred.
    let view = editor.state.viewport.rect.expect("viewport laid out");
    let magenta: Vec<(u32, u32)> = image
        .enumerate_pixels()
        .filter(|(_, _, p)| p.0[0] > 240 && p.0[1] < 20 && p.0[2] > 240)
        .map(|(x, y, _)| (x, y))
        .collect();
    assert!(magenta.len() > 500, "{} magenta pixels", magenta.len());
    let (sx, sy) = magenta.iter().fold((0u64, 0u64), |(a, b), (x, y)| {
        (a + *x as u64, b + *y as u64)
    });
    let centre = pos2(
        sx as f32 / magenta.len() as f32,
        sy as f32 / magenta.len() as f32,
    );
    assert!(
        view.contains(centre),
        "{centre:?} outside the game view {view:?}"
    );
    assert!(
        (centre.x - view.center().x).abs() < 20.0,
        "{centre:?} vs {view:?}"
    );
}
