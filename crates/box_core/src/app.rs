//! The app loop: plugins, ordered system lists and the phases of a frame.
//!
//! A frame ([`App::tick`]) runs, in order:
//! 1. frame start: event queues advance ([`App::add_event`]), but only on
//!    frames where a fixed step ran since the last advance (see below);
//! 2. `FixedUpdate` zero or more times, at [`FixedTime::step`] (60 Hz by
//!    default), catching up on accumulated time up to
//!    [`FixedTime::max_steps`]. While a fixed step runs, [`Time::delta`] is
//!    the step and [`Time::elapsed`] the simulated time, so systems can read
//!    `Time` in either stage. After each step the transforms of
//!    [`Interpolated`](crate::interpolate::Interpolated) entities are
//!    recorded;
//! 3. `Update` once;
//! 4. `PostUpdate` once: transform propagation first, then user systems,
//!    then interpolated entities are drawn between their last two fixed-step
//!    transforms ([`interpolate`](crate::interpolate)).
//!
//! [`Time::scale`] and [`Time::paused`] slow down or stop game time: the
//! fixed accumulator advances by the scaled delta.
//!
//! **Around every system** (and at the start of each stage), the app applies
//! the world's deferred commands ([`World::defer`]) and then runs the
//! [`on_add`](App::on_add) / [`on_remove`](App::on_remove) hooks, so a
//! system never sees a hooked component whose `on_add` has not run, except
//! on entities it added itself.
//!
//! **Order within a stage** is the order systems were added, adjusted by
//! name with [`App::system`] (`.after("name")`, `.before("name")`).
//!
//! **Events and fixed steps.** At display rates above the fixed rate
//! (120/144/240 Hz) most frames run no fixed step. Event queues therefore
//! advance only on frames where at least one fixed step ran since the last
//! advance, so an event sent in `Update` or `PostUpdate` is still held when
//! the next `FixedUpdate` runs. Readers keep an
//! [`EventCursor`](crate::event::EventCursor) (or use [`App::on_event`],
//! which keeps it for them), so `Update` readers still see each event
//! exactly once. Apps without `FixedUpdate` systems, frames where game time
//! is paused, and frames run with [`App::update`] (no time), advance every
//! frame.
//!
//! [`App::new`] is the low-level constructor (core resources only). Games
//! start from `box_engine::Game` (or `box_engine::default_app`), which adds
//! the engine's resources, plugins and type registry.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::event::{EventCursor, Events};
use crate::hooks::Hooks;
use crate::input::Input;
use crate::reflect::Registry;
use crate::schedule::{StageSystems, System};
use crate::time::Time;
use crate::transform::propagate_transforms;
use crate::world::{Component, Entity, World};

pub use crate::schedule::{OrderError, SystemBuilder, current_system};

/// Where a system runs in the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Simulation at a fixed rate, zero or more times per frame, before
    /// `Update`. Inside it [`Time::delta`] is the fixed step
    /// ([`FixedTime::step`]).
    FixedUpdate,
    /// Gameplay and tools, once per frame; read [`Time::delta`].
    Update,
    /// Derived state after gameplay: transform propagation runs first, then
    /// user PostUpdate systems.
    PostUpdate,
}

/// Set `AppExit(true)` to ask the runner to quit after the current frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AppExit(pub bool);

/// The fixed-rate clock that drives [`Stage::FixedUpdate`].
#[derive(Clone, Copy, Debug)]
pub struct FixedTime {
    /// Seconds per fixed step.
    pub step: f32,
    /// At most this many steps run in one frame; time beyond that is dropped
    /// so a slow frame cannot snowball.
    pub max_steps: u32,
    /// Unspent time, below `step` after each frame.
    pub accumulator: f32,
    /// Steps run in the current frame.
    pub steps_this_frame: u32,
    /// Fixed steps run so far.
    pub tick: u64,
}

impl Default for FixedTime {
    fn default() -> Self {
        Self {
            step: 1.0 / 60.0,
            max_steps: 5,
            accumulator: 0.0,
            steps_this_frame: 0,
            tick: 0,
        }
    }
}

impl FixedTime {
    /// How far the current frame is between two fixed steps, in 0..1; used
    /// to draw [`Interpolated`](crate::interpolate::Interpolated) entities.
    pub fn alpha(&self) -> f32 {
        if self.step > 0.0 {
            (self.accumulator / self.step).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A unit of engine or game functionality: registers types and adds
/// resources, events and systems. Engine modules and games use the same trait.
///
/// ```
/// use box_core::prelude::*;
///
/// struct Spin;
/// impl Plugin for Spin {
///     fn build(&self, app: &mut App) {
///         app.add_system(Stage::Update, |w| {
///             let dt = w.resource::<Time>().delta;
///             for t in w.query_mut::<&mut Transform>() {
///                 t.rotation *= Quat::from_rotation_y(dt);
///             }
///         });
///     }
/// }
///
/// let mut app = App::new();
/// app.add_plugin(&Spin);
/// app.tick(1.0 / 60.0);
/// ```
pub trait Plugin {
    /// A readable name for logs and errors.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// Register this plugin's saved component and resource types (usually
    /// `registry.register_all::<(A, B)>()`). Called once when the registry is
    /// built (`box_engine::Game::with_plugin`, which logs registration
    /// errors), before any world uses it, so editors and players load the
    /// plugin's scene data and show it in the inspector. [`App::add_plugin`]
    /// does not register; it warns when the app's registry lacks these types.
    fn register(&self, _registry: &mut Registry) {}

    /// Add resources, events and systems to an app. Called for every app the
    /// plugin runs in (each Play session builds a fresh one).
    fn build(&self, app: &mut App);
}

/// Resource: the systems added with [`App::add_edit_world_system`], by
/// label. An editor's edit world has no `App` and never runs gameplay; it
/// runs only these systems (after its rules and generators), so derived
/// state that follows saved data (anchored decorations following a wall)
/// stays correct while editing. Systems listed here must be change-driven,
/// read only saved data plus their own caches, and never move entities on
/// their own.
#[derive(Clone, Default)]
pub struct EditWorldSystems {
    systems: Vec<(&'static str, EditWorldSystem)>,
}

/// A system an editor's edit world runs (see [`EditWorldSystems`]).
pub type EditWorldSystem = fn(&mut World);

impl EditWorldSystems {
    /// Adds `system` as `label` (a label added before is replaced).
    pub fn add(&mut self, label: &'static str, system: fn(&mut World)) {
        self.systems.retain(|(l, _)| *l != label);
        self.systems.push((label, system));
    }

    /// The labels, in the order the systems run.
    pub fn labels(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.systems.iter().map(|(l, _)| *l)
    }

    pub fn len(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Runs every system once on `world`, in order.
    pub fn run(&self, world: &mut World) {
        for (_label, system) in &self.systems {
            profiling::scope!("edit world system", _label);
            system(world);
        }
    }

    /// Runs the systems of the world's own `EditWorldSystems` resource (a
    /// no-op without one). Returns how many ran.
    pub fn run_in(world: &mut World) -> usize {
        let Some(systems) = world.remove_resource::<EditWorldSystems>() else {
            return 0;
        };
        systems.run(world);
        let n = systems.len();
        world.insert_resource(systems);
        n
    }
}

/// Short names of the types `plugin` registers ([`Plugin::register`], run
/// on a scratch registry) that `registry` lacks. Empty when the app's
/// registry was built with the plugin.
pub fn unregistered_types(plugin: &dyn Plugin, registry: &Registry) -> Vec<&'static str> {
    let mut scratch = Registry::new();
    plugin.register(&mut scratch);
    scratch.names().filter(|n| !registry.contains(n)).collect()
}

/// A world plus the systems that update it.
pub struct App {
    pub world: World,
    event_updates: Vec<System>,
    fixed_update: StageSystems,
    update: StageSystems,
    post_update: StageSystems,
    hooks: Hooks,
    /// A fixed step ran since the event queues last advanced.
    fixed_ran_since_events: bool,
    /// Simulated seconds: the sum of the steps run.
    fixed_elapsed: f64,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// A bare app with `Time`, `FixedTime`, `Input` and `AppExit` resources
    /// and transform propagation at the start of PostUpdate.
    ///
    /// Low level: games start from `box_engine::Game` (`game.new_app()`) or
    /// `box_engine::default_app()`, which add the engine's resources, plugins
    /// and type registry.
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(Time::default());
        world.insert_resource(FixedTime::default());
        world.insert_resource(Input::default());
        world.insert_resource(AppExit::default());
        let mut app = Self {
            world,
            event_updates: Vec::new(),
            fixed_update: StageSystems::default(),
            update: StageSystems::default(),
            post_update: StageSystems::default(),
            hooks: Hooks::default(),
            fixed_ran_since_events: false,
            fixed_elapsed: 0.0,
        };
        app.post_update.push(System::new(propagate_transforms));
        app
    }

    pub(crate) fn stage_systems(&mut self, stage: Stage) -> &mut StageSystems {
        match stage {
            Stage::FixedUpdate => &mut self.fixed_update,
            Stage::Update => &mut self.update,
            Stage::PostUpdate => &mut self.post_update,
        }
    }

    /// Add a system. Systems in a stage run in the order they were added
    /// (use [`App::system`] to order by name instead). The system's type
    /// name (`my_game::move_players` for a function) names it in profiler
    /// scopes, ordering constraints and [`current_system`].
    pub fn add_system<F: FnMut(&mut World) + 'static>(
        &mut self,
        stage: Stage,
        system: F,
    ) -> &mut Self {
        self.stage_systems(stage).push(System::new(system));
        self
    }

    /// Add a system and return a [`SystemBuilder`] to label it and order it
    /// by name: `app.system(Stage::Update, aim).after("move_players")`.
    pub fn system<F: FnMut(&mut World) + 'static>(
        &mut self,
        stage: Stage,
        system: F,
    ) -> SystemBuilder<'_> {
        let seq = self.stage_systems(stage).push(System::new(system));
        SystemBuilder::new(self, stage, seq)
    }

    /// [`App::system`] labelled `label`, also recorded in the world's
    /// [`EditWorldSystems`] so an editor runs it in its edit world too (which
    /// otherwise runs only rules and generators, never gameplay). For
    /// change-driven systems that keep derived state in step with saved data,
    /// such as attachment anchors.
    pub fn add_edit_world_system(
        &mut self,
        stage: Stage,
        label: &'static str,
        system: fn(&mut World),
    ) -> SystemBuilder<'_> {
        self.world
            .resource_or_default::<EditWorldSystems>()
            .add(label, system);
        self.system(stage, system).label(label)
    }

    /// Order every stage now and return the first [`OrderError`] (a cycle or
    /// a name that matches no system). Frames do the same lazily, logging
    /// the error once and running that stage in insertion order; call this
    /// in tests to fail loudly.
    pub fn check_order(&mut self) -> Result<(), OrderError> {
        for stage in [Stage::FixedUpdate, Stage::Update, Stage::PostUpdate] {
            self.stage_systems(stage).check(stage)?;
        }
        Ok(())
    }

    /// Names of a stage's systems, in run order (PostUpdate starts with
    /// transform propagation).
    pub fn system_names(&self, stage: Stage) -> Vec<&'static str> {
        let list = match stage {
            Stage::FixedUpdate => &self.fixed_update,
            Stage::Update => &self.update,
            Stage::PostUpdate => &self.post_update,
        };
        list.names(stage)
    }

    /// Run `f(world, entity)` once for every entity that newly has a `T`:
    /// spawned with it, given it with `insert`, or loaded by a scene (a
    /// scene switch included). The usual place to add runtime state to saved
    /// components (`PlayerController` → `PlayerState`), replacing a
    /// `.without::<State>()` pass in a system.
    ///
    /// **When.** At the start of each stage and after every system (after
    /// its [deferred commands](World::defer)), so systems never see a `T`
    /// whose `on_add` has not run, except on entities they added
    /// themselves. A component removed and re-added between two of these
    /// points counts as kept. Several callbacks for one type run in the
    /// order they were added.
    ///
    /// **Cost.** Entities are found through a runtime marker (never saved),
    /// so a check costs O(archetypes) and the work is proportional to new
    /// entities, never to all entities.
    ///
    /// ```
    /// use box_core::prelude::*;
    ///
    /// struct Enemy;
    /// struct Brain { target: Option<Entity> }
    ///
    /// let mut app = App::new();
    /// app.on_add::<Enemy>(|world, e| {
    ///     let _ = world.insert_one(e, Brain { target: None });
    /// });
    /// let e = app.world.spawn((Enemy,));
    /// app.update();
    /// assert!(app.world.has::<Brain>(e));
    /// ```
    pub fn on_add<T: Component>(
        &mut self,
        f: impl FnMut(&mut World, Entity) + 'static,
    ) -> &mut Self {
        let name = callback_name(&f);
        self.hooks.add::<T>(name, Box::new(f));
        self
    }

    /// Run `f(world, entity)` for every entity that lost its `T`: the
    /// component was removed, or the entity was despawned (directly, with a
    /// despawned ancestor, or by a scene switch). Runs at the same points as
    /// [`on_add`](Self::on_add), after the change. The component is already
    /// gone, and after a despawn so is the entity (`world.contains(entity)`
    /// is false): use it to clean up state kept elsewhere (maps keyed by
    /// entity, counters, external handles). Despawns through
    /// `world.ecs_mut()` are not seen.
    pub fn on_remove<T: Component>(
        &mut self,
        f: impl FnMut(&mut World, Entity) + 'static,
    ) -> &mut Self {
        let name = callback_name(&f);
        self.hooks.remove::<T>(&mut self.world, name, Box::new(f));
        self
    }

    /// Call `f(world, &event)` for every `T` event, once each, in a system
    /// in `stage`: the system owns the [`EventCursor`] and clones the events
    /// out before calling, so `f` gets `&mut World`. Adds the event type
    /// ([`add_event`](Self::add_event)) if needed. Send with
    /// [`World::send`]. In `FixedUpdate` an event is seen at the first
    /// fixed step after it was sent; in `Update`/`PostUpdate` at the first
    /// frame. Returns a [`SystemBuilder`] to label or order the system.
    ///
    /// ```
    /// use box_core::prelude::*;
    ///
    /// #[derive(Clone)]
    /// struct Hit { damage: u32 }
    ///
    /// let mut app = App::new();
    /// app.world.insert_resource(100u32);
    /// app.on_event(Stage::Update, |w: &mut World, hit: &Hit| {
    ///     *w.resource_mut::<u32>() -= hit.damage;
    /// });
    /// app.world.send(Hit { damage: 30 });
    /// app.update();
    /// assert_eq!(*app.world.resource::<u32>(), 70);
    /// ```
    pub fn on_event<T, F>(&mut self, stage: Stage, mut f: F) -> SystemBuilder<'_>
    where
        T: Clone + 'static,
        F: FnMut(&mut World, &T) + 'static,
    {
        self.add_event::<T>();
        let mut cursor = EventCursor::<T>::default();
        let run = move |world: &mut World| {
            let Some(events) = world.get_resource::<Events<T>>() else {
                return;
            };
            let batch: Vec<T> = cursor.read(events).cloned().collect();
            for event in &batch {
                f(world, event);
                world.apply_deferred();
            }
        };
        let seq = self
            .stage_systems(stage)
            .push(System::named(std::any::type_name::<F>(), run));
        SystemBuilder::new(self, stage, seq)
    }

    /// Run `plugin.build` on this app: `app.add_plugin(&MyGame)` (a borrowed
    /// temporary works too: `app.add_plugin(&Tuning::new(2.0))`).
    ///
    /// Type registration ([`Plugin::register`]) happens separately, when the
    /// registry is built (`box_engine::Game::with_plugin`). If this app's
    /// registry lacks types the plugin registers, they would silently not be
    /// saved or shown in the inspector, so a warning is logged (once per
    /// plugin).
    pub fn add_plugin(&mut self, plugin: &dyn Plugin) -> &mut Self {
        let missing = unregistered_types(plugin, self.world.registry());
        if !missing.is_empty() {
            warn_unregistered(plugin.name(), &missing);
        }
        plugin.build(self);
        self
    }

    /// Insert an [`Events<T>`] resource (if missing) and advance it at frame
    /// start (see the [module docs](self) for which frames). Adding the same
    /// event type twice is harmless.
    pub fn add_event<T: 'static>(&mut self) -> &mut Self {
        if self.world.get_resource::<Events<T>>().is_none() {
            self.world.insert_resource(Events::<T>::default());
            self.event_updates.push(System::new(|w: &mut World| {
                if let Some(e) = w.get_resource_mut::<Events<T>>() {
                    e.update();
                }
            }));
        }
        self
    }

    /// Run one frame without advancing time: frame start (event queues always
    /// advance), any fixed steps already due, Update and PostUpdate.
    #[profiling::function]
    pub fn update(&mut self) {
        self.frame(false);
    }

    fn frame(&mut self, timed: bool) {
        if !timed || self.events_may_advance() {
            profiling::scope!("FrameStart");
            for s in &mut self.event_updates {
                s.run(&mut self.world);
            }
            self.fixed_ran_since_events = false;
        }
        self.run_fixed_steps();
        {
            profiling::scope!("Update");
            run_stage(
                &mut self.world,
                &mut self.hooks,
                &mut self.update,
                Stage::Update,
            );
        }
        {
            profiling::scope!("PostUpdate");
            crate::interpolate::restore(&mut self.world);
            run_stage(
                &mut self.world,
                &mut self.hooks,
                &mut self.post_update,
                Stage::PostUpdate,
            );
            let alpha = self
                .world
                .get_resource::<FixedTime>()
                .map_or(1.0, FixedTime::alpha);
            crate::interpolate::blend(&mut self.world, alpha);
        }
    }

    /// Event queues advance unless a fixed step may still need to read them:
    /// a fixed step ran since the last advance, or the app has no running
    /// fixed clock (no fixed systems, no step, or game time stopped).
    fn events_may_advance(&self) -> bool {
        self.fixed_ran_since_events
            || self.fixed_update.is_empty()
            || self
                .world
                .get_resource::<FixedTime>()
                .is_none_or(|f| f.step <= 0.0)
            || self
                .world
                .get_resource::<Time>()
                .is_some_and(|t| t.paused || t.scale <= 0.0)
    }

    /// Advance time by `dt` real seconds (scaled by [`Time::scale`], zero
    /// while [`Time::paused`]), then run one frame.
    #[profiling::function]
    pub fn tick(&mut self, dt: f32) {
        let dt = {
            let time = self.world.resource_mut::<Time>();
            time.advance(dt);
            time.delta
        };
        if let Some(fixed) = self.world.get_resource_mut::<FixedTime>() {
            fixed.accumulator += dt;
        }
        self.frame(true);
    }

    /// Run `n` frames of `dt` seconds as the runner does:
    /// [`tick`](Self::tick), then clear the frame's input edges
    /// ([`Input::end_frame`]). For gameplay tests: press a key on the `Input`
    /// resource, run frames, assert.
    pub fn run_frames(&mut self, n: usize, dt: f32) {
        for _ in 0..n {
            self.tick(dt);
            if let Some(input) = self.world.get_resource_mut::<Input>() {
                input.end_frame();
            }
        }
    }

    fn run_fixed_steps(&mut self) {
        profiling::scope!("FixedUpdate");
        let Some(mut fixed) = self.world.get_resource_mut::<FixedTime>().map(|f| *f) else {
            return;
        };
        fixed.steps_this_frame = 0;
        if fixed.step <= 0.0 {
            *self.world.resource_mut::<FixedTime>() = fixed;
            return;
        }
        let frame_time = self.world.get_resource::<Time>().copied();
        while fixed.accumulator >= fixed.step && fixed.steps_this_frame < fixed.max_steps {
            fixed.accumulator -= fixed.step;
            fixed.steps_this_frame += 1;
            fixed.tick += 1;
            self.fixed_elapsed += f64::from(fixed.step);
            *self.world.resource_mut::<FixedTime>() = fixed;
            // Inside a fixed step `Time` reads as the fixed clock.
            if let Some(time) = self.world.get_resource_mut::<Time>() {
                time.delta = fixed.step;
                time.elapsed_f64 = self.fixed_elapsed;
                time.elapsed = self.fixed_elapsed as f32;
            }
            crate::interpolate::before_step(&mut self.world);
            run_stage(
                &mut self.world,
                &mut self.hooks,
                &mut self.fixed_update,
                Stage::FixedUpdate,
            );
            crate::interpolate::record(&mut self.world);
            // Systems may change the step; pick up their changes.
            let current = *self.world.resource::<FixedTime>();
            fixed.step = current.step;
            fixed.max_steps = current.max_steps;
        }
        if fixed.steps_this_frame > 0 {
            self.fixed_ran_since_events = true;
            if let (Some(saved), Some(time)) = (frame_time, self.world.get_resource_mut::<Time>()) {
                time.delta = saved.delta;
                time.elapsed_f64 = saved.elapsed_f64;
                time.elapsed = saved.elapsed;
            }
        }
        if fixed.steps_this_frame == fixed.max_steps && fixed.accumulator >= fixed.step {
            fixed.accumulator %= fixed.step;
        }
        *self.world.resource_mut::<FixedTime>() = fixed;
    }

    pub fn exit_requested(&self) -> bool {
        self.world.get_resource::<AppExit>().is_some_and(|e| e.0)
    }
}

/// The name a hook callback is reported under: its closure or function
/// type name.
fn callback_name<F>(_: &F) -> &'static str {
    std::any::type_name::<F>()
}

/// Run a stage's systems in order. Deferred commands are applied and hooks
/// run at the start and after every system.
fn run_stage(world: &mut World, hooks: &mut Hooks, systems: &mut StageSystems, stage: Stage) {
    world.apply_deferred();
    hooks.run(world);
    for s in systems.ordered(stage) {
        s.run(world);
        world.apply_deferred();
        hooks.run(world);
    }
}

/// Logs the [`App::add_plugin`] registry warning, once per plugin name.
fn warn_unregistered(plugin: &str, missing: &[&str]) {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let first = WARNED
        .lock()
        .map(|mut w| {
            w.get_or_insert_with(HashSet::new)
                .insert(plugin.to_string())
        })
        .unwrap_or(true);
    if first {
        let names: Vec<String> = missing.iter().map(|n| format!("`{n}`")).collect();
        tracing::warn!(
            "plugin `{plugin}` registers {} but this app's registry lacks them, so they are \
             neither saved nor shown in the inspector; build the app with \
             Game::new().with_plugin({plugin}) and game.new_app() / game.app(world)",
            names.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventCursor;
    use crate::transform::{GlobalTransform, Transform};

    #[derive(Default)]
    struct Log(Vec<&'static str>);

    #[test]
    fn systems_run_in_stage_and_insertion_order() {
        let mut app = App::new();
        app.world.insert_resource(Log::default());
        app.add_system(Stage::PostUpdate, |w| {
            w.resource_mut::<Log>().0.push("post")
        });
        app.add_system(Stage::Update, |w| w.resource_mut::<Log>().0.push("u1"));
        app.add_system(Stage::FixedUpdate, |w| {
            w.resource_mut::<Log>().0.push("fixed")
        });
        app.add_system(Stage::Update, |w| w.resource_mut::<Log>().0.push("u2"));
        app.tick(1.0 / 60.0);
        assert_eq!(
            app.world.resource::<Log>().0,
            vec!["fixed", "u1", "u2", "post"]
        );
    }

    #[test]
    fn edit_world_systems_run_in_the_app_and_are_listed() {
        let mut app = App::new();
        app.world.insert_resource(Log::default());
        app.add_edit_world_system(Stage::PostUpdate, "mark", |w| {
            w.resource_mut::<Log>().0.push("mark")
        });
        app.add_system(Stage::Update, |w| w.resource_mut::<Log>().0.push("game"));
        app.update();
        assert_eq!(app.world.resource::<Log>().0, vec!["game", "mark"]);
        // A world without an App (an editor's edit world) runs only them.
        let systems = app.world.resource::<EditWorldSystems>().clone();
        assert_eq!(systems.labels().collect::<Vec<_>>(), vec!["mark"]);
        let mut edit = World::new();
        edit.insert_resource(Log::default());
        edit.insert_resource(systems);
        assert_eq!(EditWorldSystems::run_in(&mut edit), 1);
        assert_eq!(edit.resource::<Log>().0, vec!["mark"]);
    }

    #[test]
    fn transforms_propagate_before_post_update_systems() {
        let mut app = App::new();
        let e = app.world.spawn((Transform::from_xyz(1.0, 2.0, 3.0),));
        app.add_system(Stage::PostUpdate, move |w| {
            let g = w
                .get::<GlobalTransform>(e)
                .expect("global transform present");
            assert_eq!(g.translation(), glam::Vec3::new(1.0, 2.0, 3.0));
        });
        app.update();
    }

    #[test]
    fn headless_app_runs_100_frames() {
        let mut app = App::new();
        let e = app.world.spawn((Transform::default(),));
        app.add_system(Stage::Update, move |w| {
            let dt = w.resource::<Time>().delta;
            if let Some(mut t) = w.get_mut::<Transform>(e) {
                t.translation.x += dt;
            }
        });
        for _ in 0..100 {
            app.tick(0.01);
        }
        let time = app.world.resource::<Time>();
        assert_eq!(time.frame, 100);
        assert!((time.elapsed - 1.0).abs() < 1e-4);
        let x = app.world.get::<GlobalTransform>(e).unwrap().translation().x;
        assert!((x - 1.0).abs() < 1e-4);
    }

    #[derive(Default)]
    struct Steps(u32);

    fn fixed_app() -> App {
        let mut app = App::new();
        app.world.insert_resource(Steps::default());
        app.add_system(Stage::FixedUpdate, |w| w.resource_mut::<Steps>().0 += 1);
        app
    }

    #[test]
    fn fixed_update_runs_at_its_own_rate() {
        let mut app = fixed_app();
        // 60 Hz frames: exactly one step each.
        for _ in 0..60 {
            app.tick(1.0 / 60.0);
        }
        assert_eq!(app.world.resource::<Steps>().0, 60);
        // 30 Hz frames: two steps each.
        for _ in 0..30 {
            app.tick(1.0 / 30.0);
            assert_eq!(app.world.resource::<FixedTime>().steps_this_frame, 2);
        }
        assert_eq!(app.world.resource::<Steps>().0, 120);
        // 120 Hz frames: a step every other frame.
        let mut app = fixed_app();
        for _ in 0..120 {
            app.tick(1.0 / 120.0);
        }
        let steps = app.world.resource::<Steps>().0;
        assert!((59..=60).contains(&steps), "{steps}");
        assert_eq!(app.world.resource::<FixedTime>().tick, steps as u64);
    }

    #[test]
    fn a_slow_frame_runs_at_most_max_steps_and_drops_the_rest() {
        let mut app = fixed_app();
        app.tick(0.2); // 12 steps due, 5 allowed
        assert_eq!(app.world.resource::<Steps>().0, 5);
        let fixed = *app.world.resource::<FixedTime>();
        assert!(fixed.accumulator < fixed.step, "{}", fixed.accumulator);
        assert!((0.0..=1.0).contains(&fixed.alpha()));
    }

    #[test]
    fn update_without_time_runs_no_fixed_step() {
        let mut app = fixed_app();
        app.update();
        assert_eq!(app.world.resource::<Steps>().0, 0);
    }

    struct Hit(u32);

    #[test]
    fn events_sent_in_fixed_update_are_read_once_in_update() {
        let mut app = App::new();
        app.add_event::<Hit>().add_event::<Hit>();
        app.add_system(Stage::FixedUpdate, |w| {
            let tick = w.resource::<FixedTime>().tick as u32;
            w.resource_mut::<Events<Hit>>().send(Hit(tick));
        });
        app.world.insert_resource(Vec::<u32>::new());
        let mut cursor = EventCursor::<Hit>::default();
        app.add_system(Stage::Update, move |w| {
            let got: Vec<u32> = cursor
                .read(w.resource::<Events<Hit>>())
                .map(|h| h.0)
                .collect();
            w.resource_mut::<Vec<u32>>().extend(got);
        });
        app.tick(1.0 / 30.0); // two fixed steps
        app.tick(1.0 / 60.0);
        app.tick(0.0);
        assert_eq!(*app.world.resource::<Vec<u32>>(), vec![1, 2, 3]);
    }

    struct Counter;
    impl Plugin for Counter {
        fn build(&self, app: &mut App) {
            app.world.insert_resource(Steps::default());
            app.add_system(Stage::Update, |w| w.resource_mut::<Steps>().0 += 1);
        }
    }

    #[test]
    fn a_plugin_builds_into_each_app() {
        for _ in 0..2 {
            let mut app = App::new();
            app.add_plugin(&Counter);
            app.update();
            assert_eq!(app.world.resource::<Steps>().0, 1);
        }
        assert!(Counter.name().ends_with("Counter"));
    }

    #[derive(Default)]
    struct Received {
        fixed: Vec<u64>,
        update: Vec<u64>,
    }

    /// One event per frame from Update and one per fixed step from
    /// FixedUpdate, each read by a reader in the other stage.
    fn event_app() -> App {
        let mut app = App::new();
        app.add_event::<Hit>().add_event::<u64>();
        app.world.insert_resource(Received::default());
        app.add_system(Stage::Update, |w| {
            let frame = w.resource::<Time>().frame;
            w.resource_mut::<Events<u64>>().send(frame);
        });
        app.add_system(Stage::FixedUpdate, |w| {
            let tick = w.resource::<FixedTime>().tick as u32;
            w.resource_mut::<Events<Hit>>().send(Hit(tick));
        });
        let mut frames = EventCursor::<u64>::default();
        app.add_system(Stage::FixedUpdate, move |w| {
            let got: Vec<u64> = frames.read(w.resource::<Events<u64>>()).copied().collect();
            w.resource_mut::<Received>().fixed.extend(got);
        });
        let mut ticks = EventCursor::<Hit>::default();
        app.add_system(Stage::Update, move |w| {
            let got: Vec<u64> = ticks
                .read(w.resource::<Events<Hit>>())
                .map(|h| u64::from(h.0))
                .collect();
            w.resource_mut::<Received>().update.extend(got);
        });
        app
    }

    #[test]
    fn fixed_update_readers_see_every_event_at_any_frame_rate() {
        for hz in [30.0_f32, 60.0, 144.0, 240.0] {
            let mut app = event_app();
            let frames = (2.0 * hz) as u64;
            for _ in 0..frames {
                app.tick(1.0 / hz);
            }
            let sent = app.world.resource::<Time>().frame;
            let ticks = app.world.resource::<FixedTime>().tick;
            // Events sent after the last fixed step are still pending: a
            // fixed reader gets them at its next step.
            let r = app.world.resource::<Received>();
            let pending = r.fixed.last().map_or(sent, |last| sent - last);
            assert!(
                pending <= (hz / 60.0).ceil() as u64,
                "{hz} Hz: {pending} pending"
            );
            assert_eq!(r.fixed, (1..=sent - pending).collect::<Vec<_>>(), "{hz} Hz");
            assert_eq!(r.update, (1..=ticks).collect::<Vec<_>>(), "{hz} Hz");
            assert!(ticks >= 119, "{hz} Hz: {ticks} steps");
        }
    }

    #[test]
    fn apps_without_fixed_systems_advance_events_every_frame() {
        let mut app = App::new();
        app.add_event::<Hit>();
        app.add_system(Stage::Update, |w| {
            w.resource_mut::<Events<Hit>>().send(Hit(0))
        });
        for _ in 0..10 {
            app.tick(1.0 / 240.0);
        }
        // This frame's event and last frame's.
        assert_eq!(app.world.resource::<Events<Hit>>().len(), 2);
    }

    #[derive(Default)]
    struct Seen(Vec<(f32, f64)>);

    #[test]
    fn time_inside_fixed_update_is_the_fixed_clock() {
        let mut app = App::new();
        app.world.insert_resource(Seen::default());
        app.add_system(Stage::FixedUpdate, |w| {
            let t = *w.resource::<Time>();
            w.resource_mut::<Seen>().0.push((t.delta, t.elapsed_f64));
        });
        app.tick(1.0 / 30.0); // two steps
        app.tick(1.0 / 120.0);
        app.tick(1.0 / 120.0); // one step
        let step = 1.0 / 60.0;
        let seen = &app.world.resource::<Seen>().0;
        assert_eq!(seen.len(), 3);
        for (i, (delta, elapsed)) in seen.iter().enumerate() {
            assert_eq!(*delta, step);
            assert!((elapsed - (i + 1) as f64 * f64::from(step)).abs() < 1e-9);
        }
        // Outside FixedUpdate, Time is the frame clock again.
        let time = app.world.resource::<Time>();
        assert_eq!(time.delta, 1.0 / 120.0);
        assert!((time.elapsed_f64 - (1.0 / 30.0 + 2.0 / 120.0)).abs() < 1e-6);
        // FixedTime keeps working.
        assert_eq!(app.world.resource::<FixedTime>().tick, 3);
    }

    fn named_system(w: &mut World) {
        w.resource_mut::<Steps>().0 += 1;
    }

    fn panicking_system(_: &mut World) {
        panic!("boom");
    }

    #[test]
    fn systems_are_named_and_a_panicking_one_is_reported() {
        let mut app = App::new();
        app.world.insert_resource(Steps::default());
        app.add_system(Stage::Update, named_system);
        app.add_system(Stage::Update, |_w| {});
        let names = app.system_names(Stage::Update);
        assert_eq!(names[0], "box_core::app::tests::named_system");
        assert!(names[1].contains("{{closure}}"), "{names:?}");
        assert!(app.system_names(Stage::PostUpdate)[0].ends_with("propagate_transforms"));
        app.update();
        assert_eq!(current_system(), None);

        app.add_system(Stage::PostUpdate, panicking_system);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()));
        assert!(caught.is_err());
        assert_eq!(
            current_system(),
            Some("box_core::app::tests::panicking_system")
        );
    }

    #[test]
    fn run_frames_ticks_and_clears_input_edges() {
        let mut app = App::new();
        app.world.insert_resource(Steps::default());
        app.add_system(Stage::Update, |w| {
            if w.resource::<Input>()
                .key_pressed(crate::input::KeyCode::Space)
            {
                w.resource_mut::<Steps>().0 += 1;
            }
        });
        app.world
            .resource_mut::<Input>()
            .press_key(crate::input::KeyCode::Space);
        app.run_frames(5, 0.01);
        assert_eq!(app.world.resource::<Steps>().0, 1, "pressed once");
        assert_eq!(app.world.resource::<Time>().frame, 5);
        assert!(
            app.world
                .resource::<Input>()
                .key_down(crate::input::KeyCode::Space)
        );
    }

    use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};

    #[derive(Reflect, Default)]
    #[reflect(Component, Default)]
    struct Gem;

    struct Gems;
    impl Plugin for Gems {
        fn register(&self, registry: &mut Registry) {
            registry.register_all::<(Gem,)>();
        }
        fn build(&self, _app: &mut App) {}
    }

    #[test]
    fn plugins_whose_types_the_registry_lacks_are_detected() {
        let app = App::new();
        assert_eq!(unregistered_types(&Gems, app.world.registry()), vec!["Gem"]);
        let mut registry = Registry::with_core_types();
        Gems.register(&mut registry);
        assert!(unregistered_types(&Gems, &registry).is_empty());
        assert!(unregistered_types(&Counter, app.world.registry()).is_empty());
        // Adding it still builds it (and logs the warning once).
        App::new().add_plugin(&Gems).add_plugin(&Gems);
    }
}
