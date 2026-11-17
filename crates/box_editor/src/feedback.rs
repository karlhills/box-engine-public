//! Clear feedback: nothing fails silently and the editor's state is always
//! visible.
//!
//! - **Toasts** over the bottom right of the viewport: warnings and errors
//!   logged by box-engine code (`box*` targets, so not wgpu or other
//!   dependency noise), read from the console's
//!   [`LogBuffer`](crate::log::LogBuffer) once per
//!   frame, plus short notes for explicit actions (Saved, Undo, Redo).
//!   Repeats coalesce into one toast with a count; at most
//!   [`MAX_VISIBLE`] are shown and the rest are counted ("N more in the
//!   Console"). Info toasts last [`INFO_S`], warnings [`WARN_S`], errors
//!   [`ERROR_S`]; hovering holds them. A click opens the Console at the
//!   record. Warnings and errors have a **Copy** button (the whole message)
//!   and, when the message names a file that exists (absolute, or relative
//!   to the project or its `assets/`), **Show File**, which reveals it in
//!   the file manager.
//! - **Console badge**: warnings and errors logged since the Console was
//!   last shown, drawn on its tab.
//! - **Play banner** in the inspector, **unknown components** (types this
//!   editor does not register, kept as raw data) shown read-only, and
//!   **asset error marks** for entities whose mesh or prefab asset failed or
//!   is missing.
//!
//! Per-frame cost: one log lock and an integer compare when nothing was
//! logged; the asset marks are rebuilt only when the world or the asset
//! server's state changed.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use box_core::asset_ref::AssetRef;
use box_core::identity::Uid;
use box_core::mesh_asset::MeshAsset;
use box_core::prefab::PrefabInstance;
use box_core::prelude::Entity;
use box_core::scene::UnknownComponents;
use egui::text::LayoutJob;
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};

use crate::assets::RefStatus;
use crate::log::Severity;
use crate::play::PlayState;
use crate::state::EditorState;
use crate::theme::Tokens;

/// Toasts shown at once; older warnings and infos make room first.
pub const MAX_VISIBLE: usize = 4;
/// New log records turned into toasts per frame; a burst beyond this is
/// only counted.
pub const MAX_PER_FRAME: usize = 32;
/// Seconds an info toast stays.
pub const INFO_S: f64 = 2.5;
/// Seconds a warning toast stays.
pub const WARN_S: f64 = 4.0;
/// Seconds an error toast stays (unless clicked or closed first).
pub const ERROR_S: f64 = 8.0;
/// Characters of a message shown in a toast (the Console has all of it).
const TEXT_MAX: usize = 320;
/// Height a toast with three lines of text and its actions takes, with
/// spacing: how many fit in the viewport.
const TOAST_SLOT_H: f32 = 96.0;
/// Toast width in points (less in a narrow viewport).
const TOAST_W: f32 = 340.0;

/// What [`announce_unknown_components`](EditorState::announce_unknown_components)
/// tells the user to do.
pub const UNKNOWN_HINT: &str = "This component belongs to the game's code. Open the project with the game's own editor: in the game's folder run `cargo editor` (projects made with `box new --native`), or `cargo run --features editor --bin <the game's editor binary> -- .`";

/// One toast.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    id: u64,
    pub severity: Severity,
    /// The first line of the message, shortened.
    pub text: String,
    /// How many times it was reported (coalesced repeats).
    pub count: u32,
    /// The console record it came from, if any (a click selects it).
    pub seq: Option<u64>,
    /// The whole message (Copy copies it).
    pub full: String,
    /// A file the message names that exists (Show File reveals it).
    pub file: Option<PathBuf>,
    /// UI time it disappears; set when it is first shown.
    expires: Option<f64>,
}

impl Toast {
    fn lifetime(&self) -> f64 {
        match self.severity {
            Severity::Info => INFO_S,
            Severity::Warn => WARN_S,
            Severity::Error => ERROR_S,
        }
    }
}

/// The toast stack and the log reader that feeds it.
#[derive(Debug, Default)]
pub struct Toasts {
    /// Oldest first.
    items: Vec<Toast>,
    next_id: u64,
    /// The next log record to look at.
    cursor: u64,
    /// Log generation the cursor is current for.
    generation: Option<u64>,
    /// Warnings and errors not shown as toasts (bursts, pushed out).
    overflow: usize,
    /// Unknown component types already announced by the editor: log records
    /// about them (from the scene loader) are not toasted again.
    quiet: BTreeSet<String>,
}

impl Toasts {
    /// The toasts, oldest first.
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// Warnings and errors that did not get a toast of their own.
    pub fn overflow(&self) -> usize {
        self.overflow
    }

    /// Adds a toast, or counts a repeat of one that is showing (it moves to
    /// the front and its time starts again).
    pub fn push(&mut self, severity: Severity, text: &str, seq: Option<u64>) {
        self.push_with_file(severity, text, seq, None);
    }

    /// [`push`](Self::push) for a message that names `file`.
    pub fn push_with_file(
        &mut self,
        severity: Severity,
        message: &str,
        seq: Option<u64>,
        file: Option<PathBuf>,
    ) {
        let full = message.trim().to_owned();
        let text = one_line(message);
        if let Some(i) = self
            .items
            .iter()
            .position(|t| t.severity == severity && t.text == text)
        {
            let mut t = self.items.remove(i);
            t.count += 1;
            t.seq = seq.or(t.seq);
            t.file = file.or(t.file);
            t.full = full;
            t.expires = None;
            self.items.push(t);
            return;
        }
        self.next_id += 1;
        self.items.push(Toast {
            id: self.next_id,
            severity,
            text,
            count: 1,
            seq,
            full,
            file,
            expires: None,
        });
        while self.items.len() > MAX_VISIBLE {
            let i = self
                .items
                .iter()
                .position(|t| t.severity != Severity::Error)
                .unwrap_or(0);
            let t = self.items.remove(i);
            if t.severity != Severity::Info {
                self.overflow += t.count as usize;
            }
        }
    }

    /// The Console was shown: the overflow count is seen.
    pub(crate) fn clear_overflow(&mut self) {
        self.overflow = 0;
    }

    /// Removes the toast with `id`.
    fn dismiss(&mut self, id: u64) {
        self.items.retain(|t| t.id != id);
        if self.items.is_empty() {
            self.overflow = 0;
        }
    }

    /// Starts the clock of new toasts and drops expired ones. Returns the
    /// seconds until the next one expires.
    fn tick(&mut self, now: f64) -> Option<f64> {
        for t in &mut self.items {
            if t.expires.is_none() {
                t.expires = Some(now + t.lifetime());
            }
        }
        self.items.retain(|t| t.expires.is_some_and(|e| e > now));
        if self.items.is_empty() {
            self.overflow = 0;
        }
        self.items
            .iter()
            .filter_map(|t| t.expires)
            .map(|e| e - now)
            .reduce(f64::min)
    }

    /// Keeps a hovered toast until at least `until`.
    fn hold(&mut self, id: u64, until: f64) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            t.expires = Some(t.expires.map_or(until, |e| e.max(until)));
        }
    }

    fn is_quiet(&self, message: &str) -> bool {
        if self.quiet.is_empty() {
            return false;
        }
        let lower = message.to_lowercase();
        (lower.contains("unknown") || lower.contains("not registered"))
            && self.quiet.iter().any(|n| message.contains(n.as_str()))
    }
}

/// `text` split into what happened and what to do: the part after the
/// first "; " when there is one (the engine's message style).
fn split_fix(text: &str) -> (&str, Option<&str>) {
    match text.split_once("; ") {
        Some((what, fix)) if !what.is_empty() && !fix.trim().is_empty() => (what, Some(fix)),
        _ => (text, None),
    }
}

/// The first line of `text`, at most [`TEXT_MAX`] characters.
fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(TEXT_MAX) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_owned(),
    }
}

/// The first file `message` names that exists: an absolute path, or one
/// relative to one of `roots` (the project folder, its `assets/`). Tokens
/// lose quotes, brackets and trailing punctuation, a `:line[:column]` suffix
/// and an asset `#Label`. Only tokens with a file extension are looked up.
pub fn mentioned_file(message: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let strip = |c: char| "`'\"()[]{},;<>".contains(c);
    for raw in message.split_whitespace().take(64) {
        let mut token = raw.trim_matches(strip).trim_end_matches(['.', ':']);
        // `file.rs:12:5`, `scripts/a.luau:3`
        while let Some((head, tail)) = token.rsplit_once(':') {
            if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
                token = head;
            } else {
                break;
            }
        }
        let token = token.split('#').next().unwrap_or_default();
        let path = Path::new(token);
        if path.extension().is_none() || token.contains("://") {
            continue;
        }
        let found = if path.is_absolute() {
            path.is_file().then(|| path.to_path_buf())
        } else {
            roots.iter().map(|r| r.join(path)).find(|p| p.is_file())
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Shows `path` selected in the platform's file manager.
pub(crate) fn reveal_in_os(path: &Path) {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg("-R").arg(path);
        c
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    if let Err(e) = cmd.spawn() {
        tracing::warn!("could not show {}: {e}", path.display());
    }
}

/// Log targets whose warnings and errors become toasts: the engine's own
/// crates (`box_editor`, `box_assets`, ...), not dependencies.
pub fn toasts_for_target(target: &str) -> bool {
    target == "box" || target.starts_with("box_")
}

/// Entities whose mesh or prefab asset failed to load or is missing, by
/// uid, with the reason. Rebuilt when the world or an asset changed.
#[derive(Debug, Default)]
pub(crate) struct AssetMarks {
    errors: HashMap<Uid, String>,
    /// (world generation, asset events, asset index generation, playing,
    /// entity count) at the last rebuild.
    built: Option<(u64, u64, u64, bool, u32)>,
    /// Bumped for every asset event (`after_asset_update`).
    pub events: u64,
}

impl EditorState {
    /// The toast stack.
    pub fn toasts(&self) -> &Toasts {
        &self.toasts
    }

    /// A short note for an explicit action (Saved, Undo, Redo).
    pub fn notify(&mut self, text: &str) {
        self.toasts.push(Severity::Info, text, None);
    }

    /// Reads the records logged since the last call: warnings and errors of
    /// box-engine targets become toasts, and every warning and error counts
    /// towards the Console badge. Called once per frame; costs one lock and
    /// a compare when nothing was logged.
    pub fn poll_log(&mut self) {
        let log = self.console.log.clone();
        let inner = log.lock();
        if self.toasts.generation == Some(inner.generation()) {
            return;
        }
        self.toasts.generation = Some(inner.generation());
        let mut new = Vec::new();
        let mut burst = 0;
        for r in inner.since(self.toasts.cursor) {
            let severity = Severity::of(r.level);
            if severity == Severity::Info {
                continue;
            }
            self.console.unseen[severity as usize - 1] += 1;
            if !toasts_for_target(&r.target) {
                continue;
            }
            if new.len() < MAX_PER_FRAME {
                new.push((severity, r.message.clone(), r.seq));
            } else {
                burst += 1;
            }
        }
        self.toasts.cursor = inner.next_seq();
        // Nothing above may log: the tracing layer pushes into this lock.
        drop(inner);
        let roots: Vec<PathBuf> = self
            .project
            .as_ref()
            .map(|p| vec![p.root().to_path_buf(), p.assets_dir()])
            .unwrap_or_default();
        for (severity, message, seq) in new {
            if !self.toasts.is_quiet(&one_line(&message)) {
                let file = mentioned_file(&message, &roots);
                self.toasts
                    .push_with_file(severity, &message, Some(seq), file);
            }
        }
        self.toasts.overflow += burst;
    }

    /// After a scene is opened: logs the component types it has that this
    /// editor does not register (kept as raw data and saved back), and
    /// toasts the ones not announced before in this session.
    pub(crate) fn announce_unknown_components(&mut self) {
        let mut names = BTreeSet::new();
        for u in self.world.query::<&UnknownComponents>().iter() {
            names.extend(u.0.keys().cloned());
        }
        if names.is_empty() {
            return;
        }
        let all: Vec<&str> = names.iter().map(String::as_str).collect();
        let message = match all.as_slice() {
            [one] => format!(
                "component `{one}` in this scene is not registered in this editor (data kept). {UNKNOWN_HINT}"
            ),
            many => format!(
                "{} components in this scene are not registered in this editor (data kept): {}. {UNKNOWN_HINT}",
                many.len(),
                many.join(", ")
            ),
        };
        let before = self.console.log.next_seq();
        tracing::warn!("{message}");
        let seq = (self.console.log.next_seq() > before).then_some(before);
        let fresh = names.iter().any(|n| !self.toasts.quiet.contains(n));
        // The record itself is quiet now: counted for the badge, not
        // toasted a second time by `poll_log`.
        if fresh {
            // Short enough to read in a toast; the Console has the rest.
            let list = match all.as_slice() {
                [a] => (*a).to_owned(),
                [a, b] => format!("{a} and {b}"),
                [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
                [] => String::new(),
            };
            let short = format!(
                "{list}: not registered in this editor (data kept). Open the project with the game's editor."
            );
            self.toasts.push(Severity::Warn, &short, seq);
        }
        self.toasts.quiet.extend(names);
    }

    /// The reason the entity's mesh or prefab asset draws nothing, if it
    /// failed to load or is missing (see [`Self::refresh_asset_marks`]).
    pub fn asset_error(&self, uid: Uid) -> Option<&str> {
        self.asset_marks.errors.get(&uid).map(String::as_str)
    }

    /// Rebuilds the asset error marks when the world (an edit, undo, load,
    /// Play) or an asset (load, failure, reload) changed since the last
    /// rebuild. Cheap otherwise.
    pub fn refresh_asset_marks(&mut self) {
        let index = self
            .asset_server
            .as_ref()
            .map_or(0, |s| s.index_generation());
        let key = (
            self.generation,
            self.asset_marks.events,
            index,
            self.is_playing(),
            self.world.entity_count(),
        );
        if self.asset_marks.built == Some(key) {
            return;
        }
        self.asset_marks.built = Some(key);
        let mut refs: Vec<(Uid, &'static str, AssetRef)> = Vec::new();
        for (e, m) in self.world.query::<(Entity, &MeshAsset)>().iter() {
            if let Some(u) = self.world.uid(e) {
                refs.push((u, "Mesh", m.mesh.clone()));
            }
        }
        for (e, p) in self.world.query::<(Entity, &PrefabInstance)>().iter() {
            if let Some(u) = self.world.uid(e) {
                refs.push((u, "Prefab scene", p.scene.clone()));
            }
        }
        let mut errors = HashMap::new();
        for (uid, what, r) in refs {
            let error = match self.ref_status(&r) {
                RefStatus::Failed(m) => format!("{what} {r} failed to load: {m}"),
                RefStatus::Missing if self.project.is_none() => {
                    format!("{what} {r} cannot load: no project is open")
                }
                RefStatus::Missing => format!("{what} {r} is missing from the project's assets"),
                RefStatus::None | RefStatus::Loading | RefStatus::Loaded => continue,
            };
            // A prefab error wins over a mesh one on the same entity.
            errors.insert(uid, error);
        }
        self.asset_marks.errors = errors;
    }
}

/// The unregistered components of `e` and their raw data (pretty JSON).
pub fn unknown_components(world: &box_core::prelude::World, e: Entity) -> Vec<(String, String)> {
    world
        .get::<UnknownComponents>(e)
        .map(|u| {
            u.0.iter()
                .map(|(name, json)| {
                    let text = serde_json::to_string_pretty(json).unwrap_or_default();
                    (name.clone(), text)
                })
                .collect()
        })
        .unwrap_or_default()
}

// ----- drawing -----

/// The toast stack at the bottom right of the viewport (of the window when
/// the viewport is hidden). Returns `Some(seq)` when a toast was clicked:
/// the Console should open at that record (or just open for `None`).
pub(crate) fn toasts_ui(
    ctx: &egui::Context,
    state: &mut EditorState,
    t: &Tokens,
) -> Option<Option<u64>> {
    let now = ctx.input(|i| i.time);
    let next = state.toasts.tick(now);
    if state.toasts.items.is_empty() {
        return None;
    }
    if let Some(s) = next {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(s.max(0.0) + 0.01));
    }
    let area = match (state.viewport.shown, state.viewport.rect) {
        (true, Some(r)) => r,
        _ => ctx.content_rect(),
    };
    let anchor = area.right_bottom() + vec2(-12.0, -12.0);
    let width = TOAST_W.min(area.width() - 24.0).max(160.0);
    let mut clicked = None;
    let mut dismissed = None;
    let mut hovered = None;
    let mut copied = false;
    // A short viewport shows only the newest toasts that fit; the others
    // are counted with the overflow.
    let fit = (((area.height() - 24.0) / TOAST_SLOT_H).floor() as usize).max(1);
    let skip = state.toasts.items.len().saturating_sub(fit);
    let overflow = state.toasts.overflow
        + state.toasts.items[..skip]
            .iter()
            .filter(|t| t.severity != Severity::Info)
            .map(|t| t.count as usize)
            .sum::<usize>();
    egui::Area::new(egui::Id::unique("box_editor_toasts"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(anchor)
        .interactable(true)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.set_width(width);
            if overflow > 0 {
                let label = format!("{overflow} more in the Console");
                let r = ui.add(
                    egui::Button::new(egui::RichText::new(label).font(t.small()).color(t.text))
                        .fill(t.field)
                        .stroke(Stroke::new(1.0, t.border)),
                );
                if r.clicked() {
                    clicked = Some(None);
                }
            }
            for toast in &state.toasts.items[skip..] {
                let (c, h, d) = toast_ui(ui, toast, width, t);
                match c {
                    Some(ToastClick::Body) => {
                        clicked = Some(toast.seq);
                        dismissed = Some(toast.id);
                    }
                    Some(ToastClick::Copy) => {
                        ui.ctx().copy_text(toast.full.clone());
                        copied = true;
                    }
                    Some(ToastClick::ShowFile) => {
                        if let Some(f) = &toast.file {
                            reveal_in_os(f);
                        }
                    }
                    None => {}
                }
                if d {
                    dismissed = Some(toast.id);
                }
                if h {
                    hovered = Some(toast.id);
                }
            }
        });
    if let Some(id) = hovered {
        state.toasts.hold(id, now + 1.5);
    }
    if copied {
        state.notify("Copied the message");
    }
    if let Some(id) = dismissed {
        state.toasts.dismiss(id);
    }
    clicked
}

/// What was clicked on a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToastClick {
    /// The toast itself: open the Console at the record.
    Body,
    Copy,
    ShowFile,
}

/// Height of the Copy / Show File row of warning and error toasts.
const ACTIONS_H: f32 = 20.0;

/// One toast: a level bar, the message (what happened, up to two lines, then
/// the fix after "; ", up to three lines), a repeat count,
/// a close button and, for warnings and errors, Copy and Show File.
/// Returns (clicked, hovered, closed).
fn toast_ui(
    ui: &mut Ui,
    toast: &Toast,
    width: f32,
    t: &Tokens,
) -> (Option<ToastClick>, bool, bool) {
    let color = level_color(toast.severity, t);
    // Messages say what happened, then what to do after "; ". The fix gets
    // its own rows so a long first part cannot push it out of the toast.
    let (what, fix) = split_fix(&toast.text);
    let mut job = LayoutJob::simple(what.to_owned(), t.body(), t.text_strong, width - 58.0);
    job.wrap.max_rows = if fix.is_some() { 2 } else { 4 };
    let galley = ui.painter().layout_job(job);
    let fix_galley = fix.map(|fix| {
        let mut job = LayoutJob::simple(fix.to_owned(), t.body(), t.text_weak, width - 58.0);
        job.wrap.max_rows = 3;
        ui.painter().layout_job(job)
    });
    let actions = toast.severity != Severity::Info;
    let fix_h = fix_galley.as_ref().map_or(0.0, |g| g.size().y + 2.0);
    let text_h = galley.size().y + fix_h + 18.0;
    let h = text_h + if actions { ACTIONS_H } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let id = egui::Id::unique(("box_editor_toast", toast.id));
    // Action buttons, right-aligned under the text.
    let mut buttons: Vec<(ToastClick, &str, Rect)> = Vec::new();
    if actions {
        let mut x = rect.right() - 10.0;
        let mut add = |kind, label: &'static str| {
            let w = ui
                .painter()
                .layout_no_wrap(label.to_owned(), t.small(), t.text)
                .size()
                .x
                + 12.0;
            let r = Rect::from_min_size(pos2(x - w, rect.top() + text_h - 6.0), vec2(w, 18.0));
            x -= w + 6.0;
            buttons.push((kind, label, r));
        };
        add(ToastClick::Copy, "Copy");
        if toast.file.is_some() {
            add(ToastClick::ShowFile, "Show File");
        }
    }
    let body = ui.interact(rect, id, Sense::click());
    let close_rect = Rect::from_center_size(
        pos2(rect.right() - 14.0, rect.top() + 14.0),
        vec2(18.0, 18.0),
    );
    let close = ui.interact(close_rect, id.with("close"), Sense::click());
    let p = ui.painter();
    let fill = if body.hovered() {
        t.field_hover
    } else {
        t.field
    };
    p.rect_filled(rect, CornerRadius::same(5), fill);
    p.rect_stroke(
        rect,
        CornerRadius::same(5),
        Stroke::new(1.0, color.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );
    p.rect_filled(
        Rect::from_min_size(rect.min, vec2(4.0, rect.height())),
        CornerRadius {
            nw: 5,
            sw: 5,
            ne: 0,
            se: 0,
        },
        color,
    );
    let what_h = galley.size().y;
    p.galley(
        pos2(rect.left() + 14.0, rect.top() + 9.0),
        galley,
        t.text_strong,
    );
    if let Some(fix) = fix_galley {
        p.galley(
            pos2(rect.left() + 14.0, rect.top() + 11.0 + what_h),
            fix,
            t.text_weak,
        );
    }
    if toast.count > 1 {
        let g = p.layout_no_wrap(format!("×{}", toast.count), t.mono(), t.text_strong);
        let r = Rect::from_min_size(
            pos2(close_rect.left() - g.size().x - 14.0, rect.top() + 6.0),
            g.size() + vec2(8.0, 4.0),
        );
        p.rect_filled(r, CornerRadius::same(3), color.gamma_multiply(0.35));
        p.galley(r.min + vec2(4.0, 2.0), g, t.text_strong);
    }
    let mut action = None;
    let mut action_hovered = false;
    for (kind, label, r) in &buttons {
        let resp = ui.interact(*r, id.with(label), Sense::click());
        let fill = if resp.hovered() {
            t.field_hover
        } else {
            t.bg_app
        };
        let p = ui.painter();
        p.rect_filled(*r, CornerRadius::same(3), fill);
        p.rect_stroke(
            *r,
            CornerRadius::same(3),
            Stroke::new(1.0, t.border),
            egui::StrokeKind::Inside,
        );
        let g = p.layout_no_wrap((*label).to_owned(), t.small(), t.text_strong);
        p.galley(r.center() - g.size() / 2.0, g, t.text_strong);
        if resp.clicked() {
            action = Some(*kind);
        }
        action_hovered |= resp.hovered();
    }
    let p = ui.painter();
    let xc = if close.hovered() {
        t.text_strong
    } else {
        t.text_weak
    };
    let c = close_rect.center();
    let s = 4.0;
    p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], Stroke::new(1.4, xc));
    p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], Stroke::new(1.4, xc));
    let closed = close.clicked();
    let clicked =
        action.or((body.clicked() && !closed && !action_hovered).then_some(ToastClick::Body));
    (
        clicked,
        body.hovered() || close.hovered() || action_hovered,
        closed,
    )
}

fn level_color(s: Severity, t: &Tokens) -> Color32 {
    match s {
        Severity::Info => t.accent,
        Severity::Warn => t.warn,
        Severity::Error => t.error,
    }
}

/// Unseen warning and error counts as small coloured pills after a tab
/// title, starting at `x`. Returns the width used (0 for no badge).
pub(crate) fn badge_width(unseen: [usize; 2], ui: &Ui, t: &Tokens) -> f32 {
    pills(unseen, t)
        .map(|(text, _)| ui.painter().layout_no_wrap(text, t.mono(), t.text).size().x + 10.0 + 4.0)
        .sum()
}

pub(crate) fn paint_badge(p: &egui::Painter, unseen: [usize; 2], x: f32, cy: f32, t: &Tokens) {
    let mut x = x;
    for (text, color) in pills(unseen, t) {
        let g = p.layout_no_wrap(text, t.mono(), t.bg_app);
        let r = Rect::from_min_size(pos2(x, cy - 8.0), vec2(g.size().x + 10.0, 16.0));
        p.rect_filled(r, CornerRadius::same(8), color);
        p.galley(pos2(r.left() + 5.0, cy - g.size().y / 2.0), g, t.bg_app);
        x = r.right() + 4.0;
    }
}

fn pills(unseen: [usize; 2], t: &Tokens) -> impl Iterator<Item = (String, Color32)> {
    let count = |n: usize| {
        if n > 99 {
            "99+".to_owned()
        } else {
            n.to_string()
        }
    };
    [(unseen[0], t.warn), (unseen[1], t.error)]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(move |(n, c)| (count(n), c))
}

/// "Playing — changes are discarded when you stop" (or Paused) above the
/// inspector while Play runs.
pub(crate) fn play_banner(ui: &mut Ui, state: &EditorState, t: &Tokens) {
    let (color, state_text) = match state.play_state() {
        PlayState::Editing => return,
        PlayState::Playing => (t.accent, "Playing"),
        PlayState::Paused => (t.warn, "Paused"),
    };
    // Values marked "Keep after Stop" (inspector menus) survive Stop.
    let text = match state.kept_changes().len() {
        0 => format!("{state_text} — changes are discarded when you stop"),
        1 => format!("{state_text} — 1 change will be kept, the rest discarded"),
        n => format!("{state_text} — {n} changes will be kept, the rest discarded"),
    };
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let r = r.shrink2(vec2(t.pad_x, 2.0));
    let p = ui.painter();
    p.rect_filled(
        r,
        CornerRadius::same(t.radius + 1),
        color.gamma_multiply(0.20),
    );
    p.rect_stroke(
        r,
        CornerRadius::same(t.radius + 1),
        Stroke::new(1.0, color.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    p.circle_filled(pos2(r.left() + 12.0, r.center().y), 3.5, color);
    p.with_clip_rect(r).text(
        pos2(r.left() + 22.0, r.center().y),
        Align2::LEFT_CENTER,
        text,
        t.body(),
        t.text_strong,
    );
    ui.add_space(4.0);
}

/// Read-only, collapsed sections for the primary entity's unregistered
/// components: the type name, a hint, and the raw data that is kept.
pub(crate) fn unknown_components_ui(ui: &mut Ui, state: &EditorState, e: Entity, t: &Tokens) {
    for (name, json) in unknown_components(&state.world, e) {
        let id = egui::Id::unique(("box_editor_unknown_open", &name));
        let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
        let title = format!("{name} — not registered (data kept)");
        crate::widgets::section_with_menu(
            ui,
            &title,
            Some(UNKNOWN_HINT),
            &mut open,
            None::<fn(&mut Ui)>,
            |ui| {
                let inner = ui.available_width() - 2.0 * (t.pad_x + 4.0);
                ui.horizontal(|ui| {
                    ui.add_space(t.pad_x + 4.0);
                    ui.vertical(|ui| {
                        ui.set_width(inner);
                        ui.label(egui::RichText::new(UNKNOWN_HINT).color(t.text_weak));
                        ui.add_space(4.0);
                        let lines: Vec<&str> = json.lines().collect();
                        let mut text = lines
                            .iter()
                            .take(24)
                            .copied()
                            .collect::<Vec<_>>()
                            .join("\n");
                        if lines.len() > 24 {
                            text.push_str("\n…");
                        }
                        ui.label(egui::RichText::new(text).font(t.mono()).color(t.text_faint));
                    });
                });
            },
        );
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
}

/// Under a MeshAsset or PrefabInstance section: why the entity draws
/// nothing, when its asset failed or is missing.
pub(crate) fn asset_error_note(ui: &mut Ui, state: &EditorState, component: &str, t: &Tokens) {
    if component != "MeshAsset" && component != "PrefabInstance" {
        return;
    }
    let Some(error) = state.primary().and_then(|u| state.asset_error(u)) else {
        return;
    };
    let wanted = if component == "MeshAsset" {
        "Mesh "
    } else {
        "Prefab "
    };
    if !error.starts_with(wanted) {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(t.pad_x + 4.0);
        let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
        warn_glyph(ui.painter(), r.center(), t);
        ui.add(egui::Label::new(egui::RichText::new(error).color(t.warn)).wrap());
    });
}

/// A small warning triangle with an exclamation mark.
pub(crate) fn warn_glyph(p: &egui::Painter, c: egui::Pos2, t: &Tokens) {
    let pts = vec![c + vec2(0.0, -6.0), c + vec2(6.5, 5.0), c + vec2(-6.5, 5.0)];
    p.add(egui::Shape::convex_polygon(pts, t.warn, Stroke::NONE));
    p.line_segment(
        [c + vec2(0.0, -2.5), c + vec2(0.0, 1.5)],
        Stroke::new(1.5, t.bg_app),
    );
    p.circle_filled(c + vec2(0.0, 3.4), 0.9, t.bg_app);
}

#[cfg(test)]
mod split_tests {
    use super::split_fix;

    #[test]
    fn the_fix_after_the_first_semicolon_gets_its_own_rows() {
        assert_eq!(
            split_fix("mesh a.glb: no such file; fix the reference; drawing the placeholder"),
            (
                "mesh a.glb: no such file",
                Some("fix the reference; drawing the placeholder")
            )
        );
        assert_eq!(split_fix("scene saved"), ("scene saved", None));
        assert_eq!(split_fix("; nothing before"), ("; nothing before", None));
    }
}
