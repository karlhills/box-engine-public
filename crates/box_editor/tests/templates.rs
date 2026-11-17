//! New Project from a template and Open Example (a copy) in the editor,
//! run headlessly: the dialog flow through `Editor` methods, the Welcome
//! card and File menu driven by egui events, and captures of both modals
//! and the Welcome card in `target/captures/templates/` (skipped with
//! `BOX_SKIP_GPU_TESTS` or without an adapter).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{
    Dialog, DocAction, Editor, EditorOptions, NewEntity, UnsavedChoice, build_editor,
};
use box_engine::templates;
use box_render::RenderError;
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "box_editor_templates_{name}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn editor() -> Editor {
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).unwrap();
    editor.tree = box_editor::layout::default_layout();
    editor
}

/// Every file below `dir` with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if e.file_type().unwrap().is_dir() {
                walk(root, &p, out);
            } else {
                out.insert(
                    p.strip_prefix(root).unwrap().into(),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn new_project_from_a_template_creates_and_opens_it() {
    let base = temp_dir("new");
    let mut ed = editor();
    let cube = ed.state.add_entity(NewEntity::Cube, None).unwrap();
    ed.begin(DocAction::NewProject);
    ed.answer_unsaved(UnsavedChoice::DontSave);
    match ed.dialog() {
        Some(Dialog::NewProject {
            templates,
            template,
            path,
            discard: true,
            ..
        }) => {
            let names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
            assert_eq!(names, ["empty", "third-person", "procedural", "native"]);
            assert_eq!(template, "empty");
            assert!(
                path.ends_with("my-game") || path.contains("my-game-"),
                "{path}"
            );
        }
        other => panic!("{other:?}"),
    }
    // Cancelling keeps the scene and its changes.
    ed.answer_new_project(None);
    assert_eq!(ed.dialog(), None);
    assert!(ed.state.world.entity_by_uid(cube).is_some());

    ed.begin(DocAction::NewProject);
    ed.answer_unsaved(UnsavedChoice::DontSave);
    ed.select_template("no-such-template");
    ed.select_template("third-person");
    assert!(matches!(
        ed.dialog(),
        Some(Dialog::NewProject { template, .. }) if template == "third-person"
    ));
    // A blank folder is an error that keeps the modal open.
    ed.answer_new_project(Some("  "));
    assert!(matches!(
        ed.dialog(),
        Some(Dialog::NewProject { error: Some(_), .. })
    ));
    let dir = base.join("Hero Run");
    ed.answer_new_project(Some(dir.to_str().unwrap()));
    assert_eq!(ed.dialog(), None, "created and opened");
    let project = ed.state.project.as_ref().unwrap();
    assert_eq!(project.name(), "Hero Run");
    assert_eq!(
        project.root().canonicalize().unwrap(),
        dir.canonicalize().unwrap()
    );
    assert!(ed.state.world.find_by_name("Player").is_some());
    assert!(ed.state.world.find_by_name("Pickup 1").is_some());
    assert!(dir.join("README.md").is_file());
    assert!(!dir.join("template.json").exists());
    assert_eq!(ed.state.recent.entries()[0].name, "Hero Run");

    // The same folder again: refused, the open project stays.
    ed.begin(DocAction::NewProject);
    ed.answer_new_project(Some(dir.to_str().unwrap()));
    match ed.dialog() {
        Some(Dialog::NewProject { error: Some(e), .. }) => {
            assert!(e.contains("already contains a project"), "{e}")
        }
        other => panic!("{other:?}"),
    }
    // Native projects are made by `box new --native`, never here.
    ed.select_template("native");
    let native = base.join("rusty");
    ed.answer_new_project(Some(native.to_str().unwrap()));
    match ed.dialog() {
        Some(Dialog::NewProject { error: Some(e), .. }) => {
            assert!(e.contains("box new --native"), "{e}")
        }
        other => panic!("{other:?}"),
    }
    assert!(!native.exists());
    ed.answer_new_project(None);
    assert_eq!(ed.state.project.as_ref().unwrap().name(), "Hero Run");
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn open_example_copies_it_and_never_touches_the_original() {
    let base = temp_dir("example");
    let examples = templates::examples();
    let rules = templates::find_example(&examples, "script_rules").unwrap();
    let collector = templates::find_example(&examples, "collector_luau").unwrap();
    let before = (snapshot(&rules.dir), snapshot(&collector.dir));

    let mut ed = editor();
    ed.begin(DocAction::OpenExample(Some("script_rules".into())));
    let first_path = match ed.dialog() {
        Some(Dialog::OpenExample {
            examples,
            example,
            path,
            ..
        }) => {
            assert!(examples.len() >= 3);
            assert_eq!(example, "script_rules");
            assert!(path.contains("script_rules"), "{path}");
            path.clone()
        }
        other => panic!("{other:?}"),
    };
    // The folder follows the chosen example while it is named after it.
    ed.select_example("collector_luau");
    match ed.dialog() {
        Some(Dialog::OpenExample { example, path, .. }) => {
            assert_eq!(example, "collector_luau");
            assert!(path.contains("collector_luau"), "{path}");
            assert_eq!(
                Path::new(path).parent(),
                Path::new(&first_path).parent(),
                "same parent folder"
            );
        }
        other => panic!("{other:?}"),
    }
    ed.select_example("script_rules");

    // A folder with files in it is refused.
    let busy = base.join("busy");
    std::fs::create_dir_all(&busy).unwrap();
    std::fs::write(busy.join("notes.txt"), "mine").unwrap();
    ed.answer_open_example(Some(busy.to_str().unwrap()));
    match ed.dialog() {
        Some(Dialog::OpenExample { error: Some(e), .. }) => {
            assert!(e.contains("new or empty folder"), "{e}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_dir(&busy).unwrap().count(), 1);

    let copy = base.join("my rules");
    ed.answer_open_example(Some(copy.to_str().unwrap()));
    assert_eq!(ed.dialog(), None);
    let project = ed.state.project.as_ref().unwrap();
    assert_eq!(
        project.root().canonicalize().unwrap(),
        copy.canonicalize().unwrap()
    );
    assert_eq!(project.name(), "Script Rules");
    assert!(ed.state.world.find_by_name("Fence West").is_some());
    assert!(ed.state.world.find_by_name("Colonnade").is_some());
    // Edit and save the copy: the original stays as it was.
    ed.state.add_entity(NewEntity::Sphere, None).unwrap();
    ed.save();
    assert!(!ed.state.is_dirty());

    // The Collector, opened over the edited copy.
    ed.begin(DocAction::OpenExample(Some("collector_luau".into())));
    let copy2 = base.join("collector");
    ed.answer_open_example(Some(copy2.to_str().unwrap()));
    assert_eq!(ed.dialog(), None);
    assert!(ed.state.world.find_by_name("Player").is_some());

    assert!(
        snapshot(&rules.dir) == before.0 && snapshot(&collector.dir) == before.1,
        "an original example changed"
    );
    let _ = std::fs::remove_dir_all(base);
}

// ----- the Welcome card and File menu, driven by egui events -----

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
            Some(Node {
                role: n.role(),
                text: n.label().or(n.value()).unwrap_or_default().to_string(),
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
}

impl Driver {
    fn new(editor: &Editor) -> Self {
        let ctx = egui::Context::default();
        editor.apply_theme(&ctx);
        ctx.enable_accesskit();
        Self {
            ctx,
            time: 0.0,
            nodes: Vec::new(),
        }
    }

    fn frame(&mut self, editor: &mut Editor, events: Vec<Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            focused: true,
            events,
            ..Default::default()
        };
        let out = self.ctx.run_ui(raw, |ui| editor.ui(ui));
        self.nodes = nodes_of(out.platform_output.accesskit_update.as_ref());
        self.time += 1.0 / 60.0;
    }

    fn frames(&mut self, editor: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(editor, Vec::new());
        }
    }

    /// The widget labelled `text`, else one whose label starts with `text`
    /// and a space (a button with text on its right).
    fn find(&self, text: &str) -> Option<Node> {
        let widgets = || {
            self.nodes
                .iter()
                .rev()
                .filter(|n| n.role != Role::TextInput)
        };
        widgets()
            .find(|n| n.text == text)
            .or_else(|| widgets().find(|n| n.text.starts_with(&format!("{text} "))))
            .cloned()
    }

    fn click(&mut self, editor: &mut Editor, text: &str) {
        let pos = self
            .find(text)
            .unwrap_or_else(|| {
                let all: Vec<&str> = self.nodes.iter().map(|n| n.text.as_str()).collect();
                panic!("no widget `{text}` among {all:?}")
            })
            .rect
            .center();
        let b = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(editor, vec![Event::PointerMoved(pos)]);
        self.frame(editor, vec![b(true)]);
        self.frame(editor, vec![b(false)]);
        self.frames(editor, 3);
    }
}

fn chosen_example(ed: &Editor) -> Option<String> {
    match ed.dialog() {
        Some(Dialog::OpenExample { example, .. }) => Some(example.clone()),
        _ => None,
    }
}

fn chosen_template(ed: &Editor) -> Option<String> {
    match ed.dialog() {
        Some(Dialog::NewProject { template, .. }) => Some(template.clone()),
        _ => None,
    }
}

#[test]
fn welcome_card_and_file_menu_open_the_dialogs() {
    let mut ed = editor();
    ed.state.welcome = true;
    let mut d = Driver::new(&ed);
    d.frames(&mut ed, 3);
    assert!(
        d.find("Examples").is_some(),
        "the Welcome card lists the examples"
    );
    d.click(&mut ed, "Cinder Keep");
    assert_eq!(chosen_example(&ed).as_deref(), Some("cinder_keep"));
    // The modal's cards choose another example.
    d.click(&mut ed, "Script Rules");
    assert_eq!(chosen_example(&ed).as_deref(), Some("script_rules"));
    d.click(&mut ed, "Cancel");
    assert_eq!(ed.dialog(), None);

    d.click(&mut ed, "File");
    d.click(&mut ed, "Open Example…");
    assert_eq!(chosen_example(&ed).as_deref(), Some("collector_luau"));
    d.click(&mut ed, "Cancel");

    d.click(&mut ed, "File");
    d.click(&mut ed, "New Project…");
    assert_eq!(chosen_template(&ed).as_deref(), Some("empty"));
    d.click(&mut ed, "Procedural scene");
    assert_eq!(chosen_template(&ed).as_deref(), Some("procedural"));
    assert!(d.find("Create").is_some());
    // Native: the command to run instead of Create.
    d.click(&mut ed, "Native (Rust)");
    assert_eq!(chosen_template(&ed).as_deref(), Some("native"));
    assert!(d.find("Create").is_none());
    assert!(d.find("Copy Command").is_some());
    assert!(
        d.nodes
            .iter()
            .any(|n| n.text.starts_with("box new ") && n.text.ends_with("--native")),
        "the command is shown"
    );
    d.click(&mut ed, "Cancel");
    assert_eq!(ed.dialog(), None);
}

/// Captures the Welcome card with its examples, New Project with the
/// third-person template chosen and Open Example.
#[test]
fn template_dialog_captures() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/templates");
    std::fs::create_dir_all(&out).unwrap();
    let options = CaptureOptions {
        size: [1600, 1000],
        pixels_per_point: 2.0,
        frames: 30,
    };
    type Setup = fn(&mut Editor);
    let shots: [(&str, Setup); 3] = [
        ("editor_welcome.png", |ed| ed.state.welcome = true),
        ("editor_new_project.png", |ed| {
            ed.begin(DocAction::NewProject);
            ed.select_template("third-person");
        }),
        ("editor_open_example.png", |ed| {
            ed.begin(DocAction::OpenExample(None))
        }),
    ];
    for (name, setup) in shots {
        let log = LogBuffer::default();
        log.set_manual_time(0.0);
        let mut ed = build_editor(&EditorOptions::default(), log).unwrap();
        ed.tree = box_editor::layout::default_layout();
        setup(&mut ed);
        let image = match capture(&mut ed, &options) {
            Ok(image) => image,
            Err(e)
                if matches!(
                    e.downcast_ref::<RenderError>(),
                    Some(RenderError::NoAdapter)
                ) =>
            {
                return;
            }
            Err(e) => panic!("{e:#}"),
        };
        image.save(out.join(name)).unwrap();
    }
}
