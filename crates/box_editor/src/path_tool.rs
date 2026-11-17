//! Draw paths (M6, docs/PROCEDURAL_DIRECTION.md §6 "Draw walls and paths"):
//! a generic viewport tool that draws and edits the points of a game's path
//! intent (a wall, a fence, a road), configured by [`PathToolConfig`] and
//! registered like any tool ([`EditorState::register_tool`], or from a
//! game's [`EditorPlugin`](crate::EditorPlugin)).
//!
//! **Drawing.** Click to place points on surfaces (the editor's mesh ray,
//! `box_physics::MeshRayCache`, passing through the generated geometry of
//! the paths it snaps to; the ground plane y = 0 without a hit). Points
//! snap to other paths' ends and bodies with hysteresis
//! ([`box_engine::box_proc::snap::Snapper`]: connect within 10 cm, let go
//! beyond 15 cm). While drawing, a **ghost** shows the real result: a
//! runtime-only preview entity ([`Preview`]) carries the intent with the
//! placed points and the cursor, so the game's rules and generators build
//! it, and its neighbours' junctions change with it. The preview is never
//! saved, never in undo and never in the hierarchy; it is rewritten only
//! when the snapped cursor point changes (no work while the mouse is still).
//! **Backspace** removes the last point; **Enter**, a click on the last
//! point (a double-click) or a click on the first point (closing, when the
//! config allows it) commits the path as **one undo step** through
//! `box_core::edit`; **Esc** cancels the drawing (a second Esc leaves the
//! tool).
//!
//! **Editing.** With a path of the tool's component selected, its points
//! show as handles: drag one on the horizontal plane through it (its ends
//! snap like drawn points); each drag is one undo step. A click on a
//! handle without dragging starts a new path there (snapped to it).
//!
//! Cost per pointer move while drawing: one mesh ray (bounds pre-filter,
//! cached BVHs), one snap over the cached targets (rebuilt only when the
//! scene changed), and one reflected write of the preview's points; the
//! next frame's rule pass and the throttled flush (at most once per 30 ms)
//! do the rest. Nothing runs while the pointer and camera are still.

use box_core::color::Color;
use box_core::derived::Preview;
use box_core::identity::{Name, Uid};
use box_core::math::Ray;
use box_core::prelude::{Entity, GlobalTransform, Transform};
use box_core::reflect::{FromReflect, PartialReflect, Value};
use box_core::world::{Placement, World};
use box_engine::box_proc::snap::{SnapHit, SnapSegment, SnapTargets, Snapper};
use box_render::{DebugDraw, LineLayer};
use glam::{Affine3A, Vec2, Vec3};

use crate::state::EditorState;
use crate::theme::Tokens;
use crate::tools::{ToolEvent, ToolEventKind, ToolResponse, ViewportTool};

/// Pick radius of points (handles, the first and last drawn point), in
/// points.
const HIT: f32 = 9.0;
/// Points closer than this are the same point (metres).
const SAME_POINT: f32 = 1e-3;

/// How a drawn path closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    /// Never: a click on the first point is an ordinary point.
    Open,
    /// A click on the first point (with 3 or more points) appends it
    /// again, so the path ends where it starts (walls: the rules mitre the
    /// junction).
    RepeatFirstPoint,
    /// A click on the first point (with 3 or more points) sets this `bool`
    /// field of the component.
    Field(&'static str),
}

/// What points snap to: the ends and bodies of paths of `component`.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapSource {
    pub component: &'static str,
    /// Its `Vec<Vec3>` field of owner-local points.
    pub points: &'static str,
    /// A field giving the body's width and the factor to its half width
    /// (`("thickness", 0.5)`): a point within that plus 10 cm of the
    /// centreline snaps onto it. `None`: bodies snap within 10 cm.
    pub half_width: Option<(&'static str, f32)>,
}

/// A path tool: which component it draws, with which defaults (see the
/// module docs).
///
/// ```
/// use box_editor::path_tool::{Closing, PathTool, PathToolConfig};
///
/// let walls = PathToolConfig::new("draw_wall", "Wall", "WallPath", "points")
///     .name("Draw Wall")
///     .tooltip("Click to draw a wall; Enter to finish")
///     .default_field("height", serde_json::json!(2.5))
///     .closing(Closing::RepeatFirstPoint)
///     .snap_half_width("thickness", 0.5);
/// let tool = PathTool::new(walls);
/// # let _ = tool;
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct PathToolConfig {
    pub id: &'static str,
    /// Toolbar label (one short word).
    pub label: &'static str,
    /// Palette name ("`<name> Tool`") and hints.
    pub name: &'static str,
    pub tooltip: &'static str,
    /// The intent component created and edited (`WallPath`).
    pub component: &'static str,
    /// Its `Vec<Vec3>` field of owner-local points (`points`).
    pub points: &'static str,
    /// Values of other fields for new paths, by field path relative to the
    /// component, as JSON in the scene-file representation.
    pub defaults: Vec<(&'static str, serde_json::Value)>,
    pub closing: Closing,
    /// Base name of new entities (numbered when taken).
    pub entity_name: &'static str,
    /// What points snap to; the tool's own component first by default.
    pub snap_to: Vec<SnapSource>,
    /// New paths start as a copy of a path's whole component instead of
    /// its default ([`PathToolConfig::copy_selected`]).
    pub copy_selected: bool,
}

impl PathToolConfig {
    /// A tool `id` (palette action `tool.<id>`) labelled `label` that draws
    /// `component` paths through its `points` field, open, snapping to the
    /// same component's ends and bodies.
    pub fn new(
        id: &'static str,
        label: &'static str,
        component: &'static str,
        points: &'static str,
    ) -> Self {
        Self {
            id,
            label,
            name: label,
            tooltip: "Click to place points, Enter to finish, Backspace removes the last one",
            component,
            points,
            defaults: Vec::new(),
            closing: Closing::Open,
            entity_name: label,
            snap_to: vec![SnapSource {
                component,
                points,
                half_width: None,
            }],
            copy_selected: false,
        }
    }

    /// New paths copy the component of the selected path (or, with none
    /// selected, of the path last drawn or selected while the tool was
    /// active), so its other fields carry over: a `ScriptIntent`'s rule
    /// and properties. With nothing to copy yet, clicks select as usual
    /// and the hint asks for a path to draw more like.
    pub fn copy_selected(mut self) -> Self {
        self.copy_selected = true;
        self
    }

    pub fn name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    pub fn tooltip(mut self, tooltip: &'static str) -> Self {
        self.tooltip = tooltip;
        self
    }

    pub fn entity_name(mut self, name: &'static str) -> Self {
        self.entity_name = name;
        self
    }

    /// A value for `field` (relative to the component) on new paths.
    pub fn default_field(mut self, field: &'static str, value: serde_json::Value) -> Self {
        self.defaults.push((field, value));
        self
    }

    pub fn closing(mut self, closing: Closing) -> Self {
        self.closing = closing;
        self
    }

    /// The tool's own component's bodies have a half width: `field` ×
    /// `factor`.
    pub fn snap_half_width(mut self, field: &'static str, factor: f32) -> Self {
        if let Some(own) = self
            .snap_to
            .iter_mut()
            .find(|s| s.component == self.component)
        {
            own.half_width = Some((field, factor));
        }
        self
    }

    /// Also snap to the ends and bodies of another component's paths.
    pub fn snap_to(mut self, source: SnapSource) -> Self {
        self.snap_to.retain(|s| s.component != source.component);
        self.snap_to.push(source);
        self
    }

    fn points_path(&self) -> String {
        format!("{}.{}", self.component, self.points)
    }
}

/// The snap targets of the scene, cached while it does not change.
struct Targets {
    generation: u64,
    /// The path left out (the one whose point is dragged).
    without: Option<Uid>,
    snap: SnapTargets,
}

/// A handle drag in progress.
#[derive(Clone, Copy, Debug)]
struct PointDrag {
    uid: Uid,
    index: usize,
    anchor: Vec3,
    anchor_hit: Option<Vec3>,
    moved: bool,
    /// The point is an end (it snaps).
    end: bool,
    snapper: Snapper,
}

/// The path tool (see the module docs).
pub struct PathTool {
    config: PathToolConfig,
    /// Placed points (world space) while drawing.
    points: Vec<Vec3>,
    /// The snapped point under the cursor and what it snapped to.
    cursor: Option<Vec3>,
    snap: Option<SnapHit>,
    /// The cursor is on the first point and a click closes the path.
    closing: bool,
    snapper: Snapper,
    /// The preview entity while drawing, and the points last written to it.
    preview: Option<Uid>,
    written: Vec<Vec3>,
    targets: Option<Targets>,
    drag: Option<PointDrag>,
    hover_handle: Option<usize>,
    /// What new paths copy ([`PathToolConfig::copy_selected`]).
    template: Option<Value>,
}

impl PathTool {
    pub fn new(config: PathToolConfig) -> Self {
        Self {
            config,
            points: Vec::new(),
            cursor: None,
            snap: None,
            closing: false,
            snapper: Snapper::new(),
            preview: None,
            written: Vec::new(),
            targets: None,
            drag: None,
            hover_handle: None,
            template: None,
        }
    }

    /// The selected path's whole component, which new paths copy (the
    /// last one is kept while nothing with the component is selected).
    fn take_template(&mut self, state: &EditorState) {
        if !self.config.copy_selected {
            return;
        }
        let world = &state.world;
        let value = state
            .primary_entity()
            .filter(|e| !world.has::<Preview>(*e))
            .and_then(|e| {
                let whole = world.field_path(self.config.component).ok()?;
                world.get_field(e, &whole).ok()
            });
        if value.is_some() {
            self.template = value;
        }
    }

    /// Nothing to copy yet (a `copy_selected` tool before any path was
    /// selected).
    fn needs_template(&self, state: &EditorState) -> bool {
        self.config.copy_selected
            && self.template.is_none()
            && !self.is_drawing()
            && self.editing(state).is_none()
    }

    pub fn config(&self) -> &PathToolConfig {
        &self.config
    }

    /// Points placed so far (world space).
    pub fn placed(&self) -> &[Vec3] {
        &self.points
    }

    pub fn is_drawing(&self) -> bool {
        !self.points.is_empty()
    }

    /// What the cursor is snapped to.
    pub fn snap_hit(&self) -> Option<SnapHit> {
        self.snap
    }

    /// The preview entity's uid while drawing.
    pub fn preview_uid(&self) -> Option<Uid> {
        self.preview
    }
}

impl std::fmt::Debug for PathTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PathTool")
            .field("id", &self.config.id)
            .field("points", &self.points)
            .field("cursor", &self.cursor)
            .finish()
    }
}

// ----- reading paths by reflection -----

fn read_points(world: &World, e: Entity, component: &str, field: &str) -> Option<Vec<Vec3>> {
    let path = world.field_path(&format!("{component}.{field}")).ok()?;
    let v = world.get_field(e, &path).ok()?;
    Vec::<Vec3>::from_reflect(v.as_ref())
}

fn read_f32(world: &World, e: Entity, component: &str, field: &str) -> Option<f32> {
    let path = world.field_path(&format!("{component}.{field}")).ok()?;
    let v = world.get_field(e, &path).ok()?;
    f32::from_reflect(v.as_ref())
}

fn global(world: &World, e: Entity) -> Affine3A {
    world
        .get::<GlobalTransform>(e)
        .map_or(Affine3A::IDENTITY, |g| g.0)
}

/// The ends and bodies of every saved path of the snap sources, world
/// space, without `without`.
fn snap_targets(world: &World, sources: &[SnapSource], without: Option<Uid>) -> SnapTargets {
    let mut out = SnapTargets::default();
    let entities: Vec<(Entity, Uid)> = world
        .query::<box_core::prefab::NotGenerated<(Entity, &Uid)>>()
        .iter()
        .map(|(e, u)| (e, *u))
        .filter(|(_, u)| Some(*u) != without)
        .collect();
    for source in sources {
        for &(e, _) in &entities {
            let Some(points) = read_points(world, e, source.component, source.points) else {
                continue;
            };
            if points.len() < 2 {
                continue;
            }
            let g = global(world, e);
            let pts: Vec<Vec3> = points.iter().map(|p| g.transform_point3(*p)).collect();
            let reach = source
                .half_width
                .and_then(|(f, k)| read_f32(world, e, source.component, f).map(|w| w * k))
                .unwrap_or(0.0);
            out.points.push(pts[0]);
            out.points.push(pts[pts.len() - 1]);
            out.segments.extend(
                pts.windows(2)
                    .filter(|s| s[0].distance(s[1]) > SAME_POINT)
                    .map(|s| SnapSegment::new(s[0], s[1], reach)),
            );
        }
    }
    out
}

/// True if `e` belongs to a path the tool snaps to (its generated parts
/// are passed through by the surface ray) or to a preview.
fn is_path_geometry(world: &World, e: Entity, sources: &[SnapSource]) -> bool {
    let mut cur = Some(e);
    while let Some(c) = cur {
        if world.has::<Preview>(c) {
            return true;
        }
        if !box_core::prefab::is_generated(world, c) {
            let names = world.component_names(c);
            return sources.iter().any(|s| names.contains(&s.component));
        }
        cur = world
            .parent(c)
            .or_else(|| world.get::<box_core::derived::Derived>(c).map(|d| d.owner));
    }
    false
}

impl PathTool {
    fn targets(&mut self, state: &EditorState, without: Option<Uid>) -> &SnapTargets {
        let fresh = self
            .targets
            .as_ref()
            .is_some_and(|t| t.generation == state.generation && t.without == without);
        if !fresh {
            self.targets = Some(Targets {
                generation: state.generation,
                without,
                snap: snap_targets(&state.world, &self.config.snap_to, without),
            });
        }
        &self.targets.as_ref().expect("just set").snap
    }

    /// Where the ray meets a surface: the nearest mesh that is not path
    /// geometry, else the ground plane.
    fn surface(&self, state: &EditorState, ray: &Ray) -> Option<Vec3> {
        let sources = &self.config.snap_to;
        let hit = crate::picking::pick_where(&state.world, ray, |e| {
            state.blocked_in_viewport(e) || is_path_geometry(&state.world, e, sources)
        });
        match hit {
            Some(h) => Some(ray.origin + ray.direction * h.distance),
            None if ray.direction.y < -1e-4 => {
                let t = -ray.origin.y / ray.direction.y;
                Some(ray.origin + ray.direction * t)
            }
            None => None,
        }
    }

    /// The snapped point under the cursor (and the closing state).
    fn update_cursor(&mut self, state: &EditorState, e: &ToolEvent) {
        let Some(raw) = self.surface(state, &e.ray) else {
            self.cursor = None;
            self.snap = None;
            self.closing = false;
            return;
        };
        // The first point closes the path.
        let closable = self.config.closing != Closing::Open && self.points.len() >= 3;
        self.closing = closable
            && state.gizmo_view().is_some_and(|v| {
                v.project(self.points[0])
                    .is_some_and(|s| s.distance(e.cursor) <= HIT)
            });
        if self.closing {
            self.cursor = Some(self.points[0]);
            self.snap = Some(SnapHit::Point(self.points[0]));
            return;
        }
        let mut snapper = self.snapper;
        let (p, hit) = snapper.snap_to(raw, self.targets(state, None));
        self.snapper = snapper;
        self.cursor = Some(p);
        self.snap = hit;
    }

    /// The points the preview shows: placed, then the cursor.
    fn preview_points(&self) -> Vec<Vec3> {
        let mut pts = self.points.clone();
        if self.closing {
            if self.config.closing == Closing::RepeatFirstPoint {
                pts.push(self.points[0]);
            }
        } else if let Some(c) = self.cursor
            && pts.last().is_none_or(|l| l.distance(c) > SAME_POINT)
        {
            pts.push(c);
        }
        pts
    }

    /// Creates or updates the preview entity (runtime only: no edit, no
    /// generation bump). Writes only when its points changed.
    fn sync_preview(&mut self, state: &mut EditorState) {
        if self.points.is_empty() {
            self.remove_preview(state);
            return;
        }
        let pts = self.preview_points();
        let entity = self.preview.and_then(|u| {
            state
                .world
                .entity_by_uid(u)
                .filter(|e| state.world.has::<Preview>(*e))
        });
        let e = match entity {
            Some(e) => e,
            None => match spawn_preview(&mut state.world, &self.config, self.template.as_deref()) {
                Some((e, uid)) => {
                    self.preview = Some(uid);
                    self.written.clear();
                    e
                }
                None => return,
            },
        };
        if pts == self.written {
            return;
        }
        let closed = self.closing;
        let world = &mut state.world;
        let ok = world
            .field_path(&self.config.points_path())
            .ok()
            .is_some_and(|p| world.set_field(e, &p, &pts).is_ok());
        if let Closing::Field(f) = self.config.closing
            && let Ok(p) = world.field_path(&format!("{}.{f}", self.config.component))
        {
            let _ = world.set_field(e, &p, &closed);
        }
        if ok {
            self.written = pts;
        }
    }

    fn remove_preview(&mut self, state: &mut EditorState) {
        if let Some(uid) = self.preview.take()
            && let Some(e) = state.world.entity_by_uid(uid)
            && state.world.has::<Preview>(e)
        {
            state.world.despawn(e);
        }
        self.written.clear();
    }

    fn stop_drawing(&mut self, state: &mut EditorState) {
        self.points.clear();
        self.closing = false;
        self.snapper.reset();
        self.remove_preview(state);
    }

    /// Commits the drawn path as one undo step and selects it.
    fn commit(&mut self, state: &mut EditorState) -> Option<Uid> {
        let mut pts = self.points.clone();
        let closed = self.closing;
        if closed && self.config.closing == Closing::RepeatFirstPoint {
            pts.push(pts[0]);
        }
        if pts.len() < 2 {
            state.notify(&format!("{}: place at least two points", self.config.name));
            return None;
        }
        // The preview holds the component with the defaults applied.
        self.sync_preview(state);
        let preview = self
            .preview
            .and_then(|u| state.world.entity_by_uid(u))
            .filter(|e| state.world.has::<Preview>(*e));
        let origin = pts[0];
        let local: Vec<Vec3> = pts.iter().map(|p| *p - origin).collect();
        let value: Option<Value> = preview.and_then(|e| {
            let world = &mut state.world;
            let points = world.field_path(&self.config.points_path()).ok()?;
            world.set_field(e, &points, &local).ok()?;
            if let Closing::Field(f) = self.config.closing {
                let p = world
                    .field_path(&format!("{}.{f}", self.config.component))
                    .ok()?;
                world.set_field(e, &p, &closed).ok()?;
            }
            let whole = world.field_path(self.config.component).ok()?;
            world.get_field(e, &whole).ok()
        });
        self.stop_drawing(state);
        let value = value?;
        let label = format!("{} ({} points)", self.config.name, local.len());
        let name = self.config.entity_name;
        let component = self.config.component;
        let uid = state.transaction(label, |s| {
            s.spawn_path(
                name,
                Transform::from_translation(origin),
                &[(component, value.as_ref())],
            )
        })?;
        state.select(Some(uid));
        Some(uid)
    }

    /// The path the handles edit: the primary selection when it has the
    /// tool's component (uid, entity, world points).
    fn editing(&self, state: &EditorState) -> Option<(Uid, Entity, Vec<Vec3>)> {
        let e = state.primary_entity()?;
        let uid = state.world.uid(e)?;
        let pts = read_points(&state.world, e, self.config.component, self.config.points)?;
        let g = global(&state.world, e);
        Some((uid, e, pts.iter().map(|p| g.transform_point3(*p)).collect()))
    }

    fn handle_at(&self, state: &EditorState, cursor: Vec2) -> Option<usize> {
        let (_, e, pts) = self.editing(state)?;
        if state.blocked_in_viewport(e) {
            return None;
        }
        let view = state.gizmo_view()?;
        pts.iter()
            .enumerate()
            .filter_map(|(i, p)| view.project(*p).map(|s| (i, s.distance(cursor))))
            .filter(|(_, d)| *d <= HIT)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    fn press(&mut self, state: &mut EditorState, e: &ToolEvent) {
        if self.is_drawing() {
            return;
        }
        let Some(index) = self.handle_at(state, e.cursor) else {
            return;
        };
        let (Some((uid, _, pts)), Some(view)) = (self.editing(state), state.gizmo_view()) else {
            return;
        };
        let anchor = pts[index];
        self.drag = Some(PointDrag {
            uid,
            index,
            anchor,
            anchor_hit: crate::spline_tool::constrained(&e.ray, anchor, false, &view),
            moved: false,
            end: index == 0 || index + 1 == pts.len(),
            snapper: Snapper::new(),
        });
    }

    fn drag_to(&mut self, state: &mut EditorState, e: &ToolEvent) {
        let Some(mut d) = self.drag else { return };
        let Some(view) = state.gizmo_view() else {
            return;
        };
        let Some(entity) = state.world.entity_by_uid(d.uid) else {
            self.drag = None;
            return;
        };
        if d.anchor_hit.is_none() {
            d.anchor_hit = crate::spline_tool::constrained(&e.ray, d.anchor, false, &view);
        }
        let (Some(start), Some(now)) = (
            d.anchor_hit,
            crate::spline_tool::constrained(&e.ray, d.anchor, false, &view),
        ) else {
            self.drag = Some(d);
            return;
        };
        let raw = d.anchor + (now - start);
        let world_point = if d.end {
            let mut snapper = d.snapper;
            let (p, hit) = snapper.snap_to(raw, self.targets(state, Some(d.uid)));
            d.snapper = snapper;
            self.snap = hit;
            p
        } else {
            raw
        };
        self.cursor = Some(world_point);
        let local = global(&state.world, entity)
            .inverse()
            .transform_point3(world_point);
        let current = read_points(
            &state.world,
            entity,
            self.config.component,
            self.config.points,
        )
        .and_then(|p| p.get(d.index).copied());
        if current.is_some_and(|c| c != local) && local.is_finite() {
            if !d.moved {
                state.begin_edit(format!(
                    "Move {} Point {}",
                    self.config.entity_name, d.index
                ));
                d.moved = true;
            }
            let path = format!("{}[{}]", self.config.points_path(), d.index);
            state.set_field(d.uid, &path, &local);
        }
        self.drag = Some(d);
    }

    fn release(&mut self, state: &mut EditorState) {
        if let Some(d) = self.drag.take()
            && d.moved
        {
            state.finish_edit();
        }
        self.snap = None;
    }

    fn click(&mut self, state: &mut EditorState, e: &ToolEvent) {
        // A press on a handle without moving is a click: it starts (or
        // continues) a path there, snapped to that point.
        if let Some(d) = self.drag.take()
            && d.moved
        {
            state.finish_edit();
            return;
        }
        self.update_cursor(state, e);
        if self.closing {
            self.commit(state);
            return;
        }
        // A click on the last point (a double-click) finishes.
        if self.points.len() >= 2
            && let Some(view) = state.gizmo_view()
            && view
                .project(*self.points.last().expect("not empty"))
                .is_some_and(|s| s.distance(e.cursor) <= HIT)
        {
            self.commit(state);
            return;
        }
        let Some(p) = self.cursor else {
            state.notify("Point at a surface or the ground to place a point");
            return;
        };
        if self.points.is_empty() {
            self.take_template(state);
            if self.config.copy_selected && self.template.is_none() {
                state.notify(&format!(
                    "{}: select a path first; new ones copy its settings",
                    self.config.name
                ));
                return;
            }
        }
        if self
            .points
            .last()
            .is_some_and(|l| l.distance(p) <= SAME_POINT)
        {
            return;
        }
        self.points.push(p);
        // A new point starts a new snap gesture.
        self.snapper.reset();
        self.sync_preview(state);
    }
}

/// Spawns the runtime-only preview entity: a `Preview` with the
/// component's default value and the configured defaults, or a copy of
/// `template`.
fn spawn_preview(
    world: &mut World,
    config: &PathToolConfig,
    template: Option<&dyn PartialReflect>,
) -> Option<(Entity, Uid)> {
    let start = match template {
        Some(t) => box_core::reflect::clone_value(t).ok(),
        None => world
            .registry()
            .component(config.component)
            .and_then(|c| c.default_value()),
    };
    let Some(default) = start else {
        tracing::warn!(
            "{}: `{}` is not a registered component with a default value",
            config.name,
            config.component
        );
        return None;
    };
    let uid = Uid::new();
    let e = world.spawn((
        uid,
        Name::new(format!("{} (drawing)", config.entity_name)),
        Transform::IDENTITY,
        Preview,
    ));
    if let Err(err) = world.insert_reflect(e, config.component, default.as_ref()) {
        tracing::warn!("{}: {err}", config.name);
        world.despawn(e);
        return None;
    }
    if template.is_some() {
        return Some((e, uid));
    }
    for (field, json) in &config.defaults {
        if let Err(err) = set_from_json(world, e, &format!("{}.{field}", config.component), json) {
            tracing::warn!("{}: default `{field}`: {err}", config.name);
        }
    }
    Some((e, uid))
}

fn set_from_json(
    world: &mut World,
    e: Entity,
    path: &str,
    json: &serde_json::Value,
) -> Result<(), String> {
    let path = world.field_path(path).map_err(|e| e.to_string())?;
    let current = world.get_field(e, &path).map_err(|e| e.to_string())?;
    let ty = current
        .get_represented_type_info()
        .ok_or("the field's type is not known")?
        .type_id();
    let value = world
        .registry()
        .value_from_json(ty, json)
        .map_err(|e| e.to_string())?;
    world
        .set_field(e, &path, value.as_ref())
        .map_err(|e| e.to_string())
}

impl EditorState {
    /// Spawns a saved root entity named `name` (numbered when taken) with
    /// `transform` and `components` through the edit stack (inside an open
    /// transaction it merges into that step). Returns its uid.
    pub(crate) fn spawn_path(
        &mut self,
        name: &str,
        transform: Transform,
        components: &[(&str, &dyn PartialReflect)],
    ) -> Option<Uid> {
        let mut all: Vec<(&str, &dyn PartialReflect)> = vec![("Transform", &transform)];
        all.extend_from_slice(components);
        let uid = Uid::new();
        let name = self.unique_name(name);
        let r = self
            .edits
            .spawn(&mut self.world, uid, &name, Placement::last(None), &all);
        self.report(&format!("add {name}"), r)?;
        self.changed(true);
        Some(uid)
    }
}

impl ViewportTool for PathTool {
    fn id(&self) -> &'static str {
        self.config.id
    }

    fn label(&self) -> &'static str {
        self.config.label
    }

    fn name(&self) -> &'static str {
        self.config.name
    }

    fn tooltip(&self) -> &'static str {
        self.config.tooltip
    }

    fn wants_input(&self, state: &EditorState) -> bool {
        !self.needs_template(state)
    }

    fn on_pointer(&mut self, state: &mut EditorState, e: &ToolEvent) -> ToolResponse {
        match e.kind {
            ToolEventKind::Hover => {
                if self.drag.is_some() && e.primary_down {
                    self.drag_to(state, e);
                } else {
                    self.update_cursor(state, e);
                    self.hover_handle = if self.is_drawing() {
                        None
                    } else {
                        self.handle_at(state, e.cursor)
                    };
                    if self.is_drawing() {
                        self.sync_preview(state);
                    }
                }
            }
            ToolEventKind::Press => self.press(state, e),
            ToolEventKind::Release => self.release(state),
            ToolEventKind::Click => self.click(state, e),
        }
        ToolResponse::Handled
    }

    fn on_key(
        &mut self,
        state: &mut EditorState,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        if !modifiers.is_none() || self.drag.is_some() {
            return false;
        }
        match key {
            egui::Key::Backspace | egui::Key::Delete if self.is_drawing() => {
                self.points.pop();
                self.closing = false;
                if self.points.is_empty() {
                    self.stop_drawing(state);
                } else {
                    self.sync_preview(state);
                }
                true
            }
            egui::Key::Enter if self.is_drawing() => {
                self.commit(state);
                true
            }
            _ => false,
        }
    }

    fn on_escape(&mut self, state: &mut EditorState) -> bool {
        if let Some(d) = self.drag.take() {
            if d.moved {
                state.rollback_edit();
            }
            return true;
        }
        if self.is_drawing() {
            self.stop_drawing(state);
            return true;
        }
        false
    }

    fn cancel(&mut self, state: &mut EditorState) {
        if let Some(d) = self.drag.take()
            && d.moved
        {
            state.rollback_edit();
        }
        self.stop_drawing(state);
        self.cursor = None;
        self.snap = None;
        self.hover_handle = None;
        self.targets = None;
    }

    fn draw_overlay(&self, state: &EditorState, dd: &mut DebugDraw, tokens: &Tokens) {
        let accent = crate::tools::color(tokens.accent);
        let line = |dd: &mut DebugDraw, pts: &[Vec3], c: Color| {
            for w in pts.windows(2) {
                dd.line_on(
                    LineLayer::DisplayOverlay,
                    w[0] + Vec3::Y * 0.01,
                    w[1] + Vec3::Y * 0.01,
                    c,
                );
            }
        };
        if self.is_drawing() {
            line(dd, &self.preview_points(), accent);
        } else if let Some((_, _, pts)) = self.editing(state) {
            line(dd, &pts, accent.with_alpha(0.7));
        }
    }

    fn paint(&self, state: &EditorState, painter: &egui::Painter, viewport: egui::Rect) {
        let Some(view) = state.gizmo_view() else {
            return;
        };
        let tk = crate::theme::tokens(painter.ctx());
        let at = |w: Vec3| view.project(w).map(|s| viewport.min + egui::vec2(s.x, s.y));
        let outline = egui::Stroke::new(1.5, egui::Color32::from_black_alpha(200));
        // Placed points; the first is larger while it can close the path.
        for (i, p) in self.points.iter().enumerate() {
            let Some(s) = at(*p) else { continue };
            let r = if i == 0 && self.closing { 7.0 } else { 5.0 };
            painter.circle_filled(s, r, tk.accent);
            painter.circle_stroke(s, r, outline);
        }
        // Handles of the selected path.
        if !self.is_drawing()
            && let Some((_, _, pts)) = self.editing(state)
        {
            let dragged = self.drag.map(|d| d.index);
            for (i, p) in pts.iter().enumerate() {
                let Some(s) = at(*p) else { continue };
                let lit = self.hover_handle == Some(i) || dragged == Some(i);
                let r = if lit { 6.5 } else { 5.5 };
                let fill = if dragged == Some(i) {
                    tk.accent
                } else {
                    egui::Color32::WHITE
                };
                painter.circle_filled(s, r, fill);
                painter.circle_stroke(s, r, outline);
            }
        }
        // The cursor point and the snap indicator.
        let Some(c) = self.cursor.and_then(at) else {
            return;
        };
        match self.snap {
            Some(hit) => {
                let label = match hit {
                    SnapHit::Point(_) if self.closing => "close",
                    SnapHit::Point(_) => "end",
                    SnapHit::Segment { .. } => "body",
                };
                painter.circle_stroke(c, 9.0, egui::Stroke::new(2.5, tk.accent));
                painter.circle_filled(c, 3.5, tk.accent);
                let g = painter.layout_no_wrap(
                    format!("snap: {label}"),
                    egui::FontId::proportional(11.0),
                    tk.overlay_text,
                );
                let r = egui::Rect::from_min_size(
                    c + egui::vec2(12.0, -10.0 - g.size().y),
                    g.size() + egui::vec2(8.0, 4.0),
                );
                painter.rect_filled(r, 3.0, tk.overlay_bg);
                painter.galley(r.min + egui::vec2(4.0, 2.0), g, tk.overlay_text);
            }
            None => {
                painter.circle_stroke(c, 5.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
                painter.circle_stroke(c, 6.5, outline);
            }
        }
    }

    fn hint(&self, state: &EditorState) -> Option<String> {
        let name = self.config.name;
        Some(if self.is_drawing() {
            let n = self.points.len();
            let close = if self.config.closing != Closing::Open && n >= 3 {
                "  ·  click the first point to close"
            } else {
                ""
            };
            format!(
                "{name}  ·  {n} point{}  ·  click to add  ·  Enter or click the last point: done{close}  ·  Backspace: remove last  ·  Esc: cancel",
                if n == 1 { "" } else { "s" }
            )
        } else if self.editing(state).is_some() {
            format!("{name}  ·  click to start a new one  ·  drag a point to move it  ·  Esc: Move")
        } else if self.needs_template(state) {
            format!(
                "{name}  ·  select a path to draw more like it (new ones copy its settings)  ·  Esc: Move"
            )
        } else {
            format!("{name}  ·  click to place the first point  ·  Esc: Move")
        })
    }
}
