//! The editor's actions: one registry that the menus (`toolbar.rs`) and the
//! command palette (`palette.rs`) both read, so a new action shows up in
//! the palette with its shortcut, its disabled reason and its tooltip
//! without further wiring.
//!
//! - [`ActionRegistry::builtin`] holds the fixed actions (File, Edit, Tools,
//!   View, Play, Help). Shortcuts come from the one table in `shortcuts.rs`.
//! - [`context_actions`] lists the actions that depend on what is open (Add
//!   ▸ kinds, Add Component for the selection, Open Scene ▸ the project's
//!   scenes, assets, recent projects, undo history); the palette builds them
//!   when it opens.
//! - [`EditorState::register_action`] is the extension point for game code
//!   and, from M5, editor tools: a registered action is in the palette (and
//!   can be put in a menu with its id).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use box_core::identity::Uid;

use crate::assets::AssetItem;
use crate::camera::EditorCamera;
use crate::layout::Pane;
use crate::play::PlayState;
use crate::shortcuts::Command;
use crate::state::{EditorState, NewEntity, TransformTool};

type RunFn = Arc<dyn Fn(&mut EditorState) + Send + Sync>;
type EnabledFn = Arc<dyn Fn(&EditorState) -> Result<(), String> + Send + Sync>;
type CheckedFn = Arc<dyn Fn(&EditorState) -> bool + Send + Sync>;
type LabelFn = Arc<dyn Fn(&EditorState) -> String + Send + Sync>;

/// Something the user can do from a menu or the command palette.
///
/// ```
/// use box_editor::actions::Action;
/// # fn f(state: &mut box_editor::EditorState) {
/// state.register_action(
///     Action::new("game.reset_score", "Game", "Reset Score", |s| {
///         s.notify("Score reset");
///     })
///     .tooltip("Sets the score back to zero")
///     .enabled_when(|s| if s.is_playing() { Ok(()) } else { Err("Start Play first".into()) }),
/// );
/// # }
/// ```
#[derive(Clone)]
pub struct Action {
    id: String,
    category: String,
    label: String,
    keywords: String,
    tooltip: Option<String>,
    shortcut: Option<Command>,
    enabled: Option<EnabledFn>,
    checked: Option<CheckedFn>,
    title: Option<LabelFn>,
    run: RunFn,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Action")
            .field("id", &self.id)
            .field("category", &self.category)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Action {
    /// An action with a stable `id` (`"file.save"`), the `category` it is
    /// listed under (`"File"`, or a path like `"Open Scene"`), its `label`
    /// and what it does.
    pub fn new(
        id: impl Into<String>,
        category: impl Into<String>,
        label: impl Into<String>,
        run: impl Fn(&mut EditorState) + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            category: category.into(),
            label: label.into(),
            keywords: String::new(),
            tooltip: None,
            shortcut: None,
            enabled: None,
            checked: None,
            title: None,
            run: Arc::new(run),
        }
    }

    /// The keyboard shortcut (shown in menus and the palette; handled by the
    /// shortcut code in `editor.rs`).
    pub fn shortcut(mut self, command: Command) -> Self {
        self.shortcut = Some(command);
        self
    }

    /// Extra words the palette search matches.
    pub fn keywords(mut self, words: impl Into<String>) -> Self {
        self.keywords = words.into();
        self
    }

    /// The hover text in menus, also shown under the selected palette row.
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// When the action is available: `Err(reason)` disables it and the
    /// reason is shown (menu hover, palette row).
    pub fn enabled_when(
        mut self,
        f: impl Fn(&EditorState) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.enabled = Some(Arc::new(f));
        self
    }

    /// A toggle: menus show a checkbox, the palette On or Off.
    pub fn checked_when(
        mut self,
        f: impl Fn(&EditorState) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.checked = Some(Arc::new(f));
        self
    }

    /// A label that follows the state ("Undo Move", "Stop"); the plain label
    /// is still what the palette searches.
    pub fn label_with(
        mut self,
        f: impl Fn(&EditorState) -> String + Send + Sync + 'static,
    ) -> Self {
        self.title = Some(Arc::new(f));
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    /// The fixed label (see [`Self::label_in`] for the shown one).
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn search_keywords(&self) -> &str {
        &self.keywords
    }

    pub fn tooltip_text(&self) -> Option<&str> {
        self.tooltip.as_deref()
    }

    pub fn shortcut_command(&self) -> Option<Command> {
        self.shortcut
    }

    /// The label as shown now.
    pub fn label_in(&self, state: &EditorState) -> String {
        match &self.title {
            Some(f) => f(state),
            None => self.label.clone(),
        }
    }

    /// `Ok` when it can run now, else why not.
    pub fn availability(&self, state: &EditorState) -> Result<(), String> {
        self.enabled.as_ref().map_or(Ok(()), |f| f(state))
    }

    /// For toggles, whether it is on.
    pub fn checked(&self, state: &EditorState) -> Option<bool> {
        self.checked.as_ref().map(|f| f(state))
    }
}

/// How many recently used action ids are kept (most recent first).
pub const RECENT_ACTIONS: usize = 30;

/// Every registered action, plus the recently used ones for the palette.
#[derive(Debug, Default)]
pub struct ActionRegistry {
    actions: Vec<Action>,
    recent: Vec<String>,
}

impl ActionRegistry {
    /// The editor's own actions.
    pub fn builtin() -> Self {
        Self {
            actions: builtin_actions(),
            recent: Vec::new(),
        }
    }

    /// Adds `action`, replacing one with the same id.
    pub fn register(&mut self, action: Action) {
        match self.actions.iter_mut().find(|a| a.id == action.id) {
            Some(old) => *old = action,
            None => self.actions.push(action),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Action> {
        self.actions.iter()
    }

    /// Recently run action ids, most recent first (this session).
    pub fn recent(&self) -> &[String] {
        &self.recent
    }

    pub(crate) fn note_used(&mut self, id: &str) {
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(RECENT_ACTIONS);
    }
}

impl EditorState {
    /// Every registered action (built-in and [`Self::register_action`]).
    pub fn actions(&self) -> &ActionRegistry {
        &self.actions
    }

    /// Adds an action to the command palette (replacing one with the same
    /// id). The extension point for game code and editor tools.
    pub fn register_action(&mut self, action: Action) {
        self.actions.register(action);
    }

    /// Runs the registered action `id` if it is available. `Err` says why
    /// not (unknown id, or its disabled reason).
    pub fn run_action(&mut self, id: &str) -> Result<(), String> {
        let action = self
            .actions
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no action `{id}`"))?;
        self.perform(&action)
    }

    /// Runs `action` (registered or from [`context_actions`]) if available,
    /// and remembers it as recently used.
    pub fn perform(&mut self, action: &Action) -> Result<(), String> {
        action.availability(self)?;
        self.actions.note_used(&action.id);
        (action.run)(self);
        Ok(())
    }
}

// ----- availability checks -----

fn editing(s: &EditorState) -> Result<(), String> {
    if s.is_playing() {
        Err("Stop Play first".into())
    } else {
        Ok(())
    }
}

fn has_selection(s: &EditorState) -> Result<(), String> {
    if s.selection.is_empty() {
        Err("Select something first".into())
    } else {
        Ok(())
    }
}

fn has_project(s: &EditorState) -> Result<(), String> {
    if s.project.is_some() {
        Ok(())
    } else {
        Err("Open a project first".into())
    }
}

fn playing(s: &EditorState) -> Result<(), String> {
    if s.is_playing() {
        Ok(())
    } else {
        Err("Start Play first".into())
    }
}

fn builtin_actions() -> Vec<Action> {
    let mut v = vec![
        // ----- File -----
        Action::new("file.new_project", "File", "New Project…", |s| {
            s.requests.new_project = true
        })
        .keywords("create"),
        Action::new("file.open_project", "File", "Open Project…", |s| {
            s.requests.open_project = true
        }),
        Action::new("file.open_example", "File", "Open Example…", |s| {
            s.requests.open_example = Some(None)
        })
        .tooltip("Copy one of the example projects into a folder and open the copy")
        .keywords("sample demo gallery"),
        Action::new("file.new_scene", "File", "New Scene", |s| {
            s.requests.new_scene = true
        })
        .tooltip("A new unsaved scene with a sun, a ground and a camera"),
        Action::new("file.save", "File", "Save Scene", |s| {
            s.requests.save_scene = true
        })
        .shortcut(Command::Save)
        .enabled_when(|s| editing(s).map_err(|_| "Stop Play to save".to_string())),
        Action::new("file.save_as", "File", "Save Scene As…", |s| {
            s.requests.save_scene_as = true
        })
        .shortcut(Command::SaveAs)
        .enabled_when(|s| editing(s).map_err(|_| "Stop Play to save".to_string())),
        Action::new("file.export", "File", "Export…", EditorState::open_export)
            .keywords("build bundle app exe pak release package ship")
            .tooltip(
                "Make a runnable game for macOS or Windows: the project cooked into game.pak \
                 with the player, as an .app or a folder with an .exe",
            )
            .enabled_when(has_project),
        Action::new("game.export_settings", "Project Settings", "Export Settings…", EditorState::open_export)
            .keywords("app name bundle id version icon")
            .tooltip("The exported game's name, bundle id, version and icon (box.project.json)")
            .enabled_when(has_project),
        Action::new("file.agent_control", "File", "Allow Agent Control", |s| {
            let on = s.agent_control().is_none();
            s.set_agent_control(on);
        })
        .keywords("mcp agent ai control")
        .tooltip(
            "Let `box mcp` and other local tools query, edit, screenshot and play \
             this editor (through <project>/.box/control.json)",
        )
        .checked_when(|s| s.agent_control().is_some())
        .enabled_when(|s| {
            has_project(s).map_err(|_| {
                "Agent control needs a project: its discovery file is <project>/.box/control.json"
                    .to_string()
            })
        }),
        Action::new("file.clear_recent", "File", "Clear Recent Projects", |s| {
            s.recent.clear()
        })
        .enabled_when(|s| {
            if s.recent.entries().is_empty() {
                Err("No recent projects".into())
            } else {
                Ok(())
            }
        }),
        Action::new("file.quit", "File", "Quit", |s| s.requests.quit = true).keywords("exit"),
        // ----- Edit -----
        // With an editable material file in the inspector, undo and redo
        // step that file's own history (`asset_undo.rs`).
        Action::new("edit.undo", "Edit", "Undo", EditorState::undo)
            .shortcut(Command::Undo)
            .label_with(|s| s.undo_menu_item(false).0)
            .enabled_when(|s| {
                if s.asset_undo_active() {
                    return if s.undo_menu_item(false).1 {
                        Ok(())
                    } else {
                        Err("Nothing to undo in this file".into())
                    };
                }
                editing(s)?;
                if s.edits.can_undo() {
                    Ok(())
                } else {
                    Err("Nothing to undo".into())
                }
            }),
        Action::new("edit.redo", "Edit", "Redo", EditorState::redo)
            .shortcut(Command::Redo)
            .label_with(|s| s.undo_menu_item(true).0)
            .enabled_when(|s| {
                if s.asset_undo_active() {
                    return if s.undo_menu_item(true).1 {
                        Ok(())
                    } else {
                        Err("Nothing to redo in this file".into())
                    };
                }
                editing(s)?;
                if s.edits.can_redo() {
                    Ok(())
                } else {
                    Err("Nothing to redo".into())
                }
            }),
        Action::new(
            "edit.duplicate",
            "Edit",
            "Duplicate",
            EditorState::duplicate_selection,
        )
        .shortcut(Command::Duplicate)
        .keywords("copy clone")
        .enabled_when(has_selection),
        Action::new(
            "edit.delete",
            "Edit",
            "Delete",
            EditorState::delete_selection,
        )
        .shortcut(Command::Delete)
        .keywords("remove")
        .enabled_when(has_selection),
        Action::new("edit.select_none", "Edit", "Select None", |s| {
            s.select(None)
        })
        .shortcut(Command::Deselect)
        .keywords("deselect clear selection")
        .enabled_when(has_selection),
        Action::new(
            "edit.hide",
            "Edit",
            "Hide / Show Selection",
            EditorState::toggle_hide_selection,
        )
        .shortcut(Command::Hide)
        .keywords("visibility eye hidden")
        .tooltip("Hidden entities are not drawn in the viewport (editor only)")
        .enabled_when(|s| {
            editing(s)?;
            has_selection(s)
        }),
        Action::new(
            "edit.show_all",
            "Edit",
            "Show All Hidden",
            EditorState::unhide_all,
        )
        .shortcut(Command::UnhideAll)
        .keywords("unhide visibility eye")
        .enabled_when(editing),
        Action::new("edit.snap_to_ground", "Edit", "Snap to Ground", |s| {
            s.snap_selection_to_ground();
        })
        .shortcut(Command::SnapToGround)
        .keywords("terrain drop floor land place height")
        .tooltip("Move the selection onto the terrain under it: its lowest point lands on the ground (undoable)")
        .enabled_when(|s| {
            editing(s)?;
            has_selection(s)
        }),
        Action::new("edit.bake", "Edit", "Bake to Mesh", |s| {
            s.bake_selection();
        })
        .keywords("procedural generator freeze glb export mesh asset")
        .tooltip(
            "Write the selected procedural object's mesh to assets/baked/ and replace its generator with that mesh (undoable)",
        )
        .enabled_when(EditorState::bake_availability),
        Action::new("edit.hide_part", "Edit", "Hide Generated Part", |s| {
            if let Some(part) = s.inspected_part() {
                s.hide_part(part);
            }
        })
        .keywords("procedural derived rule override part")
        .tooltip(
            "Do not build the inspected generated part: an override saved on its owner \
             (Cmd+Alt+click a part to inspect it)",
        )
        .enabled_when(|s| {
            if s.is_playing() {
                Err("Stop Play first".into())
            } else if s.inspected_part().is_none() {
                Err("Cmd+Alt+click a generated part to inspect it first".into())
            } else {
                Ok(())
            }
        }),
        // ----- Tools -----
        Action::new("tool.move", "Tools", "Move Tool", |s| {
            s.select_transform_tool(TransformTool::Translate)
        })
        .shortcut(Command::Move)
        .keywords("translate gizmo")
        .checked_when(|s| s.tool == TransformTool::Translate && s.active_tool().is_none()),
        Action::new("tool.rotate", "Tools", "Rotate Tool", |s| {
            s.select_transform_tool(TransformTool::Rotate)
        })
        .shortcut(Command::Rotate)
        .keywords("gizmo")
        .checked_when(|s| s.tool == TransformTool::Rotate && s.active_tool().is_none()),
        Action::new("tool.scale", "Tools", "Scale Tool", |s| {
            s.select_transform_tool(TransformTool::Scale)
        })
        .shortcut(Command::Scale)
        .keywords("gizmo resize")
        .checked_when(|s| s.tool == TransformTool::Scale && s.active_tool().is_none()),
        Action::new("tool.snap", "Tools", "Snap", |s| {
            s.snap.enabled = !s.snap.enabled;
            s.save_prefs();
        })
        .shortcut(Command::ToggleSnap)
        .keywords("grid step")
        .checked_when(|s| s.snap.enabled),
        // ----- View -----
        Action::new("view.grid", "View", "Grid", |s| s.show_grid = !s.show_grid)
            .checked_when(|s| s.show_grid),
        Action::new(
            "view.frame",
            "View",
            "Frame Selection",
            EditorState::frame_view,
        )
        .shortcut(Command::Frame)
        .keywords("focus zoom fit scene")
        .label_with(|s| {
            if s.selection.is_empty() {
                "Frame Scene".into()
            } else {
                "Frame Selection".into()
            }
        }),
        Action::new(
            "view.scene_settings",
            "View",
            "Scene Settings",
            EditorState::show_scene_settings,
        )
        .keywords(
            "environment sky fog exposure tone mapping lighting haze mist atmosphere stars vignette wind",
        )
        .tooltip(
            "Sky, fog, atmosphere, exposure, tone mapping, post-processing and wind (the scene's Environment)",
        ),
        Action::new("view.rules", "View", "Rules", |s| {
            let open = !s.rules_panel_open();
            s.set_rules_panel(open);
        })
        .keywords("procedural generated parts derived rule list enable disable")
        .tooltip(
            "The rules that generate parts from saved intent: intents, parts, last run, \
             warnings, and a switch to turn one off for this session",
        )
        .checked_when(EditorState::rules_panel_open),
        Action::new("view.assistant", "View", "Assistant", |s| {
            let open = !s.assistant_open();
            s.set_assistant(open);
        })
        .keywords("ai agent claude ask chat help build describe assistant prompt")
        .tooltip(
            "Describe what to build or change and an AI agent (Claude Code) does it in this \
             project; each request is one undo step",
        )
        .checked_when(EditorState::assistant_open),
        Action::new("view.mixer", "View", "Mixer", |s| {
            let open = !s.mixer_open();
            s.set_mixer(open);
        })
        .keywords("audio sound volume levels solo mute bus fader meter")
        .tooltip(
            "Bus volumes, and during Play every playing sound with its level at the listener, \
             distance, attenuation and pan, volume, solo and mute (until Stop)",
        )
        .checked_when(EditorState::mixer_open),
        Action::new("view.audio_ranges", "View", "Audio Ranges", |s| {
            s.viewport.view.audio = !s.viewport.view.audio
        })
        .keywords("sound sources spatial distance listener debug")
        .tooltip(
            "Draw sound sources' full-volume and silent distances, and during Play a line \
             from each to the listener coloured by its level",
        )
        .checked_when(|s| s.viewport.view.audio),
        Action::new("view.navigation", "View", "Navigation", |s| {
            s.viewport.view.nav = !s.viewport.view.nav
        })
        .keywords("navmesh pathfinding enemies walkable ai paths")
        .tooltip(
            "Draw the navmesh: where enemies and other agents can walk. It is built from \
             the scene's static colliders and follows your edits",
        )
        .checked_when(|s| s.viewport.view.nav),
        Action::new("view.reset_camera", "View", "Reset Camera", |s| {
            s.camera = EditorCamera::default()
        }),
        Action::new("view.save_layout", "View", "Save Layout", |s| {
            s.requests.save_layout = true
        })
        .enabled_when(|s| {
            has_project(s).map_err(|_| "Open a project to save its layout".to_string())
        }),
        Action::new("view.reset_layout", "View", "Reset Layout", |s| {
            s.requests.reset_layout = true
        }),
    ];
    for pane in Pane::ALL {
        v.push(
            Action::new(
                format!("view.show_{}", pane.title().to_lowercase()),
                "View",
                format!("Show {}", pane.title()),
                move |s| s.requests.focus_pane = Some(pane),
            )
            .keywords("panel tab pane"),
        );
    }
    v.extend([
        // ----- Play -----
        Action::new("play.toggle", "Play", "Play", EditorState::toggle_play)
            .shortcut(Command::Play)
            .keywords("run start stop game")
            .label_with(|s| if s.is_playing() { "Stop" } else { "Play" }.into()),
        Action::new("play.pause", "Play", "Pause", EditorState::toggle_pause)
            .shortcut(Command::Pause)
            .keywords("resume")
            .label_with(|s| {
                if s.play_state() == PlayState::Paused {
                    "Resume"
                } else {
                    "Pause"
                }
                .into()
            })
            .enabled_when(playing),
        Action::new(
            "play.step",
            "Play",
            "Step One Frame",
            EditorState::step_play,
        )
        .shortcut(Command::Step)
        .keywords("advance frame")
        .enabled_when(playing),
        Action::new("play.free_camera", "Play", "Free Camera", |s| {
            let on = !s.free_camera();
            s.set_free_camera(on);
        })
        .tooltip("Look at the play world through the editor camera")
        .checked_when(EditorState::free_camera)
        .enabled_when(playing),
        // ----- Game -----
        Action::new(
            "game.rebuild",
            "Game",
            "Rebuild and Restart",
            EditorState::request_rebuild,
        )
        .shortcut(Command::Rebuild)
        .keywords("build compile cargo restart reload code native")
        .tooltip(
            "Rebuild the game's code (cargo build of this editor binary) and restart the \
             editor with the same scene, camera and selection; compiler errors appear as \
             toasts and the editor keeps running",
        )
        .label_with(|s| {
            if s.rebuild_running() {
                "Building…".into()
            } else {
                "Rebuild and Restart".into()
            }
        })
        .enabled_when(EditorState::rebuild_availability),
        // ----- Help -----
        Action::new("help.palette", "Help", "Command Palette…", |s| {
            s.open_palette()
        })
        .shortcut(Command::Palette)
        .keywords("search commands actions find"),
        Action::new("help.shortcuts", "Help", "Keyboard Shortcuts…", |s| {
            s.requests.show_shortcuts = true
        })
        .keywords("keys bindings hotkeys"),
        Action::new("help.welcome", "Help", "Welcome Screen", |s| {
            s.welcome = true
        })
        .keywords("start recent projects")
        .tooltip("New Project, Open Project and the recent projects"),
    ]);
    if let Some(docs) = docs_dir() {
        let tip = docs.display().to_string();
        v.push(
            Action::new("help.docs", "Help", "Documentation", move |_| {
                open_in_os(&docs)
            })
            .keywords("manual guide")
            .tooltip(tip),
        );
    }
    v
}

/// Game ▸ Project Settings ▸ one action per settings type of `game`
/// (`settings.<Type>`), registered when the editor state is made.
pub(crate) fn settings_actions(names: &[&'static str]) -> Vec<Action> {
    names
        .iter()
        .map(|&name| {
            Action::new(
                format!("settings.{name}"),
                "Project Settings",
                crate::inspect::friendly_name(name),
                move |s| {
                    s.show_project_settings(name);
                },
            )
            .keywords(format!("project settings tuning {name}"))
            .tooltip(format!(
                "Edit {name} in the inspector (assets/settings/{name}.json; hot-reloads, shared by every scene)"
            ))
            .checked_when(move |s| s.project_settings_shown() == Some(name))
            .enabled_when(|s| {
                has_project(s).map_err(|_| "Open a project to edit its settings".to_string())
            })
        })
        .collect()
}

/// The engine's `docs/` folder when the editor runs from a source checkout.
fn docs_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs");
    dir.is_dir().then(|| dir.canonicalize().unwrap_or(dir))
}

/// Opens a folder in the platform's file manager, or a file in its default
/// app.
pub(crate) fn open_in_os(path: &Path) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        tracing::warn!("could not open {}: {e}", path.display());
    }
}

// ----- actions that depend on what is open -----

/// Add ▸ `kind`, as in the Add menus.
pub(crate) fn add_entity_action(kind: NewEntity) -> Action {
    let id = format!("add.{}", kind.label().to_lowercase().replace(' ', "_"));
    Action::new(id, "Add", kind.label(), move |s| {
        s.add_entity(kind, None);
    })
    .keywords("new create object entity spawn")
    .enabled_when(move |s| match kind.required_component() {
        Some(c) if s.world.registry().component(c).is_none() => {
            Err(format!("Needs the {c} component, which is not registered"))
        }
        _ => Ok(()),
    })
}

/// Add ▸ `<category>` ▸ `<generator>`, as in the Add menus.
fn add_generator_action(g: crate::procedural::GeneratorEntry) -> Action {
    let name = g.name;
    let mut a = Action::new(
        format!("add.generator:{name}"),
        "Add",
        format!("{} ▸ {}", g.category, g.label),
        move |s| {
            s.add_generator(name);
        },
    )
    .keywords(format!(
        "new create procedural generator {name} {}",
        g.category
    ));
    if let Some(doc) = g.doc {
        a = a.tooltip(doc);
    }
    a
}

/// Add ▸ `<category>` ▸ `<preset>`, as in the Add menus.
fn add_preset_action(p: crate::presets::PresetEntry) -> Action {
    let name = p.name.clone();
    let mut a = Action::new(
        format!("add.preset:{}", p.name),
        "Add",
        format!("{} ▸ {}", p.category, p.title),
        move |s| {
            s.add_preset(&name);
        },
    )
    .keywords(format!(
        "new create preset gameplay {} {} {}",
        p.name, p.category, p.description
    ));
    if !p.description.is_empty() {
        a = a.tooltip(p.description);
    }
    a
}

/// The actions that depend on what is open, for the palette: Add ▸ kinds
/// and generators,
/// Add Component ▸ (for the selection), Open Scene ▸ the project's scenes,
/// Assets ▸ files (selects them in the browser), Open Recent ▸ projects and
/// the undo history. Built when the palette opens (it reads the project's
/// scene folder once).
pub fn context_actions(state: &mut EditorState) -> Vec<Action> {
    let mut out: Vec<Action> = NewEntity::ALL.into_iter().map(add_entity_action).collect();
    out.extend(
        state
            .generator_entries()
            .into_iter()
            .map(add_generator_action),
    );
    out.extend(state.preset_entries().into_iter().map(add_preset_action));
    component_actions(state, &mut out);
    if let Some(project) = &state.project {
        match project.scene_files() {
            Ok(files) => {
                for file in files {
                    let rel = project.relative(&file);
                    let label = rel.strip_prefix("assets/").unwrap_or(&rel).to_owned();
                    let current = state.scene.path.as_deref() == Some(file.as_path());
                    let mut a =
                        Action::new(format!("scene.open:{rel}"), "Open Scene", label, move |s| {
                            s.requests.open_scene = Some(file.clone())
                        })
                        .tooltip(rel);
                    if current {
                        a = a.enabled_when(|_| Err("Already open".into()));
                    }
                    out.push(a);
                }
            }
            Err(e) => tracing::warn!("palette: {e}"),
        }
        for (id, path) in state.asset_files() {
            out.push(
                Action::new(format!("asset:{path}"), "Assets", path.clone(), move |s| {
                    s.reveal_asset(id)
                })
                .keywords("file asset browser")
                .tooltip("Select it in the asset browser"),
            );
        }
    }
    for r in state.recent.entries() {
        let path = r.path.clone();
        let current = state.project.as_ref().is_some_and(|p| p.root() == path);
        let exists = path.join(box_engine::project::PROJECT_FILE).is_file();
        let mut a = Action::new(
            format!("recent:{}", path.display()),
            "Open Recent",
            r.name.clone(),
            move |s| s.requests.open_recent = Some(path.clone()),
        )
        .keywords(format!("project {}", r.path.display()))
        .tooltip(r.path.display().to_string());
        if current {
            a = a.enabled_when(|_| Err("Already open".into()));
        } else if !exists {
            a = a.enabled_when(|_| Err("Folder not found (moved or deleted?)".into()));
        }
        out.push(a);
    }
    for entry in state.history_entries() {
        let depth = entry.depth;
        let mut a = Action::new(
            format!("history:{depth}"),
            "History",
            entry.label.clone(),
            move |s| s.jump_history(depth),
        )
        .keywords("undo redo history step");
        if entry.current {
            a = a.enabled_when(|_| Err("Current state".into()));
        } else {
            a = a.enabled_when(editing);
        }
        out.push(a);
    }
    out
}

/// Add Component ▸ for the selection (friendly names, as in the
/// inspector's Add Component list).
fn component_actions(state: &EditorState, out: &mut Vec<Action>) {
    let world = &state.world;
    let entities: Vec<_> = state
        .selection
        .iter()
        .filter_map(|u| world.entity_by_uid(*u))
        .collect();
    if entities.is_empty() {
        return;
    }
    let addable = crate::inspect::addable_to_any(world, &entities);
    let uids: Vec<Uid> = state.selection.clone();
    for c in crate::inspect::component_choices(world.registry(), &addable, "") {
        let uids = uids.clone();
        let name = c.name;
        let doc = world.registry().component(name).and_then(|r| r.doc());
        let mut a = Action::new(
            format!("component.add:{name}"),
            "Add Component",
            c.label,
            move |s| s.add_component_to(&uids, name),
        )
        .keywords(format!("{name} {}", c.category.label()));
        if let Some(doc) = doc {
            a = a.tooltip(doc);
        }
        out.push(a);
    }
}

impl EditorState {
    /// Shows an asset file in the browser: its folder, selected (and in the
    /// inspector), with the Assets tab in front.
    pub fn reveal_asset(&mut self, id: box_core::asset_ref::AssetId) {
        let Some(path) = self.asset_server().and_then(|s| s.path_of(id)) else {
            return;
        };
        self.assets.filter.clear();
        self.assets.folder = crate::assets::parent_folder(&path).to_owned();
        self.select_asset(Some(AssetItem::file(id)));
        self.assets.active = true;
        self.requests.focus_pane = Some(Pane::Assets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_shortcuts_are_in_the_table() {
        let r = ActionRegistry::builtin();
        let ids: Vec<&str> = r.iter().map(Action::id).collect();
        for (i, id) in ids.iter().enumerate() {
            assert!(!ids[i + 1..].contains(id), "{id} twice");
        }
        for a in r.iter() {
            if let Some(c) = a.shortcut_command() {
                // Panics when a command has no row.
                let _ = crate::shortcuts::shortcut(c);
            }
        }
    }

    #[test]
    fn register_replaces_by_id_and_recent_is_most_recent_first() {
        let mut r = ActionRegistry::default();
        r.register(Action::new("a", "X", "One", |_| {}));
        r.register(Action::new("a", "X", "Two", |_| {}));
        assert_eq!(r.iter().count(), 1);
        assert_eq!(r.get("a").unwrap().label(), "Two");
        r.note_used("a");
        r.note_used("b");
        r.note_used("a");
        assert_eq!(r.recent(), ["a", "b"]);
    }
}
