//! Linear colours.

use crate::pmath;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::reflect::{Reflect, ReflectDefault, ReflectDeserialize, ReflectSerialize};

/// A colour in linear space. Use [`Color::srgb`] for values picked in sRGB.
///
/// Serialized as `[r, g, b, a]` (linear). Deserialization also accepts
/// `[r, g, b]` (alpha 1) and the object form `{ "r", "g", "b", "a"? }`.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[type_path = "box_engine"]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self::WHITE
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        [self.r, self.g, self.b, self.a].serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ColorVisitor;

        impl<'de> Visitor<'de> for ColorVisitor {
            type Value = Color;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a colour as [r, g, b], [r, g, b, a] or { r, g, b, a }")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Color, A::Error> {
                let mut c = [0.0f32, 0.0, 0.0, 1.0];
                let mut n = 0;
                while let Some(v) = seq.next_element::<f32>()? {
                    if n == 4 {
                        return Err(de::Error::invalid_length(5, &self));
                    }
                    c[n] = v;
                    n += 1;
                }
                if n < 3 {
                    return Err(de::Error::invalid_length(n, &self));
                }
                Ok(Color {
                    r: c[0],
                    g: c[1],
                    b: c[2],
                    a: c[3],
                })
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Color, A::Error> {
                let (mut r, mut g, mut b, mut a) = (None, None, None, None);
                while let Some(key) = map.next_key::<String>()? {
                    let slot = match key.as_str() {
                        "r" => &mut r,
                        "g" => &mut g,
                        "b" => &mut b,
                        "a" => &mut a,
                        other => {
                            return Err(de::Error::unknown_field(other, &["r", "g", "b", "a"]));
                        }
                    };
                    *slot = Some(map.next_value::<f32>()?);
                }
                Ok(Color {
                    r: r.ok_or_else(|| de::Error::missing_field("r"))?,
                    g: g.ok_or_else(|| de::Error::missing_field("g"))?,
                    b: b.ok_or_else(|| de::Error::missing_field("b"))?,
                    a: a.unwrap_or(1.0),
                })
            }
        }

        deserializer.deserialize_any(ColorVisitor)
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        pmath::powf((c + 0.055) / 1.055, 2.4)
    }
}

impl Color {
    pub const WHITE: Self = Self::linear(1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::linear(0.0, 0.0, 0.0);
    pub const RED: Self = Self::linear(1.0, 0.0, 0.0);
    pub const GREEN: Self = Self::linear(0.0, 1.0, 0.0);
    pub const BLUE: Self = Self::linear(0.0, 0.0, 1.0);

    pub const fn linear(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn linear_rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// From sRGB-encoded components in 0..=1.
    pub fn srgb(r: f32, g: f32, b: f32) -> Self {
        Self::linear(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b))
    }

    /// From an sRGB hex value such as `0xd9c7a8`.
    pub fn hex(rgb: u32) -> Self {
        let c = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.0;
        Self::srgb(c(16), c(8), c(0))
    }

    pub fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn to_vec3(self) -> glam::Vec3 {
        glam::Vec3::new(self.r, self.g, self.b)
    }

    pub fn scaled(self, k: f32) -> Self {
        Self::linear_rgba(self.r * k, self.g * k, self.b * k, self.a)
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self::linear_rgba(
            l(self.r, other.r),
            l(self.g, other.g),
            l(self.b, other.b),
            l(self.a, other.a),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_conversion_endpoints() {
        assert_eq!(Color::srgb(0.0, 1.0, 0.0), Color::linear(0.0, 1.0, 0.0));
        let mid = Color::srgb(0.5, 0.5, 0.5).r;
        assert!((mid - 0.214).abs() < 0.001, "{mid}");
        assert_eq!(Color::hex(0xff0000), Color::RED);
    }

    #[test]
    fn serializes_as_array_and_accepts_short_and_object_forms() {
        let c = Color {
            r: 0.25,
            g: 0.5,
            b: 0.75,
            a: 0.5,
        };
        assert_eq!(serde_json::to_string(&c).unwrap(), "[0.25,0.5,0.75,0.5]");
        let from = |s: &str| serde_json::from_str::<Color>(s);
        assert_eq!(from("[0.25,0.5,0.75,0.5]").unwrap(), c);
        assert_eq!(from("[1,0,0]").unwrap(), Color::RED);
        assert_eq!(from(r#"{"r":1,"g":0,"b":0}"#).unwrap(), Color::RED);
        assert!(from("[1,0]").is_err());
        assert!(from("[1,0,0,1,1]").is_err());
        assert!(from(r#"{"r":1,"g":0,"x":0}"#).is_err());
    }

    #[test]
    fn reflected_scene_value_uses_the_array_form() {
        let registry = crate::reflect::Registry::with_core_types();
        let json = registry.value_to_json(&Color::RED).unwrap();
        assert_eq!(json, serde_json::json!([1.0, 0.0, 0.0, 1.0]));
    }
}
