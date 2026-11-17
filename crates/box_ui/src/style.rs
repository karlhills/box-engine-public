//! Styles and themes.
//!
//! A [`Style`] is a set of optional properties (sizes, spacing, colours,
//! text, widget parts) plus state variants (`hover`, `focus`, `pressed`,
//! `disabled`, `checked`). A node's style is resolved by overlaying, in
//! order: the built-in defaults, the theme's class named after the node's
//! kind (`"button"`), each class in the node's `class` (space separated),
//! the node's own `style`, then the variants of its current state.
//!
//! A [`Theme`] (`*.theme.json`) names fonts by role, a colour palette and
//! the classes; it is merged over the built-in theme
//! (`crates/box_ui/src/default.theme.json`), field by field.

use std::collections::BTreeMap;

use epaint::Color32;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A length along one axis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Size {
    /// Fit the content.
    #[default]
    Auto,
    /// Reference points (see [`ScaleMode`](crate::ScaleMode)).
    Px(f32),
    /// A fraction (0..=1) of the parent's content box (the screen for a root).
    Percent(f32),
    /// Share the parent's remaining main-axis space (by `grow`); across the
    /// parent's axis, its full content size.
    Fill,
}

impl Serialize for Size {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Size::Auto => s.serialize_str("auto"),
            Size::Fill => s.serialize_str("fill"),
            Size::Px(v) => s.serialize_f32(*v),
            Size::Percent(p) => s.serialize_str(&format!("{}%", p * 100.0)),
        }
    }
}

impl<'de> Deserialize<'de> for Size {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => Ok(Size::Px(n.as_f64().unwrap_or(0.0) as f32)),
            serde_json::Value::String(s) => match s.as_str() {
                "auto" => Ok(Size::Auto),
                "fill" => Ok(Size::Fill),
                p if p.ends_with('%') => p[..p.len() - 1]
                    .trim()
                    .parse::<f32>()
                    .map(|v| Size::Percent(v / 100.0))
                    .map_err(|_| D::Error::custom(format!("bad size `{s}`"))),
                _ => Err(D::Error::custom(format!(
                    "bad size `{s}` (a number, \"auto\", \"fill\" or \"50%\")"
                ))),
            },
            other => Err(D::Error::custom(format!("bad size {other}"))),
        }
    }
}

/// Padding: one number, `[vertical, horizontal]` or `[top, right, bottom, left]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub const fn all(v: f32) -> Self {
        Self {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    pub const fn symmetric(vertical: f32, horizontal: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

impl Serialize for Edges {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.top, self.right, self.bottom, self.left].serialize(s)
    }
}

impl<'de> Deserialize<'de> for Edges {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let num = |v: &serde_json::Value| {
            v.as_f64()
                .map(|f| f as f32)
                .ok_or_else(|| D::Error::custom(format!("bad padding value {v}")))
        };
        match &v {
            serde_json::Value::Number(_) => Ok(Edges::all(num(&v)?)),
            serde_json::Value::Array(a) => match a.as_slice() {
                [vv, h] => Ok(Edges::symmetric(num(vv)?, num(h)?)),
                [t, r, b, l] => Ok(Edges {
                    top: num(t)?,
                    right: num(r)?,
                    bottom: num(b)?,
                    left: num(l)?,
                }),
                _ => Err(D::Error::custom(
                    "padding: a number, [vertical, horizontal] or [top, right, bottom, left]",
                )),
            },
            other => Err(D::Error::custom(format!("bad padding {other}"))),
        }
    }
}

/// Cross-axis alignment of children (or of one child with `align_self`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Start,
    Center,
    End,
    /// Take the parent's full cross size (unless the child has a size).
    #[default]
    Stretch,
}

/// Main-axis distribution of children.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Horizontal text alignment within a node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

/// A drop shadow.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shadow {
    pub color: String,
    pub blur: f32,
    pub offset: [f32; 2],
    pub spread: f32,
}

/// Style properties; every field is optional so styles overlay. Colours are
/// `"#rrggbb"`, `"#rrggbbaa"` (sRGB) or `"$name"` from the theme palette.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Style {
    // Box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<Size>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding: Option<Edges>,
    /// Space between children.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<f32>,
    /// Share of the parent's remaining space for `Fill` sizes (default 1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grow: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align_self: Option<Align>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub justify: Option<Justify>,
    // Look.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow: Option<Shadow>,
    /// Multiplies the alpha of everything the node draws (not its children).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    // Text.
    /// A font role from the theme's `fonts` (`"body"`, `"display"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_align: Option<TextAlign>,
    /// Wrap text at the node's width (default true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrap: Option<bool>,
    // Widget parts (slider, toggle, progress bar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knob_color: Option<String>,
    /// Width of a slider or toggle control next to its label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_width: Option<f32>,
    /// Height of a slider track, toggle or progress bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_height: Option<f32>,
    /// Image tint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<String>,
    // State variants.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hover: Option<Box<Style>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<Box<Style>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressed: Option<Box<Style>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<Box<Style>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<Box<Style>>,
}

macro_rules! overlay_fields {
    ($dst:ident, $src:ident; $($f:ident),*) => {
        $( if $src.$f.is_some() { $dst.$f = $src.$f.clone(); } )*
    };
}

impl Style {
    /// Sets every field `other` sets (variants merge recursively).
    pub fn overlay(&mut self, other: &Style) {
        overlay_fields!(self, other; width, height, min_width, min_height, max_width, max_height,
            padding, gap, grow, align, align_self, justify, background, border_color, border_width,
            radius, shadow, opacity, font, text_size, text_color, text_align, wrap, track_color,
            fill_color, knob_color, control_width, control_height, tint);
        for (dst, src) in [
            (&mut self.hover, &other.hover),
            (&mut self.focus, &other.focus),
            (&mut self.pressed, &other.pressed),
            (&mut self.disabled, &other.disabled),
            (&mut self.checked, &other.checked),
        ] {
            if let Some(src) = src {
                match dst {
                    Some(d) => d.overlay(src),
                    None => *dst = Some(src.clone()),
                }
            }
        }
    }

    /// This style with the variants for a state applied (and removed).
    pub fn with_state(&self, state: NodeState) -> Style {
        let mut out = self.clone();
        let mut apply = |v: &Option<Box<Style>>| {
            if let Some(v) = v {
                out.overlay(v);
            }
        };
        if state.checked {
            apply(&self.checked);
        }
        if state.disabled {
            apply(&self.disabled);
        } else {
            if state.hovered {
                apply(&self.hover);
            }
            if state.focused {
                apply(&self.focus);
            }
            if state.pressed {
                apply(&self.pressed);
            }
        }
        out.hover = None;
        out.focus = None;
        out.pressed = None;
        out.disabled = None;
        out.checked = None;
        out
    }
}

/// The interaction state a style resolves against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NodeState {
    pub hovered: bool,
    pub focused: bool,
    pub pressed: bool,
    pub disabled: bool,
    pub checked: bool,
}

/// The focus ring drawn around the focused node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FocusRing {
    pub color: String,
    pub width: f32,
    /// Distance outside the node's rectangle.
    pub offset: f32,
}

impl Default for FocusRing {
    fn default() -> Self {
        Self {
            color: "#ffd166".into(),
            width: 3.0,
            offset: 3.0,
        }
    }
}

/// A theme: font roles, a colour palette, the focus ring and style classes.
/// The asset form is `*.theme.json`; see the module docs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Font role → font file (path relative to `assets/`). The role
    /// `"body"` is the default font; roles without a loadable file use it.
    /// The bundled Inter is the fallback for every role.
    pub fonts: BTreeMap<String, String>,
    /// Palette: name → colour, referenced as `"$name"`.
    pub colors: BTreeMap<String, String>,
    pub focus_ring: Option<FocusRing>,
    /// Style classes. A class named after a node kind applies to every node
    /// of that kind.
    pub styles: BTreeMap<String, Style>,
}

impl Theme {
    /// The built-in theme.
    pub fn builtin() -> Theme {
        static JSON: &str = include_str!("default.theme.json");
        serde_json::from_str(JSON).unwrap_or_else(|e| {
            tracing::error!(target: "box_ui", "built-in theme does not parse: {e}");
            Theme::default()
        })
    }

    /// Parses theme JSON.
    pub fn from_json(json: &str) -> Result<Theme, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// `other` over this theme: its fonts, colours and focus ring replace
    /// these; its classes overlay these field by field.
    pub fn overlay(&mut self, other: &Theme) {
        self.fonts
            .extend(other.fonts.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.colors
            .extend(other.colors.iter().map(|(k, v)| (k.clone(), v.clone())));
        if other.focus_ring.is_some() {
            self.focus_ring = other.focus_ring.clone();
        }
        for (name, style) in &other.styles {
            self.styles.entry(name.clone()).or_default().overlay(style);
        }
    }

    /// A colour: `#rgb`, `#rrggbb`, `#rrggbbaa` or `$palette`. `None` when it
    /// does not parse.
    pub fn color(&self, value: &str) -> Option<Color32> {
        let value = value.trim();
        if let Some(name) = value.strip_prefix('$') {
            let v = self.colors.get(name)?;
            if v.starts_with('$') {
                return None; // No chains.
            }
            return parse_hex(v);
        }
        parse_hex(value)
    }
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa` (sRGB, straight alpha).
pub fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let (r, g, b, a) = match h.len() {
        3 => {
            let n = |i: usize| {
                u8::from_str_radix(h.get(i..i + 1)?, 16)
                    .ok()
                    .map(|v| v * 17)
            };
            (n(0)?, n(1)?, n(2)?, 255)
        }
        6 => (byte(0)?, byte(2)?, byte(4)?, 255),
        8 => (byte(0)?, byte(2)?, byte(4)?, byte(6)?),
        _ => return None,
    };
    Some(Color32::from_rgba_unmultiplied(r, g, b, a))
}

/// A style with every value decided, in reference points and premultiplied
/// sRGB colours.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Resolved {
    pub width: Size,
    pub height: Size,
    pub min: [f32; 2],
    pub max: [f32; 2],
    pub padding: Edges,
    pub gap: f32,
    pub grow: f32,
    pub align: Align,
    pub align_self: Option<Align>,
    pub justify: Justify,
    pub background: Color32,
    pub border_color: Color32,
    pub border_width: f32,
    pub radius: f32,
    pub shadow: Option<epaint::Shadow>,
    pub font: String,
    pub text_size: f32,
    pub text_color: Color32,
    pub text_align: TextAlign,
    pub wrap: bool,
    pub track_color: Color32,
    pub fill_color: Color32,
    pub knob_color: Color32,
    pub control_width: f32,
    pub control_height: f32,
    pub tint: Color32,
}

impl Default for Resolved {
    fn default() -> Self {
        Self {
            width: Size::Auto,
            height: Size::Auto,
            min: [0.0; 2],
            max: [f32::INFINITY; 2],
            padding: Edges::default(),
            gap: 0.0,
            grow: 1.0,
            align: Align::Stretch,
            align_self: None,
            justify: Justify::Start,
            background: Color32::TRANSPARENT,
            border_color: Color32::TRANSPARENT,
            border_width: 0.0,
            radius: 0.0,
            shadow: None,
            font: "body".into(),
            text_size: 24.0,
            text_color: Color32::WHITE,
            text_align: TextAlign::Start,
            wrap: true,
            track_color: Color32::from_gray(60),
            fill_color: Color32::WHITE,
            knob_color: Color32::WHITE,
            control_width: 220.0,
            control_height: 12.0,
            tint: Color32::WHITE,
        }
    }
}

impl Resolved {
    /// Resolves `style` (state variants already applied) against `theme`.
    /// `bad` collects colour strings that do not parse.
    pub fn from_style(style: &Style, theme: &Theme, bad: &mut Vec<String>) -> Resolved {
        let mut r = Resolved::default();
        let mut color = |v: &Option<String>, dst: &mut Color32| {
            if let Some(v) = v {
                match theme.color(v) {
                    Some(c) => *dst = c,
                    None => bad.push(v.clone()),
                }
            }
        };
        color(&style.background, &mut r.background);
        color(&style.border_color, &mut r.border_color);
        color(&style.text_color, &mut r.text_color);
        color(&style.track_color, &mut r.track_color);
        color(&style.fill_color, &mut r.fill_color);
        color(&style.knob_color, &mut r.knob_color);
        color(&style.tint, &mut r.tint);
        if let Some(s) = &style.shadow {
            let mut c = Color32::from_black_alpha(80);
            color(&Some(s.color.clone()), &mut c);
            r.shadow = Some(epaint::Shadow {
                offset: [
                    s.offset[0].clamp(-127.0, 127.0) as i8,
                    s.offset[1].clamp(-127.0, 127.0) as i8,
                ],
                blur: s.blur.clamp(0.0, 255.0) as u8,
                spread: s.spread.clamp(0.0, 255.0) as u8,
                color: c,
            });
        }
        if let Some(o) = style.opacity {
            let o = o.clamp(0.0, 1.0);
            for c in [
                &mut r.background,
                &mut r.border_color,
                &mut r.text_color,
                &mut r.track_color,
                &mut r.fill_color,
                &mut r.knob_color,
                &mut r.tint,
            ] {
                *c = c.gamma_multiply(o);
            }
        }
        r.width = style.width.unwrap_or(Size::Auto);
        r.height = style.height.unwrap_or(Size::Auto);
        r.min = [
            style.min_width.unwrap_or(0.0).max(0.0),
            style.min_height.unwrap_or(0.0).max(0.0),
        ];
        r.max = [
            style.max_width.unwrap_or(f32::INFINITY),
            style.max_height.unwrap_or(f32::INFINITY),
        ];
        r.padding = style.padding.unwrap_or_default();
        r.gap = style.gap.unwrap_or(0.0).max(0.0);
        r.grow = style.grow.unwrap_or(1.0).max(0.0);
        r.align = style.align.unwrap_or_default();
        r.align_self = style.align_self;
        r.justify = style.justify.unwrap_or_default();
        r.border_width = style.border_width.unwrap_or(0.0).max(0.0);
        r.radius = style.radius.unwrap_or(0.0).clamp(0.0, 255.0);
        if let Some(f) = &style.font {
            r.font = f.clone();
        }
        r.text_size = style.text_size.unwrap_or(r.text_size).clamp(1.0, 512.0);
        r.text_align = style.text_align.unwrap_or_default();
        r.wrap = style.wrap.unwrap_or(true);
        r.control_width = style.control_width.unwrap_or(r.control_width).max(0.0);
        r.control_height = style.control_height.unwrap_or(r.control_height).max(1.0);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_edges_parse_from_json() {
        let s: Style = serde_json::from_str(
            r#"{"width": 300, "height": "50%", "min_width": 10, "padding": [4, 8], "background": "$accent"}"#,
        )
        .unwrap();
        assert_eq!(s.width, Some(Size::Px(300.0)));
        assert_eq!(s.height, Some(Size::Percent(0.5)));
        assert_eq!(s.padding, Some(Edges::symmetric(4.0, 8.0)));
        assert!(serde_json::from_str::<Style>(r#"{"widht": 3}"#).is_err());
        assert!(serde_json::from_str::<Style>(r#"{"width": "wide"}"#).is_err());
    }

    #[test]
    fn colours_parse_and_resolve_through_the_palette() {
        assert_eq!(parse_hex("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_hex("#ff000080").map(|c| c.a()), Some(128));
        assert_eq!(parse_hex("red"), None);
        let mut t = Theme::default();
        t.colors.insert("accent".into(), "#102030".into());
        assert_eq!(t.color("$accent"), Some(Color32::from_rgb(16, 32, 48)));
        assert_eq!(t.color("$missing"), None);
    }

    #[test]
    fn state_variants_overlay_in_order() {
        let s: Style = serde_json::from_str(
            r##"{"background": "#000", "focus": {"background": "#111"}, "pressed": {"background": "#222"}, "disabled": {"background": "#333"}}"##,
        )
        .unwrap();
        let st = |focused, pressed, disabled| NodeState {
            focused,
            pressed,
            disabled,
            ..Default::default()
        };
        assert_eq!(
            s.with_state(st(false, false, false)).background.as_deref(),
            Some("#000")
        );
        assert_eq!(
            s.with_state(st(true, false, false)).background.as_deref(),
            Some("#111")
        );
        assert_eq!(
            s.with_state(st(true, true, false)).background.as_deref(),
            Some("#222")
        );
        assert_eq!(
            s.with_state(st(true, true, true)).background.as_deref(),
            Some("#333")
        );
    }

    #[test]
    fn the_builtin_theme_parses() {
        let t = Theme::builtin();
        assert!(t.styles.contains_key("button"));
        assert!(t.focus_ring.is_some());
        let mut bad = Vec::new();
        for s in t.styles.values() {
            Resolved::from_style(&s.with_state(NodeState::default()), &t, &mut bad);
        }
        assert!(bad.is_empty(), "{bad:?}");
    }
}
