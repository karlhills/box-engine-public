//! [`Game`]: the plugins that make up a game, and the registry they share.
//!
//! The editor, the player and Play sessions all start from a `Game`: its
//! [`registry`](Game::registry) loads and saves scenes with the game's types,
//! and [`app`](Game::app) builds a runnable app around a world.
//!
//! ```
//! use box_engine::prelude::*;
//!
//! struct MyGame;
//! impl Plugin for MyGame {
//!     fn build(&self, app: &mut App) {
//!         app.add_system(Stage::Update, |_world| {});
//!     }
//! }
//!
//! let game = Game::new().with_plugin(MyGame);
//! let mut app = game.new_app();
//! app.run_frames(3, 1.0 / 60.0);
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use box_core::app::EditWorldSystems;
use box_core::prelude::*;

use crate::runner::RunOptions;
use crate::scene_glue::{engine_registry, shared_engine_registry};

/// Game plugins plus the type registry built from the engine's types and
/// every plugin's [`Plugin::register`].
#[derive(Clone)]
pub struct Game {
    plugins: Vec<Arc<dyn Plugin>>,
    registry: Arc<Registry>,
    /// Failed registrations, as "plugin `X`: message".
    errors: Arc<Vec<String>>,
    /// The errors were logged (by the first [`Game::app`]).
    reported: Arc<AtomicBool>,
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

impl Game {
    /// The engine alone: no game plugins, the shared engine registry.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            registry: shared_engine_registry(),
            errors: Arc::default(),
            reported: Arc::default(),
        }
    }

    /// Adds a plugin and rebuilds the registry with its types.
    ///
    /// Registration errors (two types with the same short name, a type
    /// without `#[reflect(Component)]`), even ones the plugin ignored, are
    /// kept: [`registration_errors`](Self::registration_errors) returns them
    /// and the first [`app`](Self::app) built from this game logs them as
    /// warnings naming the plugin and the fix (by then the player and the
    /// editor have set up logging).
    pub fn with_plugin(mut self, plugin: impl Plugin + 'static) -> Self {
        self.plugins.push(Arc::new(plugin));
        let mut registry = engine_registry();
        let _ = registry.take_errors();
        let mut errors = Vec::new();
        for p in &self.plugins {
            p.register(&mut registry);
            errors.extend(
                registry
                    .take_errors()
                    .into_iter()
                    .map(|e| format!("plugin `{}`: {e}", p.name())),
            );
        }
        self.registry = Arc::new(registry);
        self.errors = Arc::new(errors);
        self.reported = Arc::default();
        self
    }

    pub fn plugins(&self) -> &[Arc<dyn Plugin>] {
        &self.plugins
    }

    /// Registration errors of every plugin, as "plugin `X`: message".
    pub fn registration_errors(&self) -> &[String] {
        &self.errors
    }

    /// Logs the [`registration_errors`](Self::registration_errors) as errors
    /// naming the fix, once per game (later calls, and the first
    /// [`app`](Self::app), log nothing). The editor calls it at startup so
    /// they show as soon as it opens. Returns how many were logged.
    pub fn report_registration_errors(&self) -> usize {
        if self.reported.swap(true, Ordering::Relaxed) {
            return 0;
        }
        for e in self.errors.iter() {
            tracing::error!(
                "{e}. Saved types need unique short names (scene files and the inspector \
                 address them by name; rename one) and #[reflect(Component)] or \
                 #[reflect(Resource)]"
            );
        }
        self.errors.len()
    }

    /// The registry every world of this game uses.
    pub fn registry(&self) -> Arc<Registry> {
        self.registry.clone()
    }

    /// An empty world using the game's registry, for building scenes in code
    /// or tests (instead of `World::with_registry(game.registry())`).
    pub fn new_world(&self) -> World {
        World::with_registry(self.registry())
    }

    /// An app around `world` ([`crate::app_with_world`]) with every plugin
    /// built into it, in the order they were added. When the world has an
    /// asset server (or a Play world's mirror of one), the project's settings
    /// resources are loaded before the plugins build and its action map file
    /// is applied after ([`crate::settings`]). With the `script` feature the
    /// script runtime (`box_script::ScriptPlugin`, plus the `scenes` script
    /// API) is added last, so scripts run after the game's native systems of
    /// the same stage, except that `on_fixed_update` runs before
    /// `move_characters` (after native systems ordered before it), so script
    /// input moves characters in the same fixed step.
    pub fn app(&self, mut world: World) -> App {
        self.report_registration_errors();
        if !Arc::ptr_eq(world.registry(), &self.registry) {
            world.set_registry(self.registry.clone());
        }
        // Settings before the plugins build, so they can read them; the
        // action map file after, over the plugins' default bindings.
        crate::settings::load_settings(&mut world, crate::settings::Missing::UseDefault);
        let mut app = crate::app_with_world(world);
        for p in &self.plugins {
            app.add_plugin(p.as_ref());
        }
        #[cfg(feature = "script")]
        crate::script_glue::add_scripts(&mut app);
        // The behaviours' actions (`move`, `jump`, `run`, `look`, `pause`)
        // where the game's plugins did not define them, before the file.
        crate::behaviours::add_default_actions(&mut app.world);
        crate::settings::apply_action_file(&mut app.world);
        app
    }

    /// The script APIs the game's plugins add (`app.add_script_api` in
    /// `Plugin::build`) plus the engine's (`scenes`), for generating type
    /// definitions without running the game. Built like
    /// [`rules`](Self::rules).
    #[cfg(feature = "script")]
    pub fn script_apis(&self) -> box_script::ScriptApis {
        let mut app = crate::app_with_world(self.new_world());
        for p in &self.plugins {
            p.build(&mut app);
        }
        crate::script_glue::add_scripts(&mut app);
        app.world
            .remove_resource::<box_script::ScriptApis>()
            .unwrap_or_default()
    }

    /// The luau-lsp definitions file for this game's scripts: every
    /// registered component, resource and event plus the script APIs
    /// ([`script_apis`](Self::script_apis)).
    #[cfg(feature = "script")]
    pub fn script_definitions(&self) -> String {
        let apis: Vec<_> = self.script_apis().iter().cloned().collect();
        box_script::types::definitions(&self.registry, &apis)
    }

    /// The rules the game's plugins add (`app.add_rule` in
    /// `Plugin::build`), fresh, for a world that runs without an `App`: the
    /// editor installs them in its edit world so rules (and the parts they
    /// generate) work while editing; `sync_scene_meshes` runs them. Builds
    /// the plugins into a scratch app that is then dropped, so it is as
    /// cheap as `Plugin::build`. With the `script` feature it also holds
    /// the script rule host (`box_script::rules`), which lists a script
    /// rule once an intent names it. Empty when the game has no rules.
    pub fn rules(&self) -> box_proc::rule::RuleRegistry {
        let mut app = crate::app_with_world(self.new_world());
        for p in &self.plugins {
            p.build(&mut app);
        }
        #[cfg(feature = "script")]
        box_script::add_script_rules(&mut app);
        app.world
            .remove_resource::<box_proc::rule::RuleRegistry>()
            .unwrap_or_default()
    }

    /// The systems the game's plugins marked for an editor's edit world
    /// (`app.add_edit_world_system` in `Plugin::build`, such as box_arch's
    /// anchors), fresh, like [`rules`](Self::rules). The editor installs
    /// them next to the rules and runs them after its rules and generators;
    /// gameplay systems never run there. Empty when no plugin adds one.
    pub fn edit_world_systems(&self) -> EditWorldSystems {
        let mut app = crate::app_with_world(self.new_world());
        for p in &self.plugins {
            p.build(&mut app);
        }
        app.world
            .remove_resource::<EditWorldSystems>()
            .unwrap_or_default()
    }

    /// An app around an empty world with the game's registry: the engine's
    /// resources and plugins, then the game's. For tests (drive it with
    /// `app.run_frames(n, dt)`) and code-only games.
    pub fn new_app(&self) -> App {
        self.app(self.new_world())
    }

    /// Run the game as a code-only app ([`new_app`](Self::new_app)) in a
    /// window titled `title`, or headless with the runner flags
    /// (`--capture <png>`, `--headless`, `--frames`, `--size`, `--report`;
    /// see [`RunOptions`]). To play a project's scene files use
    /// [`crate::player::run_game`] instead.
    pub fn run(&self, title: &str) -> anyhow::Result<()> {
        crate::init_logging();
        crate::crash::install("player");
        crate::run(self.new_app(), RunOptions::new(title))
    }
}

#[cfg(test)]
mod tests {
    // Only the prelude, as a game crate has it: no `bevy_reflect` dependency
    // (box_engine has none), no `#[type_path]`.
    use crate::prelude::*;
    use std::sync::Arc;

    #[derive(Reflect, Default, Clone)]
    #[reflect(Component, Default, Clone)]
    struct Coin {
        #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("pts"), @Step(1.0))]
        value: u32,
    }

    mod clash {
        use crate::prelude::*;

        /// Same short name as the outer `Coin`.
        #[derive(Reflect, Default, Clone)]
        #[reflect(Component, Default, Clone)]
        pub struct Coin;

        pub struct Clashing;
        impl Plugin for Clashing {
            fn register(&self, registry: &mut Registry) {
                registry.register_all::<(Coin,)>();
            }
            fn build(&self, _app: &mut App) {
                info!("built");
            }
        }
    }

    #[derive(Default)]
    struct Ticks(u32);

    struct Coins;
    impl Plugin for Coins {
        fn register(&self, registry: &mut Registry) {
            registry.register::<Coin>().expect("unique name");
        }
        fn build(&self, app: &mut App) {
            app.world.insert_resource(Ticks::default());
            app.add_system(Stage::Update, |w| w.resource_mut::<Ticks>().0 += 1);
        }
    }

    #[test]
    fn plugins_register_types_and_build_into_every_app() {
        let engine = Game::new();
        assert!(engine.registry().component("Coin").is_none());

        let game = Game::new().with_plugin(Coins);
        assert!(game.registry().component("Coin").is_some());
        assert!(game.registry().component("Transform").is_some());
        for _ in 0..2 {
            let mut app = game.app(World::new());
            assert!(Arc::ptr_eq(app.world.registry(), &game.registry()));
            app.tick(1.0 / 60.0);
            assert_eq!(app.world.resource::<Ticks>().0, 1);
        }
        let _ = Coin { value: 1 }.value;
        let meta = game.registry().component("Coin").unwrap().fields();
        assert_eq!(meta[0].unit, Some("pts"));
        assert_eq!(
            game.registry().component("Coin").unwrap().type_path(),
            "box_engine::game::tests::Coin"
        );
    }

    #[test]
    fn new_app_and_new_world_use_the_game_registry() {
        let game = Game::new().with_plugin(Coins);
        let world = game.new_world();
        assert!(Arc::ptr_eq(world.registry(), &game.registry()));
        let mut app = game.new_app();
        assert!(Arc::ptr_eq(app.world.registry(), &game.registry()));
        app.run_frames(3, 1.0 / 60.0);
        assert_eq!(app.world.resource::<Ticks>().0, 3);
        assert!(app.world.contains_resource::<Environment>());
    }

    #[test]
    fn registration_errors_are_surfaced_with_the_plugin_name() {
        let game = Game::new().with_plugin(Coins).with_plugin(clash::Clashing);
        let errors = game.registration_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("Clashing"), "{errors:?}");
        assert!(
            errors[0].contains("`Coin` is already registered"),
            "{errors:?}"
        );
        assert!(
            Game::new()
                .with_plugin(Coins)
                .registration_errors()
                .is_empty()
        );
    }

    #[test]
    fn a_plugin_added_to_an_app_without_its_types_is_detected() {
        let mut app = crate::default_app();
        let missing = box_core::app::unregistered_types(&Coins, app.world.registry());
        assert_eq!(missing, vec!["Coin"]);
        app.add_plugin(&Coins); // warns once, still builds
        app.update();
        assert_eq!(app.world.resource::<Ticks>().0, 1);
        let game = Game::new().with_plugin(Coins);
        assert!(box_core::app::unregistered_types(&Coins, &game.registry()).is_empty());
    }
}
