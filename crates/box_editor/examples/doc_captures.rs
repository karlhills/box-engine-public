//! Screenshots for the user guides (docs/guide/images/), made by driving a
//! headless editor the way a person would: a fresh project, menus clicked by
//! their labels (through the accessibility tree), presets placed, the game
//! played to its win screen, the Assistant with a recorded conversation.
//!
//! ```bash
//! cargo run -p box_editor --example doc_captures [-- <out dir>]
//! ```
//!
//! Needs a GPU. Re-run it after changing the editor's look and review the
//! images before committing them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use box_core::edit::Command;
use box_editor::assistant::backend::{AgentEvent, ScriptStep, ScriptedBackend};
use box_editor::headless::{CaptureOptions, Headless};
use box_editor::log::LogBuffer;
use box_editor::{DocAction, Editor, EditorOptions, build_editor, layout};
use egui::accesskit::{Role, TreeUpdate};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2};
use serde_json::json;

const SIZE: [u32; 2] = [1600, 1000];

struct Node {
    role: Role,
    text: String,
    rect: Rect,
}

fn nodes_of(update: Option<&TreeUpdate>) -> Vec<Node> {
    let Some(u) = update else { return Vec::new() };
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

struct Shots {
    h: Headless,
    out: PathBuf,
    nodes: Vec<Node>,
}

impl Shots {
    fn frame(&mut self, ed: &mut Editor, events: Vec<Event>) {
        self.h.frame(ed, events);
        self.nodes = nodes_of(self.h.accesskit_update());
    }

    fn frames(&mut self, ed: &mut Editor, n: usize) {
        for _ in 0..n {
            self.frame(ed, Vec::new());
        }
    }

    fn at(&self, text: &str) -> Result<Pos2> {
        self.nodes
            .iter()
            .rev()
            .find(|n| n.text == text && n.role != Role::TextInput)
            .map(|n| n.rect.center())
            .with_context(|| format!("no widget `{text}` on screen"))
    }

    fn hover(&mut self, ed: &mut Editor, text: &str) -> Result<()> {
        let p = self.at(text)?;
        self.frame(ed, vec![Event::PointerMoved(p)]);
        self.frames(ed, 3);
        Ok(())
    }

    fn click(&mut self, ed: &mut Editor, text: &str) -> Result<()> {
        let pos = self.at(text)?;
        let b = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.frame(ed, vec![Event::PointerMoved(pos)]);
        self.frame(ed, vec![b(true)]);
        self.frame(ed, vec![b(false)]);
        self.frames(ed, 3);
        Ok(())
    }

    fn key(&mut self, ed: &mut Editor, key: Key) {
        let e = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        self.frame(ed, vec![e(true)]);
        self.frame(ed, vec![e(false)]);
        self.frames(ed, 2);
    }

    /// The screen, or the part of it in `crop` (points).
    fn grab(&mut self, ed: &mut Editor, crop: Option<Rect>) -> Result<image::RgbaImage> {
        self.frames(ed, 3);
        let mut img = self.h.image()?;
        if let Some(c) = crop {
            img = image::imageops::crop_imm(
                &img,
                c.left() as u32,
                c.top() as u32,
                c.width() as u32,
                c.height() as u32,
            )
            .to_image();
        }
        Ok(img)
    }

    fn write(&self, name: &str, img: &image::RgbaImage) -> Result<()> {
        let path = self.out.join(name);
        img.save(&path)?;
        println!("{}", path.display());
        Ok(())
    }

    /// Saves the screen, or the part of it in `crop` (points).
    fn save(&mut self, ed: &mut Editor, name: &str, crop: Option<Rect>) -> Result<()> {
        let img = self.grab(ed, crop)?;
        self.write(name, &img)
    }
}

fn apply(ed: &mut Editor, commands: serde_json::Value) -> Result<()> {
    let cmds: Vec<Command> = serde_json::from_value(commands)?;
    ed.state
        .apply_agent_commands(&cmds, false)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn select(ed: &mut Editor, name: &str) -> Result<()> {
    let e = ed
        .state
        .world
        .find_by_name(name)
        .with_context(|| format!("no `{name}`"))?;
    let uid = ed.state.world.uid(e).context("no uid")?;
    ed.state.select(Some(uid));
    Ok(())
}

fn project(dir: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(dir);
    let list = box_engine::templates::templates();
    let empty = list
        .iter()
        .find(|t| t.name == "empty")
        .context("empty template")?;
    box_engine::templates::create_project(dir, "coin-run", &list, empty)?;
    Ok(())
}

fn main() -> Result<()> {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide/images")
        });
    std::fs::create_dir_all(&out)?;
    let dir = std::env::temp_dir().join(format!("box_doc_captures_{}", std::process::id()));
    project(&dir)?;
    let mut ed = build_editor(
        &EditorOptions {
            project: Some(dir.clone()),
            ..Default::default()
        },
        LogBuffer::default(),
    )?;
    ed.tree = layout::default_layout();
    let options = CaptureOptions {
        size: SIZE,
        pixels_per_point: 1.0,
        frames: 1,
    };
    let h = Headless::new(&mut ed, &options)?;
    h.enable_accesskit();
    let mut s = Shots {
        h,
        out,
        nodes: Vec::new(),
    };
    s.frames(&mut ed, 4);
    let inspector = Rect::from_min_max(pos2(1230.0, 36.0), pos2(1600.0, 1000.0));

    // New Project: the template list, a tidy folder typed in (the default is
    // in the temporary folder), the modal's fade-in finished.
    ed.begin(DocAction::NewProject);
    s.frames(&mut ed, 4);
    let field = s
        .nodes
        .iter()
        .rfind(|n| n.role == Role::TextInput)
        .map(|n| n.rect.center())
        .context("no folder field")?;
    let press = |pressed| Event::PointerButton {
        pos: field,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    s.frame(&mut ed, vec![Event::PointerMoved(field), press(true)]);
    s.frame(&mut ed, vec![press(false)]);
    let select_all = |pressed| Event::Key {
        key: Key::A,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    };
    s.frame(&mut ed, vec![select_all(true), select_all(false)]);
    s.frame(&mut ed, vec![Event::Text("~/Games/coin-run".into())]);
    s.frame(&mut ed, vec![Event::PointerMoved(pos2(10.0, 990.0))]);
    s.frames(&mut ed, 30);
    s.save(&mut ed, "new-project.png", None)?;
    s.key(&mut ed, Key::Escape);

    // The starter scene, the cube selected.
    select(&mut ed, "Cube")?;
    s.save(&mut ed, "starter-scene.png", None)?;
    ed.state.delete_selection();

    // + Add ▸ Gameplay.
    s.click(&mut ed, "+ Add")?;
    s.hover(&mut ed, "Gameplay")?;
    s.hover(&mut ed, "Player")?;
    s.save(&mut ed, "add-gameplay-menu.png", None)?;
    s.key(&mut ed, Key::Escape);
    s.key(&mut ed, Key::Escape);

    // The player and its controller.
    ed.state.add_preset("player").context("player")?;
    s.save(&mut ed, "player-inspector.png", None)?;

    // Five coins and the rest of the course.
    apply(
        &mut ed,
        json!([
            { "op": "set", "entity": "Player", "path": "Transform.translation", "value": [0.0, 0.9, 6.0] },
            { "op": "set", "entity": "Player Camera", "path": "Transform.translation", "value": [0.0, 4.4, 13.5] },
            { "op": "add_preset", "preset": "pickup", "at": [-2.0, 0.0, 2.0] },
            { "op": "add_preset", "preset": "pickup", "at": [-1.0, 0.0, 0.0] },
            { "op": "add_preset", "preset": "pickup", "at": [0.0, 0.0, -2.0] },
            { "op": "add_preset", "preset": "pickup", "at": [1.0, 0.0, -4.0] },
            { "op": "add_preset", "preset": "pickup", "at": [2.0, 0.0, -6.0] },
        ]),
    )?;
    select(&mut ed, "Pickup 3")?;
    s.save(&mut ed, "pickups.png", None)?;
    apply(
        &mut ed,
        json!([
            { "op": "add_preset", "preset": "moving_platform", "at": [-6.0, 0.0, -3.0] },
            { "op": "add_preset", "preset": "hazard", "at": [3.0, 0.0, 1.0] },
            { "op": "add_preset", "preset": "checkpoint", "at": [-3.0, 0.0, 4.0] },
            { "op": "add_preset", "preset": "goal", "at": [0.0, 0.0, -10.0] },
        ]),
    )?;

    // Scene Settings: Add Scene Setting, then the rules.
    ed.state.select(None);
    s.frames(&mut ed, 3);
    s.click(&mut ed, "Add Scene Setting")?;
    let menu = s.grab(&mut ed, Some(inspector))?;
    s.key(&mut ed, Key::Escape);
    apply(
        &mut ed,
        json!([
            { "op": "set_resource", "path": "GameRules", "value": { "win": [{ "GroupDone": { "group": "pickups" } }], "time_limit": 60 } },
            { "op": "set_resource", "path": "HudSettings", "value": {} },
        ]),
    )?;
    s.frames(&mut ed, 3);
    if s.at("Win 0").is_ok() {
        s.click(&mut ed, "Win 0")?;
    }
    // Both inspector views side by side: narrow images look wrong stretched
    // to a page's width.
    let rules = s.grab(&mut ed, Some(inspector))?;
    let gap = 24;
    let back = *menu.get_pixel(menu.width() - 1, menu.height() - 1);
    let mut both = image::RgbaImage::from_pixel(menu.width() * 2 + gap, menu.height(), back);
    image::imageops::overlay(&mut both, &menu, 0, 0);
    image::imageops::overlay(&mut both, &rules, i64::from(menu.width() + gap), 0);
    s.write("scene-settings.png", &both)?;

    // Play, then win.
    ed.state
        .start_play()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    s.frames(&mut ed, 30);
    s.save(&mut ed, "playing.png", None)?;
    let pickups: Vec<String> = (1..=5)
        .map(|i| {
            if i == 1 {
                "Pickup".to_string()
            } else {
                format!("Pickup {i}")
            }
        })
        .collect();
    let set_used: Vec<serde_json::Value> = pickups
        .iter()
        .map(|p| json!({ "op": "set", "entity": p, "path": "Interactable.uses", "value": 1 }))
        .collect();
    apply(&mut ed, json!(set_used))?;
    s.frames(&mut ed, 10);
    s.save(&mut ed, "win-screen.png", None)?;
    ed.state.stop_play();
    s.frames(&mut ed, 3);

    // The Assistant, with a recorded conversation.
    let platform: Vec<Command> = serde_json::from_value(json!([
        { "op": "add_preset", "preset": "moving_platform", "at": [6.0, 0.0, -6.0] }
    ]))?;
    let turn = vec![
        ScriptStep::Event(AgentEvent::Text(
            "I'll add a platform that carries the player toward the goal.".into(),
        )),
        ScriptStep::Event(AgentEvent::ToolStart {
            id: "1".into(),
            name: "apply_commands".into(),
            input: json!({ "commands": [{ "op": "add_preset", "preset": "moving_platform" }] }),
        }),
        ScriptStep::Apply(platform),
        ScriptStep::Event(AgentEvent::ToolDone {
            id: "1".into(),
            ok: true,
            summary: "created 1".into(),
        }),
        ScriptStep::Event(AgentEvent::ToolStart {
            id: "2".into(),
            name: "check".into(),
            input: json!({}),
        }),
        ScriptStep::Event(AgentEvent::ToolDone {
            id: "2".into(),
            ok: true,
            summary: "ok".into(),
        }),
        ScriptStep::Event(AgentEvent::Text(
            "Done: a second moving platform now slides between the hazard and the goal. \
             Press Play and ride it across."
                .into(),
        )),
        ScriptStep::Event(AgentEvent::TurnDone {
            cost_usd: Some(0.04),
            error: None,
        }),
    ];
    ed.state
        .set_assistant_backend(Box::new(ScriptedBackend::new(vec![turn])), false);
    ed.state
        .assistant_send("Add a moving platform that carries the player toward the goal")
        .map_err(|e| anyhow::anyhow!(e))?;
    for _ in 0..20 {
        ed.state.poll_assistant();
    }
    ed.state.set_assistant(true);
    ed.state
        .set_assistant_input("Make the coins spin twice as fast");
    s.save(&mut ed, "assistant.png", None)?;

    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}
