//! box-engine scripting: Luau scripts through `mlua` (decision D4, shaped by
//! spike S4).
//!
//! - [`Script`]: a saved component naming a `.luau` file under `assets/`,
//!   with property values that override the script's declared defaults.
//! - [`ScriptAsset`] and [`ScriptImporter`]: script files as assets (hot
//!   reloaded like any other asset).
//! - [`ScriptPlugin`]: runs the scripts of a world that plays (the player,
//!   Play mode). `box_engine::Game::app` adds it after the game's plugins:
//!   in Update scripts run after the game's native systems; in FixedUpdate
//!   they run after the rigid-body step and the native systems ordered
//!   before `move_characters`, then characters move (script input applies
//!   in the same fixed step), then the other native fixed systems.
//! - The engine API scripts see ([`api`]): value types, entity methods,
//!   `world`, `time`, `input`, `physics` (queries, `move_and_slide`, and
//!   with the `dynamics` feature forces and body reads), `character`,
//!   `events`, `task`, `log`, plus every [`ScriptApi`] plugins add to
//!   [`ScriptApis`] (box_engine: `scenes`, `animation`, `save`, `data`,
//!   `particles`; box_ui: `ui`, `strings`). Component, resource and event
//!   types come from the registry, unbound. Bindings add entity methods
//!   with [`entity_methods`] and turn Rust events into script handlers
//!   with [`ScriptSignals`] and [`subscribe`].
//! - [`types::definitions`]: luau-lsp definitions for editor completion.
//! - [`save_script_state`] / [`load_script_state`]: the script half of saved
//!   games (`on_save` / `on_load`).
//!
//! A script:
//!
//! ```lua
//! local Spinner = {}
//!
//! Spinner.properties = {
//!     speed = { type = "number", default = 90, min = 0, max = 720, unit = "deg/s" },
//!     axis = { default = vector.create(0, 1, 0) },
//! }
//!
//! function Spinner.on_update(self, dt: number)
//!     local turn = Quat.from_axis_angle(self.props.axis, math.rad(self.props.speed) * dt)
//!     self.entity:rotate(turn)
//! end
//!
//! return Spinner
//! ```
//!
//! See `docs/SCRIPTING.md` ("M7 as built") for the full model: lifecycle,
//! value semantics, waits, batch queries, state and hot reload, errors.

pub mod api;
mod asset;
mod component;
pub mod convert;
pub mod docs;
pub mod rules;
mod runtime;
pub mod types;

pub use api::{
    ScriptApi, ScriptApis, ScriptCall, ScriptCalls, ScriptSignals, deprecated, deprecated_alias,
    entity_methods, subscribe, unsubscribe, with_world, with_world_mut,
};
pub use asset::{ScriptAsset, ScriptImporter};
pub use component::{PropType, PropertyDecl, Script, ScriptProperty, ScriptValue};
pub use mlua;
pub use rules::{ScriptIntent, ScriptRules, add_script_rules};
pub use runtime::{
    DEFAULT_WATCHDOG, ScriptQueue, ScriptRuntime, ScriptSeed, ScriptStats, describe_error,
    load_script_state, save_script_state, script_fixed_update, script_update,
};

use box_assets::AssetServer;
use box_core::app::{App, Plugin, Stage};
use box_core::reflect::Registry;

/// Registers the `Script` and `ScriptIntent` components (and their value
/// types). Registering twice is harmless.
pub fn register_script_types(registry: &mut Registry) {
    for e in registry.register_all::<(Script, ScriptIntent)>() {
        tracing::error!(target: "box_script", "script type registration failed: {e}");
    }
}

/// Adds the [`ScriptAsset`] type and the `.luau` importer to a server.
/// `box_engine::asset_server` calls it.
pub fn register_script_assets(server: &AssetServer) {
    server.register_asset_type::<ScriptAsset>(None);
    server.register_importer(ScriptImporter);
}

/// Runs scripts: tracks `Script` components (hooks), and adds
/// [`script_fixed_update`] (label `scripts_fixed_update`) and
/// [`script_update`] (label `scripts_update`). The VM is created on the
/// first frame that has a script.
///
/// `scripts_fixed_update` runs before `move_characters` (it adds
/// [`CharacterPlugin`](box_physics::CharacterPlugin), which is harmless
/// twice), like a native input system: `character.move`/`jump` from
/// `on_fixed_update` move the character in the same fixed step, and
/// `on_fixed_update` sees the character as the previous step left it.
/// `scripts_update` runs after the game's native Update systems.
pub struct ScriptPlugin;

impl Plugin for ScriptPlugin {
    fn register(&self, registry: &mut Registry) {
        register_script_types(registry);
    }

    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<ScriptQueue>();
        app.world.resource_or_default::<ScriptApis>();
        app.on_add::<Script>(|w, e| w.resource_or_default::<ScriptQueue>().added.push(e));
        app.on_remove::<Script>(|w, e| w.resource_or_default::<ScriptQueue>().removed.push(e));
        // Script input reaches characters in the step it is given, as a
        // native system ordered before `move_characters` does.
        app.add_plugin(&box_physics::CharacterPlugin);
        app.system(Stage::FixedUpdate, script_fixed_update)
            .label("scripts_fixed_update")
            .before("move_characters");
        app.system(Stage::Update, script_update)
            .label("scripts_update");
    }
}

/// `app.add_script_api(api)`: adds a [`ScriptApi`] to the app's
/// [`ScriptApis`] (call it in `Plugin::build`).
pub trait AppScriptExt {
    fn add_script_api(&mut self, api: impl ScriptApi) -> &mut Self;
}

impl AppScriptExt for App {
    fn add_script_api(&mut self, api: impl ScriptApi) -> &mut Self {
        self.world.resource_or_default::<ScriptApis>().add(api);
        self
    }
}
