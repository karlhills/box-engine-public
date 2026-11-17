//! Material descriptions: [`StandardMaterial`], [`AlphaMode`] and
//! [`WindSway`].
//!
//! `StandardMaterial` is saved data. It is a scene component (on an entity
//! with a `Primitive` it holds that entity's material) and the content of
//! material assets (`.material.json`, or materials imported from glTF). Its
//! textures are [`AssetRef`]s; the renderer works with a runtime material
//! whose textures are resolved to loaded images by the asset glue.

use glam::Vec2;

use crate::asset_ref::AssetRef;
use crate::color::Color;
use crate::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit};

/// How a material's alpha is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum AlphaMode {
    /// Alpha is ignored.
    #[default]
    Opaque,
    /// Pixels with alpha below `cutoff` are discarded; the rest are opaque.
    Mask {
        #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
        cutoff: f32,
    },
    /// Alpha blending, drawn back to front after opaque geometry.
    Blend,
    /// Colour is added to what is behind (glows, sparks).
    Additive,
    /// Colour multiplies what is behind (tinted glass, shadows decals).
    Multiply,
}

impl AlphaMode {
    /// True for modes drawn in the sorted transparent pass.
    pub fn is_transparent(self) -> bool {
        matches!(self, Self::Blend | Self::Additive | Self::Multiply)
    }
}

/// How a material's vertices sway in the wind (the renderer's global wind,
/// `Environment::wind`). The displacement is computed in the vertex shader,
/// identically in every pass (main, shadows, depth prepass, selection
/// outline), so shadows and outlines move with the surface. Normals and
/// culling bounds are not changed, so keep the sway small next to the mesh.
/// Off (strength 0) costs nothing.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct WindSway {
    /// Sway in metres per unit of wind strength at full weight. 0 turns
    /// wind off for the material.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Unit("m"), @Step(0.005))]
    pub strength: f32,
    /// Flutter frequency (leaves trembling on top of the slower gusts).
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("Hz"), @Step(0.05))]
    pub frequency: f32,
    /// Where each vertex's weight (0 pinned, 1 free) comes from.
    pub weight: WindWeight,
}

impl Default for WindSway {
    fn default() -> Self {
        Self {
            strength: 0.0,
            frequency: 1.2,
            weight: WindWeight::default(),
        }
    }
}

impl WindSway {
    /// True when the material moves in the wind.
    pub fn is_on(&self) -> bool {
        self.strength > 0.0
    }
}

/// The per-vertex weight of [`WindSway`]: 0 pins a vertex, 1 lets it move
/// fully.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum WindWeight {
    /// Rises smoothly from 0 at `bottom` to 1 at `top` along the mesh's own
    /// (object-space) Y: trees, grass and hanging cloth need no painted data.
    HeightRamp {
        #[reflect(@Unit("m"), @Step(0.01))]
        bottom: f32,
        #[reflect(@Unit("m"), @Step(0.01))]
        top: f32,
    },
    /// The vertex colour's alpha (painted or generated). With this weight
    /// the vertex alpha no longer affects opacity or the alpha cutoff.
    VertexAlpha,
}

impl Default for WindWeight {
    fn default() -> Self {
        Self::HeightRamp {
            bottom: 0.0,
            top: 1.0,
        }
    }
}

/// The built-in lit material: metallic/roughness PBR parameters and optional
/// textures. Vertex colours multiply `base_color`.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct StandardMaterial {
    /// Surface colour (linear). Its alpha is used by the alpha mode.
    pub base_color: Color,
    /// Microsurface roughness: 0 is mirror-like, 1 fully diffuse.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub roughness: f32,
    /// 0 for dielectrics (stone, wood, plastic), 1 for bare metal.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub metallic: f32,
    /// Linear emitted radiance, added after lighting. Black emits nothing.
    pub emissive: Color,
    /// sRGB colour texture, multiplied with `base_color` (alpha too).
    #[reflect(default)]
    pub base_color_texture: AssetRef,
    /// Tangent-space normal map (linear).
    #[reflect(default)]
    pub normal_texture: AssetRef,
    /// Occlusion (R), roughness (G) and metallic (B) in one linear texture,
    /// as in glTF; roughness and metallic multiply the factors above.
    #[reflect(default)]
    pub orm_texture: AssetRef,
    /// sRGB emissive texture, multiplied with `emissive`.
    #[reflect(default)]
    pub emissive_texture: AssetRef,
    /// How alpha is used.
    #[reflect(default)]
    pub alpha_mode: AlphaMode,
    /// Draw both faces (foliage, thin sheets); back faces get flipped normals.
    #[reflect(default)]
    pub double_sided: bool,
    /// Transparent modes (blend, additive, multiply): fade out over this
    /// distance in front of the opaque surface behind, so sheets and puffs
    /// meet geometry softly instead of in a hard line (soft particles). 0 is
    /// off. Needs the depth prepass, which runs while such a material is
    /// visible.
    #[reflect(default, @Range { min: 0.0, max: 50.0 }, @Unit("m"), @Step(0.05))]
    pub depth_fade: f32,
    /// Vertex sway in the wind.
    #[reflect(default)]
    pub wind: WindSway,
    /// Texture repeats per UV unit (all four textures). With UVs in metres
    /// (generated meshes), a texture spanning 2 m is `Vec2::splat(0.5)`.
    #[reflect(default = "uv_scale_one")]
    pub uv_scale: Vec2,
}

fn uv_scale_one() -> Vec2 {
    Vec2::ONE
}

impl Default for StandardMaterial {
    fn default() -> Self {
        Self {
            base_color: Color::linear(0.8, 0.8, 0.8),
            roughness: 0.5,
            metallic: 0.0,
            emissive: Color::BLACK,
            base_color_texture: AssetRef::NONE,
            normal_texture: AssetRef::NONE,
            orm_texture: AssetRef::NONE,
            emissive_texture: AssetRef::NONE,
            alpha_mode: AlphaMode::Opaque,
            double_sided: false,
            depth_fade: 0.0,
            wind: WindSway::default(),
            uv_scale: Vec2::ONE,
        }
    }
}

impl From<Color> for StandardMaterial {
    fn from(color: Color) -> Self {
        Self::color(color)
    }
}

impl StandardMaterial {
    /// The default material with this base colour:
    /// `StandardMaterial::color(Color::hex(0xe8b04a))`.
    pub fn color(base_color: Color) -> Self {
        Self {
            base_color,
            ..Self::default()
        }
    }

    /// The same material with another roughness (0 mirror-like, 1 diffuse).
    pub fn with_roughness(mut self, roughness: f32) -> Self {
        self.roughness = roughness;
        self
    }

    /// The same material with another metallic factor (0 dielectric, 1 metal).
    pub fn with_metallic(mut self, metallic: f32) -> Self {
        self.metallic = metallic;
        self
    }

    /// The same material with an emissive colour (linear radiance).
    pub fn with_emissive(mut self, emissive: Color) -> Self {
        self.emissive = emissive;
        self
    }

    /// Every texture reference, in field order: base colour, normal, ORM,
    /// emissive.
    pub fn textures(&self) -> [&AssetRef; 4] {
        [
            &self.base_color_texture,
            &self.normal_texture,
            &self.orm_texture,
            &self.emissive_texture,
        ]
    }

    /// True when it uses any texture.
    pub fn has_textures(&self) -> bool {
        self.textures().iter().any(|t| t.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflect::Registry;

    #[test]
    fn old_scene_data_without_the_m4_fields_still_loads() {
        let registry = Registry::with_core_types();
        let ty = registry.component("StandardMaterial").unwrap();
        let json = serde_json::json!({
            "base_color": [1.0, 0.0, 0.0, 1.0],
            "roughness": 0.25,
            "metallic": 1.0,
            "emissive": [0.0, 0.0, 0.0, 1.0]
        });
        let value = registry
            .value_from_json(ty.type_id(), &json)
            .expect("M3 material JSON loads");
        let m =
            <StandardMaterial as bevy_reflect::FromReflect>::from_reflect(value.as_ref()).unwrap();
        assert_eq!(m.roughness, 0.25);
        assert!(!m.has_textures());
        assert_eq!(m.alpha_mode, AlphaMode::Opaque);
        assert_eq!((m.depth_fade, m.wind), (0.0, WindSway::default()));
        assert!(!m.wind.is_on());
    }

    #[test]
    fn textured_material_round_trips() {
        let registry = Registry::with_core_types();
        let m = StandardMaterial {
            base_color_texture: AssetRef::path("textures/stone.png"),
            alpha_mode: AlphaMode::Mask { cutoff: 0.4 },
            double_sided: true,
            ..Default::default()
        };
        let json = registry.value_to_json(&m).unwrap();
        assert_eq!(
            json["base_color_texture"],
            serde_json::json!({"path": "textures/stone.png"})
        );
        assert_eq!(json["normal_texture"], serde_json::Value::Null);
        let ty = registry.component("StandardMaterial").unwrap();
        let back = registry.value_from_json(ty.type_id(), &json).unwrap();
        assert_eq!(
            <StandardMaterial as bevy_reflect::FromReflect>::from_reflect(back.as_ref()).unwrap(),
            m
        );
    }
}
