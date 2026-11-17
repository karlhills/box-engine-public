//! Painting: epaint (egui's painter: text shaping and layout, the glyph
//! atlas, anti-aliased shapes) turns the laid-out tree into triangles, which
//! go to the renderer as the `box_render::Overlay` resource. No egui type
//! leaves this module.

use std::collections::BTreeMap;
use std::sync::Arc;

use box_assets::FontAsset;
use box_render::{Overlay, OverlayBatch, OverlayTexture, OverlayTextureUpdate, OverlayVertex};
use epaint::text::{
    FontData, FontDefinitions, FontFamily, FontId, Fonts, LayoutJob, TextFormat, TextOptions,
};
use epaint::{
    ClippedPrimitive, ClippedShape, CornerRadius, Galley, Mesh, Pos2, Primitive, Rect, RectShape,
    Shape, Stroke, StrokeKind, TessellationOptions, Tessellator, TextureId, pos2, vec2,
};

use crate::layout::control_height;
use crate::markup;
use crate::node::NodeKind;
use crate::style::{Resolved, TextAlign, Theme};
use crate::ui::{NodeId, Ui};

/// The bundled default font: Inter (SIL Open Font License 1.1; the licence
/// is `crates/box_editor/assets/fonts/OFL-Inter.txt`, shipped with the
/// editor's copy of the same file).
pub const DEFAULT_FONT: &[u8] = include_bytes!("../../box_editor/assets/fonts/Inter-Variable.ttf");

/// Largest font atlas side.
const MAX_ATLAS: usize = 4096;

/// Fonts by role: the loaded font assets the current `Fonts` was built from.
type FontKey = BTreeMap<String, (String, usize)>;

/// Text layout and painting state, owned by [`Ui`].
pub(crate) struct Painter {
    fonts: Option<Fonts>,
    font_key: FontKey,
    /// Font assets by role (from the theme), as loaded.
    pub(crate) font_assets: BTreeMap<String, FontAsset>,
    roles: Vec<String>,
    atlas_id: u64,
    scale: f32,
}

impl Default for Painter {
    fn default() -> Self {
        Self {
            fonts: None,
            font_key: FontKey::new(),
            font_assets: BTreeMap::new(),
            roles: Vec::new(),
            // Assigned when the font set is built (`ensure_fonts`).
            atlas_id: 0,
            scale: 1.0,
        }
    }
}

impl Painter {
    /// Builds the font set when the theme's fonts changed. Expensive (parses
    /// every font): only on change.
    pub(crate) fn ensure_fonts(&mut self, theme: &Theme) {
        let key: FontKey = self
            .font_assets
            .iter()
            .map(|(role, f)| {
                (
                    role.clone(),
                    (f.family.clone(), Arc::as_ptr(&f.bytes) as usize),
                )
            })
            .collect();
        let mut roles: Vec<String> = theme.fonts.keys().cloned().collect();
        roles.push("body".into());
        roles.sort();
        roles.dedup();
        if self.fonts.is_some() && key == self.font_key && roles == self.roles {
            return;
        }
        profiling::scope!("ui fonts");
        let mut defs = FontDefinitions {
            font_data: BTreeMap::new(),
            families: BTreeMap::new(),
        };
        defs.font_data.insert(
            "default".into(),
            Arc::new(FontData::from_static(DEFAULT_FONT)),
        );
        for (role, f) in &self.font_assets {
            let bytes = f.bytes.clone();
            defs.font_data.insert(
                format!("role:{role}"),
                Arc::new(FontData::from_blob(bytes, 0)),
            );
        }
        let family_for = |role: &str| -> Vec<String> {
            let mut list = Vec::new();
            if self.font_assets.contains_key(role) {
                list.push(format!("role:{role}"));
            } else if self.font_assets.contains_key("body") {
                list.push("role:body".into());
            }
            list.push("default".into());
            list
        };
        defs.families
            .insert(FontFamily::Proportional, family_for("body"));
        defs.families
            .insert(FontFamily::Monospace, family_for("body"));
        for role in &roles {
            defs.families
                .insert(FontFamily::Name(role.as_str().into()), family_for(role));
        }
        self.fonts = Some(Fonts::new(text_options(), defs));
        self.font_key = key;
        self.roles = roles;
        // A new atlas: the renderer gets a full upload under a new id.
        self.atlas_id = Overlay::new_texture_id();
    }

    /// Starts a layout pass at `scale` pixels per point.
    pub(crate) fn begin_pass(&mut self, scale: f32) {
        self.scale = scale;
        if self.fonts.is_none() {
            self.ensure_fonts(&Theme::default());
        }
        if let Some(f) = &mut self.fonts {
            f.begin_pass(text_options(), Default::default());
        }
    }

    fn family(&self, role: &str) -> FontFamily {
        if self.roles.iter().any(|r| r == role) {
            FontFamily::Name(role.into())
        } else {
            FontFamily::Proportional
        }
    }

    /// Lays out `text` (with inline tags) in the node's text style.
    pub(crate) fn layout_text(
        &mut self,
        text: &str,
        r: &Resolved,
        wrap_width: Option<f32>,
        theme: &Theme,
    ) -> Arc<Galley> {
        let mut job = LayoutJob::default();
        for span in markup::parse(text) {
            let color = span
                .color
                .as_deref()
                .and_then(|c| theme.color(c))
                .unwrap_or(r.text_color);
            let family = self.family(span.font.as_deref().unwrap_or(&r.font));
            let mut format =
                TextFormat::simple(FontId::new(span.size.unwrap_or(r.text_size), family), color);
            format.italics = span.italic;
            if span.underline {
                format.underline = Stroke::new((r.text_size / 16.0).max(1.0), color);
            }
            job.append(&span.text, 0.0, format);
        }
        job.wrap.max_width = wrap_width.unwrap_or(f32::INFINITY).max(1.0);
        job.halign = match r.text_align {
            TextAlign::Start => epaint::emath::Align::LEFT,
            TextAlign::Center => epaint::emath::Align::Center,
            TextAlign::End => epaint::emath::Align::RIGHT,
        };
        let scale = self.scale;
        let fonts = self.fonts.as_mut().expect("begin_pass creates the fonts");
        fonts.with_pixels_per_point(scale).layout_job(job)
    }

    /// The font atlas change since the last call, as an overlay update.
    fn atlas_update(&mut self) -> Option<OverlayTextureUpdate> {
        let delta = self.fonts.as_mut()?.font_image_delta()?;
        let epaint::ImageData::Color(image) = &delta.image;
        let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
        for c in &image.pixels {
            rgba.extend_from_slice(&c.to_array());
        }
        Some(OverlayTextureUpdate {
            id: self.atlas_id,
            pos: delta.pos.map(|[x, y]| [x as u32, y as u32]),
            size: [image.size[0] as u32, image.size[1] as u32],
            rgba,
            linear: true,
        })
    }

    fn tessellate(&self, shapes: Vec<ClippedShape>) -> Vec<ClippedPrimitive> {
        profiling::scope!("ui tessellate");
        let Some(fonts) = &self.fonts else {
            return Vec::new();
        };
        let atlas = fonts.texture_atlas();
        let mut t = Tessellator::new(
            self.scale,
            TessellationOptions::default(),
            fonts.font_image_size(),
            atlas.prepared_discs(),
        );
        t.tessellate_shapes(shapes)
    }
}

fn text_options() -> TextOptions {
    TextOptions {
        max_texture_side: MAX_ATLAS,
        ..Default::default()
    }
}

impl Ui {
    /// Paints the laid-out tree into `overlay`.
    pub(crate) fn paint(&mut self, overlay: &mut Overlay) {
        profiling::scope!("ui paint");
        let mut shapes = Vec::new();
        for root in self.draw_order() {
            self.paint_node(root, &mut shapes);
        }
        let primitives = self.painter.tessellate(shapes);
        if let Some(update) = self.painter.atlas_update() {
            overlay.texture_updates.push(update);
        }
        overlay.live_textures.clear();
        overlay.live_textures.push(self.painter.atlas_id);
        let scale = self.scale;
        let atlas = self.painter.atlas_id;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut batches: Vec<OverlayBatch> = Vec::new();
        for p in primitives {
            let Primitive::Mesh(mesh) = p.primitive else {
                continue;
            };
            if mesh.is_empty() {
                continue;
            }
            let texture = match mesh.texture_id {
                TextureId::Managed(_) => OverlayTexture::Managed(atlas),
                TextureId::User(id) => OverlayTexture::Image(id),
            };
            let c = p.clip_rect;
            let x0 = (c.min.x * scale).max(0.0).round();
            let y0 = (c.min.y * scale).max(0.0).round();
            let x1 = (c.max.x * scale).min(65535.0).round();
            let y1 = (c.max.y * scale).min(65535.0).round();
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let clip = [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32];
            let base = vertices.len() as u32;
            vertices.extend(mesh.vertices.iter().map(|v| OverlayVertex {
                pos: [v.pos.x * scale, v.pos.y * scale],
                uv: [v.uv.x, v.uv.y],
                color: v.color.to_array(),
            }));
            let first = indices.len() as u32;
            indices.extend(mesh.indices.iter().map(|i| i + base));
            let count = mesh.indices.len() as u32;
            match batches.last_mut() {
                Some(b) if b.texture == texture && b.clip == clip => b.index_count += count,
                _ => batches.push(OverlayBatch {
                    texture,
                    clip,
                    first_index: first,
                    index_count: count,
                }),
            }
        }
        overlay.set_geometry(vertices, indices, batches);
    }

    fn paint_node(&self, id: NodeId, out: &mut Vec<ClippedShape>) {
        let Some(n) = self.node(id) else { return };
        if !n.shown || !n.rect.is_positive() {
            return;
        }
        let r = &n.resolved;
        let rect = n.rect;
        let clip = n.clip;
        let radius = CornerRadius::from(r.radius);
        let mut push = |shape: Shape| out.push(ClippedShape::new(clip, shape));
        if let Some(s) = r.shadow
            && r.background.a() > 0
        {
            push(Shape::Rect(s.as_shape(rect, radius)));
        }
        if r.background.a() > 0 || (r.border_width > 0.0 && r.border_color.a() > 0) {
            push(Shape::Rect(RectShape::new(
                rect,
                radius,
                r.background,
                Stroke::new(r.border_width, r.border_color),
                StrokeKind::Inside,
            )));
        }
        let inner = Rect::from_min_max(
            pos2(rect.min.x + r.padding.left, rect.min.y + r.padding.top),
            pos2(rect.max.x - r.padding.right, rect.max.y - r.padding.bottom),
        );
        let text_at = |galley: &Arc<Galley>, area: Rect| -> Pos2 {
            // Galleys align within their wrap width; centre unwrapped text
            // ourselves.
            let size = galley.size();
            let x = match r.text_align {
                TextAlign::Start => area.min.x,
                TextAlign::Center => area.center().x - size.x / 2.0,
                TextAlign::End => area.max.x - size.x,
            };
            let x = x - galley.rect.min.x;
            pos2(x, area.center().y - size.y / 2.0)
        };
        let t = if n.max > n.min {
            ((n.value - n.min) / (n.max - n.min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        match n.kind {
            NodeKind::Text | NodeKind::Button => {
                if let Some(g) = &n.galley {
                    let mut pos = text_at(g, inner);
                    if n.kind == NodeKind::Text {
                        pos.y = inner.min.y;
                    }
                    push(Shape::galley(pos, g.clone(), r.text_color));
                }
            }
            NodeKind::Slider | NodeKind::Toggle => {
                let ch = control_height(n.kind, r);
                let control = Rect::from_min_size(
                    pos2(inner.max.x - r.control_width, inner.center().y - ch / 2.0),
                    vec2(r.control_width, ch),
                );
                if let Some(g) = &n.galley {
                    let size = g.size();
                    push(Shape::galley(
                        pos2(inner.min.x, inner.center().y - size.y / 2.0),
                        g.clone(),
                        r.text_color,
                    ));
                }
                if n.kind == NodeKind::Slider {
                    let h = r.control_height;
                    let track = Rect::from_min_size(
                        pos2(control.min.x, control.center().y - h / 2.0),
                        vec2(control.width(), h),
                    );
                    let round = CornerRadius::from(h / 2.0);
                    push(Shape::rect_filled(track, round, r.track_color));
                    let mut filled = track;
                    filled.max.x = track.min.x + track.width() * t;
                    if filled.width() > 0.5 {
                        push(Shape::rect_filled(filled, round, r.fill_color));
                    }
                    push(Shape::circle_filled(
                        pos2(filled.max.x, track.center().y),
                        ch / 2.0,
                        r.knob_color,
                    ));
                } else {
                    let round = CornerRadius::from(control.height() / 2.0);
                    let track = if n.checked {
                        r.fill_color
                    } else {
                        r.track_color
                    };
                    push(Shape::rect_filled(control, round, track));
                    let knob_r = control.height() / 2.0 - 3.0;
                    let x = if n.checked {
                        control.max.x - control.height() / 2.0
                    } else {
                        control.min.x + control.height() / 2.0
                    };
                    push(Shape::circle_filled(
                        pos2(x, control.center().y),
                        knob_r.max(1.0),
                        r.knob_color,
                    ));
                }
            }
            NodeKind::Progress => {
                let mut filled = rect;
                filled.max.x = rect.min.x + rect.width() * t;
                if filled.width() > 0.5 {
                    push(Shape::rect_filled(filled, radius, r.fill_color));
                }
            }
            NodeKind::Image => {
                if let Some(h) = n.image_handle
                    && inner.is_positive()
                {
                    let mut mesh = Mesh::with_texture(TextureId::User(h.id()));
                    mesh.add_rect_with_uv(
                        inner,
                        Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
                        r.tint,
                    );
                    push(Shape::mesh(mesh));
                }
            }
            _ => {}
        }
        for c in &n.children {
            self.paint_node(*c, out);
        }
        // The focus ring, over the node's children.
        if self.focused == Some(id)
            && let Some(ring) = &self.theme.focus_ring
            && let Some(color) = self.theme.color(&ring.color)
        {
            let grown = rect.expand(ring.offset);
            out.push(ClippedShape::new(
                Rect::EVERYTHING,
                Shape::rect_stroke(
                    grown,
                    CornerRadius::from(r.radius + ring.offset),
                    Stroke::new(ring.width, color),
                    StrokeKind::Outside,
                ),
            ));
        }
    }
}
