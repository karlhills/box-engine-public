//! Our own small widget set: property rows, axis drag fields, slider fields, checkboxes,
//! section headers, segmented controls and painted glyphs.

use crate::theme::{self, tokens};
use egui::{
    Align, Align2, Color32, CornerRadius, Layout, Pos2, Rect, Response, Sense, Shape, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};
use std::ops::RangeInclusive;

/// A row with a fixed label column and a control area filling the rest.
pub fn property_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = tokens();
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, t.row_h), Sense::hover());
    let label_w = (w * 0.40).clamp(t.label_w, 140.0).round();
    let mut job = egui::text::LayoutJob::single_section(
        label.to_string(),
        egui::TextFormat {
            font_id: theme::body(),
            color: t.text_weak,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: label_w - t.pad_x - 4.0 - 6.0,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            rect.left() + t.pad_x + 4.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        t.text_weak,
    );
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
    add(&mut child)
}

/// Three drag fields with a subtle axis-coloured tick on each.
pub fn vec3(ui: &mut Ui, v: &mut [f32; 3], speed: f32, decimals: usize, suffix: &str) -> bool {
    let t = tokens();
    let gap = 4.0;
    let w = ((ui.available_width() - gap * 2.0) / 3.0).floor();
    let h = ui.available_height();
    let mut changed = false;
    let axes = [t.axis_x, t.axis_y, t.axis_z];
    for i in 0..3 {
        let r = ui.add(
            egui::DragValue::new(&mut v[i])
                .speed(speed)
                .fixed_decimals(decimals)
                .suffix(suffix)
                .min_size(vec2(w, h)),
        );
        let bar = Rect::from_min_size(r.rect.min + vec2(0.0, 0.0), vec2(2.0, r.rect.height()));
        ui.painter().rect_filled(
            bar,
            CornerRadius {
                nw: t.radius,
                sw: t.radius,
                ne: 0,
                se: 0,
            },
            axes[i],
        );
        changed |= r.changed();
    }
    changed
}

/// A Blender-style slider: the field itself fills in proportion to the value.
pub fn slider(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    decimals: usize,
    suffix: &str,
) -> Response {
    let t = tokens();
    let size = vec2(ui.available_width(), ui.available_height());
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let f = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let nv = lo + (hi - lo) * f;
        if nv != *value {
            *value = nv;
            resp.mark_changed();
        }
    }
    let f = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
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
        Color32::from_rgb(0x3a, 0x3b, 0x42)
    };
    p.with_clip_rect(fill_rect).rect_filled(rect, r, fill);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{:.*}{}", decimals, value, suffix),
        theme::mono(),
        if resp.hovered() {
            t.text_strong
        } else {
            t.text
        },
    );
    resp
}

pub fn checkbox(ui: &mut Ui, on: &mut bool) -> Response {
    let t = tokens();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let p = ui.painter();
    if *on {
        p.rect_filled(rect, CornerRadius::same(3), t.accent);
        let pts = [
            rect.lerp_inside(vec2(0.24, 0.52)),
            rect.lerp_inside(vec2(0.43, 0.70)),
            rect.lerp_inside(vec2(0.76, 0.32)),
        ];
        p.add(Shape::line(pts.to_vec(), Stroke::new(1.6, t.accent_text)));
    } else {
        let fill = if resp.hovered() {
            t.field_hover
        } else {
            t.field
        };
        p.rect_filled(rect, CornerRadius::same(3), fill);
        p.rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.0, Color32::from_rgb(0x44, 0x44, 0x4b)),
            StrokeKind::Inside,
        );
    }
    resp
}

/// Collapsible section header with a hairline above it.
pub fn section(ui: &mut Ui, title: &str, open: &mut bool, add: impl FnOnce(&mut Ui)) {
    let t = tokens();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
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
        theme::strong(),
        t.text_strong,
    );
    if *open {
        ui.add_space(2.0);
        add(ui);
        ui.add_space(8.0);
    }
}

/// A row of mutually exclusive text buttons; the selected one gets the accent tint.
pub fn segmented(ui: &mut Ui, selected: &mut usize, labels: &[&str], height: f32) -> bool {
    let t = tokens();
    let font = theme::body();
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
        let resp = ui.interact(r, ui.scope_id().with(("seg", l)), Sense::click());
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

/// A flat toolbar button. `on` gives it the active tint. Content is painted by `paint`.
pub fn tool_button(
    ui: &mut Ui,
    id: &str,
    size: Vec2,
    on: bool,
    enabled: bool,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> Response {
    let t = tokens();
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(id);
    let p = ui.painter();
    if on {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.accent_bg);
    } else if resp.hovered() && enabled {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.field_hover);
    }
    let fg = if !enabled {
        t.text_faint
    } else if on {
        t.accent_text
    } else if resp.hovered() {
        t.text_strong
    } else {
        t.text
    };
    paint(p, rect, fg);
    resp
}

/// A neutral text button for toolbars (no accent unless `on`).
pub fn text_button(ui: &mut Ui, label: &str, h: f32, on: bool) -> Response {
    let t = tokens();
    let g = ui
        .painter()
        .layout_no_wrap(label.into(), theme::body(), t.text);
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

pub fn play_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    let pts = vec![
        m + vec2(-3.5, -5.0),
        m + vec2(5.0, 0.0),
        m + vec2(-3.5, 5.0),
    ];
    p.add(Shape::convex_polygon(pts, c, Stroke::NONE));
}

pub fn pause_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    let m = r.center();
    for dx in [-2.75, 2.75] {
        p.rect_filled(
            Rect::from_center_size(m + vec2(dx, 0.0), vec2(2.5, 10.0)),
            0.5,
            c,
        );
    }
}

pub fn stop_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    p.rect_filled(Rect::from_center_size(r.center(), vec2(9.0, 9.0)), 1.0, c);
}

pub fn search_glyph(p: &egui::Painter, c: Pos2, color: Color32) {
    p.circle_stroke(c + vec2(-1.0, -1.0), 3.8, Stroke::new(1.2, color));
    p.line_segment(
        [c + vec2(1.8, 1.8), c + vec2(4.5, 4.5)],
        Stroke::new(1.3, color),
    );
}

pub fn magnet_glyph(p: &egui::Painter, r: Rect, c: Color32) {
    // Snap: a small grid with a filled corner node.
    let m = r.center();
    let s = 4.5;
    let st = Stroke::new(1.1, c);
    for k in [-1.0f32, 0.0, 1.0] {
        p.line_segment([m + vec2(-s, k * s), m + vec2(s, k * s)], st);
        p.line_segment([m + vec2(k * s, -s), m + vec2(k * s, s)], st);
    }
    p.circle_filled(m + vec2(0.0, 0.0), 2.2, c);
}

/// Tiny monochrome type glyphs for hierarchy rows.
pub fn kind_glyph(p: &egui::Painter, c: Pos2, kind: crate::scene::Kind, color: Color32) {
    use crate::scene::Kind::*;
    let st = Stroke::new(1.1, color);
    match kind {
        Group => {
            let r = Rect::from_center_size(c + vec2(0.0, 0.5), vec2(10.0, 7.5));
            p.rect_stroke(r, CornerRadius::same(1), st, StrokeKind::Middle);
            p.line_segment(
                [r.left_top(), r.left_top() + vec2(4.0, 0.0)],
                Stroke::new(2.4, color),
            );
        }
        Mesh => {
            // isometric cube outline
            let s = 4.6;
            let top = c + vec2(0.0, -s);
            let pts = vec![
                top,
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
        Light => {
            p.circle_filled(c, 2.4, color);
            p.circle_stroke(c, 4.6, st);
        }
        Camera => {
            let r = Rect::from_center_size(c + vec2(-1.5, 0.0), vec2(7.0, 6.0));
            p.rect_stroke(r, CornerRadius::same(1), st, StrokeKind::Middle);
            let pts = vec![
                pos2(r.right(), c.y),
                pos2(r.right() + 3.5, c.y - 3.0),
                pos2(r.right() + 3.5, c.y + 3.0),
            ];
            p.add(Shape::closed_line(pts, st));
        }
        Prefab => {
            let s = 4.6;
            let pts = vec![
                c + vec2(0.0, -s),
                c + vec2(s, 0.0),
                c + vec2(0.0, s),
                c + vec2(-s, 0.0),
            ];
            p.add(Shape::closed_line(pts, st));
            p.circle_filled(c, 1.5, color);
        }
    }
}

pub fn eye_off_glyph(p: &egui::Painter, c: Pos2, color: Color32) {
    let st = Stroke::new(1.1, color);
    p.circle_stroke(c, 3.6, st);
    p.line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], st);
}
