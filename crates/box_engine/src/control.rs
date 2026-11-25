//! The live control socket: lets tools such as `box mcp` query and drive a
//! running editor or player (ARCHITECTURE.md section 12, decision D15).
//!
//! **Opt-in and local only.** A host (the editor, the player) creates a
//! [`ControlServer`] only when the user asks for it. The server listens on
//! `127.0.0.1` on a free port and writes a discovery file,
//! `<project>/.box/control.json` ([`Discovery`]), holding the port and a
//! random token. Every request must carry the token, so only processes that
//! can read the project's `.box/` directory can connect. The file is removed
//! when the server is dropped.
//!
//! **Wire format.** Newline-delimited JSON over TCP, one message per line:
//!
//! ```text
//! -> {"id": 1, "token": "…", "method": "world_query", "params": {"name": "lamp"}}
//! <- {"id": 1, "result": {"entities": [ … ], "total": 1, "truncated": false}}
//! <- {"id": 2, "error": {"code": "entity_not_found", "message": "…", "command": 0}}
//! ```
//!
//! Methods: `status`, `world_query`, `apply_commands`, `screenshot`, `play`,
//! `stop`, `pause`, `step_frames`, `read_log`, `frame_stats`, `send_input`
//! (press, hold or move a named action of the playing game), `teleport`,
//! `record_start`/`record_stop` (record the game's actions into a
//! playtest), `playtest`/`playtest_status` (replay one live; see
//! [`crate::playtest`]).
//! Error codes are stable strings ([`code`]). Requests are limited to [`MAX_REQUEST_BYTES`];
//! malformed input gets an error response, never a panic.
//!
//! **Threading.** The server never spawns threads and never touches the
//! world on its own: the host calls [`ControlServer::poll`] once per frame on
//! the main thread with a [`ControlHost`], and requests are answered there.
//! Edits go through [`box_core::edit`] (see [`apply_to_world`]) so they are
//! undoable like user edits.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use box_core::edit::{self, Command, CommandError, CommandParseError, EditError, EditStack};
use box_core::identity::{Name, Uid};
use box_core::reflect::ReflectError;
use box_core::scene;
use box_core::world::{Entity, World};
use box_render::{Renderer, ViewCamera};

use crate::camera::CameraPose;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json, json};

use crate::Game;

/// Version of the control protocol, written to the discovery file.
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest accepted request line, in bytes. Longer requests get a
/// `too_large` error and the connection is closed.
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// Largest response line a [`ControlClient`] accepts (screenshots are large).
pub const MAX_RESPONSE_BYTES: usize = 256 << 20;
/// Simultaneous client connections; more are refused with `busy`.
pub const MAX_CONNECTIONS: usize = 8;
/// Requests answered per connection per [`ControlServer::poll`], so a chatty
/// client cannot stall a frame.
pub const MAX_REQUESTS_PER_POLL: usize = 16;
/// Default and maximum number of entities a `world_query` returns.
pub const DEFAULT_QUERY_LIMIT: usize = 100;
pub const MAX_QUERY_LIMIT: usize = 10_000;
/// Largest `step_frames` request.
pub const MAX_STEP_FRAMES: u32 = 10_000;
/// Longest `send_input`, in input frames (10 s at 60 fps).
pub const MAX_INPUT_FRAMES: u32 = 600;

/// Largest screenshot side, in pixels.
pub const MAX_SCREENSHOT_SIZE: u32 = 8192;

/// Stable error codes of the control protocol.
pub mod code {
    /// The request's token is missing or wrong.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// The line is not a JSON object with a string `method`.
    pub const BAD_REQUEST: &str = "bad_request";
    /// The request line exceeds [`super::MAX_REQUEST_BYTES`].
    pub const TOO_LARGE: &str = "too_large";
    /// Too many connections.
    pub const BUSY: &str = "busy";
    pub const UNKNOWN_METHOD: &str = "unknown_method";
    /// `params` do not match the method.
    pub const INVALID_PARAMS: &str = "invalid_params";
    /// The host does not implement this method (for example `play` in a host
    /// without Play mode, or `screenshot` without a renderer).
    pub const UNSUPPORTED: &str = "unsupported";
    /// `step_frames`/`pause` while not playing.
    pub const NOT_PLAYING: &str = "not_playing";
    /// The commands JSON does not parse.
    pub const COMMAND_PARSE: &str = "command_parse";
    pub const ENTITY_NOT_FOUND: &str = "entity_not_found";
    pub const AMBIGUOUS_ENTITY: &str = "ambiguous_entity";
    pub const UNKNOWN_TYPE: &str = "unknown_type";
    pub const BAD_PATH: &str = "bad_path";
    pub const COMMAND_FAILED: &str = "command_failed";
    pub const RENDER: &str = "render";
    pub const INTERNAL: &str = "internal";
    /// `audio_*` on a world without audio (no `AudioPlugin`, or a build
    /// without the `audio` module).
    pub const NO_AUDIO: &str = "no_audio";
    /// Client side: no discovery file, or nothing answers on its port.
    pub const NOT_RUNNING: &str = "not_running";
    /// Client side: no response in time.
    pub const TIMEOUT: &str = "timeout";
    /// Client side: the response was not valid protocol JSON.
    pub const PROTOCOL: &str = "protocol";
    pub const IO: &str = "io";
}

/// The stable error code for a failed edit, shared by the control socket and
/// the `box` CLI.
pub fn edit_error_code(e: &EditError) -> &'static str {
    match e {
        EditError::NoEntity(_) | EditError::UnknownEntity(_) => code::ENTITY_NOT_FOUND,
        EditError::AmbiguousName { .. } => code::AMBIGUOUS_ENTITY,
        EditError::Reflect(ReflectError::UnknownType(_)) => code::UNKNOWN_TYPE,
        EditError::Reflect(ReflectError::Path { .. }) => code::BAD_PATH,
        _ => code::COMMAND_FAILED,
    }
}

/// An error response: `{"code", "message", ...details}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("error[{code}]: {message}")]
pub struct ControlError {
    pub code: String,
    pub message: String,
    /// Extra machine-readable fields (command index, op, entity, path, ...).
    #[serde(flatten, default)]
    pub details: Map<String, Json>,
}

impl ControlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: Map::new(),
        }
    }

    /// The default answer of [`ControlHost`] methods a host does not implement.
    pub fn unsupported(method: &str) -> Self {
        Self::new(
            code::UNSUPPORTED,
            format!("this host does not support `{method}`"),
        )
        .with("method", json!(method))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, message)
    }

    pub fn not_playing(method: &str) -> Self {
        Self::new(
            code::NOT_PLAYING,
            format!("`{method}` needs a running Play session; call `play` first"),
        )
    }

    pub fn with(mut self, key: &str, value: Json) -> Self {
        self.details.insert(key.to_string(), value);
        self
    }

    /// A failed command of a command list, with its index, op, entity and path.
    pub fn command(e: &CommandError) -> Self {
        let mut err = Self::new(edit_error_code(&e.source), e.to_string())
            .with("command", json!(e.index))
            .with("op", json!(e.op));
        if let Some(entity) = &e.entity {
            err = err.with("entity", json!(entity));
        }
        if let Some(path) = &e.path {
            err = err.with("path", json!(path));
        }
        err
    }

    pub fn command_parse(e: &CommandParseError) -> Self {
        let err = Self::new(code::COMMAND_PARSE, e.to_string());
        match e {
            CommandParseError::Command { index, .. } => err.with("command", json!(index)),
            CommandParseError::Json(_) => err,
        }
    }

    pub fn to_json(&self) -> Json {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "code": self.code }))
    }
}

// ------------------------------------------------------------ data ----

/// Which kind of program hosts the socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostKind {
    Editor,
    Player,
}

/// What the host is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Editing the scene; the game does not run.
    Edit,
    /// The game runs.
    Play,
    /// A Play session is paused; `step_frames` advances it.
    Paused,
}

/// The discovery file, `<project>/.box/control.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    /// [`PROTOCOL_VERSION`].
    pub version: u32,
    pub pid: u32,
    /// TCP port on 127.0.0.1.
    pub port: u16,
    /// Required in every request.
    pub token: String,
    pub kind: HostKind,
    /// The open scene, relative to the project root.
    pub scene: Option<String>,
}

/// The discovery file of a project.
pub fn discovery_path(project_root: &Path) -> PathBuf {
    project_root.join(".box").join("control.json")
}

/// The project root of a [`discovery_path`], quoted for a shell when needed
/// (for messages that name commands).
fn project_of_discovery(discovery: &Path) -> String {
    let root = discovery
        .parent()
        .and_then(Path::parent)
        .unwrap_or(discovery)
        .display()
        .to_string();
    crate::player::shell_line(&[root])
}

/// The answer to `status` (the server adds `kind`, `pid` and `version`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub mode: Mode,
    /// The open scene, relative to the project root.
    pub scene: Option<String>,
    /// The project root.
    pub project: Option<String>,
    /// Frames simulated in the current Play session (0 in Edit mode).
    pub frame: u64,
    /// Undo steps available (editors), when the host has an undo stack.
    pub undo: Option<usize>,
}

/// A PNG capture of the viewport (editor) or the game view (player).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Screenshot {
    /// Encode an RGBA image as PNG.
    pub fn from_rgba(image: &image::RgbaImage) -> Result<Self, ControlError> {
        let mut png = Vec::new();
        image
            .write_to(&mut io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| ControlError::new(code::INTERNAL, format!("PNG encoding: {e}")))?;
        Ok(Self {
            png,
            width: image.width(),
            height: image.height(),
        })
    }
}

/// One log record, as returned by `read_log`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Increases by one per record.
    pub seq: u64,
    /// Seconds since the host started.
    pub time: f64,
    /// `error`, `warn`, `info`, `debug` or `trace`.
    pub level: String,
    pub target: String,
    pub message: String,
}

/// The answer to `read_log`: records with `seq >= since`, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LogPage {
    pub entries: Vec<LogEntry>,
    /// Pass as `since` to get only newer records.
    pub next: u64,
}

/// Parameters of `world_query`. All filters are optional and combine with AND.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorldQuery {
    /// Case-insensitive substring of the entity name.
    pub name: Option<String>,
    /// Exact entity uid.
    pub uid: Option<String>,
    /// Only entities that have this registered component.
    pub component: Option<String>,
    /// Maximum entities returned ([`DEFAULT_QUERY_LIMIT`], at most [`MAX_QUERY_LIMIT`]).
    pub limit: Option<usize>,
    /// Include component values (default true).
    pub include_components: Option<bool>,
    /// Include resource values (default false).
    pub include_resources: bool,
}

// ------------------------------------------------------------ host ----

/// What an editor or player implements to be driven over the socket. Every
/// method runs on the main thread, inside [`ControlServer::poll`]. Methods a
/// host does not override answer [`ControlError::unsupported`].
///
/// Result shapes: `world_query` and `apply_commands` return what
/// [`query_world`] and [`apply_to_world`] return; `step_frames` returns
/// `{"frames": n, "frame": total}`; `frame_stats` returns a host-defined
/// object (frame times, render statistics).
pub trait ControlHost {
    fn status(&mut self) -> Status;

    /// Entities with uid, name, parent and component values. Use [`query_world`].
    fn world_query(&mut self, query: &WorldQuery) -> Result<Json, ControlError> {
        let _ = query;
        Err(ControlError::unsupported("world_query"))
    }

    /// Apply edit commands as one transaction. Use [`apply_to_world`] with the
    /// host's undo stack, so agent edits are undoable like user edits.
    fn apply_commands(
        &mut self,
        commands: &[Command],
        dry_run: bool,
    ) -> Result<Json, ControlError> {
        let _ = (commands, dry_run);
        Err(ControlError::unsupported("apply_commands"))
    }

    /// `apply_commands` with `"save": true`: apply `commands` so they end up
    /// in the scene file (an editor: to the edit world, undoable, then save
    /// it; during Play to the play world and the parked edit world, saved
    /// at Stop). Hosts that cannot save leave it unsupported.
    fn apply_and_save(&mut self, commands: &[Command]) -> Result<Json, ControlError> {
        let _ = commands;
        Err(ControlError::new(
            code::UNSUPPORTED,
            "this host cannot save the scene: `save` needs the editor (or edit files with scene_apply)",
        ))
    }

    /// Capture the view. `size` is a requested size in pixels; `None` means
    /// the current view size. `view` is a camera to render from instead of
    /// the host's own (a pose an agent asked for); it is not added to the
    /// world.
    fn screenshot(
        &mut self,
        size: Option<(u32, u32)>,
        view: Option<&ViewCamera>,
    ) -> Result<Screenshot, ControlError> {
        let _ = (size, view);
        Err(ControlError::unsupported("screenshot"))
    }

    /// Start a Play session (or resume a paused one).
    fn play(&mut self) -> Result<Status, ControlError> {
        Err(ControlError::unsupported("play"))
    }

    /// End the Play session and return to the edited scene.
    fn stop(&mut self) -> Result<Status, ControlError> {
        Err(ControlError::unsupported("stop"))
    }

    fn pause(&mut self) -> Result<Status, ControlError> {
        Err(ControlError::unsupported("pause"))
    }

    /// Advance a paused (or running) Play session by exactly `frames` frames
    /// before returning (fixed 1/60 s steps), so a following request sees
    /// their result.
    fn step_frames(&mut self, frames: u32) -> Result<Json, ControlError> {
        let _ = frames;
        Err(ControlError::unsupported("step_frames"))
    }

    /// Log records with `seq >= since`, at most `limit`.
    fn read_log(&mut self, since: u64, limit: usize) -> Result<LogPage, ControlError> {
        let _ = (since, limit);
        Err(ControlError::unsupported("read_log"))
    }

    fn frame_stats(&mut self) -> Result<Json, ControlError> {
        Err(ControlError::unsupported("frame_stats"))
    }

    /// Start the game again from its scene, for `record_start` and
    /// `playtest`: a fresh Play session (an editor, [`HeadlessHost`]) or a
    /// freshly loaded app (the player) that has not run a frame yet, with
    /// `seed` as the scripts' `math.random` seed
    /// ([`box_script::ScriptSeed`]). `scene` (relative to the project root,
    /// or a scene name) starts another scene where the host can; others
    /// refuse a scene that is not theirs.
    fn restart_play(&mut self, scene: Option<&str>, seed: Option<u32>) -> Result<(), ControlError> {
        let _ = (scene, seed);
        Err(ControlError::unsupported("restart_play"))
    }

    /// The world a game is playing in, for `send_input` and `teleport`;
    /// [`ControlError::not_playing`] when there is none (an editor that is
    /// not in Play).
    fn play_world(&mut self, method: &str) -> Result<&mut World, ControlError> {
        Err(ControlError::unsupported(method))
    }

    /// What the user is looking at (editors): the open scene, the
    /// selection, the viewport camera, Play state and the assistant turn.
    fn editor_context(&mut self) -> Result<Json, ControlError> {
        Err(ControlError::unsupported("editor_context"))
    }

    /// Undoes the edits of the current assistant turn so far (editors).
    fn undo_turn(&mut self) -> Result<Json, ControlError> {
        Err(ControlError::unsupported("undo_turn"))
    }
}

/// The kind of action `name` in the world's `ActionMap`, or an
/// `invalid_params` error that lists the actions (with a did-you-mean).
pub(crate) fn action_kind(
    world: &World,
    name: &str,
) -> Result<box_core::action::ActionKind, ControlError> {
    use box_core::action::ActionMap;
    let map = world.get_resource::<ActionMap>();
    map.and_then(|m| m.get(name)).cloned().ok_or_else(|| {
        let names: Vec<String> = map
            .map(|m| m.names().map(str::to_owned).collect())
            .unwrap_or_default();
        let mut message = format!(
            "no action `{name}`; actions: {}",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        );
        if let Some(g) = box_core::reflect::did_you_mean(name, names.iter().map(String::as_str)) {
            message.push_str(&format!(" (did you mean `{g}`?)"));
        }
        ControlError::invalid_params(message)
    })
}

/// `send_input`: inject input into a named action of the playing world.
fn send_input(world: &mut World, p: &InputParams) -> Result<Json, ControlError> {
    use box_core::action::{ActionKind, Actions, InjectedInput};
    use glam::Vec2;
    if !(1..=MAX_INPUT_FRAMES).contains(&p.frames) {
        return Err(ControlError::invalid_params(format!(
            "`frames` must be 1..={MAX_INPUT_FRAMES}"
        )));
    }
    let kind = action_kind(world, &p.action)?;
    let value = match &p.value {
        None => None,
        Some(Json::Number(n)) => Some(Vec2::new(n.as_f64().unwrap_or(0.0) as f32, 0.0)),
        Some(Json::Array(a)) if a.len() == 2 && a.iter().all(Json::is_number) => Some(Vec2::new(
            a[0].as_f64().unwrap_or(0.0) as f32,
            a[1].as_f64().unwrap_or(0.0) as f32,
        )),
        Some(_) => {
            return Err(ControlError::invalid_params(
                "`value` is a number (1D axis) or [x, y] (2D axis; +y is forward)",
            ));
        }
    };
    let (kind_name, input) = match (&kind, value) {
        (ActionKind::Button { .. }, None) => ("button", InjectedInput::press(p.frames)),
        (ActionKind::Button { .. }, Some(_)) => {
            return Err(ControlError::invalid_params(format!(
                "`{}` is a button: give no `value`",
                p.action
            )));
        }
        (ActionKind::Axis1 { .. }, Some(v)) if v.y == 0.0 => {
            ("axis1", InjectedInput::axis(v, p.frames))
        }
        (ActionKind::Axis2 { .. }, Some(v)) => ("axis2", InjectedInput::axis(v, p.frames)),
        (ActionKind::Axis1 { .. }, _) => {
            return Err(ControlError::invalid_params(format!(
                "`{}` is a 1D axis: give `value` as a number in -1..1",
                p.action
            )));
        }
        (ActionKind::Axis2 { .. }, None) => {
            return Err(ControlError::invalid_params(format!(
                "`{}` is a 2D axis: give `value` as [x, y] (+y is forward)",
                p.action
            )));
        }
    };
    let actions = world
        .get_resource_mut::<Actions>()
        .ok_or_else(|| ControlError::new(code::INTERNAL, "the world has no Actions resource"))?;
    actions.inject(&p.action, input);
    Ok(json!({
        "action": p.action,
        "kind": kind_name,
        "frames": input.frames,
        "value": [input.value.x, input.value.y],
    }))
}

/// `teleport`: move an entity of the playing world to a position (and
/// yaw), stopping a character's motion and interpolation so it is drawn
/// there at once.
pub(crate) fn teleport(
    world: &mut World,
    entity: &str,
    position: [f32; 3],
    yaw: Option<f32>,
) -> Result<Json, ControlError> {
    use box_core::transform::Transform;
    use glam::{Quat, Vec3};
    let uid = edit::resolve_entity(world, entity)
        .map_err(|e| ControlError::new(edit_error_code(&e), e.to_string()))?;
    let e = world
        .entity_by_uid(uid)
        .ok_or_else(|| ControlError::new(code::ENTITY_NOT_FOUND, format!("no entity {uid}")))?;
    let at = Vec3::from_array(position);
    if !at.is_finite() || yaw.is_some_and(|y| !y.is_finite()) {
        return Err(ControlError::invalid_params(
            "position and yaw must be finite",
        ));
    }
    let Some(t) = world.component_mut::<Transform>(e) else {
        return Err(ControlError::invalid_params(format!(
            "entity `{entity}` has no Transform"
        )));
    };
    t.translation = at;
    if let Some(yaw) = yaw {
        t.rotation = Quat::from_rotation_y(yaw.to_radians());
    }
    #[cfg(feature = "physics")]
    if let Some(c) = world.component_mut::<box_physics::KinematicCharacter>(e) {
        c.velocity = Vec3::ZERO;
        c.grounded = false;
        c.ground_entity = None;
        c.ground_pose = None;
    }
    box_core::interpolate::Interpolated::reset(world, e);
    Ok(json!({ "entity": uid.to_string(), "position": position, "yaw": yaw }))
}

/// Saved entities in hierarchy pre-order, then every other entity by id.
fn entity_order(world: &World) -> Vec<Entity> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut stack: Vec<Entity> = world.roots().iter().rev().copied().collect();
    while let Some(e) = stack.pop() {
        if world.contains(e) && seen.insert(e) {
            out.push(e);
            stack.extend(world.children(e).into_iter().rev());
        }
    }
    let mut rest: Vec<Entity> = world
        .query::<Entity>()
        .iter()
        .filter(|e| !seen.contains(e))
        .collect();
    rest.sort_unstable_by_key(|e| e.to_bits());
    out.extend(rest);
    out
}

/// `world_query` against a world: the shared implementation for hosts.
///
/// Result: `{"entities": [{"uid", "entity", "name", "parent", "children",
/// "components": {Name: value}}], "total", "truncated", "resources"?}`.
/// `uid` is null for runtime entities that are not saved (they cannot be
/// edited by commands); `entity` is the runtime id. Component values use the
/// scene-file representation; a value that cannot be serialized is
/// `{"$error": message}`.
pub fn query_world(world: &World, query: &WorldQuery) -> Result<Json, ControlError> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .min(MAX_QUERY_LIMIT);
    let uid = match &query.uid {
        Some(s) => Some(
            Uid::parse(s)
                .ok_or_else(|| ControlError::invalid_params(format!("`{s}` is not a uid")))?,
        ),
        None => None,
    };
    if let Some(c) = &query.component
        && world.registry().component(c).is_none()
    {
        return Err(ControlError::new(
            code::UNKNOWN_TYPE,
            format!("no registered component named `{c}`"),
        ));
    }
    let needle = query.name.as_ref().map(|n| n.to_lowercase());
    let include_components = query.include_components.unwrap_or(true);
    let registry = world.registry();

    let mut entities = Vec::new();
    let mut total = 0usize;
    for e in entity_order(world) {
        if uid.is_some() && world.uid(e) != uid {
            continue;
        }
        let name = world.get::<Name>(e).map(|n| n.to_string());
        if let Some(needle) = &needle
            && !name
                .as_ref()
                .is_some_and(|n| n.to_lowercase().contains(needle))
        {
            continue;
        }
        let components = world.component_names(e);
        if let Some(c) = &query.component
            && !components.contains(&c.as_str())
        {
            continue;
        }
        total += 1;
        if entities.len() >= limit {
            continue;
        }
        let mut record = json!({
            "uid": world.uid(e).map(|u| u.to_string()),
            "entity": e.to_bits().get(),
            "name": name,
            "parent": world.parent(e).and_then(|p| world.uid(p)).map(|u| u.to_string()),
            "children": world.children(e).into_iter().filter_map(|c| world.uid(c)).map(|u| u.to_string()).collect::<Vec<_>>(),
        });
        let mut values = Map::new();
        for c in components {
            if c == "Uid" || c == "Name" {
                continue;
            }
            let value = if include_components {
                world
                    .get_reflect(e, c)
                    .and_then(|r| registry.value_to_json((**r).as_partial_reflect()))
                    .unwrap_or_else(|err| json!({ "$error": err.to_string() }))
            } else {
                Json::Null
            };
            values.insert(c.to_string(), value);
        }
        record["components"] = if include_components {
            Json::Object(values)
        } else {
            json!(values.keys().collect::<Vec<_>>())
        };
        entities.push(record);
    }
    let mut out = json!({
        "entities": entities,
        "total": total,
        "truncated": total > entities.len(),
    });
    if query.include_resources {
        let mut resources = Map::new();
        for r in world.resource_names() {
            let value = world
                .get_resource_reflect(r)
                .and_then(|v| registry.value_to_json(v.as_partial_reflect()))
                .unwrap_or_else(|err| json!({ "$error": err.to_string() }));
            resources.insert(r.to_string(), value);
        }
        out["resources"] = Json::Object(resources);
    }
    Ok(out)
}

/// `apply_commands` against a world: the shared implementation for hosts.
///
/// The commands run through [`box_core::edit::apply_commands`] as one
/// all-or-nothing transaction labelled `label` on `stack` (one undo step). A
/// dry run applies them on a scratch stack and undoes them, leaving the world
/// and `stack` unchanged. Result: `{"commands", "created": [uids], "dry_run",
/// "undoable"}`.
pub fn apply_to_world(
    world: &mut World,
    stack: &mut EditStack,
    commands: &[Command],
    dry_run: bool,
    label: &str,
) -> Result<Json, ControlError> {
    let applied = if dry_run {
        let mut scratch = EditStack::new();
        let applied = edit::apply_commands_with_warnings(world, &mut scratch, commands, label)
            .map_err(|e| ControlError::command(&e))?;
        scratch
            .undo(world)
            .map_err(|e| ControlError::new(code::INTERNAL, format!("dry-run undo: {e}")))?;
        applied
    } else {
        edit::apply_commands_with_warnings(world, stack, commands, label)
            .map_err(|e| ControlError::command(&e))?
    };
    for w in &applied.warnings {
        tracing::warn!("{w}");
    }
    // Files the commands need (Convert to Script), into the project.
    let mut files_written = Vec::new();
    if !dry_run
        && !applied.files.is_empty()
        && let Some(server) = world.get_resource::<box_assets::AssetServer>()
    {
        files_written =
            crate::behaviours::scripts::write_missing_files(server.assets_dir(), &applied.files)
                .map_err(|e| ControlError::new(code::INTERNAL, format!("writing scripts: {e}")))?;
    }
    Ok(json!({
        "commands": commands.len(),
        "created": applied.created.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "files_written": files_written,
        "dry_run": dry_run,
        "undoable": !dry_run,
    }))
}

// ---------------------------------------------------------- server ----

/// The socket a host polls once per frame. See the [module docs](self).
pub struct ControlServer {
    listener: TcpListener,
    port: u16,
    token: String,
    kind: HostKind,
    scene: Option<String>,
    discovery_file: Option<PathBuf>,
    conns: Vec<Conn>,
}

struct Conn {
    stream: TcpStream,
    inbuf: Vec<u8>,
    out: Vec<u8>,
    out_pos: usize,
    eof: bool,
    /// Close once `out` is flushed.
    closing: bool,
    dead: bool,
}

impl Conn {
    fn queue(&mut self, response: &Json) {
        if let Ok(mut bytes) = serde_json::to_vec(response) {
            bytes.push(b'\n');
            self.out.extend_from_slice(&bytes);
        }
    }

    fn flush(&mut self) {
        while self.out_pos < self.out.len() {
            match self.stream.write(&self.out[self.out_pos..]) {
                Ok(0) => {
                    self.dead = true;
                    return;
                }
                Ok(n) => self.out_pos += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.dead = true;
                    return;
                }
            }
        }
        self.out.clear();
        self.out_pos = 0;
    }

    /// Before closing: end our side and discard unread input, so the peer
    /// reads the last response instead of a connection reset.
    fn finish(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
        let mut chunk = [0u8; 16 * 1024];
        while matches!(self.stream.read(&mut chunk), Ok(n) if n > 0) {}
    }

    fn fill(&mut self) {
        let mut chunk = [0u8; 16 * 1024];
        while !self.eof && self.inbuf.len() <= MAX_REQUEST_BYTES {
            match self.stream.read(&mut chunk) {
                Ok(0) => self.eof = true,
                Ok(n) => self.inbuf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.dead = true;
                    return;
                }
            }
        }
    }
}

fn new_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Write `bytes` to `path` via a temporary file and a rename, readable only by
/// the current user on Unix (the file holds the token).
fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_file_name(format!(".control.json.tmp-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

impl ControlServer {
    /// Bind `127.0.0.1` on a free port without writing a discovery file
    /// (tests, embedding). Clients connect with [`ControlServer::discovery`].
    pub fn bind(kind: HostKind, scene: Option<&str>) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            port,
            token: new_token(),
            kind,
            scene: scene.map(str::to_string),
            discovery_file: None,
            conns: Vec::new(),
        })
    }

    /// Bind and write `<project_root>/.box/control.json` so `box mcp` finds
    /// this host. An existing file (another instance, or a crashed one) is
    /// replaced. The file is removed on drop.
    pub fn start(project_root: &Path, kind: HostKind, scene: Option<&str>) -> io::Result<Self> {
        let mut server = Self::bind(kind, scene)?;
        let path = discovery_path(project_root);
        // Another live host loses its discovery file: say so, since `box
        // mcp` will reach this one only.
        if let Ok(other) = read_discovery(project_root)
            && other.pid != std::process::id()
            && TcpStream::connect_timeout(
                &SocketAddr::from((Ipv4Addr::LOCALHOST, other.port)),
                Duration::from_millis(200),
            )
            .is_ok()
        {
            tracing::warn!(
                target: "box::control",
                "another {:?} (pid {}) already serves live control for this project; `box mcp` now reaches this {:?} instead. Quit the other one, or start only one app with --control",
                other.kind,
                other.pid,
                kind
            );
        }
        server.discovery_file = Some(path);
        server.write_discovery()?;
        tracing::info!(
            target: "box::control",
            port = server.port,
            file = %discovery_path(project_root).display(),
            "control socket listening on 127.0.0.1"
        );
        Ok(server)
    }

    fn write_discovery(&self) -> io::Result<()> {
        let Some(path) = &self.discovery_file else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&self.discovery()).map_err(io::Error::other)?;
        write_private_atomic(path, &bytes)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn kind(&self) -> HostKind {
        self.kind
    }

    /// What the discovery file holds.
    pub fn discovery(&self) -> Discovery {
        Discovery {
            version: PROTOCOL_VERSION,
            pid: std::process::id(),
            port: self.port,
            token: self.token.clone(),
            kind: self.kind,
            scene: self.scene.clone(),
        }
    }

    pub fn discovery_file(&self) -> Option<&Path> {
        self.discovery_file.as_deref()
    }

    /// Record the open scene (relative to the project root) and rewrite the
    /// discovery file. Call after Open, Save As or New Scene.
    pub fn set_scene(&mut self, scene: Option<&str>) -> io::Result<()> {
        self.scene = scene.map(str::to_string);
        self.write_discovery()
    }

    /// Open client connections.
    pub fn connections(&self) -> usize {
        self.conns.len()
    }

    /// Accept connections, answer complete requests with `host` and flush
    /// responses. Never blocks; call once per frame on the main thread.
    pub fn poll(&mut self, host: &mut dyn ControlHost) {
        profiling::scope!("ControlServer::poll");
        self.accept();
        let token = &self.token;
        let kind = self.kind;
        for conn in &mut self.conns {
            conn.flush();
            // Back-pressure: read more only once earlier answers are out.
            if conn.dead || !conn.out.is_empty() || conn.closing {
                continue;
            }
            conn.fill();
            let mut handled = 0;
            while handled < MAX_REQUESTS_PER_POLL && !conn.closing {
                let Some(nl) = conn.inbuf.iter().position(|&b| b == b'\n') else {
                    break;
                };
                let line: Vec<u8> = conn.inbuf.drain(..=nl).collect();
                let response = if nl > MAX_REQUEST_BYTES {
                    conn.closing = true;
                    Some(too_large())
                } else {
                    handle_line(token, kind, &line[..nl], host)
                };
                if let Some(r) = response {
                    conn.queue(&r);
                }
                handled += 1;
            }
            let has_line = conn.inbuf.contains(&b'\n');
            if !has_line && conn.inbuf.len() > MAX_REQUEST_BYTES {
                conn.inbuf.clear();
                conn.closing = true;
                conn.queue(&too_large());
            } else if conn.eof && !has_line {
                // A last request without a trailing newline.
                if !conn.inbuf.is_empty() {
                    let line = std::mem::take(&mut conn.inbuf);
                    if let Some(r) = handle_line(token, kind, &line, host) {
                        conn.queue(&r);
                    }
                }
                conn.closing = true;
            }
            conn.flush();
        }
        self.conns.retain_mut(|c| {
            if c.dead {
                return false;
            }
            if c.closing && c.out.is_empty() {
                c.finish();
                return false;
            }
            true
        });
    }

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((mut stream, addr)) => {
                    if !addr.ip().is_loopback() {
                        continue;
                    }
                    if self.conns.len() >= MAX_CONNECTIONS {
                        let err = ControlError::new(code::BUSY, "too many control connections");
                        let _ =
                            writeln!(stream, "{}", json!({ "id": null, "error": err.to_json() }));
                        continue;
                    }
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let _ = stream.set_nodelay(true);
                    self.conns.push(Conn {
                        stream,
                        inbuf: Vec::new(),
                        out: Vec::new(),
                        out_pos: 0,
                        eof: false,
                        closing: false,
                        dead: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::warn!(target: "box::control", "accept failed: {e}");
                    return;
                }
            }
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // Remove the discovery file only if it is still ours (another
        // instance may have replaced it).
        if let Some(path) = &self.discovery_file
            && let Ok(bytes) = std::fs::read(path)
            && serde_json::from_slice::<Discovery>(&bytes).is_ok_and(|d| d.token == self.token)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn too_large() -> Json {
    let err = ControlError::new(
        code::TOO_LARGE,
        format!("request exceeds {MAX_REQUEST_BYTES} bytes"),
    );
    json!({ "id": null, "error": err.to_json() })
}

/// One request line to one response (`None` for a blank line).
fn handle_line(
    token: &str,
    kind: HostKind,
    line: &[u8],
    host: &mut dyn ControlHost,
) -> Option<Json> {
    let line = line.trim_ascii();
    if line.is_empty() {
        return None;
    }
    let reply = |id: Json, r: Result<Json, ControlError>| match r {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(e) => json!({ "id": id, "error": e.to_json() }),
    };
    let request: Json = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(reply(
                Json::Null,
                Err(ControlError::new(
                    code::BAD_REQUEST,
                    format!("invalid JSON: {e}"),
                )),
            ));
        }
    };
    let Some(obj) = request.as_object() else {
        return Some(reply(
            Json::Null,
            Err(ControlError::new(
                code::BAD_REQUEST,
                "a request is a JSON object",
            )),
        ));
    };
    let id = obj.get("id").cloned().unwrap_or(Json::Null);
    if !obj
        .get("token")
        .and_then(Json::as_str)
        .is_some_and(|t| token_matches(token, t))
    {
        return Some(reply(
            id,
            Err(ControlError::new(
                code::UNAUTHORIZED,
                "missing or wrong token (read it from <project>/.box/control.json)",
            )),
        ));
    }
    let Some(method) = obj.get("method").and_then(Json::as_str) else {
        return Some(reply(
            id,
            Err(ControlError::new(
                code::BAD_REQUEST,
                "`method` must be a string",
            )),
        ));
    };
    let params = match obj.get("params") {
        None | Some(Json::Null) => Json::Object(Map::new()),
        Some(p @ Json::Object(_)) => p.clone(),
        Some(_) => {
            return Some(reply(
                id,
                Err(ControlError::invalid_params("`params` must be an object")),
            ));
        }
    };
    Some(reply(id, dispatch(kind, method, params, host)))
}

fn parse<T: serde::de::DeserializeOwned>(method: &str, params: Json) -> Result<T, ControlError> {
    serde_json::from_value(params)
        .map_err(|e| ControlError::invalid_params(format!("{method}: {e}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyParams {
    commands: Json,
    #[serde(default)]
    dry_run: bool,
    /// Also write the change to the scene file (editor only).
    #[serde(default)]
    save: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenshotParams {
    width: Option<u32>,
    height: Option<u32>,
    /// A camera pose to render from (with `look_at`, optional `fov`).
    eye: Option<[f32; 3]>,
    look_at: Option<[f32; 3]>,
    fov: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputParams {
    action: String,
    #[serde(default = "one")]
    frames: u32,
    value: Option<Json>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeleportParams {
    entity: String,
    position: [f32; 3],
    /// Degrees about +Y (0 faces -Z).
    yaw: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordStartParams {
    /// Restart the game from its scene first (default true).
    #[serde(default = "yes")]
    restart: bool,
    /// The player entity (default: the first named character).
    player: Option<String>,
    /// Scripts' `math.random` seed for the restart.
    seed: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordStopParams {
    /// Write the playtest: a name (`playtests/<name>.playtest.json`) or a
    /// path relative to the project root.
    save: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaytestParams {
    /// A playtest name or file (relative to the project root).
    file: Option<String>,
    /// A playtest given inline.
    playtest: Option<Json>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepParams {
    #[serde(default = "one")]
    frames: u32,
}

fn one() -> u32 {
    1
}

#[cfg(feature = "audio")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioLevelsParams {
    /// Only the voices this target applies to (see `audio_mixer::resolve_target`).
    target: Option<String>,
}

#[cfg(feature = "audio")]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioMixParams {
    target: Option<String>,
    #[serde(default = "yes")]
    on: bool,
    #[serde(default)]
    clear: bool,
}

/// `audio_levels`, `audio_solo`, `audio_mute` on the playing world (see
/// [`crate::audio_mixer`]).
#[cfg(feature = "audio")]
fn audio_method(
    method: &str,
    params: Json,
    host: &mut dyn ControlHost,
) -> Result<Json, ControlError> {
    use crate::audio_mixer::{self, MixList};
    let no_audio = |e: String| ControlError::new(code::NO_AUDIO, e);
    if method == "audio_levels" {
        let p: AudioLevelsParams = parse(method, params)?;
        let world = host.play_world(method)?;
        if !world.contains_resource::<box_audio::Audio>() {
            return Err(no_audio("this world has no audio (no AudioPlugin)".into()));
        }
        let filter = p
            .target
            .as_deref()
            .map(|t| audio_mixer::resolve_target(world, t))
            .transpose()
            .map_err(ControlError::invalid_params)?;
        return audio_mixer::levels_json(world, filter.as_ref()).map_err(no_audio);
    }
    let p: AudioMixParams = parse(method, params)?;
    let list = if method == "audio_solo" {
        MixList::Solo
    } else {
        MixList::Mute
    };
    let world = host.play_world(method)?;
    if !world.contains_resource::<box_audio::Audio>() {
        return Err(no_audio("this world has no audio (no AudioPlugin)".into()));
    }
    audio_mixer::set_mix(world, list, p.target.as_deref(), p.on, p.clear)
        .map_err(ControlError::invalid_params)
}

#[cfg(not(feature = "audio"))]
fn audio_method(
    method: &str,
    params: Json,
    host: &mut dyn ControlHost,
) -> Result<Json, ControlError> {
    let _ = (params, host);
    Err(ControlError::new(
        code::NO_AUDIO,
        format!("`{method}`: this build has no audio module (box_engine feature `audio`)"),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogParams {
    #[serde(default)]
    since: u64,
    limit: Option<usize>,
}

fn status_json(kind: HostKind, status: &Status) -> Json {
    let mut v = serde_json::to_value(status).unwrap_or_else(|_| json!({}));
    v["kind"] = json!(kind);
    v["pid"] = json!(std::process::id());
    v["version"] = json!(PROTOCOL_VERSION);
    v
}

fn dispatch(
    kind: HostKind,
    method: &str,
    params: Json,
    host: &mut dyn ControlHost,
) -> Result<Json, ControlError> {
    let no_params = |params: Json| parse::<NoParams>(method, params).map(|_| ());
    match method {
        "status" => {
            no_params(params)?;
            Ok(status_json(kind, &host.status()))
        }
        "world_query" => host.world_query(&parse(method, params)?),
        "editor_context" => {
            no_params(params)?;
            host.editor_context()
        }
        "undo_turn" => {
            no_params(params)?;
            host.undo_turn()
        }
        "apply_commands" => {
            let p: ApplyParams = parse(method, params)?;
            let commands = edit::parse_commands(&p.commands.to_string())
                .map_err(|e| ControlError::command_parse(&e))?;
            if p.save {
                if p.dry_run {
                    return Err(ControlError::invalid_params(
                        "`save` and `dry_run` cannot be combined",
                    ));
                }
                return host.apply_and_save(&commands);
            }
            host.apply_commands(&commands, p.dry_run)
        }
        "screenshot" => {
            let p: ScreenshotParams = parse(method, params)?;
            let size = match (p.width, p.height) {
                (None, None) => None,
                (Some(w), Some(h)) => {
                    let ok = |v: u32| (1..=MAX_SCREENSHOT_SIZE).contains(&v);
                    if !ok(w) || !ok(h) {
                        return Err(ControlError::invalid_params(format!(
                            "screenshot size must be 1..={MAX_SCREENSHOT_SIZE} pixels per side"
                        )));
                    }
                    Some((w, h))
                }
                _ => {
                    return Err(ControlError::invalid_params(
                        "give both `width` and `height`, or neither",
                    ));
                }
            };
            let view = match (p.eye, p.look_at) {
                (None, None) => {
                    if p.fov.is_some() {
                        return Err(ControlError::invalid_params(
                            "`fov` needs `eye` and `look_at`",
                        ));
                    }
                    None
                }
                (Some(eye), Some(look_at)) => {
                    let pose = CameraPose {
                        eye,
                        look_at,
                        fov: p.fov,
                    };
                    Some(pose.view_camera().map_err(ControlError::invalid_params)?)
                }
                _ => {
                    return Err(ControlError::invalid_params(
                        "give both `eye` and `look_at`, or neither",
                    ));
                }
            };
            let shot = host.screenshot(size, view.as_ref())?;
            Ok(json!({
                "mime_type": "image/png",
                "width": shot.width,
                "height": shot.height,
                "png_base64": base64_encode(&shot.png),
            }))
        }
        "play" | "stop" | "pause" => {
            no_params(params)?;
            let status = match method {
                "play" => host.play()?,
                "stop" => host.stop()?,
                _ => host.pause()?,
            };
            Ok(status_json(kind, &status))
        }
        "send_input" => {
            let p: InputParams = parse(method, params)?;
            send_input(host.play_world(method)?, &p)
        }
        "teleport" => {
            let p: TeleportParams = parse(method, params)?;
            teleport(host.play_world(method)?, &p.entity, p.position, p.yaw)
        }
        "record_start" => {
            let p: RecordStartParams = parse(method, params)?;
            if p.restart {
                host.restart_play(None, p.seed)?;
            } else if p.seed.is_some() {
                return Err(ControlError::invalid_params(
                    "`seed` applies to a restart: drop it or `restart: false`",
                ));
            }
            let world = host.play_world(method)?;
            if world.contains_resource::<crate::playtest::PlaytestRecorder>() {
                return Err(ControlError::invalid_params(
                    "already recording: call record_stop first",
                ));
            }
            let recorder = crate::playtest::PlaytestRecorder::new(
                world,
                p.player.as_deref(),
                !p.restart,
                p.seed,
            )?;
            world.insert_resource(recorder);
            tracing::info!(target: "box::control", restarted = p.restart, "recording a playtest");
            Ok(json!({
                "recording": true,
                "restarted": p.restart,
                "scene": crate::playtest::scene_relative(world),
            }))
        }
        "record_stop" => {
            let p: RecordStopParams = parse(method, params)?;
            let world = host.play_world(method)?;
            let Some(recorder) = world.remove_resource::<crate::playtest::PlaytestRecorder>()
            else {
                return Err(ControlError::invalid_params(
                    "not recording: call record_start first",
                ));
            };
            let mut playtest = recorder.finish(world);
            playtest.description = p.description;
            let mut out = json!({
                "frames": playtest.frames(),
                "steps": playtest.steps.len(),
                "checks": playtest.checks(),
                "playtest": serde_json::to_value(&playtest).unwrap_or(Json::Null),
            });
            if let Some(save) = &p.save {
                let root = world
                    .get_resource::<crate::scenes::Scenes>()
                    .and_then(crate::scenes::Scenes::project_root)
                    .map(Path::to_path_buf)
                    .ok_or_else(|| {
                        ControlError::new(code::UNSUPPORTED, "no project to save the playtest in")
                    })?;
                let path = crate::playtest::path_in_project(&root, save);
                crate::playtest::write(&path, &playtest)?;
                let rel = path.strip_prefix(&root).map_or_else(
                    |_| path.display().to_string(),
                    |r| r.to_string_lossy().replace('\\', "/"),
                );
                tracing::info!(target: "box::control", "recorded playtest saved to {rel}");
                out["file"] = json!(rel);
            }
            Ok(out)
        }
        "playtest" => {
            let p: PlaytestParams = parse(method, params)?;
            let (playtest, name) = match (p.file, p.playtest) {
                (Some(file), None) => {
                    let root = host
                        .play_world(method)
                        .ok()
                        .and_then(|w| w.get_resource::<crate::scenes::Scenes>())
                        .and_then(crate::scenes::Scenes::project_root)
                        .map(Path::to_path_buf)
                        .or_else(|| host.status().project.map(PathBuf::from))
                        .ok_or_else(|| {
                            ControlError::new(
                                code::UNSUPPORTED,
                                "no project to read playtests from",
                            )
                        })?;
                    let path = crate::playtest::path_in_project(&root, &file);
                    (
                        crate::playtest::Playtest::read(&path)?,
                        crate::playtest::name_of(&path),
                    )
                }
                (None, Some(inline)) => (
                    crate::playtest::Playtest::from_json(inline)?,
                    "inline".to_string(),
                ),
                _ => {
                    return Err(ControlError::invalid_params(
                        "give `file` (a playtest name or path) or `playtest` (inline)",
                    ));
                }
            };
            host.restart_play(playtest.scene.as_deref(), playtest.seed)?;
            let world = host.play_world(method)?;
            let total = playtest.frames();
            let checks = playtest.checks();
            let run = crate::playtest::PlaytestRun::new(playtest, &name, world)?;
            world.insert_resource(run);
            tracing::info!(target: "box::control", "replaying playtest `{name}` ({total} frames)");
            Ok(json!({
                "started": true,
                "name": name,
                "total_frames": total,
                "checks": checks,
                "note": "poll playtest_status until `finished`",
            }))
        }
        "playtest_status" => {
            no_params(params)?;
            let world = host.play_world(method)?;
            let Some(run) = world.get_resource::<crate::playtest::PlaytestRun>() else {
                return Err(ControlError::invalid_params(
                    "no playtest in this game: start one with `playtest`",
                ));
            };
            serde_json::to_value(run.report(world))
                .map_err(|e| ControlError::new(code::INTERNAL, e.to_string()))
        }
        "step_frames" => {
            let p: StepParams = parse(method, params)?;
            if !(1..=MAX_STEP_FRAMES).contains(&p.frames) {
                return Err(ControlError::invalid_params(format!(
                    "`frames` must be 1..={MAX_STEP_FRAMES}"
                )));
            }
            host.step_frames(p.frames)
        }
        "read_log" => {
            let p: LogParams = parse(method, params)?;
            let limit = p.limit.unwrap_or(200).min(10_000);
            let page = host.read_log(p.since, limit)?;
            serde_json::to_value(page).map_err(|e| ControlError::new(code::INTERNAL, e.to_string()))
        }
        "frame_stats" => {
            no_params(params)?;
            host.frame_stats()
        }
        "audio_levels" | "audio_solo" | "audio_mute" => audio_method(method, params, host),
        other => Err(
            ControlError::new(code::UNKNOWN_METHOD, format!("unknown method `{other}`")).with(
                "methods",
                json!([
                    "status",
                    "world_query",
                    "apply_commands",
                    "screenshot",
                    "play",
                    "stop",
                    "pause",
                    "step_frames",
                    "read_log",
                    "frame_stats",
                    "send_input",
                    "teleport",
                    "record_start",
                    "record_stop",
                    "playtest",
                    "playtest_status",
                    "audio_levels",
                    "audio_solo",
                    "audio_mute"
                ]),
            ),
        ),
    }
}

// ---------------------------------------------------------- client ----

/// A failed [`ControlClient`] call.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// No discovery file: nothing has opted in to live control.
    #[error(
        "no running editor or player for this project ({} does not exist); start one with live control on: `box-editor {project} --control` (or File ▸ Allow Agent Control in an open editor), `box-player {project} --control`, or `box run {project} --control`",
        .0.display(),
        project = project_of_discovery(.0)
    )]
    NotRunning(PathBuf),
    /// A discovery file exists but nothing answers on its port (the process
    /// exited without removing it).
    #[error(
        "stale control file {file}: nothing answers on 127.0.0.1:{port} (pid {pid}); start the editor or player again"
    )]
    Stale { file: PathBuf, port: u16, pid: u32 },
    #[error("no response within {0:?}")]
    Timeout(Duration),
    #[error("control protocol error: {0}")]
    Protocol(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The host answered with an error.
    #[error("{0}")]
    Remote(ControlError),
}

impl ClientError {
    /// The stable error code ([`code`]).
    pub fn code(&self) -> &str {
        match self {
            Self::NotRunning(_) | Self::Stale { .. } => code::NOT_RUNNING,
            Self::Timeout(_) => code::TIMEOUT,
            Self::Protocol(_) => code::PROTOCOL,
            Self::Io(_) => code::IO,
            Self::Remote(e) => &e.code,
        }
    }

    /// As a [`ControlError`] (remote errors keep their details).
    pub fn to_control_error(&self) -> ControlError {
        match self {
            Self::Remote(e) => e.clone(),
            other => ControlError::new(other.code(), other.to_string()),
        }
    }
}

/// Read and parse a project's discovery file.
pub fn read_discovery(project_root: &Path) -> Result<Discovery, ClientError> {
    let path = discovery_path(project_root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(ClientError::NotRunning(path)),
        Err(e) => return Err(ClientError::Io(e)),
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| ClientError::Protocol(format!("{}: {e}", path.display())))
}

/// A blocking client with timeouts, used by `box mcp`.
pub struct ControlClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    discovery: Discovery,
    timeout: Duration,
    next_id: u64,
}

impl ControlClient {
    /// Connect to the host announced in `<project_root>/.box/control.json`.
    pub fn connect_project(project_root: &Path, timeout: Duration) -> Result<Self, ClientError> {
        let discovery = read_discovery(project_root)?;
        Self::connect(discovery, timeout).map_err(|e| {
            // Nothing listens on the port. Windows retries a refused loopback
            // connect for about two seconds, so it can end in a timeout; a
            // live host accepts loopback connections at once.
            let stale = match &e {
                ClientError::Timeout(_) => true,
                ClientError::Io(io) => is_refused(io),
                _ => false,
            };
            if !stale {
                return e;
            }
            let d = read_discovery(project_root).ok();
            ClientError::Stale {
                file: discovery_path(project_root),
                port: d.as_ref().map_or(0, |d| d.port),
                pid: d.as_ref().map_or(0, |d| d.pid),
            }
        })
    }

    /// Connect to a host. `timeout` bounds the connection and each response.
    pub fn connect(discovery: Discovery, timeout: Duration) -> Result<Self, ClientError> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, discovery.port));
        let stream = TcpStream::connect_timeout(&addr, timeout).map_err(|e| {
            if e.kind() == io::ErrorKind::TimedOut {
                ClientError::Timeout(timeout)
            } else {
                ClientError::Io(e)
            }
        })?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            discovery,
            timeout,
            next_id: 1,
        })
    }

    pub fn discovery(&self) -> &Discovery {
        &self.discovery
    }

    /// Change the response timeout (for example for a slow screenshot).
    pub fn set_timeout(&mut self, timeout: Duration) -> Result<(), ClientError> {
        self.reader.get_ref().set_read_timeout(Some(timeout))?;
        self.writer.set_write_timeout(Some(timeout))?;
        self.timeout = timeout;
        Ok(())
    }

    /// Send one request and wait for its response.
    pub fn call(&mut self, method: &str, params: Json) -> Result<Json, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "id": id,
            "token": self.discovery.token,
            "method": method,
            "params": params,
        });
        let mut line =
            serde_json::to_vec(&request).map_err(|e| ClientError::Protocol(e.to_string()))?;
        line.push(b'\n');
        self.writer.write_all(&line).map_err(|e| self.io_error(e))?;
        self.writer.flush().map_err(|e| self.io_error(e))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let mut buf = Vec::new();
            let n = (&mut self.reader)
                .take(MAX_RESPONSE_BYTES as u64 + 1)
                .read_until(b'\n', &mut buf)
                .map_err(|e| self.io_error(e))?;
            if n == 0 {
                return Err(ClientError::Protocol(
                    "the host closed the connection".into(),
                ));
            }
            if buf.last() != Some(&b'\n') {
                return Err(ClientError::Protocol(format!(
                    "response larger than {MAX_RESPONSE_BYTES} bytes or truncated"
                )));
            }
            let response: Json = serde_json::from_slice(&buf)
                .map_err(|e| ClientError::Protocol(format!("invalid response JSON: {e}")))?;
            let rid = response.get("id");
            // An id-less error (busy, too large, bad request) answers us.
            if rid == Some(&json!(id)) || rid.is_none_or(Json::is_null) {
                if let Some(err) = response.get("error") {
                    let err: ControlError = serde_json::from_value(err.clone())
                        .map_err(|e| ClientError::Protocol(format!("invalid error: {e}")))?;
                    return Err(ClientError::Remote(err));
                }
                return response
                    .get("result")
                    .cloned()
                    .ok_or_else(|| ClientError::Protocol("response has no `result`".into()));
            }
            // A late answer to an earlier, timed-out request: skip it.
            if Instant::now() > deadline {
                return Err(ClientError::Timeout(self.timeout));
            }
        }
    }

    fn io_error(&self, e: io::Error) -> ClientError {
        match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                ClientError::Timeout(self.timeout)
            }
            _ => ClientError::Io(e),
        }
    }
}

fn is_refused(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::AddrNotAvailable
    )
}

// ---------------------------------------------------------- base64 ----

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648), for PNGs in JSON.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode [`base64_encode`] output. `None` on invalid input.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=').as_bytes();
    if text.len() % 4 == 1 {
        return None;
    }
    let value = |c: u8| B64.iter().position(|&b| b == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for chunk in text.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len() - 1]);
    }
    Some(out)
}

// ---------------------------------------------------- headless host ----

/// Fixed step of [`HeadlessHost`] Play sessions.
pub const FIXED_DT: f32 = 1.0 / 60.0;

/// A complete [`ControlHost`] for a game without a window: an edit world with
/// an undo stack, Play sessions built from the [`Game`] on a copy of it, and
/// screenshots through an optional headless [`Renderer`]. Tests use it; it is
/// also the reference for editor and player integrations.
///
/// - Edit mode: `world_query` and `apply_commands` act on the edit world;
///   commands are undoable on [`HeadlessHost::edit_stack`].
/// - `play` copies the edit world (saved data only, like the editor) into a
///   fresh app from [`Game::app`] and runs its first frame; [`update`](Self::update)
///   then advances it one frame per call. Commands during Play edit the play
///   world and are discarded by `stop`.
/// - `pause` freezes the session; `step_frames` advances it synchronously.
pub struct HeadlessHost {
    game: Game,
    edit: crate::App,
    edit_stack: EditStack,
    play: Option<crate::App>,
    paused: bool,
    frame: u64,
    renderer: Option<Renderer>,
    log: Vec<LogEntry>,
    started: Instant,
    project: Option<String>,
    scene: Option<String>,
}

impl HeadlessHost {
    /// A host editing `world` (usually a loaded scene) for `game`.
    pub fn new(game: Game, world: World) -> Self {
        let edit = crate::app_with_world(world);
        Self {
            game,
            edit,
            edit_stack: EditStack::new(),
            play: None,
            paused: false,
            frame: 0,
            renderer: None,
            log: Vec::new(),
            started: Instant::now(),
            project: None,
            scene: None,
        }
    }

    /// Enable `screenshot` with a headless renderer.
    pub fn with_renderer(mut self, renderer: Renderer) -> Self {
        self.renderer = Some(renderer);
        self
    }

    /// Reported by `status`.
    pub fn with_location(mut self, project: Option<&Path>, scene: Option<&str>) -> Self {
        self.project = project.map(|p| p.display().to_string());
        self.scene = scene.map(str::to_string);
        self
    }

    pub fn mode(&self) -> Mode {
        match (&self.play, self.paused) {
            (None, _) => Mode::Edit,
            (Some(_), false) => Mode::Play,
            (Some(_), true) => Mode::Paused,
        }
    }

    /// The edit world.
    pub fn edit_world(&self) -> &World {
        &self.edit.world
    }

    /// The Play session's world, if one runs.
    pub fn play_world(&self) -> Option<&World> {
        self.play.as_ref().map(|a| &a.world)
    }

    pub fn edit_stack(&self) -> &EditStack {
        &self.edit_stack
    }

    /// Add a record to the log `read_log` returns.
    pub fn log(&mut self, level: &str, message: impl Into<String>) {
        let message = message.into();
        tracing::info!(target: "box::control", "{message}");
        self.log.push(LogEntry {
            seq: self.log.len() as u64,
            time: self.started.elapsed().as_secs_f64(),
            level: level.to_string(),
            target: "box::control".to_string(),
            message,
        });
    }

    /// One frame: advances a running Play session by [`FIXED_DT`]. Does
    /// nothing in Edit mode or while paused.
    pub fn update(&mut self) {
        if let (Some(app), false) = (&mut self.play, self.paused) {
            app.tick(FIXED_DT);
            // End the input frame, as a runner does.
            if let Some(input) = app.world.get_resource_mut::<box_core::input::Input>() {
                input.end_frame();
            }
            self.frame += 1;
        }
    }

    fn active_app(&mut self) -> &mut crate::App {
        self.play.as_mut().unwrap_or(&mut self.edit)
    }
}

impl ControlHost for HeadlessHost {
    fn status(&mut self) -> Status {
        Status {
            mode: self.mode(),
            scene: self.scene.clone(),
            project: self.project.clone(),
            frame: self.frame,
            undo: Some(self.edit_stack.undo_len()),
        }
    }

    fn world_query(&mut self, query: &WorldQuery) -> Result<Json, ControlError> {
        query_world(&self.active_app().world, query)
    }

    fn apply_commands(
        &mut self,
        commands: &[Command],
        dry_run: bool,
    ) -> Result<Json, ControlError> {
        let result = match &mut self.play {
            // Play-world edits are not undoable in the editor and end with the session.
            Some(app) => apply_to_world(
                &mut app.world,
                &mut EditStack::new(),
                commands,
                dry_run,
                "control",
            ),
            None => apply_to_world(
                &mut self.edit.world,
                &mut self.edit_stack,
                commands,
                dry_run,
                "control",
            ),
        }?;
        if !dry_run {
            self.log("info", format!("applied {} command(s)", commands.len()));
        }
        Ok(result)
    }

    fn screenshot(
        &mut self,
        size: Option<(u32, u32)>,
        view: Option<&ViewCamera>,
    ) -> Result<Screenshot, ControlError> {
        let editing = self.play.is_none();
        let Some(renderer) = self.renderer.as_mut() else {
            return Err(ControlError::new(
                code::UNSUPPORTED,
                "this host has no renderer; screenshots are unavailable",
            ));
        };
        if let Some((w, h)) = size {
            renderer.resize(w, h);
        }
        let app = self.play.as_mut().unwrap_or(&mut self.edit);
        if editing {
            // Meshes and global transforms of the edit world are derived in PostUpdate.
            app.update();
        }
        let image = match view {
            Some(view) => {
                let (w, h) = renderer.size();
                let target = renderer.create_viewport_target(w, h);
                renderer.render_to(&mut app.world, &target, Some(view));
                renderer.read_viewport_target(&target)
            }
            None => renderer.capture(&mut app.world),
        }
        .map_err(|e| ControlError::new(code::RENDER, e.to_string()))?;
        Screenshot::from_rgba(&image)
    }

    fn play(&mut self) -> Result<Status, ControlError> {
        if self.play.is_none() {
            let world = scene::clone_world(&self.edit.world)
                .map_err(|e| ControlError::new(code::INTERNAL, format!("play snapshot: {e}")))?;
            let mut app = self.game.app(world);
            app.tick(FIXED_DT);
            self.play = Some(app);
            self.frame = 1;
            self.log("info", "play");
        } else if self.paused {
            self.log("info", "resume");
        }
        self.paused = false;
        Ok(self.status())
    }

    fn restart_play(&mut self, scene: Option<&str>, seed: Option<u32>) -> Result<(), ControlError> {
        if let Some(scene) = scene
            && self.scene.as_deref() != Some(scene)
        {
            return Err(ControlError::invalid_params(format!(
                "this host plays {}, not `{scene}`",
                self.scene.as_deref().unwrap_or("an unnamed scene")
            )));
        }
        #[cfg_attr(not(feature = "script"), allow(unused_mut))]
        let mut world = scene::clone_world(&self.edit.world)
            .map_err(|e| ControlError::new(code::INTERNAL, format!("play snapshot: {e}")))?;
        #[cfg(feature = "script")]
        if let Some(seed) = seed {
            world.insert_resource(box_script::ScriptSeed(seed));
        }
        #[cfg(not(feature = "script"))]
        let _ = seed;
        self.play = Some(self.game.app(world));
        self.paused = false;
        self.frame = 0;
        self.log("info", "restart");
        Ok(())
    }

    fn stop(&mut self) -> Result<Status, ControlError> {
        if self.play.take().is_some() {
            self.log("info", "stop");
        }
        self.paused = false;
        self.frame = 0;
        Ok(self.status())
    }

    fn pause(&mut self) -> Result<Status, ControlError> {
        if self.play.is_none() {
            return Err(ControlError::not_playing("pause"));
        }
        if !self.paused {
            self.paused = true;
            self.log("info", "pause");
        }
        Ok(self.status())
    }

    fn step_frames(&mut self, frames: u32) -> Result<Json, ControlError> {
        let Some(app) = self.play.as_mut() else {
            return Err(ControlError::not_playing("step_frames"));
        };
        for _ in 0..frames {
            app.tick(FIXED_DT);
            // End the input frame, as a runner does, so actions are
            // evaluated again next frame (send_input).
            if let Some(input) = app.world.get_resource_mut::<box_core::input::Input>() {
                input.end_frame();
            }
        }
        self.frame += u64::from(frames);
        Ok(json!({ "frames": frames, "frame": self.frame }))
    }

    fn read_log(&mut self, since: u64, limit: usize) -> Result<LogPage, ControlError> {
        let entries: Vec<LogEntry> = self
            .log
            .iter()
            .filter(|e| e.seq >= since)
            .take(limit)
            .cloned()
            .collect();
        let next = entries
            .last()
            .map_or(since.max(self.log.len() as u64), |e| e.seq + 1);
        Ok(LogPage { entries, next })
    }

    fn play_world(&mut self, method: &str) -> Result<&mut World, ControlError> {
        match self.play.as_mut() {
            Some(app) => Ok(&mut app.world),
            None => Err(ControlError::not_playing(method)),
        }
    }

    fn frame_stats(&mut self) -> Result<Json, ControlError> {
        let mode = self.mode();
        let entities = self.active_app().world.entity_count();
        let renderer = self.renderer.as_ref().map(|r| {
            let s = r.stats();
            json!({
                "objects": s.objects,
                "visible": s.visible,
                "draws": s.draws,
                "triangles": s.triangles,
                "validation_errors": s.validation_errors,
                "cpu_render_ms": s.cpu_render_ms,
            })
        });
        Ok(json!({
            "mode": mode,
            "frame": self.frame,
            "entities": entities,
            "renderer": renderer,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for (raw, enc) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw), enc);
            assert_eq!(base64_decode(enc).as_deref(), Some(raw));
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(base64_decode(&base64_encode(&all)), Some(all));
        assert_eq!(base64_decode("Zm9v!"), None);
        assert_eq!(base64_decode("Z"), None);
    }

    #[test]
    fn headless_host_update_advances_only_a_running_session() {
        let mut host = HeadlessHost::new(Game::new(), crate::project::starter_scene());
        host.update();
        assert_eq!(host.status().frame, 0);
        assert_eq!(host.play().unwrap().frame, 1);
        host.update();
        host.update();
        assert_eq!(host.status().frame, 3);
        host.pause().unwrap();
        host.update();
        assert_eq!(host.status().frame, 3);
        assert_eq!(host.stop().unwrap().mode, Mode::Edit);
        assert!(host.play_world().is_none());
    }

    #[test]
    fn token_comparison() {
        assert!(token_matches("abc", "abc"));
        assert!(!token_matches("abc", "abd"));
        assert!(!token_matches("abc", "ab"));
        assert_eq!(new_token().len(), 64);
        assert_ne!(new_token(), new_token());
    }
}
