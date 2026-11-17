//! Structured CLI errors: `{ "error": { "code", "message", "file"?, "entity"?, ... } }`.

use std::fmt;
use std::path::Path;

use box_engine::ProjectError;
use box_engine::box_core::edit::{CommandError, CommandParseError, EditError};
use box_engine::box_core::migrate::MigrateError;
use box_engine::box_core::reflect::ReflectError;
use box_engine::box_core::scene::SceneError;
use box_engine::box_core::world::World;
use box_engine::control::edit_error_code;
use serde_json::{Map, Value as Json, json};

/// A failed command (boxed, so `Result<_, CliError>` stays small). Fields are
/// reached through `Deref` to [`ErrorBody`].
#[derive(Debug)]
pub struct CliError(Box<ErrorBody>);

/// The contents of a [`CliError`]. `code` is stable and machine-matchable;
/// `message` is for people.
#[derive(Debug)]
pub struct ErrorBody {
    pub code: &'static str,
    pub message: String,
    pub file: Option<String>,
    pub entity: Option<String>,
    /// Additional machine-readable fields (command index, op, path, ...).
    pub extra: Map<String, Json>,
}

impl std::ops::Deref for CliError {
    type Target = ErrorBody;
    fn deref(&self) -> &ErrorBody {
        &self.0
    }
}

impl std::ops::DerefMut for CliError {
    fn deref_mut(&mut self) -> &mut ErrorBody {
        &mut self.0
    }
}

impl CliError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self(Box::new(ErrorBody {
            code,
            message: message.into(),
            file: None,
            entity: None,
            extra: Map::new(),
        }))
    }

    pub fn file(mut self, file: impl AsRef<Path>) -> Self {
        self.file = Some(file.as_ref().display().to_string());
        self
    }

    pub fn entity(mut self, entity: impl Into<String>) -> Self {
        self.entity = Some(entity.into());
        self
    }

    pub fn with(mut self, key: &str, value: Json) -> Self {
        self.extra.insert(key.to_string(), value);
        self
    }

    /// Add `entity_name` (shown with the uid) when the error's entity is a
    /// uid of a named entity in `world`.
    pub fn name_entity(self, world: &World) -> Self {
        let name = self
            .entity
            .as_deref()
            .and_then(|uid| crate::problems::entity_name(world, uid));
        match name {
            Some(name) => self.with("entity_name", json!(name)),
            None => self,
        }
    }

    /// The process exit code to use instead of 1 (a child process's code).
    pub fn exit_with(self, code: u8) -> Self {
        self.with("exit_code", json!(code))
    }

    pub fn io(path: &Path, e: std::io::Error) -> Self {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            "not_found"
        } else {
            "io"
        };
        let err = Self::new(code, format!("{}: {e}", path.display())).file(path);
        match box_engine::io_hint::hint(&e) {
            Some(h) => err.with("hint", json!(h)),
            None => err,
        }
    }

    /// The process exit code for this error: 2 for a usage error, wherever it
    /// was detected, a child process's own code when it failed
    /// ([`exit_with`](Self::exit_with)), otherwise 1.
    pub fn exit_code(&self) -> u8 {
        if self.code == "usage" {
            return 2;
        }
        self.extra
            .get("exit_code")
            .and_then(Json::as_u64)
            .and_then(|c| u8::try_from(c).ok())
            .filter(|c| *c != 0)
            .unwrap_or(1)
    }

    pub fn to_json(&self) -> Json {
        let mut body = Map::new();
        body.insert("code".into(), json!(self.code));
        body.insert("message".into(), json!(self.message));
        if let Some(f) = &self.file {
            body.insert("file".into(), json!(f));
        }
        if let Some(e) = &self.entity {
            body.insert("entity".into(), json!(e));
        }
        for (k, v) in &self.extra {
            body.insert(k.clone(), v.clone());
        }
        json!({ "error": body })
    }

    /// A scene that failed to load or save.
    pub fn scene(path: &Path, e: SceneError) -> Self {
        let (code, entity) = match &e {
            SceneError::Json(_) => ("scene_parse", None),
            SceneError::NewerVersion { .. } | SceneError::UnsupportedVersion { .. } => {
                ("scene_version", None)
            }
            SceneError::Component { uid, .. } => ("scene_invalid", Some(uid.to_string())),
            SceneError::ComponentMigration { uid, source, .. } => {
                let code = if matches!(**source, MigrateError::Newer { .. }) {
                    "scene_version"
                } else {
                    "scene_migration"
                };
                (code, Some(uid.to_string()))
            }
            SceneError::Migrate(m) if matches!(**m, MigrateError::Newer { .. }) => {
                ("scene_version", None)
            }
            SceneError::Migrate(_) => ("scene_migration", None),
            SceneError::Reflect(ReflectError::DuplicateUid(uid)) => {
                ("scene_invalid", Some(uid.to_string()))
            }
            _ => ("scene_invalid", None),
        };
        let mut err = Self::new(code, format!("{}: {e}", path.display())).file(path);
        if let SceneError::Component { component, .. }
        | SceneError::ComponentMigration { component, .. } = &e
        {
            err = err.with("component", json!(component));
        }
        if matches!(e.migration(), Some(MigrateError::NeedsGameCode { .. })) {
            err = err.with(
                "hint",
                json!("upgrade the project with the game's own player: `cargo run --bin player -- --upgrade` in the project folder"),
            );
        }
        // The scene did not load: find the entity's name in the file itself.
        if let Some(name) = entity.as_deref().and_then(|uid| name_in_file(path, uid)) {
            err = err.with("entity_name", json!(name));
        }
        err.entity = entity;
        err
    }

    pub fn commands_parse(path: &str, e: CommandParseError) -> Self {
        let mut err = Self::new("command_parse", format!("{path}: {e}")).file(path);
        if let CommandParseError::Command { index, .. } = e {
            err = err.with("command", json!(index));
        }
        err
    }

    pub fn command(scene: &Path, e: CommandError) -> Self {
        let code = edit_error_code(&e.source);
        let mut err = Self::new(code, e.to_string())
            .file(scene)
            .with("command", json!(e.index))
            .with("op", json!(e.op));
        if let Some(p) = &e.path {
            err = err.with("path", json!(p));
        }
        err.entity = e.entity;
        err
    }

    pub fn edit(e: EditError) -> Self {
        match e {
            EditError::NoEntity(uid) => {
                Self::new("entity_not_found", format!("no entity with uid {uid}"))
                    .entity(uid.to_string())
            }
            EditError::UnknownEntity(name) => {
                Self::new("entity_not_found", format!("no entity named `{name}`")).entity(name)
            }
            EditError::AmbiguousName { name, count } => Self::new(
                "ambiguous_entity",
                format!("{count} entities are named `{name}`; use a uid"),
            )
            .entity(name),
            other => Self::new("edit_failed", other.to_string()),
        }
    }
}

impl From<ProjectError> for CliError {
    fn from(e: ProjectError) -> Self {
        let code = match &e {
            ProjectError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
                "not_found"
            }
            ProjectError::Io { .. } => "io",
            ProjectError::NotFound(_) => "not_a_project",
            ProjectError::AlreadyExists(_) => "already_exists",
            ProjectError::Parse { .. } => "project_invalid",
            ProjectError::NewerFormat { .. } | ProjectError::NewerEngine { .. } => {
                "project_too_new"
            }
            ProjectError::OutsideProject { .. } => "path_outside_project",
            ProjectError::EmptyName => "usage",
            ProjectError::Scene { .. } => "scene_invalid",
        };
        let mut err = Self::new(code, e.to_string());
        err.file = e.path().map(|p| p.display().to_string());
        err
    }
}

/// The `"name"` of the entity with `uid` in a scene file's JSON.
fn name_in_file(path: &Path, uid: &str) -> Option<String> {
    let tree: Json = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    tree["entities"]
        .as_array()?
        .iter()
        .find(|e| e["uid"] == uid)?["name"]
        .as_str()
        .map(str::to_string)
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}]: {}", self.code, self.message)?;
        if let Some(e) = &self.entity {
            match self.extra.get("entity_name").and_then(Json::as_str) {
                Some(name) => write!(f, " (entity `{name}`, {e})")?,
                None => write!(f, " (entity {e})")?,
            }
        }
        if let Some(hint) = self.extra.get("hint").and_then(Json::as_str) {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for CliError {}
