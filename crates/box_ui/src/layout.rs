//! Layout: a small flex-like model in reference points.
//!
//! - Every node has a width and height ([`Size`]): `auto` (fit content),
//!   points, a percentage of the parent's content box, or `fill` (share the
//!   parent's remaining main-axis space by `grow`; the full cross size).
//! - `column`, `panel` and `list` stack children vertically, `row`
//!   horizontally, with `gap` between them, `padding` inside, `justify` on
//!   the main axis and `align` (or a child's `align_self`) across it.
//!   `stretch` (the default) gives auto-sized children the full cross size.
//! - `stack` and every leaf lay children over each other inside the
//!   padding, placed by `align` (x) and `justify` (y).
//! - Roots sit on the screen at `anchor` (0..=1 per axis, also the pivot)
//!   plus `offset`, or follow a projected world position.
//! - Text wraps at the available width unless `wrap` is false.
//!
//! Layout runs only when something changed (a setter, focus, the window
//! size, the theme, strings, a world anchor that moved).

use box_core::assets::Assets;
use box_core::image::ImageData;
use epaint::{Pos2, Rect, Vec2, pos2, vec2};

use crate::node::NodeKind;
use crate::paint::Painter;
use crate::strings::{Strings, fill};
use crate::style::{Align, Justify, NodeState, Resolved, Size, Style};
use crate::ui::{NodeId, ScaleMode, Ui};

/// What layout reads from the world.
pub(crate) struct LayoutInputs<'a> {
    pub strings: Option<&'a Strings>,
    pub images: Option<&'a Assets<ImageData>>,
}

/// Pixels per point and the screen in points for a window.
pub(crate) fn scale_for(mode: ScaleMode, window_px: Vec2, scale_factor: f32) -> (f32, Vec2) {
    let s = match mode {
        ScaleMode::Reference { width, height } => {
            (window_px.x / width.max(1.0)).min(window_px.y / height.max(1.0))
        }
        ScaleMode::Dpi(f) => scale_factor.max(0.1) * f,
    }
    .max(0.05);
    (s, window_px / s)
}

impl Ui {
    /// Recomputes styles, text and rectangles of every node.
    pub(crate) fn layout(&mut self, inputs: &LayoutInputs) {
        profiling::scope!("ui layout");
        let mut painter = std::mem::take(&mut self.painter);
        painter.begin_pass(self.scale);
        let roots = self.roots.clone();
        for root in &roots {
            self.resolve(*root, true, inputs);
        }
        let screen = self.screen;
        for root in roots {
            let Some(n) = self.node(root) else { continue };
            if !n.shown {
                continue;
            }
            let (anchor, offset, world) =
                (n.anchor, n.offset, n.world.as_ref().and_then(|w| w.screen));
            let r = n.resolved.clone();
            let m = self.measure(root, [Some(screen.x), Some(screen.y)], &mut painter, inputs);
            let size = vec2(
                if r.width == Size::Fill { screen.x } else { m.x },
                if r.height == Size::Fill {
                    screen.y
                } else {
                    m.y
                },
            );
            let anchor_v = vec2(anchor[0], anchor[1]);
            let pos = match world {
                Some(p) => p - anchor_v * size + vec2(offset[0], offset[1]),
                None => (screen - size) * anchor_v + vec2(offset[0], offset[1]),
            };
            self.arrange(
                root,
                Rect::from_min_size(Pos2::ZERO + pos, size),
                Rect::EVERYTHING,
                &mut painter,
                inputs,
            );
        }
        self.painter = painter;
    }

    /// Visibility, resolved style and display text of `id`'s subtree.
    fn resolve(&mut self, id: NodeId, parent_shown: bool, inputs: &LayoutInputs) {
        let state = NodeState {
            hovered: self.hovered == Some(id),
            focused: self.focused == Some(id),
            pressed: self.pressed == Some(id),
            disabled: false,
            checked: false,
        };
        let Some(n) = self.node(id) else { return };
        let anchored_out = n.world.as_ref().is_some_and(|w| w.screen.is_none());
        let shown = parent_shown && !n.hidden && !anchored_out;
        let mut style = self
            .theme
            .styles
            .get(n.kind.class())
            .cloned()
            .unwrap_or_default();
        let mut missing = Vec::new();
        for c in &n.classes {
            match self.theme.styles.get(c) {
                Some(s) => style.overlay(s),
                None => missing.push(c.clone()),
            }
        }
        // A theme file still loading may define them: warn once it applied.
        let theme_pending = self.theme_path.is_some()
            && self
                .theme_loaded
                .is_none_or(|(_, version)| version == u64::MAX);
        for c in missing.into_iter().filter(|_| !theme_pending) {
            self.warn_once(format!("UI class `{c}` is not in the theme"));
        }
        let Some(n) = self.node(id) else { return };
        style.overlay(&n.style);
        let state = NodeState {
            disabled: n.disabled,
            checked: n.checked,
            ..state
        };
        let style: Style = style.with_state(state);
        let mut bad = Vec::new();
        let resolved = Resolved::from_style(&style, &self.theme, &mut bad);
        let template = match (&n.key, &n.text) {
            (Some(k), _) if !k.is_empty() => inputs.strings.map_or_else(|| k.clone(), |s| s.get(k)),
            (_, Some(t)) => t.clone(),
            _ => String::new(),
        };
        let args = &n.args;
        let text = fill(&template, |name| args.get(name).cloned());
        let children = n.children.clone();
        for b in bad {
            self.warn_once(format!(
                "UI colour `{b}` does not parse (#rrggbb, #rrggbbaa or $palette)"
            ));
        }
        if let Some(n) = self.node_mut(id) {
            n.shown = shown;
            n.resolved = resolved;
            n.display_text = text;
            if !shown {
                n.rect = Rect::NOTHING;
                n.galley = None;
            }
        }
        for c in children {
            self.resolve(c, shown, inputs);
        }
    }

    /// Preferred outer size of `id` given the available size (points).
    fn measure(
        &self,
        id: NodeId,
        avail: [Option<f32>; 2],
        painter: &mut Painter,
        inputs: &LayoutInputs,
    ) -> Vec2 {
        let Some(n) = self.node(id) else {
            return Vec2::ZERO;
        };
        let r = &n.resolved;
        let pad = vec2(r.padding.horizontal(), r.padding.vertical());
        let axis = |size: Size, avail: Option<f32>, pad: f32, max: f32| -> Option<f32> {
            match size {
                Size::Px(v) => Some(v - pad),
                Size::Percent(p) => avail.map(|a| a * p - pad),
                Size::Auto | Size::Fill => avail.map(|a| a.min(max) - pad),
            }
            .map(|v| v.max(0.0))
        };
        let inner_avail = [
            axis(r.width, avail[0], pad.x, r.max[0]),
            axis(r.height, avail[1], pad.y, r.max[1]),
        ];
        let content = self.content_size(id, inner_avail, painter, inputs);
        let outer = |size: Size, avail: Option<f32>, content: f32| match size {
            Size::Px(v) => v,
            Size::Percent(p) => avail.map_or(content, |a| a * p),
            Size::Auto | Size::Fill => content,
        };
        let mut w = outer(r.width, avail[0], content.x + pad.x);
        let mut h = outer(r.height, avail[1], content.y + pad.y);
        // Images keep their aspect ratio when one side is set.
        if n.kind == NodeKind::Image && content.x > 0.0 && content.y > 0.0 {
            match (r.width, r.height) {
                (Size::Px(_) | Size::Percent(_), Size::Auto) => {
                    h = (w - pad.x) * content.y / content.x + pad.y;
                }
                (Size::Auto, Size::Px(_) | Size::Percent(_)) => {
                    w = (h - pad.y) * content.x / content.y + pad.x;
                }
                _ => {}
            }
        }
        vec2(
            w.clamp(r.min[0], r.max[0].max(r.min[0])),
            h.clamp(r.min[1], r.max[1].max(r.min[1])),
        )
    }

    /// The content size (inside the padding).
    fn content_size(
        &self,
        id: NodeId,
        avail: [Option<f32>; 2],
        painter: &mut Painter,
        inputs: &LayoutInputs,
    ) -> Vec2 {
        let Some(n) = self.node(id) else {
            return Vec2::ZERO;
        };
        let r = &n.resolved;
        let text_size = |painter: &mut Painter, wrap: Option<f32>| -> Vec2 {
            if n.display_text.is_empty() {
                return Vec2::ZERO;
            }
            painter
                .layout_text(&n.display_text, r, wrap, &self.theme)
                .size()
        };
        let own = match n.kind {
            NodeKind::Text | NodeKind::Button => {
                text_size(painter, if r.wrap { avail[0] } else { None })
            }
            NodeKind::Slider | NodeKind::Toggle => {
                let t = text_size(painter, None);
                let control_h = control_height(n.kind, r);
                let gap = if t.x > 0.0 { r.gap } else { 0.0 };
                vec2(t.x + gap + r.control_width, t.y.max(control_h))
            }
            NodeKind::Progress => vec2(r.control_width, r.control_height),
            NodeKind::Image => n
                .image_handle
                .and_then(|h| inputs.images?.get(h))
                .map_or(Vec2::ZERO, |img| vec2(img.width as f32, img.height as f32)),
            _ => Vec2::ZERO,
        };
        let kids: Vec<NodeId> = n
            .children
            .iter()
            .copied()
            .filter(|c| self.node(*c).is_some_and(|c| c.shown))
            .collect();
        if kids.is_empty() {
            return own;
        }
        let gaps = r.gap * (kids.len() as f32 - 1.0);
        let mut acc = Vec2::ZERO;
        if n.kind.is_row() {
            for c in &kids {
                let m = self.measure(*c, [None, avail[1]], painter, inputs);
                acc.x += m.x;
                acc.y = acc.y.max(m.y);
            }
            acc.x += gaps;
        } else if n.kind.is_stack() {
            for c in &kids {
                let m = self.measure(*c, avail, painter, inputs);
                acc = acc.max(m);
            }
        } else {
            for c in &kids {
                let m = self.measure(*c, [avail[0], None], painter, inputs);
                acc.x = acc.x.max(m.x);
                acc.y += m.y;
            }
            acc.y += gaps;
        }
        acc.max(own)
    }

    /// Places `id` at `rect` and lays out its children.
    fn arrange(
        &mut self,
        id: NodeId,
        rect: Rect,
        clip: Rect,
        painter: &mut Painter,
        inputs: &LayoutInputs,
    ) {
        let theme = &self.theme;
        let Some(n) = self.node(id) else { return };
        let r = n.resolved.clone();
        let kind = n.kind;
        let inner = Rect::from_min_max(
            pos2(rect.min.x + r.padding.left, rect.min.y + r.padding.top),
            pos2(
                (rect.max.x - r.padding.right).max(rect.min.x + r.padding.left),
                (rect.max.y - r.padding.bottom).max(rect.min.y + r.padding.top),
            ),
        );
        // The final text layout, at the final width.
        let galley = (!n.display_text.is_empty()
            && matches!(
                kind,
                NodeKind::Text | NodeKind::Button | NodeKind::Slider | NodeKind::Toggle
            ))
        .then(|| {
            let wrap = (r.wrap && matches!(kind, NodeKind::Text | NodeKind::Button))
                .then_some(inner.width());
            painter.layout_text(&n.display_text, &r, wrap, theme)
        });
        let kids: Vec<NodeId> = n
            .children
            .iter()
            .copied()
            .filter(|c| self.node(*c).is_some_and(|c| c.shown))
            .collect();
        let scroll = n.scroll;
        let focused = self.focused;
        if let Some(n) = self.node_mut(id) {
            n.rect = rect;
            n.clip = clip;
            n.galley = galley;
        }
        if kids.is_empty() {
            return;
        }
        let child_clip = if kind == NodeKind::List {
            clip.intersect(inner)
        } else {
            clip
        };
        let avail = [Some(inner.width()), Some(inner.height())];
        let measured: Vec<(NodeId, Vec2, Resolved)> = kids
            .iter()
            .map(|c| {
                let m = self.measure(*c, avail, painter, inputs);
                let cr = self
                    .node(*c)
                    .map(|n| n.resolved.clone())
                    .unwrap_or_default();
                (*c, m, cr)
            })
            .collect();
        if kind.is_stack() {
            for (c, m, cr) in measured {
                let w = match cr.width {
                    Size::Fill => inner.width(),
                    Size::Auto if r.align == Align::Stretch => {
                        inner.width().max(m.x.min(inner.width()))
                    }
                    _ => m.x,
                };
                let h = if cr.height == Size::Fill {
                    inner.height()
                } else {
                    m.y
                };
                let align = cr.align_self.unwrap_or(r.align);
                let x = inner.min.x + align_offset(align, inner.width() - w);
                let y = inner.min.y + justify_offset(r.justify, inner.height() - h);
                self.arrange(
                    c,
                    Rect::from_min_size(pos2(x, y), vec2(w, h)),
                    child_clip,
                    painter,
                    inputs,
                );
            }
            return;
        }
        let row = kind.is_row();
        let (main_len, cross_len) = if row {
            (inner.width(), inner.height())
        } else {
            (inner.height(), inner.width())
        };
        let main_of = |v: Vec2| if row { v.x } else { v.y };
        let cross_of = |v: Vec2| if row { v.y } else { v.x };
        let main_size = |cr: &Resolved| if row { cr.width } else { cr.height };
        let cross_size = |cr: &Resolved| if row { cr.height } else { cr.width };
        let gaps = r.gap * (measured.len() as f32 - 1.0);
        let fixed: f32 = measured
            .iter()
            .filter(|(_, _, cr)| main_size(cr) != Size::Fill)
            .map(|(_, m, _)| main_of(*m))
            .sum();
        let grow_total: f32 = measured
            .iter()
            .filter(|(_, _, cr)| main_size(cr) == Size::Fill)
            .map(|(_, _, cr)| cr.grow)
            .sum();
        let free = (main_len - fixed - gaps).max(0.0);
        let has_fill = grow_total > 0.0;
        let mains: Vec<f32> = measured
            .iter()
            .map(|(_, m, cr)| {
                if main_size(cr) == Size::Fill {
                    if has_fill {
                        free * cr.grow / grow_total
                    } else {
                        0.0
                    }
                    .max(main_of(*m))
                } else {
                    main_of(*m)
                }
            })
            .collect();
        let used: f32 = mains.iter().sum::<f32>() + gaps;
        let leftover = (main_len - used).max(0.0);
        let (mut cursor, extra_gap) = if has_fill || kind == NodeKind::List {
            (0.0, 0.0)
        } else {
            match r.justify {
                Justify::Start => (0.0, 0.0),
                Justify::Center => (leftover / 2.0, 0.0),
                Justify::End => (leftover, 0.0),
                Justify::SpaceBetween if measured.len() > 1 => {
                    (0.0, leftover / (measured.len() as f32 - 1.0))
                }
                Justify::SpaceBetween => (0.0, 0.0),
            }
        };
        // Main-axis offsets before scrolling.
        let mut offsets = Vec::with_capacity(measured.len());
        for main in &mains {
            offsets.push(cursor);
            cursor += main + r.gap + extra_gap;
        }
        let mut scroll = scroll;
        if kind == NodeKind::List {
            let content = used;
            // Keep the item holding focus visible.
            if let Some(f) = focused
                && let Some(i) = kids.iter().position(|k| self.is_ancestor_or_self(*k, f))
            {
                let (start, end) = (offsets[i], offsets[i] + mains[i]);
                if start < scroll {
                    scroll = start;
                } else if end > scroll + main_len {
                    scroll = end - main_len;
                }
            }
            scroll = scroll.clamp(0.0, (content - main_len).max(0.0));
            if let Some(n) = self.node_mut(id) {
                n.scroll = scroll;
            }
        } else {
            scroll = 0.0;
        }
        for (i, (c, m, cr)) in measured.into_iter().enumerate() {
            let main = mains[i];
            let align = cr.align_self.unwrap_or(r.align);
            let cross = match cross_size(&cr) {
                Size::Fill => cross_len,
                Size::Auto if align == Align::Stretch => cross_len,
                _ => cross_of(m),
            };
            let cross_pos = align_offset(align, cross_len - cross);
            let main_pos = offsets[i] - scroll;
            let child = if row {
                Rect::from_min_size(
                    pos2(inner.min.x + main_pos, inner.min.y + cross_pos),
                    vec2(main, cross),
                )
            } else {
                Rect::from_min_size(
                    pos2(inner.min.x + cross_pos, inner.min.y + main_pos),
                    vec2(cross, main),
                )
            };
            self.arrange(c, child, child_clip, painter, inputs);
        }
    }

    /// True if `node` is `ancestor` or below it.
    pub(crate) fn is_ancestor_or_self(&self, ancestor: NodeId, mut node: NodeId) -> bool {
        loop {
            if node == ancestor {
                return true;
            }
            match self.parent(node) {
                Some(p) => node = p,
                None => return false,
            }
        }
    }
}

/// Height of a slider or toggle control.
pub(crate) fn control_height(kind: NodeKind, r: &Resolved) -> f32 {
    match kind {
        // The knob is taller than the track.
        NodeKind::Slider => r.control_height * 2.4,
        _ => r.control_height,
    }
}

fn align_offset(align: Align, free: f32) -> f32 {
    match align {
        Align::Start | Align::Stretch => 0.0,
        Align::Center => free / 2.0,
        Align::End => free,
    }
}

fn justify_offset(justify: Justify, free: f32) -> f32 {
    match justify {
        Justify::Start | Justify::SpaceBetween => 0.0,
        Justify::Center => free / 2.0,
        Justify::End => free,
    }
}
