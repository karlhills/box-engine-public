//! The Claude Code backend: runs the user's installed `claude` CLI headless
//! (`-p` with stream-JSON input and output), with `box mcp` for the open
//! project as its only tools. The editor holds no key; Claude Code uses its
//! own sign-in. The process stays up between turns; Stop kills it and the
//! next message resumes the same session.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use serde_json::{Value as Json, json};

use super::backend::{AgentBackend, AgentEvent};

/// Overrides the agent command (a path to `claude`, or a stand-in).
pub const COMMAND_ENV: &str = "BOX_ASSISTANT_COMMAND";
/// Overrides the `box` executable given to the agent as its MCP server.
pub const BOX_ENV: &str = "BOX_ASSISTANT_BOX";

/// The MCP server name; tool names arrive as `mcp__box__<tool>`.
const SERVER: &str = "box";

/// Where to find the programs the backend needs.
#[derive(Clone, Debug)]
pub struct ClaudeCodeSetup {
    pub claude: PathBuf,
    pub box_exe: PathBuf,
}

impl ClaudeCodeSetup {
    /// Finds `claude` and `box`, or says what is missing and how to fix it.
    pub fn find() -> Result<Self, String> {
        let claude = std::env::var_os(COMMAND_ENV)
            .map(PathBuf::from)
            .or_else(|| find_program("claude", &claude_dirs()))
            .ok_or_else(|| {
                "Claude Code is not installed (no `claude` command found). Install it from \
                 claude.com/claude-code and sign in, then reopen this panel."
                    .to_string()
            })?;
        let box_exe = std::env::var_os(BOX_ENV)
            .map(PathBuf::from)
            .or_else(|| {
                let here = std::env::current_exe().ok()?;
                let sibling = here.parent()?.join(exe_name("box"));
                sibling.is_file().then_some(sibling)
            })
            .or_else(|| find_program("box", &[]))
            .ok_or_else(|| {
                "The `box` command was not found next to the editor or on PATH; the assistant \
                 uses it to edit the project (build it with `cargo build -p box_cli`)."
                    .to_string()
            })?;
        Ok(Self { claude, box_exe })
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Usual install folders of `claude`, for an editor started from Finder
/// (whose PATH lacks the shell's additions).
fn claude_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".claude").join("local"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
    }
    dirs
}

fn find_program(name: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    let file = exe_name(name);
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(extra.iter().cloned())
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Option<String>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
}

/// Runs `claude` for one project.
pub struct ClaudeCodeBackend {
    setup: ClaudeCodeSetup,
    project: PathBuf,
    system_prompt: String,
    session: Option<String>,
    running: Option<Running>,
    turn_open: bool,
}

impl ClaudeCodeBackend {
    pub fn new(setup: ClaudeCodeSetup, project: &Path, system_prompt: &str) -> Self {
        Self {
            setup,
            project: project.to_path_buf(),
            system_prompt: system_prompt.to_string(),
            session: None,
            running: None,
            turn_open: false,
        }
    }

    /// The command line (for logs and tests).
    pub fn args(&self) -> Vec<String> {
        let mcp = json!({
            "mcpServers": {
                SERVER: {
                    "command": self.setup.box_exe.display().to_string(),
                    "args": ["mcp", "--project", self.project.display().to_string()],
                }
            }
        });
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--strict-mcp-config",
            // No shell or file tools: only the project's MCP tools.
            "--tools",
            "",
            "--allowedTools",
            "mcp__box",
            // The project's settings and CLAUDE.md, not the user's hooks.
            "--setting-sources",
            "project,local",
        ]
        .map(String::from)
        .to_vec();
        args.extend(["--mcp-config".into(), mcp.to_string()]);
        args.extend(["--append-system-prompt".into(), self.system_prompt.clone()]);
        if let Some(s) = &self.session {
            args.extend(["--resume".into(), s.clone()]);
        }
        args
    }

    fn start(&mut self) -> Result<(), String> {
        let mut child = Command::new(&self.setup.claude)
            .args(self.args())
            .current_dir(&self.project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", self.setup.claude.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr_pipe = child.stderr.take().ok_or("no stderr")?;
        let (tx, lines) = channel();
        std::thread::Builder::new()
            .name("assistant-stdout".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    match line {
                        Ok(l) => {
                            if tx.send(Some(l)).is_err() {
                                return;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = tx.send(None);
            })
            .map_err(|e| e.to_string())?;
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr.clone();
        std::thread::Builder::new()
            .name("assistant-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
                    if let Ok(mut t) = tail.lock() {
                        t.push_back(line);
                        if t.len() > 20 {
                            t.pop_front();
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        tracing::info!(target: "box_editor::assistant", "started {}", self.setup.claude.display());
        self.running = Some(Running {
            child,
            stdin,
            lines,
            stderr,
        });
        Ok(())
    }

    fn stop_process(&mut self) {
        if let Some(mut r) = self.running.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}

impl Drop for ClaudeCodeBackend {
    fn drop(&mut self) {
        self.stop_process();
    }
}

impl AgentBackend for ClaudeCodeBackend {
    fn name(&self) -> &str {
        "Claude Code"
    }

    fn send(&mut self, message: &str) -> Result<(), String> {
        let alive = self
            .running
            .as_mut()
            .is_some_and(|r| matches!(r.child.try_wait(), Ok(None)));
        if !alive {
            self.stop_process();
            self.start()?;
        }
        let line = json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": message }] },
        });
        let r = self.running.as_mut().ok_or("not running")?;
        writeln!(r.stdin, "{line}")
            .and_then(|_| r.stdin.flush())
            .map_err(|e| format!("could not reach Claude Code: {e}"))?;
        self.turn_open = true;
        Ok(())
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let mut ended = false;
        if let Some(r) = &self.running {
            loop {
                match r.lines.try_recv() {
                    Ok(Some(line)) => out.extend(parse_line(&line)),
                    Ok(None) | Err(TryRecvError::Disconnected) => {
                        ended = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
        }
        for e in &out {
            match e {
                AgentEvent::Session(id) => self.session = Some(id.clone()),
                AgentEvent::TurnDone { .. } => self.turn_open = false,
                _ => {}
            }
        }
        if ended {
            let tail = self
                .running
                .as_ref()
                .and_then(|r| {
                    r.stderr
                        .lock()
                        .ok()
                        .map(|t| t.iter().cloned().collect::<Vec<_>>())
                })
                .unwrap_or_default();
            self.stop_process();
            if self.turn_open {
                self.turn_open = false;
                let why = tail.last().cloned().unwrap_or_else(|| "no output".into());
                out.push(AgentEvent::Failed(format!("Claude Code stopped: {why}")));
            }
        }
        out
    }

    fn cancel(&mut self) {
        self.stop_process();
        self.turn_open = false;
    }

    fn warm_up(&mut self) {
        if self.running.is_none()
            && let Err(e) = self.start()
        {
            tracing::warn!(target: "box_editor::assistant", "{e}");
        }
    }
}

/// Events in one line of Claude Code's stream-JSON output.
pub fn parse_line(line: &str) -> Vec<AgentEvent> {
    let Ok(v) = serde_json::from_str::<Json>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            if let Some(id) = v["session_id"].as_str() {
                out.push(AgentEvent::Session(id.to_string()));
            }
        }
        Some("assistant") => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        let t = block["text"].as_str().unwrap_or_default().trim();
                        if !t.is_empty() {
                            out.push(AgentEvent::Text(t.to_string()));
                        }
                    }
                    Some("tool_use") => out.push(AgentEvent::ToolStart {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: tool_name(block["name"].as_str().unwrap_or_default()),
                        input: block["input"].clone(),
                    }),
                    _ => {}
                }
            }
        }
        Some("user") => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_result" {
                    out.push(AgentEvent::ToolDone {
                        id: block["tool_use_id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        ok: !block["is_error"].as_bool().unwrap_or(false),
                        summary: result_text(&block["content"]),
                    });
                }
            }
        }
        Some("result") => {
            let is_error = v["is_error"].as_bool().unwrap_or(false);
            out.push(AgentEvent::TurnDone {
                cost_usd: v["total_cost_usd"].as_f64(),
                error: is_error.then(|| {
                    let e = v["result"]
                        .as_str()
                        .or(v["subtype"].as_str())
                        .unwrap_or("the agent stopped on an error")
                        .to_string();
                    if e.contains("authenticate") || e.contains("login") || e.contains("log in") {
                        format!("{e}. Run `claude` in a terminal and sign in, then send again.")
                    } else {
                        e
                    }
                }),
            });
        }
        _ => {}
    }
    out
}

/// `mcp__box__apply_commands` → `apply_commands`.
fn tool_name(raw: &str) -> String {
    raw.strip_prefix(&format!("mcp__{SERVER}__"))
        .unwrap_or(raw)
        .to_string()
}

/// The first text of a tool result, at most 400 characters.
fn result_text(content: &Json) -> String {
    let text = match content {
        Json::String(s) => s.clone(),
        Json::Array(items) => items
            .iter()
            .find_map(|i| i["text"].as_str().map(str::to_string))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let mut t: String = text.chars().take(400).collect();
    if text.chars().count() > 400 {
        t.push('…');
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_turn() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s-1","tools":["mcp__box__check"]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Adding coins."},{"type":"tool_use","id":"t1","name":"mcp__box__apply_commands","input":{"commands":[]}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"{\"created\":[]}"}]}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Done","total_cost_usd":0.012,"session_id":"s-1"}"#,
            "not json",
        ];
        let events: Vec<AgentEvent> = lines.iter().flat_map(|l| parse_line(l)).collect();
        assert_eq!(
            events,
            vec![
                AgentEvent::Session("s-1".into()),
                AgentEvent::Text("Adding coins.".into()),
                AgentEvent::ToolStart {
                    id: "t1".into(),
                    name: "apply_commands".into(),
                    input: json!({ "commands": [] }),
                },
                AgentEvent::ToolDone {
                    id: "t1".into(),
                    ok: true,
                    summary: "{\"created\":[]}".into(),
                },
                AgentEvent::TurnDone {
                    cost_usd: Some(0.012),
                    error: None,
                },
            ]
        );
    }

    #[test]
    fn a_sign_in_failure_says_how_to_fix_it() {
        let e = parse_line(
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate: OAuth session expired and could not be refreshed"}"#,
        );
        let AgentEvent::TurnDone {
            error: Some(msg), ..
        } = &e[0]
        else {
            panic!("{e:?}")
        };
        assert!(msg.contains("sign in"), "{msg}");
    }

    #[test]
    fn errors_and_resume() {
        let e = parse_line(r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#);
        assert_eq!(
            e,
            vec![AgentEvent::TurnDone {
                cost_usd: None,
                error: Some("error_max_turns".into()),
            }]
        );
        let setup = ClaudeCodeSetup {
            claude: "claude".into(),
            box_exe: "/x/box".into(),
        };
        let mut b = ClaudeCodeBackend::new(setup, Path::new("/p"), "rules");
        assert!(!b.args().contains(&"--resume".to_string()));
        b.session = Some("s-9".into());
        let args = b.args();
        let i = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[i + 1], "s-9");
        let mcp: Json =
            serde_json::from_str(&args[args.iter().position(|a| a == "--mcp-config").unwrap() + 1])
                .unwrap();
        assert_eq!(
            mcp["mcpServers"]["box"]["args"],
            json!(["mcp", "--project", "/p"])
        );
    }
}
