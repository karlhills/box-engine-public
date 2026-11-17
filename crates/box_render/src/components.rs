//! Render components and resources read from the [`World`](box_core::world::World).
//!
//! Scene data derives `Reflect` so the inspector, scene files and the `box` CLI
//! can read and edit it: [`Camera`], [`DirectionalLight`], [`PointLight`],
//! [`SpotLight`], [`AmbientVolume`], [`StandardMaterial`] and
//! [`ShaderMaterial`] are components (so is `ParticleEmitter`, in
//! particles.rs),
//! [`Environment`] is a resource. [`MeshRenderer`] holds asset handles; it is
//! derived runtime data (built from `Primitive` by box_engine's scene glue)
//! and is never reflected or saved. [`RenderSettings`] is a debug switch, not
//! scene data.

use box_core::assets::Handle;
use box_core::color::Color;
pub use box_core::material::{AlphaMode, StandardMaterial, WindSway, WindWeight};
use box_core::mesh::MeshData;
use box_core::reflect::{
    Logarithmic, Range, Reflect, ReflectComponent, ReflectDefault, ReflectResource,
    ReflectVariantDefaults, Step, Unit, VariantDefaults,
};
pub use box_core::shader_material::{ShaderMaterial, ShaderSource};
use glam::{Vec2, Vec3};

/// A perspective camera. Uses the entity's `GlobalTransform` (looks down −Z).
/// The projection is an infinite reverse-Z perspective; the aspect ratio comes
/// from the render target.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Camera {
    /// Vertical field of view in radians.
    #[reflect(@Range { min: 0.02, max: 3.0 }, @Unit("rad"), @Step(0.01))]
    pub fov_y: f32,
    /// Distance to the near clipping plane. There is no far plane.
    #[reflect(@Range { min: 0.001, max: 100.0 }, @Unit("m"), @Step(0.01))]
    pub near: f32,
    /// Whether this camera can render the scene. The renderer uses the first
    /// active camera it finds.
    pub active: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            fov_y: 50f32.to_radians(),
            near: 0.1,
            active: true,
        }
    }
}

/// A material asset of either kind, as a [`MeshRenderer`] slot holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaterialHandle {
    /// In `Assets<StandardMaterial>`.
    Standard(Handle<StandardMaterial>),
    /// In `Assets<ShaderMaterial>`.
    Shader(Handle<ShaderMaterial>),
}

impl MaterialHandle {
    /// The asset id (unique across every asset store).
    pub fn id(&self) -> u64 {
        match self {
            Self::Standard(h) => h.id(),
            Self::Shader(h) => h.id(),
        }
    }
}

impl From<Handle<StandardMaterial>> for MaterialHandle {
    fn from(h: Handle<StandardMaterial>) -> Self {
        Self::Standard(h)
    }
}

impl From<Handle<ShaderMaterial>> for MaterialHandle {
    fn from(h: Handle<ShaderMaterial>) -> Self {
        Self::Shader(h)
    }
}

/// Draws a mesh at the entity's `GlobalTransform`, each material group of
/// the mesh (`MeshData::groups`, or the whole mesh as slot 0 when it has
/// none) with the material in its slot. A slot past the end of `materials`,
/// or a handle whose asset is missing, draws with the default material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshRenderer {
    pub mesh: Handle<MeshData>,
    /// Materials by material group slot.
    pub materials: Vec<MaterialHandle>,
    /// Opaque and alpha-masked materials cast shadows when this is set;
    /// transparent ones never do.
    pub cast_shadows: bool,
}

/// Runtime marker: the entity's [`MeshRenderer`] is not drawn (no main,
/// shadow or outline draw). Not reflected or saved, and not inherited:
/// tools that hide a subtree mark every entity in it. The editor uses it
/// for hierarchy visibility (the eye toggle) in the edit world only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hidden;

impl MeshRenderer {
    /// A shadow-casting mesh renderer with one material (slot 0).
    pub fn new(mesh: Handle<MeshData>, material: impl Into<MaterialHandle>) -> Self {
        Self::with_materials(mesh, vec![material.into()])
    }

    /// A shadow-casting mesh renderer with a material per slot.
    pub fn with_materials(mesh: Handle<MeshData>, materials: Vec<MaterialHandle>) -> Self {
        Self {
            mesh,
            materials,
            cast_shadows: true,
        }
    }

    /// The material in `slot`, if any.
    pub fn material(&self, slot: usize) -> Option<MaterialHandle> {
        self.materials.get(slot).copied()
    }
}

/// The primary directional light (the sun). Its direction is the entity's
/// `GlobalTransform::forward()`. The renderer uses the first one it finds.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct DirectionalLight {
    /// Light colour (linear).
    pub color: Color,
    /// Brightness: a white Lambert surface facing the light reflects `color * intensity`.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Step(0.05))]
    pub intensity: f32,
    /// Whether the light casts shadows.
    pub shadows: bool,
    /// View distance covered by the shadow cascades.
    #[reflect(@Range { min: 1.0, max: 1000.0 }, @Unit("m"), @Step(1.0))]
    pub shadow_distance: f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            color: Color::linear(1.0, 0.95, 0.88),
            intensity: 3.0,
            shadows: true,
            shadow_distance: 60.0,
        }
    }
}

/// A point light at the entity's position, emitting equally in all
/// directions.
///
/// `intensity` is a luminous intensity in the engine's light unit: at
/// distance `d` a surface facing the light receives `intensity / d²`, the
/// same unit as [`DirectionalLight::intensity`] (candela, if the sun's
/// intensity is read as lux). The light fades smoothly to zero at `range`
/// (inverse square with a window), so nothing beyond it is affected or
/// costs shading time. Up to `MAX_LOCAL_LIGHTS` (256) visible point and
/// spot lights are drawn per frame, the nearest and brightest first.
///
/// With `shadows` set it can be the frame's one shadowed local light (see
/// [`LocalShadow`]); a point light's shadow is six views (a cube), so it
/// costs about six times a spot light's.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct PointLight {
    /// Light colour (linear).
    pub color: Color,
    /// Luminous intensity: irradiance at 1 m, falling off with distance squared.
    #[reflect(@Range { min: 0.0, max: 10000.0 }, @Step(0.1))]
    pub intensity: f32,
    /// Distance at which the light has faded to nothing.
    #[reflect(@Range { min: 0.01, max: 1000.0 }, @Unit("m"), @Step(0.1))]
    pub range: f32,
    /// Cast shadows. Only one local light (point or spot) is shadowed per
    /// frame: the most important visible one that asks for it.
    pub shadows: bool,
    /// Shadow map resolution and biases (used while `shadows` is set).
    pub shadow: LocalShadow,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: Color::linear(1.0, 0.9, 0.75),
            intensity: 8.0,
            range: 10.0,
            shadows: false,
            shadow: LocalShadow::default(),
        }
    }
}

/// A spot light at the entity's position, shining down the entity's −Z
/// (`GlobalTransform::forward()`) in a cone.
///
/// `intensity` is in the same unit as [`PointLight::intensity`]: a surface
/// on the axis at distance `d`, facing the light, receives
/// `intensity / d²`, so narrowing the cone does not brighten it. Inside
/// `inner_angle` (from the axis) the light is at full strength; it fades
/// smoothly to nothing at `outer_angle`, and with distance like a point
/// light (to nothing at `range`). Spot and point lights share the counted
/// light buffer (at most 256 visible per frame).
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct SpotLight {
    /// Light colour (linear).
    pub color: Color,
    /// Luminous intensity on the axis: irradiance at 1 m, falling off with
    /// distance squared.
    #[reflect(@Range { min: 0.0, max: 10000.0 }, @Step(0.1))]
    pub intensity: f32,
    /// Distance at which the light has faded to nothing.
    #[reflect(@Range { min: 0.01, max: 1000.0 }, @Unit("m"), @Step(0.1))]
    pub range: f32,
    /// Angle from the axis within which the light is at full strength.
    #[reflect(@Range { min: 0.0, max: 1.5707964 }, @Unit("rad"), @Step(0.01))]
    pub inner_angle: f32,
    /// Angle from the axis at which the light has faded to nothing (at
    /// least `inner_angle`, at most 90°).
    #[reflect(@Range { min: 0.0, max: 1.5707964 }, @Unit("rad"), @Step(0.01))]
    pub outer_angle: f32,
    /// Cast shadows. Only one local light (point or spot) is shadowed per
    /// frame: the most important visible one that asks for it.
    pub shadows: bool,
    /// Shadow map resolution and biases (used while `shadows` is set).
    pub shadow: LocalShadow,
}

impl Default for SpotLight {
    fn default() -> Self {
        Self {
            color: Color::linear(1.0, 0.9, 0.75),
            intensity: 20.0,
            range: 12.0,
            inner_angle: 20f32.to_radians(),
            outer_angle: 30f32.to_radians(),
            shadows: false,
            shadow: LocalShadow::default(),
        }
    }
}

impl SpotLight {
    /// Brightness factor (0..1) of a direction `cos_angle` (cosine of the
    /// angle from the axis): 1 inside `inner_angle`, 0 beyond `outer_angle`,
    /// a smooth square in between. The shader computes the same
    /// (`spot_attenuation` in lighting.wgsl).
    pub fn cone_attenuation(&self, cos_angle: f32) -> f32 {
        let (scale, offset) = spot_scale_offset(self.inner_angle, self.outer_angle);
        let t = (cos_angle * scale + offset).clamp(0.0, 1.0);
        t * t
    }
}

/// `(scale, offset)` with `saturate(cos * scale + offset)` rising from 0
/// at `outer` to 1 at `inner` (angles from the axis, clamped to 0..90° and
/// inner <= outer). A point light is `(0, 1)`.
pub(crate) fn spot_scale_offset(inner: f32, outer: f32) -> (f32, f32) {
    let outer = outer.clamp(0.0, std::f32::consts::FRAC_PI_2);
    let inner = inner.clamp(0.0, outer);
    let (cos_inner, cos_outer) = (inner.cos(), outer.cos());
    let scale = 1.0 / (cos_inner - cos_outer).max(1e-4);
    (scale, -cos_outer * scale)
}

/// Shadow settings of a point or spot light. Only one local light is
/// shadowed per frame: of the visible lights with `shadows` set, the one
/// with the highest importance from the camera (brightness over one plus
/// the squared distance from the camera to its range; the same ranking
/// that picks lights beyond the 256 cap). Its shadow map is
/// `resolution`² texels (six of them for a point light), filtered with
/// 3x3 bilinear PCF for soft edges.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct LocalShadow {
    /// Shadow map width and height in texels (per cube face for a point
    /// light). Changing it reallocates the map.
    #[reflect(@Range { min: 128.0, max: 4096.0 }, @Step(128.0))]
    pub resolution: u32,
    /// Moves the lookup toward the light by this many shadow texels
    /// (removes acne on surfaces facing the light).
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Step(0.05))]
    pub depth_bias: f32,
    /// Moves the lookup off the surface along its normal by this many
    /// shadow texels, more at grazing angles (removes acne on slopes).
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Step(0.05))]
    pub normal_bias: f32,
}

impl Default for LocalShadow {
    fn default() -> Self {
        Self {
            resolution: 1024,
            depth_bias: 1.0,
            normal_bias: 1.5,
        }
    }
}

/// Replaces the sky ambient light inside a box, so interiors get no sky
/// ambient leak. The box is the entity's transform (centre, rotation,
/// scale) times `half_extents`.
///
/// Inside, ambient light is `sky_contribution` of the `Environment`'s sky
/// ambient plus `color * intensity`; outside it is the sky ambient. Over
/// `blend_distance` metres outward from the box the two blend smoothly, so
/// fit the box to the room's inner walls and keep the blend thinner than
/// the walls. Screen-space ambient occlusion darkens the result like any
/// ambient light; direct light (sun, point and spot lights) is not
/// affected (shadows keep the sun out). Overlapping volumes apply in
/// order; at most 8 visible volumes count per frame, the nearest first.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AmbientVolume {
    /// Half size of the box in the entity's space (scaled by its transform).
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"), @Step(0.05))]
    pub half_extents: Vec3,
    /// Ambient light colour inside (linear).
    pub color: Color,
    /// Brightness of `color`, in the unit of `Environment::ambient_intensity`
    /// times the sky colours.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Step(0.005))]
    pub intensity: f32,
    /// Fraction of the sky ambient kept inside: 0 for a closed room, 1 for
    /// none of the override.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub sky_contribution: f32,
    /// Distance outward from the box over which inside and outside blend.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("m"), @Step(0.01))]
    pub blend_distance: f32,
}

impl Default for AmbientVolume {
    fn default() -> Self {
        Self {
            half_extents: Vec3::new(2.0, 1.5, 2.0),
            color: Color::linear(1.0, 0.85, 0.7),
            intensity: 0.04,
            sky_contribution: 0.0,
            blend_distance: 0.1,
        }
    }
}

/// Tone-mapping operator applied after exposure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum Tonemapper {
    /// Analytic AgX approximation (polynomial fit, no LUT).
    #[default]
    AgX,
    /// Stephen Hill's fitted ACES RRT+ODT.
    AcesFitted,
    /// Clamp only.
    None,
}

/// Sky, ambient light, fog, exposure and post-processing (ambient
/// occlusion, bloom, colour grading), atmosphere (haze, mist, quieting),
/// vignette and wind. A scene resource; the default is
/// [`Environment::day`]. Fields missing from a scene file take their
/// default values. Every value is a uniform: animating them (time of day)
/// never rebuilds a pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Resource, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Environment {
    /// Sky colour straight up (linear).
    pub sky_zenith: Color,
    /// Sky colour at the horizon (linear); also tints the fog.
    pub sky_horizon: Color,
    /// Colour below the horizon and of light bounced up from the ground (linear).
    pub ground: Color,
    /// Multiplier for the hemisphere ambient light.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Step(0.01))]
    pub ambient_intensity: f32,
    /// Angular radius of the sun disc in radians.
    #[reflect(@Range { min: 0.0, max: 0.2 }, @Unit("rad"), @Step(0.001))]
    pub sun_disc_size: f32,
    /// Fog density per metre at `fog_base_height`. 0 disables fog.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("1/m"), @Step(0.001), @Logarithmic)]
    pub fog_density: f32,
    /// How quickly fog thins out with height (per metre).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Unit("1/m"), @Step(0.001))]
    pub fog_height_falloff: f32,
    /// Height at which fog has its full density.
    #[reflect(@Unit("m"), @Step(0.1))]
    pub fog_base_height: f32,
    /// Exposure multiplier applied before tone mapping.
    #[reflect(@Range { min: 0.0, max: 16.0 }, @Step(0.01))]
    pub exposure: f32,
    /// Tone-mapping operator.
    pub tonemapper: Tonemapper,
    /// Screen-space ambient occlusion: darkens ambient light in creases,
    /// corners and where objects meet.
    pub ambient_occlusion: AmbientOcclusion,
    /// Soft glow around bright areas.
    pub bloom: Bloom,
    /// White balance, contrast, saturation, shadows/midtones/highlights and
    /// lift/gamma/gain.
    pub color_grading: ColorGrading,
    /// Sky gradient style, sun glow and disc, horizon glow, stars.
    pub sky: Sky,
    /// Distance haze toward a colour, from a start distance, up to a maximum
    /// opacity (aerial perspective).
    pub aerial_haze: AerialHaze,
    /// Mist pooling in a height band (a valley or cloud sea).
    pub height_mist: HeightMist,
    /// Desaturation and contrast loss with distance, so the background
    /// stays quiet behind the foreground.
    pub distance_quieting: DistanceQuieting,
    /// Darkened image corners.
    pub vignette: Vignette,
    /// The global wind that materials with `wind` sway in.
    pub wind: Wind,
}

/// How the sky is drawn. Every value is a uniform: animating them (time of
/// day) never rebuilds a pipeline. The default reproduces the RS1 sky.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Sky {
    /// The colour gradient from the horizon to the zenith.
    pub gradient: SkyGradient,
    /// Strength of the sun-coloured glow around the sun (also tints the fog
    /// toward the sun); 1 is the default look, 0 none.
    #[reflect(@Range { min: 0.0, max: 4.0 }, @Step(0.01))]
    pub sun_glow: f32,
    /// Brightness of the sun disc; 1 is the default, 0 hides it.
    #[reflect(@Range { min: 0.0, max: 8.0 }, @Step(0.01))]
    pub sun_disc: f32,
    /// A warm band along the horizon, strongest toward the sun's azimuth.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.01))]
    pub horizon_glow: f32,
    /// Brightness of the star field (0 none). Raise it at night.
    #[reflect(@Range { min: 0.0, max: 4.0 }, @Step(0.01))]
    pub stars: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            gradient: SkyGradient::Simple,
            sun_glow: 1.0,
            sun_disc: 1.0,
            horizon_glow: 0.0,
            stars: 0.0,
        }
    }
}

/// The sky's colour gradient.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq, VariantDefaults)]
#[type_path = "box_engine"]
pub enum SkyGradient {
    /// `sky_horizon` to `sky_zenith` above the horizon, fading slowly to
    /// `ground` below it.
    #[default]
    Simple,
    /// `sky_horizon` → `upper` (by a fifth of the way up) → `sky_zenith`,
    /// with soft curves; below the horizon it fades quickly to
    /// `below_horizon` (a haze or cloud-sea colour) so there is no seam.
    Layered {
        /// Colour of the upper sky, between the horizon and the zenith
        /// (linear).
        upper: Color,
        /// Colour just below the horizon (linear).
        below_horizon: Color,
    },
}

/// `Layered` in the default day sky's blues (not black): an upper sky
/// between the default horizon and zenith, the horizon colour below.
impl VariantDefaults for SkyGradient {
    fn variant_default(variant: &str) -> Option<Self> {
        (variant == "Layered").then(|| Self::Layered {
            upper: Color::linear(0.25, 0.42, 0.75),
            below_horizon: Color::linear(0.55, 0.68, 0.85),
        })
    }
}

/// Aerial perspective: surfaces fade toward `color` with distance,
/// `1 - exp(-density * (distance - start_distance))`, at most
/// `max_opacity`. Applied to opaque and transparent surfaces and to the sky
/// near the horizon (up to `sky_height`), so distant geometry and the
/// horizon meet without a seam. Off by default.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AerialHaze {
    pub enabled: bool,
    /// Haze colour (linear).
    pub color: Color,
    /// Density per metre beyond `start_distance`.
    #[reflect(@Range { min: 0.0, max: 0.1 }, @Unit("1/m"), @Step(0.0001), @Logarithmic)]
    pub density: f32,
    /// No haze closer than this.
    #[reflect(@Range { min: 0.0, max: 5000.0 }, @Unit("m"), @Step(1.0))]
    pub start_distance: f32,
    /// The most the haze ever covers (1 hides the farthest surfaces).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub max_opacity: f32,
    /// The haze brightens toward the sun by this much of the sun's hue.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub sun_scatter: f32,
    /// How far up the sky the haze (and distance quieting) reaches, as the
    /// sine of the elevation: the sky below it counts as far scenery. 0
    /// leaves the sky alone.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.005))]
    pub sky_height: f32,
}

impl Default for AerialHaze {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Color::linear(0.37, 0.51, 0.71),
            density: 0.0005,
            start_distance: 60.0,
            max_opacity: 0.97,
            sun_scatter: 0.12,
            sky_height: 0.12,
        }
    }
}

/// Mist pooling in a height band: none above `top`, rising smoothly to
/// `max_opacity` at `bottom` and below. It depends on the surface's height
/// (not on the path through the band), fades in over `near_fade` from the
/// camera, and covers the sky below the horizon when the band lies below.
/// Off by default.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct HeightMist {
    pub enabled: bool,
    /// Mist colour (linear).
    pub color: Color,
    /// Height of the top of the band (no mist above).
    #[reflect(@Unit("m"), @Step(0.5))]
    pub top: f32,
    /// Height of the bottom of the band (full mist below).
    #[reflect(@Unit("m"), @Step(0.5))]
    pub bottom: f32,
    /// Opacity of the mist at the bottom of the band.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub max_opacity: f32,
    /// Distance over which the mist fades in from the camera, so the camera
    /// can stand in it.
    #[reflect(@Range { min: 0.0, max: 2000.0 }, @Unit("m"), @Step(1.0))]
    pub near_fade: f32,
}

impl Default for HeightMist {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Color::linear(0.53, 0.6, 0.74),
            top: -20.0,
            bottom: -60.0,
            max_opacity: 1.0,
            near_fade: 120.0,
        }
    }
}

/// Background quieting: between `start` and `end` (metres from the camera)
/// surfaces lose `desaturate` of their saturation and move `contrast` of
/// the way toward `tone`, so the background reads quieter than the
/// foreground. Off by default.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct DistanceQuieting {
    pub enabled: bool,
    /// Distance where quieting starts.
    #[reflect(@Range { min: 0.0, max: 10000.0 }, @Unit("m"), @Step(1.0))]
    pub start: f32,
    /// Distance where it is complete.
    #[reflect(@Range { min: 0.0, max: 10000.0 }, @Unit("m"), @Step(1.0))]
    pub end: f32,
    /// Saturation removed at `end` (1 is grey).
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub desaturate: f32,
    /// Fraction of the way toward `tone` at `end`: compresses contrast.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub contrast: f32,
    /// Colour that distant surfaces' contrast is compressed toward
    /// (linear), usually a slightly darker haze colour.
    pub tone: Color,
}

impl Default for DistanceQuieting {
    fn default() -> Self {
        Self {
            enabled: false,
            start: 90.0,
            end: 1800.0,
            desaturate: 0.5,
            contrast: 0.38,
            tone: Color::linear(0.3, 0.42, 0.58),
        }
    }
}

/// Darkens the image toward its corners (applied to the HDR image before
/// tone mapping, like Unity's URP vignette). Off while `intensity` is 0.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Vignette {
    /// How dark the corners get; 0 is off.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub intensity: f32,
    /// Width of the falloff toward the centre.
    #[reflect(@Range { min: 0.01, max: 1.0 }, @Step(0.01))]
    pub smoothness: f32,
    /// A circle (on any aspect ratio) instead of an ellipse fitted to the
    /// image.
    pub rounded: bool,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            intensity: 0.0,
            smoothness: 0.5,
            rounded: false,
        }
    }
}

/// The global wind. Materials with `WindSway` sway by `strength` times their
/// own strength and per-vertex weight: a lean downwind that swells with gust
/// fronts rolling along `direction` every `gust_length` metres at
/// `gust_speed`, plus a flutter at each material's frequency. Driven by
/// `Time::elapsed`; every value is a uniform, so animating it (gusts,
/// storms) costs nothing.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Wind {
    /// Horizontal direction the wind blows toward, (x, z); normalised when
    /// used.
    pub direction: Vec2,
    /// Overall strength; 0 is calm.
    #[reflect(@Range { min: 0.0, max: 4.0 }, @Step(0.01))]
    pub strength: f32,
    /// Distance between gust fronts.
    #[reflect(@Range { min: 0.5, max: 200.0 }, @Unit("m"), @Step(0.1))]
    pub gust_length: f32,
    /// Speed of the gust fronts downwind.
    #[reflect(@Range { min: 0.0, max: 30.0 }, @Unit("m/s"), @Step(0.1))]
    pub gust_speed: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            direction: Vec2::new(1.0, 0.0),
            strength: 1.0,
            gust_length: 17.0,
            gust_speed: 3.0,
        }
    }
}

/// Screen-space ambient occlusion (SSAO), computed at half resolution from a
/// depth and normal prepass and upsampled per pixel with depth awareness.
/// It darkens ambient (sky) light only, unless `direct_lighting` is raised.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct AmbientOcclusion {
    /// Compute ambient occlusion. Off skips the prepass and the occlusion passes.
    pub enabled: bool,
    /// How dark occluded areas get.
    #[reflect(@Range { min: 0.0, max: 4.0 }, @Step(0.01))]
    pub intensity: f32,
    /// Distance within which nearby geometry occludes a point.
    #[reflect(@Range { min: 0.05, max: 5.0 }, @Unit("m"), @Step(0.01))]
    pub radius: f32,
    /// Ignores occluders this close to a surface's tangent plane, so flat and
    /// gently curved surfaces do not darken themselves.
    #[reflect(@Range { min: 0.0, max: 0.5 }, @Unit("m"), @Step(0.001))]
    pub bias: f32,
    /// Contrast of the occlusion: values above 1 darken creases more
    /// sharply.
    #[reflect(@Range { min: 0.25, max: 4.0 }, @Step(0.01))]
    pub power: f32,
    /// How much of the occlusion also darkens direct light (sun and point
    /// lights). 0 affects ambient light only.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub direct_lighting: f32,
}

impl Default for AmbientOcclusion {
    fn default() -> Self {
        Self {
            enabled: true,
            intensity: 1.5,
            radius: 1.0,
            bias: 0.02,
            power: 1.2,
            direct_lighting: 0.0,
        }
    }
}

/// Bloom: a blurred copy of the image (a chain of downsampled mips, added
/// back up) blended over it before tone mapping. Energy-conserving: the glow
/// redistributes light rather than adding it, so there is no threshold.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Bloom {
    /// Draw bloom. Off skips its passes.
    pub enabled: bool,
    /// Fraction of the image replaced by its blurred copy.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.005))]
    pub intensity: f32,
    /// How far the glow spreads: 0 keeps it tight, 1 makes it wide.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub scatter: f32,
}

impl Default for Bloom {
    fn default() -> Self {
        Self {
            enabled: true,
            intensity: 0.025,
            scatter: 0.7,
        }
    }
}

/// Colour grading. White balance, contrast, shadows/midtones/highlights and
/// saturation act on the exposed scene before tone mapping; lift, gamma and
/// gain on the tone-mapped (display) image. The default changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ColorGrading {
    /// White balance: positive warms (toward orange), negative cools (toward blue).
    #[reflect(@Range { min: -1.0, max: 1.0 }, @Step(0.01))]
    pub temperature: f32,
    /// White balance: positive toward magenta, negative toward green.
    #[reflect(@Range { min: -1.0, max: 1.0 }, @Step(0.01))]
    pub tint: f32,
    /// Contrast around middle grey (in log space); 1 leaves it unchanged.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.01))]
    pub contrast: f32,
    /// Colour saturation: 0 is grey, 1 unchanged.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.01))]
    pub saturation: f32,
    /// Shadows tint: multiplies scene-linear colour where the luminance is
    /// below `shadows_end` (fading out toward it); 1 unchanged. Unity's
    /// shadows/midtones/highlights values are gamma encoded: use value^2.2.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.005))]
    pub shadows: Vec3,
    /// Midtones tint: multiplies colour between the shadows and highlights.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.005))]
    pub midtones: Vec3,
    /// Highlights tint: multiplies colour above `highlights_start` (fading
    /// in up to luminance 1).
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.005))]
    pub highlights: Vec3,
    /// Luminance (exposed scene-linear) where the shadows tint has faded out.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub shadows_end: f32,
    /// Luminance where the highlights tint starts.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub highlights_start: f32,
    /// Shadows: added per channel, fading out toward white; 0 unchanged.
    #[reflect(@Range { min: -0.5, max: 0.5 }, @Step(0.005))]
    pub lift: Vec3,
    /// Midtones: a per-channel gamma; above 1 brightens, 1 unchanged.
    #[reflect(@Range { min: 0.2, max: 5.0 }, @Step(0.01))]
    pub gamma: Vec3,
    /// Highlights: multiplied per channel; 1 unchanged.
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Step(0.01))]
    pub gain: Vec3,
}

impl Default for ColorGrading {
    fn default() -> Self {
        Self {
            temperature: 0.0,
            tint: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            shadows: Vec3::ONE,
            midtones: Vec3::ONE,
            highlights: Vec3::ONE,
            shadows_end: 0.3,
            highlights_start: 0.55,
            lift: Vec3::ZERO,
            gamma: Vec3::ONE,
            gain: Vec3::ONE,
        }
    }
}

impl ColorGrading {
    /// True when the grading changes nothing (the passes skip it).
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::day()
    }
}

impl Environment {
    /// Clear midday sky.
    pub fn day() -> Self {
        Self {
            sky_zenith: Color::linear(0.09, 0.24, 0.66),
            sky_horizon: Color::linear(0.55, 0.68, 0.85),
            ground: Color::linear(0.18, 0.16, 0.13),
            ambient_intensity: 1.0,
            sun_disc_size: 0.6f32.to_radians(),
            fog_density: 0.006,
            fog_height_falloff: 0.08,
            fog_base_height: 0.0,
            exposure: 0.55,
            tonemapper: Tonemapper::AgX,
            ambient_occlusion: AmbientOcclusion::default(),
            bloom: Bloom::default(),
            color_grading: ColorGrading::default(),
            sky: Sky::default(),
            aerial_haze: AerialHaze::default(),
            height_mist: HeightMist::default(),
            distance_quieting: DistanceQuieting::default(),
            vignette: Vignette::default(),
            wind: Wind::default(),
        }
    }

    /// Warm, hazy late-afternoon look. Pair it with a low, orange
    /// [`DirectionalLight`] (the light is an entity the game moves).
    pub fn evening() -> Self {
        Self {
            sky_zenith: Color::linear(0.16, 0.20, 0.38),
            sky_horizon: Color::linear(0.72, 0.46, 0.30),
            ground: Color::linear(0.14, 0.11, 0.09),
            ambient_intensity: 0.8,
            sun_disc_size: 0.9f32.to_radians(),
            fog_density: 0.007,
            fog_height_falloff: 0.06,
            fog_base_height: 0.0,
            exposure: 0.7,
            tonemapper: Tonemapper::AgX,
            ambient_occlusion: AmbientOcclusion::default(),
            bloom: Bloom::default(),
            color_grading: ColorGrading::default(),
            sky: Sky::default(),
            aerial_haze: AerialHaze::default(),
            height_mist: HeightMist::default(),
            distance_quieting: DistanceQuieting::default(),
            vignette: Vignette::default(),
            wind: Wind::default(),
        }
    }
}

/// Screen-space outline around the silhouettes of selected entities, drawn
/// after tone mapping in display colour (like display-referred lines), so it
/// looks the same at any exposure. A resource; tools (the editor) set it
/// every frame. Costs nothing while both lists are empty.
///
/// Entities need a [`MeshRenderer`]; children are not included implicitly.
/// Overlapping selected entities merge into one silhouette. The parts of the
/// outline whose silhouette is hidden behind other geometry are drawn with
/// `occluded_alpha`, so a selection stays findable behind walls.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionOutline {
    /// Entities outlined in `color` (the primary selection).
    pub primary: Vec<box_core::world::Entity>,
    /// Entities outlined in `secondary_color`.
    pub secondary: Vec<box_core::world::Entity>,
    /// Linear colour with alpha (`Color::srgb` converts from sRGB).
    pub color: Color,
    pub secondary_color: Color,
    /// Outline width in physical pixels, clamped to `0.5..=4`.
    pub width_px: f32,
    /// Alpha multiplier for outline parts whose silhouette is occluded.
    pub occluded_alpha: f32,
}

impl Default for SelectionOutline {
    fn default() -> Self {
        Self {
            primary: Vec::new(),
            secondary: Vec::new(),
            color: Color::hex(0x4b8bf5),
            secondary_color: Color::hex(0x4b8bf5).with_alpha(0.55),
            width_px: 2.0,
            occluded_alpha: 0.3,
        }
    }
}

impl SelectionOutline {
    pub fn is_empty(&self) -> bool {
        self.primary.is_empty() && self.secondary.is_empty()
    }
}

/// Debug switches for the renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderSettings {
    /// Tint surfaces by shadow cascade (red, green, blue, yellow).
    pub show_cascades: bool,
    /// Show the ambient occlusion term (white unoccluded, black occluded)
    /// instead of lit surfaces. Needs `Environment::ambient_occlusion`.
    pub show_ambient_occlusion: bool,
}
