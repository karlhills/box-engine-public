//! Viewport tools: the editor's extension point for direct manipulation
//! beyond the transform gizmo (ROADMAP M5; spline editing in M5, wall and
//! path drawing in M6).
//!
//! A [`ViewportTool`] is registered with [`EditorState::register_tool`]
//! (or [`crate::Editor::register_tool`]; game plugins call it from their
//! editor setup). Registering adds it to the toolbar's tool group after
//! Move, Rotate and Scale, and an action `tool.<id>` to the command palette
//! (Tools). While a tool is active and [`ViewportTool::wants_input`]:
//!
//! - left-button input over the viewport goes to the tool
//!   ([`ViewportTool::on_pointer`]) instead of the gizmo, the marquee and
//!   picking; Alt + drag, middle drag, the wheel and fly still navigate;
//! - key presses go to [`ViewportTool::on_key`] first (Escape excepted);
//! - the gizmo is hidden; the tool draws its own overlay in 3D
//!   ([`ViewportTool::draw_overlay`], post-tonemap lines) and 2D
//!   ([`ViewportTool::paint`]), and a hint pill ([`ViewportTool::hint`]).
//!
//! Escape first goes to [`ViewportTool::on_escape`], which may cancel just
//! the operation in progress (a drawing) and keep the tool; otherwise
//! Escape, W/E/R or picking a transform tool calls [`ViewportTool::cancel`]
//! and goes back to the transform tools. Every scene change a tool makes
//! goes through the `EditStack`, one undo step per completed operation:
//! wrap it in [`EditorState::transaction`].
//!
//! The built-in tools are [`PlaceOnSurface`], Edit Spline
//! ([`crate::spline_tool::SplineTool`]) and Paint Scatter
//! ([`crate::scatter_brush::ScatterBrush`]). [`crate::path_tool::PathTool`]
//! is a configurable tool games register for their path intents (walls,
//! fences). Games register tools and palette actions from an
//! [`EditorPlugin`](crate::EditorPlugin) passed to
//! [`run_game_with`](crate::run_game_with).

use box_core::color::Color;
use box_core::identity::Uid;
use box_core::math::{Aabb, Ray};
use box_core::mesh::MeshData;
use box_core::prelude::{Assets, Entity, GlobalTransform, Transform};
use box_render::{DebugDraw, LineLayer, MeshRenderer};
use glam::{Affine3A, Quat, Vec2, Vec3};

use crate::state::{EditorState, TransformTool};
use crate::theme::Tokens;

/// What happened with the pointer over the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolEventKind {
    /// The pointer moved (or stayed) over the viewport; sent every frame
    /// while it is there, button up or down.
    Hover,
    /// The left button went down.
    Press,
    /// The left button went up after a drag (see [`Self::Click`] for one
    /// without movement).
    Release,
    /// A left click (press and release without dragging).
    Click,
}

/// A pointer event for a tool, in the viewport.
#[derive(Clone, Copy, Debug)]
pub struct ToolEvent {
    pub kind: ToolEventKind,
    /// Cursor position in points from the viewport's top-left corner.
    pub cursor: Vec2,
    /// The world-space ray through the cursor.
    pub ray: Ray,
    pub modifiers: egui::Modifiers,
    /// The left button is held.
    pub primary_down: bool,
}

/// What a tool did with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolResponse {
    /// Not used.
    Ignored,
    /// Used; the tool stays active.
    Handled,
    /// Used, and the tool is done: back to the transform tools.
    Finished,
}

/// An editor viewport tool (see the module docs).
///
/// ```
/// use box_editor::tools::{ToolEvent, ToolEventKind, ToolResponse, ViewportTool};
/// use box_editor::EditorState;
///
/// /// Clicks print the point under the cursor on the ground plane.
/// struct Probe;
/// impl ViewportTool for Probe {
///     fn id(&self) -> &'static str { "probe" }
///     fn label(&self) -> &'static str { "Probe" }
///     fn tooltip(&self) -> &'static str { "Click to log the ground point" }
///     fn on_pointer(&mut self, _state: &mut EditorState, e: &ToolEvent) -> ToolResponse {
///         if e.kind != ToolEventKind::Click {
///             return ToolResponse::Ignored;
///         }
///         let t = -e.ray.origin.y / e.ray.direction.y;
///         tracing::info!("ground at {}", e.ray.origin + e.ray.direction * t);
///         ToolResponse::Handled
///     }
/// }
/// # fn f(state: &mut EditorState) {
/// state.register_tool(Probe);
/// assert!(state.activate_tool("probe"));
/// # }
/// ```
pub trait ViewportTool {
    /// Stable id; the palette action is `tool.<id>`.
    fn id(&self) -> &'static str;
    /// The toolbar label (one short word).
    fn label(&self) -> &'static str;
    /// The full name, for the palette ("<name> Tool") and hints; the label
    /// by default.
    fn name(&self) -> &'static str {
        self.label()
    }
    /// Toolbar and palette tooltip.
    fn tooltip(&self) -> &'static str;
    /// Whether the tool takes viewport input now. When false (for example
    /// nothing is selected for a tool that needs a selection), clicks
    /// select as usual and the gizmo shows.
    fn wants_input(&self, _state: &EditorState) -> bool {
        true
    }
    /// A pointer event over the viewport (while [`Self::wants_input`]).
    fn on_pointer(&mut self, state: &mut EditorState, event: &ToolEvent) -> ToolResponse;
    /// A key press while active (not Escape); return true to consume it.
    fn on_key(
        &mut self,
        _state: &mut EditorState,
        _key: egui::Key,
        _modifiers: egui::Modifiers,
    ) -> bool {
        false
    }
    /// Escape while active: return true to cancel only the operation in
    /// progress (a drawing, a drag) and stay active; false leaves the tool
    /// ([`Self::cancel`] runs).
    fn on_escape(&mut self, _state: &mut EditorState) -> bool {
        false
    }
    /// The tool is being left (Escape, another tool): drop any operation in
    /// progress. A tool that holds an open transaction rolls it back here.
    fn cancel(&mut self, _state: &mut EditorState) {}
    /// 3D overlay lines, queued before the viewport renders. Use the
    /// post-tonemap `LineLayer::Display*` layers.
    fn draw_overlay(&self, _state: &EditorState, _dd: &mut DebugDraw, _tokens: &Tokens) {}
    /// 2D overlay over the viewport (`viewport` is its rectangle).
    fn paint(&self, _state: &EditorState, _painter: &egui::Painter, _viewport: egui::Rect) {}
    /// A one-line hint shown at the top of the viewport while active.
    fn hint(&self, _state: &EditorState) -> Option<String> {
        None
    }
}

/// The registered tools and the active one.
#[derive(Default)]
pub(crate) struct ToolRegistry {
    /// `None` while a tool is taken out to be called with the state.
    tools: Vec<Option<Box<dyn ViewportTool>>>,
    /// What the toolbar and palette show (kept while a tool is taken out).
    info: Vec<ToolInfo>,
    active: Option<usize>,
    /// What the last hover event was sent for.
    last_hover: Option<HoverKey>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HoverKey {
    cursor: Vec2,
    modifiers: egui::Modifiers,
    eye: Affine3A,
    generation: u64,
    primary: Option<Uid>,
    selected: usize,
}

impl ToolRegistry {
    /// The built-in tools.
    pub(crate) fn builtin() -> Self {
        let mut r = Self::default();
        r.add(Box::new(PlaceOnSurface::default()));
        r.add(Box::new(crate::spline_tool::SplineTool::default()));
        r.add(Box::new(crate::scatter_brush::ScatterBrush::default()));
        r
    }

    pub(crate) fn info(&self) -> &[ToolInfo] {
        &self.info
    }

    fn add(&mut self, tool: Box<dyn ViewportTool>) {
        let entry = ToolInfo::of(tool.as_ref());
        match self.info.iter().position(|i| i.id == entry.id) {
            Some(i) => {
                if self.active == Some(i) {
                    self.active = None;
                }
                self.tools[i] = Some(tool);
                self.info[i] = entry;
            }
            None => {
                self.tools.push(Some(tool));
                self.info.push(entry);
            }
        }
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.info)
            .field("active", &self.active)
            .finish()
    }
}

/// A registered tool as the toolbar and the palette show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub name: &'static str,
    pub tooltip: &'static str,
}

impl ToolInfo {
    fn of(tool: &dyn ViewportTool) -> Self {
        Self {
            id: tool.id(),
            label: tool.label(),
            name: tool.name(),
            tooltip: tool.tooltip(),
        }
    }
}

/// The palette action for a tool.
pub(crate) fn tool_action(info: ToolInfo) -> crate::actions::Action {
    let ToolInfo {
        id, name, tooltip, ..
    } = info;
    crate::actions::Action::new(
        format!("tool.{id}"),
        "Tools",
        format!("{name} Tool"),
        move |s| {
            if s.active_tool() == Some(id) {
                s.cancel_tool();
            } else {
                s.activate_tool(id);
            }
        },
    )
    .tooltip(tooltip)
    .keywords("viewport tool")
    .checked_when(move |s| s.active_tool() == Some(id))
    .enabled_when(|s| {
        if s.is_playing() {
            Err("Stop Play first".into())
        } else {
            Ok(())
        }
    })
}

impl EditorState {
    /// Adds a viewport tool (replacing one with the same id): a segment in
    /// the toolbar's tool group and the palette action `tool.<id>`.
    pub fn register_tool(&mut self, tool: impl ViewportTool + 'static) {
        let info = ToolInfo::of(&tool);
        self.tools.add(Box::new(tool));
        self.register_action(tool_action(info));
    }

    /// The registered tools, in toolbar order.
    pub fn tools(&self) -> &[ToolInfo] {
        &self.tools.info
    }

    /// The active viewport tool's id (`None`: the transform tools).
    pub fn active_tool(&self) -> Option<&'static str> {
        self.tools.active.map(|i| self.tools.info[i].id)
    }

    /// Makes `id` the active tool, leaving the current one (and ending a
    /// gizmo or inspector drag). False if no tool has that id or Play runs.
    pub fn activate_tool(&mut self, id: &str) -> bool {
        let Some(i) = self.tools.info.iter().position(|t| t.id == id) else {
            return false;
        };
        if self.is_playing() {
            return false;
        }
        self.finish_edit();
        if self.tools.active != Some(i) {
            self.cancel_tool();
            self.tools.active = Some(i);
            self.tools.last_hover = None;
        }
        true
    }

    /// Leaves the active tool (its [`ViewportTool::cancel`] runs): back to
    /// the transform tools. False when none was active.
    pub fn cancel_tool(&mut self) -> bool {
        if self.tools.active.is_none() {
            return false;
        }
        self.with_active_tool(|tool, s| tool.cancel(s));
        self.tools.active = None;
        true
    }

    /// Picks a transform tool (Move, Rotate, Scale), leaving a viewport tool.
    pub fn select_transform_tool(&mut self, tool: TransformTool) {
        self.cancel_tool();
        self.tool = tool;
    }

    /// The active tool takes viewport input now.
    pub fn tool_wants_input(&self) -> bool {
        self.tools
            .active
            .and_then(|i| self.tools.tools[i].as_ref())
            .is_some_and(|t| t.wants_input(self))
    }

    /// Calls `f` with the active tool and the state (the tool is taken out
    /// of the registry meanwhile). `None` when no tool is active.
    pub(crate) fn with_active_tool<R>(
        &mut self,
        f: impl FnOnce(&mut dyn ViewportTool, &mut EditorState) -> R,
    ) -> Option<R> {
        let i = self.tools.active?;
        let mut tool = self.tools.tools.get_mut(i)?.take()?;
        let r = f(tool.as_mut(), self);
        if let Some(slot) = self.tools.tools.get_mut(i)
            && slot.is_none()
        {
            *slot = Some(tool);
        }
        Some(r)
    }

    /// True when a hover event is due: the cursor, the modifiers, the camera,
    /// the scene or the selection changed since the last one (so a tool's
    /// hover work, such as a pick, does not run every idle frame).
    pub(crate) fn tool_hover_changed(&mut self, cursor: Vec2, modifiers: egui::Modifiers) -> bool {
        let eye = self.camera.view().transform.0;
        let key = HoverKey {
            cursor,
            modifiers,
            eye,
            generation: self.generation,
            primary: self.primary(),
            selected: self.selection.len(),
        };
        if self.tools.last_hover == Some(key) {
            return false;
        }
        self.tools.last_hover = Some(key);
        true
    }

    /// Sends a pointer event to the active tool; a finished tool goes back
    /// to the transform tools.
    pub fn tool_pointer(&mut self, event: &ToolEvent) -> ToolResponse {
        let r = self
            .with_active_tool(|tool, s| tool.on_pointer(s, event))
            .unwrap_or(ToolResponse::Ignored);
        if r == ToolResponse::Finished {
            self.tools.active = None;
        }
        r
    }

    /// Offers Escape to the active tool; true if it cancelled an operation
    /// in progress and stays active.
    pub fn tool_escape(&mut self) -> bool {
        self.with_active_tool(|tool, s| tool.on_escape(s))
            .unwrap_or(false)
    }

    /// Offers a key press to the active tool; true if it used it.
    pub fn tool_key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> bool {
        self.with_active_tool(|tool, s| tool.on_key(s, key, modifiers))
            .unwrap_or(false)
    }

    /// The active tool's hint.
    pub fn tool_hint(&self) -> Option<String> {
        let i = self.tools.active?;
        self.tools.tools[i].as_ref()?.hint(self)
    }

    pub(crate) fn draw_tool_overlay(&self, dd: &mut DebugDraw, tokens: &Tokens) {
        if let Some(t) = self.tools.active.and_then(|i| self.tools.tools[i].as_ref())
            && t.wants_input(self)
        {
            t.draw_overlay(self, dd, tokens);
        }
    }

    pub(crate) fn paint_tool(&self, painter: &egui::Painter, viewport: egui::Rect) {
        if let Some(t) = self.tools.active.and_then(|i| self.tools.tools[i].as_ref())
            && t.wants_input(self)
        {
            t.paint(self, painter, viewport);
        }
    }

    /// Runs `f` as one undo step labelled `label`: every edit it makes
    /// through the state's edit methods (`set_field`, `add_entity`, ...)
    /// merges into that step. Ends a drag in progress first. What viewport
    /// tools use to commit an operation.
    pub fn transaction<R>(
        &mut self,
        label: impl Into<String>,
        f: impl FnOnce(&mut EditorState) -> R,
    ) -> R {
        self.begin_edit(label);
        let r = f(self);
        self.finish_edit();
        r
    }
}

// ----------------------------------------------------------- built-in ----

/// Place on Surface: click a surface to drop the selection onto it, its up
/// axis turned to the surface normal and its lowest point resting on it
/// (Shift keeps the rotation). Other selected entities move with the
/// primary one as a rigid group. Each click is one undo step; the tool stays
/// active for the next one. Without a mesh under the cursor the ground plane
/// (y = 0) is used. The selection itself is never hit.
#[derive(Debug, Default)]
pub struct PlaceOnSurface {
    preview: Option<Placement>,
}

/// Where the primary selection would go.
#[derive(Clone, Copy, Debug)]
struct Placement {
    point: Vec3,
    normal: Vec3,
    /// The primary's new world transform.
    target: Affine3A,
    /// The primary's mesh bounds in its own space, for the ghost.
    bounds: Option<Aabb>,
    /// From the primary's current to its new world transform.
    delta: Affine3A,
}

impl ViewportTool for PlaceOnSurface {
    fn id(&self) -> &'static str {
        "place_on_surface"
    }

    fn label(&self) -> &'static str {
        "Place"
    }

    fn name(&self) -> &'static str {
        "Place on Surface"
    }

    fn tooltip(&self) -> &'static str {
        "Place on Surface: click a surface to put the selection on it, turned to the \
         surface (Shift keeps the rotation). Esc returns to Move."
    }

    fn wants_input(&self, state: &EditorState) -> bool {
        !state.gizmo_targets().is_empty()
    }

    fn on_pointer(&mut self, state: &mut EditorState, event: &ToolEvent) -> ToolResponse {
        match event.kind {
            ToolEventKind::Hover => {
                self.preview = placement(state, &event.ray, event.modifiers.shift);
                ToolResponse::Handled
            }
            ToolEventKind::Click => {
                self.preview = placement(state, &event.ray, event.modifiers.shift);
                if let Some(p) = self.preview {
                    commit(state, &p);
                    // The selection moved: the next hover recomputes.
                    self.preview = None;
                }
                ToolResponse::Handled
            }
            ToolEventKind::Press | ToolEventKind::Release => ToolResponse::Handled,
        }
    }

    fn cancel(&mut self, _state: &mut EditorState) {
        self.preview = None;
    }

    fn draw_overlay(&self, _state: &EditorState, dd: &mut DebugDraw, tokens: &Tokens) {
        let Some(p) = &self.preview else {
            return;
        };
        let accent = color(tokens.accent);
        // A ring on the surface and the normal.
        let (u, v) = p.normal.any_orthonormal_pair();
        let r = 0.35;
        let n = 32;
        let ring = |k: usize| {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            p.point + (u * a.cos() + v * a.sin()) * r + p.normal * 0.005
        };
        for k in 0..n {
            dd.line_on(LineLayer::DisplayOverlay, ring(k), ring(k + 1), accent);
        }
        dd.line_on(
            LineLayer::DisplayOverlay,
            p.point,
            p.point + p.normal * 0.6,
            accent,
        );
        // The ghost: the primary's bounds where it would go.
        if let Some(b) = p.bounds {
            let ghost = accent.with_alpha(0.75);
            let corners = box_corners(&b).map(|c| p.target.transform_point3(c));
            for (a, b) in crate::marquee::BOX_EDGES {
                dd.line_on(LineLayer::Display, corners[a], corners[b], ghost);
            }
        }
    }

    fn hint(&self, state: &EditorState) -> Option<String> {
        Some(match state.primary_entity() {
            Some(e) if self.wants_input(state) => format!(
                "Place on Surface  ·  click to put {} there  ·  Shift keeps rotation  ·  Esc: Move",
                state.display_name(e)
            ),
            _ => "Place on Surface  ·  select what to place  ·  Esc: Move".into(),
        })
    }
}

pub(crate) fn color(c: egui::Color32) -> Color {
    Color::srgb(
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
    )
}

fn box_corners(b: &Aabb) -> [Vec3; 8] {
    let (lo, hi) = (b.min, b.max);
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    })
}

/// True if `e` is a selected (topmost) target or below one.
fn in_targets(state: &EditorState, targets: &[Uid], e: Entity) -> bool {
    let mut cur = Some(e);
    while let Some(c) = cur {
        if state.world.uid(c).is_some_and(|u| targets.contains(&u)) {
            return true;
        }
        cur = state.world.parent(c);
    }
    false
}

/// The bounds of every mesh in `root`'s subtree, in `root`'s space.
fn local_bounds(state: &EditorState, root: Entity, root_global: &Affine3A) -> Option<Aabb> {
    let meshes = state.world.get_resource::<Assets<MeshData>>()?;
    let inv = root_global.inverse();
    let mut out: Option<Aabb> = None;
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        stack.extend(state.world.children(e));
        let (Some(mr), Some(gt)) = (
            state.world.get::<MeshRenderer>(e),
            state.world.get::<GlobalTransform>(e),
        ) else {
            continue;
        };
        let Some(mesh) = meshes.get(mr.mesh).filter(|m| !m.positions.is_empty()) else {
            continue;
        };
        let b = mesh.bounds().transformed(&(inv * gt.0));
        out = Some(match out {
            Some(acc) => acc.including(b.min).including(b.max),
            None => b,
        });
    }
    out
}

/// Where the primary selection lands for a click along `ray`.
fn placement(state: &EditorState, ray: &Ray, keep_rotation: bool) -> Option<Placement> {
    let targets: Vec<Uid> = state.gizmo_targets().iter().map(|(_, u)| *u).collect();
    let primary = state.primary_entity()?;
    let current = state.world.get::<GlobalTransform>(primary)?.0;
    let hit = crate::picking::pick_where(&state.world, ray, |e| {
        in_targets(state, &targets, e) || state.blocked_in_viewport(e)
    });
    let (point, normal) = match hit {
        Some(h) => (ray.origin + ray.direction * h.distance, h.normal),
        None if ray.direction.y < -1e-4 => {
            let t = -ray.origin.y / ray.direction.y;
            (ray.origin + ray.direction * t, Vec3::Y)
        }
        None => return None,
    };
    let normal = normal.normalize_or(Vec3::Y);
    let (scale, rotation, _) = current.to_scale_rotation_translation();
    let rotation = if keep_rotation {
        rotation
    } else {
        (Quat::from_rotation_arc((rotation * Vec3::Y).normalize_or(Vec3::Y), normal) * rotation)
            .normalize()
    };
    let bounds = local_bounds(state, primary, &current);
    // Rest the lowest point (along the normal) on the surface.
    let lift = bounds.map_or(0.0, |b| {
        -box_corners(&b)
            .iter()
            .map(|c| (rotation * (scale * *c)).dot(normal))
            .fold(f32::INFINITY, f32::min)
    });
    let target = Affine3A::from_scale_rotation_translation(scale, rotation, point + normal * lift);
    Some(Placement {
        point,
        normal,
        target,
        bounds,
        delta: target * current.inverse(),
    })
}

/// Moves the targets by the placement's delta: one undo step.
fn commit(state: &mut EditorState, p: &Placement) {
    let targets = state.gizmo_targets();
    let what = match targets.as_slice() {
        [(e, _)] => state.display_name(*e),
        many => format!("{} entities", many.len()),
    };
    let mut moves: Vec<(Uid, Transform)> = Vec::new();
    for (e, uid) in &targets {
        let Some(global) = state.world.get::<GlobalTransform>(*e).map(|g| g.0) else {
            continue;
        };
        let parent = state
            .world
            .parent(*e)
            .and_then(|p| state.world.get::<GlobalTransform>(p).map(|g| g.0))
            .unwrap_or(Affine3A::IDENTITY);
        let local = parent.inverse() * p.delta * global;
        let (scale, rotation, translation) = local.to_scale_rotation_translation();
        // Keep the exact local scale: the delta is rigid.
        let scale = state.world.get::<Transform>(*e).map_or(scale, |t| t.scale);
        moves.push((
            *uid,
            Transform {
                translation,
                rotation: rotation.normalize(),
                scale,
            },
        ));
    }
    state.transaction(format!("Place {what} on Surface"), |s| {
        for (uid, t) in &moves {
            s.set_field(*uid, "Transform", t);
        }
    });
}
