//! Glue between scene data and the renderer.
//!
//! - [`engine_registry`]: the one place a game, the editor or the CLI gets the
//!   reflection registry (box_core types plus render types).
//! - [`sync_scene_meshes`]: the change-driven system that keeps renderers in
//!   step with scene data: asset-backed meshes and prefabs
//!   ([`crate::asset_glue`]), rules, the generator runtime (every generator,
//!   `Primitive` included) and the renderers of generated meshes
//!   ([`crate::proc_glue`]). [`default_app`](crate::default_app) runs it at
//!   the start of `PostUpdate`, after transform propagation; a tool with its
//!   own loop calls it before rendering.
//! - [`scene_bounds`] and [`framing_transform`]: frame a scene with a camera.

use std::sync::{Arc, OnceLock};

use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::material::AlphaMode;
use box_core::math::Aabb;
use box_core::mesh::MeshData;
use box_core::reflect::Registry;
use box_core::shader_material::ShaderParam;
use box_core::transform::{GlobalTransform, Transform};
use box_core::world::World;
use box_render::{
    AmbientVolume, Camera, DirectionalLight, Environment, MaterialHandle, MeshRenderer,
    ParticleEmitter, PointLight, ShaderMaterial, SpotLight, StandardMaterial,
};
use glam::Vec3;

/// box_core's scene types (including the `StandardMaterial` and
/// `ShaderMaterial` components) plus the render types: `Camera`,
/// `DirectionalLight`, `PointLight`, `SpotLight`, `AmbientVolume` and
/// `ParticleEmitter` components and the `Environment` resource, and box_proc's `Procedural`; with the `physics` feature, also
/// `Collider`; with the `audio` feature, `AudioSource`, `AudioListener` and
/// the `AudioSettings` settings resource; with the `script` feature,
/// `Script`. Games add their generators with
/// `registry.register_generator::<T>()` in `Plugin::register`; the built-in
/// generators (`Primitive`, `Lathe`, `Extrude`) are registered here.
/// `MeshRenderer` is derived and is not registered.
pub fn engine_registry() -> Registry {
    let mut registry = Registry::with_core_types();
    register_render_types(&mut registry);
    box_proc::register_proc_types(&mut registry);
    #[cfg(feature = "physics")]
    box_physics::register_physics_types(&mut registry);
    #[cfg(feature = "audio")]
    box_audio::register_audio_types(&mut registry);
    #[cfg(feature = "script")]
    box_script::register_script_types(&mut registry);
    registry
}

/// [`engine_registry`] with only the optional modules named in `features`
/// (`"physics"`, `"dynamics"`, `"audio"`, `"script"`; see
/// [`crate::modules`]) among those this build includes: the registry a
/// lean native game runs with, for tools that link every module (`box
/// check`, `box mcp`).
pub fn engine_registry_for(features: &[&str]) -> Registry {
    let mut registry = Registry::with_core_types();
    register_render_types(&mut registry);
    box_proc::register_proc_types(&mut registry);
    #[cfg(feature = "physics")]
    if features.contains(&"physics") {
        if features.contains(&"dynamics") {
            box_physics::register_physics_types(&mut registry);
        } else {
            box_physics::register_query_types(&mut registry);
        }
    }
    #[cfg(feature = "audio")]
    if features.contains(&"audio") {
        box_audio::register_audio_types(&mut registry);
    }
    #[cfg(feature = "script")]
    if features.contains(&"script") {
        box_script::register_script_types(&mut registry);
    }
    let _ = features;
    registry
}

/// [`engine_registry`] built once per process and shared, for worlds that
/// should all use the same registry (edit and play worlds, loaded scenes).
pub fn shared_engine_registry() -> Arc<Registry> {
    static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(engine_registry())).clone()
}

/// Register the render crate's scene types into `registry`.
pub fn register_render_types(registry: &mut Registry) {
    // The short names are unique among engine types, so this cannot fail
    // unless a game registered a different type under one of these names.
    let results = [
        registry.register::<Camera>(),
        registry.register::<DirectionalLight>(),
        registry.register::<PointLight>(),
        registry.register::<SpotLight>(),
        registry.register::<AmbientVolume>(),
        registry.register::<ParticleEmitter>(),
        registry.register_resource::<Environment>(),
    ];
    for r in results {
        if let Err(e) = r {
            tracing::error!("render type registration failed: {e}");
        }
    }
}

/// A `MaterialRef` as last resolved. Resolved again when the reference
/// changes or the world's asset lookup or finished imports change (a load
/// finished, a file was renamed or changed kind), never per frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    pub(crate) source: AssetRef,
    /// `(AssetLookup::generation, AssetServer::finished_imports)` when it
    /// was resolved.
    pub(crate) stamp: (u64, u64),
    pub(crate) state: LinkState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LinkState {
    /// The asset's handle (not owned: the asset server's).
    Ready(MaterialHandle),
    /// Loading: the inline material is drawn meanwhile.
    Pending,
    /// Cannot be loaded: the placeholder material is drawn.
    Missing,
}

/// Resource: the placeholder material drawn for links that cannot be
/// loaded (owned by the sync, shared by every such entity).
struct LinkPlaceholder(Handle<StandardMaterial>);

/// The material an entity draws with: the linked asset when it is ready, the placeholder when it cannot be loaded, else the asset built
/// from its inline material.
pub(crate) fn drawn_material(
    owned: MaterialHandle,
    link: Option<&Link>,
    placeholder: Option<Handle<StandardMaterial>>,
) -> MaterialHandle {
    match link.map(|l| l.state) {
        Some(LinkState::Ready(h)) => h,
        Some(LinkState::Missing) => placeholder.map_or(owned, MaterialHandle::Standard),
        Some(LinkState::Pending) | None => owned,
    }
}

/// The current resolve stamp of `world` (see [`Link::stamp`]).
pub(crate) fn link_stamp(world: &World) -> (u64, u64) {
    let lookup = world
        .get_resource::<box_core::assets::AssetLookup>()
        .map_or(0, box_core::assets::AssetLookup::generation);
    let finished = crate::asset_glue::loader(world)
        .as_ref()
        .map_or(0, box_assets::AssetServer::finished_imports);
    (lookup, finished)
}

/// Resolves a `MaterialRef` through the world's asset lookup and server
/// (either material kind), logging once when it becomes unloadable.
pub(crate) fn resolve_link(
    world: &World,
    source: &AssetRef,
    stamp: (u64, u64),
    old: Option<&Link>,
) -> Link {
    use crate::asset_glue::{Resolved, loader, resolve_material};
    let server = loader(world);
    let state = match resolve_material(world, server.as_ref(), source) {
        Resolved::Ready(h) => LinkState::Ready(h),
        Resolved::Pending => LinkState::Pending,
        Resolved::Missing(message) => {
            let was_missing =
                old.is_some_and(|l| l.source == *source && l.state == LinkState::Missing);
            if !was_missing {
                tracing::error!("material {source}: {message}; drawing the placeholder");
            }
            LinkState::Missing
        }
    };
    Link {
        source: source.clone(),
        stamp,
        state,
    }
}

/// The material component an entity's material asset was built from.
#[derive(Clone, Debug)]
pub(crate) enum SceneMaterial {
    /// A `StandardMaterial` component, or none (the default material).
    Standard(Option<Box<StandardMaterial>>),
    /// A `ShaderMaterial` component (it wins over a `StandardMaterial`).
    Shader(ShaderMaterial),
}

impl SceneMaterial {
    pub(crate) fn of(standard: Option<&StandardMaterial>, shader: Option<&ShaderMaterial>) -> Self {
        match shader {
            Some(m) => Self::Shader(m.clone()),
            None => Self::Standard(standard.cloned().map(Box::new)),
        }
    }

    /// Whether the entity's components still describe this material,
    /// without cloning them.
    pub(crate) fn matches(
        &self,
        standard: Option<&StandardMaterial>,
        shader: Option<&ShaderMaterial>,
    ) -> bool {
        match (self, shader) {
            (Self::Shader(a), Some(b)) => same_shader_material(a, b),
            (Self::Standard(cached), None) => match (cached, standard) {
                (None, None) => true,
                (Some(a), Some(b)) => same_material(a, b),
                _ => false,
            },
            _ => false,
        }
    }
}

/// Keep every entity's renderer in step with its scene data. Runs
/// [`sync_assets`](crate::asset_glue::sync_assets) (prefabs, `MeshAsset`
/// renderers, texture requests; cheap when nothing changed), then the
/// world's rules ([`box_proc::rule::run_rules`], when it has a
/// `RuleRegistry`), the generator runtime ([`box_proc::flush`]: stamp,
/// throttled regeneration) and the renderers of generated meshes
/// ([`crate::proc_glue`]), so a tool that calls only this function before
/// rendering gets assets and procedural objects too.
///
/// `Primitive` is one of those generators (its `Generator` impl is in
/// `box_proc::builtin`): a new primitive gets its mesh in the frame it
/// appears, a shape edit regenerates it in place (same handle; during a
/// drag at most once per throttle interval), and its material follows the
/// precedence described in [`crate::proc_glue`] (`MaterialRef`, else an
/// inline `ShaderMaterial`, else an inline `StandardMaterial`, else the
/// default). Removing the `Primitive` or despawning the entity frees the
/// mesh and the material asset built for it.
pub fn sync_scene_meshes(world: &mut World) {
    crate::asset_glue::sync_assets(world);
    // Rules first, so parts they spawn or change generate this frame. A
    // no-op without a `RuleRegistry`, and cheap when no intent changed (the
    // app's own rules system then finds nothing to do).
    box_proc::rule::run_rules(world);
    box_proc::flush(world);
    crate::proc_glue::sync_procedural_renderers(world);
    // After generators and their renderers, so a generated ground is current.
    crate::scatter_glue::sync_scatter(world);
}

pub(crate) fn add_material(world: &mut World, material: &SceneMaterial) -> MaterialHandle {
    match material {
        SceneMaterial::Standard(m) => world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(m.as_deref().cloned().unwrap_or_default())
            .into(),
        SceneMaterial::Shader(m) => world
            .resource_mut::<Assets<ShaderMaterial>>()
            .add(m.clone())
            .into(),
    }
}

pub(crate) fn remove_material(world: &mut World, handle: MaterialHandle) {
    match handle {
        MaterialHandle::Standard(h) => {
            world.resource_mut::<Assets<StandardMaterial>>().remove(h);
        }
        MaterialHandle::Shader(h) => {
            world.resource_mut::<Assets<ShaderMaterial>>().remove(h);
        }
    }
}

pub(crate) fn material_exists(world: &World, handle: MaterialHandle) -> bool {
    match handle {
        MaterialHandle::Standard(h) => world
            .resource::<Assets<StandardMaterial>>()
            .get(h)
            .is_some(),
        MaterialHandle::Shader(h) => world.resource::<Assets<ShaderMaterial>>().get(h).is_some(),
    }
}

/// Writes `material` into the asset behind `handle` (same handle, version
/// bumped) when it is of the same kind; otherwise replaces the asset with one
/// in the right store.
pub(crate) fn write_material(
    world: &mut World,
    handle: MaterialHandle,
    material: &SceneMaterial,
) -> MaterialHandle {
    match (handle, material) {
        (MaterialHandle::Standard(h), SceneMaterial::Standard(m)) => {
            if let Some(slot) = world.resource_mut::<Assets<StandardMaterial>>().get_mut(h) {
                *slot = m.as_deref().cloned().unwrap_or_default();
            }
            handle
        }
        (MaterialHandle::Shader(h), SceneMaterial::Shader(m)) => {
            if let Some(slot) = world.resource_mut::<Assets<ShaderMaterial>>().get_mut(h) {
                *slot = m.clone();
            }
            handle
        }
        _ => {
            remove_material(world, handle);
            add_material(world, material)
        }
    }
}

/// Equal, or the same bit patterns. Float `==` alone says a NaN field differs
/// from its own cached copy, which would rewrite the material every frame.
fn same_shader_material(a: &ShaderMaterial, b: &ShaderMaterial) -> bool {
    fn param_bits(
        p: &ShaderParam,
    ) -> (
        &str,
        std::mem::Discriminant<box_core::shader_material::ShaderParamValue>,
        [u32; 4],
    ) {
        (
            p.name.as_str(),
            std::mem::discriminant(&p.value),
            p.value.words(),
        )
    }
    // Destructured in full, so a new field cannot be forgotten here.
    let ShaderMaterial {
        shader,
        params,
        alpha_mode,
        double_sided,
        depth_fade,
        sort_bias,
        depth_write,
        depth_test,
        prewrite_depth,
        scene_depth,
        bounds_padding,
    } = a;
    if a == b {
        return true;
    }
    *shader == b.shader
        && *double_sided == b.double_sided
        && depth_fade.to_bits() == b.depth_fade.to_bits()
        && (
            *sort_bias,
            *depth_write,
            *depth_test,
            *prewrite_depth,
            *scene_depth,
        ) == (
            b.sort_bias,
            b.depth_write,
            b.depth_test,
            b.prewrite_depth,
            b.scene_depth,
        )
        && bounds_padding.to_bits() == b.bounds_padding.to_bits()
        && same_alpha(*alpha_mode, b.alpha_mode)
        && params.len() == b.params.len()
        && params
            .iter()
            .zip(&b.params)
            .all(|(x, y)| param_bits(x) == param_bits(y))
}

fn same_alpha(a: AlphaMode, b: AlphaMode) -> bool {
    match (a, b) {
        (AlphaMode::Mask { cutoff: x }, AlphaMode::Mask { cutoff: y }) => {
            x.to_bits() == y.to_bits()
        }
        _ => a == b,
    }
}

/// See [`same_shader_material`].
fn same_material(a: &StandardMaterial, b: &StandardMaterial) -> bool {
    // Destructured in full, so a new field cannot be forgotten here.
    type State = (bool, i32, box_core::material::DepthWrite, bool, bool);
    fn split(m: &StandardMaterial) -> ([u32; 18], [&AssetRef; 4], AlphaMode, State) {
        let StandardMaterial {
            base_color: c,
            roughness,
            metallic,
            emissive: e,
            base_color_texture,
            normal_texture,
            orm_texture,
            emissive_texture,
            alpha_mode,
            double_sided,
            depth_fade,
            wind,
            uv_scale,
            sort_bias,
            depth_write,
            depth_test,
            prewrite_depth,
        } = m;
        let (weight, bottom, top) = match wind.weight {
            box_core::material::WindWeight::HeightRamp { bottom, top } => (0.0, bottom, top),
            box_core::material::WindWeight::VertexAlpha => (1.0, 0.0, 0.0),
        };
        (
            [
                c.r,
                c.g,
                c.b,
                c.a,
                *roughness,
                *metallic,
                e.r,
                e.g,
                e.b,
                e.a,
                *depth_fade,
                wind.strength,
                wind.frequency,
                weight,
                bottom,
                top,
                uv_scale.x,
                uv_scale.y,
            ]
            .map(f32::to_bits),
            [
                base_color_texture,
                normal_texture,
                orm_texture,
                emissive_texture,
            ],
            *alpha_mode,
            (
                *double_sided,
                *sort_bias,
                *depth_write,
                *depth_test,
                *prewrite_depth,
            ),
        )
    }
    if a == b {
        return true;
    }
    let (ab, at, aa, ad) = split(a);
    let (bb, bt, ba, bd) = split(b);
    ab == bb && at == bt && same_alpha(aa, ba) && ad == bd
}

/// The placeholder material for unloadable links, if it was created.
pub(crate) fn existing_placeholder(world: &World) -> Option<Handle<StandardMaterial>> {
    world.get_resource::<LinkPlaceholder>().map(|p| p.0)
}

/// The placeholder material for unloadable links, created on first use.
pub(crate) fn link_placeholder(world: &mut World) -> Handle<StandardMaterial> {
    if let Some(p) = world.get_resource::<LinkPlaceholder>() {
        return p.0;
    }
    let h = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(box_assets::placeholder::placeholder_material());
    world.insert_resource(LinkPlaceholder(h));
    h
}

/// World-space bounds of every [`MeshRenderer`] whose mesh is in
/// `Assets<MeshData>`, using `GlobalTransform` (run transform propagation and
/// [`sync_scene_meshes`] first). `None` when there is nothing to frame.
pub fn scene_bounds(world: &World) -> Option<Aabb> {
    let meshes = world.get_resource::<Assets<MeshData>>()?;
    let mut bounds: Option<Aabb> = None;
    for (gt, mr) in world.query::<(&GlobalTransform, &MeshRenderer)>().iter() {
        let Some(mesh) = meshes.get(mr.mesh) else {
            continue;
        };
        if mesh.positions.is_empty() {
            continue;
        }
        let b = mesh.bounds().transformed(&gt.0);
        bounds = Some(match bounds {
            Some(acc) => acc.including(b.min).including(b.max),
            None => b,
        });
    }
    bounds
}

/// A camera transform that shows all of `bounds` from above and to the front
/// right, for a camera with vertical field of view `fov_y` and the given
/// aspect ratio (width / height).
pub fn framing_transform(bounds: &Aabb, fov_y: f32, aspect: f32) -> Transform {
    let center = bounds.center();
    let radius = bounds.half_extents().length().max(0.01);
    // The narrower of the two fields of view must hold the bounding sphere.
    let half_v = (fov_y * 0.5).clamp(0.01, 1.5);
    let half_h = (half_v.tan() * aspect.max(0.01)).atan();
    let half = half_v.min(half_h);
    let distance = radius / half.sin() * 1.05;
    let direction = Vec3::new(0.55, 0.45, 0.7).normalize();
    Transform::from_translation(center + direction * distance).looking_at(center, Vec3::Y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::color::Color;
    use box_core::identity::Uid;
    use box_core::mesh_asset::MaterialRef;
    use box_core::primitive::{Primitive, Shape};
    use box_core::world::Entity;
    use glam::Vec2;

    /// Every engine enum's `VariantDefaults` names its own variant and stays
    /// inside the fields' ranges.
    #[test]
    fn engine_variant_defaults_are_consistent() {
        let registry = engine_registry();
        assert_eq!(registry.variant_default_problems(), Vec::<String>::new());
        let with_defaults = registry
            .type_registry()
            .iter()
            .filter(|r| {
                r.data::<box_core::reflect::ReflectVariantDefaults>()
                    .is_some()
            })
            .map(|r| r.type_info().type_path_table().short_path())
            .collect::<std::collections::BTreeSet<_>>();
        for name in [
            "Shape",
            "WindWeight",
            "SkyGradient",
            "SweepProfile",
            "LatheShape",
            "Outline",
            "Hole",
        ] {
            assert!(with_defaults.contains(name), "{name}: {with_defaults:?}");
        }
        #[cfg(feature = "physics")]
        assert!(with_defaults.contains("ColliderShape"));
    }

    /// Primitives draw through the generator runtime; without a throttle
    /// every sync applies the latest edit, as these tests expect.
    fn world() -> World {
        let mut w = World::with_registry(shared_engine_registry());
        w.insert_resource(Assets::<MeshData>::new());
        w.insert_resource(Assets::<StandardMaterial>::new());
        w.insert_resource(box_proc::ProcSettings {
            throttle: std::time::Duration::ZERO,
            ..Default::default()
        });
        w
    }

    fn renderer(w: &World, e: Entity) -> MeshRenderer {
        (*w.get::<MeshRenderer>(e).expect("MeshRenderer")).clone()
    }

    /// The renderer's one material, which must be a standard material.
    fn standard(r: &MeshRenderer) -> Handle<StandardMaterial> {
        match r.materials.as_slice() {
            [MaterialHandle::Standard(h)] => *h,
            other => panic!("expected one standard material, got {other:?}"),
        }
    }

    fn mesh_version(w: &World, e: Entity) -> u64 {
        let r = renderer(w, e);
        w.resource::<Assets<MeshData>>().version(r.mesh).unwrap()
    }

    fn material_version(w: &World, e: Entity) -> u64 {
        let r = renderer(w, e);
        w.resource::<Assets<StandardMaterial>>()
            .version(standard(&r))
            .unwrap()
    }

    #[test]
    fn registry_has_render_types() {
        let r = engine_registry();
        for c in [
            "Camera",
            "DirectionalLight",
            "PointLight",
            "SpotLight",
            "AmbientVolume",
            "ParticleEmitter",
            "StandardMaterial",
            "ShaderMaterial",
            "Primitive",
            "Transform",
        ] {
            assert!(r.component(c).is_some(), "{c}");
        }
        assert!(r.resource("Environment").is_some());
        assert!(r.component("MeshRenderer").is_none());
        #[cfg(feature = "physics")]
        assert!(r.component("Collider").is_some());
        let fields = r.component("StandardMaterial").unwrap().fields();
        let roughness = fields.iter().find(|f| f.name == "roughness").unwrap();
        assert_eq!(roughness.range.map(|r| (r.min, r.max)), Some((0.0, 1.0)));
        assert!(roughness.doc.is_some());
    }

    /// The post-processing settings nested in `Environment` carry their
    /// inspector metadata, and a scene saved before they existed loads with
    /// their defaults.
    #[test]
    fn environment_post_processing_settings_are_reflected_and_default_when_missing() {
        use box_core::reflect::FieldPath;
        let r = engine_registry();
        let meta = |path: &str| {
            r.field_meta(&FieldPath::parse(&r, path).unwrap())
                .unwrap_or_else(|| panic!("{path}"))
        };
        let radius = meta("Environment.ambient_occlusion.radius");
        assert_eq!(radius.unit, Some("m"));
        assert!(radius.range.is_some() && radius.doc.is_some());
        assert!(meta("Environment.bloom.intensity").range.is_some());
        assert!(meta("Environment.color_grading.lift").doc.is_some());
        let old =
            br#"{"version": 1, "resources": {"Environment": {"exposure": 0.9}}, "entities": []}"#;
        let world = box_core::scene::load(old, std::sync::Arc::new(engine_registry())).unwrap();
        let env = world.resource::<Environment>();
        assert_eq!(env.exposure, 0.9);
        assert_eq!(
            env.ambient_occlusion,
            box_render::AmbientOcclusion::default()
        );
        assert_eq!(env.bloom, box_render::Bloom::default());
        assert!(env.color_grading.is_identity());
        // RS3 on-demand settings: metadata, off when missing (also nested
        // fields missing from a partial struct, as in an RS3 scene).
        for path in [
            "Environment.aerial_haze.density",
            "Environment.height_mist.max_opacity",
            "Environment.distance_quieting.desaturate",
            "Environment.vignette.intensity",
            "Environment.wind.strength",
            "Environment.sky.stars",
            "Environment.color_grading.shadows",
        ] {
            let m = meta(path);
            assert!(m.range.is_some() && m.doc.is_some(), "{path}");
        }
        assert_eq!(env.sky, box_render::Sky::default());
        assert!(!env.aerial_haze.enabled && !env.height_mist.enabled);
        assert!(!env.distance_quieting.enabled);
        assert_eq!(env.vignette, box_render::Vignette::default());
        let rs3 = br#"{"version": 1, "resources": {"Environment": {"color_grading": {"contrast": 1.2}}}, "entities": []}"#;
        let world = box_core::scene::load(rs3, std::sync::Arc::new(engine_registry())).unwrap();
        let g = &world.resource::<Environment>().color_grading;
        assert_eq!(g.contrast, 1.2);
        assert_eq!(g.shadows, Vec3::ONE);
    }

    /// RS4 components save and load (curves and gradients included), and a
    /// point light saved before shadows existed loads without them.
    #[test]
    fn lights_volumes_and_emitters_round_trip_through_scenes() {
        use box_render::{CurveKey, GradientKey, LocalShadow, ParticleBlend};
        let registry = shared_engine_registry();
        let mut w = World::with_registry(registry.clone());
        let point = PointLight {
            shadows: true,
            shadow: LocalShadow {
                resolution: 512,
                ..Default::default()
            },
            ..Default::default()
        };
        let spot = SpotLight {
            inner_angle: 0.2,
            outer_angle: 0.4,
            ..Default::default()
        };
        let volume = AmbientVolume {
            sky_contribution: 0.25,
            ..Default::default()
        };
        let emitter = ParticleEmitter {
            size: vec![CurveKey { t: 0.5, value: 2.0 }],
            color: vec![GradientKey {
                t: 0.0,
                color: Color::linear(1.0, 0.5, 0.0),
            }],
            blend: ParticleBlend::Additive,
            seed: 99,
            ..Default::default()
        };
        w.spawn_saved(Uid::new(), "Lamp", (Transform::default(), point))
            .unwrap();
        w.spawn_saved(Uid::new(), "Spot", (Transform::default(), spot))
            .unwrap();
        w.spawn_saved(Uid::new(), "Room", (Transform::default(), volume))
            .unwrap();
        w.spawn_saved(Uid::new(), "Fire", (Transform::default(), emitter.clone()))
            .unwrap();
        let bytes = box_core::scene::save(&w).unwrap();
        let loaded = box_core::scene::load(&bytes, registry.clone()).unwrap();
        let one = |name: &str| loaded.find_by_name(name).unwrap();
        assert_eq!(*loaded.get::<PointLight>(one("Lamp")).unwrap(), point);
        assert_eq!(*loaded.get::<SpotLight>(one("Spot")).unwrap(), spot);
        assert_eq!(*loaded.get::<AmbientVolume>(one("Room")).unwrap(), volume);
        assert_eq!(
            *loaded.get::<ParticleEmitter>(one("Fire")).unwrap(),
            emitter
        );

        // An RS2 point light: no shadow fields.
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for e in json["entities"].as_array_mut().unwrap() {
            if let Some(p) = e
                .get_mut("components")
                .and_then(|c| c.get_mut("PointLight"))
                .and_then(|p| p.as_object_mut())
            {
                p.remove("shadows");
                p.remove("shadow");
            }
        }
        let old = box_core::scene::load_value(&json, registry).unwrap();
        let lamp = old.find_by_name("Lamp").unwrap();
        let l = *old.get::<PointLight>(lamp).unwrap();
        assert!(!l.shadows);
        assert_eq!(l.shadow, LocalShadow::default());
        assert_eq!(l.intensity, point.intensity);
    }

    #[test]
    fn creates_renderer_and_skips_unchanged_frames() {
        let mut w = world();
        let e = w.spawn((Primitive::default(),));
        sync_scene_meshes(&mut w);
        let r = renderer(&w, e);
        assert!(r.cast_shadows);
        assert_eq!(w.resource::<Assets<MeshData>>().len(), 1);
        assert_eq!(
            w.resource::<Assets<StandardMaterial>>().get(standard(&r)),
            Some(&StandardMaterial::default())
        );
        let (mv, matv) = (mesh_version(&w, e), material_version(&w, e));
        for _ in 0..3 {
            sync_scene_meshes(&mut w);
        }
        assert_eq!(mesh_version(&w, e), mv, "no rebuild when unchanged");
        assert_eq!(material_version(&w, e), matv);
        assert_eq!(renderer(&w, e), r);
    }

    #[test]
    fn shape_change_rebuilds_mesh_in_place() {
        let mut w = world();
        let e = w.spawn((Primitive::default(),));
        sync_scene_meshes(&mut w);
        let before = renderer(&w, e);
        let v = mesh_version(&w, e);
        w.get_mut::<Primitive>(e).unwrap().shape = Shape::Plane {
            size: Vec2::new(4.0, 2.0),
            cells: glam::UVec2::new(1, 1),
        };
        sync_scene_meshes(&mut w);
        assert_eq!(renderer(&w, e).mesh, before.mesh, "same handle");
        assert_eq!(mesh_version(&w, e), v + 1, "version bumped once");
        let meshes = w.resource::<Assets<MeshData>>();
        assert_eq!(meshes.get(before.mesh).unwrap().triangle_count(), 2);
        sync_scene_meshes(&mut w);
        assert_eq!(mesh_version(&w, e), v + 1);
    }

    #[test]
    fn cast_shadows_change_does_not_rebuild() {
        let mut w = world();
        let e = w.spawn((Primitive::default(),));
        sync_scene_meshes(&mut w);
        let v = mesh_version(&w, e);
        w.get_mut::<Primitive>(e).unwrap().cast_shadows = false;
        sync_scene_meshes(&mut w);
        assert!(!renderer(&w, e).cast_shadows);
        assert_eq!(mesh_version(&w, e), v);
    }

    #[test]
    fn material_component_drives_material_asset() {
        let mut w = world();
        let red = StandardMaterial {
            base_color: Color::RED,
            ..Default::default()
        };
        let e = w.spawn((Primitive::default(), red.clone()));
        sync_scene_meshes(&mut w);
        let handle = standard(&renderer(&w, e));
        let material = |w: &World| {
            w.resource::<Assets<StandardMaterial>>()
                .get(handle)
                .unwrap()
                .clone()
        };
        assert_eq!(material(&w), red);
        let v = material_version(&w, e);

        w.get_mut::<StandardMaterial>(e).unwrap().roughness = 0.9;
        sync_scene_meshes(&mut w);
        assert_eq!(material(&w).roughness, 0.9);
        assert_eq!(material_version(&w, e), v + 1);
        let mv = mesh_version(&w, e);

        w.remove_one::<StandardMaterial>(e).unwrap();
        sync_scene_meshes(&mut w);
        assert_eq!(material(&w), StandardMaterial::default());
        assert_eq!(
            mesh_version(&w, e),
            mv,
            "material edits never rebuild the mesh"
        );
    }

    #[test]
    fn removing_primitive_or_despawning_frees_assets() {
        let mut w = world();
        let a = w.spawn((Primitive::default(),));
        let b = w.spawn((Primitive::default(), Uid::new()));
        sync_scene_meshes(&mut w);
        assert_eq!(w.resource::<Assets<MeshData>>().len(), 2);

        w.remove_one::<Primitive>(a).unwrap();
        sync_scene_meshes(&mut w);
        assert!(!w.has::<MeshRenderer>(a));
        assert_eq!(w.resource::<Assets<MeshData>>().len(), 1);
        assert_eq!(w.resource::<Assets<StandardMaterial>>().len(), 1);

        w.despawn(b);
        sync_scene_meshes(&mut w);
        assert!(w.resource::<Assets<MeshData>>().is_empty());
        assert!(w.resource::<Assets<StandardMaterial>>().is_empty());
        assert_eq!(
            w.resource_mut::<Assets<MeshData>>().drain_removed().len(),
            2
        );
    }

    #[test]
    fn removed_mesh_renderer_is_restored() {
        let mut w = world();
        let e = w.spawn((Primitive::default(),));
        sync_scene_meshes(&mut w);
        let r = renderer(&w, e);
        w.remove_one::<MeshRenderer>(e).unwrap();
        sync_scene_meshes(&mut w);
        assert_eq!(renderer(&w, e), r);
        assert_eq!(w.resource::<Assets<MeshData>>().len(), 1);
    }

    #[test]
    fn nan_fields_sync_once() {
        let mut w = world();
        let nan_material = StandardMaterial {
            roughness: f32::NAN,
            ..Default::default()
        };
        let e = w.spawn((
            Primitive::new(Shape::Cuboid {
                size: Vec3::new(1.0, f32::NAN, 1.0),
            }),
            nan_material,
        ));
        sync_scene_meshes(&mut w);
        let (mv, matv) = (mesh_version(&w, e), material_version(&w, e));
        for _ in 0..3 {
            sync_scene_meshes(&mut w);
        }
        assert_eq!(mesh_version(&w, e), mv, "NaN shape rebuilt every frame");
        assert_eq!(
            material_version(&w, e),
            matv,
            "NaN material rewritten every frame"
        );

        // A NaN written later still counts as one change.
        w.get_mut::<StandardMaterial>(e).unwrap().metallic = f32::NAN;
        w.get_mut::<Primitive>(e).unwrap().shape = Shape::Sphere {
            radius: f32::NAN,
            sectors: 8,
            stacks: 4,
        };
        for _ in 0..3 {
            sync_scene_meshes(&mut w);
        }
        assert_eq!(mesh_version(&w, e), mv + 1);
        assert_eq!(material_version(&w, e), matv + 1);
    }

    #[test]
    fn assets_removed_by_something_else_are_recreated() {
        let mut w = world();
        let e = w.spawn((Primitive::default(), StandardMaterial::default()));
        sync_scene_meshes(&mut w);
        let before = renderer(&w, e);
        w.resource_mut::<Assets<MeshData>>().remove(before.mesh);
        w.resource_mut::<Assets<StandardMaterial>>()
            .remove(standard(&before));

        w.get_mut::<Primitive>(e).unwrap().shape = Shape::Plane {
            size: Vec2::new(4.0, 2.0),
            cells: glam::UVec2::ONE,
        };
        w.get_mut::<StandardMaterial>(e).unwrap().roughness = 0.9;
        sync_scene_meshes(&mut w);
        let after = renderer(&w, e);
        let mesh = w.resource::<Assets<MeshData>>().get(after.mesh);
        assert_eq!(
            mesh.map(MeshData::triangle_count),
            Some(2),
            "dead mesh handle"
        );
        let material = w
            .resource::<Assets<StandardMaterial>>()
            .get(standard(&after));
        assert_eq!(
            material.map(|m| m.roughness),
            Some(0.9),
            "dead material handle"
        );

        // The recreated assets are owned: nothing leaks on despawn.
        w.despawn(e);
        sync_scene_meshes(&mut w);
        assert!(w.resource::<Assets<MeshData>>().is_empty());
        assert!(w.resource::<Assets<StandardMaterial>>().is_empty());
    }

    #[test]
    fn existing_mesh_renderer_is_replaced_but_its_assets_are_not_freed() {
        let mut w = world();
        let user_mesh = w
            .resource_mut::<Assets<MeshData>>()
            .add(Shape::default().mesh());
        let user_material = w
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let e = w.spawn((
            Primitive::default(),
            MeshRenderer::new(user_mesh, user_material),
        ));
        sync_scene_meshes(&mut w);
        let r = renderer(&w, e);
        assert_ne!(r.mesh, user_mesh);
        assert_ne!(standard(&r), user_material);
        w.despawn(e);
        sync_scene_meshes(&mut w);
        // Only what the sync created is freed; the caller's assets survive.
        let meshes = w.resource::<Assets<MeshData>>();
        assert_eq!(meshes.len(), 1);
        assert!(meshes.get(user_mesh).is_some());
        let materials = w.resource::<Assets<StandardMaterial>>();
        assert_eq!(materials.len(), 1);
        assert!(materials.get(user_material).is_some());
    }

    #[test]
    fn shader_material_component_drives_a_shader_material_asset() {
        use box_core::asset_ref::AssetRef;
        use box_core::shader_material::ShaderParamValue;
        let mut w = world();
        let stripes = ShaderMaterial::new(AssetRef::path("shaders/stripes.wgsl"))
            .with_param("frequency", ShaderParamValue::Float(4.0));
        // A ShaderMaterial wins over a StandardMaterial on the same entity.
        let e = w.spawn((
            Primitive::default(),
            StandardMaterial::default(),
            stripes.clone(),
        ));
        sync_scene_meshes(&mut w);
        let shader_handle = |w: &World| match renderer(w, e).materials.as_slice() {
            [MaterialHandle::Shader(h)] => *h,
            other => panic!("expected a shader material, got {other:?}"),
        };
        let h = shader_handle(&w);
        assert_eq!(
            w.resource::<Assets<ShaderMaterial>>().get(h),
            Some(&stripes)
        );
        assert!(w.resource::<Assets<StandardMaterial>>().is_empty());

        // Unchanged (also with a NaN parameter): no rewrite.
        w.get_mut::<ShaderMaterial>(e).unwrap().params[0].value = ShaderParamValue::Float(f32::NAN);
        sync_scene_meshes(&mut w);
        let v = w.resource::<Assets<ShaderMaterial>>().version(h).unwrap();
        for _ in 0..3 {
            sync_scene_meshes(&mut w);
        }
        assert_eq!(w.resource::<Assets<ShaderMaterial>>().version(h), Some(v));

        // A parameter edit rewrites the same asset.
        w.get_mut::<ShaderMaterial>(e).unwrap().params[0].value = ShaderParamValue::Float(9.0);
        sync_scene_meshes(&mut w);
        assert_eq!(shader_handle(&w), h);
        assert_eq!(
            w.resource::<Assets<ShaderMaterial>>().version(h),
            Some(v + 1)
        );

        // Removing it falls back to the StandardMaterial, in the other store.
        w.remove_one::<ShaderMaterial>(e).unwrap();
        sync_scene_meshes(&mut w);
        let r = renderer(&w, e);
        assert!(w.resource::<Assets<ShaderMaterial>>().is_empty());
        assert!(
            w.resource::<Assets<StandardMaterial>>()
                .get(standard(&r))
                .is_some()
        );

        w.despawn(e);
        sync_scene_meshes(&mut w);
        assert!(w.resource::<Assets<StandardMaterial>>().is_empty());
        assert!(w.resource::<Assets<MeshData>>().is_empty());
    }

    /// A `MaterialRef` wins over the inline material, follows its asset
    /// (same handle, so in-place reloads show), falls back to the
    /// placeholder when it names nothing loadable, and unlinking goes back
    /// to the inline material.
    #[test]
    fn material_ref_links_a_material_asset() {
        use box_core::asset_ref::AssetId;
        use box_core::assets::AssetLookup;
        let mut w = world();
        let stone = StandardMaterial {
            base_color: Color::GREEN,
            ..Default::default()
        };
        let asset = w.resource_mut::<Assets<StandardMaterial>>().add(stone);
        let id = AssetId::from_u128(42);
        let mut lookup = AssetLookup::default();
        lookup.insert(id, None, Some("materials/stone.material.json"), asset);
        w.insert_resource(lookup);
        let red = StandardMaterial {
            base_color: Color::RED,
            ..Default::default()
        };
        let e = w.spawn((
            Primitive::default(),
            red.clone(),
            MaterialRef::new(AssetRef::id(id)),
        ));
        sync_scene_meshes(&mut w);
        assert_eq!(
            standard(&renderer(&w, e)),
            asset,
            "the linked asset is drawn"
        );
        // An edit of the asset (a hot reload) changes it in place.
        w.resource_mut::<Assets<StandardMaterial>>()
            .get_mut(asset)
            .unwrap()
            .roughness = 0.1;
        sync_scene_meshes(&mut w);
        assert_eq!(standard(&renderer(&w, e)), asset);
        // Unchanged frames do no work.
        let before = renderer(&w, e);
        for _ in 0..3 {
            sync_scene_meshes(&mut w);
        }
        assert_eq!(renderer(&w, e), before);

        // A reference to nothing: the placeholder, one asset for all.
        w.get_mut::<MaterialRef>(e).unwrap().material = AssetRef::id(AssetId::from_u128(7));
        sync_scene_meshes(&mut w);
        let placeholder = standard(&renderer(&w, e));
        assert_ne!(placeholder, asset);
        assert_eq!(
            w.resource::<Assets<StandardMaterial>>().get(placeholder),
            Some(&box_assets::placeholder::placeholder_material())
        );

        // Unlinked: the inline material again; the linked asset survives.
        w.remove_one::<MaterialRef>(e).unwrap();
        sync_scene_meshes(&mut w);
        let inline = standard(&renderer(&w, e));
        assert_eq!(
            w.resource::<Assets<StandardMaterial>>().get(inline),
            Some(&red)
        );
        assert!(
            w.resource::<Assets<StandardMaterial>>()
                .get(asset)
                .is_some()
        );
        w.despawn(e);
        sync_scene_meshes(&mut w);
        assert!(
            w.resource::<Assets<StandardMaterial>>()
                .get(asset)
                .is_some(),
            "the linked asset is not the sync's to free"
        );
    }

    #[test]
    fn framing_contains_bounds() {
        let b = Aabb::new(Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 2.0, 10.0));
        let t = framing_transform(&b, 50f32.to_radians(), 16.0 / 9.0);
        let to_center = (b.center() - t.translation).normalize();
        assert!(t.forward().dot(to_center) > 0.999);
        assert!((t.translation - b.center()).length() > b.half_extents().length());
    }
}
