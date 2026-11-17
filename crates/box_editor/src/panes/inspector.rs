//! Inspector pane: reflection-driven sections for every registered component
//! of the selection (or the scene's resources when nothing is selected, or a
//! project settings resource from Game ▸ Project Settings),
//! built from `crate::inspect` and the widget kit.
//!
//! With several entities selected it shows the components they all share.
//! A field whose values differ shows "—" (per lane for vectors and
//! rotations); editing it sets it on every selected entity (for a vector
//! lane, only that lane), and Add / Remove Component apply to all of them,
//! each as one undo step. The name is not edited across a multi-selection;
//! the header shows the count.
//!
//! Every edit goes through the `EditStack`. A continuous edit (drag, slider,
//! colour picker, typing) opens a transaction on its first change and commits
//! when the pointer is released and no field has keyboard focus, so it is
//! one undo step. Undo, redo, save and the other discrete edits end it first
//! (`EditorState::finish_edit`); Escape during a drag cancels it.
//!
//! Section headers use friendly names ("Standard Material", the type name in
//! the tooltip). Add Component is pinned below the scrolling sections and
//! opens a searchable list grouped by category
//! (`inspect::component_category`); Enter adds the first match.
//!
//! Nested structs (`StandardMaterial.wind`, `Environment.bloom`, list items
//! such as a scatter's species) are folding headings ([`rows_ui`]): closed
//! by default, open or closed per type and field path for the session
//! ([`fold_open`]). A heading shows the modified dot and ↺ when the struct
//! differs from its default, "—" while closed when its fields differ across
//! the selection, and the struct's `enabled` bool as a check box; its
//! label's context menu resets, copies and pastes the whole struct.
//!
//! Reset, copy and paste (`values.rs`): a value that differs from the
//! registry default has a dot before a brighter label and a ↺ while
//! hovered; right-clicking a field's label offers Reset to Default, Copy
//! Value and Paste Value (same type only); a section's ⋯ menu offers Reset,
//! Copy and Paste Values for the whole component, and Paste as New
//! Component appears under Add Component when the clipboard holds one the
//! selection lacks. Each reset or paste is one undo step for every selected
//! entity. During Play both menus also offer "Keep after Stop" (marked rows
//! get an accent bar; the Play banner counts them).
//!
//! A Material Ref section has Edit (show the material asset) and Unlink
//! (copy its values into the inline material); an inline material that a
//! link overrides says so.

use box_core::color::Color;
use box_core::identity::{Name, Uid};
use box_core::prelude::Entity;
use box_core::reflect::{PartialReflect, Value};
use egui::{Align, Align2, Layout, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use glam::{IVec2, IVec3, Quat, UVec2, UVec3, Vec2, Vec3, Vec4};

pub(crate) use crate::inspect::EulerEdit;
use crate::inspect::{self, FieldKind, FieldRow, Section};
use crate::state::EditorState;
use crate::theme::{Tokens, tokens};
use crate::values;
use crate::widgets::{self, MIXED};

/// What the panel asks for this frame; applied after drawing.
pub(crate) enum Op {
    Set {
        path: String,
        value: Value,
        /// For vector rows: the edited lanes (mask) and the edited vector.
        /// Other selected entities keep their own values in the other lanes.
        lanes: Option<(u8, Vec<f32>)>,
    },
    SwitchVariant {
        path: String,
        current: Value,
        variant: &'static str,
    },
    Add(&'static str),
    Remove(&'static str),
    /// Reset the field (or whole component) at this path to its registry
    /// default; one undo step for every target.
    Reset(String),
    /// Copy the value at this path (whole components as components).
    Copy(String),
    /// Paste the clipboard's value into the field at this path
    /// (type-checked); one undo step for every target.
    Paste(String),
    /// Paste the clipboard's component over this component's values.
    PasteComponent(&'static str),
    /// Add the clipboard's component to the targets that lack it.
    PasteNewComponent,
    /// During Play: keep (true) or stop keeping the value at this path when
    /// Play stops.
    Keep(String, bool),
    /// Material Ref section: show the linked material asset.
    EditMaterial(box_core::asset_ref::AssetRef),
    /// Material Ref section: copy the linked values inline, drop the link.
    Unlink,
}

fn open_id(name: &str) -> egui::Id {
    egui::Id::unique(("box_editor_inspector_open", name))
}

/// The entities the inspector edits: the existing selected ones, primary
/// last; empty when there is no primary selection.
fn targets(state: &EditorState) -> Vec<(Uid, Entity)> {
    if state.primary().is_none() {
        return Vec::new();
    }
    state
        .selection
        .iter()
        .filter_map(|u| state.world.entity_by_uid(*u).map(|e| (*u, e)))
        .collect()
}

pub fn inspector_ui(ui: &mut Ui, state: &mut EditorState) {
    // An asset picked in the browser (until the entity selection changes).
    if state.inspected_asset().is_some() {
        crate::panes::asset_inspector::asset_inspector_ui(ui, state);
        return;
    }
    // Game ▸ Project Settings (until the selection changes).
    if let Some(name) = state.project_settings_shown() {
        settings_ui(ui, state, name);
        return;
    }
    let t = tokens(ui.ctx());
    // A derived part inspected read-only (Cmd+Alt+click; `generated.rs`).
    if let Some(part) = state.inspected_part() {
        egui::ScrollArea::vertical()
            .id_salt("inspector_part")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add_space(8.0);
                crate::generated::part_inspector_ui(ui, state, part, &t);
                ui.add_space(12.0);
            });
        return;
    }
    let mut ops = Vec::new();
    let targets = targets(state);
    // Paste targets are shown: the windowed runner keeps the system
    // clipboard's text fresh (at most twice a second).
    state.clipboard_mut().wanted = true;
    let entities: Vec<Entity> = targets.iter().map(|(_, e)| *e).collect();
    // Add Component is pinned below the scrolling sections, so it is in
    // view however many components (or open sections) there are.
    if !targets.is_empty() {
        egui::Panel::bottom("box_editor_inspector_footer")
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| add_component(ui, state, &entities, &t, &mut ops));
    }
    egui::ScrollArea::vertical()
        .id_salt("inspector")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(8.0);
            crate::feedback::play_banner(ui, state, &t);
            match targets.as_slice() {
                [] => {}
                [(uid, e)] => {
                    header(ui, state, *uid, *e, &t, &mut ops);
                    // Rule warnings first, where they are seen.
                    crate::generated::owner_warnings_ui(ui, state, *e, &t);
                }
                many => multi_header(ui, state, many, &t),
            }
            if !targets.is_empty() {
                for section in inspect::shared_sections(&state.world, &entities) {
                    section_ui(ui, state, &section, true, &t, &mut ops);
                }
                if let [e] = entities.as_slice() {
                    crate::feedback::unknown_components_ui(ui, state, *e, &t);
                    // What rules made from this entity (read-only).
                    crate::generated::generated_section_ui(ui, state, *e, &t);
                }
            } else {
                scene_header(ui, state, &t);
                let sections = inspect::resource_sections(&state.world);
                if sections.is_empty() {
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("No scene settings").color(t.text_weak));
                    });
                }
                for section in sections {
                    section_ui(ui, state, &section, false, &t, &mut ops);
                }
            }
            ui.add_space(12.0);
        });
    let uids: Vec<Uid> = targets.iter().map(|(u, _)| *u).collect();
    apply(ui, state, &uids, ops);
}

/// A project settings resource (`project_settings.rs`): its fields, edited
/// in its file (no undo; shared by every scene).
fn settings_ui(ui: &mut Ui, state: &mut EditorState, name: &'static str) {
    let t = tokens(ui.ctx());
    let mut ops = Vec::new();
    let registry = state.world.registry().clone();
    let value = state
        .world
        .get_resource_reflect(name)
        .ok()
        .and_then(|v| box_core::reflect::clone_value(v.as_partial_reflect()).ok());
    egui::ScrollArea::vertical()
        .id_salt("inspector_settings")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(8.0);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            ui.painter().text(
                pos2(r.left() + t.pad_x + 4.0, r.center().y),
                Align2::LEFT_CENTER,
                format!("Project Settings · {}", inspect::friendly_name(name)),
                t.strong(),
                t.text_strong,
            );
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            ui.painter().text(
                pos2(r.left() + t.pad_x + 4.0, r.center().y),
                Align2::LEFT_CENTER,
                format!(
                    "assets/{} · all scenes · no undo",
                    box_engine::settings::settings_file(name)
                ),
                t.small(),
                t.text_weak,
            );
            ui.add_space(6.0);
            match &value {
                Some(value) => {
                    let section = Section {
                        component: name,
                        doc: registry.resource(name).and_then(|r| r.doc()),
                        rows: inspect::rows(&registry, name, value.as_ref()),
                    };
                    section_ui(ui, state, &section, false, &t, &mut ops);
                }
                None => {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Not loaded (open a project)").color(t.text_weak),
                        );
                    });
                }
            }
            ui.add_space(12.0);
        });
    let now = ui.input(|i| i.time);
    for op in ops {
        match op {
            Op::Set { path, value, .. } => {
                state.edit_settings(&path, value.as_ref(), now);
            }
            Op::SwitchVariant {
                path,
                current,
                variant,
            } => match inspect::switch_variant(&registry, current.as_ref(), variant) {
                Some(v) => {
                    state.edit_settings(&path, v.as_ref(), now);
                }
                None => tracing::warn!("cannot switch {path} to {variant}"),
            },
            // Reset, copy and paste work on the settings file like any edit.
            Op::Reset(path) => match values::field_default(&registry, &path) {
                Some(default) => {
                    state.edit_settings(&path, default.as_ref(), now);
                }
                None => tracing::warn!("{path} has no default value"),
            },
            Op::Copy(path) => {
                let field = value
                    .as_ref()
                    .and_then(|v| values::field_of(v.as_ref(), &path));
                let text = field.and_then(|f| {
                    if path.contains(['.', '[']) {
                        values::copy_value_text(&registry, f.as_ref()).ok()
                    } else {
                        values::copy_component_text(&registry, &path, f.as_ref()).ok()
                    }
                });
                if let Some(text) = text {
                    ui.ctx().copy_text(text);
                }
            }
            Op::Paste(path) => {
                let like = value
                    .as_ref()
                    .and_then(|v| values::field_of(v.as_ref(), &path));
                let pasted = match (state.clipboard.clip(), like) {
                    (Some(clip), Some(like)) => {
                        values::paste_value(&registry, &clip, like.as_ref())
                    }
                    (None, _) => Err("the clipboard holds no value".to_owned()),
                    (_, None) => Err(format!("{path} cannot be read")),
                };
                match pasted {
                    Ok(v) => {
                        state.edit_settings(&path, v.as_ref(), now);
                    }
                    Err(e) => tracing::warn!("cannot paste into {path}: {e}"),
                }
            }
            Op::PasteComponent(c) => {
                let pasted = match state.clipboard.clip() {
                    Some(values::Clip::Component {
                        component,
                        value: json,
                    }) if component == c => registry
                        .resource(c)
                        .ok_or_else(|| format!("{c} is not a resource"))
                        .and_then(|r| {
                            registry
                                .value_from_json(r.type_id(), &json)
                                .map_err(|e| e.to_string())
                        }),
                    _ => Err(format!("the clipboard holds no {c}")),
                };
                match pasted {
                    Ok(v) => {
                        state.edit_settings(c, v.as_ref(), now);
                    }
                    Err(e) => tracing::warn!("cannot paste {c}: {e}"),
                }
            }
            // No components to add or remove, and settings edits already
            // persist after Play: keep-after-Stop does not apply.
            Op::Add(_)
            | Op::Remove(_)
            | Op::PasteNewComponent
            | Op::Keep(..)
            | Op::EditMaterial(_)
            | Op::Unlink => {}
        }
    }
}

/// Applies the frame's ops to `targets` (primary last), or to the scene's
/// resources when there are none.
fn apply(ui: &Ui, state: &mut EditorState, targets: &[Uid], ops: Vec<Op>) {
    for op in ops {
        let op = match op {
            // Several entities: each switches its own value, keeping its
            // own matching fields.
            Op::SwitchVariant { path, variant, .. } if targets.len() > 1 => {
                begin_inspector_edit(state, targets, &path);
                for &u in targets {
                    let current = state
                        .world
                        .entity_by_uid(u)
                        .zip(state.world.field_path(&path).ok())
                        .and_then(|(e, p)| state.world.get_field(e, &p).ok());
                    let switched = current.and_then(|c| {
                        inspect::switch_variant(state.world.registry(), c.as_ref(), variant)
                    });
                    match switched {
                        Some(v) => {
                            state.set_field(u, &path, v.as_ref());
                        }
                        None => tracing::warn!("cannot switch {path} to {variant}"),
                    }
                }
                continue;
            }
            Op::SwitchVariant {
                path,
                current,
                variant,
            } => match inspect::switch_variant(state.world.registry(), current.as_ref(), variant) {
                Some(value) => Op::Set {
                    path,
                    value,
                    lanes: None,
                },
                None => {
                    tracing::warn!("cannot switch {path} to {variant}");
                    continue;
                }
            },
            op => op,
        };
        match op {
            Op::Set { path, value, lanes } => {
                begin_inspector_edit(state, targets, &path);
                if targets.is_empty() {
                    state.set_resource_field(&path, value.as_ref());
                } else {
                    let lanes = lanes.as_ref().map(|(m, v)| (*m, v.as_slice()));
                    state.set_field_on(targets, &path, value.as_ref(), lanes);
                }
            }
            Op::Add(c) if !targets.is_empty() => state.add_component_to(targets, c),
            Op::Remove(c) if !targets.is_empty() => state.remove_component_from(targets, c),
            Op::Reset(path) => {
                state.reset_field(targets, &path);
            }
            Op::Copy(path) => {
                let whole = !path.contains(['.', '[']);
                let text = if whole {
                    state.copy_component(targets, &path)
                } else {
                    state.copy_field(targets, &path)
                };
                if let Some(text) = text {
                    ui.ctx().copy_text(text);
                }
            }
            Op::Paste(path) => {
                state.paste_field(targets, &path);
            }
            Op::PasteComponent(c) => {
                state.paste_component_values(targets, c);
            }
            Op::PasteNewComponent if !targets.is_empty() => {
                state.paste_as_new_component(targets);
            }
            Op::Keep(path, true) => {
                state.keep_after_stop(targets, &path);
            }
            Op::Keep(path, false) => state.forget_kept(targets, &path),
            Op::EditMaterial(r) => state.edit_linked_material(&r),
            Op::Unlink if !targets.is_empty() => {
                state.unlink_material(targets);
            }
            _ => {}
        }
    }
    // Close a continuous edit once nothing is being dragged or typed.
    let idle = ui.input(|i| !i.pointer.any_down()) && ui.ctx().memory(|m| m.focused().is_none());
    if idle && state.inspector_edit.is_some() {
        state.finish_edit();
    }
}

/// Opens the inspector's transaction for an edit of `path`: a new edit, or
/// one whose transaction was closed under it (undo, redo or save in the
/// middle of a drag). A continuing edit of the same field merges.
fn begin_inspector_edit(state: &mut EditorState, targets: &[Uid], path: &str) {
    if state.inspector_edit.as_deref() == Some(path) && state.edits.is_open() {
        return;
    }
    let subject = match targets {
        [] => None,
        [u] => state.world.entity_by_uid(*u).map(|e| state.display_name(e)),
        many => Some(format!("{} entities", many.len())),
    };
    state.begin_edit(inspect::edit_label(subject.as_deref(), path));
    state.inspector_edit = Some(path.to_owned());
}

/// The header of a multi-selection: the count (names are not edited
/// across several entities) and the primary selection's name.
fn multi_header(ui: &mut Ui, state: &EditorState, targets: &[(Uid, Entity)], t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    ui.painter().text(
        pos2(r.left() + t.pad_x + 8.0, r.center().y),
        Align2::LEFT_CENTER,
        format!("{} entities", targets.len()),
        t.strong(),
        t.text_strong,
    );
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    if let Some((_, e)) = targets.last() {
        ui.painter().text(
            pos2(r.left() + t.pad_x + 8.0, r.center().y),
            Align2::LEFT_CENTER,
            format!("Primary: {}  ·  shared components", state.display_name(*e)),
            t.small(),
            t.text_weak,
        );
    }
    ui.add_space(6.0);
}

fn header(
    ui: &mut Ui,
    state: &EditorState,
    uid: Uid,
    e: box_core::prelude::Entity,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let mut hui = ui.new_child(
        UiBuilder::new()
            .max_rect(r.shrink2(vec2(t.pad_x + 2.0, 1.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let mut name = state
        .world
        .get::<Name>(e)
        .map(|n| n.0.clone())
        .unwrap_or_default();
    let resp = hui.add(
        egui::TextEdit::singleline(&mut name)
            .font(t.strong())
            .hint_text("Name")
            .margin(egui::Margin::symmetric(6, 4))
            .desired_width(hui.available_width()),
    );
    if resp.changed() {
        ops.push(Op::Set {
            path: "Name".into(),
            value: Box::new(Name::new(name)),
            lanes: None,
        });
    }
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    ui.painter().text(
        pos2(r.right() - t.pad_x - 2.0, r.center().y),
        Align2::RIGHT_CENTER,
        format!("uid {}", short_uid(uid)),
        t.mono(),
        t.text_faint,
    );
    ui.add_space(6.0);
}

fn short_uid(uid: Uid) -> String {
    let s = uid.to_string();
    s[s.len() - 12..].to_owned()
}

fn scene_header(ui: &mut Ui, state: &EditorState, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().text(
        pos2(r.left() + t.pad_x + 4.0, r.center().y),
        Align2::LEFT_CENTER,
        format!("Scene · {}", state.scene.name),
        t.strong(),
        t.text_strong,
    );
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    ui.painter().text(
        pos2(r.left() + t.pad_x + 4.0, r.center().y),
        Align2::LEFT_CENTER,
        "Sky, fog, atmosphere, exposure, post-processing, wind",
        t.small(),
        t.text_weak,
    );
    ui.add_space(6.0);
}

pub(crate) fn section_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    section: &Section,
    removable: bool,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let id = open_id(section.component);
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
    let component = section.component;
    let can_remove = removable && component != "Transform";
    let title = inspect::friendly_name(component);
    let tip = match section.doc {
        Some(doc) => format!("{component}\n\n{doc}"),
        None => component.to_owned(),
    };
    // What the ⋯ menu offers, worked out before it borrows nothing.
    let registry = state.world.registry().clone();
    let has_default = values::target_default(&registry, component).is_some();
    let clip = state.clipboard().clip();
    let pastable = clip.as_ref().is_some_and(|c| match removable {
        true => values::paste_component(&registry, c, Some(component)).is_ok(),
        false => state
            .field_value(&[], component)
            .is_some_and(|like| values::paste_value(&registry, c, like.as_ref()).is_ok()),
    });
    let keep = keep_state(state, component);
    let mut choice = None;
    widgets::section_with_menu(
        ui,
        &title,
        Some(&tip),
        &mut open,
        Some(|ui: &mut Ui| {
            choice = section_menu(
                ui,
                &title,
                component,
                has_default,
                pastable,
                can_remove,
                keep,
            );
        }),
        |ui| {
            let shader = section.component == "ShaderMaterial";
            if matches!(component, "StandardMaterial" | "ShaderMaterial")
                && linked_material(state).is_some()
            {
                widgets::field_row(ui, "", 0.0, None, |ui| {
                    ui.label(
                        egui::RichText::new("Overridden by Material Ref")
                            .font(t.small())
                            .color(t.text_weak),
                    )
                    .on_hover_text(
                        "The linked material asset is drawn. These values are kept and \
                         drawn again when the link is cleared; Unlink replaces them with \
                         the asset's values.",
                    );
                });
            }
            rows_ui(ui, state, &section.rows, t, ops, |ui, state, row, ops| {
                if shader && row.path == "ShaderMaterial.params" {
                    crate::panes::asset_inspector::shader_params_ui(
                        ui, state, section, row, t, ops,
                    );
                    true
                } else if row.path == "Script.properties" || row.path == "ScriptIntent.properties" {
                    crate::script_props::properties_ui(ui, state, section, row, t, ops);
                    true
                } else {
                    false
                }
            });
            if shader {
                crate::panes::asset_inspector::shader_error_ui(ui, state, section, t);
            }
            if component == "MaterialRef" {
                material_ref_actions(ui, state, section, t, ops);
            }
            crate::feedback::asset_error_note(ui, state, component, t);
        },
    );
    if let Some(op) = choice {
        ops.push(op);
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, open));
}

/// The ⋯ menu of a section: reset, copy and paste of the whole component
/// (or resource), Remove, and during Play "Keep after Stop".
fn section_menu(
    ui: &mut Ui,
    title: &str,
    component: &'static str,
    has_default: bool,
    pastable: bool,
    can_remove: bool,
    keep: Option<bool>,
) -> Option<Op> {
    let mut op = None;
    if ui
        .add_enabled(has_default, egui::Button::new(format!("Reset {title}")))
        .on_hover_text("Every value back to its default")
        .clicked()
    {
        op = Some(Op::Reset(component.to_owned()));
    }
    if ui.button(format!("Copy {title}")).clicked() {
        op = Some(Op::Copy(component.to_owned()));
    }
    if ui
        .add_enabled(pastable, egui::Button::new(format!("Paste {title} Values")))
        .on_disabled_hover_text(format!("Copy a {title} first"))
        .clicked()
    {
        op = Some(Op::PasteComponent(component));
    }
    if let Some(kept) = keep {
        ui.separator();
        let mut on = kept;
        if widgets::checkbox_labeled(ui, &mut on, "Keep after Stop")
            .on_hover_text("Apply this component's values to the edit scene when Play stops")
            .changed()
        {
            op = Some(Op::Keep(component.to_owned(), on));
        }
    }
    if can_remove {
        ui.separator();
        if ui.button(format!("Remove {title}")).clicked() {
            op = Some(Op::Remove(component));
        }
    }
    if op.is_some() {
        ui.close();
    }
    op
}

/// During Play with entities in the inspector: whether `path` of the
/// primary is kept after Stop (`None` when the option does not apply).
fn keep_state(state: &EditorState, path: &str) -> Option<bool> {
    if !state.is_playing() || state.assets.inspect.is_some() || state.settings_view.is_some() {
        return None;
    }
    let uid = state.primary()?;
    Some(state.is_kept(uid, path))
}

/// The primary's linked material reference, when it has a non-empty one.
fn linked_material(state: &EditorState) -> Option<box_core::asset_ref::AssetRef> {
    let e = state.primary_entity()?;
    let r = state
        .world
        .get::<box_core::mesh_asset::MaterialRef>(e)?
        .material
        .clone();
    r.is_some().then_some(r)
}

/// The Material Ref section's buttons: Edit (show the material asset in
/// the inspector) and Unlink (copy its values into the inline material).
fn material_ref_actions(
    ui: &mut Ui,
    state: &EditorState,
    section: &Section,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let r = section
        .rows
        .iter()
        .find(|r| r.path == "MaterialRef.material")
        .and_then(|r| {
            r.value
                .try_downcast_ref::<box_core::asset_ref::AssetRef>()
                .cloned()
        })
        .unwrap_or_default();
    let mixed = section.rows.iter().any(|r| r.mixed.any());
    let linked = r.is_some() && !mixed;
    let loaded = linked && state.ref_status(&r) == crate::assets::RefStatus::Loaded;
    ui.add_space(2.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let mut bui = ui.new_child(
        UiBuilder::new()
            .max_rect(row.shrink2(vec2(t.pad_x + 2.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
    );
    if bui
        .add_enabled(
            loaded,
            egui::Button::new("Unlink").min_size(vec2(70.0, 24.0)),
        )
        .on_hover_text(
            "Copy the material's values into this entity's own material and drop the link",
        )
        .on_disabled_hover_text("Needs a loaded material")
        .clicked()
    {
        ops.push(Op::Unlink);
    }
    if bui
        .add_enabled(
            linked && state.asset_server().is_some(),
            egui::Button::new("Edit").min_size(vec2(70.0, 24.0)),
        )
        .on_hover_text("Show the material asset in the inspector; edits are saved to its file")
        .clicked()
    {
        ops.push(Op::EditMaterial(r));
    }
}

fn add_component(
    ui: &mut Ui,
    state: &EditorState,
    entities: &[Entity],
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let addable = match entities {
        [e] => inspect::addable_components(&state.world, *e),
        many => inspect::addable_to_any(&state.world, many),
    };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, t.border));
    ui.add_space(FOOTER_PAD);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let mut bui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(t.pad_x + 2.0, 0.0))));
    let w = bui.available_width();
    bui.add_enabled_ui(!addable.is_empty(), |ui| {
        let button = ui.add(egui::Button::new("Add Component").min_size(vec2(w, 26.0)));
        if button.clicked() {
            // A fresh search, focused, each time the list opens.
            ui.data_mut(|d| {
                d.insert_temp(egui::Id::unique(QUERY_ID), String::new());
                d.insert_temp(egui::Id::unique(FOCUS_ID), true);
            });
        }
        // On the panel colour, so the search field (field colour) stands
        // out; above the button, which sits at the bottom of the inspector.
        egui::Popup::from_toggle_button_response(&button)
            .align(egui::RectAlign::TOP_START)
            .frame(egui::Frame::popup(&ui.ctx().global_style()).fill(t.surface))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(w.max(220.0))
            .show(|ui| add_component_list(ui, state, &addable, t, ops));
    });
    // A component copied from another entity (Copy in a section's ⋯ menu),
    // when the selection lacks it.
    let uids: Vec<Uid> = entities
        .iter()
        .filter_map(|e| state.world.uid(*e))
        .collect();
    if let Some(name) = state.pastable_new_component(&uids) {
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let mut bui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(t.pad_x + 2.0, 0.0))));
        let w = bui.available_width();
        let label = format!("Paste {} as New Component", inspect::friendly_name(name));
        if bui
            .add(egui::Button::new(label).min_size(vec2(w, 26.0)))
            .on_hover_text("Add the copied component, with its values")
            .clicked()
        {
            ops.push(Op::PasteNewComponent);
        }
    }
    ui.add_space(FOOTER_PAD);
}

/// Space above and below the pinned Add Component button.
const FOOTER_PAD: f32 = 8.0;

const QUERY_ID: &str = "box_editor_add_component_query";
const FOCUS_ID: &str = "box_editor_add_component_focus";

/// The Add Component popup: a search field (focused when it opens; Enter
/// adds the first match) over the addable components by category, with
/// friendly names, the type name on the right and the doc comment on hover.
fn add_component_list(
    ui: &mut Ui,
    state: &EditorState,
    addable: &[&'static str],
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let mut query: String = ui
        .data(|d| d.get_temp(egui::Id::unique(QUERY_ID)))
        .unwrap_or_default();
    let focus = ui
        .data_mut(|d| d.remove_temp::<bool>(egui::Id::unique(FOCUS_ID)))
        .unwrap_or(false);
    let r = ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("Search components")
            .desired_width(f32::INFINITY),
    );
    if focus {
        r.request_focus();
    }
    let registry = state.world.registry();
    let choices = inspect::component_choices(registry, addable, &query);
    let mut add = None;
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        add = choices.first().map(|c| c.name);
    }
    ui.add_space(4.0);
    if choices.is_empty() {
        ui.label(egui::RichText::new("No matching components").color(t.text_weak));
    }
    egui::ScrollArea::vertical()
        .max_height(320.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let mut category = None;
            for c in &choices {
                if category != Some(c.category) {
                    category = Some(c.category);
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(c.category.label())
                            .font(t.strong_small())
                            .color(t.text_weak),
                    );
                }
                let mut b = ui.add(
                    egui::Button::new(c.label.as_str())
                        .right_text(egui::RichText::new(c.name).color(t.text_faint))
                        .frame_when_inactive(false)
                        .min_size(vec2(ui.available_width(), 0.0)),
                );
                if let Some(d) = registry.component(c.name).and_then(|c| c.doc()) {
                    b = b.on_hover_text(d);
                }
                if b.clicked() {
                    add = Some(c.name);
                }
            }
        });
    ui.data_mut(|d| d.insert_temp(egui::Id::unique(QUERY_ID), query));
    if let Some(name) = add {
        ops.push(Op::Add(name));
        ui.close();
    }
}

fn fold_id(path: &str) -> egui::Id {
    egui::Id::unique(("box_editor_inspector_fold", path))
}

/// Whether the fields of the nested struct at `path` are shown: closed
/// by default; the choice is remembered per type and field path
/// (`Environment.bloom`, `StandardMaterial.wind`) for the session, so it
/// holds for every entity, scene and selection.
pub fn fold_open(ctx: &egui::Context, path: &str) -> bool {
    ctx.data(|d| d.get_temp::<bool>(fold_id(path)))
        .unwrap_or(false)
}

/// Opens or closes the nested struct at `path` (see [`fold_open`]).
pub fn set_fold_open(ctx: &egui::Context, path: &str, open: bool) {
    ctx.data_mut(|d| d.insert_temp(fold_id(path), open));
}

/// A row that folds: a nested struct, list items included (a scatter's
/// species).
fn foldable(row: &FieldRow) -> bool {
    matches!(row.kind, FieldKind::Struct)
}

/// The rows of a section. Nested structs are folding headings (closed by
/// default, see [`fold_open`]); the fields of closed lists and structs are
/// skipped. A struct's `enabled` switch is shown in its heading instead of
/// as a row. `custom` draws a row itself when it returns true.
pub(crate) fn rows_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    rows: &[FieldRow],
    t: &Tokens,
    ops: &mut Vec<Op>,
    mut custom: impl FnMut(&mut Ui, &mut EditorState, &FieldRow, &mut Vec<Op>) -> bool,
) {
    let mut hidden: Option<String> = None;
    let mut in_heading: Vec<String> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if let Some(prefix) = &hidden {
            if row.path.starts_with(prefix.as_str()) {
                continue;
            }
            hidden = None;
        }
        if in_heading.contains(&row.path) {
            continue;
        }
        if let FieldKind::List { len } = row.kind
            && !list_open(ui.ctx(), &row.path, len)
        {
            hidden = Some(format!("{}[", row.path));
        }
        if foldable(row) {
            let prefix = format!("{}.", row.path);
            let n = rows[i + 1..]
                .iter()
                .take_while(|r| r.path.starts_with(prefix.as_str()))
                .count();
            let fields = &rows[i + 1..i + 1 + n];
            let switch = fields
                .iter()
                .find(|r| r.kind == FieldKind::Bool && r.path[prefix.len()..] == *"enabled");
            if let Some(s) = switch {
                in_heading.push(s.path.clone());
            }
            if !struct_heading_ui(ui, state, row, fields, switch, t, ops) {
                hidden = Some(prefix);
            }
            continue;
        }
        if custom(ui, state, row, ops) {
            // A custom row draws its list's items itself.
            hidden = Some(format!("{}[", row.path));
        } else {
            row_ui(ui, state, row, t, ops);
        }
    }
}

/// The heading of a nested struct: a chevron and the label (a click
/// anywhere on the row opens or closes it), the struct's `enabled` switch
/// when it has one, and "—" when its fields differ across the selection.
/// A value that differs from its default gets the modified dot and ↺; the
/// label's context menu resets, copies and pastes the whole struct. In a
/// disabled (read-only) inspector it is a plain, open heading. Returns
/// whether its fields are shown.
fn struct_heading_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    row: &FieldRow,
    fields: &[FieldRow],
    switch: Option<&FieldRow>,
    t: &Tokens,
    ops: &mut Vec<Op>,
) -> bool {
    let editable = ui.is_enabled();
    let open = !editable || fold_open(ui.ctx(), &row.path);
    let indent = row.depth as f32 * 12.0;
    let label = row_label(row);
    let mixed = fields
        .iter()
        .any(|r| r.mixed.any() && Some(r.path.as_str()) != switch.map(|s| s.path.as_str()));
    let switch_mixed = switch.is_some_and(|s| s.mixed.any());
    let registry = state.world.registry().clone();
    let modified = editable
        && !mixed
        && !switch_mixed
        && values::differs_from_default(&registry, &row.path, row.value.as_ref()) == Some(true);
    let keep = keep_state(state, &row.path);
    let mark = widgets::RowMark {
        modified,
        kept: keep == Some(true),
        fold: editable.then_some(open),
    };
    let what = if open {
        "Click to hide its fields"
    } else {
        "Click to show its fields"
    };
    let tip = match row.meta.as_ref().and_then(|m| m.doc) {
        Some(doc) => format!("{doc}\n\n{what}"),
        None => what.to_owned(),
    };
    let (toggled, resp) = widgets::field_row_ex(ui, &label, indent, Some(&tip), mark, |ui| {
        if let Some(s) = switch {
            let mut on = s.value.try_downcast_ref::<bool>().copied().unwrap_or(false);
            let doc = s.meta.as_ref().and_then(|m| m.doc).unwrap_or("Enabled");
            if widgets::checkbox_mixed(ui, &mut on, s.mixed.any())
                .on_hover_text(doc)
                .changed()
            {
                set(ops, s, on);
            }
        }
        if mixed && !open {
            ui.label(egui::RichText::new(MIXED).font(t.mono()).color(t.text_weak))
                .on_hover_text("Some fields differ across the selection");
        }
        // The rest of the row opens and closes it too.
        let rest = ui.available_rect_before_wrap();
        editable && rest.width() > 0.0 && ui.allocate_rect(rest, Sense::click()).clicked()
    });
    let mut open = open;
    if editable && (toggled || resp.label.clicked()) {
        open = !open;
        set_fold_open(ui.ctx(), &row.path, open);
    }
    if resp.revert {
        ops.push(Op::Reset(row.path.clone()));
    }
    resp.label.context_menu(|ui| {
        if let Some(op) = row_menu(ui, state, &registry, row, editable, keep) {
            ops.push(op);
            ui.close();
        }
    });
    open
}

/// Base decimals shown for a number: 3 for a range at most 0.5 wide, else
/// 2, and at least as many as the `@Step` has (a 0.001 step shows 0.006,
/// not 0.01). The value's magnitude can add more
/// ([`widgets::decimals_for`]): a non-zero value never shows as zero.
pub(crate) fn decimals(range_width: Option<f64>, step: Option<f64>) -> usize {
    let from_range = match range_width {
        Some(w) if w <= 0.5 => 3,
        _ => 2,
    };
    let from_step = step
        .filter(|s| *s > 0.0 && s.is_finite())
        .map_or(0, |s| (-s.log10() - 1e-9).ceil().clamp(0.0, 6.0) as usize);
    from_range.max(from_step)
}

fn suffix(row: &FieldRow) -> String {
    row.meta
        .as_ref()
        .and_then(|m| m.unit)
        .map(|u| format!(" {u}"))
        .unwrap_or_default()
}

fn speed(row: &FieldRow, fallback: f64) -> f64 {
    row.meta.as_ref().and_then(|m| m.step).unwrap_or(fallback)
}

fn range(row: &FieldRow) -> Option<(f64, f64)> {
    row.meta
        .as_ref()
        .and_then(|m| m.range)
        .map(|r| (r.min, r.max))
}

pub(crate) fn set(ops: &mut Vec<Op>, row: &FieldRow, value: impl PartialReflect) {
    ops.push(Op::Set {
        path: row.path.clone(),
        value: Box::new(value),
        lanes: None,
    });
}

/// Like [`set`] for a vector row: also records which lanes changed from
/// `before` to `after`, so a multi-selection edit leaves the other lanes of
/// each entity alone.
fn set_lanes(
    ops: &mut Vec<Op>,
    row: &FieldRow,
    value: impl PartialReflect,
    before: &[f32],
    after: &[f32],
) {
    let mask = before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .fold(0u8, |m, (i, _)| m | (1 << i));
    ops.push(Op::Set {
        path: row.path.clone(),
        value: Box::new(value),
        lanes: Some((mask, after.to_vec())),
    });
}

/// The lane mask of a row's mixed state.
fn mixed_mask(row: &FieldRow) -> u8 {
    (0..4)
        .filter(|i| row.mixed.lane(*i))
        .fold(0u8, |m, i| m | (1 << i))
}

/// A row's label: the field name made readable, with the unit for vector
/// fields (their controls are narrow).
pub(crate) fn row_label(row: &FieldRow) -> String {
    let vector = matches!(
        row.kind,
        FieldKind::Vec2
            | FieldKind::Vec3
            | FieldKind::Vec4
            | FieldKind::UVec2
            | FieldKind::UVec3
            | FieldKind::IVec2
            | FieldKind::IVec3
    );
    match row.meta.as_ref().and_then(|m| m.unit) {
        Some(u) if vector => format!("{} ({u})", pretty(&row.label)),
        _ => pretty(&row.label),
    }
}

pub(crate) fn row_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    row: &FieldRow,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let indent = row.depth as f32 * 12.0;
    let doc = row.meta.as_ref().and_then(|m| m.doc);
    let label = row_label(row);
    let mixed = row.mixed.any();
    let editable = ui.is_enabled() && !matches!(row.kind, FieldKind::ReadOnly(_));
    let registry = state.world.registry().clone();
    // Headings (structs) are not marked: their fields are.
    let modified = editable
        && !mixed
        && !matches!(row.kind, FieldKind::Struct)
        && values::differs_from_default(&registry, &row.path, row.value.as_ref()) == Some(true);
    let keep = keep_state(state, &row.path);
    let mark = widgets::RowMark {
        modified,
        kept: keep == Some(true),
        fold: None,
    };
    let (_, resp) = widgets::field_row_ex(ui, &label, indent, doc, mark, |ui| {
        field_control(ui, state, row, t, ops)
    });
    if resp.revert {
        ops.push(Op::Reset(row.path.clone()));
    }
    resp.label.context_menu(|ui| {
        if let Some(op) = row_menu(ui, state, &registry, row, editable, keep) {
            ops.push(op);
            ui.close();
        }
    });
}

/// A field's context menu: Reset to Default, Copy Value, Paste Value
/// (only a value of the field's type) and during Play "Keep after Stop".
fn row_menu(
    ui: &mut Ui,
    state: &mut EditorState,
    registry: &box_core::reflect::Registry,
    row: &FieldRow,
    editable: bool,
    keep: Option<bool>,
) -> Option<Op> {
    // A menu with Paste is open: keep the system clipboard fresh.
    state.clipboard_mut().wanted = true;
    let mut op = None;
    let has_default = values::field_default(registry, &row.path).is_some();
    if ui
        .add_enabled(
            editable && has_default,
            egui::Button::new("Reset to Default"),
        )
        .clicked()
    {
        op = Some(Op::Reset(row.path.clone()));
    }
    if ui.button("Copy Value").clicked() {
        op = Some(Op::Copy(row.path.clone()));
    }
    let clip = state.clipboard().clip();
    let paste = clip
        .as_ref()
        .map(|c| values::paste_value(registry, c, row.value.as_ref()));
    let (text, ok, why) = match (&clip, &paste) {
        (Some(c), Some(Ok(_))) => (format!("Paste Value ({})", c.describe()), true, None),
        (_, Some(Err(e))) => ("Paste Value".to_owned(), false, Some(e.clone())),
        _ => (
            "Paste Value".to_owned(),
            false,
            Some("Copy a value first".into()),
        ),
    };
    let r = ui.add_enabled(editable && ok, egui::Button::new(text));
    let r = match why {
        Some(why) => r.on_disabled_hover_text(why),
        None => r,
    };
    if r.clicked() {
        op = Some(Op::Paste(row.path.clone()));
    }
    if let Some(kept) = keep {
        ui.separator();
        let mut on = kept;
        if widgets::checkbox_labeled(ui, &mut on, "Keep after Stop")
            .on_hover_text("Apply this value to the edit scene when Play stops")
            .changed()
        {
            op = Some(Op::Keep(row.path.clone(), on));
        }
    }
    op
}

/// The control of a field row (everything right of the label).
pub(crate) fn field_control(
    ui: &mut Ui,
    state: &mut EditorState,
    row: &FieldRow,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let mixed = row.mixed.any();
    match &row.kind {
        FieldKind::F32 | FieldKind::F64 => {
            let mut v = row
                .value
                .try_downcast_ref::<f32>()
                .map(|v| *v as f64)
                .or_else(|| row.value.try_downcast_ref::<f64>().copied())
                .unwrap_or_default();
            let changed = match range(row) {
                Some((lo, hi)) if hi - lo <= 10.0 => {
                    let mut f = v as f32;
                    let step = row.meta.as_ref().and_then(|m| m.step);
                    let log = row.meta.as_ref().is_some_and(|m| m.logarithmic);
                    let r = widgets::slider_mixed(
                        ui,
                        &mut f,
                        lo as f32..=hi as f32,
                        decimals(Some(hi - lo), step),
                        &suffix(row),
                        mixed,
                        log,
                    );
                    v = f as f64;
                    r.changed()
                }
                r => {
                    let w = ui.available_width();
                    let parser: fn(&str) -> Option<f64> = if matches!(row.kind, FieldKind::F32) {
                        widgets::parse_finite_f32
                    } else {
                        widgets::parse_finite
                    };
                    let step = row.meta.as_ref().and_then(|m| m.step);
                    let shown = v;
                    let mut dv = egui::DragValue::new(&mut v)
                        .speed(speed(row, 0.01))
                        .custom_parser(parser)
                        .min_size(vec2(w, ui.available_height()));
                    dv = widgets::precise_drag(dv, shown, decimals(None, step).max(3));
                    if let Some((lo, hi)) = r {
                        dv = dv.range(lo..=hi);
                    }
                    dv = match mixed {
                        true => dv.custom_formatter(|_, _| MIXED.to_owned()),
                        false => dv.suffix(suffix(row)),
                    };
                    ui.add(dv).changed()
                }
            };
            if changed {
                if matches!(row.kind, FieldKind::F32) {
                    set(ops, row, v as f32);
                } else {
                    set(ops, row, v);
                }
            }
        }
        FieldKind::Int { signed, bits } => {
            let mut v = int_of(row.value.as_ref()).unwrap_or_default();
            let (tlo, thi) = int_bounds(*signed, *bits);
            let (lo, hi) = range(row).map_or((tlo, thi), |(a, b)| {
                (a.max(tlo as f64) as i64, b.min(thi as f64) as i64)
            });
            let w = ui.available_width();
            let mut dv = egui::DragValue::new(&mut v)
                .speed(speed(row, 0.2))
                .range(lo..=hi)
                .custom_parser(widgets::parse_finite)
                .min_size(vec2(w, ui.available_height()));
            dv = match mixed {
                true => dv.custom_formatter(|_, _| MIXED.to_owned()),
                false => dv.suffix(suffix(row)),
            };
            let r = ui.add(dv);
            if r.changed()
                && let Some(value) = int_value(*signed, *bits, row.value.as_ref(), v)
            {
                ops.push(Op::Set {
                    path: row.path.clone(),
                    value,
                    lanes: None,
                });
            }
        }
        FieldKind::Bool => {
            let mut b = row
                .value
                .try_downcast_ref::<bool>()
                .copied()
                .unwrap_or_default();
            if widgets::checkbox_mixed(ui, &mut b, mixed).changed() {
                set(ops, row, b);
            }
        }
        FieldKind::String => {
            // A mixed value starts empty; typing sets it on every entity.
            let mut s = match mixed {
                true => String::new(),
                false => row
                    .value
                    .try_downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default(),
            };
            let w = ui.available_width();
            if ui
                .add(
                    egui::TextEdit::singleline(&mut s)
                        .hint_text(if mixed { MIXED } else { "" })
                        .desired_width(w),
                )
                .changed()
            {
                set(ops, row, s);
            }
        }
        FieldKind::Vec2 | FieldKind::Vec3 | FieldKind::Vec4 => {
            let mut v: Vec<f32> = if let Some(v) = row.value.try_downcast_ref::<Vec3>() {
                v.to_array().to_vec()
            } else if let Some(v) = row.value.try_downcast_ref::<Vec2>() {
                v.to_array().to_vec()
            } else if let Some(v) = row.value.try_downcast_ref::<Vec4>() {
                v.to_array().to_vec()
            } else {
                Vec::new()
            };
            let lo_hi = range(row).map(|(a, b)| (a as f32, b as f32));
            let before = v.clone();
            let step = row.meta.as_ref().and_then(|m| m.step);
            let places = decimals(range(row).map(|(a, b)| b - a), step);
            let speed = speed(row, 0.01) as f32;
            if widgets::axes_edit_mixed(ui, &mut v, speed, places, "", lo_hi, mixed_mask(row))
                .changed()
            {
                match v.len() {
                    2 => set_lanes(ops, row, Vec2::from_slice(&v), &before, &v),
                    3 => set_lanes(ops, row, Vec3::from_slice(&v), &before, &v),
                    _ => set_lanes(ops, row, Vec4::from_slice(&v), &before, &v),
                }
            }
        }
        FieldKind::UVec2 | FieldKind::UVec3 | FieldKind::IVec2 | FieldKind::IVec3 => {
            let mut v: Vec<f32> = if let Some(v) = row.value.try_downcast_ref::<UVec2>() {
                v.as_vec2().to_array().to_vec()
            } else if let Some(v) = row.value.try_downcast_ref::<UVec3>() {
                v.as_vec3().to_array().to_vec()
            } else if let Some(v) = row.value.try_downcast_ref::<IVec2>() {
                v.as_vec2().to_array().to_vec()
            } else if let Some(v) = row.value.try_downcast_ref::<IVec3>() {
                v.as_vec3().to_array().to_vec()
            } else {
                Vec::new()
            };
            let unsigned = matches!(row.kind, FieldKind::UVec2 | FieldKind::UVec3);
            let lo_hi = range(row)
                .map(|(a, b)| (a as f32, b as f32))
                .or(unsigned.then_some((0.0, u32::MAX as f32)));
            let before = v.clone();
            let speed = speed(row, 0.2) as f32;
            if widgets::axes_edit_mixed(ui, &mut v, speed, 0, "", lo_hi, mixed_mask(row)).changed()
            {
                let i: Vec<i32> = v.iter().map(|x| x.round() as i32).collect();
                let after: Vec<f32> = i.iter().map(|x| *x as f32).collect();
                let (b, a) = (&before, &after);
                match row.kind {
                    FieldKind::UVec2 => {
                        set_lanes(ops, row, UVec2::new(i[0] as u32, i[1] as u32), b, a)
                    }
                    FieldKind::UVec3 => set_lanes(
                        ops,
                        row,
                        UVec3::new(i[0] as u32, i[1] as u32, i[2] as u32),
                        b,
                        a,
                    ),
                    FieldKind::IVec2 => set_lanes(ops, row, IVec2::new(i[0], i[1]), b, a),
                    _ => set_lanes(ops, row, IVec3::new(i[0], i[1], i[2]), b, a),
                }
            }
        }
        FieldKind::Quat => {
            let q = row
                .value
                .try_downcast_ref::<Quat>()
                .copied()
                .unwrap_or_default();
            let id = egui::Id::unique(("box_editor_euler", &row.path));
            let remembered = ui.data(|d| d.get_temp::<EulerEdit>(id));
            let mut v = EulerEdit::degrees_for(remembered, q).to_vec();
            let before = v.clone();
            if widgets::axes_edit_mixed(ui, &mut v, 0.5, 1, "°", None, mixed_mask(row)).changed() {
                let edit = EulerEdit::from_degrees([v[0], v[1], v[2]]);
                ui.data_mut(|d| d.insert_temp(id, edit));
                // The primary gets exactly these angles; other entities
                // change only the edited angle of their own rotation.
                set_lanes(ops, row, edit.rotation, &before, &v);
            }
        }
        FieldKind::Color => {
            let c = row
                .value
                .try_downcast_ref::<Color>()
                .copied()
                .unwrap_or_default();
            let mut rgba = [c.r, c.g, c.b, c.a];
            // egui's colour button edits linear values, like `Color`.
            if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                set(
                    ops,
                    row,
                    Color::linear_rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                );
            }
            ui.add_space(4.0);
            let s = |v: f32| (linear_to_srgb(v) * 255.0).round() as u8;
            let hex = match mixed {
                true => MIXED.to_owned(),
                false => format!("#{:02X}{:02X}{:02X}", s(c.r), s(c.g), s(c.b)),
            };
            ui.label(egui::RichText::new(hex).font(t.mono()).color(t.text));
        }
        FieldKind::Enum { variants, current } => {
            let w = ui.available_width();
            let mut chosen = None;
            egui::ComboBox::from_id_salt(&row.path)
                .width(w)
                .selected_text(if mixed { MIXED } else { current.as_str() })
                .show_ui(ui, |ui| {
                    for v in variants {
                        // With mixed variants every choice changes something.
                        let on = v == current && !mixed;
                        if ui.selectable_label(on, *v).clicked() && !on {
                            chosen = Some(*v);
                        }
                    }
                });
            if let Some(v) = chosen
                && let Ok(current) = box_core::reflect::clone_value(row.value.as_ref())
            {
                ops.push(Op::SwitchVariant {
                    path: row.path.clone(),
                    current,
                    variant: v,
                });
            }
        }
        FieldKind::Struct => {}
        FieldKind::AssetRef => {
            crate::panes::asset_inspector::asset_ref_field(ui, state, row, mixed, ops);
        }
        FieldKind::AssetList { len } => {
            crate::panes::asset_inspector::asset_list_field(ui, row, *len, mixed, t, ops);
        }
        FieldKind::List { len } => list_field(ui, state, row, *len, mixed, t, ops),
        FieldKind::ReadOnly(text) => {
            let text = if mixed { MIXED } else { text.as_str() };
            ui.label(egui::RichText::new(text).font(t.mono()).color(t.text_faint));
        }
    }
}

fn list_open_id(path: &str) -> egui::Id {
    egui::Id::unique(("box_editor_inspector_list_open", path))
}

/// Whether a list row's items are shown: toggled with its chevron, open by
/// default for up to two items (so a spline's points do not push the
/// sections below out of view).
fn list_open(ctx: &egui::Context, path: &str, len: usize) -> bool {
    ctx.data(|d| d.get_temp::<bool>(list_open_id(path)))
        .unwrap_or(len <= 2)
}

/// A list row: a chevron that shows or hides its items, the count, and
/// buttons that add a copy of the last item and remove the last item (each
/// one undo step).
fn list_field(
    ui: &mut Ui,
    state: &EditorState,
    row: &FieldRow,
    len: usize,
    mixed: bool,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let open = list_open(ui.ctx(), &row.path, len);
    let text = match (mixed, len) {
        (true, _) => MIXED.to_string(),
        (false, 1) => "1 item".to_string(),
        (false, n) => format!("{n} items"),
    };
    // The chevron is painted in a gap before the count.
    let toggle = ui
        .add(
            egui::Button::new(egui::RichText::new(format!("    {text}")).color(t.text_weak))
                .frame(false),
        )
        .on_hover_text(if open {
            "Hide the items"
        } else {
            "Show the items"
        });
    widgets::chevron(
        ui.painter(),
        toggle.rect.left_center() + vec2(8.0, 0.0),
        open,
        if toggle.hovered() {
            t.text
        } else {
            t.text_weak
        },
    );
    if toggle.clicked() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(list_open_id(&row.path), !open));
    }
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_enabled_ui(!mixed, |ui| {
            if len > 0
                && ui
                    .add(egui::Button::new("−").min_size(vec2(22.0, 18.0)))
                    .on_hover_text("Remove the last item")
                    .clicked()
                && let Some(value) = inspect::list_with_last_removed(row.value.as_ref())
            {
                ops.push(Op::Set {
                    path: row.path.clone(),
                    value,
                    lanes: None,
                });
            }
            if ui
                .add(egui::Button::new("+").min_size(vec2(22.0, 18.0)))
                .on_hover_text("Add an item (a copy of the last one)")
                .clicked()
                && let Some(value) =
                    inspect::list_with_item_added(state.world.registry(), row.value.as_ref())
            {
                ops.push(Op::Set {
                    path: row.path.clone(),
                    value,
                    lanes: None,
                });
            }
        });
    });
}

/// Words written in capitals in field labels.
const ACRONYMS: &[&str] = &[
    "ao", "fov", "hdr", "ibl", "id", "lod", "orm", "rgb", "rgba", "srgb", "ui", "uv",
];

/// A field name as a label: `cast_shadows` -> `Cast shadows`,
/// `orm_texture` -> `ORM texture`, `fov_y` -> `FOV y`.
pub(crate) fn pretty(label: &str) -> String {
    let words: Vec<String> = label
        .split('_')
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, w)| {
            if ACRONYMS.contains(&w.to_ascii_lowercase().as_str()) {
                w.to_ascii_uppercase()
            } else if i == 0 {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            } else {
                w.to_owned()
            }
        })
        .collect();
    if words.is_empty() {
        return label.to_owned();
    }
    words.join(" ")
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn int_of(v: &dyn PartialReflect) -> Option<i64> {
    macro_rules! try_int {
        ($($t:ty),*) => {
            $(if let Some(x) = v.try_downcast_ref::<$t>() { return Some(*x as i64); })*
        };
    }
    try_int!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);
    None
}

fn int_bounds(signed: bool, bits: u32) -> (i64, i64) {
    match (signed, bits) {
        (false, 64) => (0, i64::MAX),
        (true, 64) => (i64::MIN, i64::MAX),
        (false, b) => (0, (1i64 << b) - 1),
        (true, b) => (-(1i64 << (b - 1)), (1i64 << (b - 1)) - 1),
    }
}

/// `v` as the same integer type as `like`.
fn int_value(_signed: bool, _bits: u32, like: &dyn PartialReflect, v: i64) -> Option<Value> {
    macro_rules! as_like {
        ($($t:ty),*) => {
            $(if like.try_downcast_ref::<$t>().is_some() { return Some(Box::new(v as $t)); })*
        };
    }
    as_like!(u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::LogBuffer;
    use box_core::prelude::{Transform, World};

    /// Dragging the X angle up in half-degree steps, as the rotation row
    /// does: show the angles for the stored quaternion, change one, store
    /// the new quaternion.
    #[test]
    fn labels_know_acronyms() {
        assert_eq!(pretty("cast_shadows"), "Cast shadows");
        assert_eq!(pretty("orm_texture"), "ORM texture");
        assert_eq!(pretty("fov_y"), "FOV y");
        assert_eq!(pretty("uv_scale"), "UV scale");
        assert_eq!(pretty("base_color"), "Base color");
        assert_eq!(pretty("hdr"), "HDR");
        assert_eq!(pretty("0"), "0");
        assert_eq!(pretty("[3]"), "[3]");
    }

    #[test]
    fn decimals_follow_the_step() {
        // Fog density: a 0..1 slider with a 0.001 step shows 0.006.
        assert_eq!(decimals(Some(1.0), Some(0.001)), 3);
        assert_eq!(
            format!("{:.*}", decimals(Some(1.0), Some(0.001)), 0.006),
            "0.006"
        );
        assert_eq!(decimals(Some(1.0), None), 2);
        assert_eq!(decimals(Some(0.2), Some(0.01)), 3);
        assert_eq!(decimals(Some(10.0), Some(0.1)), 2);
        assert_eq!(decimals(None, Some(0.0001)), 4);
    }

    #[test]
    fn euler_angles_pass_through_90_degrees_and_beyond() {
        let mut rotation = Quat::IDENTITY;
        let mut remembered = None;
        for step in 0..800 {
            let shown = EulerEdit::degrees_for(remembered, rotation);
            let expected = step as f32 * 0.5;
            assert!(
                (shown[0] - expected).abs() < 1e-3 && shown[1] == 0.0 && shown[2] == 0.0,
                "step {step}: shown {shown:?}, expected X {expected}"
            );
            let edit = EulerEdit::from_degrees([shown[0] + 0.5, shown[1], shown[2]]);
            rotation = edit.rotation;
            remembered = Some(edit);
        }
    }

    /// The defect this guards against: decomposing the quaternion every
    /// frame cannot follow X past 90 degrees.
    #[test]
    fn decomposing_every_frame_flips_past_90_degrees() {
        let past = EulerEdit::from_degrees([90.5, 0.0, 0.0]);
        let shown = EulerEdit::degrees_for(None, past.rotation);
        assert!((shown[0] - 89.5).abs() < 0.01, "{shown:?}");
        assert!((shown[1].abs() - 180.0).abs() < 0.01, "{shown:?}");
        assert!((shown[2].abs() - 180.0).abs() < 0.01, "{shown:?}");
        // With the angles remembered they are shown as entered.
        assert_eq!(
            EulerEdit::degrees_for(Some(past), past.rotation),
            [90.5, 0.0, 0.0]
        );
        // q and -q are the same rotation.
        assert_eq!(
            EulerEdit::degrees_for(Some(past), -past.rotation),
            [90.5, 0.0, 0.0]
        );
    }

    #[test]
    fn euler_angles_resync_when_the_rotation_changes_from_outside() {
        let edit = EulerEdit::from_degrees([120.0, 10.0, 0.0]);
        // A gizmo drag, an undo or another entity: a different rotation.
        let outside = Quat::from_rotation_y(30f32.to_radians());
        let shown = EulerEdit::degrees_for(Some(edit), outside);
        assert!(
            shown[0].abs() < 1e-3 && (shown[1] - 30.0).abs() < 1e-3 && shown[2].abs() < 1e-3,
            "{shown:?}"
        );
    }

    fn drag_state() -> (EditorState, Uid) {
        let mut world = World::with_registry(box_engine::shared_engine_registry());
        let uid = Uid::from_u128(7);
        world
            .spawn_saved(uid, "Box", (Transform::default(),))
            .unwrap();
        (EditorState::new(world, LogBuffer::default()), uid)
    }

    /// One inspector frame that sets `Transform.translation.x` (a slider
    /// drag: the pointer is held from the first frame until `release`).
    struct Frames {
        ctx: egui::Context,
        n: u32,
    }

    impl Frames {
        fn run(&mut self, state: &mut EditorState, uid: Uid, x: Option<f32>, release: bool) {
            let ops = x
                .map(|x| Op::Set {
                    path: "Transform.translation".into(),
                    value: Box::new(Vec3::new(x, 0.0, 0.0)),
                    lanes: None,
                })
                .into_iter()
                .collect();
            self.run_ops(state, &[uid], ops, release);
        }

        fn run_ops(
            &mut self,
            state: &mut EditorState,
            targets: &[Uid],
            ops: Vec<Op>,
            release: bool,
        ) {
            let pos = pos2(5.0, 5.0);
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut events = Vec::new();
            if self.n == 0 {
                events.extend([egui::Event::PointerMoved(pos), button(true)]);
            }
            if release {
                events.push(button(false));
            }
            let raw = egui::RawInput {
                time: Some(self.n as f64 / 60.0),
                events,
                ..Default::default()
            };
            self.n += 1;
            let mut ops = Some(ops);
            let _ = self.ctx.run_ui(raw, |ui| {
                apply(ui, state, targets, ops.take().unwrap_or_default());
            });
        }
    }

    fn x_of(state: &EditorState, uid: Uid) -> f32 {
        let e = state.world.entity_by_uid(uid).unwrap();
        state.world.get::<Transform>(e).unwrap().translation.x
    }

    #[test]
    fn a_slider_drag_survives_undo_as_whole_steps() {
        let (mut state, uid) = drag_state();
        let mut f = Frames {
            ctx: egui::Context::default(),
            n: 0,
        };
        f.run(&mut state, uid, Some(1.0), false);
        f.run(&mut state, uid, Some(2.0), false);
        assert_eq!(state.edits.undo_len(), 0, "still one open transaction");
        assert!(state.is_dirty());

        // Cmd+Z in the middle of the drag: the drag so far is one step, undone.
        state.undo();
        assert_eq!(x_of(&state, uid), 0.0);
        assert_eq!((state.edits.undo_len(), state.edits.redo_len()), (0, 1));
        assert!(!state.is_dirty());

        // The drag goes on: the rest of it is one more step, not one per frame.
        for x in [3.0, 4.0, 5.0] {
            f.run(&mut state, uid, Some(x), false);
        }
        f.run(&mut state, uid, None, true);
        f.run(&mut state, uid, None, false);
        assert_eq!(
            state.edits.undo_len(),
            1,
            "one step for the rest of the drag"
        );
        assert!(!state.edit_in_progress());
        state.undo();
        assert_eq!(x_of(&state, uid), 0.0);
    }

    /// An X drag in the inspector with three entities selected: one undo
    /// step, every entity gets the X, each keeps its own Y and Z.
    #[test]
    fn a_multi_selection_drag_is_one_step_and_keeps_other_lanes() {
        let (mut state, a) = drag_state();
        let (b, c) = (Uid::from_u128(8), Uid::from_u128(9));
        for (u, y) in [(b, 1.0), (c, 2.0)] {
            state
                .world
                .spawn_saved(u, "Other", (Transform::from_xyz(0.0, y, 3.0),))
                .unwrap();
        }
        let before = box_core::scene::save(&state.world).unwrap();
        let mut f = Frames {
            ctx: egui::Context::default(),
            n: 0,
        };
        let targets = [b, c, a];
        for x in [1.0f32, 2.0, 4.0] {
            let op = Op::Set {
                path: "Transform.translation".into(),
                value: Box::new(Vec3::new(x, 0.0, 0.0)),
                lanes: Some((0b001, vec![x, 0.0, 0.0])),
            };
            f.run_ops(&mut state, &targets, vec![op], false);
        }
        f.run_ops(&mut state, &targets, Vec::new(), true);
        f.run_ops(&mut state, &targets, Vec::new(), false);
        assert_eq!(state.edits.undo_len(), 1);
        assert_eq!(state.edits.undo_label(), Some("3 entities: Translation"));
        let t = |u| {
            let e = state.world.entity_by_uid(u).unwrap();
            state.world.get::<Transform>(e).unwrap().translation
        };
        assert_eq!(t(a), Vec3::new(4.0, 0.0, 0.0));
        assert_eq!(t(b), Vec3::new(4.0, 1.0, 3.0));
        assert_eq!(t(c), Vec3::new(4.0, 2.0, 3.0));
        state.undo();
        assert_eq!(box_core::scene::save(&state.world).unwrap(), before);
    }

    /// What the asset reference widget sends (picker choice, drop, clear,
    /// adding a material slot): each is one undo step through the normal
    /// field path.
    #[test]
    fn asset_reference_edits_are_one_undo_step_each() {
        use box_core::asset_ref::{AssetId, AssetRef};
        use box_core::mesh_asset::MeshAsset;
        let (mut state, uid) = drag_state();
        state.add_component(uid, "MeshAsset");
        let base = state.edits.undo_len();
        let before = box_core::scene::save(&state.world).unwrap();
        let mut f = Frames {
            ctx: egui::Context::default(),
            n: 0,
        };
        let r = AssetRef::id(AssetId::from_u128(9))
            .with_label("Mesh/Roof")
            .with_path("models/tower.glb");
        let set = |path: &str, value: Value| Op::Set {
            path: path.into(),
            value,
            lanes: None,
        };
        // A click in the picker: press and release in one frame.
        f.run_ops(
            &mut state,
            &[uid],
            vec![set("MeshAsset.mesh", Box::new(r.clone()))],
            true,
        );
        f.run_ops(&mut state, &[uid], Vec::new(), false);
        assert_eq!(state.edits.undo_len(), base + 1);
        let slots = vec![AssetRef::NONE];
        f.run_ops(
            &mut state,
            &[uid],
            vec![set("MeshAsset.materials", Box::new(slots))],
            true,
        );
        f.run_ops(&mut state, &[uid], Vec::new(), false);
        f.run_ops(
            &mut state,
            &[uid],
            vec![set("MeshAsset.materials[0]", Box::new(r.clone()))],
            true,
        );
        f.run_ops(&mut state, &[uid], Vec::new(), false);
        assert_eq!(state.edits.undo_len(), base + 3);
        let e = state.world.entity_by_uid(uid).unwrap();
        let m = state
            .world
            .get::<MeshAsset>(e)
            .map(|m| MeshAsset::clone(&m))
            .unwrap();
        assert_eq!((m.mesh, m.materials), (r.clone(), vec![r]));
        for _ in 0..3 {
            state.undo();
        }
        assert_eq!(box_core::scene::save(&state.world).unwrap(), before);
    }

    #[test]
    fn cancelling_a_slider_drag_restores_the_value() {
        let (mut state, uid) = drag_state();
        let mut f = Frames {
            ctx: egui::Context::default(),
            n: 0,
        };
        f.run(&mut state, uid, Some(1.0), false);
        f.run(&mut state, uid, Some(2.0), false);
        assert!(state.edit_in_progress());
        assert!(state.cancel_edit());
        assert_eq!(x_of(&state, uid), 0.0);
        assert!(!state.edits.can_undo() && !state.is_dirty());
        assert!(!state.cancel_edit(), "nothing left to cancel");
    }
}
