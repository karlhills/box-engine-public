//! The Script section's properties: one row per property the script file
//! declares (type, default, range, unit and doc from its `properties`
//! table, read when the file is imported), editing the overrides stored in
//! `Script.properties`. A property without an override shows the script's
//! default (labelled "default"); ↺ removes an override. While playing, the
//! running script sees edits within a few frames.
//!
//! The Script Intent section uses the same rows for its rule file's
//! declared properties (`ScriptIntent.properties`, file from
//! `ScriptIntent.rule`); edits re-run the rule at once, in the edit world
//! and in Play.

use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets};
use box_core::color::Color;
use box_core::identity::{Name, Uid};
use box_core::reflect::FromReflect;
use box_engine::box_script::{PropType, PropertyDecl, ScriptAsset, ScriptProperty, ScriptValue};
use egui::{Align, Layout, Rect, Ui, vec2};

use crate::inspect::{FieldRow, Section};
use crate::panes::inspector::{Op, decimals, pretty, set};
use crate::state::EditorState;
use crate::theme::Tokens;
use crate::widgets::{self, MIXED};

/// The script file of a Script section, or the rule file of a
/// ScriptIntent section.
fn section_source(section: &Section) -> AssetRef {
    section
        .rows
        .iter()
        .find(|r| r.path == "Script.source" || r.path == "ScriptIntent.rule")
        .and_then(|r| r.value.try_downcast_ref::<AssetRef>().cloned())
        .unwrap_or_default()
}

/// The imported script `r`, or why there is none. Requests the import the
/// first time a script is shown.
fn script_asset(state: &EditorState, r: &AssetRef) -> Result<ScriptAsset, String> {
    for world in [&state.world, state.edit_world()] {
        let (Some(lookup), Some(store)) = (
            world.get_resource::<AssetLookup>(),
            world.get_resource::<Assets<ScriptAsset>>(),
        ) else {
            continue;
        };
        if let Some(asset) = lookup.get::<ScriptAsset>(r).and_then(|h| store.get(h)) {
            return Ok(asset.clone());
        }
    }
    let Some(server) = &state.asset_server else {
        return Err("Open a project to read the script's properties".into());
    };
    if let Some(box_assets::LoadState::Failed(msg)) = server.state(r) {
        return Err(msg);
    }
    match server.load::<ScriptAsset>(r) {
        Ok(_) => Err("Loading the script…".into()),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn properties_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    section: &Section,
    row: &FieldRow,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let stored =
        <Vec<ScriptProperty> as FromReflect>::from_reflect(row.value.as_ref()).unwrap_or_default();
    let note = |ui: &mut Ui, text: &str, color: egui::Color32| {
        widgets::field_row(ui, "Properties", 0.0, None, |ui| {
            ui.label(egui::RichText::new(text).color(color))
                .on_hover_text(text);
        });
    };
    let source = section_source(section);
    let rule = row.path.starts_with("ScriptIntent.");
    if source.is_none() {
        let pick = if rule {
            "Pick a .luau rule file first (one that defines rule(intents, out))"
        } else {
            "Pick a .luau script first"
        };
        note(ui, pick, t.text_weak);
        return;
    }
    let asset = match script_asset(state, &source) {
        Ok(a) => a,
        Err(msg) => {
            let color = if msg.ends_with('…') {
                t.text_weak
            } else {
                t.warn
            };
            note(ui, &msg, color);
            return;
        }
    };
    if !asset.is_module {
        note(
            ui,
            "This file does not return a script table (it is a library for require)",
            t.warn,
        );
        return;
    }
    if rule && !asset.functions.iter().any(|f| f == "rule") {
        note(
            ui,
            "This file defines no rule(intents, out) function, so it is not a rule",
            t.warn,
        );
        return;
    }
    // Overrides that differ across a multi-selection are edited one
    // entity at a time: setting the whole list would overwrite the others.
    let editable = !row.mixed.any();
    let unused = stored
        .iter()
        .filter(|p| asset.property(&p.name).is_none())
        .count();
    let mut text = match asset.properties.len() {
        0 => "The file declares no properties".to_owned(),
        1 => "1 property".to_owned(),
        n => format!("{n} properties"),
    };
    if unused > 0 {
        text.push_str(&format!(" · {unused} unused kept"));
    }
    if !editable {
        text = format!("{MIXED} (select one entity to edit)");
    }
    note(ui, &text, t.text_weak);
    for decl in &asset.properties {
        let current = stored
            .iter()
            .find(|p| p.name == decl.name)
            .map(|p| &p.value)
            .filter(|v| v.ty() == decl.ty);
        let is_default = current.is_none();
        let mut value = decl.effective(current);
        // "Speed (deg/s)", "Axis (default)": the unit as the inspector
        // shows it for reflected fields, and whether the default is used.
        let notes: Vec<&str> = decl
            .unit
            .as_deref()
            .into_iter()
            .chain(is_default.then_some("default"))
            .collect();
        let label = match notes.is_empty() {
            true => pretty(&decl.name),
            false => format!("{} ({})", pretty(&decl.name), notes.join(", ")),
        };
        let tip = match &decl.doc {
            Some(d) => format!("{d}\n{}: {}", decl.name, decl.ty.name()),
            None => format!("{}: {}", decl.name, decl.ty.name()),
        };
        let mut changed = false;
        let mut reset = false;
        widgets::field_row(ui, &label, 12.0, Some(&tip), |ui| {
            ui.add_enabled_ui(editable, |ui| {
                if is_default {
                    changed = edit_value(ui, state, &mut value, decl, t);
                    return;
                }
                let w = ui.available_width();
                let area = Rect::from_min_size(
                    ui.cursor().min,
                    vec2((w - 26.0).max(20.0), ui.available_height()),
                );
                let mut sub = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(area)
                        .layout(Layout::left_to_right(Align::Center)),
                );
                changed = edit_value(&mut sub, state, &mut value, decl, t);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    reset = ui
                        .add(egui::Button::new("↺").min_size(vec2(22.0, 18.0)))
                        .on_hover_text("Back to the file's default")
                        .clicked();
                });
            });
        });
        if changed || reset {
            let mut list = stored.clone();
            if reset {
                list.retain(|p| p.name != decl.name);
            } else {
                match list.iter_mut().find(|p| p.name == decl.name) {
                    Some(p) => p.value = value,
                    None => list.push(ScriptProperty {
                        name: decl.name.clone(),
                        value,
                    }),
                }
            }
            set(ops, row, list);
        }
    }
}

/// One property's widget; returns whether `value` changed.
fn edit_value(
    ui: &mut Ui,
    state: &EditorState,
    value: &mut ScriptValue,
    decl: &PropertyDecl,
    t: &Tokens,
) -> bool {
    let w = ui.available_width();
    let h = ui.available_height();
    let finite = decl
        .range
        .filter(|(lo, hi)| lo.is_finite() && hi.is_finite());
    match (decl.ty, value) {
        (PropType::Number, ScriptValue::Number(n)) => {
            let mut f = *n as f32;
            let changed = match finite {
                Some((lo, hi)) => {
                    let places = decimals(Some(hi - lo), decl.step);
                    widgets::slider(ui, &mut f, lo as f32..=hi as f32, places, "").changed()
                }
                None => {
                    let shown = f64::from(f);
                    let mut dv = egui::DragValue::new(&mut f)
                        .speed(decl.step.unwrap_or(0.01))
                        .custom_parser(widgets::parse_finite_f32);
                    dv = widgets::precise_drag(dv, shown, decimals(None, decl.step).max(3));
                    if let Some((lo, hi)) = decl.range {
                        dv = dv.range(lo as f32..=hi as f32);
                    }
                    ui.add(dv.min_size(vec2(w, h))).changed()
                }
            };
            *n = f64::from(f);
            changed
        }
        (PropType::Integer, ScriptValue::Integer(i)) => {
            let mut dv = egui::DragValue::new(i).speed(0.2).min_size(vec2(w, h));
            if let Some((lo, hi)) = decl.range {
                dv = dv.range(lo.max(i64::MIN as f64) as i64..=hi.min(i64::MAX as f64) as i64);
            }
            ui.add(dv).changed()
        }
        (PropType::Boolean, ScriptValue::Boolean(b)) => {
            widgets::checkbox_labeled(ui, b, "").changed()
        }
        (PropType::String, ScriptValue::String(s)) => ui
            .add(egui::TextEdit::singleline(s).desired_width(w))
            .changed(),
        (PropType::Vector, ScriptValue::Vector(v)) => {
            let mut lanes = v.to_array();
            let range = finite.map(|(lo, hi)| (lo as f32, hi as f32));
            let speed = decl.step.unwrap_or(0.01) as f32;
            let places = decimals(finite.map(|(lo, hi)| hi - lo), decl.step);
            let changed = widgets::axes_edit(ui, &mut lanes, speed, places, "", range).changed();
            *v = glam::Vec3::from_array(lanes);
            changed
        }
        (PropType::Color, ScriptValue::Color(c)) => {
            let mut rgba = [c.r, c.g, c.b, c.a];
            let changed = ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed();
            *c = Color::linear_rgba(rgba[0], rgba[1], rgba[2], rgba[3]);
            changed
        }
        (PropType::Asset, ScriptValue::Asset(r)) => {
            let mut text = r.path.clone().unwrap_or_default();
            let changed = ui
                .add(
                    egui::TextEdit::singleline(&mut text)
                        .hint_text("path under assets/")
                        .desired_width(w),
                )
                .changed();
            if changed {
                *r = if text.trim().is_empty() {
                    AssetRef::NONE
                } else {
                    AssetRef::path(text.trim())
                };
            }
            changed
        }
        (PropType::Entity, ScriptValue::Entity(uid)) => entity_combo(ui, state, uid, t),
        _ => false,
    }
}

/// A saved entity picked by name (or none).
fn entity_combo(ui: &mut Ui, state: &EditorState, uid: &mut Option<Uid>, t: &Tokens) -> bool {
    let world = &state.world;
    let name_of = |u: Uid| {
        world
            .entity_by_uid(u)
            .and_then(|e| world.get::<Name>(e).map(|n| n.0.clone()))
            .unwrap_or_else(|| format!("missing {u}"))
    };
    let shown = uid.map_or_else(|| "None".to_owned(), name_of);
    let mut changed = false;
    egui::ComboBox::from_id_salt(("script_entity", ui.next_auto_id()))
        .selected_text(egui::RichText::new(shown).color(t.text))
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(uid, None, "None").changed();
            let mut named: Vec<(String, Uid)> = world
                .query::<(&Uid, &Name)>()
                .iter()
                .map(|(u, n)| (n.0.clone(), *u))
                .collect();
            named.sort();
            for (name, u) in named.into_iter().take(500) {
                changed |= ui.selectable_value(uid, Some(u), name).changed();
            }
        });
    changed
}
