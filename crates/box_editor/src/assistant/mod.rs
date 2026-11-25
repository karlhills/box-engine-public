//! The Assistant panel (View ▸ Assistant; docs/AI_PANEL.md, D20): the user
//! describes what to build or change, an agent backend does it through
//! `box mcp` and the editor's control socket, and the panel shows what it
//! said and did.
//!
//! - **Backends** ([`backend`]): Claude Code, the user's installed `claude`
//!   CLI run headless ([`claude_code`]), or a scripted replay (tests).
//! - **One undo step per turn**: a turn opens an undo group
//!   ([`box_core::edit::EditStack::begin_group`]) that merges the agent's
//!   command lists ("Agent edit") into one step labelled "Assistant: <the
//!   request>"; the user's own edits during the turn stay separate. Files
//!   the agent wrote (`write_asset`) are journalled and put back by Undo
//!   Turn.
//! - **Control**: while a Claude Code turn runs, Allow Agent Control is on
//!   (switched off again after the turn if the panel turned it on).
//!
//! Nothing here connects to a network: the backend process does, once the
//! user sends a message.

pub mod backend;
pub mod claude_code;
mod window;

use std::path::PathBuf;

use box_engine::agent_files;
use serde_json::Value as Json;

use crate::control::AGENT_EDIT;
use crate::state::EditorState;
use backend::{AgentBackend, AgentEvent};

pub(crate) use window::{assistant_window, start_at};

/// The instructions appended to the agent's system prompt.
pub const SYSTEM_PROMPT: &str = include_str!("prompt.md");

/// One request and what came of it.
#[derive(Clone, Debug)]
pub struct Turn {
    pub prompt: String,
    pub items: Vec<TurnItem>,
    pub status: TurnStatus,
    /// The undo label of its scene edits.
    pub undo_label: String,
    /// Undo steps its scene edits became (runs split by the user's edits).
    pub undo_steps: usize,
    /// Files it wrote (journal entries), oldest first.
    pub files: Vec<Json>,
    pub cost_usd: Option<f64>,
}

/// A line of a turn.
#[derive(Clone, Debug, PartialEq)]
pub enum TurnItem {
    Text(String),
    /// A tool call in plain words; `ok` is `None` while it runs.
    Step {
        id: String,
        label: String,
        ok: Option<bool>,
        detail: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TurnStatus {
    Running,
    Done,
    Stopped,
    Failed(String),
    /// Undone with Undo Turn.
    Undone,
}

/// The panel's state (on [`EditorState`]).
#[derive(Default)]
pub struct AssistantPanel {
    pub open: bool,
    /// The message being typed.
    pub input: String,
    pub turns: Vec<Turn>,
    backend: Option<Box<dyn AgentBackend>>,
    /// The project the backend works in (a different project gets a new one).
    backend_root: Option<PathBuf>,
    /// Why no backend could be made (shown in the panel).
    pub backend_error: Option<String>,
    /// Journal length when the running turn began.
    journal_start: usize,
    /// The panel switched Allow Agent Control on for the running turn.
    enabled_control: bool,
    /// Uses the control socket (Claude Code) rather than applying commands
    /// itself (scripted).
    uses_socket: bool,
}

impl EditorState {
    /// Whether the Assistant window is open.
    pub fn assistant_open(&self) -> bool {
        self.assistant.open
    }

    /// Opens or closes the Assistant window (a running turn goes on).
    /// Opening looks for the agent again if it was missing.
    pub fn set_assistant(&mut self, open: bool) {
        if open && !self.assistant.open {
            self.assistant.backend_error = None;
            // Start the agent now: its tools connect while the user types.
            if self.ensure_backend().is_ok()
                && let Some(b) = self.assistant.backend.as_mut()
            {
                b.warm_up();
            }
        }
        self.assistant.open = open;
    }

    /// Puts `text` in the message box.
    pub fn set_assistant_input(&mut self, text: &str) {
        self.assistant.input = text.to_string();
    }

    /// The panel's turns, oldest first.
    pub fn assistant_turns(&self) -> &[Turn] {
        &self.assistant.turns
    }

    /// Whether a turn is running.
    pub fn assistant_busy(&self) -> bool {
        self.assistant
            .turns
            .last()
            .is_some_and(|t| t.status == TurnStatus::Running)
    }

    /// Uses `backend` instead of Claude Code (tests, other agents). A
    /// backend that applies commands itself ([`AgentBackend::take_commands`])
    /// needs no control socket.
    pub fn set_assistant_backend(&mut self, backend: Box<dyn AgentBackend>, uses_socket: bool) {
        self.assistant.backend = Some(backend);
        self.assistant.backend_root = self.project_root();
        self.assistant.backend_error = None;
        self.assistant.uses_socket = uses_socket;
    }

    /// The backend's name, or why there is none.
    pub fn assistant_backend_status(&mut self) -> Result<String, String> {
        self.ensure_backend().map(|()| {
            self.assistant
                .backend
                .as_ref()
                .map_or_else(String::new, |b| b.name().to_string())
        })
    }

    fn ensure_backend(&mut self) -> Result<(), String> {
        if self.assistant.backend.is_some() {
            if self.assistant.backend_root == self.project_root() || self.assistant_busy() {
                return Ok(());
            }
            // Another project was opened: start over there.
            self.assistant.backend = None;
            self.assistant.turns.clear();
        }
        if let Some(e) = &self.assistant.backend_error {
            return Err(e.clone());
        }
        let Some(root) = self.project.as_ref().map(|p| p.root().to_path_buf()) else {
            return Err("Open or create a project first: the assistant edits a project.".into());
        };
        match claude_code::ClaudeCodeSetup::find() {
            Ok(setup) => {
                self.assistant.backend = Some(Box::new(claude_code::ClaudeCodeBackend::new(
                    setup,
                    &root,
                    SYSTEM_PROMPT,
                )));
                self.assistant.uses_socket = true;
                self.assistant.backend_root = Some(root);
                self.assistant.backend_error = None;
                Ok(())
            }
            Err(e) => {
                self.assistant.backend_error = Some(e.clone());
                Err(e)
            }
        }
    }

    fn project_root(&self) -> Option<PathBuf> {
        self.project.as_ref().map(|p| p.root().to_path_buf())
    }

    /// Starts a turn with `request`. Errors (no backend, a turn already
    /// running, an empty request) leave everything as it was.
    pub fn assistant_send(&mut self, request: &str) -> Result<(), String> {
        let request = request.trim();
        if request.is_empty() {
            return Err("Type what to build or change first.".into());
        }
        if self.assistant_busy() {
            return Err("The assistant is still working: wait, or press Stop.".into());
        }
        if self.is_playing() {
            return Err(
                "Stop Play first: changes made during Play are thrown away at Stop.".into(),
            );
        }
        self.ensure_backend()?;
        if self.assistant.uses_socket && self.agent_control().is_none() {
            if !self.set_agent_control(true) {
                return Err("Could not start the editor's agent control (see the Console).".into());
            }
            self.assistant.enabled_control = true;
        }
        self.finish_edit();
        let label = format!("Assistant: {}", short(request));
        self.edits.begin_group(label.clone(), AGENT_EDIT);
        self.assistant.journal_start = self
            .project_root()
            .map_or(0, |r| agent_files::journal_entries(&r).len());
        let message = format!("{}\n\n{request}", self.context_note());
        let sent = self
            .assistant
            .backend
            .as_mut()
            .map_or(Err("no backend".to_string()), |b| b.send(&message));
        self.assistant.turns.push(Turn {
            prompt: request.to_string(),
            items: Vec::new(),
            status: TurnStatus::Running,
            undo_label: label,
            undo_steps: 0,
            files: Vec::new(),
            cost_usd: None,
        });
        if let Err(e) = sent {
            self.finish_turn(TurnStatus::Failed(e.clone()), None);
            return Err(e);
        }
        Ok(())
    }

    /// Stops the running turn; what it changed stays, as one undo step.
    pub fn assistant_stop(&mut self) {
        if !self.assistant_busy() {
            return;
        }
        if let Some(b) = self.assistant.backend.as_mut() {
            b.cancel();
        }
        self.finish_turn(TurnStatus::Stopped, None);
    }

    /// Takes the backend's events and applies them; call once per frame.
    /// Returns whether a turn is running (keep repainting).
    pub fn poll_assistant(&mut self) -> bool {
        let Some(backend) = self.assistant.backend.as_mut() else {
            return false;
        };
        let events = backend.poll();
        let commands = backend.take_commands();
        for list in commands {
            let r = self.apply_agent_commands(&list, false);
            let (ok, detail) = match &r {
                Ok(j) => (true, j.to_string()),
                Err(e) => (false, e.to_string()),
            };
            if let Some(t) = self.assistant.turns.last_mut() {
                t.items.push(TurnItem::Step {
                    id: String::new(),
                    label: format!("Edited the scene ({})", plural(list.len(), "change")),
                    ok: Some(ok),
                    detail,
                });
            }
        }
        for e in events {
            self.on_event(e);
        }
        self.assistant_busy()
    }

    fn on_event(&mut self, e: AgentEvent) {
        let Some(turn) = self.assistant.turns.last_mut() else {
            return;
        };
        if turn.status != TurnStatus::Running {
            return;
        }
        match e {
            AgentEvent::Session(_) => {}
            AgentEvent::Text(t) => turn.items.push(TurnItem::Text(t)),
            AgentEvent::ToolStart { id, name, input } => turn.items.push(TurnItem::Step {
                id,
                label: step_label(&name, &input),
                ok: None,
                detail: String::new(),
            }),
            AgentEvent::ToolDone { id, ok, summary } => {
                for item in turn.items.iter_mut().rev() {
                    if let TurnItem::Step {
                        id: i,
                        ok: o,
                        detail,
                        ..
                    } = item
                        && *i == id
                    {
                        *o = Some(ok);
                        *detail = summary;
                        break;
                    }
                }
            }
            AgentEvent::TurnDone { cost_usd, error } => {
                let status = match error {
                    Some(e) => TurnStatus::Failed(e),
                    None => TurnStatus::Done,
                };
                self.finish_turn(status, cost_usd);
            }
            AgentEvent::Failed(e) => self.finish_turn(TurnStatus::Failed(e), None),
        }
    }

    /// Closes the running turn: its undo group, the files it wrote, and the
    /// control socket if the panel opened it.
    fn finish_turn(&mut self, status: TurnStatus, cost_usd: Option<f64>) {
        let steps = if self.edits.group_open() {
            self.edits.end_group()
        } else {
            0
        };
        let files = self.project_root().map_or_else(Vec::new, |r| {
            agent_files::journal_entries(&r)
                .into_iter()
                .skip(self.assistant.journal_start)
                .collect()
        });
        if self.assistant.enabled_control {
            self.set_agent_control(false);
            self.assistant.enabled_control = false;
        }
        if let Some(t) = self.assistant.turns.last_mut() {
            for item in &mut t.items {
                if let TurnItem::Step { ok: o @ None, .. } = item {
                    *o = Some(false);
                }
            }
            t.status = status;
            t.undo_steps = steps;
            t.files = files;
            t.cost_usd = cost_usd;
        }
    }

    /// Whether turn `index` can be undone now, or why not.
    pub fn assistant_can_undo(&self, index: usize) -> Result<(), String> {
        let Some(t) = self.assistant.turns.get(index) else {
            return Err("no such turn".into());
        };
        match t.status {
            TurnStatus::Running => return Err("Wait for it to finish, or press Stop".into()),
            TurnStatus::Undone => return Err("Already undone".into()),
            _ => {}
        }
        if t.undo_steps == 0 && t.files.is_empty() {
            return Err("It changed nothing".into());
        }
        let top: Vec<&str> = self.edits.undo_labels().rev().take(t.undo_steps).collect();
        if top.len() < t.undo_steps || top.iter().any(|l| *l != t.undo_label) {
            return Err("Undo the later changes first (Cmd+Z), or edit by hand".into());
        }
        if self.is_playing() {
            return Err("Stop Play first".into());
        }
        Ok(())
    }

    /// Undo Turn: reverts turn `index`'s scene edits and the files it wrote.
    pub fn assistant_undo(&mut self, index: usize) -> Result<(), String> {
        self.assistant_can_undo(index)?;
        let (steps, files) = {
            let t = &self.assistant.turns[index];
            (t.undo_steps, t.files.clone())
        };
        for _ in 0..steps {
            self.undo_scene();
        }
        if let Some(root) = self.project_root() {
            for f in files.iter().rev() {
                if let Err(e) = agent_files::restore(&root, f) {
                    tracing::warn!("Undo Turn could not restore a file: {e}");
                }
            }
        }
        self.assistant.turns[index].status = TurnStatus::Undone;
        Ok(())
    }

    /// The editor context sent before each request.
    fn context_note(&self) -> String {
        let c = self.editor_context();
        let names: Vec<String> = c["selection"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| {
                format!(
                    "{} ({})",
                    s["name"].as_str().unwrap_or("unnamed"),
                    s["uid"].as_str().unwrap_or("?")
                )
            })
            .collect();
        let v3 = |v: &Json| -> String {
            let n: Vec<String> = v
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| format!("{:.1}", x.as_f64().unwrap_or(0.0)))
                .collect();
            format!("[{}]", n.join(", "))
        };
        format!(
            "[Editor context] scene: {}; selected: {}; camera at {} looking at {}; mode: {}.",
            c["scene"].as_str().unwrap_or("unsaved"),
            if names.is_empty() {
                "nothing".to_string()
            } else {
                names.join(", ")
            },
            v3(&c["camera"]["eye"]),
            v3(&c["camera"]["look_at"]),
            c["mode"].as_str().unwrap_or("edit"),
        )
    }
}

/// The first words of a request, for its undo label.
fn short(request: &str) -> String {
    let words: Vec<&str> = request.split_whitespace().take(6).collect();
    let mut s = words.join(" ");
    if request.split_whitespace().count() > 6 {
        s.push('…');
    }
    s
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// A tool call in plain words.
pub fn step_label(tool: &str, input: &Json) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    match tool {
        "apply_commands" => {
            let commands: Vec<&Json> = match &input["commands"] {
                Json::Array(a) => a.iter().collect(),
                Json::Object(_) => vec![&input["commands"]],
                _ => Vec::new(),
            };
            let presets: Vec<String> = commands
                .iter()
                .filter(|c| c["op"] == "add_preset")
                .filter_map(|c| c["preset"].as_str().map(|p| p.replace('_', " ")))
                .collect();
            if !presets.is_empty() && presets.len() == commands.len() {
                format!("Placed {}", summarize(&presets))
            } else if input["dry_run"] == true {
                format!("Checked {}", plural(commands.len(), "change"))
            } else {
                format!("Edited the scene ({})", plural(commands.len(), "change"))
            }
        }
        "scene_apply" => "Edited a scene file".into(),
        "screenshot" => "Looked at the viewport".into(),
        "render" => "Rendered the scene".into(),
        "check" => "Checked the project".into(),
        "playtest" => "Played a playtest".into(),
        "record_start" => "Started recording a playtest".into(),
        "record_stop" => "Finished recording a playtest".into(),
        "walk_route" => "Walked a route as the player".into(),
        "registry" => match input["name"].as_str() {
            Some(n) => format!("Looked up {n}"),
            None => "Looked up the component types".into(),
        },
        "world_query" | "editor_context" | "status" | "scene_show" | "scene_list" => {
            "Looked at the scene".into()
        }
        "read_asset" => format!("Read {}", s("path")),
        "write_asset" => format!("Wrote {}", s("path")),
        "play" => "Started Play".into(),
        "stop" => "Stopped Play".into(),
        "pause" => "Paused Play".into(),
        "step_frames" => "Stepped the game".into(),
        "send_input" => format!("Pressed {}", s("action")),
        "teleport" => "Moved the player".into(),
        "read_log" => "Read the log".into(),
        "undo_turn" => "Undid its changes so far".into(),
        "import" => format!("Imported {}", s("file")),
        other => format!("Used {other}"),
    }
}

/// "pickup ×3 and goal".
fn summarize(items: &[String]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for i in items {
        match counts.iter_mut().find(|(n, _)| n == i) {
            Some((_, c)) => *c += 1,
            None => counts.push((i.clone(), 1)),
        }
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(n, c)| if c > 1 { format!("{n} ×{c}") } else { n })
        .collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        _ => format!(
            "{} and {}",
            parts[..parts.len() - 1].join(", "),
            parts[parts.len() - 1]
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_step_labels() {
        let c = json!({ "commands": [
            { "op": "add_preset", "preset": "pickup" },
            { "op": "add_preset", "preset": "pickup" },
            { "op": "add_preset", "preset": "moving_platform" }
        ] });
        assert_eq!(
            step_label("apply_commands", &c),
            "Placed pickup ×2 and moving platform"
        );
        let c = json!({ "commands": { "op": "set", "entity": "A", "path": "x", "value": 1 } });
        assert_eq!(
            step_label("apply_commands", &c),
            "Edited the scene (1 change)"
        );
        assert_eq!(
            step_label("write_asset", &json!({ "path": "scripts/a.luau" })),
            "Wrote scripts/a.luau"
        );
        assert_eq!(
            short("make a coin game on a floating island please"),
            "make a coin game on a…"
        );
    }
}
