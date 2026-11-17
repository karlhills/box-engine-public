//! Design tokens and their mapping onto egui's `Style` (from spike S1).
//!
//! Everything visual in the editor reads from [`Tokens`]; nothing else
//! hard-codes colours. The active tokens live in the egui context
//! ([`apply`] stores them, [`tokens`] reads them), so a light theme is a second
//! `Tokens` value, not a code change.

use std::sync::Arc;

use egui::epaint::text::{FontTweak, VariationCoords};
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    TextStyle, Vec2,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    // Surfaces, darkest to lightest.
    pub bg_app: Color32,
    pub surface: Color32,
    pub surface_header: Color32,
    /// Panel background while playing: the surface with a cool tint, so
    /// panes that show the (throwaway) play world look different.
    pub surface_play: Color32,
    pub field: Color32,
    pub field_hover: Color32,
    pub field_active: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub guide: Color32,
    // Text.
    pub text_strong: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub text_faint: Color32,
    // The one accent, plus its derived selection tint.
    pub accent: Color32,
    pub accent_bg: Color32,
    pub accent_text: Color32,
    // Functional colours (never decorative).
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
    pub warn: Color32,
    pub error: Color32,
    /// Message text of error lines in the console.
    pub error_text: Color32,
    // Viewport overlays (drawn over the 3D image, not over chrome).
    pub overlay_text: Color32,
    pub overlay_bg: Color32,
    // Metrics (points).
    pub row_h: f32,
    pub label_w: f32,
    pub radius: u8,
    pub pad_x: f32,
    pub tab_h: f32,
    pub toolbar_h: f32,
    pub font_body: f32,
    pub font_small: f32,
    pub font_mono: f32,
}

pub const DARK: Tokens = Tokens {
    bg_app: Color32::from_rgb(0x12, 0x12, 0x14),
    surface: Color32::from_rgb(0x1d, 0x1d, 0x20),
    surface_header: Color32::from_rgb(0x18, 0x18, 0x1b),
    surface_play: Color32::from_rgb(0x1a, 0x20, 0x2b),
    field: Color32::from_rgb(0x28, 0x28, 0x2c),
    field_hover: Color32::from_rgb(0x2f, 0x2f, 0x34),
    field_active: Color32::from_rgb(0x36, 0x36, 0x3c),
    hover: Color32::from_rgb(0x25, 0x25, 0x29),
    border: Color32::from_rgb(0x2a, 0x2a, 0x2f),
    divider: Color32::from_rgb(0x0c, 0x0c, 0x0e),
    guide: Color32::from_rgb(0x2b, 0x2b, 0x30),
    text_strong: Color32::from_rgb(0xee, 0xee, 0xf0),
    text: Color32::from_rgb(0xc8, 0xc8, 0xcd),
    text_weak: Color32::from_rgb(0x8a, 0x8a, 0x93),
    text_faint: Color32::from_rgb(0x5a, 0x5a, 0x63),
    accent: Color32::from_rgb(0x4b, 0x8b, 0xf5),
    accent_bg: Color32::from_rgb(0x24, 0x44, 0x78),
    accent_text: Color32::from_rgb(0xf4, 0xf7, 0xff),
    axis_x: Color32::from_rgb(0xc8, 0x5a, 0x55),
    axis_y: Color32::from_rgb(0x79, 0xa8, 0x56),
    axis_z: Color32::from_rgb(0x55, 0x80, 0xc8),
    warn: Color32::from_rgb(0xd6, 0xa4, 0x4c),
    error: Color32::from_rgb(0xe0, 0x6c, 0x64),
    error_text: Color32::from_rgb(0xe8, 0xb4, 0xae),
    overlay_text: Color32::from_rgb(0xe6, 0xe6, 0xea),
    overlay_bg: Color32::from_rgba_premultiplied(0, 0, 0, 110),
    row_h: 22.0,
    label_w: 96.0,
    radius: 3,
    pad_x: 10.0,
    tab_h: 28.0,
    toolbar_h: 36.0,
    font_body: 12.0,
    font_small: 11.0,
    font_mono: 11.0,
};

impl Default for Tokens {
    fn default() -> Self {
        DARK
    }
}

impl Tokens {
    pub fn small(&self) -> FontId {
        FontId::proportional(self.font_small)
    }

    pub fn body(&self) -> FontId {
        FontId::proportional(self.font_body)
    }

    pub fn strong(&self) -> FontId {
        FontId::new(self.font_body, FontFamily::Name(SEMIBOLD.into()))
    }

    pub fn strong_small(&self) -> FontId {
        FontId::new(self.font_small, FontFamily::Name(SEMIBOLD.into()))
    }

    pub fn mono(&self) -> FontId {
        FontId::monospace(self.font_mono)
    }
}

pub(crate) const SEMIBOLD: &str = "semibold";

/// The tokens [`apply`] installed in this context (the dark theme if none).
pub fn tokens(ctx: &egui::Context) -> Tokens {
    ctx.data(|d| d.get_temp::<Tokens>(egui::Id::NULL))
        .unwrap_or(DARK)
}

/// Inter (variable; `wght` 430 for body text and 590 for semibold) and
/// Source Code Pro for numbers and the console. Both are SIL OFL 1.1; the
/// licences are next to the files in `assets/fonts`. egui 0.37 ships no fonts
/// by default, and a monospace font without ligatures keeps `->` literal.
pub fn fonts() -> FontDefinitions {
    let inter: &'static [u8] = include_bytes!("../assets/fonts/Inter-Variable.ttf");
    let mono: &'static [u8] = include_bytes!("../assets/fonts/SourceCodePro-Regular.ttf");

    let mut defs = FontDefinitions::empty();
    let weight = |w: f32| FontTweak {
        coords: VariationCoords::new([(b"wght", w)]),
        ..Default::default()
    };
    defs.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(inter).tweak(weight(430.0))),
    );
    defs.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(inter).tweak(weight(590.0))),
    );
    defs.font_data
        .insert("mono".into(), Arc::new(FontData::from_static(mono)));
    defs.families.insert(
        FontFamily::Proportional,
        vec!["inter".into(), "mono".into()],
    );
    defs.families
        .insert(FontFamily::Monospace, vec!["mono".into(), "inter".into()]);
    defs.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        vec!["inter-semibold".into(), "mono".into()],
    );
    defs
}

/// Installs fonts, maps `t` onto egui's dark style and stores `t` for
/// [`tokens`].
pub fn apply(ctx: &egui::Context, t: &Tokens) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, *t));
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Small, t.small()),
            (TextStyle::Body, t.body()),
            (TextStyle::Button, t.body()),
            (TextStyle::Heading, t.strong()),
            (TextStyle::Monospace, t.mono()),
        ]
        .into();
        style.drag_value_text_style = TextStyle::Monospace;

        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(6.0, 4.0);
        s.button_padding = Vec2::new(8.0, 3.0);
        s.interact_size = Vec2::new(24.0, t.row_h - 2.0);
        s.indent = 14.0;
        s.icon_width = 14.0;
        s.icon_width_inner = 8.0;
        s.icon_spacing = 6.0;
        s.combo_height = 300.0;
        s.menu_margin = Margin::same(4);
        s.window_margin = Margin::same(8);
        s.slider_rail_height = 4.0;
        s.scroll.bar_width = 6.0;
        s.scroll.floating = true;
        s.scroll.floating_width = 2.0;
        s.scroll.bar_inner_margin = 2.0;
        s.scroll.bar_outer_margin = 2.0;

        style.interaction.selectable_labels = false;
        style.interaction.tooltip_delay = 0.4;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = t.surface;
        v.window_fill = t.field;
        v.window_stroke = Stroke::new(1.0, t.border);
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.window_corner_radius = CornerRadius::same(5);
        v.menu_corner_radius = CornerRadius::same(5);
        v.extreme_bg_color = t.bg_app;
        v.text_edit_bg_color = Some(t.field);
        v.faint_bg_color = t.hover;
        v.code_bg_color = t.field;
        v.warn_fg_color = t.warn;
        v.error_fg_color = t.error;
        v.hyperlink_color = t.accent;
        v.selection.bg_fill = t.accent_bg;
        v.selection.stroke = Stroke::new(1.0, t.accent);
        v.collapsing_header_frame = false;
        v.indent_has_left_vline = false;
        v.striped = false;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
        v.button_frame = true;
        v.text_cursor.stroke = Stroke::new(1.5, t.accent);

        let r = CornerRadius::same(t.radius);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = t.surface;
        w.noninteractive.weak_bg_fill = t.surface;
        w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
        w.noninteractive.corner_radius = r;

        w.inactive.bg_fill = t.field;
        w.inactive.weak_bg_fill = t.field;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(1.0, t.text);
        w.inactive.corner_radius = r;
        w.inactive.expansion = 0.0;

        w.hovered.bg_fill = t.field_hover;
        w.hovered.weak_bg_fill = t.field_hover;
        w.hovered.bg_stroke = Stroke::NONE;
        w.hovered.fg_stroke = Stroke::new(1.0, t.text_strong);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = t.field_active;
        w.active.weak_bg_fill = t.field_active;
        w.active.bg_stroke = Stroke::new(1.0, t.accent);
        w.active.fg_stroke = Stroke::new(1.0, t.text_strong);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;

        w.open.bg_fill = t.field_active;
        w.open.weak_bg_fill = t.field_active;
        w.open.bg_stroke = Stroke::NONE;
        w.open.fg_stroke = Stroke::new(1.0, t.text_strong);
        w.open.corner_radius = r;
    });
}
