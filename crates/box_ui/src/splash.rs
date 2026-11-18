//! The startup splash's picture: a black screen with a small grey "made
//! with" line above the "box engine" wordmark (DM Sans: "box" bold, " engine"
//! regular, tight letter spacing), centred and sized to the target. The
//! engine's `box_engine::splash` decides when it shows and times the fades;
//! this module only paints one frame of it into an [`Overlay`].

use std::collections::BTreeMap;
use std::sync::Arc;

use box_render::{Overlay, OverlayBatch, OverlayTexture, OverlayTextureUpdate, OverlayVertex};
use epaint::text::{
    FontData, FontDefinitions, FontFamily, FontId, Fonts, LayoutJob, TextFormat, TextOptions,
};
use epaint::{
    ClippedShape, Color32, Galley, Primitive, Rect, Shape, TessellationOptions, Tessellator, pos2,
};

/// DM Sans Regular (SIL Open Font License 1.1; the licence is
/// `crates/box_ui/assets/fonts/OFL-DMSans.txt`).
pub const DM_SANS_REGULAR: &[u8] = include_bytes!("../assets/fonts/DMSans-Regular.ttf");
/// DM Sans Bold (same licence).
pub const DM_SANS_BOLD: &[u8] = include_bytes!("../assets/fonts/DMSans-Bold.ttf");

/// The wordmark's letter spacing, in ems (-1 px at 21 px).
pub const LETTER_SPACING_EM: f32 = -1.0 / 21.0;
/// The wordmark's font size as a fraction of the target's shorter side.
pub const WORDMARK_FRACTION: f32 = 0.07;
/// Smallest and largest wordmark font size, in points.
pub const WORDMARK_MIN: f32 = 24.0;
pub const WORDMARK_MAX: f32 = 120.0;
/// "made with" font size as a fraction of the wordmark's.
pub const MADE_WITH_SCALE: f32 = 0.38;
/// Gap between the two lines' boxes as a fraction of the wordmark size.
pub const GAP_SCALE: f32 = 0.06;

const WORDMARK_COLOR: [u8; 3] = [0xff, 0xff, 0xff];
const MADE_WITH_COLOR: [u8; 3] = [0x9a, 0x9a, 0x9a];

const BOLD: &str = "dm-sans-bold";
const REGULAR: &str = "dm-sans-regular";

/// The wordmark's font size in points for a target of `width` x `height`
/// points.
pub fn wordmark_size(width: f32, height: f32) -> f32 {
    (width.min(height) * WORDMARK_FRACTION).clamp(WORDMARK_MIN, WORDMARK_MAX)
}

/// Paints splash frames. Owns its own font set (DM Sans only) and font
/// atlas, so it works without a [`Ui`](crate::Ui) or any loaded asset.
pub struct SplashPainter {
    fonts: Fonts,
    atlas_id: u64,
}

impl Default for SplashPainter {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SplashPainter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplashPainter")
            .field("atlas_id", &self.atlas_id)
            .finish_non_exhaustive()
    }
}

impl SplashPainter {
    /// Parses the two bundled faces (once).
    pub fn new() -> Self {
        let mut defs = FontDefinitions {
            font_data: BTreeMap::new(),
            families: BTreeMap::new(),
        };
        defs.font_data
            .insert(BOLD.into(), Arc::new(FontData::from_static(DM_SANS_BOLD)));
        defs.font_data.insert(
            REGULAR.into(),
            Arc::new(FontData::from_static(DM_SANS_REGULAR)),
        );
        defs.families
            .insert(FontFamily::Name(BOLD.into()), vec![BOLD.into()]);
        defs.families
            .insert(FontFamily::Name(REGULAR.into()), vec![REGULAR.into()]);
        defs.families
            .insert(FontFamily::Proportional, vec![REGULAR.into()]);
        defs.families
            .insert(FontFamily::Monospace, vec![REGULAR.into()]);
        Self {
            fonts: Fonts::new(TextOptions::default(), defs),
            atlas_id: Overlay::new_texture_id(),
        }
    }

    /// Replaces `overlay`'s geometry with one splash frame for a target of
    /// `width` x `height` physical pixels at `scale` pixels per point: an
    /// opaque black rectangle over the whole target, then the text at
    /// `alpha` (0 invisible, 1 fully shown) over it.
    pub fn paint(
        &mut self,
        overlay: &mut Overlay,
        width: u32,
        height: u32,
        scale: f32,
        alpha: f32,
    ) {
        profiling::scope!("splash paint");
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let alpha = alpha.clamp(0.0, 1.0);
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let (pw, ph) = (w / scale, h / scale);
        let size = wordmark_size(pw, ph);

        self.fonts
            .begin_pass(TextOptions::default(), Default::default());
        let format = |family: &str, size: f32, rgb: [u8; 3]| TextFormat {
            font_id: FontId::new(size, FontFamily::Name(family.into())),
            extra_letter_spacing: size * LETTER_SPACING_EM,
            color: Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
            ..Default::default()
        };
        let mut wordmark = LayoutJob::default();
        wordmark.append("box", 0.0, format(BOLD, size, WORDMARK_COLOR));
        wordmark.append(" engine", 0.0, format(REGULAR, size, WORDMARK_COLOR));
        let small = size * MADE_WITH_SCALE;
        let mut made_with = LayoutJob::default();
        made_with.append(
            "made with",
            0.0,
            TextFormat {
                extra_letter_spacing: 0.0,
                ..format(REGULAR, small, MADE_WITH_COLOR)
            },
        );
        let (wordmark, made_with) = {
            let mut view = self.fonts.with_pixels_per_point(scale);
            (view.layout_job(wordmark), view.layout_job(made_with))
        };

        // The group: "made with", a gap, the wordmark; centred as a whole,
        // each line centred on its ink (letter spacing leaves a trailing gap
        // in the line box). Origins snap to whole pixels for crisp glyphs.
        let gap = size * GAP_SCALE;
        let total = made_with.rect.height() + gap + wordmark.rect.height();
        let top = (ph - total) / 2.0;
        let snap = |v: f32| (v * scale).round() / scale;
        let place = |g: &Arc<Galley>, y: f32| {
            let ink = if g.mesh_bounds.is_positive() {
                g.mesh_bounds
            } else {
                g.rect
            };
            pos2(snap(pw / 2.0 - ink.center().x), snap(y - g.rect.min.y))
        };
        let shapes = vec![
            ClippedShape::new(
                Rect::EVERYTHING,
                Shape::galley(place(&made_with, top), made_with.clone(), Color32::WHITE),
            ),
            ClippedShape::new(
                Rect::EVERYTHING,
                Shape::galley(
                    place(&wordmark, top + made_with.rect.height() + gap),
                    wordmark.clone(),
                    Color32::WHITE,
                ),
            ),
        ];
        let primitives = {
            let atlas = self.fonts.texture_atlas();
            Tessellator::new(
                scale,
                TessellationOptions::default(),
                self.fonts.font_image_size(),
                atlas.prepared_discs(),
            )
            .tessellate_shapes(shapes)
        };

        // The black background: the atlas's white texel tinted black.
        let white = [epaint::WHITE_UV.x, epaint::WHITE_UV.y];
        let black = [0, 0, 0, 255];
        let mut vertices: Vec<OverlayVertex> = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]]
            .into_iter()
            .map(|pos| OverlayVertex {
                pos,
                uv: white,
                color: black,
            })
            .collect();
        let mut indices = vec![0, 1, 2, 0, 2, 3];
        if alpha > 0.0 {
            // Premultiplied colours: fading scales every channel.
            let fade = |c: u8| (c as f32 * alpha).round() as u8;
            for p in primitives {
                let Primitive::Mesh(mesh) = p.primitive else {
                    continue;
                };
                let base = vertices.len() as u32;
                vertices.extend(mesh.vertices.iter().map(|v| OverlayVertex {
                    pos: [v.pos.x * scale, v.pos.y * scale],
                    uv: [v.uv.x, v.uv.y],
                    color: v.color.to_array().map(fade),
                }));
                indices.extend(mesh.indices.iter().map(|i| i + base));
            }
        }
        let batch = OverlayBatch {
            texture: OverlayTexture::Managed(self.atlas_id),
            clip: [0, 0, width.max(1), height.max(1)],
            first_index: 0,
            index_count: indices.len() as u32,
        };
        if let Some(delta) = self.fonts.font_image_delta() {
            let epaint::ImageData::Color(image) = &delta.image;
            let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
            for c in &image.pixels {
                rgba.extend_from_slice(&c.to_array());
            }
            overlay.texture_updates.push(OverlayTextureUpdate {
                id: self.atlas_id,
                pos: delta.pos.map(|[x, y]| [x as u32, y as u32]),
                size: [image.size[0] as u32, image.size[1] as u32],
                rgba,
                linear: true,
            });
        }
        overlay.live_textures.clear();
        overlay.live_textures.push(self.atlas_id);
        overlay.set_geometry(vertices, indices, vec![batch]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_shorter_side_within_limits() {
        assert_eq!(wordmark_size(1280.0, 720.0), 720.0 * WORDMARK_FRACTION);
        assert_eq!(wordmark_size(800.0, 1200.0), 800.0 * WORDMARK_FRACTION);
        assert_eq!(wordmark_size(100.0, 100.0), WORDMARK_MIN);
        assert_eq!(wordmark_size(8000.0, 8000.0), WORDMARK_MAX);
    }

    #[test]
    fn paints_a_black_screen_and_fading_text() {
        let mut painter = SplashPainter::new();
        let mut overlay = Overlay::default();
        painter.paint(&mut overlay, 1280, 720, 1.0, 0.0);
        // Hidden text: only the background, opaque black over the target.
        assert_eq!(overlay.vertices().len(), 4);
        assert!(overlay.vertices().iter().all(|v| v.color == [0, 0, 0, 255]));
        assert_eq!(overlay.batches()[0].clip, [0, 0, 1280, 720]);
        assert_eq!(overlay.texture_updates.len(), 1, "the first atlas upload");

        overlay.texture_updates.clear();
        painter.paint(&mut overlay, 1280, 720, 1.0, 1.0);
        let text = &overlay.vertices()[4..];
        assert!(!text.is_empty());
        // White wordmark, grey "made with", centred around the middle.
        assert!(text.iter().any(|v| v.color == [255, 255, 255, 255]));
        assert!(text.iter().any(|v| v.color == [0x9a, 0x9a, 0x9a, 255]));
        let (min, max) = text
            .iter()
            .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), v| {
                (
                    [lo[0].min(v.pos[0]), lo[1].min(v.pos[1])],
                    [hi[0].max(v.pos[0]), hi[1].max(v.pos[1])],
                )
            });
        let centre = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
        assert!((centre[0] - 640.0).abs() < 4.0, "{centre:?}");
        assert!((centre[1] - 360.0).abs() < 12.0, "{centre:?}");
        assert!(max[0] - min[0] < 1280.0 * 0.5, "{min:?} {max:?}");

        painter.paint(&mut overlay, 1280, 720, 1.0, 0.5);
        let half = &overlay.vertices()[4..];
        assert!(half.iter().all(|v| v.color[3] <= 128));
        assert!(half.iter().any(|v| v.color == [128, 128, 128, 128]));
        assert_eq!(overlay.vertices()[0].color, [0, 0, 0, 255]);
    }

    #[test]
    fn high_dpi_targets_scale_the_text() {
        let mut painter = SplashPainter::new();
        let mut one = Overlay::default();
        let mut two = Overlay::default();
        painter.paint(&mut one, 1280, 720, 1.0, 1.0);
        painter.paint(&mut two, 2560, 1440, 2.0, 1.0);
        let width = |o: &Overlay| {
            let xs = o.vertices()[4..].iter().map(|v| v.pos[0]);
            xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
        };
        let ratio = width(&two) / width(&one);
        assert!((ratio - 2.0).abs() < 0.05, "{ratio}");
    }
}
