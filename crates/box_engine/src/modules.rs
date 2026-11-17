//! The engine's optional modules: `box_engine`'s cargo features `physics`,
//! `dynamics`, `audio` and `script` (all on by default).
//!
//! A native game that leaves one out with `default-features = false` does
//! not compile it, and its registry lacks the module's types. A scene that
//! still uses one loads (the data is kept, as for any unknown type) and
//! [`warn_unknown_types`](crate::warn_unknown_types) names the feature that
//! provides it ([`missing_module`]); a settings file of a left-out module
//! (`assets/settings/PhysicsSettings.json` without `dynamics`) is ignored
//! with a warning ([`left_out_settings_files`]).
//!
//! A native game's binaries write the modules they were built with into
//! `.box/registry.json` (`modules`), so `box check` and `box mcp`, which
//! link every module, validate the project as the game builds it
//! ([`engine_registry_for`](crate::scene_glue::engine_registry_for)) and
//! report left-out module types and settings files as `module_disabled`.

/// One optional engine module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Module {
    /// The `box_engine` cargo feature.
    pub feature: &'static str,
    /// Whether this build includes it.
    pub enabled: bool,
    /// The saved types it adds to [`engine_registry`](crate::engine_registry)
    /// (components, resources and settings; not events, which scenes never
    /// hold), by short name.
    pub types: &'static [&'static str],
}

/// Every optional module, in dependency order (`dynamics` and `script`
/// need `physics`).
pub const MODULES: [Module; 4] = [
    Module {
        feature: "physics",
        enabled: cfg!(feature = "physics"),
        types: &["Collider", "KinematicCharacter"],
    },
    Module {
        feature: "dynamics",
        enabled: cfg!(feature = "dynamics"),
        types: &["Joint", "PhysicsSettings", "RigidBody", "Velocity"],
    },
    Module {
        feature: "audio",
        enabled: cfg!(feature = "audio"),
        types: &["AudioListener", "AudioSettings", "AudioSource"],
    },
    Module {
        feature: "script",
        enabled: cfg!(feature = "script"),
        types: &["Script", "ScriptIntent"],
    },
];

/// The features of the modules this build includes, in [`MODULES`] order.
pub fn enabled_features() -> Vec<&'static str> {
    MODULES
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.feature)
        .collect()
}

/// The module that provides the type named `name`, whether or not this
/// build includes it.
pub fn module_of(name: &str) -> Option<&'static Module> {
    MODULES.iter().find(|m| m.types.contains(&name))
}

/// The settings files (relative to `assets/`) under `assets_dir` that belong
/// to modules `modules` marks as left out, with their module: files the
/// build ignores.
pub fn left_out_settings_files<'m>(
    assets_dir: &std::path::Path,
    modules: &'m [Module],
) -> Vec<(String, &'m Module)> {
    let mut out = Vec::new();
    for m in modules.iter().filter(|m| !m.enabled) {
        for name in m.types {
            let rel = crate::settings::settings_file(name);
            if assets_dir.join(&rel).is_file() {
                out.push((rel, m));
            }
        }
    }
    out
}

/// The log line for a settings file of a left-out module.
pub fn left_out_settings_message(rel: &str, module: &Module) -> String {
    format!(
        "{rel} is ignored: its type belongs to box_engine's `{}` feature, which this build \
         leaves out; enable that feature in the game's Cargo.toml to use it",
        module.feature
    )
}

/// The module that provides the type named `name` when this build leaves
/// that module out; `None` for types of included modules and for types no
/// module provides.
pub fn missing_module(name: &str) -> Option<&'static Module> {
    MODULES
        .iter()
        .find(|m| !m.enabled && m.types.contains(&name))
}

/// The log line for a scene type that belongs to a module this build
/// leaves out: what it is, where, and how to include the module.
pub(crate) fn missing_module_message(
    unknown: &box_core::scene::UnknownType,
    module: &Module,
    scene: &str,
) -> String {
    let what = if unknown.resource {
        format!("resource `{}`", unknown.name)
    } else {
        let s = if unknown.entities == 1 { "y" } else { "ies" };
        format!(
            "component `{}` on {} entit{s}",
            unknown.name, unknown.entities
        )
    };
    format!(
        "scene {scene} uses {what} (data kept, not run): it belongs to box_engine's `{}` \
         feature, which this build leaves out; enable that feature in the game's \
         Cargo.toml to use it",
        module.feature
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_files_of_left_out_modules_are_found() {
        let dir = std::env::temp_dir().join(format!(
            "box_modules_settings_{}",
            box_core::identity::Uid::new()
        ));
        std::fs::create_dir_all(dir.join("settings")).unwrap();
        std::fs::write(dir.join("settings/PhysicsSettings.json"), "{}").unwrap();
        std::fs::write(dir.join("settings/AudioSettings.json"), "{}").unwrap();
        let mut lean = MODULES;
        for m in &mut lean {
            m.enabled = m.feature != "dynamics" && m.feature != "script";
        }
        let found = left_out_settings_files(&dir, &lean);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "settings/PhysicsSettings.json");
        assert_eq!(found[0].1.feature, "dynamics");
        assert!(left_out_settings_message(&found[0].0, found[0].1).contains("`dynamics` feature"));
        assert!(
            left_out_settings_files(&dir, &MODULES)
                .iter()
                .all(|(_, m)| !m.enabled)
        );
        assert_eq!(module_of("RigidBody").map(|m| m.feature), Some("dynamics"));
        assert_eq!(module_of("Transform"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_modules_follow_the_build() {
        assert_eq!(
            missing_module("Script").is_some(),
            !cfg!(feature = "script")
        );
        assert_eq!(
            missing_module("RigidBody").map(|m| m.feature),
            (!cfg!(feature = "dynamics")).then_some("dynamics")
        );
        assert!(missing_module("Transform").is_none());
        assert_eq!(
            enabled_features().contains(&"audio"),
            cfg!(feature = "audio")
        );
    }

    /// The type lists are what each module's registration function adds.
    #[cfg(all(
        feature = "physics",
        feature = "dynamics",
        feature = "audio",
        feature = "script"
    ))]
    #[test]
    fn type_lists_match_the_registrations() {
        use box_core::reflect::Registry;
        fn added(register: fn(&mut Registry)) -> Vec<&'static str> {
            let base: Vec<_> = Registry::new().names().collect();
            let mut r = Registry::new();
            register(&mut r);
            let mut names: Vec<_> = r.names().filter(|n| !base.contains(n)).collect();
            names.sort();
            names
        }
        let mut physics: Vec<_> = MODULES[0]
            .types
            .iter()
            .chain(MODULES[1].types)
            .copied()
            .collect();
        physics.sort();
        assert_eq!(added(box_physics::register_physics_types), physics);
        assert_eq!(added(box_audio::register_audio_types), MODULES[2].types);
        assert_eq!(added(box_script::register_script_types), MODULES[3].types);
    }
}
