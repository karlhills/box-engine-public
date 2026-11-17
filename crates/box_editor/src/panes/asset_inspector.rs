//! The inspector's asset side:
//!
//! - **Asset reference fields** (`AssetRef`: material textures, `MeshAsset`
//!   mesh and material slots, `PrefabInstance` scene): the referenced file
//!   name and label with its state (missing or failed in the warning
//!   colour), a thumbnail for images, a clear button, a drop target for
//!   assets dragged from the browser, and a picker listing the project's
//!   assets of the field's type. Edits go through the normal field path, so
//!   each is one undo step.
//! - **Assets picked in the browser**: import settings of images and models
//!   (Apply writes the `.meta`, which re-imports), and materials: a
//!   `.material.json` is edited with the same reflected rows and written
//!   back (when the drag or typing ends, or 300 ms after the last change;
//!   atomically), which hot-reloads it; glTF materials are read-only.
//!   Material file edits are not scene undo steps: each write is a step of
//!   the file's own history, which undo and redo step while the material is
//!   shown (`asset_undo.rs`); Revert puts the file back as it was when it
//!   was opened (a step too).

use box_core::asset_ref::AssetRef;
use box_core::image::ImageData;
use box_core::material::StandardMaterial;
use box_core::reflect::bevy_reflect::{FromReflect, ReflectPath};
use box_core::reflect::{PartialReflect, Value};
use egui::{Align, Align2, CornerRadius, Layout, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use crate::assets::{AssetItem, FileKind, RefStatus, RefType, SettingsEdit, file_name};
use crate::inspect::{self, FieldRow};
use crate::panes::assets::{kind_color, thumbnail};
use crate::panes::inspector::{Op, row_ui, rows_ui, set};
use crate::state::EditorState;
use crate::theme::{Tokens, tokens};
use crate::values;
use crate::widgets::{self, MIXED};

/// The display text of a reference: `file.ext#label`.
fn ref_text(state: &EditorState, r: &AssetRef) -> String {
    let canonical = state.asset_server().and_then(|s| s.canonical_ref(r));
    let path = canonical
        .as_ref()
        .and_then(|c| c.path.clone())
        .or_else(|| r.path.clone());
    let base = match path {
        Some(p) => file_name(&p).to_string(),
        None => r.id.map_or_else(|| "none".into(), |i| i.to_string()),
    };
    match &r.label {
        Some(l) => format!("{base}#{l}"),
        None => base,
    }
}

/// An `AssetRef` field: name and state, thumbnail, clear button, drop
/// target and picker.
pub(crate) fn asset_ref_field(
    ui: &mut Ui,
    state: &mut EditorState,
    row: &FieldRow,
    mixed: bool,
    ops: &mut Vec<Op>,
) {
    let t = tokens(ui.ctx());
    let r = row
        .value
        .try_downcast_ref::<AssetRef>()
        .cloned()
        .unwrap_or_default();
    let ty = RefType::of_field(&row.path);
    let status = if mixed {
        RefStatus::Loaded
    } else {
        state.ref_status(&r)
    };
    let h = ui.available_height();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let enabled = ui.is_enabled();
    let p = ui.painter().clone();
    p.rect_filled(
        rect,
        CornerRadius::same(t.radius),
        if resp.hovered() && enabled {
            t.field_hover
        } else {
            t.field
        },
    );
    let clear = (r.is_some() && !mixed && enabled)
        .then(|| Rect::from_min_size(pos2(rect.right() - h, rect.top()), vec2(h, h)));
    let mut x = rect.left() + 6.0;
    // Thumbnail for loaded images.
    let item = state
        .asset_server()
        .and_then(|s| s.canonical_ref(&r))
        .and_then(|c| c.id.map(|id| AssetItem { id, label: c.label }));
    if !mixed
        && status == RefStatus::Loaded
        && let Some(item) = &item
        && state.item_type(item) == RefType::Image
        && let Some(tex) = thumbnail(state, ui.ctx(), item)
    {
        let s = (h - 4.0).max(8.0);
        let tr = Rect::from_min_size(pos2(x, rect.center().y - s / 2.0), vec2(s, s));
        p.image(
            tex,
            tr,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        x += s + 5.0;
    }
    let (text, color, tip) = match &status {
        _ if mixed => (MIXED.to_string(), t.text, None),
        RefStatus::None => ("None".to_string(), t.text_faint, None),
        RefStatus::Missing => (
            format!("{} (missing)", ref_text(state, &r)),
            t.warn,
            Some(format!("{r}: not found in the project's assets")),
        ),
        RefStatus::Failed(m) => (ref_text(state, &r), t.warn, Some(format!("{r}: {m}"))),
        RefStatus::Loading => (
            ref_text(state, &r),
            t.text_weak,
            Some(format!("{r} (loading)")),
        ),
        RefStatus::Loaded => (ref_text(state, &r), t.text, Some(r.to_string())),
    };
    let right = clear.map_or(rect.right() - 16.0, |c| c.left() - 2.0);
    let mut job = egui::text::LayoutJob::single_section(
        text,
        egui::TextFormat {
            font_id: t.body(),
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (right - x).max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = ui.painter().layout_job(job);
    p.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    // Picker arrow (when there is no clear button) or the clear button.
    match clear {
        Some(c) => {
            let cr = ui.interact(c, ui.scope_id().with(("clear", &row.path)), Sense::click());
            let col = if cr.hovered() {
                t.text_strong
            } else {
                t.text_weak
            };
            let k = 3.5;
            p.line_segment(
                [c.center() - vec2(k, k), c.center() + vec2(k, k)],
                Stroke::new(1.3, col),
            );
            p.line_segment(
                [c.center() + vec2(-k, k), c.center() + vec2(k, -k)],
                Stroke::new(1.3, col),
            );
            if cr.on_hover_text("Clear").clicked() {
                ops.push(Op::Set {
                    path: row.path.clone(),
                    value: Box::new(AssetRef::NONE),
                    lanes: None,
                });
            }
        }
        None => widgets::chevron(
            &p,
            pos2(rect.right() - 9.0, rect.center().y),
            true,
            t.text_weak,
        ),
    }
    let resp = match tip {
        Some(tip) => resp.on_hover_text(tip),
        None => resp,
    };
    if !enabled {
        return;
    }
    // Drop target for browser drags of a matching type.
    if let Some(drag) = state.assets.drag.clone()
        && ty.accepts(drag.ty)
    {
        let (pos, released) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
        if pos.is_some_and(|p| rect.contains(p)) {
            p.rect_stroke(
                rect,
                CornerRadius::same(t.radius),
                Stroke::new(1.5, t.accent),
                StrokeKind::Inside,
            );
            if released {
                ops.push(Op::Set {
                    path: row.path.clone(),
                    value: Box::new(state.item_ref(&drag.item)),
                    lanes: None,
                });
            }
        }
    }
    let open = state
        .assets
        .picker
        .as_ref()
        .is_some_and(|p| p.path == row.path);
    if resp.clicked() {
        state.assets.picker = (!open).then(|| crate::assets::Picker {
            path: row.path.clone(),
            filter: String::new(),
        });
    }
    if open {
        picker_popup(ui.ctx(), state, row, rect, ty, &t, ops);
    }
}

/// The popup listing assets of the field's type.
fn picker_popup(
    ctx: &egui::Context,
    state: &mut EditorState,
    row: &FieldRow,
    field: Rect,
    ty: RefType,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let current = row.value.try_downcast_ref::<AssetRef>().cloned();
    let filter = state
        .assets
        .picker
        .as_ref()
        .map(|p| p.filter.to_lowercase())
        .unwrap_or_default();
    let candidates: Vec<(AssetItem, String)> = state
        .compatible_assets(ty)
        .into_iter()
        .filter(|(_, name)| filter.is_empty() || name.to_lowercase().contains(&filter))
        .collect();
    let current_item = current
        .as_ref()
        .and_then(|r| state.asset_server().and_then(|s| s.canonical_ref(r)))
        .and_then(|c| c.id.map(|id| AssetItem { id, label: c.label }));
    let mut chosen: Option<AssetRef> = None;
    let width = field.width().max(280.0);
    let area = egui::Area::new(egui::Id::unique(("asset_picker", row.path.as_str())))
        .order(egui::Order::Foreground)
        .fade_in(false)
        .fixed_pos(field.left_bottom() + vec2(0.0, 2.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.surface_header)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(t.radius + 1))
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(width);
                    if let Some(p) = state.assets.picker.as_mut() {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut p.filter)
                                .hint_text(format!("Search {}s", ty.label()))
                                .desired_width(width),
                        );
                        if !r.has_focus() && p.filter.is_empty() {
                            r.request_focus();
                        }
                    }
                    ui.add_space(4.0);
                    let rows = candidates.len().max(1) + 1;
                    let height = (rows as f32 * 24.0).min(264.0);
                    egui::ScrollArea::vertical()
                        .id_salt(("asset_picker_list", row.path.as_str()))
                        .max_height(height)
                        .min_scrolled_height(height)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            if picker_row(ui, state, None, "None", current_item.is_none(), t) {
                                chosen = Some(AssetRef::NONE);
                            }
                            for (item, name) in &candidates {
                                let on = current_item.as_ref() == Some(item);
                                if picker_row(ui, state, Some(item), name, on, t) {
                                    chosen = Some(state.item_ref(item));
                                }
                            }
                            if candidates.is_empty() {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "No {}s in the project",
                                        ty.label()
                                    ))
                                    .color(t.text_weak),
                                );
                            }
                        });
                });
        });
    let popup = area.response.rect;
    let pressed_outside = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !popup.contains(p) && !field.contains(p))
    });
    if let Some(r) = chosen {
        ops.push(Op::Set {
            path: row.path.clone(),
            value: Box::new(r),
            lanes: None,
        });
        state.assets.picker = None;
    } else if pressed_outside {
        state.assets.picker = None;
    }
}

/// One picker entry; returns true when clicked.
fn picker_row(
    ui: &mut Ui,
    state: &mut EditorState,
    item: Option<&AssetItem>,
    name: &str,
    on: bool,
    t: &Tokens,
) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
    let p = ui.painter().clone();
    if on {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.accent_bg);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.hover);
    }
    let icon = Rect::from_min_size(pos2(rect.left() + 4.0, rect.top() + 2.0), vec2(20.0, 20.0));
    if let Some(item) = item {
        let ty = state.item_type(item);
        match (ty, thumbnail(state, ui.ctx(), item)) {
            (RefType::Image, Some(tex)) => {
                p.image(
                    tex,
                    icon,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            _ => {
                let kind = match ty {
                    RefType::Image => FileKind::Image,
                    RefType::Material => FileKind::Material,
                    RefType::Scene => FileKind::Scene,
                    _ => FileKind::Model,
                };
                p.rect_filled(
                    icon.shrink(3.0),
                    CornerRadius::same(3),
                    kind_color(kind, t).gamma_multiply(0.5),
                );
            }
        }
    }
    let status = item.map(|i| state.ref_status(&state.item_ref(i)));
    let color = match status {
        Some(RefStatus::Failed(_) | RefStatus::Missing) => t.warn,
        _ if item.is_none() => t.text_weak,
        _ => t.text,
    };
    p.text(
        pos2(icon.right() + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        t.body(),
        if on { t.accent_text } else { color },
    );
    resp.clicked()
}

/// A list of asset references (material slots): the count, and buttons to
/// add or remove the last slot (each one undo step).
pub(crate) fn asset_list_field(
    ui: &mut Ui,
    row: &FieldRow,
    len: usize,
    mixed: bool,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let list = <Vec<AssetRef> as FromReflect>::from_reflect(row.value.as_ref()).unwrap_or_default();
    let text = if mixed {
        MIXED.to_string()
    } else if len == 1 {
        "1 slot".to_string()
    } else {
        format!("{len} slots")
    };
    ui.label(egui::RichText::new(text).color(t.text_weak));
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_enabled_ui(!mixed, |ui| {
            if len > 0
                && ui
                    .add(egui::Button::new("−").min_size(vec2(22.0, 18.0)))
                    .on_hover_text("Remove the last slot")
                    .clicked()
            {
                let mut l = list.clone();
                l.pop();
                set(ops, row, l);
            }
            if ui
                .add(egui::Button::new("+").min_size(vec2(22.0, 18.0)))
                .on_hover_text("Add a slot")
                .clicked()
            {
                let mut l = list.clone();
                l.push(AssetRef::NONE);
                set(ops, row, l);
            }
        });
    });
}

/// Sets `field` (bevy path below the component, `""` for the whole value)
/// of a material to `value`.
fn apply_to_material(m: &mut StandardMaterial, field: &str, value: &dyn PartialReflect) {
    let root = m.as_partial_reflect_mut();
    let target = if field.is_empty() {
        Ok(root)
    } else {
        field.reflect_element_mut(root)
    };
    match target {
        Ok(target) => {
            if let Err(e) = box_core::reflect::replace(target, value) {
                tracing::warn!("material {field}: {e}");
            }
        }
        Err(e) => tracing::warn!("material {field}: {e}"),
    }
}

/// The inspector for the asset picked in the browser.
pub(crate) fn asset_inspector_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    let now = ui.input(|i| i.time);
    let Some(item) = state.assets.inspect.as_ref().map(|i| i.item.clone()) else {
        return;
    };
    let mut apply = false;
    let mut revert_settings = false;
    let mut revert_material = false;
    let mut material_ops = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("inspector_asset")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(8.0);
            header(ui, state, &item, &t);
            info_rows(ui, state, &item, &t);
            if state
                .assets
                .inspect
                .as_ref()
                .is_some_and(|i| i.settings.is_some())
            {
                settings_section(ui, state, &t, &mut apply, &mut revert_settings);
            }
            if state
                .assets
                .inspect
                .as_ref()
                .is_some_and(|i| i.material.is_some())
            {
                material_section(ui, state, &t, &mut material_ops, &mut revert_material);
            }
            ui.add_space(12.0);
        });
    if apply && let Err(e) = state.apply_import_settings() {
        tracing::error!("import settings: {e:#}");
    }
    if revert_settings {
        state.revert_import_settings();
    }
    if revert_material {
        state.revert_material();
    }
    let registry = state.world.registry().clone();
    let current = |state: &EditorState, path: &str| -> Option<Value> {
        let m = state.assets.inspect.as_ref()?.material.as_ref()?;
        let field = path.strip_prefix("StandardMaterial").unwrap_or(path);
        if field.is_empty() {
            return box_core::reflect::clone_value(&m.value).ok();
        }
        let v = field.reflect_element(m.value.as_partial_reflect()).ok()?;
        box_core::reflect::clone_value(v).ok()
    };
    for op in material_ops {
        let (path, value): (String, Value) = match op {
            Op::Set { path, value, .. } => (path, value),
            Op::SwitchVariant {
                path,
                current,
                variant,
            } => match inspect::switch_variant(&registry, current.as_ref(), variant) {
                Some(v) => (path, v),
                None => continue,
            },
            // Reset, copy and paste work on the file's values like on
            // components (`values.rs`).
            Op::Reset(path) => match values::field_default(&registry, &path) {
                Some(v) => (path, v),
                None => continue,
            },
            Op::Copy(path) => {
                let text = current(state, &path).and_then(|v| {
                    let r = match path.contains(['.', '[']) {
                        true => values::copy_value_text(&registry, v.as_ref()),
                        false => values::copy_component_text(&registry, &path, v.as_ref()),
                    };
                    r.map_err(|e| tracing::warn!("cannot copy {path}: {e}"))
                        .ok()
                });
                if let Some(text) = text {
                    state.copy_to_clipboard(text.clone());
                    ui.ctx().copy_text(text);
                }
                continue;
            }
            Op::Paste(path) => {
                let clip = state.clipboard().clip();
                let pasted = match (clip, current(state, &path)) {
                    (Some(c), Some(like)) => values::paste_value(&registry, &c, like.as_ref()),
                    _ => Err("the clipboard holds no value".into()),
                };
                match pasted {
                    Ok(v) => (path, v),
                    Err(e) => {
                        tracing::warn!("cannot paste into {}: {e}", inspect::field_label(&path));
                        continue;
                    }
                }
            }
            _ => continue,
        };
        let field = path
            .strip_prefix("StandardMaterial")
            .unwrap_or(&path)
            .to_string();
        let label = (!field.is_empty()).then(|| inspect::field_label(&path));
        state.edit_material_labelled(now, label, |m| apply_to_material(m, &field, value.as_ref()));
    }
}

fn header(ui: &mut Ui, state: &EditorState, item: &AssetItem, t: &Tokens) {
    let name = state.item_name(item);
    let r = state.item_ref(item);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    ui.painter().text(
        pos2(row.left() + t.pad_x + 8.0, row.center().y),
        Align2::LEFT_CENTER,
        &name,
        t.strong(),
        t.text_strong,
    );
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    ui.painter().text(
        pos2(row.left() + t.pad_x + 8.0, row.center().y),
        Align2::LEFT_CENTER,
        r.to_string(),
        t.small(),
        t.text_weak,
    );
    ui.add_space(6.0);
}

fn info_line(ui: &mut Ui, label: &str, value: &str, color: egui::Color32) {
    widgets::field_row(ui, label, 0.0, None, |ui| {
        ui.label(egui::RichText::new(value).color(color));
    });
}

fn info_rows(ui: &mut Ui, state: &mut EditorState, item: &AssetItem, t: &Tokens) {
    let ty = state.item_type(item);
    let kind = state
        .asset_server()
        .and_then(|s| s.path_of(item.id))
        .map(|p| FileKind::of(&p));
    let type_text = match (item.label.is_some(), kind, ty) {
        (true, _, RefType::Mesh) => "Mesh (glTF)",
        (true, _, RefType::Material) => "Material (glTF)",
        (true, _, RefType::Image) => "Texture (glTF)",
        (true, _, RefType::Scene) => "Scene (glTF)",
        (_, Some(FileKind::Image), _) => "Image",
        (_, Some(FileKind::Model), _) => "Model (glTF)",
        (_, Some(FileKind::Material), _) => "Material",
        (_, Some(FileKind::Scene), _) => "Scene",
        (_, Some(FileKind::Audio), _) => "Sound",
        _ => "File",
    };
    info_line(ui, "Type", type_text, t.text);
    let status = state.ref_status(&state.item_ref(item));
    let (s, c) = match &status {
        RefStatus::Loaded => ("Loaded".to_string(), t.text),
        RefStatus::Loading => ("Importing…".to_string(), t.text_weak),
        RefStatus::Failed(m) => (format!("Failed: {m}"), t.warn),
        RefStatus::Missing => ("Missing".to_string(), t.warn),
        RefStatus::None => ("—".to_string(), t.text_weak),
    };
    widgets::field_row(ui, "State", 0.0, None, |ui| {
        let r = ui.label(egui::RichText::new(&s).color(c));
        if let RefStatus::Failed(m) = &status {
            r.on_hover_text(m);
        }
    });
    if ty == RefType::Image
        && let Some(server) = state.asset_server()
        && let Some(h) = server.get_handle::<ImageData>(&state.item_ref(item))
        && let Some(img) = state
            .edit_world()
            .get_resource::<box_core::assets::Assets<ImageData>>()
            .and_then(|a| a.get(h))
    {
        let text = format!(
            "{} × {} · {} mip{} · {}",
            img.width,
            img.height,
            img.mips.len(),
            if img.mips.len() == 1 { "" } else { "s" },
            match img.color_space {
                box_core::image::ColorSpace::Srgb => "sRGB",
                box_core::image::ColorSpace::Linear => "linear",
            }
        );
        info_line(ui, "Image", &text, t.text);
        if let Some(tex) = thumbnail(state, ui.ctx(), item) {
            let (row, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 132.0), Sense::hover());
            let area = Rect::from_min_size(
                pos2(row.left() + t.pad_x + 8.0, row.top() + 2.0),
                vec2(128.0, 128.0),
            );
            ui.painter().rect_filled(area, 0.0, t.bg_app);
            ui.painter().image(
                tex,
                area,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
    if kind == Some(FileKind::Model)
        && item.label.is_none()
        && let Some(server) = state.asset_server()
    {
        let subs = server.sub_assets(item.id);
        let count = |ty: RefType| {
            subs.iter()
                .filter(|s| RefType::of_type_name(s.type_name) == ty)
                .count()
        };
        let text = format!(
            "{} meshes · {} materials · {} textures",
            count(RefType::Mesh),
            count(RefType::Material),
            count(RefType::Image)
        );
        info_line(ui, "Contains", &text, t.text);
    }
    ui.add_space(6.0);
}

fn settings_section(
    ui: &mut Ui,
    state: &mut EditorState,
    t: &Tokens,
    apply: &mut bool,
    revert: &mut bool,
) {
    let mut open = true;
    widgets::section_with_menu(
        ui,
        "Import Settings",
        Some("Stored in the asset's .meta file. Apply re-imports the asset."),
        &mut open,
        None::<fn(&mut Ui)>,
        |ui| {
            let Some(s) = state
                .assets
                .inspect
                .as_mut()
                .and_then(|i| i.settings.as_mut())
            else {
                return;
            };
            widgets::field_row(ui, "Importer", 0.0, None, |ui| {
                ui.label(
                    egui::RichText::new(&s.importer)
                        .font(t.mono())
                        .color(t.text_weak),
                );
            });
            match &mut s.edit {
                SettingsEdit::Texture(tex) => {
                    widgets::field_row(
                        ui,
                        "Color space",
                        0.0,
                        Some("sRGB for colour data; linear for normal maps, ORM and masks."),
                        |ui| {
                            use box_assets::meta::ColorSpaceSetting as C;
                            let w = ui.available_width();
                            egui::ComboBox::from_id_salt("import_color_space")
                                .width(w)
                                .selected_text(match tex.color_space {
                                    C::Srgb => "sRGB",
                                    C::Linear => "Linear",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut tex.color_space, C::Srgb, "sRGB");
                                    ui.selectable_value(&mut tex.color_space, C::Linear, "Linear");
                                });
                        },
                    );
                    widgets::field_row(
                        ui,
                        "Mipmaps",
                        0.0,
                        Some("Generate the full mip chain at import."),
                        |ui| {
                            widgets::checkbox(ui, &mut tex.mipmaps);
                        },
                    );
                }
                SettingsEdit::Model(m) => {
                    widgets::field_row(
                        ui,
                        "Scale",
                        0.0,
                        Some(
                            "Uniform scale for positions and node translations (0.01 for centimetres).",
                        ),
                        |ui| {
                            let w = ui.available_width();
                            ui.add(
                                egui::DragValue::new(&mut m.scale)
                                    .speed(0.01)
                                    .range(0.0001..=10000.0)
                                    .max_decimals(4)
                                    .custom_parser(widgets::parse_finite_f32)
                                    .min_size(vec2(w, ui.available_height())),
                            );
                        },
                    );
                    widgets::field_row(
                        ui,
                        "Tangents",
                        0.0,
                        Some(
                            "Generate MikkTSpace tangents for meshes with normals and UVs but no tangents.",
                        ),
                        |ui| {
                            widgets::checkbox(ui, &mut m.generate_tangents);
                        },
                    );
                }
                SettingsEdit::Other(json) => {
                    let text = serde_json::to_string_pretty(json).unwrap_or_default();
                    widgets::field_row(ui, "Settings", 0.0, None, |ui| {
                        ui.label(egui::RichText::new(text).font(t.mono()).color(t.text_faint));
                    });
                }
            }
            let dirty = s.is_dirty();
            let editable = !matches!(s.edit, SettingsEdit::Other(_));
            if let Some(e) = &s.error {
                widgets::field_row(ui, "Error", 0.0, None, |ui| {
                    ui.label(egui::RichText::new(e).color(t.error_text));
                });
            }
            ui.add_space(4.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
            let mut bui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(row.shrink2(vec2(t.pad_x + 2.0, 0.0)))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            bui.add_enabled_ui(dirty && editable, |ui| {
                if ui
                    .add(egui::Button::new("Apply").min_size(vec2(70.0, 24.0)))
                    .clicked()
                {
                    *apply = true;
                }
                if ui
                    .add(egui::Button::new("Revert").min_size(vec2(70.0, 24.0)))
                    .clicked()
                {
                    *revert = true;
                }
            });
        },
    );
}

fn material_section(
    ui: &mut Ui,
    state: &mut EditorState,
    t: &Tokens,
    ops: &mut Vec<Op>,
    revert: &mut bool,
) {
    let Some((value, read_only, can_revert)) = state
        .assets
        .inspect
        .as_ref()
        .and_then(|i| i.material.as_ref())
        .map(|m| (m.value.clone(), m.read_only, m.can_revert()))
    else {
        return;
    };
    let registry = state.world.registry().clone();
    let rows = inspect::rows(&registry, "StandardMaterial", &value);
    let doc = registry.component("StandardMaterial").and_then(|c| c.doc());
    let mut open = true;
    widgets::section_with_menu(ui, "Material", doc, &mut open, None::<fn(&mut Ui)>, |ui| {
        ui.add_enabled_ui(!read_only, |ui| {
            rows_ui(ui, state, &rows, t, ops, |_, _, _, _| false);
        });
        ui.add_space(4.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
        let inner = row.shrink2(vec2(t.pad_x + 2.0, 0.0));
        let note = if read_only {
            "Imported from glTF: read-only"
        } else {
            "Saved to the file as you edit"
        };
        ui.painter().text(
            pos2(inner.left() + 4.0, inner.center().y),
            Align2::LEFT_CENTER,
            note,
            t.small(),
            t.text_weak,
        );
        if !read_only {
            let mut bui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::right_to_left(Align::Center)),
            );
            bui.add_enabled_ui(can_revert, |ui| {
                if ui
                    .add(egui::Button::new("Revert").min_size(vec2(70.0, 24.0)))
                    .on_hover_text("Put the file back as it was when it was opened")
                    .clicked()
                {
                    *revert = true;
                }
            });
        }
    });
}

// ----- shader materials -----

/// The shader reference of a `ShaderMaterial` section.
fn section_shader(section: &inspect::Section) -> AssetRef {
    section
        .rows
        .iter()
        .find(|r| r.path == "ShaderMaterial.shader")
        .and_then(|r| r.value.try_downcast_ref::<AssetRef>().cloned())
        .unwrap_or_default()
}

/// `ShaderMaterial.params` driven by the shader's `@material` header: one
/// row per declared parameter with its type, `@range`, `@color` and doc
/// tooltip. A parameter the material does not set shows the header default
/// (labelled "default"); editing it sets it. The reset button removes a set
/// value (back to the default). Each change sets the whole list, one undo
/// step per edit (drags merge). Without a readable header the list is shown
/// with the generic reflected rows, under the reason.
pub(crate) fn shader_params_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    section: &inspect::Section,
    row: &FieldRow,
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    use box_core::shader_material::ShaderParam;
    let params =
        <Vec<ShaderParam> as FromReflect>::from_reflect(row.value.as_ref()).unwrap_or_default();
    let info = state.shader_info(&section_shader(section));
    let note = |ui: &mut Ui, text: String, color: egui::Color32| {
        widgets::field_row(ui, "Params", 0.0, None, |ui| {
            ui.label(egui::RichText::new(&text).color(color))
                .on_hover_text(&text);
        });
    };
    let header = match info.map(|i| i.header) {
        Some(Ok(Some(h))) => h,
        Some(Ok(None)) => {
            note(ui, "The shader has no @material header".into(), t.text_weak);
            return generic_params(ui, state, row, &params, t, ops);
        }
        Some(Err(e)) => {
            note(ui, format!("Header: {e}"), t.warn);
            return generic_params(ui, state, row, &params, t, ops);
        }
        None => {
            note(ui, "Pick a shader first".into(), t.text_weak);
            return generic_params(ui, state, row, &params, t, ops);
        }
    };
    // Lists that differ across a multi-selection are edited one entity at a
    // time: setting the whole list would overwrite the others' values.
    let editable = !row.mixed.any();
    let unused = params
        .iter()
        .filter(|p| header.param(&p.name).is_none())
        .count();
    let mut text = format!("{} in {}", header.params.len(), header.struct_name);
    if unused > 0 {
        text.push_str(&format!(" · {unused} unused kept"));
    }
    if !editable {
        text = format!("{MIXED} (select one entity to edit)");
    }
    note(ui, text, t.text_weak);
    for decl in &header.params {
        let current = params
            .iter()
            .find(|p| p.name == decl.name)
            .map(|p| p.value)
            .filter(|v| decl.ty.accepts(v));
        let is_default = current.is_none();
        let mut value = current.unwrap_or(decl.default);
        let pretty = crate::panes::inspector::pretty(&decl.name);
        let label = if is_default {
            format!("{pretty} (default)")
        } else {
            pretty
        };
        let tip = match &decl.doc {
            Some(d) => format!("{d}\n{}: {}", decl.name, decl.ty.wgsl()),
            None => format!("{}: {}", decl.name, decl.ty.wgsl()),
        };
        let mut changed = false;
        let mut reset = false;
        widgets::field_row(ui, &label, 12.0, Some(&tip), |ui| {
            ui.add_enabled_ui(editable, |ui| {
                if is_default {
                    changed = edit_param(ui, &mut value, decl, t);
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
                changed = edit_param(&mut sub, &mut value, decl, t);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    reset = ui
                        .add(egui::Button::new("↺").min_size(vec2(22.0, 18.0)))
                        .on_hover_text("Back to the header default")
                        .clicked();
                });
            });
        });
        if changed || reset {
            let mut list = params.clone();
            if reset {
                list.retain(|p| p.name != decl.name);
            } else {
                match list.iter_mut().find(|p| p.name == decl.name) {
                    Some(p) => p.value = value,
                    None => list.push(ShaderParam {
                        name: decl.name.clone(),
                        value,
                    }),
                }
            }
            set(ops, row, list);
        }
    }
}

/// One header parameter's widget; returns whether `value` changed.
fn edit_param(
    ui: &mut Ui,
    value: &mut box_core::shader_material::ShaderParamValue,
    decl: &box_core::shader_material::ShaderParamDecl,
    t: &Tokens,
) -> bool {
    use box_core::color::Color;
    use box_core::shader_material::{ShaderParamType as T, ShaderParamValue as V};
    let range = decl.range;
    let w = ui.available_width();
    let h = ui.available_height();
    match decl.ty {
        T::Float => {
            let mut f = match value {
                V::Float(f) => *f,
                _ => 0.0,
            };
            let changed = match range {
                Some((lo, hi)) => {
                    let places = super::inspector::decimals(Some(f64::from(hi - lo)), None);
                    widgets::slider(ui, &mut f, lo..=hi, places, "").changed()
                }
                None => {
                    let shown = f64::from(f);
                    let dv = egui::DragValue::new(&mut f)
                        .speed(0.01)
                        .custom_parser(widgets::parse_finite_f32)
                        .min_size(vec2(w, h));
                    ui.add(widgets::precise_drag(dv, shown, 3)).changed()
                }
            };
            *value = V::Float(f);
            changed
        }
        T::Int => {
            let mut i = match value {
                V::Int(i) => *i,
                _ => 0,
            };
            let mut dv = egui::DragValue::new(&mut i).speed(0.2).min_size(vec2(w, h));
            if let Some((lo, hi)) = range {
                dv = dv.range(lo as i32..=hi as i32);
            }
            let changed = ui.add(dv).changed();
            *value = V::Int(i);
            changed
        }
        T::Uint => {
            let mut u = match value {
                V::Uint(u) => *u,
                _ => 0,
            };
            let (lo, hi) = range.map_or((0, u32::MAX), |(a, b)| {
                (a.max(0.0) as u32, b.max(0.0) as u32)
            });
            let changed = ui
                .add(
                    egui::DragValue::new(&mut u)
                        .speed(0.2)
                        .range(lo..=hi)
                        .min_size(vec2(w, h)),
                )
                .changed();
            *value = V::Uint(u);
            changed
        }
        T::Vec2 | T::Vec3 | T::Vec4 => {
            let mut lanes: Vec<f32> = match value {
                V::Vec2(v) => v.to_array().to_vec(),
                V::Vec3(v) => v.to_array().to_vec(),
                V::Vec4(v) => v.to_array().to_vec(),
                _ => vec![0.0; decl.ty.components()],
            };
            let changed = widgets::axes_edit(ui, &mut lanes, 0.01, 2, "", range).changed();
            *value = match decl.ty {
                T::Vec2 => V::Vec2(glam::Vec2::from_slice(&lanes)),
                T::Vec3 => V::Vec3(glam::Vec3::from_slice(&lanes)),
                _ => V::Vec4(glam::Vec4::from_slice(&lanes)),
            };
            changed
        }
        T::Rgb | T::Rgba => {
            let c = match value {
                V::Color(c) => *c,
                V::Vec3(v) => Color::linear(v.x, v.y, v.z),
                V::Vec4(v) => Color::linear_rgba(v.x, v.y, v.z, v.w),
                _ => Color::BLACK,
            };
            let changed = if decl.ty == T::Rgb {
                let mut rgb = [c.r, c.g, c.b];
                let r = ui.color_edit_button_rgb(&mut rgb).changed();
                *value = V::Color(Color::linear(rgb[0], rgb[1], rgb[2]));
                r
            } else {
                let mut rgba = [c.r, c.g, c.b, c.a];
                let r = ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed();
                *value = V::Color(Color::linear_rgba(rgba[0], rgba[1], rgba[2], rgba[3]));
                r
            };
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(if decl.ty == T::Rgb { "rgb" } else { "rgba" })
                    .font(t.mono())
                    .color(t.text_faint),
            );
            changed
        }
    }
}

/// The parameter list with the generic reflected rows (name and value per
/// entry), when there is no header to drive it.
fn generic_params(
    ui: &mut Ui,
    state: &mut EditorState,
    row: &FieldRow,
    params: &[box_core::shader_material::ShaderParam],
    t: &Tokens,
    ops: &mut Vec<Op>,
) {
    let registry = state.world.registry().clone();
    for (i, p) in params.iter().enumerate() {
        let base = format!("{}[{i}]", row.path);
        widgets::field_row(ui, &format!("param {i}"), 12.0, None, |_| {});
        let mut rows = inspect::rows(&registry, &base, p);
        for r in &mut rows {
            r.depth += 2;
            r.mixed = row.mixed;
        }
        for r in &rows {
            row_ui(ui, state, r, t, ops);
        }
    }
}

/// The renderer's compile error for the section's shader, if any.
pub(crate) fn shader_error_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    section: &inspect::Section,
    t: &Tokens,
) {
    let Some(handle) = state
        .shader_info(&section_shader(section))
        .and_then(|i| i.handle)
    else {
        return;
    };
    let Some(e) = state.shader_error(handle) else {
        return;
    };
    ui.add_space(4.0);
    let w = ui.available_width() - 2.0 * t.pad_x;
    egui::Frame::NONE
        .fill(t.error.gamma_multiply(0.12))
        .corner_radius(CornerRadius::same(t.radius))
        .inner_margin(egui::Margin::same(6))
        .outer_margin(egui::Margin::symmetric(t.pad_x as i8, 0))
        .show(ui, |ui| {
            ui.set_width(w - 12.0);
            ui.label(
                egui::RichText::new("Shader error")
                    .font(t.strong_small())
                    .color(t.error),
            );
            ui.label(egui::RichText::new(e).font(t.mono()).color(t.error_text));
        });
}
