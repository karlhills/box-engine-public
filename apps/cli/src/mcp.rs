//! `box mcp`: a Model Context Protocol server over stdio (decision D15).
//!
//! A small synchronous JSON-RPC 2.0 loop: one message per line on stdin, one
//! response per line on stdout, logs on stderr. It is dual-era:
//!
//! - **Modern** ([`PROTOCOL_VERSION`], 2026-07-28): stateless. Every request
//!   carries `_meta["io.modelcontextprotocol/protocolVersion"]` and
//!   `clientCapabilities`; `server/discover` describes the server; results
//!   carry `resultType: "complete"` and `serverInfo` in `_meta`.
//! - **Legacy** ([`LEGACY_VERSIONS`]): an `initialize` request negotiates the
//!   version for the rest of the process; `notifications/initialized` and
//!   `ping` are accepted.
//!
//! Tools are thin wrappers over the CLI library ([`crate::run`]) for project
//! files, and over the live control socket ([`box_engine::control`]) for a
//! running editor or player. The tool list is fixed: live tools are always
//! listed and answer `not_running` when nothing is running, so the list does
//! not change under the client.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use box_engine::Project;
use box_engine::box_core::glam::Vec3;
use box_engine::control::{ClientError, ControlClient, base64_encode};
use serde_json::{Map, Value as Json, json};

use crate::{
    COMMANDS_HELP, Cli, CliError, Command, ImportArgs, Output, RenderArgs, SceneCommand, scene_cmd,
};

/// The modern protocol revision implemented
/// (<https://modelcontextprotocol.io/specification/2026-07-28>).
pub const PROTOCOL_VERSION: &str = "2026-07-28";
/// Legacy revisions served after an `initialize` handshake, newest first.
pub const LEGACY_VERSIONS: [&str; 2] = ["2025-11-25", "2025-06-18"];
/// Largest accepted message line on stdin.
pub const MAX_MESSAGE_BYTES: usize = 64 << 20;

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

// JSON-RPC and MCP error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// Connection timeout for the control socket.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

const INSTRUCTIONS: &str = "box-engine game engine tools. \
Project tools (check, registry, scene_list, scene_show, scene_apply, render, walk_route, import, playtest, read_asset, write_asset) read and edit project files and need no running editor. \
Live tools (status, editor_context, undo_turn, world_query, apply_commands, screenshot, play, stop, pause, step_frames, send_input, teleport, record_start, record_stop, read_log, frame_stats, audio_levels, audio_solo, audio_mute; playtest with live=true) drive a running editor or player that enabled its control socket: an editor started with `box-editor <project> --control` (or with File ▸ Allow Agent Control turned on), a player started with `box-player <project> --control` or `box run <project> --control`; without one they fail with error code `not_running`. \
Paths are relative to the project root (the directory holding box.project.json); `scene` defaults to the project's start scene. \
Call `registry` first to learn component names, field paths, ranges and units; then edit with `scene_apply` (files) or `apply_commands` (live, undoable in the editor). \
For repeatable playtests: record_start (restarts the game from its scene), drive it with send_input (or let a person play), record_stop with save=<name> writes playtests/<name>.playtest.json; then call playtest after every change and compare `passed`, the checks and the fingerprint. \
Errors are tool results with isError=true and a JSON body {\"error\": {\"code\", \"message\", ...}} whose code is stable.";

/// `box mcp --help` text after the options: how to register the server
/// with an MCP client and how to enable the live tools.
pub fn registration_help() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/absolute/path/to/box".into());
    let exe = box_engine::player::shell_line(&[exe]);
    format!(
        "Register with an MCP client (use absolute paths), for example Claude Code:
  claude mcp add box -- {exe} mcp --project /absolute/path/to/my-game
Other clients: command `{exe}`, arguments `mcp --project <project dir>`.

Project tools (check, registry, scene_list, scene_show, scene_apply, render, import, playtest) work on files.
Live tools (status, world_query, apply_commands, screenshot, play, stop, ...) need a running
editor or player with live control on:
  box-editor <project> --control   (or File ▸ Allow Agent Control in the editor)
  box-player <project> --control   (or box run <project> --control)"
    )
}

fn server_info() -> Json {
    json!({
        "name": "box",
        "title": "box-engine",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Serve MCP on stdin/stdout until stdin closes.
pub fn serve_stdio(project: Option<&Path>) -> Result<(), CliError> {
    let project = match project {
        Some(p) => Some(std::path::absolute(p).map_err(|e| CliError::io(p, e))?),
        None => None,
    };
    tracing::info!(target: "box::mcp", "MCP server on stdio (protocol {PROTOCOL_VERSION})");
    let mut server = McpServer::new(project);
    let stdin = io::stdin();
    let stdout = io::stdout();
    server
        .serve(stdin.lock(), stdout.lock())
        .map_err(|e| CliError::new("io", format!("MCP stdio: {e}")))
}

/// The protocol state: the default project and the negotiated legacy version.
pub struct McpServer {
    project: Option<PathBuf>,
    legacy: Option<&'static str>,
}

/// A JSON-RPC error.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Json>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

fn response(id: Json, result: Result<Json, RpcError>) -> Json {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(e) => {
            let mut error = json!({ "code": e.code, "message": e.message });
            if let Some(data) = e.data {
                error["data"] = data;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": error })
        }
    }
}

impl McpServer {
    /// `project`: the default project directory for tools (absolute), else
    /// the project containing the current directory.
    pub fn new(project: Option<PathBuf>) -> Self {
        Self {
            project,
            legacy: None,
        }
    }

    /// Read messages until end of input, writing one response line per request.
    pub fn serve(&mut self, mut input: impl BufRead, mut output: impl Write) -> io::Result<()> {
        loop {
            let mut line = Vec::new();
            let n = (&mut input)
                .take(MAX_MESSAGE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if n == 0 {
                return Ok(());
            }
            let reply = if line.len() > MAX_MESSAGE_BYTES && line.last() != Some(&b'\n') {
                // Skip the rest of the oversized line.
                let mut sink = Vec::new();
                while input.read_until(b'\n', &mut sink)? > 0 && sink.last() != Some(&b'\n') {
                    sink.clear();
                }
                Some(response(
                    Json::Null,
                    Err(RpcError::new(
                        INVALID_REQUEST,
                        format!("message exceeds {MAX_MESSAGE_BYTES} bytes"),
                    )),
                ))
            } else {
                self.handle_bytes(&line)
            };
            if let Some(reply) = reply {
                let mut bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
                bytes.push(b'\n');
                output.write_all(&bytes)?;
                output.flush()?;
            }
        }
    }

    /// One message to its response (`None` for notifications and blank lines).
    pub fn handle(&mut self, message: &str) -> Option<Json> {
        self.handle_bytes(message.as_bytes())
    }

    fn handle_bytes(&mut self, message: &[u8]) -> Option<Json> {
        let message = message.trim_ascii();
        if message.is_empty() {
            return None;
        }
        let msg: Json = match serde_json::from_slice(message) {
            Ok(v) => v,
            Err(e) => {
                return Some(response(
                    Json::Null,
                    Err(RpcError::new(PARSE_ERROR, format!("parse error: {e}"))),
                ));
            }
        };
        let Some(obj) = msg.as_object() else {
            return Some(response(
                Json::Null,
                Err(RpcError::new(
                    INVALID_REQUEST,
                    "expected one JSON-RPC request object (batches are not supported)",
                )),
            ));
        };
        let id = obj.get("id").cloned();
        if obj.get("jsonrpc") != Some(&json!("2.0")) {
            return Some(response(
                id.unwrap_or(Json::Null),
                Err(RpcError::new(INVALID_REQUEST, "`jsonrpc` must be \"2.0\"")),
            ));
        }
        let Some(method) = obj.get("method").and_then(Json::as_str) else {
            // A response from the client: we never send requests, so ignore it.
            return None;
        };
        let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = id else {
            // Notifications (initialized, cancelled, ...) need no answer.
            tracing::debug!(target: "box::mcp", "notification {method}");
            return None;
        };
        if !(id.is_string() || id.is_number()) {
            return Some(response(
                Json::Null,
                Err(RpcError::new(
                    INVALID_REQUEST,
                    "`id` must be a string or an integer",
                )),
            ));
        }
        if !params.is_object() {
            return Some(response(
                id,
                Err(RpcError::new(INVALID_PARAMS, "`params` must be an object")),
            ));
        }
        Some(response(id, self.request(method, &params)))
    }

    fn request(&mut self, method: &str, params: &Json) -> Result<Json, RpcError> {
        if method == "initialize" {
            return Ok(self.initialize(params));
        }
        let meta = &params["_meta"];
        let modern = match meta.get(META_VERSION) {
            Some(version) => {
                let version = version.as_str().unwrap_or_default();
                if version != PROTOCOL_VERSION {
                    let mut e =
                        RpcError::new(UNSUPPORTED_PROTOCOL_VERSION, "Unsupported protocol version");
                    e.data = Some(json!({
                        "supported": [PROTOCOL_VERSION],
                        "requested": meta[META_VERSION],
                    }));
                    return Err(e);
                }
                if !meta.get(META_CAPABILITIES).is_some_and(Json::is_object) {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        format!("`_meta[\"{META_CAPABILITIES}\"]` is required on every request"),
                    ));
                }
                true
            }
            None if self.legacy.is_some() || method == "ping" => false,
            None => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!(
                        "missing `_meta[\"{META_VERSION}\"]`: send it with every request \
                         (protocol {PROTOCOL_VERSION}) or start with `initialize` (protocol {})",
                        LEGACY_VERSIONS.join(" or ")
                    ),
                ));
            }
        };
        let mut result = match method {
            "ping" => json!({}),
            "server/discover" => json!({
                "supportedVersions": [PROTOCOL_VERSION],
                "capabilities": { "tools": { "listChanged": false } },
                "instructions": INSTRUCTIONS,
            }),
            "tools/list" => {
                let mut r = json!({ "tools": tool_definitions() });
                if modern {
                    r["ttlMs"] = json!(3_600_000);
                    r["cacheScope"] = json!("public");
                }
                r
            }
            "tools/call" => self.tools_call(params)?,
            other => {
                return Err(RpcError::new(
                    METHOD_NOT_FOUND,
                    format!("method not found: {other}"),
                ));
            }
        };
        if modern && let Some(obj) = result.as_object_mut() {
            obj.insert("resultType".into(), json!("complete"));
            obj.insert("_meta".into(), json!({ META_SERVER_INFO: server_info() }));
        }
        Ok(result)
    }

    fn initialize(&mut self, params: &Json) -> Json {
        let requested = params["protocolVersion"].as_str().unwrap_or_default();
        let version = LEGACY_VERSIONS
            .iter()
            .copied()
            .find(|v| *v == requested)
            .unwrap_or(LEGACY_VERSIONS[0]);
        self.legacy = Some(version);
        tracing::info!(
            target: "box::mcp",
            "initialize: client {} asked for {requested}, using {version}",
            params["clientInfo"]["name"].as_str().unwrap_or("?")
        );
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": server_info(),
            "instructions": INSTRUCTIONS,
        })
    }

    fn tools_call(&self, params: &Json) -> Result<Json, RpcError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "`name` must be a string"))?;
        let args = match params.get("arguments") {
            None | Some(Json::Null) => Map::new(),
            Some(Json::Object(m)) => m.clone(),
            Some(_) => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "`arguments` must be an object",
                ));
            }
        };
        let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("Unknown tool: {name}"),
            ));
        };
        tracing::debug!(target: "box::mcp", "tools/call {name}");
        Ok(match self.call(tool, Args::new(name, args)) {
            Ok(out) => out.into_result(),
            Err(err) => tool_error(&err),
        })
    }

    fn call(&self, tool: &Tool, mut args: Args) -> Result<ToolOutput, Json> {
        let allowed: Vec<&str> = tool.properties.iter().map(|p| p.0).collect();
        args.reject_unknown(&allowed)?;
        if tool.name == "registry" {
            let name = args.str("name")?;
            // Outside a project: the engine's types alone.
            let project = match args.str("project")? {
                Some(p) => Some(self.project_root(Some(p))?),
                None => self.project_root(None).ok(),
            };
            return registry(name, project);
        }
        let project = self.project_root(args.str("project")?)?;
        match tool.name {
            "check" => cli(Command::Check {
                project: Some(project),
            }),
            "scene_list" => cli(Command::Scene(SceneCommand::List {
                project: Some(project),
            })),
            "scene_show" => cli(Command::Scene(SceneCommand::Show {
                scene: scene_path(&project, args.str("scene")?),
                entity: args.str("entity")?,
            })),
            "scene_apply" => {
                let commands = args.required("commands")?;
                let dry_run = args.bool("dry_run")?.unwrap_or(false);
                let scene = scene_path(&project, args.str("scene")?);
                scene_cmd::apply_text(&scene, &commands.to_string(), "arguments.commands", dry_run)
                    .map(ToolOutput::from_cli)
                    .map_err(|e| e.to_json())
            }
            "render" => render(&project, &mut args),
            "walk_route" => {
                let scene = scene_path(&project, args.str("scene")?);
                let mut fields = args.map.clone();
                fields.remove("scene");
                fields.remove("project");
                let spec: box_engine::route::RouteSpec =
                    serde_json::from_value(Json::Object(fields))
                        .map_err(|e| args.invalid(e.to_string()))?;
                crate::walk_cmd::walk_spec(&scene, &spec)
                    .map(ToolOutput::from_cli)
                    .map_err(|e| e.to_json())
            }
            "nav_path" => {
                let scene = scene_path(&project, args.str("scene")?);
                let from = args
                    .point("from")?
                    .ok_or_else(|| args.invalid("`from` is required: [x, y, z]".to_string()))?;
                let to = args
                    .point("to")?
                    .ok_or_else(|| args.invalid("`to` is required: [x, y, z]".to_string()))?;
                crate::nav_cmd::nav_between(&scene, from, to)
                    .map(ToolOutput::from_cli)
                    .map_err(|e| e.to_json())
            }
            "import" => {
                let file = args.str("file")?.unwrap_or_default();
                let file = std::path::absolute(&file)
                    .map_err(|e| CliError::io(Path::new(&file), e).to_json())?;
                cli(Command::Import(ImportArgs {
                    file,
                    project: Some(project),
                    to: args.str("to")?,
                }))
            }
            "status" | "play" | "stop" | "pause" | "frame_stats" | "editor_context"
            | "undo_turn" => live(&project, tool.name, json!({}), Duration::from_secs(30)),
            "read_asset" => {
                let path = args.str("path")?.unwrap_or_default();
                box_engine::agent_files::read(&project, &path)
                    .map(|json| ToolOutput { json, image: None })
                    .map_err(|e| e.to_json())
            }
            "write_asset" => {
                let path = args.str("path")?.unwrap_or_default();
                let text = args.str("text")?.unwrap_or_default();
                box_engine::agent_files::write(&project, &path, &text)
                    .map(|json| ToolOutput { json, image: None })
                    .map_err(|e| e.to_json())
            }
            "world_query" => {
                let mut p = Map::new();
                for key in ["name", "uid", "component"] {
                    if let Some(v) = args.str(key)? {
                        p.insert(key.into(), json!(v));
                    }
                }
                if let Some(v) = args.int("limit", 1, 10_000)? {
                    p.insert("limit".into(), json!(v));
                }
                for key in ["include_components", "include_resources"] {
                    if let Some(v) = args.bool(key)? {
                        p.insert(key.into(), json!(v));
                    }
                }
                live(
                    &project,
                    "world_query",
                    Json::Object(p),
                    Duration::from_secs(30),
                )
            }
            "apply_commands" => {
                let commands = args.required("commands")?;
                let dry_run = args.bool("dry_run")?.unwrap_or(false);
                let save = args.bool("save")?.unwrap_or(false);
                live(
                    &project,
                    "apply_commands",
                    json!({ "commands": commands, "dry_run": dry_run, "save": save }),
                    Duration::from_secs(30),
                )
            }
            "screenshot" => {
                let mut p = Map::new();
                if let Some(w) = args.int("width", 1, 8192)? {
                    p.insert("width".into(), json!(w));
                }
                if let Some(h) = args.int("height", 1, 8192)? {
                    p.insert("height".into(), json!(h));
                }
                if let Some(eye) = args.point("eye")? {
                    p.insert("eye".into(), json!(eye.to_array()));
                }
                if let Some(look_at) = args.point("look_at")? {
                    p.insert("look_at".into(), json!(look_at.to_array()));
                }
                if let Some(fov) = args.float("fov", 1.0, 170.0)? {
                    p.insert("fov".into(), json!(fov));
                }
                let mut out = live(
                    &project,
                    "screenshot",
                    Json::Object(p),
                    Duration::from_secs(60),
                )?;
                let data = out.json["png_base64"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if let Some(obj) = out.json.as_object_mut() {
                    obj.remove("png_base64");
                }
                out.image = Some(data);
                Ok(out)
            }
            "send_input" => {
                let mut p = Map::new();
                p.insert("action".into(), args.required("action")?);
                if let Some(f) = args.int("frames", 1, 600)? {
                    p.insert("frames".into(), json!(f));
                }
                if let Some(v) = args.map.get("value").filter(|v| !v.is_null()) {
                    p.insert("value".into(), v.clone());
                }
                live(
                    &project,
                    "send_input",
                    Json::Object(p),
                    Duration::from_secs(10),
                )
            }
            "teleport" => {
                let mut p = Map::new();
                p.insert("entity".into(), args.required("entity")?);
                let position = args
                    .point("position")?
                    .ok_or_else(|| args.invalid("`position` is required".into()))?;
                p.insert("position".into(), json!(position.to_array()));
                if let Some(yaw) = args.float("yaw", -360.0, 360.0)? {
                    p.insert("yaw".into(), json!(yaw));
                }
                live(
                    &project,
                    "teleport",
                    Json::Object(p),
                    Duration::from_secs(10),
                )
            }
            "record_start" => {
                let mut p = Map::new();
                if let Some(v) = args.bool("restart")? {
                    p.insert("restart".into(), json!(v));
                }
                if let Some(v) = args.str("player")? {
                    p.insert("player".into(), json!(v));
                }
                if let Some(v) = args.int("seed", 0, u64::from(u32::MAX))? {
                    p.insert("seed".into(), json!(v));
                }
                live(
                    &project,
                    "record_start",
                    Json::Object(p),
                    Duration::from_secs(120),
                )
            }
            "record_stop" => {
                let mut p = Map::new();
                for key in ["save", "description"] {
                    if let Some(v) = args.str(key)? {
                        p.insert(key.into(), json!(v));
                    }
                }
                live(
                    &project,
                    "record_stop",
                    Json::Object(p),
                    Duration::from_secs(30),
                )
            }
            "playtest" => playtest(&project, &mut args),
            "step_frames" => {
                let frames = args.int("frames", 1, 10_000)?.unwrap_or(1);
                live(
                    &project,
                    "step_frames",
                    json!({ "frames": frames }),
                    Duration::from_secs(120),
                )
            }
            "read_log" => {
                let mut p = Map::new();
                if let Some(v) = args.int("since", 0, u64::MAX)? {
                    p.insert("since".into(), json!(v));
                }
                if let Some(v) = args.int("limit", 1, 10_000)? {
                    p.insert("limit".into(), json!(v));
                }
                live(
                    &project,
                    "read_log",
                    Json::Object(p),
                    Duration::from_secs(30),
                )
            }
            "audio_levels" => {
                let mut p = Map::new();
                if let Some(t) = args.str("target")? {
                    p.insert("target".into(), json!(t));
                }
                live(
                    &project,
                    "audio_levels",
                    Json::Object(p),
                    Duration::from_secs(10),
                )
            }
            "audio_solo" | "audio_mute" => {
                let mut p = Map::new();
                if let Some(t) = args.str("target")? {
                    p.insert("target".into(), json!(t));
                }
                for key in ["on", "clear"] {
                    if let Some(v) = args.bool(key)? {
                        p.insert(key.into(), json!(v));
                    }
                }
                live(
                    &project,
                    tool.name,
                    Json::Object(p),
                    Duration::from_secs(10),
                )
            }
            other => Err(error_json(
                "internal",
                format!("tool `{other}` has no handler"),
            )),
        }
    }

    /// The project root for a call: the `project` argument (relative to the
    /// server's working directory), else `--project`, else the current
    /// directory. Any path inside a project resolves to its root.
    fn project_root(&self, arg: Option<String>) -> Result<PathBuf, Json> {
        let base = match (arg, &self.project) {
            (Some(p), _) => {
                std::path::absolute(&p).map_err(|e| CliError::io(Path::new(&p), e).to_json())?
            }
            (None, Some(p)) => p.clone(),
            (None, None) => {
                std::env::current_dir().map_err(|e| CliError::io(Path::new("."), e).to_json())?
            }
        };
        Project::find_root(&base).ok_or_else(|| {
            CliError::new(
                "not_a_project",
                format!(
                    "{} is not inside a box project (no box.project.json); pass `project` or start `box mcp --project <dir>`",
                    base.display()
                ),
            )
            .file(&base)
            .to_json()
        })
    }
}

/// A scene argument: relative to the project root; the root itself (its start
/// scene) when absent.
fn scene_path(project: &Path, scene: Option<String>) -> PathBuf {
    match scene {
        Some(s) => project.join(s),
        None => project.to_path_buf(),
    }
}

fn error_json(code: &str, message: impl Into<String>) -> Json {
    json!({ "error": { "code": code, "message": message.into() } })
}

/// A successful tool result: JSON plus an optional base64 PNG.
struct ToolOutput {
    json: Json,
    image: Option<String>,
}

impl ToolOutput {
    fn from_cli(out: Output) -> Self {
        Self {
            json: out.json,
            image: None,
        }
    }

    fn into_result(self) -> Json {
        // structuredContent must be an object in the 2025-06-18 revision.
        let structured = if self.json.is_object() {
            self.json
        } else {
            json!({ "result": self.json })
        };
        let mut content = Vec::new();
        if let Some(data) = self.image {
            content.push(json!({ "type": "image", "data": data, "mimeType": "image/png" }));
        }
        content.push(json!({ "type": "text", "text": structured.to_string() }));
        json!({ "content": content, "structuredContent": structured, "isError": false })
    }
}

fn tool_error(error: &Json) -> Json {
    json!({
        "content": [{ "type": "text", "text": error.to_string() }],
        "structuredContent": error,
        "isError": true,
    })
}

fn cli(command: Command) -> Result<ToolOutput, Json> {
    crate::run(&Cli {
        json: true,
        command,
    })
    .map(ToolOutput::from_cli)
    .map_err(|e| e.to_json())
}

fn registry(name: Option<String>, project: Option<PathBuf>) -> Result<ToolOutput, Json> {
    let mut out = cli(Command::Registry { project })?;
    let Some(name) = name else {
        return Ok(out);
    };
    for list in ["components", "resources"] {
        if let Some(types) = out.json[list].as_array_mut() {
            types.retain(|t| t["name"] == name.as_str());
        }
    }
    let found = ["components", "resources"]
        .iter()
        .any(|l| out.json[*l].as_array().is_some_and(|a| !a.is_empty()));
    if found {
        if let Some(obj) = out.json.as_object_mut() {
            obj.remove("presets");
        }
        return Ok(out);
    }
    // A preset (`pickup`, `Pickup`): its entities and what to pass add_preset.
    let preset = out.json["presets"].as_array().and_then(|ps| {
        ps.iter()
            .find(|p| {
                p["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&name))
                    || p["title"]
                        .as_str()
                        .is_some_and(|t| t.eq_ignore_ascii_case(&name))
            })
            .cloned()
    });
    if let Some(p) = preset {
        let use_it = format!(
            "{{\"op\": \"add_preset\", \"preset\": \"{}\", \"at\": [x, y, z]}}",
            p["name"].as_str().unwrap_or_default()
        );
        return Ok(ToolOutput {
            json: json!({ "preset": p, "place_with": use_it }),
            image: None,
        });
    }
    // A field type (`Condition`, `InteractAction`): its JSON form.
    let registry = box_engine::scene_glue::shared_engine_registry();
    let path = registry
        .type_registry()
        .get_with_short_type_path(&name)
        .map(|r| r.type_info().type_path().to_string());
    if let Some(path) = path {
        return Ok(ToolOutput {
            json: json!({
                "type": name,
                "json_form": crate::friendly_type(&path, &registry),
                "note": "a field type, not a component: it appears inside components (see the fields that use it)",
            }),
            image: None,
        });
    }
    Err(error_json(
        "unknown_type",
        format!(
            "no registered component, resource, preset or field type named `{name}` (registry with no name lists them all)"
        ),
    ))
}

static RENDER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `playtest`: replay headless through `box playtest`, or with `live` in the
/// running editor or player (restarted from its scene), polling
/// `playtest_status` until every step ran.
fn playtest(project: &Path, args: &mut Args) -> Result<ToolOutput, Json> {
    let file = args.str("file")?;
    let inline = args.map.get("playtest").filter(|v| !v.is_null()).cloned();
    let live_run = args.bool("live")?.unwrap_or(false);
    let capture_every = args.int("capture_every", 1, 100_000)?.map(|n| n as u32);
    if file.is_some() && inline.is_some() {
        return Err(args.invalid("give `file` or `playtest`, not both".into()));
    }
    if live_run {
        if capture_every.is_some() {
            return Err(args.invalid("`capture_every` is for headless replays (live=false)".into()));
        }
        let mut p = Map::new();
        match (file, inline) {
            (Some(f), None) => p.insert("file".into(), json!(f)),
            (None, Some(v)) => p.insert("playtest".into(), v),
            _ => return Err(args.invalid("give `file` (or `playtest`) for a live replay".into())),
        };
        let started = live(
            project,
            "playtest",
            Json::Object(p),
            Duration::from_secs(120),
        )?;
        let frames = started.json["total_frames"].as_u64().unwrap_or(0);
        // Real time, with room for slow frames and a paused host.
        let deadline = Instant::now() + Duration::from_secs(frames / 15 + 60);
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let status = live(
                project,
                "playtest_status",
                json!({}),
                Duration::from_secs(30),
            )?;
            if status.json["finished"] == true {
                return Ok(status);
            }
            if Instant::now() > deadline {
                return Err(error_json(
                    "timeout",
                    format!(
                        "the live playtest did not finish in time ({} of {frames} frames); is the game paused? playtest_status reads its progress",
                        status.json["frames"]
                    ),
                ));
            }
        }
    }
    let (file, temporary) = match (file, inline) {
        (Some(f), None) => (Some(f), None),
        (None, Some(v)) => {
            let parsed = box_engine::playtest::Playtest::from_json(v)
                .map_err(|e| error_json(e.code, e.message))?;
            let n = RENDER_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "box-mcp-inline-{}-{n}.playtest.json",
                std::process::id()
            ));
            box_engine::playtest::write(&path, &parsed)
                .map_err(|e| error_json(e.code, e.message))?;
            (Some(path.display().to_string()), Some(path))
        }
        (None, None) => (None, None),
        _ => unreachable!("checked above"),
    };
    let result = cli(Command::Playtest(crate::PlaytestArgs {
        project: Some(project.to_path_buf()),
        file,
        capture_every,
        capture_dir: None,
        size: None,
    }));
    if let Some(path) = temporary {
        let _ = std::fs::remove_file(path);
    }
    result
}

fn render(project: &Path, args: &mut Args) -> Result<ToolOutput, Json> {
    let width = args.int("width", 1, 4096)?.unwrap_or(1280) as u32;
    let height = args.int("height", 1, 4096)?.unwrap_or(720) as u32;
    let frames = args.int("frames", 1, 600)?.unwrap_or(4) as u32;
    let (out, temporary) = match args.str("out")? {
        Some(p) => (project.join(p), false),
        None => {
            let n = RENDER_COUNTER.fetch_add(1, Ordering::Relaxed);
            let name = format!("box-mcp-render-{}-{n}.png", std::process::id());
            (std::env::temp_dir().join(name), true)
        }
    };
    let eye = args.point("eye")?;
    let look_at = args.point("look_at")?;
    if eye.is_some() != look_at.is_some() {
        return Err(args.invalid("`eye` and `look_at` go together".into()));
    }
    let camera = args.str("camera")?;
    let overview = args.bool("overview")?.unwrap_or(false);
    if [camera.is_some(), eye.is_some(), overview]
        .iter()
        .filter(|b| **b)
        .count()
        > 1
    {
        return Err(args.invalid("give one of `camera`, `eye`/`look_at` and `overview`".into()));
    }
    let result = cli(Command::Render(RenderArgs {
        scene: scene_path(project, args.str("scene")?),
        out: out.clone(),
        size: (width, height),
        camera,
        eye,
        look_at,
        fov: args.float("fov", 1.0, 170.0)?.map(|f| f as f32),
        overview,
        views: None,
        nav: args.bool("nav")?.unwrap_or(false),
        frames,
    }));
    let png = std::fs::read(&out);
    if temporary {
        let _ = std::fs::remove_file(&out);
    }
    let mut output = result?;
    let png = png.map_err(|e| CliError::io(&out, e).to_json())?;
    if temporary && let Some(obj) = output.json.as_object_mut() {
        obj.remove("out");
    }
    output.image = Some(base64_encode(&png));
    Ok(output)
}

/// Call a method on the project's running editor or player.
fn live(project: &Path, method: &str, params: Json, timeout: Duration) -> Result<ToolOutput, Json> {
    let remote = |e: ClientError| json!({ "error": e.to_control_error().to_json() });
    let mut client = ControlClient::connect_project(project, CONNECT_TIMEOUT).map_err(remote)?;
    client.set_timeout(timeout).map_err(remote)?;
    let json = client.call(method, params).map_err(remote)?;
    Ok(ToolOutput { json, image: None })
}

// --------------------------------------------------------- arguments ----

/// Tool arguments with typed accessors; errors are `invalid_arguments` results.
struct Args {
    tool: String,
    map: Map<String, Json>,
}

impl Args {
    fn new(tool: &str, map: Map<String, Json>) -> Self {
        Self {
            tool: tool.to_string(),
            map,
        }
    }

    fn invalid(&self, message: String) -> Json {
        error_json("invalid_arguments", format!("{}: {message}", self.tool))
    }

    fn reject_unknown(&self, allowed: &[&str]) -> Result<(), Json> {
        match self.map.keys().find(|k| !allowed.contains(&k.as_str())) {
            Some(k) => Err(self.invalid(format!(
                "unknown argument `{k}` (accepted: {})",
                if allowed.is_empty() {
                    "none".to_string()
                } else {
                    allowed.join(", ")
                }
            ))),
            None => Ok(()),
        }
    }

    fn str(&self, key: &str) -> Result<Option<String>, Json> {
        match self.map.get(key) {
            None | Some(Json::Null) => Ok(None),
            Some(Json::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(self.invalid(format!("`{key}` must be a string"))),
        }
    }

    fn bool(&self, key: &str) -> Result<Option<bool>, Json> {
        match self.map.get(key) {
            None | Some(Json::Null) => Ok(None),
            Some(Json::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(self.invalid(format!("`{key}` must be a boolean"))),
        }
    }

    fn int(&self, key: &str, min: u64, max: u64) -> Result<Option<u64>, Json> {
        match self.map.get(key) {
            None | Some(Json::Null) => Ok(None),
            Some(v) => match v.as_u64().filter(|n| (min..=max).contains(n)) {
                Some(n) => Ok(Some(n)),
                None => Err(self.invalid(format!("`{key}` must be an integer in {min}..={max}"))),
            },
        }
    }

    fn float(&self, key: &str, min: f64, max: f64) -> Result<Option<f64>, Json> {
        match self.map.get(key) {
            None | Some(Json::Null) => Ok(None),
            Some(v) => match v.as_f64().filter(|n| (min..=max).contains(n)) {
                Some(n) => Ok(Some(n)),
                None => Err(self.invalid(format!("`{key}` must be a number in {min}..={max}"))),
            },
        }
    }

    /// A point `[x, y, z]`.
    fn point(&self, key: &str) -> Result<Option<Vec3>, Json> {
        match self.map.get(key) {
            None | Some(Json::Null) => Ok(None),
            Some(Json::Array(items)) if items.len() == 3 => {
                let mut v = [0.0f32; 3];
                for (out, item) in v.iter_mut().zip(items) {
                    *out =
                        item.as_f64().filter(|n| n.is_finite()).ok_or_else(|| {
                            self.invalid(format!("`{key}` must be [x, y, z] numbers"))
                        })? as f32;
                }
                Ok(Some(Vec3::from_array(v)))
            }
            Some(_) => Err(self.invalid(format!("`{key}` must be [x, y, z]"))),
        }
    }

    fn required(&self, key: &str) -> Result<Json, Json> {
        match self.map.get(key) {
            None | Some(Json::Null) => Err(self.invalid(format!("`{key}` is required"))),
            Some(v) => Ok(v.clone()),
        }
    }
}

// ------------------------------------------------------------- tools ----

/// A tool: name, title, description, input properties (name, JSON Schema),
/// required properties and whether it only reads.
struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    properties: &'static [(&'static str, &'static str)],
    required: &'static [&'static str],
    read_only: bool,
}

const PROJECT: (&str, &str) = (
    "project",
    r#"{"type": "string", "description": "Project directory or any path inside it, absolute or relative to the server's working directory. Default: the --project given to `box mcp`, else the current directory."}"#,
);
const SCENE: (&str, &str) = (
    "scene",
    r#"{"type": "string", "description": "Scene file relative to the project root, e.g. \"assets/scenes/main.scene.json\". Default: the project's start scene."}"#,
);
/// The `commands` property; its description is [`COMMANDS_HELP`], added by
/// [`tool_definitions`].
const COMMANDS: (&str, &str) = (
    "commands",
    r#"{"anyOf": [{"type": "object"}, {"type": "array", "items": {"type": "object"}}]}"#,
);
const DRY_RUN: (&str, &str) = (
    "dry_run",
    r#"{"type": "boolean", "description": "Validate and report without changing anything. Default false."}"#,
);

const TOOLS: &[Tool] = &[
    Tool {
        name: "check",
        title: "Check project",
        description: "Validate a box project: parse box.project.json and every *.scene.json under assets/, import every asset file and resolve every asset reference in every scene, and report problems with file, entity and a stable code (start_scene_missing, scene_parse, unknown_component with a did-you-mean `suggestion`, asset_import_failed, shader_header, shader_invalid, asset_not_found, log; in a native project also registry_missing/registry_stale with a `hint` to refresh the game registry, invalid_component, invalid_settings, and module_disabled with the `module` (box_engine feature) for types and settings files of engine modules the game leaves out). The result has ok, errors, warnings, scenes, assets and problems; ok=false is still a successful call.",
        properties: &[PROJECT],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "registry",
        title: "List component and resource types",
        description: "List the registered component and resource types: name, docs, and fields with type, range, unit, default and docs. Use the names and field paths in scene_apply/apply_commands (`path` is Component.field, e.g. \"StandardMaterial.roughness\"). In a native project the game's own types are included (marked \"game\": true, from <project>/.box/registry.json, which the game's editor and player write) and scene_apply validates and edits them like engine types. Pass `name` to get one type, e.g. {\"name\": \"Transform\"}; a preset name (\"pickup\") returns the preset and how to place it, a field type (\"Condition\") its JSON form.",
        properties: &[
            (
                "name",
                r#"{"type": "string", "description": "Return only this component or resource type (exact name, e.g. \"Primitive\")."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "scene_list",
        title: "List scenes",
        description: "List the project's scene files (paths relative to the project root) with entity counts, and the start scene.",
        properties: &[PROJECT],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "scene_show",
        title: "Show a scene file",
        description: "Show a scene file's resources and entities in hierarchy order: uid, name, parent, children and component values in scene-file JSON. Pass `entity` (uid or unique name) for one entity. Example: {\"scene\": \"assets/scenes/main.scene.json\", \"entity\": \"Cube\"}.",
        properties: &[
            SCENE,
            (
                "entity",
                r#"{"type": "string", "description": "Show only this entity: a uid or a unique entity name."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "scene_apply",
        title: "Edit a scene file",
        description: "Apply edit commands to a scene file as one all-or-nothing transaction with the editor's validation, then save it atomically (not while the editor has the scene open: use apply_commands for a running editor). Returns the uids of spawned/duplicated entities in `created`. On error nothing is written and the error names the failing command index, entity and path. Example commands: [{\"op\": \"spawn\", \"name\": \"Ball\", \"components\": {\"Primitive\": {\"shape\": {\"Sphere\": {\"radius\": 0.5, \"sectors\": 32, \"stacks\": 16}}, \"cast_shadows\": true}}}, {\"op\": \"set\", \"entity\": \"Ball\", \"path\": \"Transform.translation\", \"value\": [0.0, 1.0, 0.0]}].",
        properties: &[COMMANDS, DRY_RUN, SCENE, PROJECT],
        required: &["commands"],
        read_only: false,
    },
    Tool {
        name: "render",
        title: "Render a scene file",
        description: "Render a scene file headlessly on the GPU and return the PNG as an image, plus metadata (camera used, objects, triangles). The view: `eye` + `look_at` (a temporary camera; the scene is not changed), `overview` (the whole scene from above at an angle), `camera` (uid or name), else the scene's first active camera, else an automatic camera framing the scene. Needs a GPU (error code no_gpu otherwise). Example: {\"eye\": [12, 6, 14], \"look_at\": [0, 1, 0], \"width\": 960, \"height\": 540}.",
        properties: &[
            SCENE,
            (
                "width",
                r#"{"type": "integer", "minimum": 1, "maximum": 4096, "description": "Image width in pixels. Default 1280."}"#,
            ),
            (
                "height",
                r#"{"type": "integer", "minimum": 1, "maximum": 4096, "description": "Image height in pixels. Default 720."}"#,
            ),
            (
                "camera",
                r#"{"type": "string", "description": "Camera entity (uid or unique name)."}"#,
            ),
            (
                "eye",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Render from this point [x, y, z] (metres) with a temporary camera; needs look_at."}"#,
            ),
            (
                "look_at",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "The point [x, y, z] the eye camera looks at."}"#,
            ),
            (
                "fov",
                r#"{"type": "number", "minimum": 1, "maximum": 170, "description": "Vertical field of view in degrees for eye and overview."}"#,
            ),
            (
                "overview",
                r#"{"type": "boolean", "description": "Frame the whole scene from above at an angle."}"#,
            ),
            (
                "nav",
                r#"{"type": "boolean", "description": "Draw the navmesh (where enemies can walk) over the image."}"#,
            ),
            (
                "frames",
                r#"{"type": "integer", "minimum": 1, "maximum": 600, "description": "Frames to simulate before the capture. Default 4."}"#,
            ),
            (
                "out",
                r#"{"type": "string", "description": "Also save the PNG here (relative to the project root). Default: not saved."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "walk_route",
        title: "Walk a route",
        description: "Check that a route is walkable: a probe character (the scene's first KinematicCharacter's size, else the default) walks the waypoints in order against the scene's colliders, without playing or changing anything. Per leg: reached or stuck_at (where it stopped making progress), time, max_step_up (biggest rise of the ground between two steps: kerbs and stairs show their height), falls and max_drop (drops higher than the step height), steepest_slope (degrees). A failed leg is reported and the walk goes on from its waypoint. Waypoints are near the ground (within 2 m above it). Example: {\"waypoints\": [[0, 0, 10], [0, 0, -2], [12, 0, -3]]}.",
        properties: &[
            SCENE,
            (
                "waypoints",
                r#"{"type": "array", "minItems": 2, "items": {"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3}, "description": "Points [x, y, z] to walk to, in order; the walk starts on the ground at the first."}"#,
            ),
            (
                "speed",
                r#"{"type": "number", "description": "Walking speed, m/s. Default 4."}"#,
            ),
            (
                "reach",
                r#"{"type": "number", "description": "How close (horizontally) counts as reaching a waypoint, metres. Default 0.5."}"#,
            ),
            (
                "radius",
                r#"{"type": "number", "description": "Probe capsule radius (default: the scene's character)."}"#,
            ),
            (
                "height",
                r#"{"type": "number", "description": "Probe capsule height."}"#,
            ),
            (
                "step_height",
                r#"{"type": "number", "description": "Highest step climbed without jumping."}"#,
            ),
            (
                "max_slope",
                r#"{"type": "number", "description": "Steepest walkable slope, degrees."}"#,
            ),
            PROJECT,
        ],
        required: &["waypoints"],
        read_only: true,
    },
    Tool {
        name: "nav_path",
        title: "Navmesh path",
        description: "Where enemies and other agents can walk: the navmesh path between two points of a scene (feet positions), baked from its static colliders as in the game, without playing or changing anything. status is complete (points: the corners, length in metres), unreachable (points end at the reachable point nearest the goal; a wall, a gap, a hidden bridge) or no_start (no walkable ground near `from`). Use it to check that an enemy can reach the player. Example: {\"from\": [0, 0, 0], \"to\": [12, 0, -8]}.",
        properties: &[
            SCENE,
            (
                "from",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Start [x, y, z], at the feet."}"#,
            ),
            (
                "to",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Goal [x, y, z]."}"#,
            ),
            PROJECT,
        ],
        required: &["from", "to"],
        read_only: true,
    },
    Tool {
        name: "playtest",
        title: "Replay playtests",
        description: "Replay a playtest (playtests/<name>.playtest.json, see record_start/record_stop) and report every check: passed, frame, message, expected and actual values, plus `fingerprint` (names and positions at the end: equal fingerprints mean the same end state). Headless by default: the scene loads fresh, inputs are the game's named actions per 1/60 s frame with a fixed step, so a replay is deterministic and comparable after every change; without `file` every playtest of the project runs. live=true replays in the running editor or player instead (restarted from its scene; real time). passed=false is still a successful call. A file: {\"scene\"?, \"seed\"?, \"teleport\"?: {\"entity\", \"position\", \"yaw\"?}, \"steps\": [{\"input\": {\"move\": [0, 1], \"jump\": true}, \"frames\": 30}, {\"wait\": 10}, {\"check\": \"entity_near\", \"entity\": \"Player\", \"point\": [0, 1, -4], \"radius\": 1}, {\"check\": \"entity_gone\", \"entity\": \"Gem 1\"}, {\"check\": \"component_equals\", \"entity\": \"Door\", \"path\": \"Transform.translation.y\", \"value\": 3, \"tolerance\": 0.1}, {\"check\": \"event_seen\", \"event\": \"PlaySound\", \"fields\"?: {...}}, {\"check\": \"log_contains\", \"text\": \"score 20\"}]}.",
        properties: &[
            (
                "file",
                r#"{"type": "string", "description": "A playtest name (playtests/<name>.playtest.json) or a path relative to the project root. Default: every playtest (headless)."}"#,
            ),
            (
                "playtest",
                r#"{"type": "object", "description": "A playtest given inline instead of a file."}"#,
            ),
            (
                "live",
                r#"{"type": "boolean", "description": "Replay in the running editor or player instead of headless. Default false."}"#,
            ),
            (
                "capture_every",
                r#"{"type": "integer", "minimum": 1, "description": "Headless: save a PNG every N frames (and the last) under <project>/.box/playtests/<name>/; needs a GPU."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "import",
        title: "Import an asset",
        description: "Copy a file into the project's assets/ folder (a .gltf with its buffers and images; a file already inside assets/ stays where it is), write its .meta and import it once. Returns its id, path, importer, state (loaded/failed with error) and sub-assets (label, type, state), e.g. models/tower.glb#Scene for a PrefabInstance and #Mesh/<name>, #Material/<name> for MeshAsset. Example: {\"file\": \"downloads/tower.glb\"}.",
        properties: &[
            (
                "file",
                r#"{"type": "string", "description": "The file to import, absolute or relative to the server's working directory."}"#,
            ),
            (
                "to",
                r#"{"type": "string", "description": "Folder under assets/ to copy into. Default by type: models, textures, materials, shaders, scenes."}"#,
            ),
            PROJECT,
        ],
        required: &["file"],
        read_only: false,
    },
    Tool {
        name: "status",
        title: "Live: status",
        description: "Live (needs a running editor or player with its control socket enabled, else error not_running): report kind (editor/player), mode (edit/play/paused), open scene, project, Play frame count and undo depth.",
        properties: &[PROJECT],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "world_query",
        title: "Live: query the world",
        description: "Live: list entities of the running editor's or player's world (the Play world while playing) with uid, runtime id, name, parent, children and component values. Filters combine: `name` (case-insensitive substring), `uid`, `component` (has this component). Example: {\"name\": \"lamp\", \"component\": \"PointLight\", \"limit\": 10}. `total` counts all matches; `truncated` says the limit cut the list.",
        properties: &[
            (
                "name",
                r#"{"type": "string", "description": "Case-insensitive substring of the entity name."}"#,
            ),
            (
                "uid",
                r#"{"type": "string", "description": "Exact entity uid."}"#,
            ),
            (
                "component",
                r#"{"type": "string", "description": "Only entities with this component, e.g. \"Camera\"."}"#,
            ),
            (
                "limit",
                r#"{"type": "integer", "minimum": 1, "maximum": 10000, "description": "Maximum entities returned. Default 100."}"#,
            ),
            (
                "include_components",
                r#"{"type": "boolean", "description": "Include component values (default true); false lists component names only."}"#,
            ),
            (
                "include_resources",
                r#"{"type": "boolean", "description": "Also return resource values such as Environment (default false)."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "apply_commands",
        title: "Live: apply edit commands",
        description: "Live: apply edit commands to the running editor's scene as one transaction, through the same command layer as the editor UI, so the user can undo it as one step. During Play the commands edit the Play world and are discarded on stop. In a player (box-player --control) they edit the running world and are neither saved nor undoable. With `save: true` (editor only) a fix lands in the scene file once: applied to the edit scene (undoable) and saved; during Play it is applied to the Play world and to the edit scene, which is saved at Stop. Same command format as scene_apply. Example: {\"commands\": [{\"op\": \"set\", \"entity\": \"Cube\", \"path\": \"Transform.translation.y\", \"value\": 2.0}], \"save\": true}.",
        properties: &[
            COMMANDS,
            DRY_RUN,
            (
                "save",
                r#"{"type": "boolean", "description": "Also write the change to the scene file (editor only; during Play, at Stop). Default false."}"#,
            ),
            PROJECT,
        ],
        required: &["commands"],
        read_only: false,
    },
    Tool {
        name: "screenshot",
        title: "Live: screenshot",
        description: "Live: capture the running editor's viewport or the player's view as a PNG image. Give both `width` and `height` to render at that size, or neither for the current view size. Give `eye` and `look_at` to render from that pose instead (a temporary camera: the world is not changed, the game camera keeps running). Example: {\"eye\": [10, 8, 12], \"look_at\": [0, 1, 0]}.",
        properties: &[
            (
                "width",
                r#"{"type": "integer", "minimum": 1, "maximum": 8192, "description": "Width in pixels (with height)."}"#,
            ),
            (
                "height",
                r#"{"type": "integer", "minimum": 1, "maximum": 8192, "description": "Height in pixels (with width)."}"#,
            ),
            (
                "eye",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "Render from this point [x, y, z] (with look_at)."}"#,
            ),
            (
                "look_at",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "The point [x, y, z] the eye looks at."}"#,
            ),
            (
                "fov",
                r#"{"type": "number", "minimum": 1, "maximum": 170, "description": "Vertical field of view in degrees (with eye)."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "play",
        title: "Live: play",
        description: "Live: start a Play session from the edited scene (or resume a paused one); in a player, resume after pause. Returns the new status.",
        properties: &[PROJECT],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "stop",
        title: "Live: stop",
        description: "Live: end the Play session and return to the edited scene (Play-time changes are discarded). In a player, quit the player (it exits cleanly after answering). Returns the new status.",
        properties: &[PROJECT],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "pause",
        title: "Live: pause",
        description: "Live: pause the Play session (error not_playing in Edit mode). Use step_frames to advance it, play to resume.",
        properties: &[PROJECT],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "step_frames",
        title: "Live: step frames",
        description: "Live: advance the Play session by exactly `frames` fixed 1/60 s frames and return when done, so a following world_query or screenshot sees the result. Usually after pause. Example: {\"frames\": 30}.",
        properties: &[
            (
                "frames",
                r#"{"type": "integer", "minimum": 1, "maximum": 10000, "description": "Frames to advance. Default 1."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "send_input",
        title: "Live: send input",
        description: "Live, during Play: press, hold or move a named action of the game (from its ActionMap: the names the game reads, not keys), for `frames` input frames (default 1: a tap). A button takes no value; a 1D axis a number in -1..1; a 2D axis [x, y] with +y forward (as the game's own stick or WASD). Frames only pass while the game runs: follow with step_frames when paused. An unknown action lists the game's actions. Example: {\"action\": \"move\", \"value\": [0, 1], \"frames\": 90}.",
        properties: &[
            (
                "action",
                r#"{"type": "string", "description": "Action name, as the game's ActionMap defines it."}"#,
            ),
            (
                "frames",
                r#"{"type": "integer", "minimum": 1, "maximum": 600, "description": "Input frames to hold it. Default 1."}"#,
            ),
            (
                "value",
                r#"{"description": "Axis value: a number for a 1D axis, [x, y] for a 2D axis. None for a button."}"#,
            ),
            PROJECT,
        ],
        required: &["action"],
        read_only: false,
    },
    Tool {
        name: "teleport",
        title: "Live: teleport",
        description: "Live, during Play: move an entity (uid or unique name, e.g. the player) to `position`, optionally facing `yaw` degrees (0 faces -Z); a character's motion and interpolation are reset so it is drawn there at once. Not saved. Example: {\"entity\": \"Player\", \"position\": [4, 1, -6]}.",
        properties: &[
            (
                "entity",
                r#"{"type": "string", "description": "Entity uid or unique name."}"#,
            ),
            (
                "position",
                r#"{"type": "array", "items": {"type": "number"}, "minItems": 3, "maxItems": 3, "description": "World position [x, y, z]."}"#,
            ),
            (
                "yaw",
                r#"{"type": "number", "description": "Facing, degrees about +Y."}"#,
            ),
            PROJECT,
        ],
        required: &["entity", "position"],
        read_only: false,
    },
    Tool {
        name: "record_start",
        title: "Live: start recording a playtest",
        description: "Live: start recording the game's named actions each frame (what send_input injects and what a person presses), to save as a playtest with record_stop. By default the game first restarts from its scene (a fresh Play session or a freshly loaded player), so the recording starts from the same state a replay does; restart=false records from where the game is (the playtest then starts with a teleport of the player, other state is not restored).",
        properties: &[
            (
                "restart",
                r#"{"type": "boolean", "description": "Restart the game from its scene first. Default true."}"#,
            ),
            (
                "player",
                r#"{"type": "string", "description": "The player entity (uid or name) for the end-position check. Default: the first named KinematicCharacter."}"#,
            ),
            (
                "seed",
                r#"{"type": "integer", "minimum": 0, "description": "Scripts' math.random seed for the restart (default 1)."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "record_stop",
        title: "Live: stop recording a playtest",
        description: "Live: stop recording and return the playtest: the frames resampled to 1/60 s, equal frames merged into input and wait steps, then checks for what changed (entity_gone for named scene entities that disappeared, entity_near for where the player ended, radius 1 m). With `save`, write it to playtests/<name>.playtest.json (or a path relative to the project root); edit or add checks there, then replay with `playtest`.",
        properties: &[
            (
                "save",
                r#"{"type": "string", "description": "A playtest name (playtests/<name>.playtest.json) or a path relative to the project root."}"#,
            ),
            (
                "description",
                r#"{"type": "string", "description": "What the run does, saved in the file."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "read_asset",
        title: "Read a project text file",
        description: "Read a text file under the project's assets/ folder (.luau, .json, .wgsl, .md, .txt; at most 1 MB): a script, a data table, a string table, a settings file. `path` is relative to assets/ (\"scripts/door.luau\"). Returns path, bytes and text.",
        properties: &[
            (
                "path",
                r#"{"type": "string", "description": "File under assets/, e.g. \"scripts/door.luau\"."}"#,
            ),
            PROJECT,
        ],
        required: &["path"],
        read_only: true,
    },
    Tool {
        name: "write_asset",
        title: "Write a project text file",
        description: "Create or replace a text file under the project's assets/ folder (.luau, .json, .wgsl, .md, .txt; at most 1 MB). A running editor hot-reloads it. The old contents are kept in .box/assistant/ so the editor's Assistant panel can undo the write with its turn. Prefer behaviours and presets (add_preset in apply_commands) to new scripts.",
        properties: &[
            (
                "path",
                r#"{"type": "string", "description": "File under assets/, e.g. \"scripts/door.luau\"."}"#,
            ),
            (
                "text",
                r#"{"type": "string", "description": "The whole new contents."}"#,
            ),
            PROJECT,
        ],
        required: &["path", "text"],
        read_only: false,
    },
    Tool {
        name: "editor_context",
        title: "Live: what the user sees in the editor",
        description: "Live (editor): the open scene, the selected entities (uid, name), the viewport camera (eye, a point it looks at, fov in degrees), whether Play is running, the open assistant turn and the next undo step. \"This\" or \"here\" in the user's words usually means the selection or what the camera looks at.",
        properties: &[PROJECT],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "undo_turn",
        title: "Live: undo this assistant turn",
        description: "Live (editor): undo every scene edit the current assistant turn made so far, as one step (for example after a failed attempt), and keep working in the same turn. Returns undone=false when there was nothing to undo.",
        properties: &[PROJECT],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "read_log",
        title: "Live: read the log",
        description: "Live: read log records (seq, time, level, target, message), oldest first. Pass the returned `next` as `since` to get only newer records.",
        properties: &[
            (
                "since",
                r#"{"type": "integer", "minimum": 0, "description": "Return records with seq >= since. Default 0."}"#,
            ),
            (
                "limit",
                r#"{"type": "integer", "minimum": 1, "maximum": 10000, "description": "Maximum records. Default 200."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "frame_stats",
        title: "Live: frame statistics",
        description: "Live: frame timings and render statistics (draws, triangles, validation errors) of the running editor or player.",
        properties: &[PROJECT],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "audio_levels",
        title: "Live: audio levels",
        description: "Live, during Play: the audio mix at the listener. Returns output (device/capture/silent; `measured` is false on a silent output, where levels are null), listener position and forward, bus volumes (AudioSettings), the solo/mute state, the mixed output level (capture output), per-bus levels, and every playing voice loudest first: name (source entity or one-shot clip), entity uid, clip, bus, volume, distance, min/max_distance, attenuation (distance gain, 0..1, also in dB), pan (-1 left..1 right), expected_gain (volume x attenuation x bus x master), level {rms, rms_db, peak, peak_db, left, right} measured after distance, panning, bus and master volume (before solo/mute), and muted/soloed/audible. `target` filters to the voices an entity (and its children), bus or clip covers. Example: {\"target\": \"Pond\"}.",
        properties: &[
            (
                "target",
                r#"{"type": "string", "description": "Only voices of this target: an entity uid or unique name (its sources and the sources below it), a bus (`ambience`, `bus:music`) or a clip (`clip:audio/pond.ogg`)."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: true,
    },
    Tool {
        name: "audio_solo",
        title: "Live: solo audio",
        description: "Live, during Play: solo an entity (with every sound source below it in the hierarchy), a bus or a clip, so only soloed sounds are heard; several solos add up. Runtime only: nothing is saved, and it ends with Play (or the game). `on: false` unsolos it; `clear: true` removes every solo. Levels stay measured for silenced voices (audio_levels). Returns the target, the voices it covers now and the solo/mute state. Example: {\"target\": \"Orchard\"}.",
        properties: &[
            (
                "target",
                r#"{"type": "string", "description": "Entity uid or unique name, bus (`ambience`, `bus:effects`) or clip (`clip:audio/pond.ogg`)."}"#,
            ),
            (
                "on",
                r#"{"type": "boolean", "description": "Solo (true, default) or unsolo (false)."}"#,
            ),
            (
                "clear",
                r#"{"type": "boolean", "description": "Remove every solo (give no target)."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: false,
    },
    Tool {
        name: "audio_mute",
        title: "Live: mute audio",
        description: "Live, during Play: mute an entity (with every sound source below it), a bus or a clip; muted sounds keep playing silently and keep their measured level. Runtime only: nothing is saved, and it ends with Play (or the game). `on: false` unmutes; `clear: true` removes every mute. Example: {\"target\": \"bus:music\"}.",
        properties: &[
            (
                "target",
                r#"{"type": "string", "description": "Entity uid or unique name, bus (`ambience`, `bus:effects`) or clip (`clip:audio/pond.ogg`)."}"#,
            ),
            (
                "on",
                r#"{"type": "boolean", "description": "Mute (true, default) or unmute (false)."}"#,
            ),
            (
                "clear",
                r#"{"type": "boolean", "description": "Remove every mute (give no target)."}"#,
            ),
            PROJECT,
        ],
        required: &[],
        read_only: false,
    },
];

/// `tools/list` entries.
fn tool_definitions() -> Json {
    TOOLS
        .iter()
        .map(|t| {
            let properties: Map<String, Json> = t
                .properties
                .iter()
                .map(|(name, schema)| {
                    let mut schema: Json =
                        serde_json::from_str(schema).unwrap_or_else(|_| json!({}));
                    if *name == COMMANDS.0 {
                        schema["description"] = json!(COMMANDS_HELP);
                    }
                    ((*name).to_string(), schema)
                })
                .collect();
            json!({
                "name": t.name,
                "title": t.title,
                "description": t.description,
                "inputSchema": {
                    "type": "object",
                    "properties": properties,
                    "required": t.required,
                    "additionalProperties": false,
                },
                "annotations": {
                    "title": t.title,
                    "readOnlyHint": t.read_only,
                    "destructiveHint": false,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Json {
        json!({ META_VERSION: PROTOCOL_VERSION, META_CAPABILITIES: {} })
    }

    #[test]
    fn tool_schemas_are_valid_json() {
        for t in TOOLS {
            for (name, schema) in t.properties {
                serde_json::from_str::<Json>(schema)
                    .unwrap_or_else(|e| panic!("{}.{name}: {e}", t.name));
            }
            for r in t.required {
                assert!(t.properties.iter().any(|p| p.0 == *r), "{}", t.name);
            }
        }
        // Every property is documented; `commands` by the shared help text.
        for tool in tool_definitions().as_array().unwrap() {
            for (name, schema) in tool["inputSchema"]["properties"].as_object().unwrap() {
                assert!(schema["description"].is_string(), "{}.{name}", tool["name"]);
            }
        }
        let scene_apply = &tool_definitions()[4];
        assert_eq!(scene_apply["name"], "scene_apply");
        assert_eq!(
            scene_apply["inputSchema"]["properties"]["commands"]["description"],
            COMMANDS_HELP
        );
        let names: std::collections::HashSet<_> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(names.len(), TOOLS.len());
    }

    #[test]
    fn modern_requests_need_meta_and_a_supported_version() {
        let mut s = McpServer::new(None);
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);

        let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta":{META_VERSION:"1900-01-01", META_CAPABILITIES:{}}}});
        let r = s.handle(&req.to_string()).unwrap();
        assert_eq!(r["error"]["code"], UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(r["error"]["data"]["supported"], json!([PROTOCOL_VERSION]));
        assert_eq!(r["error"]["data"]["requested"], "1900-01-01");

        let req =
            json!({"jsonrpc":"2.0","id":3,"method":"server/discover","params":{"_meta": meta()}});
        let r = s.handle(&req.to_string()).unwrap();
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["supportedVersions"], json!([PROTOCOL_VERSION]));
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert_eq!(r["result"]["_meta"][META_SERVER_INFO]["name"], "box");

        let req =
            json!({"jsonrpc":"2.0","id":"x","method":"tools/list","params":{"_meta": meta()}});
        let r = s.handle(&req.to_string()).unwrap();
        assert_eq!(r["id"], "x");
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), TOOLS.len());
        assert_eq!(r["result"]["cacheScope"], "public");
    }

    #[test]
    fn malformed_messages_get_json_rpc_errors() {
        let mut s = McpServer::new(None);
        assert_eq!(s.handle("{not json").unwrap()["error"]["code"], PARSE_ERROR);
        assert_eq!(s.handle("[]").unwrap()["error"]["code"], INVALID_REQUEST);
        assert_eq!(
            s.handle(r#"{"id":1,"method":"ping"}"#).unwrap()["error"]["code"],
            INVALID_REQUEST
        );
        assert_eq!(
            s.handle(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#)
                .unwrap()["error"]["code"],
            INVALID_REQUEST
        );
        assert!(
            s.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        assert!(s.handle("   ").is_none());
        assert_eq!(
            s.handle(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
                .unwrap()["result"],
            json!({})
        );
    }

    #[test]
    fn legacy_initialize_negotiates_a_version() {
        let mut s = McpServer::new(None);
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#)
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#)
            .unwrap();
        assert_eq!(r["result"]["protocolVersion"], LEGACY_VERSIONS[0]);
        // Legacy results carry no modern fields.
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#)
            .unwrap();
        assert!(r["result"]["tools"].is_array());
        assert!(r["result"].get("resultType").is_none());
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#)
            .unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nope"}}"#)
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn bad_arguments_are_tool_errors() {
        let mut s = McpServer::new(None);
        s.handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#);
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"registry","arguments":{"nam":"x"}}}"#)
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(
            r["result"]["structuredContent"]["error"]["code"],
            "invalid_arguments"
        );
        let r = s
            .handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"render","arguments":{"width":"wide"}}}"#)
            .unwrap();
        assert_eq!(r["result"]["isError"], true);
    }
}
