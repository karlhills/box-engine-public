//! Forward renderer for box-engine, built on wgpu.
//!
//! Stage RS1: HDR scene with 4x MSAA, tone mapping (analytic AgX, ACES fitted,
//! none), one [`DirectionalLight`] with four stable shadow cascades,
//! hemisphere ambient, a gradient sky with a sun disc, distance and height
//! fog, immediate-mode [`DebugDraw`] lines, a screen-space
//! [`SelectionOutline`], headless capture and [`golden`] image comparison.
//! Shadow casters are culled per cascade.
//!
//! Stage RS2: textured [`StandardMaterial`]s (base colour, normal, ORM and
//! emissive maps with mip chains), alpha modes (opaque, mask, blend,
//! additive, multiply) with a transparent pass sorted back to front,
//! double-sided materials, unshadowed [`PointLight`]s in a counted light
//! buffer, a material per mesh group ([`MeshRenderer::materials`]) and custom
//! [`ShaderMaterial`]s: user WGSL `surface` hooks lit by the engine.
//!
//! Stage RS3: automatic instancing (objects sharing a mesh, material group
//! and pipeline are one instanced draw in every pass; per-object data in a
//! storage buffer, rewritten only on change), half-resolution screen-space
//! [`AmbientOcclusion`] from a single-sample depth + normal prepass with a
//! per-fragment depth-aware upsample (it darkens ambient light, so it works
//! with the 4x MSAA main pass), energy-conserving [`Bloom`], and
//! [`ColorGrading`], all set in [`Environment`]. Headless tools can time
//! every pass on the GPU ([`Renderer::set_gpu_timing`]).
//!
//! RS3 on demand (for the construction slice): [`AerialHaze`], [`HeightMist`]
//! and [`DistanceQuieting`] (atmosphere on every surface and the sky), a
//! layered [`Sky`] gradient with sun glow, horizon glow and stars,
//! shadows/midtones/highlights grading and a [`Vignette`], alpha-tested
//! shadow casters, [`WindSway`] driven by the global [`Wind`] (the same
//! vertex function in every pass; `wind_offset` for material shaders), and
//! soft transparency (`depth_fade`) against the prepass depth. All of it is
//! uniforms: animating the environment never rebuilds a pipeline.
//!
//! Stage RS4 (lights and particles): [`SpotLight`]s share the counted
//! light buffer with point lights; one local light per frame (spot: one
//! view, point: a six-view cube) casts shadows with PCF ([`LocalShadow`]);
//! [`AmbientVolume`]s replace the sky ambient inside boxes (interiors);
//! CPU-simulated billboard particles ([`ParticleEmitter`], advanced by
//! [`simulate_particles`]) are drawn instanced in the transparent pass,
//! sorted per emitter, with soft edges against the prepass depth.
//!
//! The [`Renderer`] reads everything from the [`World`](box_core::world::World):
//! `Assets<MeshData>`, `Assets<StandardMaterial>`, `Assets<ShaderMaterial>`,
//! `Assets<ImageData>`, `Assets<ShaderSource>` and `AssetLookup` (textures and
//! material shaders are referenced by `AssetRef` and resolved through it),
//! [`Environment`], [`RenderSettings`], [`DebugDraw`] and [`SelectionOutline`]
//! resources, the first active [`Camera`], the first [`DirectionalLight`],
//! every [`PointLight`], [`SpotLight`], [`AmbientVolume`] and
//! [`ParticleEmitter`] (with its [`ParticleState`]).
//!
//! Game UI: the [`Overlay`] resource (2D triangles in physical pixels, built
//! by `box_ui`) is drawn over the tone-mapped image in every frame target.
//!
//! Tools draw the scene into an offscreen [`ViewportTarget`] of any size with
//! [`Renderer::render_to`], optionally from a [`ViewCamera`] that is not an
//! entity, and can share the renderer's device ([`Renderer::device`]) and
//! window surface ([`Renderer::acquire_surface_frame`]).
//!
//! Projection helpers ([`Camera::world_to_viewport`] →
//! [`ViewportPoint`], [`Camera::viewport_ray`], [`Camera::projection`],
//! also on [`ViewCamera`]) use the renderer's own projection; use them
//! rather than re-deriving it.

mod cascades;
mod components;
mod culling;
mod debug_draw;
pub mod golden;
mod gpu_mesh;
mod instances;
mod lights;
#[cfg(test)]
mod look_tests;
mod material_shader;
#[cfg(test)]
mod material_state_tests;
mod materials;
mod overlay;
mod particle_render;
mod particles;
mod pipelines;
mod post;
mod projection;
mod renderer;
#[cfg(test)]
mod rs4_tests;
mod shaders;
mod skinning;
mod textures;
mod uniforms;
mod viewport;

pub use components::{
    AerialHaze, AlphaMode, AmbientOcclusion, AmbientVolume, Bloom, Camera, ColorGrading,
    DepthWrite, DirectionalLight, DistanceQuieting, Environment, HeightMist, Hidden, LocalShadow,
    MaterialHandle, MeshRenderer, PointLight, RenderSettings, SelectionOutline, ShaderMaterial,
    ShaderSource, Sky, SkyGradient, SpotLight, StandardMaterial, Tonemapper, Vignette, Wind,
    WindSway, WindWeight,
};
pub use debug_draw::{DebugDraw, LineLayer};
pub use lights::MAX_LOCAL_LIGHTS;
pub use material_shader::check_material_shader;
pub use overlay::{Overlay, OverlayBatch, OverlayTexture, OverlayTextureUpdate, OverlayVertex};
pub use particles::{
    CurveKey, GradientKey, MAX_PARTICLES_PER_EMITTER, Particle, ParticleBlend, ParticleBurst,
    ParticleEmitter, ParticleSpace, ParticleState, simulate_particles,
};
pub use projection::ViewportPoint;
pub use renderer::{FrameStats, RenderError, Renderer, SurfaceFrame, set_gpu_validation};
pub use shaders::set_shader_hot_reload;
pub use viewport::{VIEWPORT_FORMAT, ViewCamera, ViewportTarget};
pub use wgpu;
