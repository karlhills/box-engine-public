//! Viewport pane: shows the scene texture rendered by box_render, drives the
//! editor camera while hovered (or while a drag that started here is held),
//! and draws overlays.
//!
//! A left press on a gizmo handle starts a transform drag (one undo step,
//! with a readout of the change next to the cursor); a left click elsewhere
//! picks an icon (lights, cameras, empties; `crate::icons`) or else the
//! nearest mesh (empty space clears) and a left drag elsewhere draws a
//! marquee (`crate::marquee`). Shift adds to the selection, Cmd (Ctrl
//! elsewhere) toggles. A click on a generated part selects its owner; Cmd +
//! Alt + click (Ctrl + Alt elsewhere) inspects the part read-only
//! (`crate::generated`). While a viewport tool is active (`crate::tools`)
//! the left button goes to it instead of the gizmo, marquee and picking,
//! and its hint shows at the top. Owners with rule warnings get a badge.
//! Alt + left, middle and Alt + right drags navigate;
//! holding the right button flies (`crate::camera`). The mode pill opens the
//! View menu (grid, icons, collider wireframes, axis views) and clicking an
//! axis of the axis gizmo turns the view to look along it.
//!
//! During Play the viewport shows the play world (through the game's camera
//! when it has an active one) inside a tinted border. A left click gives the
//! game keyboard and pointer input, Escape or a click elsewhere takes it
//! back; while the game has input, or the game camera is shown, the editor's
//! own viewport tools are off. The game's own UI must stay fully visible,
//! so during Play the editor's status (the view, the Play state and input
//! hint, and in the game camera's view the selection) moves out of the
//! image into the pane's tab bar ([`header_ui`]); the game camera's view
//! gets no chips at all, only the border.

use egui::{
    Align2, Color32, Event, Key, MouseWheelUnit, PointerButton, Rect, Sense, Stroke, StrokeKind,
    Ui, pos2, vec2,
};
use glam::Vec2;

use crate::camera::{FlyKeys, ViewportPointer};
use crate::icons::ColliderDisplay;
use crate::play::PlayState;
use crate::state::{EditorState, SelectMode};
use crate::theme::{Tokens, tokens};
use crate::widgets::overlay_pill;

pub fn viewport_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    let ppp = ui.ctx().pixels_per_point();
    // Snap the image to whole physical pixels so it maps 1:1 onto the target.
    let full = ui.max_rect();
    let min = (full.min.to_vec2() * ppp).round() / ppp;
    let max = (full.max.to_vec2() * ppp).round() / ppp;
    let rect = Rect::from_min_max(min.to_pos2(), max.to_pos2());
    state.viewport.request_px = [
        (rect.width() * ppp).round().max(1.0) as u32,
        (rect.height() * ppp).round().max(1.0) as u32,
    ];

    let resp = ui.interact(
        rect,
        ui.scope_id().with("viewport"),
        Sense::click_and_drag(),
    );
    state.viewport.hovered = resp.hovered();
    state.viewport.shown = true;
    state.viewport.rect = Some(rect);
    let play = state.play_state();
    if play != PlayState::Editing {
        let (focus_click, any_press) = ui.input(|i| {
            (
                i.pointer.primary_pressed() && !i.modifiers.alt,
                i.pointer.any_pressed(),
            )
        });
        if focus_click && resp.hovered() {
            state.set_game_focus(true);
        } else if any_press && !resp.hovered() {
            state.set_game_focus(false);
        }
    }
    let game_view = state.shows_game_camera();
    update_icons(ui, state, rect, game_view);
    if !game_view && !state.game_focus() {
        interact(ui, state, &resp, rect);
        navigate(ui, state, &resp, rect);
    }
    let dt = ui.input(|i| i.unstable_dt).clamp(0.0, 0.1);
    if state.camera.tick(dt) || state.camera.flying {
        ui.ctx().request_repaint();
    }

    let p = ui.painter_at(rect);
    match state.viewport.texture {
        Some(id) => {
            p.image(
                id,
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            p.rect_filled(rect, 0.0, t.bg_app);
        }
    }

    if !game_view && state.viewport.view.icons {
        crate::icons::paint(&p, rect.min, state, &t);
    }
    if !game_view {
        // Owners with rule warnings (`generated.rs`) and the active tool's
        // 2D overlay (`tools.rs`).
        crate::generated::warning_badges(ui, rect, state, &t);
        state.paint_tool(&p, rect);
    }
    if !game_view {
        view_menu(ui, &p, rect, state, &t);
        let text = selection_text(state);
        overlay_label(
            ui,
            &p,
            rect.left_bottom() + vec2(10.0, -10.0),
            Align2::LEFT_BOTTOM,
            text,
            &t,
        );
        hints(ui, &p, rect, state, &t);
        asset_drop(ui, &p, rect, state, &t);
        marquee_overlay(&p, rect, state, &t);
        axis_gizmo(ui, &p, rect, state, &t);
        gizmo_readout(ui, &p, rect, state, &t);
        fly_speed_pill(ui, &p, rect, state, &t);
    }
    if let Some((color, _)) = play_status(state, play, &t) {
        // Inside the image's edge: a game's UI keeps its usual margins.
        p.rect_stroke(rect, 0.0, Stroke::new(2.0, color), StrokeKind::Inside);
    }
}

/// The selection, as the viewport's bottom-left chip (and the tab bar
/// during Play in the game camera's view) shows it.
fn selection_text(state: &EditorState) -> String {
    match (state.primary_entity(), state.selection.len()) {
        (Some(e), 1) => format!("{}  ·  1 selected", state.display_name(e)),
        (Some(e), n) => format!("{}  ·  {n} selected", state.display_name(e)),
        (None, _) => match state
            .inspected_part()
            .and_then(|u| state.world.entity_by_uid(u))
        {
            Some(e) => format!("{}  ·  generated", state.display_name(e)),
            None => "Nothing selected".into(),
        },
    }
}

/// The Play state's colour (accent while running, the warning colour while
/// paused; the viewport border and the tab bar's dot) and its input hint.
/// `None` while editing.
fn play_status(
    state: &EditorState,
    play: PlayState,
    t: &Tokens,
) -> Option<(Color32, &'static str)> {
    Some(match play {
        PlayState::Editing => return None,
        PlayState::Paused => (t.warn, "Paused"),
        PlayState::Playing if state.free_camera() => (t.accent, "Playing  ·  Free camera"),
        PlayState::Playing if state.game_focus() => {
            (t.accent, "Playing  ·  Esc gives input back to the editor")
        }
        PlayState::Playing => (t.accent, "Playing  ·  Click to give the game input"),
    })
}

/// The Viewport tab bar's right side (laid out right to left): the render
/// size and, during Play, the editor's status that would otherwise cover
/// the game's UI: which camera the view shows, a dot in the Play colour
/// with the input hint, and in the game camera's view the selection.
pub(crate) fn header_ui(ui: &mut Ui, state: &EditorState) {
    let t = tokens(ui.ctx());
    let [w, h] = state.viewport.target_px;
    ui.label(
        egui::RichText::new(format!("{w} × {h}"))
            .font(t.mono())
            .color(t.text_faint),
    );
    let Some((color, hint)) = play_status(state, state.play_state(), &t) else {
        return;
    };
    let game_view = state.shows_game_camera();
    // The selection is also in the hierarchy and the inspector: the first
    // to go when the pane is narrow.
    if game_view && ui.available_width() > 520.0 {
        ui.add_space(t.pad_x * 2.0);
        ui.label(egui::RichText::new(selection_text(state)).color(t.text_weak));
    }
    ui.add_space(t.pad_x * 2.0);
    ui.label(egui::RichText::new(hint).color(t.text_strong));
    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(dot.center(), 3.5, color);
    if game_view {
        ui.add_space(t.pad_x);
        ui.label(egui::RichText::new("Game Camera").color(t.text));
    }
}

/// [`overlay_pill`] that is also an accessible label (screen readers and
/// tests read the viewport's status from egui's accessibility tree). It
/// senses nothing, so the viewport under it keeps its pointer input.
fn overlay_label(
    ui: &Ui,
    p: &egui::Painter,
    pos: egui::Pos2,
    align: Align2,
    text: String,
    t: &Tokens,
) -> Rect {
    let r = overlay_pill(p, pos, align, text.clone(), t);
    let resp = ui.interact(
        r,
        ui.scope_id()
            .with(("overlay_label", pos.x as i32, pos.y as i32)),
        Sense::hover(),
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    r
}

/// Gizmo drags, marquee selection and picking with the primary button
/// (without Alt). Shift adds to the selection, Cmd (Ctrl) toggles.
fn interact(ui: &Ui, state: &mut EditorState, resp: &egui::Response, rect: Rect) {
    let (pressed, released, down, modifiers, mode, pos, origin) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.primary_down(),
            i.modifiers,
            SelectMode::from_modifiers(&i.modifiers),
            i.pointer.interact_pos().or(i.pointer.hover_pos()),
            i.pointer.press_origin(),
        )
    });
    let alt = modifiers.alt;
    let ray_at = |state: &EditorState, c: Vec2| {
        let ndc = Vec2::new(
            c.x / rect.width() * 2.0 - 1.0,
            1.0 - c.y / rect.height() * 2.0,
        );
        crate::picking::camera_ray(&state.camera, ndc, rect.width() / rect.height().max(1.0))
    };
    // Holding Cmd (Ctrl elsewhere) during a gizmo drag inverts snapping.
    state.gizmo.invert_snap = ui.input(|i| i.modifiers.command);
    let to_local = |p: egui::Pos2| Vec2::new(p.x - rect.left(), p.y - rect.top());
    let local = pos.map(to_local);
    if state.gizmo.await_release {
        // A drag was ended from the keyboard; its press is spent.
        state.gizmo.await_release = down;
        state.gizmo.hover = None;
        return;
    }
    if state.gizmo.drag.is_some() {
        if let Some(c) = local
            && down
        {
            state.gizmo_update(c);
        }
        if released || !down {
            state.gizmo_end();
        }
        return;
    }
    if state.viewport.marquee.is_some() {
        if let Some(c) = local
            && down
        {
            state.marquee_update(c);
        }
        if released || !down {
            state.marquee_end();
        }
        return;
    }
    // The axis gizmo: a click turns the view; no picking or marquee there.
    let axis_c = axis_centre(rect);
    let in_axis = |p: egui::Pos2| p.distance(axis_c) <= AXIS_DISC;
    if pos.is_some_and(in_axis) || (down && origin.is_some_and(in_axis)) {
        state.gizmo.hover = None;
        if resp.clicked_by(PointerButton::Primary)
            && let Some(side) = pos.and_then(|p| axis_at(state, axis_c, p))
        {
            state.camera.look_from(side);
        }
        return;
    }
    // Cmd + Alt + click (Ctrl + Alt elsewhere): inspect the generated part
    // under the cursor, read-only. On anything else it selects as a click.
    if resp.clicked_by(PointerButton::Primary)
        && alt
        && modifiers.command
        && let Some(c) = local
    {
        let ray = ray_at(state, c);
        let hit = crate::picking::pick_where(&state.world, &ray, |e| state.blocked_in_viewport(e));
        match hit {
            Some(p) => match p.part.and_then(|e| state.world.uid(e)) {
                Some(part) => {
                    state.inspect_part(part);
                }
                None => state.select(state.world.uid(p.entity)),
            },
            None => state.select(None),
        }
        return;
    }
    // An active viewport tool takes the left button (`tools.rs`).
    if state.tool_wants_input() {
        state.gizmo.hover = None;
        let Some(c) = local else {
            return;
        };
        let ray = ray_at(state, c);
        let event = |kind| crate::tools::ToolEvent {
            kind,
            cursor: c,
            ray,
            modifiers,
            primary_down: down,
        };
        use crate::tools::ToolEventKind as K;
        if resp.hovered() && state.tool_hover_changed(c, modifiers) {
            state.tool_pointer(&event(K::Hover));
        }
        if pressed && resp.hovered() && !alt {
            state.tool_pointer(&event(K::Press));
        }
        if resp.clicked_by(PointerButton::Primary) && !alt {
            state.tool_pointer(&event(K::Click));
        } else if resp.drag_stopped_by(PointerButton::Primary) && !alt {
            state.tool_pointer(&event(K::Release));
        }
        return;
    }
    state.gizmo_hover(local.filter(|_| resp.hovered() && !alt));
    if pressed
        && resp.hovered()
        && !alt
        && let Some(c) = local
        && state.gizmo_begin(c)
    {
        return;
    }
    // A left drag that did not grab a handle draws a marquee from where the
    // button went down.
    if resp.drag_started_by(PointerButton::Primary)
        && !alt
        && let (Some(start), Some(c)) = (origin.map(to_local), local)
    {
        state.marquee_begin(start, mode);
        state.marquee_update(c);
        return;
    }
    if resp.clicked_by(PointerButton::Primary)
        && !alt
        && let Some(c) = local
    {
        let ray = ray_at(state, c);
        // Icons are drawn over the scene, so they win over meshes behind.
        // Locked and hidden entities are not picked: the ray passes through.
        let icon = state
            .viewport
            .view
            .icons
            .then(|| crate::icons::pick(&state.viewport.overlay.icons, c))
            .flatten()
            .filter(|u| {
                state
                    .world
                    .entity_by_uid(*u)
                    .is_none_or(|e| !state.blocked_in_viewport(e))
            });
        let hit = icon.or_else(|| {
            crate::picking::pick_where(&state.world, &ray, |e| state.blocked_in_viewport(e))
                .and_then(|p| state.world.uid(p.entity))
        });
        match (hit, mode) {
            (Some(uid), SelectMode::Replace) => state.select(Some(uid)),
            // Shift-click on a selected entity makes it the primary.
            (Some(uid), SelectMode::Add) => {
                state.selection.retain(|u| *u != uid);
                state.select_with(&[uid], SelectMode::Add);
            }
            (Some(uid), mode) => state.select_with(&[uid], mode),
            (None, SelectMode::Replace) => state.select(None),
            (None, _) => {}
        }
    }
}

/// An asset dragged from the browser over the viewport: a model, mesh or
/// scene is placed where it is released (on the surface under the pointer,
/// else on the ground plane); a material is assigned to the entity under
/// the pointer. Each is one undo step.
fn asset_drop(ui: &Ui, p: &egui::Painter, rect: Rect, state: &mut EditorState, t: &Tokens) {
    use crate::assets::RefType;
    let Some(drag) = state.assets.drag.clone() else {
        return;
    };
    let (pos, released) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
    let Some(pos) = pos.filter(|p| rect.contains(*p)) else {
        return;
    };
    let ndc = Vec2::new(
        (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
        1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
    );
    let aspect = rect.width() / rect.height().max(1.0);
    let hint = match drag.ty {
        RefType::Material => {
            let ray = crate::picking::camera_ray(&state.camera, ndc, aspect);
            let target = crate::picking::pick(&state.world, &ray)
                .and_then(|h| state.world.uid(h.entity).map(|u| (u, h.entity)));
            match target {
                Some((uid, e)) => {
                    if released {
                        state.assign_material(uid, &drag.item);
                    }
                    format!("Assign {} to {}", drag.name, state.display_name(e))
                }
                None => format!("Drop {} on a mesh", drag.name),
            }
        }
        RefType::Scene | RefType::Mesh => {
            if released {
                let at = state.viewport_drop_point(ndc, aspect);
                state.drop_asset_at(&drag.item, at);
            }
            format!("Place {}", drag.name)
        }
        _ => format!("{} cannot be placed in the scene", drag.name),
    };
    p.rect_stroke(rect, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Inside);
    overlay_pill(
        p,
        rect.center_top() + vec2(0.0, 12.0),
        Align2::CENTER_TOP,
        hint,
        t,
    );
}

/// The marquee rectangle: an accent tint with an accent border.
fn marquee_overlay(p: &egui::Painter, rect: Rect, state: &EditorState, t: &Tokens) {
    let Some(m) = &state.viewport.marquee else {
        return;
    };
    let (a, b) = m.rect();
    let r = Rect::from_min_max(rect.min + vec2(a.x, a.y), rect.min + vec2(b.x, b.y));
    p.rect_filled(r, 0.0, t.accent.gamma_multiply(0.14));
    p.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
}

fn navigate(ui: &Ui, state: &mut EditorState, resp: &egui::Response, rect: Rect) {
    let pointer = ui.input(|i| {
        let mut p = ViewportPointer {
            dragging: resp.is_pointer_button_down_on(),
            primary: i.pointer.button_down(PointerButton::Primary),
            middle: i.pointer.button_down(PointerButton::Middle),
            secondary: i.pointer.button_down(PointerButton::Secondary),
            delta: Vec2::new(i.pointer.delta().x, i.pointer.delta().y),
            hovered: resp.hovered(),
            modifiers: box_core::input::Modifiers {
                shift: i.modifiers.shift,
                ctrl: i.modifiers.ctrl,
                alt: i.modifiers.alt,
                meta: i.modifiers.mac_cmd,
            },
            keys: FlyKeys {
                forward: i.key_down(Key::W),
                back: i.key_down(Key::S),
                left: i.key_down(Key::A),
                right: i.key_down(Key::D),
                up: i.key_down(Key::E),
                down: i.key_down(Key::Q),
                fast: i.modifiers.shift,
            },
            dt: i.unstable_dt.clamp(0.0, 0.1),
            ..Default::default()
        };
        p.cursor_ndc = i
            .pointer
            .hover_pos()
            .filter(|pos| rect.contains(*pos))
            .map(|pos| {
                Vec2::new(
                    (pos.x - rect.left()) / rect.width() * 2.0 - 1.0,
                    1.0 - (pos.y - rect.top()) / rect.height() * 2.0,
                )
            });
        for e in &i.events {
            match e {
                Event::MouseWheel { unit, delta, .. } => match unit {
                    MouseWheelUnit::Point => p.scroll_points += Vec2::new(delta.x, delta.y),
                    MouseWheelUnit::Line => p.scroll_lines += Vec2::new(delta.x, delta.y),
                    // A page is roughly ten lines.
                    MouseWheelUnit::Page => p.scroll_lines += Vec2::new(delta.x, delta.y) * 10.0,
                },
                Event::Zoom(factor) if *factor > 0.0 => p.pinch += factor.ln(),
                _ => {}
            }
        }
        p
    });
    let aspect = rect.width() / rect.height().max(1.0);
    let applied = state.camera.apply(&pointer, aspect);
    if applied.fly_speed_changed {
        state.viewport.overlay.fly_pill_until = ui.input(|i| i.time) + FLY_PILL_SECONDS;
    }
}

/// How long the fly speed stays on screen after the wheel changed it.
const FLY_PILL_SECONDS: f64 = 1.5;

/// The icons for this frame (positions follow the camera), and the one
/// under the pointer. Cleared for the game camera's view or with icons off.
fn update_icons(ui: &Ui, state: &mut EditorState, rect: Rect, game_view: bool) {
    let mut icons = std::mem::take(&mut state.viewport.overlay.icons);
    icons.clear();
    if !game_view
        && state.viewport.view.icons
        && let Some(view) = state.gizmo_view()
    {
        // Entities added or moved since the last render are placed now
        // (change-driven: a compare per entity when nothing moved).
        box_core::transform::propagate_transforms(&mut state.world);
        crate::icons::collect(&state.world, &view, &mut icons);
    }
    let busy = state.gizmo.drag.is_some()
        || state.viewport.marquee.is_some()
        || state.camera.flying
        || state.gizmo.hover.is_some();
    state.viewport.overlay.hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p) && !busy)
        .and_then(|p| crate::icons::pick(&icons, Vec2::new(p.x - rect.left(), p.y - rect.top())));
    state.viewport.overlay.icons = icons;
}

/// The mode pill, which opens the View menu (not drawn in the game
/// camera's view; [`header_ui`] names that view).
fn view_menu(ui: &mut Ui, p: &egui::Painter, rect: Rect, state: &mut EditorState, t: &Tokens) {
    let at = rect.left_top() + vec2(10.0, 10.0);
    let mut mode = String::from("Perspective  ·  Lit");
    if state.show_grid {
        mode.push_str("  ·  Grid");
    }
    if state.viewport.view.icons {
        mode.push_str("  ·  Icons");
    }
    mode.push_str("     ");
    let pill = overlay_pill(p, at, Align2::LEFT_TOP, mode.clone(), t);
    crate::widgets::chevron(
        p,
        pill.right_center() - vec2(11.0, 0.0),
        true,
        t.overlay_text,
    );
    let resp = ui
        .interact(pill, ui.scope_id().with("view_menu"), Sense::click())
        .on_hover_text("View options");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, mode.trim_end()));
    if resp.hovered() {
        p.rect_stroke(
            pill,
            egui::CornerRadius::same(4),
            Stroke::new(1.0, t.overlay_text.gamma_multiply(0.5)),
            StrokeKind::Inside,
        );
    }
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| view_menu_ui(ui, state));
}

/// Views along the world axes: label and the side the eye looks from.
const AXIS_VIEWS: [(&str, glam::Vec3); 6] = [
    ("Top  (+Y)", glam::Vec3::Y),
    ("Bottom  (−Y)", glam::Vec3::NEG_Y),
    ("Front  (+Z)", glam::Vec3::Z),
    ("Back  (−Z)", glam::Vec3::NEG_Z),
    ("Right  (+X)", glam::Vec3::X),
    ("Left  (−X)", glam::Vec3::NEG_X),
];

fn view_menu_ui(ui: &mut Ui, state: &mut EditorState) {
    ui.set_min_width(190.0);
    crate::widgets::checkbox_labeled(ui, &mut state.show_grid, "Grid");
    crate::widgets::checkbox_labeled(ui, &mut state.viewport.view.icons, "Icons")
        .on_hover_text("Lights, cameras and empties: click an icon to select it");
    ui.add_space(2.0);
    ui.weak("Collider wireframes");
    ui.horizontal(|ui| {
        for d in ColliderDisplay::ALL {
            ui.radio_value(&mut state.viewport.view.colliders, d, d.label());
        }
    });
    crate::widgets::checkbox_labeled(ui, &mut state.viewport.view.contacts, "Contact points")
        .on_hover_text("While playing: where rigid bodies touch, with their normals");
    ui.add_space(2.0);
    ui.weak("Particle preview")
        .on_hover_text("Simulate particle emitters while editing (never saved)");
    ui.horizontal(|ui| {
        for m in crate::particle_preview::ParticlePreview::ALL {
            ui.radio_value(&mut state.viewport.view.particles, m, m.label());
        }
    });
    ui.separator();
    ui.weak("View from");
    for (label, side) in AXIS_VIEWS {
        if ui.button(label).clicked() {
            state.camera.turn_to(side);
            ui.close();
        }
    }
}

/// While a gizmo drag is held: its change and the snapping state, next to
/// the cursor.
fn gizmo_readout(ui: &Ui, p: &egui::Painter, rect: Rect, state: &EditorState, t: &Tokens) {
    let Some(text) = state.gizmo_readout() else {
        return;
    };
    let Some(pos) = ui.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    let at = (pos + vec2(18.0, 18.0)).min(rect.right_bottom() - vec2(220.0, 30.0));
    overlay_pill(p, at.max(rect.min), Align2::LEFT_TOP, text, t);
}

/// The fly speed, for a moment after the wheel changed it while flying.
fn fly_speed_pill(ui: &Ui, p: &egui::Painter, rect: Rect, state: &EditorState, t: &Tokens) {
    let now = ui.input(|i| i.time);
    let until = state.viewport.overlay.fly_pill_until;
    if now >= until {
        return;
    }
    overlay_pill(
        p,
        rect.center_top() + vec2(0.0, 12.0),
        Align2::CENTER_TOP,
        format!("Fly speed  {:.1} m/s", state.camera.fly_speed),
        t,
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(until - now));
}

/// Hints over the viewport: how to add objects to an empty scene, and the
/// navigation hint (until dismissed with a click; see `prefs.rs`).
fn hints(ui: &Ui, p: &egui::Painter, rect: Rect, state: &mut EditorState, t: &Tokens) {
    if let Some(hint) = state.tool_hint()
        && state.assets.drag.is_none()
    {
        overlay_pill(
            p,
            // Below the mode pill row, so the two never overlap.
            rect.center_top() + vec2(0.0, 48.0),
            Align2::CENTER_TOP,
            hint,
            t,
        );
    }
    if state.scene_is_empty() {
        overlay_pill(
            p,
            rect.center(),
            Align2::CENTER_CENTER,
            EMPTY_SCENE_HINT.into(),
            t,
        );
    }
    if !state.nav_hint {
        return;
    }
    let text = format!("{}    ×", crate::camera::NAVIGATION_HINT);
    let pill = overlay_pill(
        p,
        rect.center_bottom() + vec2(0.0, -10.0),
        Align2::CENTER_BOTTOM,
        text,
        t,
    );
    let r = ui
        .interact(pill, ui.scope_id().with("nav_hint"), Sense::click())
        .on_hover_text("Click to hide this hint");
    if r.clicked() {
        state.dismiss_nav_hint();
    }
}

/// Shown in the viewport and the hierarchy while the scene has no entities.
pub(crate) const EMPTY_SCENE_HINT: &str =
    "Empty scene  —  + Add or right-click in the Hierarchy to create objects";

/// Radius of the axis gizmo's disc, in points.
const AXIS_DISC: f32 = 30.0;
/// Distance of an axis end from the centre, and its radius.
const AXIS_LEN: f32 = 19.0;
const AXIS_END: f32 = 6.5;

fn axis_centre(rect: Rect) -> egui::Pos2 {
    rect.right_top() + vec2(-40.0, 42.0)
}

/// The axis ends as seen from the camera: (side, position, positive end),
/// back to front (the end nearest the viewer last).
fn axis_ends(state: &EditorState, c: egui::Pos2) -> Vec<(glam::Vec3, egui::Pos2, bool)> {
    let view = state.camera.orbit.transform();
    let (right, up, fwd) = (view.right(), view.up(), view.forward());
    let mut ends: Vec<(glam::Vec3, egui::Pos2, bool)> =
        [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z]
            .into_iter()
            .flat_map(|v| [(v, true), (-v, false)])
            .map(|(v, positive)| (v, c + vec2(v.dot(right), -v.dot(up)) * AXIS_LEN, positive))
            .collect();
    ends.sort_by(|a, b| b.0.dot(fwd).total_cmp(&a.0.dot(fwd)));
    ends
}

/// The side whose axis end is under `pos` (the frontmost one).
fn axis_at(state: &EditorState, c: egui::Pos2, pos: egui::Pos2) -> Option<glam::Vec3> {
    axis_ends(state, c)
        .into_iter()
        .rev()
        .find(|(_, e, _)| e.distance(pos) <= AXIS_END + 1.5)
        .map(|(v, ..)| v)
}

/// World axes as seen from the camera, top-right. Clicking an end turns the
/// view to look from that side (again: from the opposite side).
fn axis_gizmo(ui: &Ui, p: &egui::Painter, rect: Rect, state: &EditorState, t: &Tokens) {
    let c = axis_centre(rect);
    let hover = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|pos| pos.distance(c) <= AXIS_DISC);
    p.circle_filled(c, AXIS_DISC, t.overlay_bg);
    let hovered = hover.and_then(|pos| axis_at(state, c, pos));
    for (v, e, positive) in axis_ends(state, c) {
        let col = if v.x.abs() > 0.5 {
            t.axis_x
        } else if v.y.abs() > 0.5 {
            t.axis_y
        } else {
            t.axis_z
        };
        let lit = hovered == Some(v);
        if positive {
            p.line_segment([c, e], Stroke::new(1.6, col));
            p.circle_filled(e, AXIS_END, col);
            if lit {
                p.circle_stroke(e, AXIS_END + 1.0, Stroke::new(1.5, t.text_strong));
            }
            let label = if v.x > 0.5 {
                "X"
            } else if v.y > 0.5 {
                "Y"
            } else {
                "Z"
            };
            p.text(e, Align2::CENTER_CENTER, label, t.strong_small(), t.bg_app);
        } else {
            let fill = if lit { col } else { col.gamma_multiply(0.35) };
            p.circle_filled(e, AXIS_END - 1.5, fill);
            p.circle_stroke(e, AXIS_END - 1.5, Stroke::new(1.2, col));
        }
    }
}
