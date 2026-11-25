//! Playtests: a run of a game, recorded or written by hand, replayed from
//! the same start with checks, so an agent (or a person) gets the same
//! pass/fail report after every change.
//!
//! A playtest file (`<project>/playtests/<name>.playtest.json`, [`Playtest`])
//! holds the start state and steps:
//!
//! ```json
//! {
//!   "version": 1,
//!   "scene": "assets/scenes/arena.scene.json",
//!   "seed": 1,
//!   "teleport": { "entity": "Player", "position": [0, 1, 4], "yaw": 0 },
//!   "steps": [
//!     { "input": { "move": [0, 1] }, "frames": 90 },
//!     { "input": { "move": [0, 1], "jump": true }, "frames": 1 },
//!     { "wait": 30 },
//!     { "check": "entity_near", "entity": "Player", "point": [0, 0, -2], "radius": 1 },
//!     { "check": "entity_gone", "entity": "Gem 1" },
//!     { "check": "event_seen", "event": "PlaySound" },
//!     { "check": "log_contains", "text": "collected" },
//!     { "check": "component_equals", "entity": "Player", "path": "Transform.translation.y", "value": 0.0, "tolerance": 0.1 }
//!   ]
//! }
//! ```
//!
//! **Input is named actions** of the game's `ActionMap`, never keys: `true`
//! holds a button, a number sets a 1D axis, `[x, y]` a 2D axis (+y forward).
//! An input step holds its actions for `frames` frames (a new press only
//! where a button was not held the frame before); unnamed actions read only
//! their bindings. `mouse` adds `[dx, dy]` mouse motion per frame (mouse
//! look; it locks the cursor, since games read the mouse only then).
//!
//! **Frames are 1/60 s.** `box playtest` replays headless with the fixed
//! 1/60 s step (one frame per playtest frame) and waits for the assets a
//! frame asks for (a script's file, a data table) before the next, so a
//! replay is deterministic: the same file gives the same report and the
//! same [`fingerprint`](PlaytestReport::fingerprint). A live replay (control
//! method `playtest`) and a recording follow real time: a frame shorter
//! than 1/60 s repeats the current input, a longer one covers several
//! playtest frames.
//!
//! **The start state is the scene load.** A replay loads the scene fresh
//! (the start scene without `scene`), seeds the scripts' `math.random`
//! with `seed` (1 without it), moves `teleport`'s entity before the first
//! frame, then runs the steps from the game's first frame. Everything else
//! comes from the scene file: the camera's starting pose (its saved
//! transform, from which a third-person camera takes its yaw) is part of
//! it, so camera-relative movement replays the same way as long as the
//! steps carry the look input too (`look` actions, `mouse`). Recordings
//! therefore start by restarting the game from its scene.
//!
//! **Checks** run between frames, after the frames before them, and report
//! the frame, what was expected and what was found:
//! `entity_near` (an entity's world position within `radius` of `point`;
//! `horizontal: true` ignores height), `entity_gone` (despawned or hidden
//! with `Visible(false)`), `entity_exists`, `component_equals` (a reflected
//! field path `Component.field...` of an entity, or of a `resource`, equals
//! `value`: numbers within `tolerance`, objects compare the fields given),
//! `event_seen` (a registered event type sent since the start, optionally
//! with matching `fields`, at least `at_least` times) and `log_contains`
//! (a log message since the start contains `text`, optionally at `level`).
//!
//! Recording ([`PlaytestRecorder`]) saves the action values a playing world
//! evaluated each frame, injected (`send_input`) or bound (keys, gamepads),
//! resampled to 1/60 s frames, and appends checks for what changed:
//! `entity_gone` for named scene entities that disappeared and
//! `entity_near` for where the player ended.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use box_core::action::{ActionKind, ActionMap, Actions, InjectedInput};
use box_core::identity::{Name, Uid};
use box_core::input::{CursorOptions, Input};
use box_core::time::Time;
use box_core::transform::{GlobalTransform, Transform};
use box_core::visibility::Visible;
use box_core::world::{Entity, World};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value as Json, json};

use crate::control::{ControlError, LogEntry};
use crate::log_capture;
use crate::scenes::Scenes;

/// Version of the playtest file format.
pub const VERSION: u32 = 1;
/// One playtest frame, seconds.
pub const FRAME: f32 = 1.0 / 60.0;
/// File name suffix of playtests.
pub const EXTENSION: &str = ".playtest.json";
/// Folder of a project's playtests.
pub const DIR: &str = "playtests";
/// Longest playtest, in frames (one hour).
pub const MAX_FRAMES: u64 = 60 * 60 * 60;
/// Default `radius` of `entity_near`, metres.
pub const DEFAULT_RADIUS: f32 = 1.0;
/// Default `tolerance` of `component_equals`.
pub const DEFAULT_TOLERANCE: f64 = 1e-4;
/// Log records and events kept for checks.
const MAX_KEPT: usize = 100_000;
/// `entity_gone` checks a recording appends at most.
const MAX_GONE_CHECKS: usize = 100;

// ------------------------------------------------------------ format ----

/// A playtest file. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Playtest {
    #[serde(default = "version_one")]
    pub version: u32,
    /// What the run does, for people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Scene to start in (a path relative to the project root or a scene
    /// name); the project's start scene without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// Seed of the scripts' `math.random` (1 without it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u32>,
    /// Move an entity (the player) before the first frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teleport: Option<Teleport>,
    pub steps: Vec<Step>,
}

fn version_one() -> u32 {
    VERSION
}

/// The start position of an entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Teleport {
    /// Uid or unique name.
    pub entity: String,
    pub position: [f32; 3],
    /// Degrees about +Y (0 faces -Z).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
}

/// One step: input for some frames, frames without input, or a check.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Input(InputStep),
    /// Frames without playtest input.
    Wait(u32),
    Check(Check),
}

/// `{"input": {action: value}, "mouse"?: [dx, dy], "frames": n}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputStep {
    /// Action name to value: `true` (button held), a number (1D axis),
    /// `[x, y]` (2D axis, +y forward).
    #[serde(default)]
    pub input: BTreeMap<String, ActionValue>,
    /// Mouse motion per frame, pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mouse: Option<[f32; 2]>,
    pub frames: u32,
}

/// The value of one action in an input step.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionValue {
    /// A button: `true` is held (`false` is the same as leaving it out).
    Button(bool),
    Axis1(f32),
    Axis2([f32; 2]),
}

/// A check between frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "check", rename_all = "snake_case", deny_unknown_fields)]
pub enum Check {
    /// The entity's world position is within `radius` of `point`.
    EntityNear {
        entity: String,
        point: [f32; 3],
        #[serde(default = "default_radius")]
        radius: f32,
        /// Compare x and z only.
        #[serde(default, skip_serializing_if = "is_false")]
        horizontal: bool,
    },
    /// No such entity, or it is hidden (`Visible(false)`).
    EntityGone { entity: String },
    /// The entity exists.
    EntityExists { entity: String },
    /// A reflected value equals `value`. With `entity`, `path` starts with
    /// the component (`Health.current`); with `resource`, it is a path in
    /// the resource (empty for the whole value).
    ComponentEquals {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource: Option<String>,
        #[serde(default)]
        path: String,
        value: Json,
        /// Allowed difference of numbers (default 1e-4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<f64>,
    },
    /// A registered event type was sent since the start.
    EventSeen {
        event: String,
        /// Fields the event must have (a subset of its value).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fields: Option<Json>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        at_least: u32,
    },
    /// A log message since the start contains `text`.
    LogContains {
        text: String,
        /// `error`, `warn`, `info`, `debug` or `trace`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        level: Option<String>,
    },
}

fn default_radius() -> f32 {
    DEFAULT_RADIUS
}

fn one() -> u32 {
    1
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl Check {
    /// The `check` name (`entity_near`).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::EntityNear { .. } => "entity_near",
            Self::EntityGone { .. } => "entity_gone",
            Self::EntityExists { .. } => "entity_exists",
            Self::ComponentEquals { .. } => "component_equals",
            Self::EventSeen { .. } => "event_seen",
            Self::LogContains { .. } => "log_contains",
        }
    }
}

impl Step {
    fn from_json(value: Json) -> Result<Self, String> {
        let Json::Object(obj) = &value else {
            return Err(
                "a step is an object: {\"input\": {...}, \"frames\": n}, {\"wait\": n} or {\"check\": ...}"
                    .into(),
            );
        };
        if obj.contains_key("check") {
            return serde_json::from_value(value)
                .map(Step::Check)
                .map_err(|e| e.to_string());
        }
        if obj.contains_key("input") || obj.contains_key("mouse") {
            return serde_json::from_value(value)
                .map(Step::Input)
                .map_err(|e| e.to_string());
        }
        if let Some(w) = obj.get("wait") {
            if obj.len() != 1 {
                return Err("a wait step has only `wait` (frames)".into());
            }
            return w
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Step::Wait)
                .ok_or_else(|| "`wait` is a number of frames".into());
        }
        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        Err(format!(
            "unknown step with keys {keys:?}: a step has `input`, `wait` or `check`"
        ))
    }

    /// Frames the step runs (0 for a check).
    pub fn frames(&self) -> u32 {
        match self {
            Self::Input(s) => s.frames,
            Self::Wait(n) => *n,
            Self::Check(_) => 0,
        }
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Step::from_json(Json::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Step {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Input(step) => step.serialize(s),
            Self::Wait(n) => json!({ "wait": n }).serialize(s),
            Self::Check(c) => c.serialize(s),
        }
    }
}

impl Playtest {
    /// Parse a playtest file's text; errors name the step.
    pub fn parse(text: &str) -> Result<Self, PlaytestError> {
        let value: Json = serde_json::from_str(text)
            .map_err(|e| PlaytestError::invalid(format!("not valid JSON: {e}")))?;
        Self::from_json(value)
    }

    /// [`parse`](Self::parse) for a JSON value.
    pub fn from_json(mut value: Json) -> Result<Self, PlaytestError> {
        let steps = match value.get_mut("steps").map(Json::take) {
            Some(Json::Array(steps)) => steps,
            Some(_) => return Err(PlaytestError::invalid("`steps` must be an array")),
            None => return Err(PlaytestError::invalid("missing `steps`")),
        };
        let mut parsed = Vec::with_capacity(steps.len());
        for (i, step) in steps.into_iter().enumerate() {
            parsed.push(
                Step::from_json(step)
                    .map_err(|e| PlaytestError::invalid(format!("step {i}: {e}")))?,
            );
        }
        value["steps"] = json!([]);
        let mut playtest: Playtest =
            serde_json::from_value(value).map_err(|e| PlaytestError::invalid(e.to_string()))?;
        playtest.steps = parsed;
        if playtest.version > VERSION {
            return Err(PlaytestError::invalid(format!(
                "playtest version {} is newer than this engine's ({VERSION}); update the engine",
                playtest.version
            )));
        }
        playtest.check_frames()?;
        Ok(playtest)
    }

    /// Read and parse a file.
    pub fn read(path: &Path) -> Result<Self, PlaytestError> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            PlaytestError::new(
                "io",
                format!(
                    "could not read the playtest {}: {}",
                    path.display(),
                    crate::io_hint::text(&e)
                ),
            )
        })?;
        Self::parse(&text).map_err(|e| PlaytestError {
            message: format!("{}: {}", path.display(), e.message),
            ..e
        })
    }

    /// Pretty JSON, as written to files.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Total frames of the steps.
    pub fn frames(&self) -> u64 {
        self.steps.iter().map(|s| u64::from(s.frames())).sum()
    }

    /// Number of checks.
    pub fn checks(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s, Step::Check(_)))
            .count()
    }

    fn check_frames(&self) -> Result<(), PlaytestError> {
        for (i, step) in self.steps.iter().enumerate() {
            if !matches!(step, Step::Check(_)) && step.frames() == 0 {
                return Err(PlaytestError::invalid(format!(
                    "step {i}: `frames` (or `wait`) must be at least 1"
                )));
            }
            if let Step::Check(Check::EntityNear { radius, .. }) = step
                && !(radius.is_finite() && *radius >= 0.0)
            {
                return Err(PlaytestError::invalid(format!(
                    "step {i}: `radius` must be a number >= 0"
                )));
            }
        }
        if self.frames() > MAX_FRAMES {
            return Err(PlaytestError::invalid(format!(
                "the steps add up to {} frames; at most {MAX_FRAMES} (an hour)",
                self.frames()
            )));
        }
        Ok(())
    }

    /// Check the steps against a playing world before a run: every action
    /// exists with a value of its kind, every event of `event_seen` is
    /// registered, the teleport entity exists.
    pub fn validate(&self, world: &World) -> Result<(), PlaytestError> {
        for (i, step) in self.steps.iter().enumerate() {
            match step {
                Step::Input(s) => {
                    for (name, value) in &s.input {
                        let kind = crate::control::action_kind(world, name).map_err(|e| {
                            PlaytestError::new("unknown_action", format!("step {i}: {}", e.message))
                        })?;
                        let ok = matches!(
                            (&kind, value),
                            (ActionKind::Button { .. }, ActionValue::Button(_))
                                | (ActionKind::Axis1 { .. }, ActionValue::Axis1(_))
                                | (ActionKind::Axis2 { .. }, ActionValue::Axis2(_))
                        );
                        if !ok {
                            let want = match kind {
                                ActionKind::Button { .. } => "a button: give `true`",
                                ActionKind::Axis1 { .. } => "a 1D axis: give a number in -1..1",
                                ActionKind::Axis2 { .. } => {
                                    "a 2D axis: give [x, y] (+y is forward)"
                                }
                            };
                            return Err(PlaytestError::invalid(format!(
                                "step {i}: `{name}` is {want}"
                            )));
                        }
                    }
                    if let Some(m) = s.mouse
                        && !(m[0].is_finite() && m[1].is_finite())
                    {
                        return Err(PlaytestError::invalid(format!(
                            "step {i}: `mouse` must be finite"
                        )));
                    }
                }
                Step::Check(Check::EventSeen { event, .. })
                    if world.registry().event(event).is_none() =>
                {
                    let names: Vec<&str> = world.registry().events().map(|e| e.name()).collect();
                    let mut message = format!(
                        "step {i}: `{event}` is not a registered event type (registered: {})",
                        names.join(", ")
                    );
                    if let Some(g) = box_core::reflect::did_you_mean(event, names.iter().copied()) {
                        message.push_str(&format!("; did you mean `{g}`?"));
                    }
                    message.push_str(
                        "; script signals are not events: check what they cause, or log and use log_contains",
                    );
                    return Err(PlaytestError::new("unknown_event", message));
                }
                Step::Check(Check::ComponentEquals {
                    entity, resource, ..
                }) if entity.is_some() == resource.is_some() => {
                    return Err(PlaytestError::invalid(format!(
                        "step {i}: component_equals takes `entity` or `resource`"
                    )));
                }
                _ => {}
            }
        }
        if let Some(t) = &self.teleport {
            box_core::edit::resolve_entity(world, &t.entity)
                .map_err(|e| PlaytestError::new("entity_not_found", format!("teleport: {e}")))?;
        }
        Ok(())
    }
}

/// A playtest that cannot run (a bad file, an unknown action), with a
/// stable code.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PlaytestError {
    /// `playtest_invalid`, `unknown_action`, `unknown_event`,
    /// `entity_not_found`, `io`, `load`, `render`.
    pub code: &'static str,
    pub message: String,
}

impl PlaytestError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("playtest_invalid", message)
    }
}

impl From<PlaytestError> for ControlError {
    fn from(e: PlaytestError) -> Self {
        ControlError::new(e.code, e.message)
    }
}

// ------------------------------------------------------------ report ----

/// The outcome of one check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Index of the step in the playtest.
    pub step: usize,
    /// Playtest frames run before the check.
    pub frame: u64,
    /// The `check` name.
    pub check: String,
    pub passed: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub expected: Json,
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub actual: Json,
}

/// What a replay found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaytestReport {
    /// The playtest's name (its file name without `.playtest.json`).
    pub name: String,
    /// Every step ran and every check passed.
    pub passed: bool,
    /// Every step ran (a live replay reports progress before that).
    pub finished: bool,
    /// Playtest frames run.
    pub frames: u64,
    /// Frames of all steps.
    pub total_frames: u64,
    pub checks: Vec<CheckResult>,
    pub failed: usize,
    /// A hash of the names and world positions (to the millimetre) of the
    /// entities with a uid at the end, then their count: equal
    /// fingerprints mean the run ended in the same state.
    pub fingerprint: String,
    /// PNGs written (`--capture-every`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub captures: Vec<String>,
}

impl PlaytestReport {
    /// Human-readable lines (`box playtest` without `--json`).
    pub fn text(&self) -> String {
        let mut lines = vec![format!(
            "playtest `{}`: {} ({} of {} frames, {} check(s), {} failed, fingerprint {})",
            self.name,
            if self.passed { "PASSED" } else { "FAILED" },
            self.frames,
            self.total_frames,
            self.checks.len(),
            self.failed,
            self.fingerprint
        )];
        for c in &self.checks {
            lines.push(format!(
                "  {} step {} frame {}: {}: {}",
                if c.passed { "ok  " } else { "FAIL" },
                c.step,
                c.frame,
                c.check,
                c.message
            ));
        }
        if !self.finished {
            lines.push("  (did not finish: the steps did not all run)".into());
        }
        if let Some(last) = self.captures.last() {
            lines.push(format!("  {} capture(s), last {last}", self.captures.len()));
        }
        lines.join("\n")
    }
}

/// [`PlaytestReport::fingerprint`] of a world.
pub fn fingerprint(world: &World) -> String {
    // Keyed by name, not uid: entities spawned at run time get a fresh uid
    // every run.
    let mut rows: Vec<(String, [i64; 3])> = world
        .query::<(Entity, &Uid)>()
        .iter()
        .map(|(e, _)| {
            let p = position(world, e).unwrap_or(Vec3::ZERO);
            let mm = |v: f32| (f64::from(v) * 1000.0).round() as i64;
            let name = world
                .get::<Name>(e)
                .map(|n| n.to_string())
                .unwrap_or_default();
            (name, [mm(p.x), mm(p.y), mm(p.z)])
        })
        .collect();
    rows.sort();
    // FNV-1a, 64 bit: stable across runs and platforms.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for (name, p) in &rows {
        eat(name.as_bytes());
        eat(&[0]);
        for v in p {
            eat(&v.to_le_bytes());
        }
    }
    format!("{hash:016x}-{}", rows.len())
}

// ------------------------------------------------------------ replay ----

/// The input of one frame, merged from the playtest frames it covers.
#[derive(Default)]
struct FrameInput {
    actions: BTreeMap<String, InjectedInput>,
    mouse: Vec2,
}

impl FrameInput {
    fn merge(&mut self, step: &InputStep, with_mouse: bool) {
        for (name, value) in &step.input {
            let input = match *value {
                ActionValue::Button(false) => continue,
                ActionValue::Button(true) => InjectedInput::press(1),
                ActionValue::Axis1(x) => InjectedInput::axis(Vec2::new(x, 0.0), 1),
                ActionValue::Axis2([x, y]) => InjectedInput::axis(Vec2::new(x, y), 1),
            };
            self.actions.insert(name.clone(), input);
        }
        if with_mouse && let Some([x, y]) = step.mouse {
            self.mouse += Vec2::new(x, y);
        }
    }
}

/// Resource: a playtest replaying in this world. Each frame, before actions
/// are evaluated, [`playtest_frame_system`] runs the checks that are due
/// and injects the frame's input; [`crate::InputPlugin`] adds that system.
/// Read the progress with [`report`](Self::report).
pub struct PlaytestRun {
    playtest: Playtest,
    name: String,
    /// Next step to start.
    next: usize,
    /// The input or wait step running, and its frames left.
    current: Option<usize>,
    remaining: u32,
    /// Playtest frames run.
    frames: u64,
    /// Real seconds run (frames advance by it).
    elapsed: f64,
    generation: Option<u64>,
    finished: bool,
    results: Vec<CheckResult>,
    log_next: u64,
    logs: Vec<LogEntry>,
    /// Per event type of `event_seen` checks: read cursor and values seen.
    events: BTreeMap<String, (u64, Vec<Json>)>,
}

impl PlaytestRun {
    /// Validate `playtest` against the playing `world` and prepare the run:
    /// apply the teleport and start collecting logs and events. Insert the
    /// result as a resource before the first frame.
    pub fn new(playtest: Playtest, name: &str, world: &mut World) -> Result<Self, PlaytestError> {
        playtest.validate(world)?;
        if let Some(t) = &playtest.teleport {
            crate::control::teleport(world, &t.entity, t.position, t.yaw)
                .map_err(|e| PlaytestError::new("entity_not_found", e.message))?;
        }
        log_capture::enable();
        let log_next = log_capture::next_seq();
        let mut events = BTreeMap::new();
        for step in &playtest.steps {
            if let Step::Check(Check::EventSeen { event, .. }) = step
                && !events.contains_key(event)
                && let Some(ty) = world.registry().event(event)
            {
                let mut cursor = u64::MAX;
                ty.read(world, &mut cursor);
                events.insert(event.clone(), (cursor, Vec::new()));
            }
        }
        Ok(Self {
            playtest,
            name: name.to_string(),
            next: 0,
            current: None,
            remaining: 0,
            frames: 0,
            elapsed: 0.0,
            generation: None,
            finished: false,
            results: Vec::new(),
            log_next,
            logs: Vec::new(),
            events,
        })
    }

    pub fn playtest(&self) -> &Playtest {
        &self.playtest
    }

    /// Every step ran.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// The report so far (final once [`is_finished`](Self::is_finished)).
    pub fn report(&self, world: &World) -> PlaytestReport {
        let failed = self.results.iter().filter(|r| !r.passed).count();
        PlaytestReport {
            name: self.name.clone(),
            passed: self.finished && failed == 0,
            finished: self.finished,
            frames: self.frames,
            total_frames: self.playtest.frames(),
            checks: self.results.clone(),
            failed,
            fingerprint: fingerprint(world),
            captures: Vec::new(),
        }
    }

    /// Run the checks due before the next frame and start the next input or
    /// wait step; finishes the run when no step is left. Call between
    /// frames (the frame system does, and a headless replay before each
    /// frame and after the last).
    pub fn settle(&mut self, world: &mut World) {
        self.collect(world);
        while self.remaining == 0 && !self.finished {
            let Some(step) = self.playtest.steps.get(self.next) else {
                self.finished = true;
                self.current = None;
                if let Some(actions) = world.get_resource_mut::<Actions>() {
                    actions.clear_injected();
                }
                break;
            };
            let index = self.next;
            self.next += 1;
            match step {
                Step::Check(check) => {
                    let result = self.evaluate(index, check, world);
                    self.results.push(result);
                }
                Step::Wait(n) => {
                    self.current = Some(index);
                    self.remaining = *n;
                }
                Step::Input(s) => {
                    self.current = Some(index);
                    self.remaining = s.frames;
                }
            }
        }
    }

    /// The start of a frame `dt` real seconds long: checks that are due,
    /// then the input of the playtest frames it covers.
    fn frame_start(&mut self, world: &mut World, dt: f32) {
        if self.finished {
            return;
        }
        let target = ((self.elapsed + f64::from(dt)) / f64::from(FRAME) + 1e-6).floor() as u64;
        self.elapsed += f64::from(dt);
        let covered = target.saturating_sub(self.frames);
        let mut input = FrameInput::default();
        if covered == 0 {
            // Shorter than a playtest frame: keep holding the current input.
            if let Some(Step::Input(s)) = self.current.map(|i| &self.playtest.steps[i])
                && self.remaining > 0
            {
                input.merge(s, false);
            }
        }
        for _ in 0..covered {
            self.settle(world);
            let Some(i) = self.current.filter(|_| self.remaining > 0) else {
                break;
            };
            if let Step::Input(s) = &self.playtest.steps[i] {
                input.merge(s, true);
            }
            self.remaining -= 1;
            self.frames += 1;
        }
        if self.finished {
            return;
        }
        if let Some(actions) = world.get_resource_mut::<Actions>() {
            actions.clear_injected();
            for (name, i) in &input.actions {
                actions.inject(name, *i);
            }
        }
        if input.mouse != Vec2::ZERO {
            if let Some(cursor) = world.get_resource_mut::<CursorOptions>()
                && !cursor.is_locked()
            {
                cursor.lock();
            }
            if let Some(i) = world.get_resource_mut::<Input>() {
                i.add_mouse_motion(input.mouse);
            }
        }
    }

    /// Keep the logs and events sent since the last call.
    fn collect(&mut self, world: &World) {
        let page = log_capture::read(self.log_next, usize::MAX);
        self.log_next = page.next;
        let room = MAX_KEPT.saturating_sub(self.logs.len());
        self.logs.extend(page.entries.into_iter().take(room));
        let registry = world.registry();
        for (name, (cursor, seen)) in &mut self.events {
            let Some(ty) = registry.event(name) else {
                continue;
            };
            for e in ty.read(world, cursor) {
                if seen.len() < MAX_KEPT {
                    seen.push(
                        registry
                            .value_to_json(e.as_partial_reflect())
                            .unwrap_or(Json::Null),
                    );
                }
            }
        }
    }

    fn evaluate(&self, step: usize, check: &Check, world: &World) -> CheckResult {
        let mut r = CheckResult {
            step,
            frame: self.frames,
            check: check.kind().to_string(),
            passed: false,
            message: String::new(),
            expected: Json::Null,
            actual: Json::Null,
        };
        match check {
            Check::EntityNear {
                entity,
                point,
                radius,
                horizontal,
            } => {
                r.expected = json!({ "point": point, "radius": radius });
                match find_entity(world, entity) {
                    Ok(e) => {
                        let p = position(world, e).unwrap_or(Vec3::ZERO);
                        let target = Vec3::from_array(*point);
                        let d = if *horizontal {
                            Vec2::new(p.x - target.x, p.z - target.z).length()
                        } else {
                            p.distance(target)
                        };
                        r.passed = d <= *radius;
                        r.actual = json!({ "position": round3(p), "distance": round_f(d) });
                        r.message = format!(
                            "`{entity}` is {d:.2} m from ({:.2}, {:.2}, {:.2}){}, at ({:.2}, {:.2}, {:.2}) ({} {radius} m)",
                            target.x,
                            target.y,
                            target.z,
                            if *horizontal { " horizontally" } else { "" },
                            p.x,
                            p.y,
                            p.z,
                            if r.passed { "within" } else { "more than" }
                        );
                    }
                    Err(e) => r.message = e,
                }
            }
            Check::EntityGone { entity } => match find_entity(world, entity) {
                Err(e) if e.starts_with("no entity") => {
                    r.passed = true;
                    r.message = format!("`{entity}` is gone");
                    r.actual = json!("despawned");
                }
                Err(e) => r.message = e,
                Ok(e) if world.get::<Visible>(e).is_some_and(|v| !v.0) => {
                    r.passed = true;
                    r.message = format!("`{entity}` is hidden");
                    r.actual = json!("hidden");
                }
                Ok(e) => {
                    let p = position(world, e).unwrap_or(Vec3::ZERO);
                    r.actual = json!({ "position": round3(p) });
                    r.message = format!(
                        "`{entity}` is still there, at ({:.2}, {:.2}, {:.2})",
                        p.x, p.y, p.z
                    );
                }
            },
            Check::EntityExists { entity } => match find_entity(world, entity) {
                Ok(e) => {
                    r.passed = true;
                    let p = position(world, e).unwrap_or(Vec3::ZERO);
                    r.actual = json!({ "position": round3(p) });
                    r.message = format!("`{entity}` exists");
                }
                Err(e) if e.contains("entities named") => {
                    r.passed = true;
                    r.message = e;
                }
                Err(e) => r.message = e,
            },
            Check::ComponentEquals {
                entity,
                resource,
                path,
                value,
                tolerance,
            } => {
                r.expected = value.clone();
                let tolerance = tolerance.unwrap_or(DEFAULT_TOLERANCE);
                let what = match (entity, resource) {
                    (Some(e), _) => format!("`{e}` {path}"),
                    (None, Some(res)) if path.is_empty() => format!("resource {res}"),
                    (None, Some(res)) => format!("resource {res}.{path}"),
                    (None, None) => path.clone(),
                };
                match reflected_value(world, entity.as_deref(), resource.as_deref(), path) {
                    Ok(actual) => {
                        r.passed = json_matches(value, &actual, tolerance);
                        r.message = if r.passed {
                            format!("{what} is {actual}")
                        } else {
                            format!("{what} is {actual}, expected {value}")
                        };
                        r.actual = actual;
                    }
                    Err(e) => r.message = e,
                }
            }
            Check::EventSeen {
                event,
                fields,
                at_least,
            } => {
                let seen = self
                    .events
                    .get(event)
                    .map_or(&[][..], |(_, s)| s.as_slice());
                let matching = seen
                    .iter()
                    .filter(|v| fields.as_ref().is_none_or(|f| json_matches(f, v, 1e-4)))
                    .count();
                r.passed = matching >= *at_least as usize;
                r.expected = json!({ "fields": fields, "at_least": at_least });
                r.actual = json!({
                    "matching": matching,
                    "sent": seen.len(),
                    "last": seen.last(),
                });
                r.message = format!(
                    "{event}: {matching} matching of {} sent (wanted at least {at_least})",
                    seen.len()
                );
            }
            Check::LogContains { text, level } => {
                let found = self.logs.iter().find(|l| {
                    l.message.contains(text.as_str())
                        && level
                            .as_ref()
                            .is_none_or(|lv| l.level.eq_ignore_ascii_case(lv))
                });
                r.expected = json!({ "text": text, "level": level });
                match found {
                    Some(l) => {
                        r.passed = true;
                        r.actual =
                            json!({ "level": l.level, "target": l.target, "message": l.message });
                        r.message = format!("logged: {}", l.message);
                    }
                    None => {
                        r.message = format!(
                            "no log message contains {text:?} ({} message(s) since the start)",
                            self.logs.len()
                        );
                    }
                }
            }
        }
        r
    }
}

/// Runs a [`PlaytestRun`] resource's frame start once per input frame,
/// before actions are evaluated (added to `FixedUpdate` and `Update` by
/// [`crate::InputPlugin`]).
pub fn playtest_frame_system(world: &mut World) {
    if !world.contains_resource::<PlaytestRun>() {
        return;
    }
    let Some(generation) = world.get_resource::<Input>().map(Input::generation) else {
        return;
    };
    let dt = world
        .get_resource::<Time>()
        .map_or(FRAME, |t| t.unscaled_delta);
    let Some(mut run) = world.remove_resource::<PlaytestRun>() else {
        return;
    };
    if run.generation != Some(generation) {
        run.generation = Some(generation);
        run.frame_start(world, dt);
    }
    world.insert_resource(run);
}

/// A world position: the global transform, else the local one.
fn position(world: &World, e: Entity) -> Option<Vec3> {
    world
        .get::<GlobalTransform>(e)
        .map(|g| g.translation())
        .or_else(|| world.get::<Transform>(e).map(|t| t.translation))
}

fn round_f(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn round3(v: Vec3) -> [f64; 3] {
    [round_f(v.x), round_f(v.y), round_f(v.z)]
}

/// An entity by uid or name (prefab contents and runtime spawns included;
/// a name shared by several prefers the one that is not generated).
fn find_entity(world: &World, reference: &str) -> Result<Entity, String> {
    if let Some(uid) = Uid::parse(reference) {
        return world
            .entity_by_uid(uid)
            .ok_or_else(|| format!("no entity {uid}"));
    }
    let named: Vec<Entity> = world
        .query::<(Entity, &Name)>()
        .iter()
        .filter(|(_, n)| n.as_str() == reference)
        .map(|(e, _)| e)
        .collect();
    let pick = match named.as_slice() {
        [] => return Err(format!("no entity named `{reference}`")),
        [e] => vec![*e],
        many => many
            .iter()
            .copied()
            .filter(|e| !box_core::prefab::is_generated(world, *e))
            .collect(),
    };
    match pick.as_slice() {
        [e] => Ok(*e),
        _ => Err(format!(
            "{} entities named `{reference}`: use a uid",
            named.len()
        )),
    }
}

/// The JSON of a reflected value: `Component.path` of an entity, or `path`
/// in a resource.
fn reflected_value(
    world: &World,
    entity: Option<&str>,
    resource: Option<&str>,
    path: &str,
) -> Result<Json, String> {
    let registry = world.registry();
    let (root, rest) = match (entity, resource) {
        (Some(reference), _) => {
            let e = find_entity(world, reference)?;
            let (component, rest) = path.split_once('.').unwrap_or((path, ""));
            if component.is_empty() {
                return Err("`path` starts with the component: `Health.current`".into());
            }
            let value = world
                .get_reflect(e, component)
                .map_err(|err| {
                    format!(
                        "`{reference}` {component}: {err} (it has {})",
                        world.component_names(e).join(", ")
                    )
                })
                .and_then(|r| {
                    registry
                        .value_to_json((**r).as_partial_reflect())
                        .map_err(|err| err.to_string())
                })?;
            (value, rest)
        }
        (None, Some(name)) => {
            let value = world
                .get_resource_reflect(name)
                .and_then(|r| registry.value_to_json(r.as_partial_reflect()))
                .map_err(|err| format!("resource {name}: {err}"))?;
            (value, path)
        }
        (None, None) => return Err("component_equals takes `entity` or `resource`".into()),
    };
    let mut at = &root;
    for segment in rest.split(['.', '[', ']']).filter(|s| !s.is_empty()) {
        let next = match at {
            Json::Object(m) => m.get(segment),
            Json::Array(a) => {
                let index = match segment {
                    "x" => Some(0),
                    "y" => Some(1),
                    "z" => Some(2),
                    "w" => Some(3),
                    s => s.parse::<usize>().ok(),
                };
                index.and_then(|i| a.get(i))
            }
            _ => None,
        };
        at = next.ok_or_else(|| {
            let found = match at {
                Json::Object(m) => format!(
                    "fields: {}",
                    m.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
                other => format!("value {other}"),
            };
            format!("no `{segment}` in `{path}` ({found})")
        })?;
    }
    Ok(at.clone())
}

/// `actual` matches `expected`: numbers within `tolerance`, arrays element
/// by element, objects on the fields `expected` names, a one-key object
/// `{"Variant": ...}` against a plain `"Variant"`.
fn json_matches(expected: &Json, actual: &Json, tolerance: f64) -> bool {
    match (expected, actual) {
        (Json::Number(a), Json::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() <= tolerance,
            _ => a == b,
        },
        (Json::Array(a), Json::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_matches(x, y, tolerance))
        }
        (Json::Object(a), Json::Object(b)) => a
            .iter()
            .all(|(k, v)| b.get(k).is_some_and(|w| json_matches(v, w, tolerance))),
        (Json::String(s), Json::Object(b)) => b.len() == 1 && b.contains_key(s),
        (a, b) => a == b,
    }
}

// ---------------------------------------------------------- record ----

/// One evaluated input frame.
#[derive(Clone, Debug, Default, PartialEq)]
struct RecordedFrame {
    dt: f32,
    actions: BTreeMap<String, ActionValue>,
    /// Buttons that became held this frame.
    pressed: BTreeSet<String>,
    mouse: Vec2,
}

/// Resource: records the action values a playing world evaluates, once per
/// input frame ([`playtest_record_system`]), for
/// [`finish`](Self::finish) to turn into a [`Playtest`].
pub struct PlaytestRecorder {
    frames: Vec<RecordedFrame>,
    scene: Option<String>,
    seed: Option<u32>,
    teleport: Option<Teleport>,
    player: Option<String>,
    /// Named, saved entities when recording started.
    present: Vec<(Uid, String)>,
    generation: Option<u64>,
}

impl PlaytestRecorder {
    /// Start recording `world`. `player` names the player (default: the
    /// first named entity with a `KinematicCharacter`). `mid_run`: the game
    /// was not just restarted, so the playtest starts with a teleport of
    /// the player to where it is now (other state is not restored). `seed`:
    /// the scripts' seed the game was restarted with.
    pub fn new(
        world: &World,
        player: Option<&str>,
        mid_run: bool,
        seed: Option<u32>,
    ) -> Result<Self, PlaytestError> {
        let player = match player {
            Some(p) => {
                find_entity(world, p).map_err(|e| PlaytestError::new("entity_not_found", e))?;
                Some(p.to_string())
            }
            None => default_player(world),
        };
        let teleport = match (&player, mid_run) {
            (Some(p), true) => find_entity(world, p).ok().and_then(|e| {
                let t = world.get::<Transform>(e)?;
                let forward = t.rotation * Vec3::NEG_Z;
                let yaw = (-forward.x).atan2(-forward.z).to_degrees();
                let at = position(world, e)?;
                Some(Teleport {
                    entity: p.clone(),
                    position: round3(at).map(|v| v as f32),
                    yaw: Some((yaw * 10.0).round() / 10.0),
                })
            }),
            _ => None,
        };
        let present = world
            .query::<box_core::prefab::NotGenerated<(&Uid, &Name)>>()
            .iter()
            .map(|(u, n)| (*u, n.to_string()))
            .collect();
        Ok(Self {
            frames: Vec::new(),
            scene: scene_relative(world),
            seed,
            teleport,
            player,
            present,
            generation: None,
        })
    }

    /// Input frames recorded so far.
    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    /// The recording as a playtest: frames resampled to 1/60 s, equal
    /// frames merged into steps, then checks for what changed in `world`.
    pub fn finish(&self, world: &World) -> Playtest {
        let mut steps: Vec<Step> = Vec::new();
        for frame in resample(&self.frames) {
            let idle = frame.0.is_empty() && frame.1 == Vec2::ZERO;
            let mouse = (frame.1 != Vec2::ZERO).then_some([frame.1.x, frame.1.y]);
            match steps.last_mut() {
                Some(Step::Wait(n)) if idle => *n += 1,
                Some(Step::Input(s)) if !idle && s.input == frame.0 && s.mouse == mouse => {
                    s.frames += 1
                }
                _ if idle => steps.push(Step::Wait(1)),
                _ => steps.push(Step::Input(InputStep {
                    input: frame.0,
                    mouse,
                    frames: 1,
                })),
            }
        }
        let mut gone = 0;
        for (uid, name) in &self.present {
            if gone >= MAX_GONE_CHECKS {
                break;
            }
            let missing = match world.entity_by_uid(*uid) {
                None => true,
                Some(e) => world.get::<Visible>(e).is_some_and(|v| !v.0),
            };
            // A name shared by others would not find this entity.
            let unique = self.present.iter().filter(|(_, n)| n == name).count() == 1;
            if missing && unique {
                steps.push(Step::Check(Check::EntityGone {
                    entity: name.clone(),
                }));
                gone += 1;
            }
        }
        if let Some(p) = &self.player
            && let Some(at) = find_entity(world, p).ok().and_then(|e| position(world, e))
        {
            steps.push(Step::Check(Check::EntityNear {
                entity: p.clone(),
                point: round3(at).map(|v| v as f32),
                radius: DEFAULT_RADIUS,
                horizontal: false,
            }));
        }
        Playtest {
            version: VERSION,
            description: None,
            scene: self.scene.clone(),
            seed: self.seed,
            teleport: self.teleport.clone(),
            steps,
        }
    }
}

/// Records a [`PlaytestRecorder`] resource's frame after actions were
/// evaluated (added to `Update` by [`crate::InputPlugin`]).
pub fn playtest_record_system(world: &mut World) {
    if !world.contains_resource::<PlaytestRecorder>() {
        return;
    }
    let Some(input) = world.get_resource::<Input>() else {
        return;
    };
    let generation = input.generation();
    let locked = world
        .get_resource::<CursorOptions>()
        .is_some_and(CursorOptions::is_locked);
    let mouse = if locked {
        input.mouse_delta
    } else {
        Vec2::ZERO
    };
    let dt = world
        .get_resource::<Time>()
        .map_or(FRAME, |t| t.unscaled_delta);
    let mut frame = RecordedFrame {
        dt,
        mouse,
        ..Default::default()
    };
    if let (Some(map), Some(actions)) = (
        world.get_resource::<ActionMap>(),
        world.get_resource::<Actions>(),
    ) {
        for name in map.names() {
            let Some(kind) = map.get(name) else {
                continue;
            };
            match kind {
                ActionKind::Button { .. } => {
                    let pressed = actions.pressed(name);
                    if actions.held(name) || pressed {
                        frame.actions.insert(name.into(), ActionValue::Button(true));
                    }
                    if pressed {
                        frame.pressed.insert(name.into());
                    }
                }
                ActionKind::Axis1 { .. } => {
                    let v = actions.axis1(name);
                    if v != 0.0 {
                        frame.actions.insert(name.into(), ActionValue::Axis1(v));
                    }
                }
                ActionKind::Axis2 { .. } => {
                    let v = actions.axis2(name);
                    if v != Vec2::ZERO {
                        frame
                            .actions
                            .insert(name.into(), ActionValue::Axis2([v.x, v.y]));
                    }
                }
            }
        }
    }
    if let Some(rec) = world.get_resource_mut::<PlaytestRecorder>()
        && rec.generation != Some(generation)
    {
        rec.generation = Some(generation);
        rec.frames.push(frame);
    }
}

/// Recorded frames as 1/60 s playtest frames: each playtest frame takes the
/// input of the recorded frame it ends in; a press in a recorded frame no
/// playtest frame ends in is carried to the next, and mouse motion is
/// summed (split evenly over the playtest frames a long frame covers).
fn resample(frames: &[RecordedFrame]) -> Vec<(BTreeMap<String, ActionValue>, Vec2)> {
    let mut out = Vec::new();
    let mut elapsed = 0.0f64;
    let mut done = 0u64;
    let mut carried: BTreeSet<String> = BTreeSet::new();
    let mut carried_mouse = Vec2::ZERO;
    for f in frames {
        elapsed += f64::from(f.dt);
        let target = (elapsed / f64::from(FRAME) + 1e-6).floor() as u64;
        let n = target.saturating_sub(done);
        if n == 0 {
            carried.extend(f.pressed.iter().cloned());
            carried_mouse += f.mouse;
            continue;
        }
        let mouse = (f.mouse + carried_mouse) / n as f32;
        for k in 0..n {
            let mut actions = f.actions.clone();
            if k == 0 {
                for p in &carried {
                    actions.insert(p.clone(), ActionValue::Button(true));
                }
            }
            out.push((actions, mouse));
        }
        done = target;
        carried.clear();
        carried_mouse = Vec2::ZERO;
    }
    out
}

/// The first named entity with a character controller.
fn default_player(world: &World) -> Option<String> {
    #[cfg(feature = "physics")]
    {
        let mut found: Vec<(u64, String)> = world
            .query::<(Entity, &box_physics::KinematicCharacter, &Name)>()
            .iter()
            .map(|(e, _, n)| (e.to_bits().get(), n.to_string()))
            .collect();
        found.sort();
        found.into_iter().next().map(|(_, n)| n)
    }
    #[cfg(not(feature = "physics"))]
    {
        let _ = world;
        None
    }
}

/// The main scene relative to the project root (`assets/scenes/x.scene.json`).
pub fn scene_relative(world: &World) -> Option<String> {
    let scenes = world.get_resource::<Scenes>()?;
    let path = scenes
        .main()
        .and_then(|id| scenes.path(id))
        .or_else(|| scenes.start_scene())?;
    let root = scenes.project_root()?;
    path.strip_prefix(root).ok().map(|rel| {
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    })
}

/// A playtest's name: its file name without `.playtest.json`.
pub fn name_of(path: &Path) -> String {
    let file = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    file.strip_suffix(EXTENSION)
        .or_else(|| file.strip_suffix(".json"))
        .unwrap_or(&file)
        .to_string()
}

/// Where a playtest named `name` (or a path) lives in a project: a path
/// with a `/` or ending in `.json` is relative to the project root, a bare
/// name is `playtests/<name>.playtest.json`.
pub fn path_in_project(project_root: &Path, name: &str) -> PathBuf {
    if name.contains('/') || name.contains('\\') || name.ends_with(".json") {
        project_root.join(name)
    } else {
        project_root.join(DIR).join(format!("{name}{EXTENSION}"))
    }
}

/// Write a playtest file (creating its folder).
pub fn write(path: &Path, playtest: &Playtest) -> Result<(), PlaytestError> {
    let io = |e: std::io::Error| {
        PlaytestError::new(
            "io",
            format!(
                "could not write {}: {}",
                path.display(),
                crate::io_hint::text(&e)
            ),
        )
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut text = playtest.to_json_pretty();
    text.push('\n');
    std::fs::write(path, text).map_err(io)
}

/// Options of a headless replay.
#[derive(Clone, Debug)]
pub struct ReplayOptions {
    /// Save a PNG every this many frames (and the last frame); needs a GPU.
    pub capture_every: Option<u32>,
    /// Folder for the PNGs (default `<project>/.box/playtests/<name>/`).
    pub capture_dir: Option<PathBuf>,
    /// Window size the game sees (and the captures' size).
    pub size: (u32, u32),
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            capture_every: None,
            capture_dir: None,
            size: (1280, 720),
        }
    }
}

/// How long a replay waits for assets before its first frame.
const ASSET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Replay `playtest` headless with `game`'s plugins on the project of
/// `player` (its `project`; the playtest's `scene` replaces its `scene`):
/// load the scene fresh, run every step with the fixed 1/60 s step, and
/// report. Deterministic: the same inputs give the same report.
pub fn replay(
    game: &crate::Game,
    player: &crate::player::PlayerOptions,
    playtest: &Playtest,
    name: &str,
    options: &ReplayOptions,
) -> Result<PlaytestReport, PlaytestError> {
    let player_options = crate::player::PlayerOptions {
        project: player.project.clone(),
        scene: playtest.scene.clone().or_else(|| player.scene.clone()),
        watch: false,
        run: crate::RunOptions {
            headless: true,
            ..crate::RunOptions::new(name)
        },
        ..Default::default()
    };
    let (mut app, project) = crate::player::build_app(game, &player_options)
        .map_err(|e| PlaytestError::new("load", crate::player::error_text(&e)))?;
    #[cfg(feature = "script")]
    if let Some(seed) = playtest.seed {
        app.world.insert_resource(box_script::ScriptSeed(seed));
    }
    if app.world.contains_resource::<box_assets::AssetServer>() {
        crate::asset_glue::settle_assets(&mut app.world, ASSET_TIMEOUT);
    }
    let (width, height) = options.size;
    crate::runner::set_window_size(&mut app.world, width, height, 1.0);
    let mut renderer = match options.capture_every {
        Some(_) => Some(
            box_render::Renderer::new_headless(width, height)
                .map_err(|e| PlaytestError::new("render", e.to_string()))?,
        ),
        None => None,
    };
    let capture_dir = options
        .capture_dir
        .clone()
        .unwrap_or_else(|| project.root().join(".box").join("playtests").join(name));
    let run = PlaytestRun::new(playtest.clone(), name, &mut app.world)?;
    app.world.insert_resource(run);
    let mut captures = Vec::new();
    let mut frame: u64 = 0;
    loop {
        let finished = {
            let mut run = app
                .world
                .remove_resource::<PlaytestRun>()
                .expect("inserted above");
            run.settle(&mut app.world);
            let finished = run.is_finished();
            app.world.insert_resource(run);
            finished
        };
        let capture_now = renderer.is_some()
            && frame > 0
            && (finished
                || options
                    .capture_every
                    .is_some_and(|n| frame.is_multiple_of(u64::from(n.max(1)))));
        if capture_now && let Some(r) = renderer.as_mut() {
            let image = r
                .capture(&mut app.world)
                .map_err(|e| PlaytestError::new("render", e.to_string()))?;
            let out = capture_dir.join(format!("{name}_f{frame:05}.png"));
            std::fs::create_dir_all(&capture_dir)
                .and_then(|()| image.save(&out).map_err(std::io::Error::other))
                .map_err(|e| {
                    PlaytestError::new("io", format!("could not write {}: {e}", out.display()))
                })?;
            captures.push(out.display().to_string());
        }
        if finished || frame >= MAX_FRAMES || app.exit_requested() {
            break;
        }
        app.tick(FRAME);
        app.world.resource_mut::<Input>().end_frame();
        // Assets the frame asked for (a script's file, a data table, a
        // spawned prefab) are there by the next frame however long they
        // take to import, so replays do not depend on the machine's speed.
        if let Some(server) = app.world.get_resource::<box_assets::AssetServer>().cloned()
            && !server.is_idle()
        {
            server.update_until_idle(&mut app.world, ASSET_TIMEOUT);
        }
        frame += 1;
    }
    let run = app
        .world
        .remove_resource::<PlaytestRun>()
        .expect("inserted above");
    let mut report = run.report(&app.world);
    report.captures = captures;
    if let Some(r) = &renderer
        && r.stats().validation_errors > 0
    {
        return Err(PlaytestError::new(
            "render",
            format!(
                "{} GPU validation error(s) during the replay",
                r.stats().validation_errors
            ),
        ));
    }
    Ok(report)
}

/// Replay results as a JSON object (what `box playtest --json` and the
/// MCP tool return), with the playtest's file.
pub fn report_json(report: &PlaytestReport, file: Option<&str>) -> Json {
    let mut v = serde_json::to_value(report).unwrap_or_else(|_| json!({}));
    if let (Some(file), Json::Object(m)) = (file, &mut v) {
        m.insert("file".into(), json!(file));
    }
    v
}
