//! The built-in presets: ready-made gameplay objects placed with the
//! `add_preset` command (also the editor's Add ▸ Gameplay menu and MCP).
//! Each is a `presets/<name>.preset.json` file compiled into the engine
//! (format: [`box_core::preset`]).

use box_core::preset::Preset;
use box_core::reflect::Registry;

/// The preset files, in menu order.
pub const BUILTIN_PRESETS: &[(&str, &str)] = &[
    ("player", include_str!("../../presets/player.preset.json")),
    ("pickup", include_str!("../../presets/pickup.preset.json")),
    ("hazard", include_str!("../../presets/hazard.preset.json")),
    (
        "moving_platform",
        include_str!("../../presets/moving_platform.preset.json"),
    ),
    (
        "checkpoint",
        include_str!("../../presets/checkpoint.preset.json"),
    ),
    ("goal", include_str!("../../presets/goal.preset.json")),
    (
        "door_to_scene",
        include_str!("../../presets/door_to_scene.preset.json"),
    ),
    (
        "key_and_lock",
        include_str!("../../presets/key_and_lock.preset.json"),
    ),
    ("lever", include_str!("../../presets/lever.preset.json")),
    ("spawner", include_str!("../../presets/spawner.preset.json")),
    ("enemy", include_str!("../../presets/enemy.preset.json")),
];

/// Presets that need the physics module (`PlayerController`).
pub const PHYSICS_PRESETS: &[&str] = &["player"];

/// Presets that need the navigation module (`Chaser`).
pub const NAVIGATION_PRESETS: &[&str] = &["enemy"];

/// Registers [`BUILTIN_PRESETS`]: with `physics`, only those in
/// [`PHYSICS_PRESETS`]; without, the others except
/// [`NAVIGATION_PRESETS`] ([`register_navigation_presets`]).
pub fn register_builtin_presets(registry: &mut Registry, physics: bool) {
    for (name, text) in BUILTIN_PRESETS {
        if PHYSICS_PRESETS.contains(name) != physics || NAVIGATION_PRESETS.contains(name) {
            continue;
        }
        register(registry, name, text);
    }
}

/// Registers [`NAVIGATION_PRESETS`] (navigation module).
pub fn register_navigation_presets(registry: &mut Registry) {
    for (name, text) in BUILTIN_PRESETS {
        if NAVIGATION_PRESETS.contains(name) {
            register(registry, name, text);
        }
    }
}

fn register(registry: &mut Registry, name: &str, text: &str) {
    match Preset::from_json(text) {
        Ok(p) => {
            registry.register_preset(p);
        }
        Err(e) => tracing::error!("built-in preset `{name}` is invalid: {e}"),
    }
}
