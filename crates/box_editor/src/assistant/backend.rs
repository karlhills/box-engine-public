//! Agent backends: what answers the Assistant panel. A backend takes the
//! user's message and streams [`AgentEvent`]s back; it acts on the editor
//! through `box mcp` and the editor's control socket, not through the panel.

use box_core::edit::Command;
use serde_json::Value as Json;

/// Something the agent did or said during a turn.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// The backend's session id (to resume after a stop).
    Session(String),
    /// Text for the user.
    Text(String),
    /// A tool call started.
    ToolStart {
        id: String,
        name: String,
        input: Json,
    },
    /// A tool call finished; `summary` is the start of its result.
    ToolDone {
        id: String,
        ok: bool,
        summary: String,
    },
    /// The turn ended. `error` when the agent stopped on an error.
    TurnDone {
        cost_usd: Option<f64>,
        error: Option<String>,
    },
    /// The backend failed (could not start, crashed).
    Failed(String),
}

/// What the panel talks to.
pub trait AgentBackend {
    /// A short name for the status line ("Claude Code").
    fn name(&self) -> &str;
    /// Starts a turn with this message.
    fn send(&mut self, message: &str) -> Result<(), String>;
    /// Events since the last call (never blocks).
    fn poll(&mut self) -> Vec<AgentEvent>;
    /// Stops the running turn at once; the next `send` continues the same
    /// conversation where the backend can.
    fn cancel(&mut self);
    /// Gets ready before the first message (the panel opened), so the
    /// agent's tools are connected when the user sends.
    fn warm_up(&mut self) {}
    /// Commands to apply to the editor now, for backends that edit through
    /// the panel instead of the control socket (the scripted test backend).
    fn take_commands(&mut self) -> Vec<Vec<Command>> {
        Vec::new()
    }
}

/// One step of a [`ScriptedBackend`] turn.
#[derive(Clone, Debug)]
pub enum ScriptStep {
    Event(AgentEvent),
    /// Apply these commands as the agent would (`apply_commands`).
    Apply(Vec<Command>),
}

/// A backend that replays recorded turns, one step per poll: for tests and
/// for trying the panel without an agent installed.
#[derive(Debug, Default)]
pub struct ScriptedBackend {
    turns: Vec<Vec<ScriptStep>>,
    current: Option<std::vec::IntoIter<ScriptStep>>,
    pending_commands: Vec<Vec<Command>>,
    /// Messages received, for tests.
    pub received: Vec<String>,
}

impl ScriptedBackend {
    /// Replays `turns` in order, one per `send`.
    pub fn new(turns: Vec<Vec<ScriptStep>>) -> Self {
        Self {
            turns,
            ..Default::default()
        }
    }
}

impl AgentBackend for ScriptedBackend {
    fn name(&self) -> &str {
        "Scripted"
    }

    fn send(&mut self, message: &str) -> Result<(), String> {
        self.received.push(message.to_string());
        if self.turns.is_empty() {
            return Err("the scripted backend has no more turns".into());
        }
        self.current = Some(self.turns.remove(0).into_iter());
        Ok(())
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        let Some(steps) = &mut self.current else {
            return Vec::new();
        };
        match steps.next() {
            Some(ScriptStep::Event(e)) => {
                if matches!(e, AgentEvent::TurnDone { .. } | AgentEvent::Failed(_)) {
                    self.current = None;
                }
                vec![e]
            }
            Some(ScriptStep::Apply(c)) => {
                self.pending_commands.push(c);
                Vec::new()
            }
            None => {
                self.current = None;
                vec![AgentEvent::TurnDone {
                    cost_usd: None,
                    error: None,
                }]
            }
        }
    }

    fn cancel(&mut self) {
        self.current = None;
        self.pending_commands.clear();
    }

    fn take_commands(&mut self) -> Vec<Vec<Command>> {
        std::mem::take(&mut self.pending_commands)
    }
}
