//! The `problems` list of command results: what went wrong while a command
//! still succeeded (an asset that failed to import, a reference to a file
//! that does not exist, a warning an importer logged). Every entry is
//! `{severity, code, message, asset?, entity?, entity_name?, ...}` with
//! severity `error` or `warning` and a stable code:
//!
//! - `asset_not_found`: a reference names no file in `assets/` (or no
//!   sub-asset of it);
//! - `asset_import_failed`: a referenced or checked file failed to import;
//! - `shader_header`: a `.wgsl` file's `@material` header is invalid;
//! - `shader_invalid`: a material shader does not compile (`box check`
//!   validates it without a GPU; the message has the file and line);
//! - `unknown_component`, `unknown_resource`: scene data no registered type
//!   reads (`box check`);
//! - `value_clamped`: `scene apply` set a number outside its range (clamped);
//! - `field_defaulted`: `scene apply` switched to an enum variant without
//!   giving a field it needs; the field type's default (often 0) was used;
//! - `log`: a warning or error the engine logged during the command
//!   (`target` names its source).
//!
//! While a [`LogCollector`] runs, the terminal leaves out the `box*`
//! warnings and errors it collects ([`collecting`]), so each problem is
//! reported once: in the result.

use std::sync::atomic::{AtomicUsize, Ordering};

use box_engine::asset_glue::held_asset_refs;
use box_engine::box_assets::{AssetServer, LoadState};
use box_engine::box_core::prelude::Name;
use box_engine::box_core::world::World;
use box_engine::log_capture;
use serde_json::{Value as Json, json};

static COLLECTING: AtomicUsize = AtomicUsize::new(0);

/// True while a command collects log records into its result.
pub fn collecting() -> bool {
    COLLECTING.load(Ordering::Relaxed) > 0
}

/// Collects the warnings and errors that `box*` targets log while it lives.
pub(crate) struct LogCollector {
    since: u64,
}

impl LogCollector {
    pub(crate) fn start() -> Self {
        log_capture::enable();
        COLLECTING.fetch_add(1, Ordering::Relaxed);
        Self {
            since: log_capture::next_seq(),
        }
    }

    /// The collected records as `log` problems, without repeats.
    pub(crate) fn finish(self) -> Vec<Json> {
        let mut out: Vec<Json> = Vec::new();
        let mut since = self.since;
        loop {
            let page = log_capture::read(since, 1000);
            if page.entries.is_empty() {
                break;
            }
            since = page.next;
            for e in page.entries {
                let severity = match e.level.as_str() {
                    "error" => "error",
                    "warn" => "warning",
                    _ => continue,
                };
                if !e.target.starts_with("box") {
                    continue;
                }
                if out.iter().any(|p| p["message"] == e.message.as_str()) {
                    continue;
                }
                out.push(json!({
                    "severity": severity,
                    "code": "log",
                    "message": e.message,
                    "target": e.target,
                }));
            }
        }
        out
    }
}

impl Drop for LogCollector {
    fn drop(&mut self) {
        COLLECTING.fetch_sub(1, Ordering::Relaxed);
    }
}

/// `asset_problems` followed by the `logged` problems that do not repeat one
/// of them (a failed import is also logged, naming the same file and error).
pub(crate) fn merge(mut asset_problems: Vec<Json>, logged: Vec<Json>) -> Vec<Json> {
    let repeats = |log: &Json| {
        let text = log["message"].as_str().unwrap_or_default();
        asset_problems.iter().any(|p| {
            let asset = p["asset"].as_str().unwrap_or("\u{0}");
            let message = p["message"].as_str().unwrap_or("\u{0}");
            let not_found = p["code"] == "asset_not_found"
                && (text.contains("not found") || text.contains("no such file"));
            text.contains(asset) && (not_found || text.contains(message) || message.contains(text))
        })
    };
    let kept: Vec<Json> = logged.into_iter().filter(|l| !repeats(l)).collect();
    asset_problems.extend(kept);
    asset_problems
}

/// `Cube (01a1…)`, or the uid alone for an unnamed entity.
pub(crate) fn entity_label(world: &World, uid: &str) -> String {
    match entity_name(world, uid) {
        Some(name) => format!("`{name}` ({uid})"),
        None => uid.to_string(),
    }
}

/// The name of the entity with this uid.
pub(crate) fn entity_name(world: &World, uid: &str) -> Option<String> {
    let uid = uid.parse().ok()?;
    let e = world.entity_by_uid(uid)?;
    world.get::<Name>(e).map(|n| n.to_string())
}

/// Problems of the asset references a loaded world holds: references that
/// name no file (`asset_not_found`) and, with `loaded` (the world's assets
/// were requested and settled), references whose asset failed
/// (`asset_import_failed`). `severity` is the not-found severity.
pub(crate) fn asset_ref_problems(
    world: &World,
    server: &AssetServer,
    loaded: bool,
    severity: &str,
) -> Vec<Json> {
    let mut out: Vec<Json> = Vec::new();
    for held in held_asset_refs(world) {
        let reference = held.asset.to_string();
        let mut problem = match server.canonical_ref(&held.asset) {
            None => json!({
                "severity": severity,
                "code": "asset_not_found",
                "asset": reference,
                "message": format!(
                    "{} references `{reference}`, which is not in assets/",
                    held.owner
                ),
            }),
            Some(canonical) => {
                let missing_label = canonical.label.as_ref().filter(|label| {
                    let id = canonical.id.expect("canonical refs have ids");
                    server.file_state(id) == Some(LoadState::Loaded)
                        && !server
                            .sub_assets(id)
                            .iter()
                            .any(|s| s.label.as_ref() == Some(*label))
                });
                if let Some(label) = missing_label {
                    json!({
                        "severity": severity,
                        "code": "asset_not_found",
                        "asset": reference,
                        "message": format!(
                            "{} references `{reference}`: {} has no sub-asset `{label}`",
                            held.owner,
                            canonical.path.as_deref().unwrap_or("the file")
                        ),
                    })
                } else {
                    let failed = loaded
                        .then(|| server.state(&canonical))
                        .flatten()
                        .or_else(|| loaded.then(|| server.file_state(canonical.id?)).flatten());
                    match failed {
                        Some(LoadState::Failed(message)) => json!({
                            "severity": "error",
                            "code": failure_code(&message),
                            "asset": canonical.path.clone().unwrap_or(reference),
                            "message": message,
                        }),
                        _ => continue,
                    }
                }
            }
        };
        if let Some(uid) = held.entity {
            let uid = uid.to_string();
            problem["entity"] = json!(uid);
            if let Some(name) = entity_name(world, &uid) {
                problem["entity_name"] = json!(name);
            }
        }
        problem["component"] = json!(held.owner);
        let duplicate = out.iter().any(|p| {
            p["code"] == problem["code"]
                && p["asset"] == problem["asset"]
                && p["entity"] == problem["entity"]
        });
        if !duplicate {
            out.push(problem);
        }
    }
    out
}

/// The code of a failed import: `shader_header` for a bad `@material`
/// header, else `asset_import_failed`.
pub(crate) fn failure_code(message: &str) -> &'static str {
    if message.contains("bad @material header") {
        "shader_header"
    } else {
        "asset_import_failed"
    }
}

/// One text line per problem, then nothing; empty without problems.
pub(crate) fn text(problems: &[Json]) -> String {
    let mut out = String::new();
    for p in problems {
        let mut line = format!(
            "{}[{}]: ",
            p["severity"].as_str().unwrap_or("?"),
            p["code"].as_str().unwrap_or("?")
        );
        if let Some(name) = p["entity_name"].as_str() {
            line.push_str(&format!("entity `{name}`: "));
        } else if let Some(uid) = p["entity"].as_str() {
            line.push_str(&format!("entity {uid}: "));
        }
        let message = p["message"].as_str().unwrap_or("?");
        if let Some(asset) = p["asset"].as_str().filter(|a| !message.contains(a)) {
            line.push_str(&format!("{asset}: "));
        }
        line.push_str(message);
        out.push('\n');
        out.push_str(&line);
    }
    out
}

/// `"; 1 error, 2 warnings"`-style counts for a summary line.
pub(crate) fn counts(problems: &[Json]) -> (usize, usize) {
    let errors = problems.iter().filter(|p| p["severity"] == "error").count();
    (errors, problems.len() - errors)
}

/// A summary line plus [`text`], or nothing without problems.
pub(crate) fn summary(problems: &[Json]) -> String {
    if problems.is_empty() {
        return String::new();
    }
    let (errors, warnings) = counts(problems);
    format!(
        "\n{} problem(s): {errors} error(s), {warnings} warning(s){}",
        problems.len(),
        text(problems)
    )
}
