//! The editor shell: shortcuts, toolbar, dock tree, request handling and
//! layout persistence around an [`EditorState`]. Document dialogs (unsaved
//! changes, recovery, Save As, New/Open Project) are in `dialogs.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use box_core::assets::Assets;
use box_core::color::Color;
use box_core::input::Input;
use box_core::math::Aabb;
use box_core::mesh::MeshData;
use box_core::transform::{GlobalTransform, propagate_transforms};
use box_core::world::{Entity, World};
use box_engine::GamepadPoller;
use box_render::{DebugDraw, LineLayer, MeshRenderer, SelectionOutline};
use egui::{Frame, Key, Modifiers, Stroke, Ui};
use egui_tiles::Tree;
use glam::Vec3;

use crate::dialogs::{Dialog, DocAction};
use crate::dock::Dock;
use crate::layout::{self, LayoutError, Pane};
use crate::play::PlayState;
use crate::shortcuts::{self, Command};
use crate::state::{EditorState, TransformTool};
use crate::theme::{self, Tokens};
use crate::toolbar::toolbar_ui;

pub struct Editor {
    pub state: EditorState,
    pub tree: Tree<Pane>,
    pub(crate) tokens: Tokens,
    /// The open modal (see `dialogs.rs`).
    pub(crate) dialog: Option<Dialog>,
    /// Use the platform's file dialogs (`rfd`) for New/Open Project and Save
    /// Scene As. Off for headless runs and tests, which must never block on
    /// a dialog.
    pub(crate) native_dialogs: bool,
    /// Quit was asked for and unsaved changes were dealt with.
    pub(crate) quit_confirmed: bool,
    /// Gamepads for Play (windowed runs; see [`Editor::with_gamepads`]).
    gamepads: Option<GamepadPoller>,
    /// Drains gamepad events while the game has no focus (only connections
    /// are kept); focus then copies the devices' state into the play world.
    idle_pads: Input,
    /// Performs Rebuild and Restart's restart (see `rebuild.rs`).
    pub(crate) restart_hook: crate::rebuild::RestartHook,
    /// Bounds boxes of capped generated meshes (see [`selection_highlight`]).
    highlight_boxes: BoxCache,
    /// A crash report from the last run, shown once no other dialog is open
    /// (see [`Editor::offer_crash_report`]).
    pub(crate) crash_notice: Option<PathBuf>,
    /// Template and example preview images by file (see `new_project.rs`);
    /// `None` for one that failed to load.
    pub(crate) previews: std::collections::HashMap<PathBuf, Option<egui::TextureHandle>>,
}

impl Editor {
    /// Uses the project's saved layout when there is a valid one, and offers
    /// the scene's recovery file when it is newer than the scene.
    pub fn new(state: EditorState) -> Self {
        let mut editor = Self {
            state,
            tree: layout::default_layout(),
            tokens: theme::DARK,
            dialog: None,
            native_dialogs: false,
            quit_confirmed: false,
            gamepads: None,
            idle_pads: Input::default(),
            restart_hook: Box::new(crate::rebuild::exec_restart),
            highlight_boxes: BoxCache::default(),
            crash_notice: None,
            previews: Default::default(),
        };
        editor.load_project_settings();
        // After Rebuild and Restart the recovery file was written by the
        // restart itself: restore it without asking.
        if !(editor.state.after_rebuild && editor.restore_recovery_after_rebuild()) {
            editor.offer_recovery();
        }
        if editor.state.after_rebuild {
            editor.state.notify("Rebuilt and restarted");
        }
        editor
    }

    /// The project's layout and editor preferences (`prefs.rs`), and its
    /// last session when sessions are restored. Notes the project as the
    /// most recent one.
    pub(crate) fn load_project_settings(&mut self) {
        self.load_layout();
        let session = self.state.load_prefs();
        if self.state.restore_session {
            self.state.apply_session(&session);
        }
        self.state.note_recent_project();
    }

    /// Adds an action to the command palette; see
    /// [`EditorState::register_action`].
    pub fn register_action(&mut self, action: crate::actions::Action) {
        self.state.register_action(action);
    }

    /// Adds a viewport tool (toolbar tool group and palette); see
    /// [`EditorState::register_tool`] and [`crate::tools`].
    pub fn register_tool(&mut self, tool: impl crate::tools::ViewportTool + 'static) {
        self.state.register_tool(tool);
    }

    pub(crate) fn load_layout(&mut self) {
        let Some(project) = &self.state.project else {
            self.tree = layout::default_layout_for(false);
            return;
        };
        let path = layout::layout_path(project.root());
        let (tree, err) = layout::load_or_default(&path);
        match err {
            Some(e) => tracing::warn!("layout: {e}; using the default layout"),
            None if path.exists() => tracing::info!("layout: {}", path.display()),
            None => tracing::info!("layout: default (no saved layout yet)"),
        }
        self.tree = tree;
    }

    /// Use native file dialogs for New/Open Project and Save Scene As
    /// (windowed runs only).
    pub fn with_native_dialogs(mut self) -> Self {
        self.native_dialogs = true;
        self
    }

    /// Feed gamepads to the play world while the game has input focus
    /// (windowed runs own one poller; headless runs have none).
    pub fn with_gamepads(mut self, gamepads: GamepadPoller) -> Self {
        self.gamepads = Some(gamepads);
        self
    }

    /// Installs the editor theme into an egui context.
    pub fn apply_theme(&self, ctx: &egui::Context) {
        theme::apply(ctx, &self.tokens);
    }

    /// Quit was asked for (menu, close button) and unsaved changes were
    /// saved or discarded: the runner should close.
    pub fn quit_requested(&self) -> bool {
        self.quit_confirmed
    }

    /// Saves the dock layout to `<project>/.box/layout.json`. Returns the path,
    /// or `None` without a project.
    pub fn save_layout(&self) -> Result<Option<PathBuf>, LayoutError> {
        let Some(project) = &self.state.project else {
            return Ok(None);
        };
        let path = layout::layout_path(project.root());
        layout::save_layout(&path, &self.tree)?;
        Ok(Some(path))
    }

    /// On quit: stop agent control, always save the layout, and keep unsaved
    /// scene changes in the recovery file rather than losing them.
    pub fn on_quit(&mut self) {
        self.state.flush_settings_edit(f64::INFINITY, true);
        self.state.set_agent_control(false);
        self.state.save_prefs();
        match self.save_layout() {
            Ok(Some(path)) => tracing::info!("layout saved to {}", path.display()),
            Ok(None) => {}
            Err(e) => tracing::error!("layout save failed: {e}"),
        }
        self.state.write_recovery_if_dirty();
    }

    /// Window title: project, scene, a dirty marker and the Play state.
    pub fn title(&self) -> String {
        let dirty = if self.state.is_dirty() { " •" } else { "" };
        let play = match self.state.play_state() {
            PlayState::Editing => "",
            PlayState::Playing => " — Playing",
            PlayState::Paused => " — Paused",
        };
        let read_only = if self.state.read_only {
            " (read-only)"
        } else {
            ""
        };
        match &self.state.project {
            Some(p) => format!(
                "box-editor — {}{read_only} — {}{dirty}{play}",
                p.name(),
                self.state.scene.name
            ),
            None => format!("box-editor — {}{dirty}{play}", self.state.scene.name),
        }
    }

    /// The whole editor UI for one frame.
    pub fn ui(&mut self, ui: &mut Ui) {
        let t = self.tokens;
        // Once per frame, not again in a discarded egui pass.
        if ui.ctx().current_pass_index() == 0 {
            self.state.refresh_rebuild_target();
            if self.poll_rebuild() | self.state.poll_export() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            self.state.update_assets();
            self.import_dropped_files(ui.ctx());
            self.play_frame(ui.ctx());
            self.state.poll_log();
            self.state.refresh_asset_marks();
            self.state.refresh_generated_marks();
        }
        self.shortcuts(ui.ctx());
        egui::Panel::top("toolbar")
            .exact_size(t.toolbar_h)
            .show_separator_line(false)
            .resizable(false)
            .frame(Frame::NONE.fill(t.surface_header))
            .show(ui, |ui| {
                toolbar_ui(ui, &mut self.state);
                let r = ui.max_rect();
                ui.painter()
                    .hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.divider));
            });
        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(t.divider))
            .show(ui, |ui| {
                let mut dock = Dock {
                    state: &mut self.state,
                    tokens: t,
                };
                self.tree.ui(&mut dock, ui);
            });
        if !self.state.viewport.shown {
            // Hidden tab: no pointer is over it.
            self.state.viewport.hovered = false;
        } else if ui.ctx().current_pass_index() == 0 {
            // Particle preview (View ▸ Particle preview): time-driven, so
            // it repaints while an emitter is previewed.
            let dt = ui.input(|i| i.unstable_dt);
            if self.state.preview_particles(dt) {
                ui.ctx().request_repaint();
            }
        }
        self.end_asset_frame(ui.ctx());
        crate::rules_panel::rules_window(ui.ctx(), &mut self.state, &t);
        crate::export::export_window(ui.ctx(), &mut self.state, &t, self.native_dialogs);
        if let Some(focus) = crate::feedback::toasts_ui(ui.ctx(), &mut self.state, &t) {
            self.state.requests.focus_console = Some(focus);
        }
        // Under a modal the card is hidden (it comes back when the modal
        // closes without opening a project).
        if self.dialog.is_none() {
            crate::start::welcome_ui(ui.ctx(), &mut self.state, &t);
        }
        crate::palette::palette_ui(ui.ctx(), &mut self.state, &t);
        self.handle_requests();
        self.dialog_ui(ui.ctx());
        // Keep the recovery copy current. Idle cost: one integer compare.
        if self.state.recovery_pending() {
            let now = ui.input(|i| i.time);
            self.state.autosave_recovery(now);
        }
    }

    /// Files dropped onto the window from the operating system are copied
    /// into the asset browser's folder (the originals are left alone).
    fn import_dropped_files(&mut self, ctx: &egui::Context) {
        let files: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if files.is_empty() {
            return;
        }
        if let Err(e) = self.state.import_files(&files) {
            tracing::warn!("{e:#}");
        }
    }

    /// After the panes: the asset drag ends with the pointer (each drop
    /// target has handled the release by now), pending material edits are
    /// written when nothing is being dragged or typed, and files dragged
    /// over the window from the OS get a hint.
    fn end_asset_frame(&mut self, ctx: &egui::Context) {
        let (down, released, now, hovering) = ctx.input(|i| {
            (
                i.pointer.any_down(),
                i.pointer.any_released(),
                i.time,
                !i.raw.hovered_files.is_empty(),
            )
        });
        if self.state.assets.drag.is_some() {
            if released || !down {
                self.state.assets.drag = None;
            } else {
                crate::panes::assets::drag_ghost(ctx, &self.state);
            }
        }
        let idle = !down && ctx.memory(|m| m.focused().is_none());
        self.state.flush_material_edit(now, idle);
        self.state.flush_settings_edit(now, idle);
        if hovering {
            let t = self.tokens;
            let p = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("os_file_hover"),
            ));
            let r = ctx.content_rect();
            p.rect_filled(r, 0.0, t.accent.gamma_multiply(0.10));
            let target = match (&self.state.project, self.state.assets.folder.as_str()) {
                (None, _) => "Open a project to add files".to_string(),
                (Some(_), "") => "Drop to copy into assets/".to_string(),
                (Some(_), f) => format!("Drop to copy into assets/{f}/"),
            };
            crate::widgets::overlay_pill(&p, r.center(), egui::Align2::CENTER_CENTER, target, &t);
        }
    }

    /// Play controls, game input and one play-world update. Runs before the
    /// other shortcuts: while the game has focus its keys are taken out of
    /// egui's input here, so only Play/Stop, Pause, Step and Escape (which
    /// gives input back to the editor) reach the editor.
    fn play_frame(&mut self, ctx: &egui::Context) {
        if self.dialog.is_none() && !self.state.palette_open() {
            if ctx.input_mut(|i| shortcuts::consume(i, Command::Step)) {
                self.state.step_play();
            }
            if ctx.input_mut(|i| shortcuts::consume(i, Command::Pause)) {
                self.state.toggle_pause();
            }
            if ctx.input_mut(|i| shortcuts::consume(i, Command::Play)) {
                self.state.toggle_play();
            }
        }
        let s = &mut self.state;
        if s.game_focus() {
            let release = !ctx.input(|i| i.focused)
                || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            if release {
                s.set_game_focus(false);
            } else if let Some(rect) = s.viewport.rect {
                // A grabbed cursor's motion arrives as raw device motion.
                let pointer_motion = !s.game_cursor_grabbed();
                if let Some(input) = s.world.get_resource_mut::<Input>() {
                    crate::play_input::forward(ctx, input, rect, pointer_motion);
                }
            }
        }
        if let Some(pads) = &mut self.gamepads {
            let focus = s.game_focus();
            pads.set_focused(focus);
            match s.world.get_resource_mut::<Input>().filter(|_| focus) {
                Some(input) => pads.update(input),
                None => pads.update(&mut self.idle_pads),
            }
        }
        if s.is_playing() {
            let dt = ctx.input(|i| i.unstable_dt).clamp(0.0, 0.25);
            // A game panic stops Play; it is logged to the console there.
            let _ = s.update_play(dt);
            ctx.request_repaint();
        }
    }

    /// The editor's keyboard shortcuts (the table is in `shortcuts.rs`;
    /// Play, Pause and Step are handled in [`Self::play_frame`], F2 by the
    /// hierarchy and asset panes). Delete and Duplicate act on entities
    /// only while the asset browser is not the active pane and the
    /// inspector is not showing an asset: there, Delete moves the selected
    /// asset file to the project's trash (after a confirmation).
    fn shortcuts(&mut self, ctx: &egui::Context) {
        // A modal owns the keyboard (its own Enter/Escape handling).
        if self.dialog.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let s = &mut self.state;
        // The palette opens (or closes) while typing too, and owns the
        // keyboard while it is open.
        if ctx.input_mut(|i| shortcuts::consume(i, Command::Palette)) {
            if s.palette_open() {
                s.close_palette();
            } else {
                s.open_palette();
            }
        }
        if s.palette_open() {
            return;
        }
        if ctx.input_mut(|i| shortcuts::consume(i, Command::Rebuild)) {
            s.request_rebuild();
        }
        // Save works while typing too.
        if ctx.input_mut(|i| shortcuts::consume(i, Command::SaveAs)) {
            s.requests.save_scene_as = true;
        }
        if ctx.input_mut(|i| shortcuts::consume(i, Command::Save)) {
            s.requests.save_scene = true;
        }
        // A focused text field owns the other keys, undo and redo included
        // (they edit its text, not the scene).
        if typing {
            return;
        }
        // An active viewport tool sees key presses first (not Escape, which
        // leaves it); the keys it uses reach nothing else.
        if s.active_tool().is_some() && !s.is_playing() {
            let keys: Vec<(Key, Modifiers)> = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } if *key != Key::Escape => Some((*key, *modifiers)),
                        _ => None,
                    })
                    .collect()
            });
            for (key, modifiers) in keys {
                if s.tool_key(key, modifiers) {
                    ctx.input_mut(|i| {
                        i.events.retain(|e| {
                            !matches!(e, egui::Event::Key { key: k, pressed: true, .. } if *k == key)
                        })
                    });
                }
            }
        }
        let take = |c: Command| ctx.input_mut(|i| shortcuts::consume(i, c));
        // Undo history belongs to the edit world, so it is off during Play
        // (a material file's history, shown in the inspector, is not).
        let undo_ok = !s.is_playing() || s.asset_undo_active();
        if take(Command::Redo) && undo_ok {
            s.redo();
        }
        if take(Command::Undo) && undo_ok {
            s.undo();
        }
        if take(Command::Hide) {
            s.toggle_hide_selection();
        }
        if take(Command::UnhideAll) && !s.is_playing() {
            s.unhide_all();
        }
        let assets_focused = s.assets.active || s.assets.inspect.is_some();
        if take(Command::Duplicate) && !assets_focused {
            s.duplicate_selection();
        }
        // Enter finishes a viewport tool (the tool saw the key first).
        if s.active_tool().is_some() && take(Command::FinishTool) {
            s.cancel_tool();
        }
        if take(Command::Delete) {
            if assets_focused {
                s.request_delete_selected_asset();
            } else {
                s.delete_selection();
            }
        }
        // While flying (right button held over the viewport) W A S D Q E
        // move the camera: the single-key tool shortcuts are off.
        let flying =
            s.camera.flying || (s.viewport.hovered && ctx.input(|i| i.pointer.secondary_down()));
        if !flying {
            for (command, tool) in [
                (Command::Move, TransformTool::Translate),
                (Command::Rotate, TransformTool::Rotate),
                (Command::Scale, TransformTool::Scale),
            ] {
                if take(command) {
                    s.select_transform_tool(tool);
                }
            }
            if take(Command::ToggleSnap) {
                s.snap.enabled = !s.snap.enabled;
                s.save_prefs();
            }
            if take(Command::Frame) {
                s.frame_view();
            }
        }
        if ctx.input(|i| shortcuts::pressed(i, Command::Deselect)) {
            if s.assets.drag.take().is_some() || s.assets.picker.take().is_some() {
                // Escape cancels an asset drag or closes the asset picker.
            } else if s.edit_in_progress() {
                // Escape cancels a gizmo or inspector drag: back to the
                // values from before it, and no undo step.
                if s.inspector_edit.is_some() {
                    ctx.stop_dragging();
                }
                s.cancel_edit();
            } else if s.marquee_cancel() {
                // Escape during a marquee restores the selection before it.
            } else if !egui::Popup::is_any_open(ctx) && s.tool_escape() {
                // Escape cancels the active tool's operation (a drawing).
            } else if !egui::Popup::is_any_open(ctx) && s.cancel_tool() {
                // Escape leaves a viewport tool: back to the transform tools
                // (the selection stays).
            } else if !egui::Popup::is_any_open(ctx) {
                // Not when the key is closing a menu, context menu or combo.
                s.select(None);
            }
        }
    }

    /// One-shot requests from menus and shortcuts. Document actions start
    /// here and may open a dialog (drawn right after, in the same frame).
    fn handle_requests(&mut self) {
        let r = std::mem::take(&mut self.state.requests);
        if r.reset_layout {
            self.tree = layout::default_layout_for(self.state.project.is_some());
            tracing::info!("layout reset to the default");
        }
        if let Some(seq) = r.focus_console {
            self.focus_console(seq);
        }
        if let Some(pane) = r.focus_pane {
            self.tree
                .make_active(|_, tile| matches!(tile, egui_tiles::Tile::Pane(p) if *p == pane));
        }
        if r.save_layout {
            match self.save_layout() {
                Ok(Some(path)) => tracing::info!("layout saved to {}", path.display()),
                Ok(None) => {}
                Err(e) => tracing::error!("layout save failed: {e}"),
            }
        }
        // One dialog at a time; menus are blocked while one is open anyway.
        if self.dialog.is_some() {
            return;
        }
        if r.rebuild {
            self.begin_rebuild();
        }
        // `world` is the play world during Play: never save it. Document
        // actions below stop Play first (see `begin`).
        if (r.save_scene || r.save_scene_as) && self.state.is_playing() {
            tracing::warn!("Stop Play to save; changes made during Play are discarded on Stop");
        } else if r.save_scene_as {
            self.begin_save_as(None);
        } else if r.save_scene {
            self.save();
        }
        if r.show_shortcuts {
            self.dialog = Some(Dialog::Shortcuts);
            return;
        }
        if let Some(id) = r.delete_asset {
            self.begin_delete_asset(id);
            return;
        }
        let action = if r.quit {
            Some(DocAction::Quit)
        } else if r.new_scene {
            Some(DocAction::NewScene)
        } else if r.new_project {
            Some(DocAction::NewProject)
        } else if r.open_project {
            Some(DocAction::OpenProject)
        } else if let Some(example) = r.open_example {
            Some(DocAction::OpenExample(example))
        } else if let Some(path) = r.open_recent {
            Some(DocAction::OpenRecent(path))
        } else {
            r.open_scene.map(DocAction::OpenScene)
        };
        if let Some(action) = action {
            self.begin(action);
        }
    }

    /// Brings the Console tab to the front and, with `seq`, selects that
    /// record and scrolls to it (a toast was clicked).
    pub fn focus_console(&mut self, seq: Option<u64>) {
        self.tree
            .make_active(|_, tile| matches!(tile, egui_tiles::Tile::Pane(Pane::Console)));
        if let Some(seq) = seq {
            self.state.console.reveal(seq);
        }
    }

    /// Before rendering the viewport: install the game's rules in a new edit
    /// world, propagate transforms, run rules and rebuild changed (and
    /// regenerate procedural) meshes, set the selection outline and queue the editor overlays (grid,
    /// helpers of selected cameras, lights and colliders (`icons.rs`),
    /// markers for selected entities without meshes when icons are off,
    /// gizmo) as
    /// display-referred lines, unaffected by exposure and tone mapping. The
    /// game camera's view during Play gets no editor overlays.
    pub fn prepare_render(&mut self) {
        let state = &mut self.state;
        // The edit world has no App: give it the game's rules once (a new
        // or loaded scene is a new world), so rule parts generate while
        // editing. Play worlds get them from their App.
        if !state.is_playing()
            && !state
                .world
                .contains_resource::<box_engine::box_proc::rule::RuleRegistry>()
        {
            let rules = state.game.rules();
            state.world.insert_resource(rules);
            // Change-driven systems the game marked for the edit world
            // (box_arch's anchors); gameplay never runs here.
            let systems = state.game.edit_world_systems();
            state.world.insert_resource(systems);
        }
        // Rules switched off in the rule list (this session).
        state.apply_rule_switches();
        propagate_transforms(&mut state.world);
        box_engine::sync_scene_meshes(&mut state.world);
        if !state.is_playing() {
            box_core::app::EditWorldSystems::run_in(&mut state.world);
        }
        // Hidden entities (hierarchy eye, H) are not drawn; never in Play.
        state.sync_hidden_markers();
        if state.shows_game_camera() {
            state.world.insert_resource(SelectionOutline::default());
            return;
        }
        let a = self.tokens.accent;
        let accent = Color::srgb(
            a.r() as f32 / 255.0,
            a.g() as f32 / 255.0,
            a.b() as f32 / 255.0,
        );
        let Highlight {
            outline,
            markers,
            boxes,
        } = selection_highlight(state, accent, &mut self.highlight_boxes);
        state.world.insert_resource(outline);
        let mut dd = state
            .world
            .remove_resource::<DebugDraw>()
            .unwrap_or_default();
        if state.show_grid {
            let minor = Color::linear_rgba(0.0, 0.0, 0.0, 0.10);
            let major = Color::linear_rgba(0.0, 0.0, 0.0, 0.22);
            dd.grid_on(LineLayer::Display, Vec3::ZERO, 50.0, 1.0, minor);
            dd.grid_on(LineLayer::Display, Vec3::ZERO, 50.0, 10.0, major);
            let axis = |c: egui::Color32| {
                Color::srgb(
                    c.r() as f32 / 255.0,
                    c.g() as f32 / 255.0,
                    c.b() as f32 / 255.0,
                )
                .with_alpha(0.85)
            };
            dd.line_on(
                LineLayer::Display,
                Vec3::new(-50.0, 0.0, 0.0),
                Vec3::new(50.0, 0.0, 0.0),
                axis(self.tokens.axis_x),
            );
            dd.line_on(
                LineLayer::Display,
                Vec3::new(0.0, 0.0, -50.0),
                Vec3::new(0.0, 0.0, 50.0),
                axis(self.tokens.axis_z),
            );
        }
        let eye = state.camera.view().transform.0.translation;
        // With icons on, a selected light or empty shows as a filled icon.
        let markers = if state.viewport.view.icons {
            Vec::new()
        } else {
            markers
        };
        crate::icons::draw_helpers(state, &mut dd, accent);
        // Many generated meshes (a scatter's instances): one box, dimmer
        // where hidden, like the outline.
        for (b, root, color) in boxes {
            let faint = color.with_alpha(color.a * 0.3);
            for (layer, c) in [
                (LineLayer::Display, color),
                (LineLayer::DisplayOverlay, faint),
            ] {
                for [p, q] in box_edges(&b, &root) {
                    dd.line_on(layer, p, q, c);
                }
            }
        }
        for (p, color) in markers {
            // A small 3D cross of constant screen size.
            let s = (Vec3::from(eye) - p).length() * 0.03;
            for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                dd.line_on(LineLayer::DisplayOverlay, p - axis * s, p + axis * s, color);
            }
        }
        crate::spline_tool::draw_selected_splines(state, &mut dd, &self.tokens);
        state.draw_gizmo(&mut dd, &self.tokens);
        state.draw_tool_overlay(&mut dd, &self.tokens);
        state.world.insert_resource(dd);
    }
}

/// Generated meshes (prefab contents, rule parts, scatter instances) below
/// one selected entity that are outlined one by one; above this many their
/// combined bounds are drawn as one box instead, so selecting a scatter of
/// thousands of instances does not draw thousands of tiny outlines (walls
/// and building parts, a few to a few dozen per owner, keep theirs).
pub const GENERATED_OUTLINE_LIMIT: usize = 256;

/// How long a capped subtree's bounds box is kept while its generated
/// mesh count is unchanged (instances that move in place, a scatter
/// regenerating with the same count, show within this).
const BOX_REFRESH: std::time::Duration = std::time::Duration::from_millis(250);

/// The bounds of each capped subtree in its root's space, so the box
/// follows the root (a gizmo drag) every frame while the expensive part,
/// one transform per generated mesh, is redone only when the count changes
/// or every [`BOX_REFRESH`].
#[derive(Default)]
pub(crate) struct BoxCache {
    entries: HashMap<Entity, BoxEntry>,
}

struct BoxEntry {
    count: usize,
    at: std::time::Instant,
    local: Option<Aabb>,
}

/// What [`selection_highlight`] asks to draw.
pub(crate) struct Highlight {
    pub outline: SelectionOutline,
    /// Selected entities with no mesh anywhere below them, such as lights
    /// and empties: a marker (position, colour).
    pub markers: Vec<(Vec3, Color)>,
    /// The bounds of the generated meshes below a selected entity that has
    /// more than [`GENERATED_OUTLINE_LIMIT`]: in that entity's space, its
    /// world transform, and the colour.
    pub boxes: Vec<(Aabb, glam::Affine3A, Color)>,
}

/// The selection highlight: a renderer outline around every mesh of each
/// selected entity and its descendants (the primary selection in the accent,
/// the others dimmer), with generated descendants over
/// [`GENERATED_OUTLINE_LIMIT`] shown as one bounds box, and a marker for
/// selected entities with no mesh anywhere below them, such as lights and
/// empties.
///
/// Cost per frame: one walk of each selected subtree (0.2 ms for a scatter
/// of 10,000 instances in a dev build); a capped subtree's box comes from
/// `cache` (one transform), recomputed only when its generated mesh count
/// changes or every [`BOX_REFRESH`] (0.7 ms at 10,000). The renderer
/// outlines at most the limit of generated meshes per selected entity.
pub(crate) fn selection_highlight(
    state: &EditorState,
    accent: Color,
    cache: &mut BoxCache,
) -> Highlight {
    let world = &state.world;
    let primary = state.primary();
    let mut outline = SelectionOutline {
        color: accent,
        secondary_color: accent.with_alpha(0.55),
        ..Default::default()
    };
    // 1.5 points, at least 2 physical pixels.
    if let Some(rect) = state.viewport.rect.filter(|r| r.width() > 0.0) {
        let ppp = state.viewport.request_px[0] as f32 / rect.width();
        outline.width_px = (1.5 * ppp).max(2.0);
    }
    let mut markers = Vec::new();
    let mut boxes = Vec::new();
    // A drawing tool's ghost (a preview owner and its parts) is outlined
    // like a secondary selection.
    for (e, _) in world
        .query::<(box_core::world::Entity, &box_core::derived::Preview)>()
        .iter()
    {
        let mut stack = vec![e];
        while let Some(e) = stack.pop() {
            if world.get::<MeshRenderer>(e).is_some() {
                outline.secondary.push(e);
            }
            stack.extend(world.children(e));
        }
    }
    // A derived part inspected read-only is outlined like a selection.
    let inspected = state.inspected_part();
    let mut generated = Vec::new();
    let mut boxed = Vec::new();
    for &uid in state.selection.iter().chain(inspected.iter()) {
        let Some(entity) = world.entity_by_uid(uid) else {
            continue;
        };
        let is_primary = Some(uid) == primary || Some(uid) == inspected;
        let (list, color) = if is_primary {
            (&mut outline.primary, outline.color)
        } else {
            (&mut outline.secondary, outline.secondary_color)
        };
        let before = list.len();
        generated.clear();
        let mut stack = vec![entity];
        while let Some(e) = stack.pop() {
            if world.get::<MeshRenderer>(e).is_some() {
                if e != entity && box_core::prefab::is_generated(world, e) {
                    generated.push(e);
                } else {
                    list.push(e);
                }
            }
            stack.extend(world.children(e));
        }
        if generated.len() <= GENERATED_OUTLINE_LIMIT {
            list.extend(generated.iter().copied());
        } else if let Some(root) = world.get::<GlobalTransform>(entity).map(|g| g.0) {
            boxed.push(entity);
            let fresh = cache
                .entries
                .get(&entity)
                .is_some_and(|c| c.count == generated.len() && c.at.elapsed() < BOX_REFRESH);
            if !fresh {
                let local = meshes_bounds(world, &generated, &root.inverse());
                cache.entries.insert(
                    entity,
                    BoxEntry {
                        count: generated.len(),
                        at: std::time::Instant::now(),
                        local,
                    },
                );
            }
            if let Some(b) = cache.entries.get(&entity).and_then(|c| c.local) {
                boxes.push((b, root, color));
            }
        }
        if list.len() == before
            && generated.is_empty()
            && let Some(gt) = world.get::<GlobalTransform>(entity)
        {
            markers.push((Vec3::from(gt.0.translation), color));
        }
    }
    cache.entries.retain(|e, _| boxed.contains(e));
    Highlight {
        outline,
        markers,
        boxes,
    }
}

/// The 12 edges of `b` (in the space of `m`) in world space.
fn box_edges(b: &Aabb, m: &glam::Affine3A) -> Vec<[Vec3; 2]> {
    let corner = |i: usize| {
        let p = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        m.transform_point3(p)
    };
    let mut edges = Vec::with_capacity(12);
    for i in 0..8 {
        for bit in [1, 2, 4] {
            if i & bit == 0 {
                edges.push([corner(i), corner(i | bit)]);
            }
        }
    }
    edges
}

/// Bounds of the meshes of `entities` in the space `to` maps world space
/// into (each mesh's own bounds computed once).
fn meshes_bounds(world: &World, entities: &[Entity], to: &glam::Affine3A) -> Option<Aabb> {
    let meshes = world.get_resource::<Assets<MeshData>>()?;
    let mut local = HashMap::new();
    let mut bounds: Option<Aabb> = None;
    for &e in entities {
        let (Some(mr), Some(gt)) = (
            world.get::<MeshRenderer>(e),
            world.get::<GlobalTransform>(e),
        ) else {
            continue;
        };
        let Some(b) = *local
            .entry(mr.mesh)
            .or_insert_with(|| meshes.get(mr.mesh).map(|m| m.bounds()))
        else {
            continue;
        };
        if b.is_empty() {
            continue;
        }
        let w = b.transformed(&(*to * gt.0));
        bounds = Some(match bounds {
            Some(a) => Aabb::new(a.min.min(w.min), a.max.max(w.max)),
            None => w,
        });
    }
    bounds
}
