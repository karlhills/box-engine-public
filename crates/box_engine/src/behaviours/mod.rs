//! Behaviours (docs/BEHAVIOURS.md, D19): the gameplay every small 3D game
//! needs, as components configured in the inspector instead of scripts.
//!
//! - Motion: [`Spinner`], [`Bobber`], [`Mover`] (moving platforms carry
//!   characters).
//! - The player: [`PlayerController`] (physics module) and [`FollowCamera`].
//! - [`Health`] (hits, invulnerability, `on_zero`) and [`Spawner`] (copies of
//!   a template at an interval).
//! - Rules: [`GameRules`] (scene resource: win, lose, time limit, lives,
//!   built-in pause and end screens), [`GameState`] (runtime), and the
//!   built-in HUD ([`HudSettings`]).
//! - [`presets`]: ready-made objects (Pickup, Hazard, Goal, ...) as commands
//!   the editor, `box scene apply` and MCP share.
//!
//! "When X happens, do Y" uses the interaction actions of
//! [`crate::interact`] (`Win`, `Respawn`, `SetCheckpoint`, ...), so there is
//! one action vocabulary.
//!
//! [`BehavioursPlugin`] (in every [`app_with_world`](crate::app_with_world))
//! adds the systems. Cost per frame: one pass over each behaviour's
//! entities (a transform write each), the rules' condition checks, and HUD
//! text setters only when a line changes.

#[cfg(feature = "navigation")]
pub mod chaser;
pub mod check;
pub mod follow_camera;
pub mod health;
pub mod motion;
#[cfg(feature = "physics")]
pub mod player;
pub mod presets;
pub mod rules;
pub mod scripts;
pub mod spawner;
pub mod view;

use box_core::action::{ActionMap, Axis2Binding, Binding, GamepadStick};
use box_core::app::{App, Plugin, Stage};
use box_core::input::{GamepadButton, KeyCode};
use box_core::reflect::Registry;
use box_core::world::{Entity, World};

#[cfg(feature = "navigation")]
pub use chaser::{Chaser, ChaserMode, ChaserState};
pub use follow_camera::{FollowCamera, FollowCameraState, FollowMode};
pub use health::{Health, HealthState, damage, heal, refill};
pub use motion::{Bobber, MotionState, Mover, MoverMode, Spinner};
#[cfg(feature = "physics")]
pub use player::PlayerController;
pub use rules::{
    GAME_SAVE_HOOK, GamePhase, GameRules, GameState, HudSettings, HudVar, PAUSE_ACTION, SpawnPoint,
    burst, hud_lines, lose, respawn, respawn_point, set_checkpoint, switch_scene, teleport, win,
};
pub use spawner::{SpawnTemplate, Spawner, SpawnerState, is_spawn_template};

use crate::savegame::SaveHooks;

/// Save hook name of the motion state.
pub const MOTION_SAVE_HOOK: &str = "motion";

/// Registers the behaviour types and the built-in presets that need no
/// optional module.
pub fn register_behaviour_types(registry: &mut Registry) {
    // Common names: a game's own `Mover` or `GameState` takes the name over
    // (`register_yielding`).
    let results = [
        registry.register_yielding::<Spinner>(),
        registry.register_yielding::<Bobber>(),
        registry.register_yielding::<Mover>(),
        registry.register_yielding::<FollowCamera>(),
        registry.register_yielding::<Health>(),
        registry.register_yielding::<Spawner>(),
        registry.register_resource_yielding::<GameRules>(),
        registry.register_resource_yielding::<GameState>(),
        registry.register_resource_yielding::<HudSettings>(),
    ];
    for r in results {
        if let Err(e) = r {
            tracing::error!("behaviour type registration failed: {e}");
        }
    }
    presets::register_builtin_presets(registry, false);
    scripts::register_behaviour_scripts(registry);
}

/// Registers [`PlayerController`] and the Player preset (physics module).
#[cfg(feature = "physics")]
pub fn register_player_types(registry: &mut Registry) {
    if let Err(e) = registry.register_yielding::<PlayerController>() {
        tracing::error!("behaviour type registration failed: {e}");
    }
    presets::register_builtin_presets(registry, true);
}

/// The actions behaviours read, merged into the action map where it lacks
/// them ([`add_default_actions`]): `move` (WASD, arrows, d-pad, left stick), `look` (right stick),
/// `jump` (Space, gamepad South), `run` (Shift, gamepad West) and `pause`
/// (Escape, P, gamepad Start).
pub fn default_game_actions() -> ActionMap {
    let dpad = Axis2Binding::Buttons {
        up: Binding::Gamepad(GamepadButton::DPadUp),
        down: Binding::Gamepad(GamepadButton::DPadDown),
        left: Binding::Gamepad(GamepadButton::DPadLeft),
        right: Binding::Gamepad(GamepadButton::DPadRight),
    };
    ActionMap::new()
        .axis2(
            "move",
            [
                Axis2Binding::wasd(),
                Axis2Binding::arrows(),
                dpad,
                Axis2Binding::Stick(GamepadStick::Left),
            ],
        )
        .axis2("look", [Axis2Binding::Stick(GamepadStick::Right)])
        .button(
            "jump",
            [
                Binding::Key(KeyCode::Space),
                Binding::Gamepad(GamepadButton::South),
            ],
        )
        .button(
            "run",
            [
                Binding::Key(KeyCode::ShiftLeft),
                Binding::Gamepad(GamepadButton::West),
            ],
        )
        .button(
            PAUSE_ACTION,
            [
                Binding::Key(KeyCode::Escape),
                Binding::Key(KeyCode::P),
                Binding::Gamepad(GamepadButton::Start),
            ],
        )
}

/// Adds the [`default_game_actions`] the world's action map lacks.
/// [`Game::app`](crate::Game::app) calls it after the game's plugins built
/// (their bindings win) and before the project's action file is applied.
pub fn add_default_actions(world: &mut World) {
    world
        .resource_or_default::<ActionMap>()
        .merge_missing(&default_game_actions());
}

/// For apps not made by `Game::app`, or whose game replaced the action map
/// later: adds the missing default actions and returns true, so the caller
/// skips reading input this frame (actions were evaluated without them).
/// Cheap when present: a few lookups.
pub(crate) fn ensure_game_actions(world: &mut World) -> bool {
    let map = world.resource_or_default::<ActionMap>();
    let names = ["move", "look", "jump", "run", PAUSE_ACTION];
    if names.iter().all(|n| map.get(n).is_some()) {
        return false;
    }
    map.merge_missing(&default_game_actions());
    true
}

/// Switches every behaviour on `e` on or off (the `SetEnabled` action).
/// Returns whether `e` had one.
pub fn set_enabled(world: &mut World, e: Entity, enabled: bool) -> bool {
    let mut found = false;
    if let Some(c) = world.component_mut::<Spinner>(e) {
        c.enabled = enabled;
        found = true;
    }
    if let Some(c) = world.component_mut::<Bobber>(e) {
        c.enabled = enabled;
        found = true;
    }
    if let Some(c) = world.component_mut::<Mover>(e) {
        c.enabled = enabled;
        found = true;
    }
    if let Some(c) = world.component_mut::<FollowCamera>(e) {
        c.enabled = enabled;
        found = true;
    }
    if let Some(c) = world.component_mut::<Health>(e) {
        c.enabled = enabled;
        found = true;
    }
    if let Some(c) = world.component_mut::<Spawner>(e) {
        c.enabled = enabled;
        found = true;
    }
    #[cfg(feature = "navigation")]
    {
        if let Some(c) = world.component_mut::<Chaser>(e) {
            c.enabled = enabled;
            found = true;
        }
        if let Some(c) = world.component_mut::<crate::nav_agent::NavAgent>(e) {
            c.enabled = enabled;
            found = true;
        }
    }
    #[cfg(feature = "physics")]
    if let Some(c) = world.component_mut::<PlayerController>(e) {
        c.enabled = enabled;
        found = true;
    }
    found
}

/// The behaviours' systems, save hooks and default actions.
pub struct BehavioursPlugin;

impl Plugin for BehavioursPlugin {
    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<rules::GameState>();
        // The default actions are added by `Game::app` after the game's
        // plugins added theirs (`add_default_actions`), so a game's own
        // `jump` or `move` bindings win.
        {
            let hooks = app.world.resource_or_default::<SaveHooks>();
            hooks.add(MOTION_SAVE_HOOK, motion::save_motion, motion::load_motion);
            hooks.add(
                GAME_SAVE_HOOK,
                rules::save_game_state,
                rules::load_game_state,
            );
        }
        let movers = app
            .system(Stage::FixedUpdate, motion::move_movers)
            .label("movers");
        #[cfg(feature = "physics")]
        movers.before("move_characters");
        #[cfg(not(feature = "physics"))]
        let _ = movers;
        #[cfg(feature = "physics")]
        {
            app.on_add::<PlayerController>(player::on_player_added);
            app.system(Stage::FixedUpdate, player::drive_players)
                .label("player_input")
                .after("movers")
                .before("move_characters");
        }
        #[cfg(feature = "navigation")]
        {
            app.on_add::<Chaser>(chaser::on_chaser_added);
            app.system(Stage::FixedUpdate, chaser::drive_chasers)
                .label("chasers")
                .after("player_input")
                .before("nav_agents");
        }
        app.system(Stage::Update, motion::spin_and_bob)
            .label("spin_and_bob");
        app.system(Stage::Update, health::track_health)
            .label("health")
            .before("interact");
        app.system(Stage::Update, spawner::run_spawners)
            .label("spawners")
            .after("interact");
        let mut rules = rules::RulesSystem::default();
        app.system(Stage::Update, move |w: &mut World| rules.run(w))
            .label("game_rules")
            .after("interact");
        #[cfg(feature = "physics")]
        app.system(Stage::Update, player::animate_players)
            .label("player_animate")
            .after("game_rules");
        let cameras = app
            .system(Stage::Update, follow_camera::follow_cameras)
            .label("follow_camera")
            .after("game_rules");
        #[cfg(feature = "physics")]
        cameras.after("player_animate");
        #[cfg(not(feature = "physics"))]
        let _ = cameras;
    }
}
