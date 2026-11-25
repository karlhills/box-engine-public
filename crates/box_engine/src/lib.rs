//! box-engine runner: window, event loop, input mapping and the prelude that
//! games import.
//!
//! A game is a [`Plugin`] wrapped in a [`Game`]; the
//! same `Game` value drives the editor, the player, Play sessions and tests:
//!
//! ```no_run
//! use box_engine::prelude::*;
//!
//! struct MyGame;
//! impl Plugin for MyGame {
//!     fn build(&self, app: &mut App) {
//!         app.world.spawn((Transform::default(), Primitive::cuboid(Vec3::ONE)));
//!     }
//! }
//!
//! fn main() -> anyhow::Result<()> {
//!     // A window, or headless with `--capture <png>` / `--headless`.
//!     Game::new().with_plugin(MyGame).run("my game")
//! }
//! ```
//!
//! Entry points, from most to least common: [`player::run_game`] (play a
//! project's scenes; what a native project's player binary calls),
//! `box_editor::run_game` (the editor with the game's types),
//! [`Game::run`] (a code-only game), [`Game::new_app`] (an app for tests;
//! drive it with `App::run_frames`), and the low-level [`default_app`] /
//! [`run`].

pub mod agent_files;
mod animation_glue;
pub mod asset_glue;
#[cfg(feature = "audio")]
pub mod audio_mixer;
pub mod bake;
pub mod behaviours;
pub mod camera;
pub mod control;
pub mod crash;
pub mod data;
pub mod game;
pub mod gamepad;
pub mod input;
mod input_map;
pub mod interact;
pub mod io_hint;
mod live;
pub mod log_capture;
pub mod modules;
#[cfg(feature = "navigation")]
pub mod nav_agent;
#[cfg(feature = "navigation")]
pub mod nav_glue;
pub mod packed;
pub mod player;
pub mod playtest;
pub mod proc_glue;
pub mod project;
pub mod registry_file;
#[cfg(feature = "physics")]
pub mod route;
mod runner;
pub mod savegame;
pub mod scatter_glue;
pub mod scene_glue;
pub mod scenes;
#[cfg(feature = "script")]
pub mod script_glue;
pub mod settings;
pub mod splash;
pub mod templates;
pub mod terrain_glue;
pub mod upgrade;

pub use asset_glue::{AssetGluePlugin, settle_assets, settle_step, sync_assets};
pub use box_assets;
#[cfg(feature = "audio")]
pub use box_audio;
pub use box_core;
#[cfg(feature = "physics")]
pub use box_physics;
pub use box_proc;
pub use box_render;
#[cfg(feature = "script")]
pub use box_script;
pub use box_ui;
pub use camera::{
    CameraPose, OrbitCamera, active_camera, cursor_ray, orbit_camera_system, overview_transform,
    parse_point, screen_ray, world_to_screen,
};
pub use game::Game;
pub use gamepad::GamepadPoller;
pub use input::{CursorController, InputPlugin, apply_window_event};
pub use project::{Project, ProjectError};
pub use runner::{RunOptions, RunSummary, WindowSize, run, run_with};
pub use scene_glue::{engine_registry, shared_engine_registry, sync_scene_meshes};
pub use scenes::{
    Persistent, RuntimeEntities, SceneEvent, SceneInstance, SceneInstanceId, ScenePlugin, Scenes,
    warn_unknown_types,
};

use box_core::input::Input;
use box_core::prelude::*;
use box_render::{DebugDraw, Environment, RenderSettings, StandardMaterial};

/// What a game imports: `use box_engine::prelude::*;`.
///
/// Besides the engine types it brings `bevy_reflect` into scope, so
/// `#[derive(Reflect)]` works without a `bevy_reflect` dependency, the
/// reflect field attributes (`Range`, `Unit`, `Step`, `Hidden`, `NotSaved`)
/// and the `tracing` log macros (`info!`, `warn!`, `error!`, `debug!`).
pub mod prelude {
    pub use crate::camera::{
        OrbitCamera, active_camera, cursor_ray, orbit_camera_system, screen_ray, world_to_screen,
    };
    pub use crate::game::Game;
    pub use crate::input::InputPlugin;
    pub use crate::runner::{RunOptions, WindowSize, run};
    pub use crate::scene_glue::{engine_registry, sync_scene_meshes};
    pub use crate::scenes::{
        Persistent, RuntimeEntities, SceneEvent, SceneInstance, SceneInstanceId, Scenes,
    };
    #[cfg(feature = "audio")]
    pub use box_audio::{Audio, AudioBus, AudioListener, AudioSettings, AudioSource, PlaySound};
    pub use box_core::prelude::*;
    // `#[derive(Reflect)]` emits `bevy_reflect::` paths; this resolves them
    // through the glob import, at the engine's exact version.
    pub use crate::data::{data_table, data_version};
    pub use crate::savegame::{SaveConfig, SaveEvent, SaveExclude, SaveGames, SaveHooks, Saveable};
    pub use box_assets::{DataTable, FontAsset, StringTable};
    pub use box_core::reflect::bevy_reflect;
    pub use box_core::reflect::{Hidden, NotSaved, Range, Step, Unit};
    // `Step` is the field metadata above; migration steps are `MigrationStep`.
    pub use box_core::migrate::Step as MigrationStep;
    #[cfg(feature = "physics")]
    pub use box_physics::prelude::*;
    pub use box_proc::{
        AppGeneratorExt, ColliderHint, ControlPoint, Extrude, FollowGround, GenContext, GenOutput,
        Generator, GeneratorRegistry, Ground, GroundSample, Hole, Inputs, Lathe, LatheShape,
        Outline, ProcSettings, ProcState, Procedural, Rng, Spline, SplineMode, Sweep,
        SweepOrientation, SweepProfile, Terrain, TerrainLayers, TerrainPath, TerrainStamp,
    };
    pub use box_render::{
        AerialHaze, AlphaMode, AmbientOcclusion, Bloom, Camera, ColorGrading, DebugDraw,
        DepthWrite, DirectionalLight, DistanceQuieting, Environment, FrameStats, HeightMist,
        MaterialHandle, MeshRenderer, PointLight, RenderSettings, Renderer, ShaderMaterial,
        ShaderSource, Sky, SkyGradient, StandardMaterial, Tonemapper, ViewportPoint, Vignette,
        Wind,
    };
    pub use box_render::{
        AmbientVolume, CurveKey, GradientKey, LocalShadow, ParticleBlend, ParticleBurst,
        ParticleEmitter, ParticleSpace, ParticleState, SpotLight,
    };
    #[cfg(feature = "script")]
    pub use box_script::{AppScriptExt, Script, ScriptApi, ScriptValue};
    pub use box_ui::{
        NodeId, NodeKind, ScaleMode, Strings, Style, Theme, Ui, UiEvent, UiEventKind, UiNode,
    };
    pub use tracing::{debug, error, info, warn};
}

/// An [`App`] whose world uses the [engine registry](engine_registry), with
/// the render resources inserted, [`InputPlugin`] (named actions, cursor
/// options) first, then runtime scenes ([`ScenePlugin`], applied at the start
/// of `Update`), the asset glue ([`AssetGluePlugin`]: runs the world's
/// `AssetServer` resource, if any, instances prefabs and builds `MeshAsset`
/// renderers, right after scene requests), settings hot reload
/// ([`settings::SettingsPlugin`]), physics ([`box_physics::PhysicsPlugin`]
/// and the kinematic characters of [`box_physics::CharacterPlugin`], with
/// the default `physics` feature), audio (`box_audio::AudioPlugin`, listening from the
/// active camera unless an entity has an `AudioListener`; default `audio`
/// feature), tweens ([`TweenPlugin`](box_core::tween::TweenPlugin)), the generator
/// runtime's settings ([`box_proc::ProcPlugin`]), the in-game UI
/// ([`box_ui::UiPlugin`], sized from [`WindowSize`]), save games
/// ([`savegame::SavePlugin`]) and the orbit camera
/// system in `Update`, and [`sync_scene_meshes`] in `PostUpdate` (after transform propagation;
/// it also regenerates procedural objects), then particle simulation
/// ([`box_render::simulate_particles`]).
pub fn default_app() -> App {
    app_with_world(World::with_registry(shared_engine_registry()))
}

/// [`default_app`] around an existing world, such as a loaded scene. Missing
/// app and render resources are inserted with default values; resources the
/// world already has (a scene's `Environment`) are kept. Saved entities
/// already in the world become the main [scene instance](scenes).
pub fn app_with_world(world: World) -> App {
    let mut app = App::new();
    app.world = world;
    insert_engine_resources(&mut app.world);
    app.add_plugin(&InputPlugin);
    app.add_plugin(&ScenePlugin);
    app.add_plugin(&AssetGluePlugin);
    app.add_plugin(&settings::SettingsPlugin);
    app.add_plugin(&terrain_glue::TerrainPlugin);
    #[cfg(feature = "physics")]
    {
        app.add_plugin(&box_physics::PhysicsPlugin);
        // Scripted and data projects get kinematic characters without Rust.
        app.add_plugin(&box_physics::CharacterPlugin);
    }
    #[cfg(feature = "navigation")]
    app.add_plugin(&nav_glue::NavigationPlugin);
    #[cfg(feature = "audio")]
    app.add_plugin(&box_audio::AudioPlugin::new().with_default_listener(active_camera_pose));
    app.add_plugin(&box_core::tween::TweenPlugin);
    app.add_plugin(&box_core::animation::AnimationPlugin);
    app.add_plugin(&box_proc::ProcPlugin);
    app.add_plugin(&box_ui::UiPlugin);
    app.system(Stage::PostUpdate, sync_ui_screen)
        .label("ui_screen")
        .before("ui_layout");
    app.add_plugin(&savegame::SavePlugin);
    app.add_plugin(&interact::InteractPlugin);
    app.add_plugin(&behaviours::BehavioursPlugin);
    app.add_system(Stage::Update, orbit_camera_system);
    app.add_system(Stage::PostUpdate, sync_scene_meshes);
    app.system(Stage::PostUpdate, box_render::simulate_particles)
        .label("simulate_particles");
    app
}

/// Keeps the UI's target size equal to [`WindowSize`] (the window, a
/// capture, or the editor's game view).
fn sync_ui_screen(world: &mut World) {
    let Some(size) = world.get_resource::<WindowSize>().copied() else {
        return;
    };
    let screen = box_ui::UiScreen {
        width: size.width,
        height: size.height,
        scale_factor: if size.scale_factor_milli == 0 {
            1.0
        } else {
            size.scale_factor_milli as f32 / 1000.0
        },
    };
    let current = world.resource_or_default::<box_ui::UiScreen>();
    if *current != screen {
        *current = screen;
    }
}

/// The global transform of the world's first active camera (where sounds
/// are heard from when no entity has an `AudioListener`).
pub fn active_camera_pose(world: &World) -> Option<GlobalTransform> {
    active_camera(world).map(|(_, g)| g)
}

/// An [`AssetServer`](box_assets::AssetServer) for `assets_dir` with the
/// built-in importers plus those of the enabled engine modules (audio
/// clips, with the `audio` feature; `.luau` scripts, with `script`). Use it
/// instead of `AssetServer::new` wherever a project's assets are opened
/// (player, editor, CLI).
pub fn asset_server(
    assets_dir: impl AsRef<std::path::Path>,
) -> Result<box_assets::AssetServer, box_assets::AssetError> {
    let server = box_assets::AssetServer::new(assets_dir)?;
    #[cfg(feature = "audio")]
    box_audio::register_audio_assets(&server);
    #[cfg(feature = "script")]
    box_script::register_script_assets(&server);
    Ok(server)
}

/// Insert, with default values, the app and render resources the runner and
/// renderer need (`Time`, `FixedTime`, `Input`, `AppExit`, the mesh and
/// material stores, `Environment`, `RenderSettings`, `DebugDraw`,
/// `WindowSize`) where the world lacks them. Resources already present are
/// kept. [`app_with_world`] and [`run`] call it, so an app built with the
/// low-level `App::new()` still runs.
pub fn insert_engine_resources(w: &mut World) {
    w.resource_or_default::<Time>();
    w.resource_or_default::<FixedTime>();
    w.resource_or_default::<Input>();
    w.resource_or_default::<AppExit>();
    w.resource_or_default::<Assets<MeshData>>();
    w.resource_or_default::<Assets<StandardMaterial>>();
    w.resource_or_default::<Environment>();
    w.resource_or_default::<RenderSettings>();
    w.resource_or_default::<DebugDraw>();
    w.resource_or_default::<WindowSize>();
}

/// The default terminal filter: warnings and errors from everything, plus
/// info from the engine's own crates (targets starting with `box`). `RUST_LOG`
/// replaces it.
pub const TERMINAL_LOG_FILTER: &str = "warn,box=info";

/// Filter for the capture ring used by playtests and the control socket's
/// `read_log`: info from everything except wgpu and naga internals.
/// Keeping it independent of `RUST_LOG` makes replay checks reproducible.
pub const CAPTURE_LOG_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn";

/// `RUST_LOG`, else `default`.
fn env_filter(default: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default))
}

/// The terminal log layer of the engine's binaries: formatted records on
/// stderr (stdout stays free for program output), filtered by `RUST_LOG` or
/// [`TERMINAL_LOG_FILTER`], with ANSI colours only when stderr is a
/// terminal.
pub fn terminal_log_layer<S>() -> Box<dyn tracing_subscriber::Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use std::io::IsTerminal;
    use tracing_subscriber::Layer;
    tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_filter(env_filter(TERMINAL_LOG_FILTER))
        .boxed()
}

/// Install a `tracing` subscriber that also captures `log` records (wgpu logs
/// through `log`). Safe to call more than once.
///
/// The terminal gets [`terminal_log_layer`] (quiet by default: warnings, and
/// info from `box*` targets). Next to it [`log_capture::CaptureLayer`] keeps
/// records for playtest checks and the control socket's `read_log` (filtered
/// by [`CAPTURE_LOG_FILTER`]) once [`log_capture::enable`] is called.
pub fn init_logging() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{EnvFilter, Layer};
    let _ = tracing_subscriber::registry()
        .with(terminal_log_layer())
        .with(log_capture::CaptureLayer.with_filter(EnvFilter::new(CAPTURE_LOG_FILTER)))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_app_gets_the_resources_the_runner_needs() {
        let mut app = App::new();
        app.world.insert_resource(Environment {
            fog_density: 0.5,
            ..Default::default()
        });
        insert_engine_resources(&mut app.world);
        for present in [
            app.world.contains_resource::<Assets<MeshData>>(),
            app.world.contains_resource::<Assets<StandardMaterial>>(),
            app.world.contains_resource::<RenderSettings>(),
            app.world.contains_resource::<DebugDraw>(),
            app.world.contains_resource::<WindowSize>(),
            app.world.contains_resource::<Time>(),
        ] {
            assert!(present);
        }
        assert_eq!(app.world.resource::<Environment>().fog_density, 0.5, "kept");
    }
}
