//! Editor state shared by the panes, and the editing operations they call.
//! Panes are plain functions of `(&mut Ui, &mut EditorState)`; every change to
//! the edit world goes through [`EditorState::edits`] (the `EditStack`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use box_core::edit::{EditError, EditStack};
use box_core::identity::{Name, Uid};
use box_core::math::Aabb;
use box_core::mesh::MeshData;
use box_core::prelude::{Assets, Entity, GlobalTransform, Primitive, Shape, Transform, World};
use box_core::reflect::{FieldPath, PartialReflect, Value, VariantDefaults};
use box_core::world::Placement;
use box_engine::Game;
use box_engine::control::ControlServer;
use box_render::MeshRenderer;
use glam::{Quat, Vec2, Vec3};

use crate::camera::EditorCamera;
use crate::gizmo::GizmoState;
use crate::log::{LogBuffer, LogRecord};
use crate::play::PlayMode;

/// The active transform tool. Set by the toolbar or W/E/R; used by the gizmo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransformTool {
    #[default]
    Translate,
    Rotate,
    Scale,
}

impl TransformTool {
    pub const ALL: [TransformTool; 3] = [Self::Translate, Self::Rotate, Self::Scale];

    pub fn label(self) -> &'static str {
        match self {
            Self::Translate => "Move",
            Self::Rotate => "Rotate",
            Self::Scale => "Scale",
        }
    }
}

/// Gizmo orientation: the selection's own axes or the world axes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Space {
    #[default]
    World,
    Local,
}

/// Where the gizmo of a multi-selection sits and what rotate and scale
/// turn about: the primary selection's origin, or the mean of the origins of
/// the entities a drag transforms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pivot {
    #[default]
    Primary,
    Centre,
}

/// How a selection gesture (click, marquee, hierarchy click) combines with
/// the current selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectMode {
    /// The hits become the selection.
    #[default]
    Replace,
    /// The hits are added (Shift).
    Add,
    /// Each hit is added, or removed if it was selected (Cmd, Ctrl elsewhere).
    Toggle,
}

impl SelectMode {
    /// Cmd (Ctrl on other platforms) toggles, Shift adds, otherwise replace.
    pub fn from_modifiers(m: &egui::Modifiers) -> Self {
        if m.command {
            Self::Toggle
        } else if m.shift {
            Self::Add
        } else {
            Self::Replace
        }
    }

    /// `base` combined with `hits` (in order). The last entry is the primary
    /// selection: the last new hit, or for Replace the last hit.
    pub fn combine(self, base: &[Uid], hits: &[Uid]) -> Vec<Uid> {
        match self {
            Self::Replace => hits.to_vec(),
            Self::Add => {
                let mut out = base.to_vec();
                out.extend(hits.iter().filter(|h| !base.contains(h)));
                out
            }
            Self::Toggle => {
                let mut out: Vec<Uid> =
                    base.iter().filter(|b| !hits.contains(b)).copied().collect();
                out.extend(hits.iter().filter(|h| !base.contains(h)));
                out
            }
        }
    }
}

/// Snapping settings, applied by the gizmo while `enabled`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub enabled: bool,
    /// Metres.
    pub translate: f32,
    /// Degrees.
    pub rotate: f32,
    /// Scale factor step.
    pub scale: f32,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            enabled: false,
            translate: 0.5,
            rotate: 15.0,
            scale: 0.1,
        }
    }
}

impl Snap {
    /// The settings a gizmo drag uses: with `invert` (Cmd, Ctrl elsewhere,
    /// held during the drag) snapping is temporarily the opposite of the
    /// toggle.
    pub fn effective(self, invert: bool) -> Self {
        Self {
            enabled: self.enabled != invert,
            ..self
        }
    }
}

/// Viewport pane bookkeeping between the UI pass and the render pass.
#[derive(Clone, Debug, Default)]
pub struct ViewportState {
    /// The size the pane wants, in physical pixels (panel rect x
    /// pixels-per-point). The paint path resizes the render target to it.
    pub request_px: [u32; 2],
    /// The registered egui texture showing the render target.
    pub texture: Option<egui::TextureId>,
    /// Size of the target currently shown.
    pub target_px: [u32; 2],
    /// Pointer over the viewport this frame (camera input and F).
    pub hovered: bool,
    /// The viewport pane was drawn this frame. Cleared by the paint path,
    /// which skips the scene render while the pane is hidden (inactive tab).
    pub shown: bool,
    /// Viewport rectangle in points, for gizmo hit testing.
    pub rect: Option<egui::Rect>,
    /// A marquee (rectangle) selection being dragged.
    pub marquee: Option<crate::marquee::Marquee>,
    /// View menu toggles: icons, collider wireframes.
    pub view: crate::icons::ViewOptions,
    /// Icons placed this frame, the hovered one, the fly speed pill.
    pub overlay: crate::icons::Overlay,
    /// The particle preview's bookkeeping (`particle_preview.rs`).
    pub(crate) particles: crate::particle_preview::PreviewClock,
}

/// Cached flatten of the hierarchy for the hierarchy pane.
#[derive(Clone, Debug)]
pub struct HierarchyState {
    pub expanded: HashSet<Uid>,
    pub rows: Vec<HierarchyRow>,
    /// Rebuild `rows` on the next frame. Set after structural edits.
    pub dirty: bool,
    /// Case-insensitive name filter.
    pub filter: String,
    pub(crate) built_for_count: u32,
    /// Saved, editable entities (not generated), counted when rows are built.
    pub(crate) listed_count: usize,
    /// Expand every entity with children the first time rows are built.
    pub(crate) expand_all_once: bool,
    /// Scroll so this row is visible on the next frame.
    pub(crate) scroll_to: Option<Uid>,
    /// Inline rename in progress: entity and text buffer.
    pub(crate) renaming: Option<(Uid, String)>,
    /// Row being dragged for reparenting.
    pub(crate) dragging: Option<Uid>,
    /// Where a Shift-click range starts: the row last clicked without Shift.
    pub(crate) anchor: Option<Uid>,
}

impl Default for HierarchyState {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            rows: Vec::new(),
            dirty: true,
            filter: String::new(),
            built_for_count: 0,
            listed_count: 0,
            expand_all_once: true,
            scroll_to: None,
            renaming: None,
            dragging: None,
            anchor: None,
        }
    }
}

impl HierarchyState {
    /// The entity being renamed inline, if any.
    pub fn renaming_uid(&self) -> Option<Uid> {
        self.renaming.as_ref().map(|(u, _)| *u)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HierarchyRow {
    pub entity: Entity,
    pub uid: Uid,
    pub depth: u16,
    pub children: u32,
    /// Shown only as an ancestor of a filter match.
    pub context_only: bool,
}

/// Console pane filter state.
#[derive(Clone, Debug)]
pub struct ConsoleState {
    pub log: LogBuffer,
    /// Shown severities: info, warnings, errors.
    pub show: [bool; 3],
    pub filter: String,
    /// Sequence numbers of the records that pass the filters.
    pub(crate) rows: Vec<u64>,
    pub(crate) built_generation: Option<u64>,
    pub(crate) dirty: bool,
    /// Copies of the records in the visible row range, so rows are painted
    /// without holding the log lock. Refreshed when the range or `rows` change.
    pub(crate) visible: Vec<Option<LogRecord>>,
    pub(crate) visible_range: Option<std::ops::Range<usize>>,
    /// Warnings and errors logged since the Console was last shown (the
    /// badge on its tab).
    pub(crate) unseen: [usize; 2],
    /// Selected records (sequence numbers), for copying.
    pub(crate) selected: BTreeSet<u64>,
    /// Where a Shift-click range starts.
    pub(crate) anchor: Option<u64>,
    /// Scroll to this record on the next frame.
    pub(crate) scroll_to: Option<u64>,
    /// The Console was clicked last, so Cmd+C copies its selected rows.
    pub(crate) focused: bool,
}

impl ConsoleState {
    pub fn new(log: LogBuffer) -> Self {
        Self {
            log,
            show: [true; 3],
            filter: String::new(),
            rows: Vec::new(),
            built_generation: None,
            dirty: true,
            visible: Vec::new(),
            visible_range: None,
            unseen: [0; 2],
            selected: BTreeSet::new(),
            anchor: None,
            scroll_to: None,
            focused: false,
        }
    }

    /// Warnings and errors logged since the Console was last shown.
    pub fn unseen(&self) -> [usize; 2] {
        self.unseen
    }

    /// The selected records' sequence numbers.
    pub fn selected(&self) -> &BTreeSet<u64> {
        &self.selected
    }

    /// Selects record `seq` alone and scrolls to it on the next frame,
    /// turning off the filters that would hide it.
    pub fn reveal(&mut self, seq: u64) {
        if let Some(r) = self.log.lock().get(seq) {
            let sev = crate::log::Severity::of(r.level) as usize;
            let f = self.filter.to_lowercase();
            if !self.show[sev]
                || !(f.is_empty()
                    || r.message.to_lowercase().contains(&f)
                    || r.target.to_lowercase().contains(&f))
            {
                self.show[sev] = true;
                self.filter.clear();
                self.dirty = true;
            }
        }
        self.selected = BTreeSet::from([seq]);
        self.anchor = Some(seq);
        self.scroll_to = Some(seq);
        self.focused = true;
    }
}

/// Numbers for the toolbar status area.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    pub fps: f32,
    pub draws: u32,
    pub triangles: u64,
}

/// The open scene document.
#[derive(Clone, Debug, Default)]
pub struct SceneDoc {
    /// Display name (file stem), e.g. `main`.
    pub name: String,
    /// Where Save writes; `None` for the unsaved scratch scene.
    pub path: Option<PathBuf>,
}

/// Which state of the undo history the world is in, so that undoing back to
/// the saved state is clean while a diverged history of the same depth is
/// not. Every committed undo step gets an id for the state it leads to;
/// `EditStack` only exposes its depth, so the ids are reconciled with it
/// before every undo, redo and save (new steps only ever appear on top).
#[derive(Clone, Debug)]
pub(crate) struct History {
    /// State after undo step `i + 1`. Entries past `cursor` are redoable.
    ids: Vec<u64>,
    /// The undo depth `ids` was last reconciled at.
    cursor: usize,
    /// State with nothing to undo.
    base: u64,
    next: u64,
    /// The state in the scene file; `None` when no reachable state is.
    saved: Option<u64>,
    /// The selection last seen in each state, restored when undo or redo
    /// returns to it (`EditorState::remember_selection`).
    selections: HashMap<u64, Vec<Uid>>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            ids: Vec::new(),
            cursor: 0,
            base: 0,
            next: 0,
            saved: Some(0),
            selections: HashMap::new(),
        }
    }
}

impl History {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// A new world with an empty undo stack; it is the saved state.
    fn reset(&mut self) {
        self.selections.clear();
        self.ids.clear();
        self.cursor = 0;
        self.base = self.fresh();
        self.saved = Some(self.base);
    }

    /// Accounts for steps recorded since the last call: they replace what
    /// was redoable from `cursor` up.
    fn sync(&mut self, depth: usize) {
        if depth > self.cursor {
            for gone in self.ids.drain(self.cursor.min(self.ids.len())..) {
                self.selections.remove(&gone);
            }
            while self.ids.len() < depth {
                let id = self.fresh();
                self.ids.push(id);
            }
        }
        self.cursor = depth;
    }

    /// After an undo or redo moved the stack to `depth` (call `sync` before).
    fn moved(&mut self, depth: usize) {
        debug_assert!(depth <= self.ids.len());
        self.cursor = depth.min(self.ids.len());
    }

    /// The world no longer matches any known state (a failed undo or redo).
    fn lost(&mut self, depth: usize) {
        self.selections.clear();
        self.ids.clear();
        self.cursor = 0;
        self.base = self.fresh();
        self.sync(depth);
        self.saved = None;
    }

    /// The state at `depth`; `None` while steps are not reconciled yet (they
    /// are new, so not the saved state).
    fn current(&self, depth: usize) -> Option<u64> {
        match depth {
            d if d > self.cursor => None,
            0 => Some(self.base),
            d => self.ids.get(d - 1).copied(),
        }
    }

    /// Records `selection` as the one seen in the state at `depth`.
    fn remember_selection(&mut self, depth: usize, selection: &[Uid]) {
        self.sync(depth);
        if let Some(id) = self.current(depth) {
            self.selections.insert(id, selection.to_vec());
        }
    }

    /// The selection last seen in the state at `depth`, if any.
    fn selection_at(&self, depth: usize) -> Option<&[Uid]> {
        self.current(depth)
            .and_then(|id| self.selections.get(&id))
            .map(Vec::as_slice)
    }

    pub(crate) fn is_saved(&self, depth: usize) -> bool {
        matches!((self.current(depth), self.saved), (Some(a), Some(b)) if a == b)
    }
}

/// Bookkeeping for the recovery copy written while editing.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RecoveryState {
    /// [`EditorState::generation`] the recovery file (or its absence) is
    /// current for. Equal to the generation while idle.
    pub generation: u64,
    /// UI time of the last periodic write, in seconds.
    pub written_at: Option<f64>,
    /// A recovery file for the current scene was written this session.
    pub written: bool,
}

/// One-shot requests from menus and shortcuts, handled by the shell after the
/// UI pass (they need the dock tree, the project or the window).
#[derive(Clone, Debug, Default)]
pub(crate) struct Requests {
    pub save_layout: bool,
    pub reset_layout: bool,
    /// Quit (menu); asks about unsaved changes first.
    pub quit: bool,
    /// Save Scene; a scene without a file goes to Save Scene As.
    pub save_scene: bool,
    pub save_scene_as: bool,
    pub new_project: bool,
    pub open_project: bool,
    /// File ▸ Open Example… (`Some(None)`) or an example of the Welcome
    /// card (`Some(Some(name))`): copy an example project and open the copy.
    pub open_example: Option<Option<String>>,
    /// Open this scene of the current project.
    pub open_scene: Option<PathBuf>,
    /// Bring the Console tab to the front, selecting this record if any
    /// (a toast was clicked).
    pub focus_console: Option<Option<u64>>,
    /// File ▸ New Scene; asks about unsaved changes first.
    pub new_scene: bool,
    /// Help ▸ Keyboard Shortcuts.
    pub show_shortcuts: bool,
    /// Delete in the asset browser: confirm, then move this file to the
    /// project's trash.
    pub delete_asset: Option<box_core::asset_ref::AssetId>,
    /// Bring this pane's tab to the front.
    pub focus_pane: Option<crate::layout::Pane>,
    /// File ▸ Open Recent (or the Welcome card): open this project folder;
    /// asks about unsaved changes first.
    pub open_recent: Option<PathBuf>,
    /// Game ▸ Rebuild and Restart.
    pub rebuild: bool,
}

/// What [`EditorState::add_entity`] creates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewEntity {
    Cube,
    Sphere,
    Cylinder,
    Capsule,
    Plane,
    Empty,
    DirectionalLight,
    PointLight,
    SpotLight,
    AmbientVolume,
    Camera,
    ParticleEmitter,
}

impl NewEntity {
    pub const ALL: [NewEntity; 12] = [
        Self::Cube,
        Self::Sphere,
        Self::Cylinder,
        Self::Capsule,
        Self::Plane,
        Self::Empty,
        Self::DirectionalLight,
        Self::PointLight,
        Self::SpotLight,
        Self::AmbientVolume,
        Self::Camera,
        Self::ParticleEmitter,
    ];

    /// Menus put a separator before this kind (meshes, empty, lights and
    /// lighting, then camera and effects).
    pub fn starts_group(self) -> bool {
        matches!(self, Self::Empty | Self::DirectionalLight | Self::Camera)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cube => "Cube",
            Self::Sphere => "Sphere",
            Self::Cylinder => "Cylinder",
            Self::Capsule => "Capsule",
            Self::Plane => "Plane",
            Self::Empty => "Empty",
            Self::DirectionalLight => "Directional Light",
            Self::PointLight => "Point Light",
            Self::SpotLight => "Spot Light",
            Self::AmbientVolume => "Ambient Volume",
            Self::Camera => "Camera",
            Self::ParticleEmitter => "Particle Emitter",
        }
    }

    /// The registered component this needs beyond core types, if any.
    pub fn required_component(self) -> Option<&'static str> {
        match self {
            Self::DirectionalLight => Some("DirectionalLight"),
            Self::PointLight => Some("PointLight"),
            Self::SpotLight => Some("SpotLight"),
            Self::AmbientVolume => Some("AmbientVolume"),
            Self::Camera => Some("Camera"),
            Self::ParticleEmitter => Some("ParticleEmitter"),
            _ => None,
        }
    }

    /// The shape it adds: the variant default of `Shape` (also what a
    /// scene or command gets for a variant given without its fields).
    fn shape(self) -> Option<Shape> {
        Shape::variant_default(match self {
            Self::Cube => "Cuboid",
            Self::Sphere => "Sphere",
            Self::Cylinder => "Cylinder",
            Self::Capsule => "Capsule",
            Self::Plane => "Plane",
            _ => return None,
        })
    }

    /// Half the size of the new object's bounds (zero without a mesh).
    fn half_extents(self) -> Vec3 {
        match self.shape() {
            Some(Shape::Cuboid { size }) => size / 2.0,
            Some(Shape::Sphere { radius, .. }) => Vec3::splat(radius),
            Some(Shape::Cylinder { radius, height, .. }) => Vec3::new(radius, height / 2.0, radius),
            Some(Shape::Capsule {
                radius,
                half_height,
                ..
            }) => Vec3::new(radius, half_height + radius, radius),
            Some(Shape::Plane { size, .. }) => Vec3::new(size.x / 2.0, 0.0, size.y / 2.0),
            _ => Vec3::ZERO,
        }
    }

    /// How far above the surface under the viewport centre its origin goes:
    /// meshes rest on it, a point light hangs above it, a spot light shines
    /// down on it from 3 m, an ambient volume (a 4 × 3 × 4 m box by
    /// default) stands on it and a particle emitter sits on it.
    fn lift(self) -> f32 {
        match self {
            Self::PointLight => 2.5,
            Self::SpotLight => 3.0,
            Self::AmbientVolume => box_render::AmbientVolume::default().half_extents.y,
            // Above the floor it lands on, so the two do not z-fight.
            Self::Plane => 0.01,
            _ => self.half_extents().y,
        }
    }
}

/// Everything the panes read and write.
pub struct EditorState {
    /// The edit world. Read freely; change it only through [`Self::edits`]
    /// (via the methods below) so every change is undoable. During Play it
    /// is the play world (the edit world is parked until Stop; see
    /// [`Self::start_play`]), so every pane shows the running game.
    pub world: World,
    pub edits: EditStack,
    /// Selected entities by uid (survives undo, redo and respawn). The last
    /// one is the primary selection the inspector and gizmo use.
    pub selection: Vec<Uid>,
    pub tool: TransformTool,
    pub space: Space,
    /// Gizmo pivot for multi-selections.
    pub pivot: Pivot,
    pub snap: Snap,
    pub camera: EditorCamera,
    pub gizmo: GizmoState,
    pub show_grid: bool,
    pub viewport: ViewportState,
    pub hierarchy: HierarchyState,
    pub console: ConsoleState,
    pub status: Status,
    /// The open project; `None` for the scratch scene.
    pub project: Option<box_engine::Project>,
    pub scene: SceneDoc,
    /// Bumped by every change to the world (edit, undo, redo, load).
    pub generation: u64,
    pub(crate) history: History,
    /// The open transaction changed the world (it is not an undo step yet).
    pub(crate) open_dirty: bool,
    pub(crate) recovery: RecoveryState,
    /// Field path of the inspector edit whose transaction is open.
    pub(crate) inspector_edit: Option<String>,
    pub(crate) requests: Requests,
    /// The game: plugins and the registry every world of the editor uses.
    pub(crate) game: Game,
    /// Play mode (see `play.rs`); while set, `world` is the play world.
    pub(crate) play: Option<PlayMode>,
    /// Where scenes without a project keep their pre-Play autosave
    /// ([`crate::play::default_autosave_dir`] unless changed).
    pub autosave_dir: PathBuf,
    /// The agent control socket while Allow Agent Control is on (see
    /// `control.rs`).
    pub(crate) control: Option<ControlServer>,
    /// The asset browser (see `assets.rs`).
    pub assets: crate::assets::BrowserState,
    /// The open project's asset server, also a resource of the edit world.
    pub(crate) asset_server: Option<box_assets::AssetServer>,
    /// Toasts and the log reader feeding them and the Console badge (see
    /// `feedback.rs`).
    pub(crate) toasts: crate::feedback::Toasts,
    /// Entities whose mesh or prefab asset failed or is missing.
    pub(crate) asset_marks: crate::feedback::AssetMarks,
    /// Show the viewport's navigation hint (until dismissed; stored in
    /// `.box/editor.json`, see `prefs.rs`).
    pub nav_hint: bool,
    /// Menu and palette actions (see `actions.rs`).
    pub(crate) actions: crate::actions::ActionRegistry,
    /// The command palette while it is open (see `palette.rs`).
    pub(crate) palette: Option<crate::palette::PaletteState>,
    /// Recently opened projects (see `start.rs`); kept in the user config
    /// folder when the editor has one.
    pub recent: crate::start::RecentProjects,
    /// The project was opened read-only instead of upgraded (its files are
    /// from an older engine; see `box_engine::upgrade`): scenes, materials,
    /// settings and asset files are not written. Reopen it and choose
    /// Upgrade to edit it.
    pub read_only: bool,
    /// Show the Welcome card over the viewport (see `start.rs`).
    pub welcome: bool,
    /// The example projects of the Welcome card, found on first use
    /// (`box_engine::templates::examples`).
    pub(crate) examples: Option<Vec<box_engine::templates::Example>>,
    /// Restore each opened project's last session (scene, camera,
    /// selection) from `.box/editor.json` (see `prefs.rs`).
    pub restore_session: bool,
    /// Hidden and locked entities (editor-only; see `visibility.rs`).
    pub(crate) visibility: crate::visibility::Visibility,
    /// Copied values and components (see `values.rs`).
    pub(crate) clipboard: crate::values::Clipboard,
    /// Started by Rebuild and Restart (`--after-rebuild`): the start
    /// project's recovery file is restored without asking.
    pub after_rebuild: bool,
    /// Game ▸ Rebuild and Restart (see `rebuild.rs`).
    pub(crate) rebuild: crate::rebuild::Rebuild,
    /// Game ▸ Project Settings: the settings resource the inspector shows
    /// (see `project_settings.rs`).
    pub(crate) settings_view: Option<crate::project_settings::SettingsView>,
    /// The derived part shown read-only in the inspector (Cmd+Alt+click;
    /// see `generated.rs`). Cleared by any selection change.
    pub(crate) inspected_part: Option<Uid>,
    /// Owners of derived parts, for the hierarchy and viewport badges.
    pub(crate) generated: crate::generated::GeneratedMarks,
    /// Viewport tools and the active one (see `tools.rs`).
    pub(crate) tools: crate::tools::ToolRegistry,
    /// View ▸ Rules and the session's rule switches (see `rules_panel.rs`).
    pub(crate) rules_panel: crate::rules_panel::RulesPanel,
    /// File ▸ Export… (`export.rs`).
    pub(crate) export: crate::export::ExportPanel,
}

impl EditorState {
    /// An editor state for the engine alone ([`Game::new`]).
    pub fn new(world: World, log: LogBuffer) -> Self {
        Self::with_game(world, log, Game::new())
    }

    /// An editor state for `game`: its plugins run in Play, and scenes are
    /// loaded with its registry. `world` keeps the registry it was built with.
    pub fn with_game(world: World, log: LogBuffer, game: Game) -> Self {
        let mut actions = crate::actions::ActionRegistry::builtin();
        let settings: Vec<&'static str> = game.registry().settings().map(|t| t.name()).collect();
        for a in crate::actions::settings_actions(&settings) {
            actions.register(a);
        }
        let tools = crate::tools::ToolRegistry::builtin();
        for &info in tools.info() {
            actions.register(crate::tools::tool_action(info));
        }
        Self {
            world,
            edits: EditStack::new(),
            selection: Vec::new(),
            tool: TransformTool::default(),
            space: Space::default(),
            pivot: Pivot::default(),
            snap: Snap::default(),
            camera: EditorCamera::default(),
            gizmo: GizmoState::default(),
            show_grid: true,
            viewport: ViewportState::default(),
            hierarchy: HierarchyState::default(),
            console: ConsoleState::new(log),
            status: Status::default(),
            project: None,
            scene: SceneDoc {
                name: "scratch".into(),
                ..Default::default()
            },
            generation: 0,
            history: History::default(),
            open_dirty: false,
            recovery: RecoveryState::default(),
            inspector_edit: None,
            requests: Requests::default(),
            game,
            play: None,
            autosave_dir: crate::play::default_autosave_dir(),
            control: None,
            assets: Default::default(),
            asset_server: None,
            toasts: Default::default(),
            asset_marks: Default::default(),
            nav_hint: true,
            actions,
            palette: None,
            recent: crate::start::RecentProjects::default(),
            read_only: false,
            welcome: false,
            examples: None,
            restore_session: false,
            visibility: Default::default(),
            clipboard: Default::default(),
            after_rebuild: false,
            rebuild: Default::default(),
            settings_view: None,
            inspected_part: None,
            generated: Default::default(),
            tools,
            rules_panel: Default::default(),
            export: Default::default(),
        }
    }

    /// The game this editor runs in Play.
    pub fn game(&self) -> &Game {
        &self.game
    }

    /// True when the scene differs from what was last saved or loaded.
    /// Undoing (or redoing) back to the saved state is clean again. During
    /// Play this is the parked edit world's state, not the play world's.
    pub fn is_dirty(&self) -> bool {
        if let Some(mode) = &self.play {
            return mode.edit_dirty();
        }
        (self.open_dirty && self.edits.is_open()) || !self.history.is_saved(self.edits.undo_len())
    }

    /// Marks the current state as the one in the scene file.
    pub(crate) fn mark_saved(&mut self) {
        let depth = self.edits.undo_len();
        self.history.sync(depth);
        self.history.saved = self.history.current(depth);
        self.recovery.generation = self.generation;
    }

    /// Marks the world as unsaved whatever is undone (restored recovery copy).
    pub(crate) fn mark_unsaved(&mut self) {
        self.history.saved = None;
    }

    /// Replaces the edit world (scene load): clears undo history and selection.
    pub fn replace_world(&mut self, world: World) {
        self.stop_play();
        self.world = world;
        self.edits.clear();
        self.selection.clear();
        self.gizmo = GizmoState::default();
        self.viewport.marquee = None;
        self.hierarchy = HierarchyState::default();
        self.inspector_edit = None;
        self.inspected_part = None;
        self.cancel_tool();
        self.open_dirty = false;
        self.history.reset();
        self.reset_hidden_markers();
        self.generation += 1;
        self.recovery = RecoveryState {
            generation: self.generation,
            ..Default::default()
        };
        // One asset server serves one world (its values live there).
        self.attach_asset_server();
    }

    /// Undo steps of the edit world (the parked one during Play).
    pub fn edit_undo_len(&self) -> usize {
        self.play
            .as_ref()
            .map_or(self.edits.undo_len(), |m| m.edit_undo_len())
    }

    // ----- selection -----

    pub fn primary(&self) -> Option<Uid> {
        self.selection
            .last()
            .copied()
            .filter(|u| self.world.entity_by_uid(*u).is_some())
    }

    pub fn primary_entity(&self) -> Option<Entity> {
        self.primary().and_then(|u| self.world.entity_by_uid(u))
    }

    pub fn is_selected(&self, uid: Uid) -> bool {
        self.selection.contains(&uid)
    }

    /// Selects `uid` alone (or nothing) and reveals it in the hierarchy.
    pub fn select(&mut self, uid: Option<Uid>) {
        self.close_asset_inspector();
        self.selection.clear();
        if let Some(u) = uid {
            self.selection.push(u);
            self.reveal(u);
        }
    }

    /// Shows the scene's settings (its resources: sky, fog, exposure, tone
    /// mapping) in the inspector: selects nothing and closes an inspected
    /// asset. View ▸ Scene Settings and the hierarchy's Scene row.
    pub fn show_scene_settings(&mut self) {
        self.select(None);
    }

    /// The inspector shows the scene's settings (nothing selected, no asset
    /// inspected).
    pub fn scene_settings_shown(&self) -> bool {
        self.primary().is_none()
            && self.assets.inspect.is_none()
            && self.settings_view.is_none()
            && self.inspected_part().is_none()
    }

    /// Adds `uid` to the selection (as primary), or removes it if present.
    pub fn toggle_selected(&mut self, uid: Uid) {
        self.close_asset_inspector();
        if let Some(i) = self.selection.iter().position(|u| *u == uid) {
            self.selection.remove(i);
        } else {
            self.selection.push(uid);
            self.reveal(uid);
        }
    }

    /// Combines `hits` with the selection (see [`SelectMode::combine`]) and
    /// reveals the new primary selection. Replace with no hits clears it.
    pub fn select_with(&mut self, hits: &[Uid], mode: SelectMode) {
        self.close_asset_inspector();
        let before = self.primary();
        self.selection = mode.combine(&self.selection, hits);
        self.prune_selection();
        if let Some(p) = self.primary()
            && Some(p) != before
        {
            self.reveal(p);
        }
    }

    /// Expands the ancestors of `uid` and scrolls the hierarchy to it.
    pub fn reveal(&mut self, uid: Uid) {
        let Some(e) = self.world.entity_by_uid(uid) else {
            return;
        };
        let mut cur = self.world.parent(e);
        while let Some(p) = cur {
            if let Some(pu) = self.world.uid(p)
                && self.hierarchy.expanded.insert(pu)
            {
                self.hierarchy.dirty = true;
            }
            cur = self.world.parent(p);
        }
        self.hierarchy.scroll_to = Some(uid);
    }

    pub(crate) fn prune_selection(&mut self) {
        let world = &self.world;
        self.selection.retain(|u| world.entity_by_uid(*u).is_some());
    }

    /// The entity's display name (or `Entity <id>` without a `Name`).
    pub fn display_name(&self, e: Entity) -> String {
        match self.world.get::<Name>(e) {
            Some(n) if !n.as_str().is_empty() => n.as_str().to_owned(),
            _ => format!("Entity {}", e.id()),
        }
    }

    // ----- edits -----

    /// Bookkeeping after any change: bump the generation, refresh the
    /// hierarchy for structural changes and drop selected entities that no
    /// longer exist.
    pub fn changed(&mut self, structural: bool) {
        self.generation += 1;
        if self.edits.is_open() {
            self.open_dirty = true;
        }
        if structural {
            self.hierarchy.dirty = true;
        }
        self.prune_selection();
    }

    /// Logs a failed edit; returns the value on success.
    pub(crate) fn report<T>(&self, what: &str, result: Result<T, EditError>) -> Option<T> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("{what}: {e}");
                None
            }
        }
    }

    // ----- continuous edits (gizmo drags, inspector drags) -----

    /// True while a gizmo drag or a continuous inspector edit has a
    /// transaction open.
    pub fn edit_in_progress(&self) -> bool {
        self.gizmo.drag.is_some() || self.inspector_edit.is_some()
    }

    /// Opens a transaction, first ending whatever continuous edit is active.
    pub(crate) fn begin_edit(&mut self, label: impl Into<String>) {
        self.finish_edit();
        self.edits.begin(label);
    }

    /// Ends an active gizmo drag or continuous inspector edit as one undo
    /// step and closes the open transaction. Everything that closes the
    /// transaction (undo, redo, save, delete, ...) goes through here, so a
    /// drag never outlives it.
    pub fn finish_edit(&mut self) {
        // Every edit starts here (or in `begin_edit`, which calls this), so
        // the selection is remembered for the state before it.
        self.remember_selection();
        if self.gizmo.drag.take().is_some() {
            self.gizmo.await_release = true;
        }
        self.inspector_edit = None;
        self.edits.commit();
        self.open_dirty = false;
    }

    /// Cancels an active gizmo drag or continuous inspector edit, restoring
    /// the values from before it. Returns false when there was none.
    pub fn cancel_edit(&mut self) -> bool {
        if !self.edit_in_progress() {
            return false;
        }
        if self.gizmo.drag.take().is_some() {
            self.gizmo.await_release = true;
        }
        self.inspector_edit = None;
        self.open_dirty = false;
        let r = self.edits.rollback(&mut self.world);
        if self.report("cancel", r).is_none() {
            // Partly reverted: no known state.
            self.history.lost(self.edits.undo_len());
        }
        self.changed(true);
        true
    }

    /// Rolls back the open transaction (a viewport tool's drag that is being
    /// cancelled), restoring the values from before it. False when none was
    /// open.
    pub(crate) fn rollback_edit(&mut self) -> bool {
        if !self.edits.is_open() {
            return false;
        }
        self.open_dirty = false;
        let r = self.edits.rollback(&mut self.world);
        if self.report("cancel", r).is_none() {
            self.history.lost(self.edits.undo_len());
        }
        self.changed(true);
        true
    }

    /// Undo: the shown material file's last write while the inspector shows
    /// an editable material (`asset_undo.rs`), otherwise the last scene step.
    pub fn undo(&mut self) {
        if self.asset_undo_active() {
            self.step_asset_history(false);
        } else {
            self.step_history(false, true);
        }
    }

    /// Redo; see [`Self::undo`].
    pub fn redo(&mut self) {
        if self.asset_undo_active() {
            self.step_asset_history(true);
        } else {
            self.step_history(true, true);
        }
    }

    /// Undo and redo of the scene's history, whatever the inspector shows.
    pub fn undo_scene(&mut self) {
        self.step_history(false, true);
    }

    /// See [`Self::undo_scene`].
    pub fn redo_scene(&mut self) {
        self.step_history(true, true);
    }

    /// Remembers the selection for the current undo state, so undo and redo
    /// can bring it back (E20). One small copy per discrete edit.
    fn remember_selection(&mut self) {
        let depth = self.edits.undo_len();
        self.history.remember_selection(depth, &self.selection);
    }

    /// One undo or redo step; `notify` shows a note for it. Returns false
    /// when there was nothing to do or the step failed.
    pub(crate) fn step_history(&mut self, forward: bool, notify: bool) -> bool {
        self.finish_edit();
        self.history.sync(self.edits.undo_len());
        let (what, label, r) = if forward {
            let label = self.edits.redo_label().map(str::to_owned);
            ("redo", label, self.edits.redo(&mut self.world))
        } else {
            let label = self.edits.undo_label().map(str::to_owned);
            ("undo", label, self.edits.undo(&mut self.world))
        };
        match r {
            Ok(true) => {
                let depth = self.edits.undo_len();
                self.history.moved(depth);
                let label = label.unwrap_or_default();
                tracing::info!("{what} {label}");
                self.changed(true);
                // Back to the selection that state had, where it still exists.
                if let Some(sel) = self.history.selection_at(depth) {
                    let world = &self.world;
                    let sel: Vec<Uid> = sel
                        .iter()
                        .copied()
                        .filter(|u| world.entity_by_uid(*u).is_some())
                        .collect();
                    self.close_asset_inspector();
                    self.selection = sel;
                    if let Some(p) = self.primary() {
                        self.reveal(p);
                    }
                }
                if notify {
                    let verb = if forward { "Redo" } else { "Undo" };
                    self.notify(&format!("{verb}: {label}"));
                }
                true
            }
            Ok(false) => false,
            Err(e) => {
                tracing::warn!("{what}: {e}");
                // Partly applied: no known state.
                self.history.lost(self.edits.undo_len());
                self.changed(true);
                false
            }
        }
    }

    /// Sets a component field on `uid` (`path` like `"Transform.translation"`).
    /// Inside an open transaction successive sets merge into one step.
    pub fn set_field(&mut self, uid: Uid, path: &str, value: &dyn PartialReflect) -> bool {
        let r = self
            .world
            .field_path(path)
            .map_err(EditError::from)
            .and_then(|p| self.edits.set_field(&mut self.world, uid, &p, value));
        let ok = self.report(&format!("set {path}"), r).is_some();
        if ok {
            self.changed(path.starts_with("Name"));
        }
        ok
    }

    /// Sets a component field on every entity in `uids` (the inspector's
    /// multi-selection, primary last). With `lanes` (a changed-lane mask and
    /// the edited vector), entities other than the primary keep their own
    /// values in the lanes that were not edited, so editing X of three
    /// different positions moves them to the same X only. Inside an open
    /// transaction successive sets merge into one edit per entity.
    pub fn set_field_on(
        &mut self,
        uids: &[Uid],
        path: &str,
        value: &dyn PartialReflect,
        lanes: Option<(u8, &[f32])>,
    ) -> bool {
        let field = match self.world.field_path(path) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("set {path}: {e}");
                return false;
            }
        };
        let primary = uids.last().copied();
        let mut values: Vec<(Uid, Option<Value>)> = Vec::with_capacity(uids.len());
        let mut ok = true;
        for &uid in uids {
            let own = match lanes {
                Some((mask, new)) if Some(uid) != primary => {
                    let current = self
                        .world
                        .entity_by_uid(uid)
                        .and_then(|e| self.world.get_field(e, &field).ok());
                    match current.and_then(|c| crate::inspect::with_lanes(c.as_ref(), mask, new)) {
                        Some(v) => Some(v),
                        None => {
                            tracing::warn!("set {path}: not a vector field");
                            ok = false;
                            continue;
                        }
                    }
                }
                _ => None,
            };
            values.push((uid, own));
        }
        let sets = values
            .iter()
            .map(|(u, v)| (*u, v.as_deref().unwrap_or(value)));
        self.set_fields(&field, sets) && ok
    }

    /// Sets the field `path` on several entities, each to its own value,
    /// with one round of bookkeeping ([`Self::changed`]) for all of them, so
    /// a drag over many entities costs one pass per frame. Returns false if
    /// any set failed (each failure is logged).
    pub(crate) fn set_fields<'v>(
        &mut self,
        path: &FieldPath,
        sets: impl IntoIterator<Item = (Uid, &'v dyn PartialReflect)>,
    ) -> bool {
        let mut ok = true;
        let mut any = false;
        for (uid, value) in sets {
            match self.edits.set_field(&mut self.world, uid, path, value) {
                Ok(()) => any = true,
                Err(e) => {
                    tracing::warn!("set {path}: {e}");
                    ok = false;
                }
            }
        }
        if any {
            self.changed(path.target() == "Name");
        }
        ok
    }

    /// Sets a resource field (`"Environment.exposure"`).
    pub fn set_resource_field(&mut self, path: &str, value: &dyn PartialReflect) -> bool {
        let r = self
            .world
            .field_path(path)
            .map_err(EditError::from)
            .and_then(|p| self.edits.set_resource_field(&mut self.world, &p, value));
        let ok = self.report(&format!("set {path}"), r).is_some();
        if ok {
            self.changed(false);
        }
        ok
    }

    /// Renames `uid`. The same name again records nothing.
    pub fn rename(&mut self, uid: Uid, name: &str) {
        let current = self
            .world
            .entity_by_uid(uid)
            .and_then(|e| self.world.get::<Name>(e).map(|n| n.0 == name));
        // An entity without a `Name` shows as unnamed: "" is no change.
        if current == Some(true) || (current.is_none() && name.is_empty()) {
            return;
        }
        self.finish_edit();
        let r = self.edits.rename(&mut self.world, uid, name);
        if self.report("rename", r).is_some() {
            self.changed(true);
        }
    }

    pub fn add_component(&mut self, uid: Uid, component: &str) {
        self.finish_edit();
        let r = self
            .edits
            .add_component(&mut self.world, uid, component, None);
        if self.report(&format!("add {component}"), r).is_some() {
            self.changed(true);
        }
    }

    pub fn remove_component(&mut self, uid: Uid, component: &str) {
        self.finish_edit();
        let r = self.edits.remove_component(&mut self.world, uid, component);
        if self.report(&format!("remove {component}"), r).is_some() {
            self.changed(true);
        }
    }

    /// Adds `component` (its default value) to every entity in `uids` that
    /// lacks it, as one undo step.
    pub fn add_component_to(&mut self, uids: &[Uid], component: &str) {
        if let [uid] = uids {
            return self.add_component(*uid, component);
        }
        self.begin_edit(format!("Add {component} to {} entities", uids.len()));
        for &uid in uids {
            let has = self
                .world
                .entity_by_uid(uid)
                .is_some_and(|e| self.world.component_names(e).contains(&component));
            if !has {
                let r = self
                    .edits
                    .add_component(&mut self.world, uid, component, None);
                self.report(&format!("add {component}"), r);
            }
        }
        self.finish_edit();
        self.changed(true);
    }

    /// Removes `component` from every entity in `uids` that has it, as one
    /// undo step.
    pub fn remove_component_from(&mut self, uids: &[Uid], component: &str) {
        if let [uid] = uids {
            return self.remove_component(*uid, component);
        }
        self.begin_edit(format!("Remove {component} from {} entities", uids.len()));
        for &uid in uids {
            let has = self
                .world
                .entity_by_uid(uid)
                .is_some_and(|e| self.world.component_names(e).contains(&component));
            if has {
                let r = self.edits.remove_component(&mut self.world, uid, component);
                self.report(&format!("remove {component}"), r);
            }
        }
        self.finish_edit();
        self.changed(true);
    }

    /// Moves `uid` under `parent` (or to the roots) at sibling `index`.
    /// Cycles are rejected with a warning. Returns false when the move was
    /// rejected; a move to where the entity already is succeeds and records
    /// nothing.
    pub fn reparent(&mut self, uid: Uid, parent: Option<Uid>, index: Option<usize>) -> bool {
        self.finish_edit();
        let placement = |world: &World| world.entity_by_uid(uid).map(|e| world.placement(e));
        let before = placement(&self.world);
        let r = self.edits.reparent(&mut self.world, uid, parent, index);
        let ok = self.report("move", r).is_some();
        if ok && placement(&self.world) != before {
            self.changed(true);
            if let Some(p) = parent {
                self.hierarchy.expanded.insert(p);
            }
        }
        ok
    }

    /// Deletes the selected entities (each with its subtree) as one step.
    pub fn delete_selection(&mut self) {
        let uids: Vec<Uid> = self.selection.clone();
        if uids.is_empty() {
            return;
        }
        self.begin_edit(format!("Delete {}", self.selection_label()));
        for uid in uids {
            // A child of an already deleted entity is gone with it.
            if self.world.entity_by_uid(uid).is_some() {
                let r = self.edits.despawn(&mut self.world, uid);
                self.report("delete", r);
            }
        }
        self.finish_edit();
        self.selection.clear();
        self.changed(true);
    }

    /// The selected entities that have no selected ancestor, in selection
    /// order: what a subtree operation acts on, so a selected child is not
    /// handled once on its own and again inside its selected parent.
    pub fn topmost_selection(&self) -> Vec<Uid> {
        let selected: HashSet<Uid> = self.selection.iter().copied().collect();
        let under_selected = |uid: Uid| {
            let mut cur = self
                .world
                .entity_by_uid(uid)
                .and_then(|e| self.world.parent(e));
            while let Some(p) = cur {
                if self.world.uid(p).is_some_and(|u| selected.contains(&u)) {
                    return true;
                }
                cur = self.world.parent(p);
            }
            false
        };
        self.selection
            .iter()
            .copied()
            .filter(|u| !under_selected(*u))
            .collect()
    }

    /// Duplicates the selected entities as one step and selects the copies.
    /// A selected descendant of a selected entity is copied with it, once.
    pub fn duplicate_selection(&mut self) {
        let uids = self.topmost_selection();
        if uids.is_empty() {
            return;
        }
        self.begin_edit(format!("Duplicate {}", self.selection_label()));
        let mut copies = Vec::new();
        for uid in uids {
            let r = self.edits.duplicate(&mut self.world, uid);
            if let Some(c) = self.report("duplicate", r) {
                copies.push(c);
            }
        }
        self.finish_edit();
        self.changed(true);
        self.selection = copies;
        if let Some(p) = self.primary() {
            self.reveal(p);
        }
    }

    fn selection_label(&self) -> String {
        match (self.selection.len(), self.primary_entity()) {
            (1, Some(e)) => self.display_name(e),
            (n, _) => format!("{n} entities"),
        }
    }

    /// Creates an entity under `parent` (at its origin), or as a root placed
    /// where the viewport centre looks (see [`Self::placement`]), and selects
    /// it. Returns its uid.
    pub fn add_entity(&mut self, kind: NewEntity, parent: Option<Uid>) -> Option<Uid> {
        self.finish_edit();
        let registry = self.world.registry().clone();
        let mut owned: Vec<(&'static str, Value)> = Vec::new();
        let transform = match parent {
            Some(_) => Transform::default(),
            None => match kind {
                NewEntity::DirectionalLight => Transform::from_xyz(0.0, 10.0, 0.0)
                    .looking_to(Vec3::new(0.35, -0.7, -0.6), Vec3::Y),
                NewEntity::Camera => {
                    let mut t = self.camera.orbit.transform();
                    t.scale = Vec3::ONE;
                    t
                }
                // Pointing straight down at the surface below.
                NewEntity::SpotLight => Transform::from_translation(self.placement(kind))
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                _ => Transform::from_translation(self.placement(kind)),
            },
        };
        owned.push(("Transform", Box::new(transform)));
        if let Some(shape) = kind.shape() {
            owned.push(("Primitive", Box::new(Primitive::new(shape))));
            if let Some(m) = registry
                .component("StandardMaterial")
                .and_then(|c| c.default_value())
            {
                owned.push(("StandardMaterial", m));
            }
        }
        if let Some(name) = kind.required_component() {
            match registry.component(name).and_then(|c| c.default_value()) {
                Some(v) => owned.push((name, v)),
                None => {
                    tracing::warn!("cannot add {}: `{name}` is not registered", kind.label());
                    return None;
                }
            }
        }
        let components: Vec<(&str, &dyn PartialReflect)> =
            owned.iter().map(|(n, v)| (*n, v.as_ref())).collect();
        let uid = Uid::new();
        let name = self.unique_name(kind.label());
        let r = self.edits.spawn(
            &mut self.world,
            uid,
            &name,
            Placement::last(parent),
            &components,
        );
        self.report(&format!("add {}", kind.label()), r)?;
        self.changed(true);
        self.select(Some(uid));
        Some(uid)
    }

    /// Where a new root entity of `kind` goes: on the surface the viewport
    /// centre looks at (a mesh, else the ground plane y = 0, else the orbit
    /// target; see [`Self::viewport_drop_point`]), resting on it by its
    /// bounds (a point light 2.5 m above it), and moved sideways (along the
    /// camera's right) while it would overlap an existing mesh, or sit
    /// exactly on another entity's origin. Runs on a menu click: the
    /// transform propagation and mesh sync it needs for picking are
    /// change-driven and cheap.
    pub fn placement(&mut self, kind: NewEntity) -> Vec3 {
        let half = kind.half_extents();
        self.place(Aabb::new(-half, half), kind.lift())
    }

    /// Where a new root entity whose mesh has the local bounds `local` goes
    /// (see [`Self::placement`]): its origin `lift` above the surface, moved
    /// sideways while those bounds would overlap an existing mesh. Empty
    /// bounds stand for an object without a mesh.
    pub(crate) fn place(&mut self, local: Aabb, lift: f32) -> Vec3 {
        box_core::transform::propagate_transforms(&mut self.world);
        box_engine::sync_scene_meshes(&mut self.world);
        let aspect = self
            .viewport
            .rect
            .filter(|r| r.height() > 0.0)
            .map_or(16.0 / 9.0, |r| r.width() / r.height());
        let hit = self.viewport_drop_point(Vec2::ZERO, aspect);
        let (offset, half) = if local.is_empty() {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (local.center(), local.half_extents())
        };
        let probe_y = hit.y + 0.05;
        // The surface straight below a point, probed from just above the hit
        // (so a wall the centre ray hit side-on gives the floor below).
        let surface = |world: &World, x: f32, z: f32| -> f32 {
            let ray = box_core::math::Ray::new(Vec3::new(x, probe_y, z), Vec3::NEG_Y);
            match crate::picking::pick(world, &ray) {
                Some(p) => probe_y - p.distance,
                None if probe_y > 0.0 => 0.0,
                None => hit.y,
            }
        };
        let mut right = self.camera.orbit.transform().right();
        right.y = 0.0;
        let right = right.try_normalize().unwrap_or(Vec3::X);
        let step = (2.0 * half.x.max(half.z)).max(0.75) + 0.25;
        let mut at = Vec3::new(hit.x, surface(&self.world, hit.x, hit.z), hit.z);
        for _ in 0..32 {
            let origin = at + Vec3::Y * lift;
            if !overlaps_existing(&self.world, origin + offset, half) {
                return origin;
            }
            at += right * step;
            at.y = surface(&self.world, at.x, at.z);
        }
        at + Vec3::Y * lift
    }

    /// No saved entities: the hierarchy and viewport show how to add some.
    pub fn scene_is_empty(&self) -> bool {
        !self.world.roots().iter().any(|e| {
            self.world.uid(*e).is_some() && !box_core::prefab::is_generated(&self.world, *e)
        })
    }

    /// `base`, or `base 2`, `base 3`, ... if the name is taken.
    pub(crate) fn unique_name(&self, base: &str) -> String {
        let taken: HashSet<String> = self
            .world
            .query::<&Name>()
            .iter()
            .map(|n| n.0.clone())
            .collect();
        if !taken.contains(base) {
            return base.to_owned();
        }
        (2..)
            .map(|i| format!("{base} {i}"))
            .find(|n| !taken.contains(n))
            .unwrap_or_else(|| base.to_owned())
    }

    /// Frames the selection, or the whole scene when nothing is selected.
    pub fn frame_view(&mut self) {
        let bounds = match self.primary_entity() {
            // Without meshes (a light, a camera, an empty): 2 m around it.
            Some(e) => entity_bounds(&self.world, e).or_else(|| {
                let p = Vec3::from(self.world.get::<GlobalTransform>(e)?.0.translation);
                Some(Aabb::new(p - Vec3::ONE, p + Vec3::ONE))
            }),
            // A scene is framed with some room around it, even a single cube.
            None => scene_bounds(&self.world).map(|b| {
                let h = b.half_extents().max(Vec3::splat(4.0));
                Aabb::new(b.center() - h, b.center() + h)
            }),
        };
        if let Some(b) = bounds {
            self.camera.frame(&b);
        }
    }
}

/// World-space bounds of an entity's mesh and all its descendants' meshes.
pub fn entity_bounds(world: &World, entity: Entity) -> Option<Aabb> {
    let meshes = world.get_resource::<Assets<MeshData>>()?;
    let mut bounds = Aabb::EMPTY;
    let mut stack = vec![entity];
    while let Some(e) = stack.pop() {
        if let (Some(mr), Some(gt)) = (
            world.get::<MeshRenderer>(e),
            world.get::<GlobalTransform>(e),
        ) && let Some(mesh) = meshes.get(mr.mesh)
        {
            bounds = union(bounds, mesh.bounds().transformed(&gt.0));
        }
        stack.extend(world.children(e));
    }
    (!bounds.is_empty()).then_some(bounds)
}

/// Union of every mesh's world bounds, skipping flat meshes (ground planes)
/// so framing the scene is not dominated by a kilometre of floor.
pub fn scene_bounds(world: &World) -> Option<Aabb> {
    let meshes = world.get_resource::<Assets<MeshData>>()?;
    let mut bounds = Aabb::EMPTY;
    for (mr, gt) in world.query::<(&MeshRenderer, &GlobalTransform)>().iter() {
        let Some(mesh) = meshes.get(mr.mesh) else {
            continue;
        };
        let local = mesh.bounds();
        if local.is_empty() || local.half_extents().min_element() < 1e-4 {
            continue;
        }
        bounds = union(bounds, local.transformed(&gt.0));
    }
    (!bounds.is_empty()).then_some(bounds)
}

/// True when an object with bounds `centre ± half` would overlap a mesh in
/// `world` (ground planes and other flat meshes do not count: things rest
/// on them), or, for an object without volume (an empty, a light, a
/// plane), would sit on another entity's origin.
fn overlaps_existing(world: &World, centre: Vec3, half: Vec3) -> bool {
    if half.min_element() < 1e-4 {
        return world
            .query::<&GlobalTransform>()
            .iter()
            .any(|gt| (Vec3::from(gt.0.translation) - centre).length() < 1e-3);
    }
    let Some(meshes) = world.get_resource::<Assets<MeshData>>() else {
        return false;
    };
    // Touching is not overlapping: an object resting on another is fine.
    let eps = Vec3::splat(1e-3);
    let (lo, hi) = (centre - half + eps, centre + half - eps);
    world
        .query::<(&MeshRenderer, &GlobalTransform)>()
        .iter()
        .filter_map(|(mr, gt)| {
            let local = meshes.get(mr.mesh)?.bounds();
            (!local.is_empty() && local.half_extents().min_element() >= 1e-4)
                .then(|| local.transformed(&gt.0))
        })
        .any(|b| b.min.cmplt(hi).all() && b.max.cmpgt(lo).all())
}

fn union(a: Aabb, b: Aabb) -> Aabb {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    Aabb::new(a.min.min(b.min), a.max.max(b.max))
}
