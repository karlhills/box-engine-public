//! The editor's own widget kit (from spike S1): property rows with a fixed
//! label column, axis-ticked vector fields, fill sliders, checkboxes, section
//! headers, segmented controls, toolbar buttons, filter chips and painted
//! monochrome glyphs. The reflection-driven inspector targets these.

use std::ops::RangeInclusive;

use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::theme::tokens;

/// A row with a fixed label column and a control area filling the rest. The
/// label is indented by `indent` points (nested fields) and can carry a
/// tooltip (doc comments).
pub fn field_row<R>(
    ui: &mut Ui,
    label: &str,
    indent: f32,
    tooltip: Option<&str>,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    row_impl(ui, label, indent, tooltip, RowMark::default(), false, add).0
}

/// How [`field_row_ex`] marks a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowMark {
    /// The value differs from its default: a dot before the label, the
    /// label in the text colour, and a revert button (↺) while hovered.
    pub modified: bool,
    /// Marked "Keep after Stop" during Play: a small accent bar at the left.
    pub kept: bool,
    /// A foldable heading (a nested struct): a chevron before the label,
    /// open (`Some(true)`) or closed.
    pub fold: Option<bool>,
}

/// What [`field_row_ex`] reports besides the control's result.
pub struct RowResponse {
    /// The label area (for a context menu and the tooltip).
    pub label: Response,
    /// The revert button (↺) of a modified row was clicked.
    pub revert: bool,
}

/// [`field_row`] with a [`RowMark`], returning the label's response (the
/// label area senses clicks, so callers attach a context menu to it).
pub fn field_row_ex<R>(
    ui: &mut Ui,
    label: &str,
    indent: f32,
    tooltip: Option<&str>,
    mark: RowMark,
    add: impl FnOnce(&mut Ui) -> R,
) -> (R, RowResponse) {
    row_impl(ui, label, indent, tooltip, mark, true, add)
}

/// A row; with `menu` the label column senses clicks and is in the
/// accessibility tree (for context menus and tests).
fn row_impl<R>(
    ui: &mut Ui,
    label: &str,
    indent: f32,
    tooltip: Option<&str>,
    mark: RowMark,
    menu: bool,
    add: impl FnOnce(&mut Ui) -> R,
) -> (R, RowResponse) {
    let t = tokens(ui.ctx());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, t.row_h), Sense::hover());
    let label_w = (w * 0.40).clamp(t.label_w, 140.0).round();
    let chevron_x = rect.left() + t.pad_x + 4.0 + indent + 3.0;
    let left = rect.left() + t.pad_x + 4.0 + indent + if mark.fold.is_some() { 12.0 } else { 0.0 };
    let hovered = ui.rect_contains_pointer(rect);
    let color = if mark.modified { t.text } else { t.text_weak };
    let mut job = egui::text::LayoutJob::single_section(
        label.to_string(),
        egui::TextFormat {
            font_id: t.body(),
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.left() + label_w - left - 6.0).max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = ui.painter().layout_job(job);
    let elided = g.elided;
    let label_rect = Rect::from_min_size(pos2(left, rect.center().y - g.size().y / 2.0), g.size());
    ui.painter().galley(label_rect.min, g, color);
    if let Some(open) = mark.fold {
        let c = if hovered { t.text } else { t.text_weak };
        chevron(ui.painter(), pos2(chevron_x, rect.center().y), open, c);
    }
    if mark.modified {
        let x = match mark.fold {
            Some(_) => chevron_x - 9.0,
            None => left - 5.0,
        };
        ui.painter()
            .circle_filled(pos2(x, rect.center().y), 2.0, t.accent);
    }
    if mark.kept {
        let bar = Rect::from_min_size(
            pos2(rect.left() + 2.0, rect.top() + 4.0),
            vec2(2.0, rect.height() - 8.0),
        );
        ui.painter().rect_filled(bar, 1.0, t.accent);
    }
    // A truncated label shows in full in the tooltip, above the doc comment.
    let tip = match (tooltip, elided) {
        (Some(tip), true) => Some(format!("{label}\n\n{tip}")),
        (Some(tip), false) => Some(tip.to_owned()),
        (None, true) => Some(label.to_owned()),
        (None, false) => None,
    };
    // With a menu, the whole label column: right-click opens it.
    let id = ui.scope_id().with(("label", label, indent as i32));
    let mut label_resp = if menu {
        let label_area = Rect::from_min_max(
            pos2(rect.left(), rect.top()),
            pos2(rect.left() + label_w, rect.bottom()),
        );
        let r = ui.interact(label_area, id, Sense::click());
        let text = label.to_owned();
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
        r
    } else {
        ui.interact(
            label_rect,
            ui.scope_id().with(("tip", label, indent as i32)),
            Sense::hover(),
        )
    };
    if let Some(tip) = tip {
        label_resp = label_resp.on_hover_text(tip);
    }
    let mut revert = false;
    // On hover, a revert button over the end of the label column.
    if mark.modified && hovered && ui.is_enabled() {
        let r = Rect::from_center_size(
            pos2(rect.left() + label_w - 13.0, rect.center().y),
            vec2(18.0, 18.0),
        );
        let resp = ui
            .interact(r, id.with("revert"), Sense::click())
            .on_hover_text("Reset to Default");
        let (fill, col) = if resp.hovered() {
            (t.field_hover, t.text_strong)
        } else {
            (t.field, t.text)
        };
        ui.painter()
            .rect_filled(r, CornerRadius::same(t.radius), fill);
        ui.painter()
            .text(r.center(), Align2::CENTER_CENTER, "↺", t.body(), col);
        revert = resp.clicked();
    }
    let ctrl = Rect::from_min_max(
        pos2(rect.left() + label_w, rect.top() + 1.0),
        pos2(rect.right() - t.pad_x, rect.bottom() - 1.0),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(ctrl)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    let r = add(&mut child);
    (
        r,
        RowResponse {
            label: label_resp,
            revert,
        },
    )
}

const AXIS_GAP: f32 = 4.0;

/// Parses typed text for a numeric field like egui does (whitespace ignored,
/// U+2212 as minus), but only finite numbers: `inf`, `nan` and friends,
/// which Rust's float parser accepts, are rejected so they never reach the
/// world. Use as `DragValue::custom_parser`.
pub fn parse_finite(text: &str) -> Option<f64> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '−' { '-' } else { c })
        .collect();
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// [`parse_finite`] for `f32` fields: also rejects numbers that overflow
/// `f32` to infinity (`1e39`). Returns the `f32` the field will hold: a
/// `DragValue` keeps the typed text only while its value equals the parsed
/// one, so an `f64` that the field then rounds to `f32` made it replace
/// the text with the shown (fewer) decimals and apply those on Enter.
pub fn parse_finite_f32(text: &str) -> Option<f64> {
    parse_finite(text)
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .map(f64::from)
}

fn axis_tick(ui: &Ui, rect: Rect, color: Color32, radius: u8) {
    let bar = Rect::from_min_size(rect.min, vec2(2.0, rect.height()));
    ui.painter().rect_filled(
        bar,
        CornerRadius {
            nw: radius,
            sw: radius,
            ne: 0,
            se: 0,
        },
        color,
    );
}

/// One drag field per component (2 to 4) with axis ticks on the first three.
/// Returns the union of the fields' responses, so `changed`, `drag_started`
/// and `drag_stopped` describe the group.
pub fn axes_edit(
    ui: &mut Ui,
    v: &mut [f32],
    speed: f32,
    decimals: usize,
    suffix: &str,
    range: Option<(f32, f32)>,
) -> Response {
    axes_edit_mixed(ui, v, speed, decimals, suffix, range, 0)
}

/// What a field shows when the selected entities disagree about its value.
pub const MIXED: &str = "—";

/// Non-zero magnitudes below this are shown in exponent form ("4.0e-7").
const EXPONENT_BELOW: f64 = 1e-6;

/// Decimals to show `value` with: at least `base` (from the field's step
/// and range), and for a non-zero magnitude below 1 enough for two
/// significant digits (0.004 needs 4), so a small non-zero value is never
/// shown as zero. Magnitudes below 1e-6 are shown in exponent form instead
/// (see [`number_text`]) and keep `base`.
pub fn decimals_for(value: f64, base: usize) -> usize {
    let a = value.abs();
    if a == 0.0 || !a.is_finite() || a >= 1.0 || a < EXPONENT_BELOW {
        return base;
    }
    base.max((-a.log10()).floor() as usize + 2)
}

/// `value` as text with [`decimals_for`] decimals; decimals beyond `base`
/// that are trailing zeros are dropped (0.004 with base 2 is "0.004", not
/// "0.0040"). Below 1e-6 the exponent form ("4.0e-7"), never "0.00".
pub fn number_text(value: f64, base: usize) -> String {
    let a = value.abs();
    if a != 0.0 && a.is_finite() && a < EXPONENT_BELOW {
        return format!("{value:.1e}");
    }
    let d = decimals_for(value, base);
    let mut s = format!("{value:.d$}");
    if d > base {
        let keep = s.len() - (d - base);
        while s.len() > keep && s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// A number field showing `value` (its current value) precisely: up to
/// [`decimals_for`] decimals and the exponent form for tiny magnitudes, so
/// a non-zero value never reads 0. Display only: typing keeps every digit,
/// and dragging rounds to the field's step as before.
pub fn precise_drag(dv: egui::DragValue<'_>, value: f64, base: usize) -> egui::DragValue<'_> {
    let dv = dv.max_decimals(decimals_for(value, base));
    let a = value.abs();
    if a != 0.0 && a.is_finite() && a < EXPONENT_BELOW {
        dv.custom_formatter(|v, _| number_text(v, 0))
    } else {
        dv
    }
}

/// [`axes_edit`] for a multi-selection: the lanes in `mixed` (X = bit 0)
/// show [`MIXED`] instead of a number. Dragging or typing still edits them.
pub fn axes_edit_mixed(
    ui: &mut Ui,
    v: &mut [f32],
    speed: f32,
    decimals: usize,
    suffix: &str,
    range: Option<(f32, f32)>,
    mixed: u8,
) -> Response {
    let t = tokens(ui.ctx());
    let n = v.len().max(1) as f32;
    let w = ((ui.available_width() - AXIS_GAP * (n - 1.0)) / n).floor();
    let h = ui.available_height();
    let axes = [t.axis_x, t.axis_y, t.axis_z];
    let mut union: Option<Response> = None;
    for (i, value) in v.iter_mut().enumerate() {
        let shown = *value as f64;
        let mut dv = egui::DragValue::new(value)
            .speed(speed)
            .min_decimals(decimals)
            .custom_parser(parse_finite_f32)
            .min_size(vec2(w, h));
        dv = precise_drag(dv, shown, decimals);
        if let Some((lo, hi)) = range {
            dv = dv.range(lo..=hi);
        }
        dv = match mixed & (1 << i) != 0 {
            true => dv.custom_formatter(|_, _| MIXED.to_owned()),
            false => dv.suffix(suffix),
        };
        let r = ui.add(dv);
        if let Some(axis) = axes.get(i) {
            axis_tick(ui, r.rect, *axis, t.radius);
        }
        union = Some(match union {
            Some(u) => u | r,
            None => r,
        });
    }
    union.unwrap_or_else(|| ui.allocate_response(vec2(0.0, 0.0), Sense::hover()))
}

/// A Blender-style slider: the field fills in proportion to the value.
pub fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    decimals: usize,
    suffix: &str,
) -> Response {
    slider_mixed(ui, value, range, decimals, suffix, false, false)
}

/// The position (0..1) of `v` on a slider over `lo..=hi`; `log` spreads
/// four decades below `hi` evenly (a range starting at or below zero starts
/// at `hi / 10^4`, with the left end itself `lo`).
pub fn slider_fraction(v: f32, lo: f32, hi: f32, log: bool) -> f32 {
    if !log || hi <= 0.0 {
        return ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    }
    let start = if lo > 0.0 { lo } else { hi * 1e-4 };
    if v <= start {
        return 0.0;
    }
    ((v / start).ln() / (hi / start).ln()).clamp(0.0, 1.0)
}

/// The inverse of [`slider_fraction`].
pub fn slider_value(f: f32, lo: f32, hi: f32, log: bool) -> f32 {
    if !log || hi <= 0.0 {
        return lo + (hi - lo) * f;
    }
    let start = if lo > 0.0 { lo } else { hi * 1e-4 };
    if f <= 0.0 {
        return lo;
    }
    start * (hi / start).powf(f)
}

/// [`slider`] that shows [`MIXED`] and no fill while `mixed`, on a
/// logarithmic scale with `log` (see [`slider_fraction`]).
pub fn slider_mixed(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    decimals: usize,
    suffix: &str,
    mixed: bool,
    log: bool,
) -> Response {
    let t = tokens(ui.ctx());
    let size = vec2(ui.available_width(), ui.available_height());
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let f = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let nv = slider_value(f, lo, hi, log);
        if nv != *value {
            *value = nv;
            resp.mark_changed();
        }
    }
    // A mixed value has no fill until it is dragged (then it is one value).
    let mixed = mixed && !resp.changed();
    let f = if mixed {
        0.0
    } else {
        slider_fraction(*value, lo, hi, log)
    };
    let bg = if resp.hovered() || resp.dragged() {
        t.field_hover
    } else {
        t.field
    };
    let r = CornerRadius::same(t.radius);
    let p = ui.painter();
    p.rect_filled(rect, r, bg);
    let fill_rect = Rect::from_min_max(
        rect.min,
        pos2(rect.left() + rect.width() * f, rect.bottom()),
    );
    let fill = if resp.dragged() {
        t.accent_bg
    } else {
        t.field_active
    };
    p.with_clip_rect(fill_rect).rect_filled(rect, r, fill);
    let text = if mixed {
        MIXED.to_owned()
    } else {
        format!("{}{suffix}", number_text(*value as f64, decimals))
    };
    // The shown text is the slider's accessible label (tests find it).
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::slider(enabled, *value as f64, &text));
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        t.mono(),
        if resp.hovered() {
            t.text_strong
        } else {
            t.text
        },
    );
    resp
}

pub fn checkbox(ui: &mut Ui, on: &mut bool) -> Response {
    checkbox_mixed(ui, on, false)
}

/// [`checkbox`] that shows a dash while `mixed`; a click then turns it on.
pub fn checkbox_mixed(ui: &mut Ui, on: &mut bool, mixed: bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    if resp.clicked() {
        *on = mixed || !*on;
        resp.mark_changed();
    }
    let checked = *on;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, checked, ""));
    paint_check(ui, rect, *on, mixed && !resp.changed(), resp.hovered());
    resp
}

/// A checkbox with a label after it; the label is part of the click
/// target. Use this, never `Ui::checkbox`: egui draws its box in the
/// field colour without a border, so on menus and windows (which have the
/// field colour) only a bare check mark shows.
pub fn checkbox_labeled(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let t = tokens(ui.ctx());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), t.body(), t.text);
    let gap = if label.is_empty() { 0.0 } else { 6.0 };
    let h = ui.spacing().interact_size.y.max(14.0);
    let size = vec2(14.0 + gap + galley.size().x, h);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let checked = *on;
    let text = label.to_owned();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, checked, &text));
    let check = Rect::from_min_size(pos2(rect.left(), rect.center().y - 7.0), vec2(14.0, 14.0));
    paint_check(ui, check, *on, false, resp.hovered());
    let color = if resp.hovered() {
        t.text_strong
    } else {
        t.text
    };
    ui.painter().galley(
        pos2(check.right() + gap, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    resp
}

/// The editor's check box: an accent square with a check when on, a dash
/// when `mixed`, and a field-coloured square with a visible border when
/// off (so it shows on panels, menus and windows alike). A disabled `Ui`
/// fades it like any widget.
fn paint_check(ui: &Ui, rect: Rect, on: bool, mixed: bool, hovered: bool) {
    let t = tokens(ui.ctx());
    let p = ui.painter();
    if mixed {
        p.rect_filled(rect, CornerRadius::same(3), t.field_active);
        p.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0, t.text_faint),
            StrokeKind::Inside,
        );
        p.line_segment(
            [
                rect.lerp_inside(vec2(0.25, 0.5)),
                rect.lerp_inside(vec2(0.75, 0.5)),
            ],
            Stroke::new(1.6, t.text_strong),
        );
    } else if on {
        p.rect_filled(rect, CornerRadius::same(3), t.accent);
        let pts = [
            rect.lerp_inside(vec2(0.24, 0.52)),
            rect.lerp_inside(vec2(0.43, 0.70)),
            rect.lerp_inside(vec2(0.76, 0.32)),
        ];
        p.add(Shape::line(pts.to_vec(), Stroke::new(1.6, t.accent_text)));
    } else {
        let fill = if hovered { t.field_hover } else { t.field };
        let edge = if hovered { t.text_weak } else { t.text_faint };
        p.rect_filled(rect, CornerRadius::same(3), fill);
        p.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0, edge),
            StrokeKind::Inside,
        );
    }
}

/// Collapsible section header with a hairline above it, an optional tooltip
/// on the title and an optional menu,
/// opened from a three-dot button at the right of the header or by right-clicking it.
pub fn section_with_menu(
    ui: &mut Ui,
    title: &str,
    tooltip: Option<&str>,
    open: &mut bool,
    menu: Option<impl FnOnce(&mut Ui)>,
    add: impl FnOnce(&mut Ui),
) {
    let t = tokens(ui.ctx());
    let w = ui.available_width();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    if let Some(tip) = tooltip {
        resp = resp.on_hover_text(tip);
    }
    if resp.clicked() {
        *open = !*open;
    }
    let p = ui.painter();
    p.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, t.border));
    if resp.hovered() {
        p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 0.0, t.hover);
    }
    chevron(
        p,
        pos2(rect.left() + t.pad_x + 4.0, rect.center().y),
        *open,
        t.text_weak,
    );
    p.text(
        pos2(rect.left() + t.pad_x + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        t.strong(),
        t.text_strong,
    );
    if let Some(menu) = menu {
        let button = Rect::from_center_size(
            pos2(rect.right() - t.pad_x - 8.0, rect.center().y),
            vec2(22.0, 20.0),
        );
        let mut bui = ui.new_child(UiBuilder::new().max_rect(button));
        let r = egui::containers::menu::MenuButton::from_button(
            egui::Button::new("").frame(false).min_size(button.size()),
        )
        .ui(&mut bui, menu)
        .0;
        // Three dots (a glyph Inter may lack).
        let col = if r.hovered() {
            t.text_strong
        } else {
            t.text_weak
        };
        for dx in [-5.0, 0.0, 5.0] {
            bui.painter()
                .circle_filled(button.center() + vec2(dx, 0.0), 1.4, col);
        }
    }
    if *open {
        ui.add_space(2.0);
        add(ui);
        ui.add_space(8.0);
    }
}

/// A row of mutually exclusive text buttons; the selected one gets the accent
/// tint. `tips` (one per label, or empty) are their tooltips. Returns true if
/// the selection changed.
pub fn segmented(
    ui: &mut Ui,
    selected: &mut usize,
    labels: &[&str],
    tips: &[String],
    height: f32,
) -> bool {
    let t = tokens(ui.ctx());
    let font = t.body();
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap((*l).into(), font.clone(), t.text)
                .size()
                .x
                + 18.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, height), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(t.radius + 1), t.field);
    let mut x = rect.left() + 2.0;
    let mut changed = false;
    for (i, (l, w)) in labels.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(*w, height - 4.0));
        let mut resp = ui.interact(r, ui.scope_id().with(("seg", l)), Sense::click());
        if let Some(tip) = tips.get(i) {
            resp = resp.on_hover_text(tip);
        }
        if resp.clicked() && *selected != i {
            *selected = i;
            changed = true;
        }
        let on = *selected == i;
        if on {
            ui.painter()
                .rect_filled(r, CornerRadius::same(t.radius), t.accent_bg);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(r, CornerRadius::same(t.radius), t.field_hover);
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            *l,
            font.clone(),
            if on {
                t.accent_text
            } else if resp.hovered() {
                t.text_strong
            } else {
                t.text
            },
        );
        x += w;
    }
    changed
}

/// A flat toolbar button with painted content. `on` gives it the active tint.
pub fn tool_button(
    ui: &mut Ui,
    tooltip: &str,
    size: Vec2,
    on: bool,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> Response {
    let t = tokens(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(tooltip);
    let p = ui.painter();
    if on {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.accent_bg);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.field_hover);
    }
    let fg = if on {
        t.accent_text
    } else if resp.hovered() {
        t.text_strong
    } else {
        t.text
    };
    paint(p, rect, fg);
    resp
}

/// A neutral text button for toolbars (accent only when `on`).
pub fn text_button(ui: &mut Ui, label: &str, h: f32, on: bool) -> Response {
    let t = tokens(ui.ctx());
    let g = ui.painter().layout_no_wrap(label.into(), t.body(), t.text);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, h), Sense::click());
    let p = ui.painter();
    let bg = if on {
        t.accent_bg
    } else if resp.hovered() {
        t.field_hover
    } else {
        t.field
    };
    p.rect_filled(rect, CornerRadius::same(t.radius + 1), bg);
    let col = if resp.hovered() {
        t.text_strong
    } else {
        t.text
    };
    p.galley(rect.center() - g.size() / 2.0, g, col);
    resp
}

/// A toggle chip with a coloured dot, a label and a count (console filters).
pub fn chip(ui: &mut Ui, label: &str, count: usize, dot: Color32, on: bool) -> Response {
    let t = tokens(ui.ctx());
    let g = ui.painter().layout_no_wrap(label.into(), t.body(), t.text);
    let cg = ui
        .painter()
        .layout_no_wrap(count.to_string(), t.mono(), t.text_weak);
    let size = vec2(g.size().x + cg.size().x + 32.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.field);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.hover);
    }
    let alpha = if on { 1.0 } else { 0.45 };
    p.circle_filled(
        pos2(rect.left() + 10.0, rect.center().y),
        3.0,
        dot.gamma_multiply(alpha),
    );
    let tc = if on { t.text_strong } else { t.text_weak };
    p.galley(
        pos2(rect.left() + 18.0, rect.center().y - g.size().y / 2.0),
        g,
        tc,
    );
    let gx = rect.right() - 8.0 - cg.size().x;
    p.galley(
        pos2(gx, rect.center().y - cg.size().y / 2.0),
        cg,
        if on { t.text_weak } else { t.text_faint },
    );
    resp
}

/// A vertical hairline separator for toolbars.
pub fn vsep(ui: &mut Ui, h: f32) {
    let t = tokens(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(1.0, h), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, t.border));
}

/// A rounded label with a translucent backing, for viewport overlays.
/// Returns the pill's rectangle.
pub fn overlay_pill(
    p: &egui::Painter,
    pos: Pos2,
    align: Align2,
    text: String,
    t: &crate::theme::Tokens,
) -> Rect {
    let g = p.layout_no_wrap(text, t.small(), t.overlay_text);
    let size = g.size() + vec2(14.0, 8.0);
    let r = align.anchor_size(pos, size);
    p.rect_filled(r, CornerRadius::same(4), t.overlay_bg);
    p.galley(r.min + vec2(7.0, 4.0), g, t.overlay_text);
    r
}

pub fn chevron(p: &egui::Painter, c: Pos2, open: bool, color: Color32) {
    let s = 3.0;
    let pts = if open {
        vec![
            c + vec2(-s, -s * 0.5),
            c + vec2(0.0, s * 0.5),
            c + vec2(s, -s * 0.5),
        ]
    } else {
        vec![
            c + vec2(-s * 0.5, -s),
            c + vec2(s * 0.5, 0.0),
            c + vec2(-s * 0.5, s),
        ]
    };
    p.add(Shape::line(pts, Stroke::new(1.3, color)));
}

pub fn search_glyph(p: &egui::Painter, c: Pos2, color: Color32) {
    p.circle_stroke(c + vec2(-1.0, -1.0), 3.8, Stroke::new(1.2, color));
    p.line_segment(
        [c + vec2(1.8, 1.8), c + vec2(4.5, 4.5)],
        Stroke::new(1.3, color),
    );
}

/// Snap: a small grid with a filled node.
pub fn snap_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    let s = 4.5;
    let st = Stroke::new(1.1, c);
    for k in [-1.0f32, 0.0, 1.0] {
        p.line_segment([m + vec2(-s, k * s), m + vec2(s, k * s)], st);
        p.line_segment([m + vec2(k * s, -s), m + vec2(k * s, s)], st);
    }
    p.circle_filled(m, 2.2, c);
}

/// Play: a right-pointing triangle.
pub fn play_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    let s = 5.0;
    p.add(Shape::convex_polygon(
        vec![
            m + vec2(-s * 0.7, -s),
            m + vec2(s, 0.0),
            m + vec2(-s * 0.7, s),
        ],
        c,
        Stroke::NONE,
    ));
}

/// Stop: a filled square.
pub fn stop_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    p.rect_filled(
        Rect::from_center_size(r.center(), vec2(9.0, 9.0)),
        CornerRadius::same(1),
        c,
    );
}

/// Pause: two bars.
pub fn pause_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    for dx in [-2.5f32, 2.5] {
        p.rect_filled(
            Rect::from_center_size(m + vec2(dx, 0.0), vec2(3.0, 10.0)),
            CornerRadius::same(1),
            c,
        );
    }
}

/// Step one frame: a small triangle against a bar.
pub fn step_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center() + vec2(-1.5, 0.0);
    let s = 4.5;
    p.add(Shape::convex_polygon(
        vec![
            m + vec2(-s * 0.7, -s),
            m + vec2(s * 0.8, 0.0),
            m + vec2(-s * 0.7, s),
        ],
        c,
        Stroke::NONE,
    ));
    p.rect_filled(
        Rect::from_center_size(m + vec2(s + 1.5, 0.0), vec2(2.0, 2.0 * s)),
        CornerRadius::ZERO,
        c,
    );
}

/// What a hierarchy row represents, for its glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    /// No renderable component, but children.
    Group,
    Mesh,
    /// A directional light.
    Light,
    PointLight,
    SpotLight,
    /// An `AmbientVolume`.
    AmbientVolume,
    /// A `ParticleEmitter`.
    Particles,
    Camera,
    /// No known component and no children.
    Empty,
}

impl EntityKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::Mesh => "mesh",
            Self::Light => "light",
            Self::PointLight => "point light",
            Self::SpotLight => "spot light",
            Self::AmbientVolume => "ambient volume",
            Self::Particles => "particle emitter",
            Self::Camera => "camera",
            Self::Empty => "entity",
        }
    }
}

/// Tiny monochrome type glyphs for hierarchy rows.
pub fn kind_glyph(p: &egui::Painter, c: Pos2, kind: EntityKind, color: Color32) {
    let st = Stroke::new(1.1, color);
    match kind {
        EntityKind::Group => {
            let r = Rect::from_center_size(c + vec2(0.0, 0.5), vec2(10.0, 7.5));
            p.rect_stroke(r, CornerRadius::same(1), st, StrokeKind::Middle);
            p.line_segment(
                [r.left_top(), r.left_top() + vec2(4.0, 0.0)],
                Stroke::new(2.4, color),
            );
        }
        EntityKind::Mesh => {
            // Isometric cube outline.
            let s = 4.6;
            let pts = vec![
                c + vec2(0.0, -s),
                c + vec2(s * 0.9, -s * 0.5),
                c + vec2(s * 0.9, s * 0.5),
                c + vec2(0.0, s),
                c + vec2(-s * 0.9, s * 0.5),
                c + vec2(-s * 0.9, -s * 0.5),
            ];
            p.add(Shape::closed_line(pts, st));
            p.line_segment([c + vec2(-s * 0.9, -s * 0.5), c], st);
            p.line_segment([c + vec2(s * 0.9, -s * 0.5), c], st);
            p.line_segment([c, c + vec2(0.0, s)], st);
        }
        EntityKind::Light => {
            p.circle_filled(c, 2.4, color);
            p.circle_stroke(c, 4.6, st);
        }
        EntityKind::PointLight => {
            // A bulb: a filled dot with four short rays.
            p.circle_filled(c, 2.2, color);
            for d in [
                vec2(1.0, 0.0),
                vec2(-1.0, 0.0),
                vec2(0.0, 1.0),
                vec2(0.0, -1.0),
            ] {
                p.line_segment([c + d * 3.6, c + d * 5.6], st);
            }
        }
        EntityKind::SpotLight => {
            // A lamp head and its cone (like the viewport icon).
            let head = vec![
                c + vec2(-2.0, -5.0),
                c + vec2(2.0, -5.0),
                c + vec2(2.8, -2.2),
                c + vec2(-2.8, -2.2),
            ];
            p.add(Shape::convex_polygon(head, color, Stroke::NONE));
            p.line_segment([c + vec2(-2.4, -1.0), c + vec2(-4.8, 5.0)], st);
            p.line_segment([c + vec2(2.4, -1.0), c + vec2(4.8, 5.0)], st);
        }
        EntityKind::AmbientVolume => {
            // A box with the light it holds.
            let r = Rect::from_center_size(c, vec2(10.0, 10.0));
            p.rect_stroke(r, CornerRadius::same(1), st, StrokeKind::Middle);
            p.circle_filled(c, 1.9, color);
        }
        EntityKind::Particles => {
            // Dots fanning up from an emitter point.
            let base = c + vec2(0.0, 4.5);
            p.circle_filled(base, 1.4, color);
            for (d, r) in [
                (vec2(-4.0, -4.5), 1.3),
                (vec2(0.0, -6.0), 1.5),
                (vec2(4.0, -4.5), 1.3),
                (vec2(-2.0, -1.0), 1.0),
                (vec2(2.0, -1.5), 1.0),
            ] {
                p.circle_filled(c + d, r, color);
            }
        }
        EntityKind::Camera => {
            let r = Rect::from_center_size(c + vec2(-1.5, 0.0), vec2(7.0, 6.0));
            p.rect_stroke(r, CornerRadius::same(1), st, StrokeKind::Middle);
            let pts = vec![
                pos2(r.right(), c.y),
                pos2(r.right() + 3.5, c.y - 3.0),
                pos2(r.right() + 3.5, c.y + 3.0),
            ];
            p.add(Shape::closed_line(pts, st));
        }
        EntityKind::Empty => {
            p.circle_stroke(c, 3.0, st);
        }
    }
}
