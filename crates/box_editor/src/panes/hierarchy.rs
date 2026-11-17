//! Hierarchy pane: saved entities in scene order, flattened to visible rows
//! and virtualised with `ScrollArea::show_rows`.
//!
//! Click selects, Cmd-click (Ctrl elsewhere) toggles, Shift-click selects
//! the range of visible rows from the last plain or Cmd-clicked row (Cmd +
//! Shift adds the range); double-click or F2 renames, dragging a
//! row onto another reparents it (middle of a row) or reorders it (top or
//! bottom edge), and dragging near the list's top or bottom edge scrolls it.
//! The context menu adds children, duplicates, deletes, hides and locks. A
//! material dragged from the asset browser onto a row is assigned to that
//! entity. Double-clicking a row's icon frames the entity in the viewport.
//!
//! Each row has an eye (hide in the viewport) and a lock (not picked or
//! moved in the viewport) at its right edge, shown while the row is hovered
//! and always when on, dimmed when only an ancestor is hidden or locked.
//! They are editor state (`visibility.rs`), not scene edits, and are not
//! shown during Play.
//!
//! A pinned Scene row at the top shows the scene's settings (Environment)
//! in the inspector. Right-clicking empty space below the rows offers Add;
//! an empty scene shows how to create objects. All changes go through the
//! `EditStack`.

use std::collections::HashSet;

use box_core::identity::{Name, Uid};
use box_core::prelude::{Children, Entity, World};
use box_render::{Camera, DirectionalLight, MeshRenderer, PointLight};
use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};

use crate::state::{EditorState, HierarchyRow, NewEntity};
use crate::theme::{Tokens, tokens};
use crate::widgets::{self, EntityKind};

pub fn kind_of(world: &World, e: Entity) -> EntityKind {
    if world.has::<Camera>(e) {
        EntityKind::Camera
    } else if world.has::<DirectionalLight>(e) {
        EntityKind::Light
    } else if world.has::<PointLight>(e) {
        EntityKind::PointLight
    } else if world.has::<box_render::SpotLight>(e) {
        EntityKind::SpotLight
    } else if world.has::<box_render::AmbientVolume>(e) {
        EntityKind::AmbientVolume
    } else if world.has::<box_render::ParticleEmitter>(e) {
        EntityKind::Particles
    } else if world.has::<MeshRenderer>(e) || world.has::<box_core::prelude::Primitive>(e) {
        EntityKind::Mesh
    } else if world.get::<Children>(e).is_some_and(|c| !c.0.is_empty()) {
        EntityKind::Group
    } else {
        EntityKind::Empty
    }
}

/// Children with a uid, without generated entities (prefab contents and
/// derived parts: runtime data rebuilt from their source; the instance root
/// or owner stands for them).
fn saved_children(world: &World, e: Entity) -> Vec<(Entity, Uid)> {
    world
        .children(e)
        .into_iter()
        .filter(|c| !box_core::prefab::is_generated(world, *c))
        .filter_map(|c| world.uid(c).map(|u| (c, u)))
        .collect()
}

fn name_of(world: &World, e: Entity) -> String {
    world
        .get::<Name>(e)
        .map(|n| n.0.clone())
        .unwrap_or_default()
}

/// Depth-first flatten in scene order. With a `filter`, only entities whose
/// name contains it (case-insensitive) are shown, plus their ancestors for
/// context, all expanded.
pub fn flatten(world: &World, expanded: &HashSet<Uid>, filter: &str, out: &mut Vec<HierarchyRow>) {
    out.clear();
    let filter = filter.trim().to_lowercase();
    // With a filter: the set of entities that match or have a matching descendant.
    let mut keep: Option<HashSet<Entity>> = None;
    if !filter.is_empty() {
        let mut set = HashSet::new();
        type Listed<'a> = box_core::prefab::NotGenerated<(Entity, &'a Name)>;
        for (e, n) in world.query::<Listed>().iter() {
            if n.0.to_lowercase().contains(&filter) {
                let mut cur = Some(e);
                while let Some(c) = cur {
                    if !set.insert(c) {
                        break;
                    }
                    cur = world.parent(c);
                }
            }
        }
        keep = Some(set);
    }
    let mut stack: Vec<(Entity, Uid, u16)> = world
        .roots()
        .iter()
        .rev()
        // A drawing tool's preview is a generated root.
        .filter(|e| !box_core::prefab::is_generated(world, **e))
        .filter_map(|e| world.uid(*e).map(|u| (*e, u, 0)))
        .collect();
    while let Some((e, uid, depth)) = stack.pop() {
        if keep.as_ref().is_some_and(|k| !k.contains(&e)) {
            continue;
        }
        let children = saved_children(world, e);
        let context_only = keep.is_some() && !name_of(world, e).to_lowercase().contains(&filter);
        out.push(HierarchyRow {
            entity: e,
            uid,
            depth,
            children: children.len() as u32,
            context_only,
        });
        if keep.is_some() || expanded.contains(&uid) {
            stack.extend(children.into_iter().rev().map(|(c, u)| (c, u, depth + 1)));
        }
    }
}

fn refresh(state: &mut EditorState) {
    let h = &mut state.hierarchy;
    let count = state.world.entity_count();
    if !h.dirty && h.built_for_count == count {
        return;
    }
    if h.expand_all_once {
        h.expand_all_once = false;
        for (e, c) in state.world.query::<(Entity, &Children)>().iter() {
            if !c.0.is_empty()
                && let Some(u) = state.world.uid(e)
            {
                h.expanded.insert(u);
            }
        }
    }
    flatten(&state.world, &h.expanded, &h.filter, &mut h.rows);
    h.listed_count = state
        .world
        .query::<box_core::prefab::NotGenerated<&Uid>>()
        .iter()
        .count();
    h.built_for_count = count;
    h.dirty = false;
}

/// Height of the auto-scroll band at each edge, in rows.
const AUTOSCROLL_BAND_ROWS: f32 = 2.0;
/// Auto-scroll speed at (or beyond) the edge, in rows per second.
const AUTOSCROLL_MAX_ROWS_PER_S: f32 = 30.0;

/// Auto-scroll speed while dragging a row, in points per second (negative
/// scrolls up): zero away from the edges, rising linearly across the `band`
/// next to the top or bottom edge of `top..bottom`, and `max` at or beyond
/// the edge. The band shrinks to a quarter of a short list.
pub fn autoscroll_speed(y: f32, top: f32, bottom: f32, band: f32, max: f32) -> f32 {
    let band = band.min((bottom - top) / 4.0).max(1.0);
    let up = (top + band - y) / band;
    let down = (y - (bottom - band)) / band;
    if up > 0.0 {
        -max * up.min(1.0)
    } else if down > 0.0 {
        max * down.min(1.0)
    } else {
        0.0
    }
}

/// Where a dragged row would land.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drop {
    Before(Uid),
    Into(Uid),
    After(Uid),
}

/// How a row click changes the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Click {
    Select,
    Toggle,
    /// Shift: the rows from the anchor to this one; `add` (Cmd + Shift)
    /// keeps the current selection.
    Range {
        add: bool,
    },
}

enum RowAction {
    Select(Click),
    Toggle,
    Rename,
    /// Double-click on the icon: select and frame it.
    Frame,
    Context(ContextAction),
    /// The eye: hide or show in the viewport.
    ToggleHidden,
    /// The lock: lock or unlock in the viewport.
    ToggleLocked,
}

/// Width of the eye and lock column at the right of each row.
const ICONS_W: f32 = 38.0;

/// The eye and lock of a row, at its right edge: shown while the row is
/// hovered, and always when the entity is hidden or locked (dimmed when only
/// an ancestor is). Returns the toggle clicked.
fn visibility_icons(
    ui: &mut Ui,
    state: &EditorState,
    row: HierarchyRow,
    rect: Rect,
    hovered: bool,
    selected: bool,
    t: &Tokens,
) -> Option<RowAction> {
    let cy = rect.center().y;
    let own_hidden = state.is_hidden(row.uid);
    let own_locked = state.is_locked(row.uid);
    let world = &state.world;
    let parent_blocked = |set: fn(&EditorState, Uid) -> bool| {
        let mut cur = world.parent(row.entity);
        while let Some(p) = cur {
            if world.uid(p).is_some_and(|u| set(state, u)) {
                return true;
            }
            cur = world.parent(p);
        }
        false
    };
    let inherited_hidden = !own_hidden && parent_blocked(EditorState::is_hidden);
    let inherited_locked = !own_locked && parent_blocked(EditorState::is_locked);
    let mut action = None;
    let base = if selected { t.accent_text } else { t.text_weak };
    let eye = Rect::from_center_size(pos2(rect.right() - 30.0, cy), vec2(18.0, t.row_h));
    let lock = Rect::from_center_size(pos2(rect.right() - 11.0, cy), vec2(18.0, t.row_h));
    let id = ui.scope_id().with(("vis", row.uid));
    let p = ui.painter().clone();
    if hovered || own_hidden || inherited_hidden {
        let r = ui.interact(eye, id.with("eye"), Sense::click());
        let col = match (r.hovered(), own_hidden, inherited_hidden) {
            (true, _, _) => t.text_strong,
            (_, true, _) => base,
            (_, _, true) => t.text_faint,
            _ => t.text_faint,
        };
        eye_glyph(&p, eye.center(), !(own_hidden || inherited_hidden), col);
        let tip = match (own_hidden, inherited_hidden) {
            (true, _) => "Hidden in the viewport: click to show (H toggles the selection)",
            (_, true) => "Hidden with its parent",
            _ => "Hide in the viewport (H)",
        };
        if r.on_hover_text(tip).clicked() {
            action = Some(RowAction::ToggleHidden);
        }
    }
    if hovered || own_locked || inherited_locked {
        let r = ui.interact(lock, id.with("lock"), Sense::click());
        let col = match (r.hovered(), own_locked, inherited_locked) {
            (true, _, _) => t.text_strong,
            (_, true, _) => base,
            _ => t.text_faint,
        };
        lock_glyph(&p, lock.center(), own_locked || inherited_locked, col);
        let tip = match (own_locked, inherited_locked) {
            (true, _) => "Locked: not picked or moved in the viewport; click to unlock",
            (_, true) => "Locked with its parent",
            _ => "Lock: not picked or moved in the viewport",
        };
        if r.on_hover_text(tip).clicked() {
            action = Some(RowAction::ToggleLocked);
        }
    }
    action
}

/// An eye (open) or an eye struck through (closed).
fn eye_glyph(p: &egui::Painter, c: egui::Pos2, open: bool, color: egui::Color32) {
    let s = Stroke::new(1.2, color);
    let (w, h) = (6.0, 3.6);
    let n = 10;
    let lid = |sign: f32| -> Vec<egui::Pos2> {
        (0..=n)
            .map(|i| {
                let x = -w + 2.0 * w * i as f32 / n as f32;
                let y = sign * h * (1.0 - (x / w).powi(2));
                c + vec2(x, y)
            })
            .collect()
    };
    p.add(egui::Shape::line(lid(-1.0), s));
    p.add(egui::Shape::line(lid(1.0), s));
    p.circle_filled(c, 1.6, color);
    if !open {
        p.line_segment([c + vec2(-5.5, 5.0), c + vec2(5.5, -5.0)], s);
    }
}

/// A padlock, closed or open.
fn lock_glyph(p: &egui::Painter, c: egui::Pos2, closed: bool, color: egui::Color32) {
    let s = Stroke::new(1.2, color);
    let body = Rect::from_center_size(c + vec2(0.0, 2.0), vec2(9.0, 6.5));
    if closed {
        p.rect_filled(body, 1.5, color);
    } else {
        p.rect_stroke(body, 1.5, s, egui::StrokeKind::Inside);
    }
    // The shackle: an arch, lifted to one side when open.
    let lift = if closed { 0.0 } else { -2.0 };
    let (r, top) = (2.8, body.top());
    let pts: Vec<egui::Pos2> = (0..=10)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 10.0;
            pos2(c.x - r * a.cos(), top - 0.5 + lift - r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(pts, s));
}

enum ContextAction {
    AddChild(NewEntity),
    Duplicate,
    Delete,
    Rename,
    ToggleHidden,
    ToggleLocked,
}

pub fn hierarchy_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    search_bar(ui, state, &t);
    scene_row(ui, state, &t);
    refresh(state);

    // F2 renames the primary selection (the asset browser's file after a
    // click there).
    if ui.input(|i| crate::shortcuts::pressed(i, crate::shortcuts::Command::Rename))
        && !state.assets.active
        && !ui.ctx().egui_wants_keyboard_input()
        && let Some(u) = state.primary()
        && let Some(e) = state.world.entity_by_uid(u)
    {
        state.hierarchy.renaming = Some((u, name_of(&state.world, e)));
    }

    ui.spacing_mut().item_spacing.y = 0.0;
    let mut area = egui::ScrollArea::vertical()
        .id_salt("hierarchy")
        .auto_shrink([false, false]);
    if let Some(target) = state.hierarchy.scroll_to.take()
        && let Some(i) = state.hierarchy.rows.iter().position(|r| r.uid == target)
    {
        area = area.vertical_scroll_offset(((i as f32) - 4.0).max(0.0) * t.row_h);
    }
    let n = state.hierarchy.rows.len();
    let mut actions: Vec<(Uid, RowAction)> = Vec::new();
    let mut drop_target: Option<Drop> = None;
    let mut rename_done: Option<Option<String>> = None;
    let mut material_drop = None;
    let dragging = state.hierarchy.dragging;
    let pointer = ui.input(|i| i.pointer.interact_pos());
    // Dragging a row near the top or bottom edge scrolls the list.
    let view = ui.available_rect_before_wrap();
    let scroll_speed = match (dragging, pointer) {
        (Some(_), Some(p)) if view.x_range().contains(p.x) => autoscroll_speed(
            p.y,
            view.top(),
            view.bottom(),
            AUTOSCROLL_BAND_ROWS * t.row_h,
            AUTOSCROLL_MAX_ROWS_PER_S * t.row_h,
        ),
        _ => 0.0,
    };
    if scroll_speed != 0.0 {
        // Keep frames coming while the pointer rests in the band.
        ui.ctx().request_repaint();
    }
    let scroll_step = scroll_speed * ui.input(|i| i.stable_dt).min(0.1);
    // Empty space below the rows (registered first, so rows are on top of
    // it): a click selects nothing, a right-click offers Add.
    let background = ui.interact(view, ui.scope_id().with("hierarchy_bg"), Sense::click());
    background.context_menu(|ui| {
        ui.menu_button("Add", |ui| crate::toolbar::add_menu(ui, state));
    });
    if background.clicked() {
        state.select(None);
    }
    if state.scene_is_empty() && state.hierarchy.filter.trim().is_empty() {
        let g = ui.painter().layout(
            "Empty scene\n+ Add, or right-click here to create objects".into(),
            t.body(),
            t.text_weak,
            (view.width() - 24.0).max(40.0),
        );
        let at = pos2(view.center().x - g.size().x / 2.0, view.top() + 24.0);
        ui.painter().galley(at, g, t.text_weak);
    }
    area.show_rows(ui, t.row_h, n, |ui, range| {
        if scroll_step != 0.0 {
            // Negative y moves the content up, i.e. scrolls down.
            ui.scroll_with_delta_animation(
                vec2(0.0, -scroll_step),
                egui::style::ScrollAnimation::none(),
            );
        }
        for i in range {
            let row = state.hierarchy.rows[i];
            let (action, rect) = row_ui(ui, state, row, &mut rename_done);
            if let Some(a) = action {
                actions.push((row.uid, a));
            }
            // A material dragged from the asset browser onto a row.
            if let (Some(drag), Some(p)) = (&state.assets.drag, pointer)
                && drag.ty == crate::assets::RefType::Material
                && rect.contains(p)
            {
                drop_indicator(ui, rect, Drop::Into(row.uid), row.depth, &t);
                if ui.input(|i| i.pointer.any_released()) {
                    material_drop = Some((row.uid, drag.item.clone()));
                }
            }
            if let (Some(d), Some(p)) = (dragging, pointer)
                && d != row.uid
                && rect.contains(p)
            {
                let f = (p.y - rect.top()) / rect.height();
                let target = if f < 0.25 {
                    Drop::Before(row.uid)
                } else if f > 0.75 {
                    Drop::After(row.uid)
                } else {
                    Drop::Into(row.uid)
                };
                drop_indicator(ui, rect, target, row.depth, &t);
                drop_target = Some(target);
            }
        }
    });

    if let Some((uid, item)) = material_drop {
        state.assign_material(uid, &item);
    }

    // Rename finished (Enter / focus lost) or cancelled (Escape). An
    // unchanged name records nothing (`EditorState::rename`).
    if let Some(result) = rename_done
        && let Some((uid, _)) = state.hierarchy.renaming.take()
        && let Some(name) = result
    {
        state.rename(uid, name.trim());
    }

    for (uid, action) in actions {
        match action {
            RowAction::Select(click) => click_row(state, uid, click),
            RowAction::Toggle => {
                let h = &mut state.hierarchy;
                if !h.expanded.remove(&uid) {
                    h.expanded.insert(uid);
                }
                h.dirty = true;
            }
            RowAction::Frame => {
                state.select(Some(uid));
                state.frame_view();
            }
            RowAction::ToggleHidden | RowAction::Context(ContextAction::ToggleHidden) => {
                state.toggle_hidden(uid)
            }
            RowAction::ToggleLocked | RowAction::Context(ContextAction::ToggleLocked) => {
                state.toggle_locked(uid)
            }
            RowAction::Rename | RowAction::Context(ContextAction::Rename) => {
                if let Some(e) = state.world.entity_by_uid(uid) {
                    state.hierarchy.renaming = Some((uid, name_of(&state.world, e)));
                }
            }
            RowAction::Context(ContextAction::AddChild(kind)) => {
                state.add_entity(kind, Some(uid));
            }
            RowAction::Context(ContextAction::Duplicate) => {
                if !state.is_selected(uid) {
                    state.selection = vec![uid];
                }
                state.duplicate_selection();
            }
            RowAction::Context(ContextAction::Delete) => {
                if !state.is_selected(uid) {
                    state.selection = vec![uid];
                }
                state.delete_selection();
            }
        }
    }

    // Drag and drop.
    if dragging.is_some() && ui.input(|i| i.pointer.any_released()) {
        if let (Some(uid), Some(target)) = (state.hierarchy.dragging.take(), drop_target) {
            drop_entity(state, uid, target);
        }
        state.hierarchy.dragging = None;
    }
}

/// Applies a row click. A range runs over the visible rows from the anchor
/// (the last row clicked without Shift) to `uid`, which becomes the primary
/// selection; without a visible anchor it selects `uid` alone.
fn click_row(state: &mut EditorState, uid: Uid, click: Click) {
    match click {
        Click::Select => {
            state.selection = vec![uid];
            state.hierarchy.anchor = Some(uid);
        }
        Click::Toggle => {
            state.toggle_selected(uid);
            state.hierarchy.anchor = Some(uid);
        }
        Click::Range { add } => {
            let rows = &state.hierarchy.rows;
            let to = rows.iter().position(|r| r.uid == uid);
            let from = state
                .hierarchy
                .anchor
                .and_then(|a| rows.iter().position(|r| r.uid == a));
            let mut range: Vec<Uid> = match (from, to) {
                (Some(a), Some(b)) => {
                    let ids = rows[a.min(b)..=a.max(b)].iter().map(|r| r.uid);
                    if a <= b {
                        ids.collect()
                    } else {
                        ids.rev().collect()
                    }
                }
                _ => {
                    state.hierarchy.anchor = Some(uid);
                    vec![uid]
                }
            };
            if add {
                let mut base: Vec<Uid> = state
                    .selection
                    .iter()
                    .filter(|u| !range.contains(u))
                    .copied()
                    .collect();
                base.append(&mut range);
                range = base;
            }
            state.selection = range;
        }
    }
}

/// The pinned Scene row: selects nothing, so the inspector shows the
/// scene's settings (see [`EditorState::show_scene_settings`]); highlighted
/// while it does.
fn scene_row(ui: &mut Ui, state: &mut EditorState, t: &Tokens) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), t.row_h), Sense::click());
    let on = state.scene_settings_shown();
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 0.0, t.accent_bg);
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, t.hover);
    }
    let (fg, weak) = if on {
        (t.accent_text, t.accent_text)
    } else {
        (t.text, t.text_faint)
    };
    // A sun over a horizon.
    let c = pos2(rect.left() + 20.0, rect.center().y);
    p.circle_stroke(c + vec2(0.0, -1.0), 3.0, Stroke::new(1.1, fg));
    p.hline(c.x - 6.0..=c.x + 6.0, c.y + 4.0, Stroke::new(1.1, fg));
    p.text(
        pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Scene",
        t.body(),
        fg,
    );
    p.with_clip_rect(rect).text(
        pos2(rect.right() - 12.0, rect.center().y),
        Align2::RIGHT_CENTER,
        &state.scene.name,
        t.small(),
        weak,
    );
    p.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, t.border),
    );
    if resp
        .on_hover_text("Scene settings: sky, fog, exposure and tone mapping")
        .clicked()
    {
        state.show_scene_settings();
    }
}

fn search_bar(ui: &mut Ui, state: &mut EditorState, t: &Tokens) {
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    let field = row.shrink2(vec2(8.0, 5.0));
    ui.painter()
        .rect_filled(field, egui::CornerRadius::same(t.radius + 1), t.field);
    widgets::search_glyph(
        ui.painter(),
        pos2(field.left() + 12.0, field.center().y),
        t.text_weak,
    );
    let inner = Rect::from_min_max(
        pos2(field.left() + 24.0, field.top()),
        pos2(field.right() - 6.0, field.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let r = child.add(
        egui::TextEdit::singleline(&mut state.hierarchy.filter)
            .frame(egui::Frame::NONE)
            .hint_text(format!("Search {} entities", state.hierarchy.listed_count))
            .font(t.body())
            .desired_width(inner.width()),
    );
    if r.changed() {
        state.hierarchy.dirty = true;
    }
    ui.painter().hline(
        row.x_range(),
        row.bottom() - 0.5,
        Stroke::new(1.0, t.border),
    );
}

fn drop_indicator(ui: &Ui, rect: Rect, target: Drop, depth: u16, t: &Tokens) {
    let x = rect.left() + 8.0 + depth as f32 * 14.0;
    let p = ui.painter();
    match target {
        Drop::Before(_) => {
            p.hline(
                x..=rect.right(),
                rect.top() + 1.0,
                Stroke::new(2.0, t.accent),
            );
        }
        Drop::After(_) => {
            p.hline(
                x..=rect.right(),
                rect.bottom() - 1.0,
                Stroke::new(2.0, t.accent),
            );
        }
        Drop::Into(_) => {
            p.rect_stroke(
                rect.shrink(1.0),
                egui::CornerRadius::same(t.radius),
                Stroke::new(1.5, t.accent),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// Moves `uid` relative to the drop target. Moving an entity into its own
/// subtree is rejected by the command layer and logged.
fn drop_entity(state: &mut EditorState, uid: Uid, target: Drop) {
    let world = &state.world;
    let (Some(e), Some(t)) = (
        world.entity_by_uid(uid),
        world.entity_by_uid(match target {
            Drop::Before(u) | Drop::Into(u) | Drop::After(u) => u,
        }),
    ) else {
        return;
    };
    match target {
        Drop::Into(parent) => {
            state.reparent(uid, Some(parent), None);
        }
        Drop::Before(_) | Drop::After(_) => {
            let at = world.placement(t);
            let mut index = at.index + usize::from(matches!(target, Drop::After(_)));
            // Removing the dragged entity first shifts later siblings up.
            let from = world.placement(e);
            if from.parent == at.parent && from.index < index {
                index -= 1;
            }
            state.reparent(uid, at.parent, Some(index));
        }
    }
}

fn row_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    row: HierarchyRow,
    rename_done: &mut Option<Option<String>>,
) -> (Option<RowAction>, Rect) {
    let t = tokens(ui.ctx());
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), t.row_h), Sense::click_and_drag());
    if !ui.is_rect_visible(rect) {
        return (None, rect);
    }
    let e = row.entity;
    let selected = state.is_selected(row.uid);
    let primary = state.primary() == Some(row.uid);
    let has_children = row.children > 0;
    let filtering = !state.hierarchy.filter.trim().is_empty();
    let open = filtering || state.hierarchy.expanded.contains(&row.uid);
    let indent = 14.0;
    let x0 = rect.left() + 8.0 + row.depth as f32 * indent;
    let cy = rect.center().y;
    let p = ui.painter();

    if selected {
        p.rect_filled(
            rect,
            0.0,
            if primary { t.accent_bg } else { t.field_active },
        );
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, t.hover);
    }
    if !selected {
        for d in 1..=row.depth {
            let x = rect.left() + 8.0 + (d as f32 - 1.0) * indent + 5.0;
            p.vline(x.round() + 0.5, rect.y_range(), Stroke::new(1.0, t.guide));
        }
    }
    let weak = if selected { t.accent_text } else { t.text_weak };
    if has_children {
        widgets::chevron(p, pos2(x0 + 5.0, cy), open, weak);
    }
    let kind = kind_of(&state.world, e);
    widgets::kind_glyph(p, pos2(x0 + 20.0, cy), kind, weak);

    // Inline rename.
    if let Some((uid, buffer)) = &mut state.hierarchy.renaming
        && *uid == row.uid
    {
        let edit_rect = Rect::from_min_max(
            pos2(x0 + 30.0, rect.top() + 1.0),
            pos2(rect.right() - 8.0, rect.bottom() - 1.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect));
        let r = child.add(
            egui::TextEdit::singleline(buffer)
                .font(t.body())
                .margin(egui::Margin::symmetric(4, 1))
                .desired_width(edit_rect.width()),
        );
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            *rename_done = Some(None);
        } else if r.lost_focus() {
            *rename_done = Some(Some(buffer.clone()));
        }
        return (None, rect);
    }

    // Hidden (eye) and locked (lock) state: editor-only, not during Play.
    let editing = !state.is_playing();
    let hidden = editing && state.hidden_in_viewport(e);
    let name_col = if selected {
        t.accent_text
    } else if row.context_only || hidden {
        t.text_weak
    } else {
        t.text
    };
    let icons_w = if editing { ICONS_W } else { 0.0 };
    let label = state.display_name(e);
    // Derived parts and rule warnings of this row (`generated.rs`).
    let generated = state.generated_mark(row.uid).cloned().unwrap_or_default();
    let gear_w = if generated.parts > 0 { 22.0 } else { 0.0 };
    p.with_clip_rect(Rect::from_min_max(
        rect.min,
        pos2(rect.right() - 56.0 - icons_w - gear_w, rect.bottom()),
    ))
    .text(
        pos2(x0 + 32.0, cy),
        Align2::LEFT_CENTER,
        label,
        t.body(),
        name_col,
    );
    if has_children && !open {
        p.text(
            pos2(rect.right() - 12.0 - icons_w, cy),
            Align2::RIGHT_CENTER,
            row.children.to_string(),
            t.mono(),
            if selected {
                t.accent_text
            } else {
                t.text_faint
            },
        );
    }
    let toggle = if editing {
        let over = ui.rect_contains_pointer(rect);
        visibility_icons(ui, state, row, rect, over, selected, &t)
    } else {
        None
    };
    let p = ui.painter();

    // A mesh or prefab asset that failed or is missing: the entity draws
    // nothing, so say why.
    // Rule warnings get the same glyph; a gear says rules made parts here.
    let warn_x = rect.right() - icons_w - if has_children && !open { 42.0 } else { 20.0 };
    let mut tips: Vec<String> = Vec::new();
    if let Some(error) = state.asset_error(row.uid) {
        tips.push(error.to_owned());
    }
    tips.extend(generated.warnings.iter().cloned());
    if !tips.is_empty() {
        crate::feedback::warn_glyph(p, pos2(warn_x, cy), &t);
    }
    if generated.parts > 0 {
        let x = if tips.is_empty() {
            warn_x
        } else {
            warn_x - 22.0
        };
        let col = if selected {
            t.accent_text
        } else {
            t.text_faint
        };
        crate::generated::gear_glyph(p, pos2(x, cy), col);
        tips.push(format!(
            "{} generated part{} (rebuilt from this entity, not saved)",
            generated.parts,
            if generated.parts == 1 { "" } else { "s" }
        ));
    }
    let resp = if tips.is_empty() {
        resp
    } else {
        resp.on_hover_text(tips.join("\n"))
    };
    if let Some(action) = toggle {
        return (Some(action), rect);
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        state.hierarchy.dragging = Some(row.uid);
    }
    let mut context = None;
    resp.context_menu(|ui| {
        ui.menu_button("Add Child", |ui| {
            for kind in NewEntity::ALL {
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
                    context = Some(ContextAction::AddChild(kind));
                }
            }
        });
        if ui.button("Rename").clicked() {
            context = Some(ContextAction::Rename);
        }
        if editing {
            let hide = if state.is_hidden(row.uid) {
                "Show in Viewport"
            } else {
                "Hide in Viewport"
            };
            if ui.button(hide).clicked() {
                context = Some(ContextAction::ToggleHidden);
            }
            let lock = if state.is_locked(row.uid) {
                "Unlock"
            } else {
                "Lock in Viewport"
            };
            if ui
                .button(lock)
                .on_hover_text("Locked entities are not picked or moved in the viewport")
                .clicked()
            {
                context = Some(ContextAction::ToggleLocked);
            }
        }
        if ui
            .add(
                egui::Button::new("Duplicate").shortcut_text(crate::shortcuts::text(
                    ui.ctx(),
                    crate::shortcuts::Command::Duplicate,
                )),
            )
            .clicked()
        {
            context = Some(ContextAction::Duplicate);
        }
        if ui
            .add(
                egui::Button::new("Delete").shortcut_text(ui.ctx().format_shortcut(
                    crate::shortcuts::main_key(crate::shortcuts::Command::Delete),
                )),
            )
            .clicked()
        {
            context = Some(ContextAction::Delete);
        }
    });
    if let Some(c) = context {
        return (Some(RowAction::Context(c)), rect);
    }

    let on_chevron = has_children
        && resp
            .interact_pointer_pos()
            .is_some_and(|pp| pp.x > x0 - 4.0 && pp.x < x0 + 13.0);
    let on_icon = resp
        .interact_pointer_pos()
        .is_some_and(|pp| pp.x >= x0 + 13.0 && pp.x < x0 + 28.0);
    let action = if resp.double_clicked() && on_icon {
        Some(RowAction::Frame)
    } else if resp.double_clicked() && !on_chevron {
        Some(RowAction::Rename)
    } else if resp.clicked() {
        if on_chevron {
            Some(RowAction::Toggle)
        } else {
            let m = ui.input(|i| i.modifiers);
            Some(RowAction::Select(if m.shift {
                Click::Range { add: m.command }
            } else if m.command {
                Click::Toggle
            } else {
                Click::Select
            }))
        }
    } else {
        None
    };
    (action, rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::prelude::Transform;
    use box_core::world::Placement;

    #[test]
    fn flatten_follows_scene_order_expansion_and_filter() {
        let mut w = World::new();
        let id = |n: u128| Uid::from_u128(n);
        let a = w
            .spawn_saved(id(1), "House", (Transform::default(),))
            .unwrap();
        let b = w
            .spawn_saved(id(2), "Door", (Transform::default(),))
            .unwrap();
        let c = w
            .spawn_saved(id(3), "Roof", (Transform::default(),))
            .unwrap();
        let d = w
            .spawn_saved(id(4), "Chimney", (Transform::default(),))
            .unwrap();
        w.place(c, Placement::last(Some(id(1)))).unwrap();
        w.place(b, Placement::last(Some(id(1)))).unwrap();
        w.place(d, Placement::last(Some(id(3)))).unwrap();
        let mut rows = Vec::new();
        flatten(&w, &HashSet::new(), "", &mut rows);
        assert_eq!(rows.iter().map(|r| r.entity).collect::<Vec<_>>(), vec![a]);
        assert_eq!(rows[0].children, 2);

        let expanded: HashSet<Uid> = [id(1), id(3)].into();
        flatten(&w, &expanded, "", &mut rows);
        let got: Vec<(Entity, u16)> = rows.iter().map(|r| (r.entity, r.depth)).collect();
        assert_eq!(got, vec![(a, 0), (c, 1), (d, 2), (b, 1)]);

        // Filtering shows matches with their ancestors, expanded.
        flatten(&w, &HashSet::new(), "chim", &mut rows);
        let got: Vec<(Entity, bool)> = rows.iter().map(|r| (r.entity, r.context_only)).collect();
        assert_eq!(got, vec![(a, true), (c, true), (d, false)]);
    }
}
