//! The single toolbar row: File/Edit/View/Game/Help menus (Game: Rebuild
//! and Restart, Project Settings) (File: New/Open
//! Project, Open Recent, New Scene, Open Scene, Save Scene, Save Scene As,
//! Allow Agent Control, Quit; Edit: Undo, Redo, History, ..., Bake to Mesh; Help: Command
//! Palette, Keyboard Shortcuts, Welcome Screen, Documentation), whose items
//! are actions of the registry in `actions.rs` (one `menu_item` line each,
//! so they are also in the command palette), the add-entity
//! menu, the transform tool selector, local/world space, the snap toggle and
//! its steps (click for a popover to edit them), the multi-selection pivot
//! (shown with 2+ selected), the Play controls (centred) and the status area
//! (agent control, Play state, project, scene with dirty marker, draws, fps).
//! Every control has a tooltip with its shortcut from `shortcuts.rs`.

use egui::{Align, Layout, Rect, Ui, UiBuilder, pos2, vec2};

use crate::play::PlayState;
use crate::shortcuts::{self, Command};
use crate::state::{EditorState, NewEntity, Pivot, Space, TransformTool};
use crate::theme::tokens;
use crate::widgets;

/// The entity kinds of the Add menus (toolbar, Edit ▸ Add, hierarchy
/// background), created as roots placed at the viewport centre.
pub(crate) fn add_menu(ui: &mut Ui, state: &mut EditorState) {
    for kind in NewEntity::ALL {
        if kind == NewEntity::Empty {
            generator_menus(ui, state);
        }
        if kind.starts_group() {
            ui.separator();
        }
        let available = kind
            .required_component()
            .is_none_or(|c| state.world.registry().component(c).is_some());
        if ui
            .add_enabled(available, egui::Button::new(kind.label()))
            .clicked()
        {
            state.add_entity(kind, None);
        }
    }
}

/// Add ▸ `<category>` ▸ `<generator>`: every registered generator (engine
/// and game types), one submenu per category (`Generator::CATEGORY`,
/// "Procedural" by default). Nothing when no generator is registered.
fn generator_menus(ui: &mut Ui, state: &mut EditorState) {
    let generators = state.generator_entries();
    let mut categories: Vec<&'static str> = generators.iter().map(|g| g.category).collect();
    categories.dedup();
    if !categories.is_empty() {
        ui.separator();
    }
    for category in categories {
        ui.menu_button(category, |ui| {
            for g in generators.iter().filter(|g| g.category == category) {
                let mut button = ui.button(&g.label);
                if let Some(doc) = g.doc {
                    button = button.on_hover_text(doc);
                }
                if button.clicked() {
                    state.add_generator(g.name);
                }
            }
        });
    }
}

/// File ▸ Open Scene: the project's `*.scene.json` files under `assets/`
/// (listed only while the submenu is open). The current scene is marked.
fn scene_menu(ui: &mut Ui, state: &mut EditorState) {
    let Some(project) = &state.project else {
        return;
    };
    let files = match project.scene_files() {
        Ok(files) => files,
        Err(e) => {
            ui.label(e.to_string());
            return;
        }
    };
    if files.is_empty() {
        ui.add_enabled(false, egui::Button::new("No scenes under assets/"));
    }
    for file in files {
        let current = state.scene.path.as_deref() == Some(file.as_path());
        let rel = project.relative(&file);
        let label = rel.strip_prefix("assets/").unwrap_or(&rel).to_owned();
        if ui
            .add(egui::Button::selectable(current, label))
            .on_hover_text(&rel)
            .clicked()
            && !current
        {
            state.requests.open_scene = Some(file);
        }
    }
}

/// A menu item for the registered action `id`: its current label, its
/// shortcut, a checkbox for toggles, its tooltip, and its disabled reason
/// on hover. Unknown ids draw nothing.
pub(crate) fn menu_item(ui: &mut Ui, state: &mut EditorState, id: &str) {
    let Some(action) = state.actions().get(id).cloned() else {
        return;
    };
    let label = action.label_in(state);
    let available = action.availability(state);
    let enabled = available.is_ok();
    let mut r = match action.checked(state) {
        Some(mut on) => ui.add_enabled(enabled, egui::Checkbox::new(&mut on, label)),
        None => {
            let mut button = egui::Button::new(label);
            if let Some(c) = action.shortcut_command() {
                button = button.shortcut_text(ui.ctx().format_shortcut(shortcuts::main_key(c)));
            }
            ui.add_enabled(enabled, button)
        }
    };
    if let Some(tip) = action.tooltip_text() {
        r = r.on_hover_text(tip);
    }
    if let Err(reason) = &available {
        r = r.on_disabled_hover_text(reason);
    }
    if r.clicked() {
        let _ = state.perform(&action);
    }
}

/// File ▸ Open Recent: the recent projects (the open one marked, missing
/// folders disabled) and Clear Recent Projects.
fn recent_menu(ui: &mut Ui, state: &mut EditorState) {
    let current = state.project.as_ref().map(|p| p.root().to_path_buf());
    if state.recent.entries().is_empty() {
        ui.add_enabled(false, egui::Button::new("No recent projects"));
    }
    let mut open = None;
    for r in state.recent.entries() {
        let is_current = current.as_deref() == Some(r.path.as_path());
        let exists = r.path.join(box_engine::project::PROJECT_FILE).is_file();
        let label = if exists {
            r.name.clone()
        } else {
            format!("{} (missing)", r.name)
        };
        let resp = ui
            .add_enabled(exists, egui::Button::selectable(is_current, label))
            .on_hover_text(r.path.display().to_string())
            .on_disabled_hover_text(format!("{} was moved or deleted", r.path.display()));
        if resp.clicked() && !is_current {
            open = Some(r.path.clone());
        }
    }
    if open.is_some() {
        state.requests.open_recent = open;
    }
    ui.separator();
    menu_item(ui, state, "file.clear_recent");
}

/// Edit ▸ History: the undo steps, most recent first, the current state
/// marked and redo steps dimmed; a click undoes or redoes up to that step.
fn history_menu(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    let entries = state.history_entries();
    if entries.is_empty() {
        ui.add_enabled(false, egui::Button::new("No edits yet"));
        return;
    }
    let mut jump = None;
    egui::ScrollArea::vertical()
        .max_height(420.0)
        .show(ui, |ui| {
            for e in &entries {
                let text = if e.redo {
                    egui::RichText::new(&e.label).color(t.text_weak)
                } else {
                    egui::RichText::new(&e.label)
                };
                let r = ui.add(egui::Button::selectable(e.current, text));
                let r = if e.current {
                    r.on_hover_text("The current state")
                } else if e.redo {
                    r.on_hover_text("Undone: click to redo up to here")
                } else {
                    r.on_hover_text("Click to undo back to here")
                };
                if r.clicked() && !e.current {
                    jump = Some(e.depth);
                }
            }
        });
    if let Some(depth) = jump {
        state.jump_history(depth);
        ui.close();
    }
}

pub fn toolbar_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    let full = ui.max_rect();
    let h = 26.0;
    let row = full.shrink2(vec2(0.0, ((full.height() - h) / 2.0).floor()));
    let mut left = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    // Right edge of the last tool on the left.
    let mut tools_right = row.left();
    {
        let ui = &mut left;
        ui.spacing_mut().item_spacing.x = 2.0;
        // MenuBar takes its row height from interact_size at construction.
        ui.spacing_mut().interact_size.y = h;
        ui.add_space(4.0);
        egui::MenuBar::new().ui(ui, |ui| {
            ui.spacing_mut().button_padding = vec2(8.0, 4.0);
            ui.spacing_mut().interact_size.y = h;
            // Each item is an action of the registry (`actions.rs`), so it
            // is also in the command palette; add one with `menu_item`.
            ui.menu_button("File", |ui| {
                menu_item(ui, state, "file.new_project");
                menu_item(ui, state, "file.open_project");
                menu_item(ui, state, "file.open_example");
                ui.menu_button("Open Recent", |ui| recent_menu(ui, state));
                menu_item(ui, state, "file.new_scene");
                let has_project = state.project.is_some();
                ui.add_enabled_ui(has_project, |ui| {
                    ui.menu_button("Open Scene", |ui| scene_menu(ui, state))
                        .response
                        .on_disabled_hover_text("Open a project to choose one of its scenes");
                });
                ui.separator();
                // Without a file, Save asks where to save (Save As). Never
                // during Play: the world shown is the play world.
                menu_item(ui, state, "file.save");
                menu_item(ui, state, "file.save_as");
                ui.separator();
                menu_item(ui, state, "file.agent_control");
                ui.separator();
                menu_item(ui, state, "file.export");
                ui.separator();
                menu_item(ui, state, "file.quit");
            });
            ui.menu_button("Edit", |ui| {
                // Undo history belongs to the edit world: off during Play.
                menu_item(ui, state, "edit.undo");
                menu_item(ui, state, "edit.redo");
                let playing = state.is_playing();
                ui.add_enabled_ui(!playing, |ui| {
                    ui.menu_button("History", |ui| history_menu(ui, state))
                        .response
                        .on_disabled_hover_text("Stop Play to use the undo history");
                });
                ui.separator();
                menu_item(ui, state, "edit.duplicate");
                menu_item(ui, state, "edit.delete");
                menu_item(ui, state, "edit.select_none");
                menu_item(ui, state, "edit.hide");
                menu_item(ui, state, "edit.show_all");
                menu_item(ui, state, "edit.bake");
                menu_item(ui, state, "edit.hide_part");
                ui.separator();
                ui.menu_button("Add", |ui| add_menu(ui, state));
            });
            ui.menu_button("View", |ui| {
                menu_item(ui, state, "view.grid");
                menu_item(ui, state, "view.frame");
                menu_item(ui, state, "view.scene_settings");
                menu_item(ui, state, "view.rules");
                menu_item(ui, state, "view.reset_camera");
                ui.separator();
                menu_item(ui, state, "view.save_layout");
                menu_item(ui, state, "view.reset_layout");
            });
            ui.menu_button("Game", |ui| {
                menu_item(ui, state, "game.rebuild");
                ui.separator();
                let names = state.settings_types();
                ui.menu_button("Project Settings", |ui| {
                    menu_item(ui, state, "game.export_settings");
                    ui.separator();
                    if names.is_empty() {
                        ui.add_enabled(false, egui::Button::new("No settings types"))
                            .on_disabled_hover_text(
                                "The game registers none: registry.register_settings::<T>() \
                                 for a #[reflect(Resource, Default)] type",
                            );
                    }
                    for name in names {
                        menu_item(ui, state, &format!("settings.{name}"));
                    }
                });
            });
            ui.menu_button("Help", |ui| {
                menu_item(ui, state, "help.palette");
                menu_item(ui, state, "help.shortcuts");
                menu_item(ui, state, "help.welcome");
                menu_item(ui, state, "help.docs");
            });
            ui.add_space(10.0);
            widgets::vsep(ui, h - 8.0);
            ui.add_space(10.0);

            egui::containers::menu::MenuButton::from_button(
                egui::Button::new(egui::RichText::new("+ Add").color(t.text))
                    .fill(t.field)
                    .min_size(vec2(0.0, h)),
            )
            .ui(ui, |ui| add_menu(ui, state))
            .0
            .on_hover_text("Add an object where the viewport centre looks");
            ui.add_space(8.0);

            // The tool group: the transform tools, then the registered
            // viewport tools (`tools.rs`).
            let viewport_tools = state.tools().to_vec();
            let mut labels: Vec<&str> = TransformTool::ALL.map(TransformTool::label).to_vec();
            let mut tips: Vec<String> = TransformTool::ALL
                .map(|tool| {
                    let command = match tool {
                        TransformTool::Translate => Command::Move,
                        TransformTool::Rotate => Command::Rotate,
                        TransformTool::Scale => Command::Scale,
                    };
                    shortcuts::tip(ui.ctx(), tool.label(), command)
                })
                .to_vec();
            for tool in &viewport_tools {
                labels.push(tool.label);
                tips.push(tool.tooltip.to_owned());
            }
            let n = TransformTool::ALL.len();
            let mut tool = match state.active_tool() {
                Some(id) => n + viewport_tools.iter().position(|t| t.id == id).unwrap_or(0),
                None => TransformTool::ALL
                    .iter()
                    .position(|x| *x == state.tool)
                    .unwrap_or(0),
            };
            if widgets::segmented(ui, &mut tool, &labels, &tips, h) {
                match viewport_tools.get(tool.wrapping_sub(n)) {
                    Some(t) if tool >= n => {
                        state.activate_tool(t.id);
                    }
                    _ => state.select_transform_tool(TransformTool::ALL[tool.min(n - 1)]),
                }
            }
            ui.add_space(6.0);
            let space = match state.space {
                Space::World => "World",
                Space::Local => "Local",
            };
            if widgets::text_button(ui, space, h, false)
                .on_hover_text(
                    "Gizmo orientation: World or Local axes, click to switch \
                     (scale always uses local axes)",
                )
                .clicked()
            {
                state.space = match state.space {
                    Space::World => Space::Local,
                    Space::Local => Space::World,
                };
            }
            ui.add_space(2.0);
            let snap = state.snap.enabled;
            let snap_tip = format!(
                "{}; hold {} while dragging a gizmo to invert it",
                shortcuts::tip(ui.ctx(), "Snap", Command::ToggleSnap),
                ui.ctx().format_modifiers(egui::Modifiers::COMMAND),
            );
            if widgets::tool_button(ui, &snap_tip, vec2(h, h), snap, widgets::snap_glyph).clicked()
            {
                state.snap.enabled = !snap;
                state.save_prefs();
            }
            let step = match state.tool {
                TransformTool::Translate => format!("{} m", state.snap.translate),
                TransformTool::Rotate => format!("{}°", state.snap.rotate),
                TransformTool::Scale => format!("{}", state.snap.scale),
            };
            let step_resp = ui
                .add(
                    egui::Button::new(egui::RichText::new(step).font(t.mono()).color(if snap {
                        t.text
                    } else {
                        t.text_faint
                    }))
                    .frame_when_inactive(false)
                    .min_size(vec2(0.0, h)),
                )
                .on_hover_text("Snap steps: click to edit");
            // On the panel colour, so its fields (field colour) stand out.
            egui::Popup::from_toggle_button_response(&step_resp)
                .frame(egui::Frame::popup(&ui.ctx().global_style()).fill(t.surface))
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| snap_popover(ui, state));
            tools_right = step_resp.rect.right();
            // The pivot matters only with several entities selected.
            if state.selection.len() > 1 {
                ui.add_space(6.0);
                let label = match state.pivot {
                    Pivot::Primary => "Pivot: Primary",
                    Pivot::Centre => "Pivot: Centre",
                };
                let r = widgets::text_button(ui, label, h, false).on_hover_text(
                    "Where the gizmo of a multi-selection sits and turns: \
                     the primary selection or the centre of the selection",
                );
                if r.clicked() {
                    state.pivot = match state.pivot {
                        Pivot::Primary => Pivot::Centre,
                        Pivot::Centre => Pivot::Primary,
                    };
                }
                tools_right = r.rect.right();
            }
        });
    }

    // Centred, but never over the tools on the left.
    let play_right = play_controls(ui, state, row, tools_right + 12.0, h);

    // Status, right-aligned; in a narrow window it is clipped rather than
    // drawn over the Play controls.
    let x = (full.center().x + 120.0).max(play_right + 12.0);
    let right = Rect::from_min_max(pos2(x, full.top()), full.max);
    let mut rui = ui.new_child(
        UiBuilder::new()
            .max_rect(right)
            .layout(Layout::right_to_left(Align::Center)),
    );
    rui.set_clip_rect(right.intersect(ui.clip_rect()));
    rui.add_space(t.pad_x + 2.0);
    rui.spacing_mut().item_spacing.x = 14.0;
    let s = state.status;
    rui.label(
        egui::RichText::new(format!("{:>3.0} fps", s.fps))
            .font(t.mono())
            .color(t.text_weak),
    );
    rui.label(
        egui::RichText::new(format!("{} draws", s.draws))
            .font(t.mono())
            .color(t.text_weak),
    );
    let dirty = if state.is_dirty() { " •" } else { "" };
    let scene = match &state.project {
        Some(p) => format!("{}  /  {}{dirty}", p.name(), state.scene.name),
        None => format!("{}{dirty}", state.scene.name),
    };
    rui.label(
        egui::RichText::new(scene)
            .font(t.body())
            .color(if dirty.is_empty() {
                t.text_weak
            } else {
                t.text
            }),
    );
    if state.rebuild_running() {
        rui.label(
            egui::RichText::new("Building…")
                .font(t.strong())
                .color(t.accent),
        )
        .on_hover_text("Rebuild and Restart: the build output is in the Console");
    }
    match state.play_state() {
        PlayState::Editing => {}
        PlayState::Playing => {
            rui.label(
                egui::RichText::new("Playing")
                    .font(t.strong())
                    .color(t.accent),
            );
        }
        PlayState::Paused => {
            rui.label(egui::RichText::new("Paused").font(t.strong()).color(t.warn));
        }
    }
    if let Some(server) = state.agent_control() {
        let clients = server.connections();
        rui.label(egui::RichText::new("Agent").font(t.body()).color(t.accent))
            .on_hover_text(format!(
                "Agent control on: 127.0.0.1:{}, {clients} connection{}. Turn it off in File ▸ Allow Agent Control.",
                server.port(),
                if clients == 1 { "" } else { "s" }
            ));
    }
}

/// Play/Stop, Pause and Step, centred in the toolbar (starting no further
/// left than `min_x`); during Play also the free-camera toggle to their right.
/// Returns the right edge of the controls.
fn play_controls(ui: &mut Ui, state: &mut EditorState, row: Rect, min_x: f32, h: f32) -> f32 {
    let key = |c| shortcuts::text(ui.ctx(), c);
    let (play_k, pause_k, step_k) = (key(Command::Play), key(Command::Pause), key(Command::Step));
    let gap = 2.0;
    let width = 3.0 * h + 2.0 * gap;
    let x = (row.center().x - width / 2.0).max(min_x);
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_max(
                pos2(x, row.center().y - h / 2.0),
                pos2(row.right().max(x + width), row.center().y + h / 2.0),
            ))
            .layout(Layout::left_to_right(Align::Center)),
    );
    cui.spacing_mut().item_spacing.x = gap;
    let play = state.play_state();
    let size = vec2(h, h);
    let playing = play != PlayState::Editing;
    let (tip, glyph): (String, fn(&egui::Painter, Rect, egui::Color32)) = if playing {
        (format!("Stop ({play_k})"), widgets::stop_glyph)
    } else {
        (format!("Play ({play_k})"), widgets::play_glyph)
    };
    if widgets::tool_button(&mut cui, &tip, size, playing, glyph).clicked() {
        state.toggle_play();
    }
    let paused = play == PlayState::Paused;
    let pause_tip = format!("Pause ({pause_k})");
    if cui
        .add_enabled_ui(playing, |ui| {
            widgets::tool_button(ui, &pause_tip, size, paused, widgets::pause_glyph)
        })
        .inner
        .clicked()
    {
        state.toggle_pause();
    }
    let step_tip = format!("Step one frame ({step_k})");
    if cui
        .add_enabled_ui(playing, |ui| {
            widgets::tool_button(ui, &step_tip, size, false, widgets::step_glyph)
        })
        .inner
        .clicked()
    {
        state.step_play();
    }
    if playing {
        cui.add_space(8.0);
        let free = state.free_camera();
        if widgets::text_button(&mut cui, "Free Camera", h, free)
            .on_hover_text(
                "Look at the play world through the editor camera (orbit, pan and zoom as \
                 while editing; click again for the game's camera)",
            )
            .clicked()
        {
            state.set_free_camera(!free);
        }
    }
    cui.min_rect().right()
}

/// The snap popover: the steps of the three tools and the toggle. The steps
/// are kept per project in `.box/editor.json` (`prefs.rs`), written when a
/// value is committed (typed, or at the end of a drag), not per frame.
pub(crate) fn snap_popover(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    ui.set_min_width(200.0);
    ui.label(
        egui::RichText::new("Snap steps")
            .font(t.strong())
            .color(t.text_strong),
    );
    ui.add_space(4.0);
    let mut save = false;
    egui::Grid::new("box_editor_snap_steps")
        .num_columns(2)
        .spacing(vec2(12.0, 6.0))
        .show(ui, |ui| {
            let snap = &mut state.snap;
            let rows: [(&str, &mut f32, f64, &str, f32); 3] = [
                ("Move", &mut snap.translate, 0.01, " m", 0.001),
                ("Rotate", &mut snap.rotate, 0.5, "°", 0.01),
                ("Scale", &mut snap.scale, 0.01, "", 0.001),
            ];
            for (label, value, speed, suffix, min) in rows {
                ui.label(label);
                let r = ui.add(
                    egui::DragValue::new(value)
                        .speed(speed)
                        .range(min..=1000.0)
                        .max_decimals(3)
                        .suffix(suffix)
                        .min_size(vec2(90.0, 0.0)),
                );
                save |= r.drag_stopped() || (r.changed() && !r.dragged());
                ui.end_row();
            }
        });
    ui.add_space(4.0);
    let tip = shortcuts::tip(ui.ctx(), "Snap", Command::ToggleSnap);
    if crate::widgets::checkbox_labeled(ui, &mut state.snap.enabled, &tip).changed() {
        save = true;
    }
    if save {
        state.save_prefs();
    }
}
