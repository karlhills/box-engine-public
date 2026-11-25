//! Data-configured interactions (docs/INTERACTIONS.md, D18): props the
//! player uses without game code.
//!
//! - [`Interactable`] (saved component) on the entity the player uses,
//!   usually a `PrefabInstance` root: a trigger (press an action nearby, or
//!   touch), a prompt, [`Condition`]s on [`GameVars`] and an ordered list of
//!   [`InteractAction`]s aimed at [`Target`]s (the entity itself, a named
//!   descendant inside the prefab's contents, or any saved entity by uid).
//! - [`Interactor`] (saved marker) is the player; without one, the first
//!   `KinematicCharacter`.
//! - [`GameVars`] (resource): named numbers for puzzle state, kept across
//!   scene switches and saved in save games (hook `"vars"`).
//! - Every use sends [`Interacted`] (scripts: `events.on("Interacted", ...)`)
//!   and calls `on_interact(self, interactor)` on the entity's script.
//!
//! [`InteractPlugin`] (in every [`app_with_world`](crate::app_with_world))
//! adds the `interact` system to Update, after `ui_input`. Its cost per frame is one
//! pass over the interactables (a distance each) plus the prompt setters
//! only when the focused interactable or its text changes.

use std::collections::{BTreeMap, HashMap, HashSet};

use box_core::action::{ActionKind, ActionMap, Actions, Binding};
use box_core::app::{App, Plugin, Stage};
use box_core::asset_ref::AssetRef;
use box_core::identity::{Name, Uid};
use box_core::input::KeyCode;
use box_core::prefab::is_generated;
use box_core::reflect::{
    NotSaved, Range, Reflect, ReflectComponent, ReflectDefault, ReflectResource,
    ReflectVariantDefaults, Registry, Step, Unit, VariantDefaults,
};
use box_core::time::Time;
use box_core::transform::GlobalTransform;
use box_core::visibility::{Visible, is_hidden};
use box_core::world::{Entity, World};
use box_render::{ParticleEmitter, PointLight, SpotLight};
use box_ui::{NodeId, Ui, UiNode};
use glam::Vec3;
use serde_json::{Value as Json, json};

use crate::savegame::SaveHooks;

/// The input action interactables use by default: E on the keyboard, the
/// gamepad's North button. Added to every action map that lacks it.
pub const INTERACT_ACTION: &str = "interact";

/// Save hook name of [`GameVars`].
pub const VARS_SAVE_HOOK: &str = "vars";

/// Something the player can use: press `action` within `radius` (or walk
/// into it, with `trigger: Touch`); if every condition in `requires` holds,
/// the `actions` run in order. Put it on the entity the player uses, usually
/// a `PrefabInstance` root, so each placed copy has its own settings (its
/// own `Entity` targets). Saved; `uses` is its state.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Interactable {
    /// Off: ignored (no prompt, cannot be used). Actions can switch it.
    pub enabled: bool,
    /// Press the action nearby, or fire on walking within `radius`.
    pub trigger: InteractTrigger,
    /// Input action that uses it (`Press` only). `interact` (E, gamepad
    /// North) exists in every project; other names come from the project's
    /// `assets/settings/input.actions.json`.
    pub action: String,
    /// How close the player must be to the interaction point.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m"), @Step(0.1))]
    pub radius: f32,
    /// The interaction point and prompt position: the entity's origin plus
    /// this offset, in world axes.
    #[reflect(@Unit("m"), @Step(0.05))]
    pub offset: Vec3,
    /// Prompt text shown next to the action's key, such as "Light the
    /// brazier".
    pub prompt: String,
    /// String-table key for the prompt; wins over `prompt` when set.
    pub prompt_key: String,
    /// Shown instead while `requires` fails, such as "Needs an ember".
    /// Empty: no prompt while blocked.
    pub blocked_prompt: String,
    /// String-table key for the blocked prompt; wins when set.
    pub blocked_prompt_key: String,
    /// Conditions that must all hold to use it.
    pub requires: Vec<Condition>,
    /// What using it does, in order.
    pub actions: Vec<InteractAction>,
    /// Usable once: stays used afterwards (pickups, one-shot puzzles).
    pub once: bool,
    /// Minimum time between two uses.
    #[reflect(@Range { min: 0.0, max: 60.0 }, @Unit("s"), @Step(0.05))]
    pub cooldown: f32,
    /// How many times it was used (state; saved in save games). Effects on
    /// prefab contents are applied again on load when it is above 0.
    pub uses: u32,
    /// Group name for "use them all" goals: the `GroupDone` condition holds
    /// once every interactable of the group was used, and the HUD shows the
    /// progress (`Pickups 3 / 12`). Empty: in no group.
    pub group: String,
}

impl Default for Interactable {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger: InteractTrigger::Press,
            action: INTERACT_ACTION.to_string(),
            radius: 2.0,
            offset: Vec3::Y,
            prompt: "Use".to_string(),
            prompt_key: String::new(),
            blocked_prompt: String::new(),
            blocked_prompt_key: String::new(),
            requires: Vec::new(),
            actions: Vec::new(),
            once: false,
            cooldown: 0.25,
            uses: 0,
            group: String::new(),
        }
    }
}

impl Interactable {
    /// True when it can be focused or fire: enabled and not used up.
    pub fn available(&self) -> bool {
        self.enabled && !(self.once && self.uses > 0)
    }
}

/// How an [`Interactable`] is used.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum InteractTrigger {
    /// The player presses its action while in range; a prompt shows.
    #[default]
    Press,
    /// Fires when the player comes within range (pickups); no prompt.
    Touch,
}

/// A test on a [`GameVars`] value (unset values are 0).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum Condition {
    /// `var >= value`.
    AtLeast { var: String, value: f64 },
    /// `var < value`.
    Below { var: String, value: f64 },
    /// `var == value`.
    Equals { var: String, value: f64 },
    /// Every `Interactable` whose `group` is this name was used (and there
    /// is at least one).
    GroupDone { group: String },
    /// More than `seconds` of game time passed since the scene started
    /// (`GameState`).
    TimeAbove { seconds: f64 },
}

impl Default for Condition {
    fn default() -> Self {
        Self::AtLeast {
            var: String::new(),
            value: 1.0,
        }
    }
}

impl Condition {
    /// Whether a var condition holds for `vars`; `GroupDone` and
    /// `TimeAbove` need the world ([`holds_in`](Self::holds_in)) and are
    /// false here.
    pub fn holds(&self, vars: &GameVars) -> bool {
        match self {
            Self::AtLeast { var, value } => vars.get(var) >= *value,
            Self::Below { var, value } => vars.get(var) < *value,
            Self::Equals { var, value } => vars.get(var) == *value,
            Self::GroupDone { .. } | Self::TimeAbove { .. } => false,
        }
    }

    /// Whether it holds in `world` now.
    pub fn holds_in(&self, world: &World) -> bool {
        match self {
            Self::GroupDone { group } => {
                let (used, total) = group_progress(world, group);
                total > 0 && used == total
            }
            Self::TimeAbove { seconds } => world
                .get_resource::<crate::behaviours::GameState>()
                .is_some_and(|s| s.elapsed > *seconds),
            _ => match world.get_resource::<GameVars>() {
                Some(v) => self.holds(v),
                None => self.holds(&GameVars::default()),
            },
        }
    }
}

/// (used, total) of the interactables in `group`.
/// Only interactables saved in the scene count: spawned copies (`Transient`)
/// and a spawner's template are left out.
pub fn group_progress(world: &World, group: &str) -> (usize, usize) {
    let mut used = 0;
    let mut total = 0;
    for (e, i) in world.query::<(Entity, &Interactable)>().iter() {
        if i.group == group
            && !box_core::visibility::is_transient(world, e)
            && !crate::behaviours::is_spawn_template(world, e)
        {
            total += 1;
            if i.uses > 0 {
                used += 1;
            }
        }
    }
    (used, total)
}

/// Which entity an [`InteractAction`] acts on.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum Target {
    /// The interactable itself.
    #[default]
    This,
    /// A descendant of the interactable with this `Name`, including a
    /// prefab's instanced contents (the brazier's own `Flame`).
    Child(String),
    /// Any entity with this uid (the bridge a placed brazier reveals).
    Entity(Option<Uid>),
    /// Whoever used it: the player (a hazard sends the player back).
    Interactor,
}

/// One step of an [`Interactable`]'s `actions`.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq, VariantDefaults)]
#[type_path = "box_engine"]
pub enum InteractAction {
    /// Make the target visible (`Visible(true)`), and with `collide` also
    /// solid (`Collidable(true)`).
    Show { target: Target, collide: bool },
    /// Hide the target, and with `collide` switch its colliders off.
    Hide { target: Target, collide: bool },
    /// Flip the target's visibility (and with `collide`, its collision).
    Toggle { target: Target, collide: bool },
    /// Start the target's `ParticleEmitter`.
    StartParticles { target: Target },
    /// Stop the target's `ParticleEmitter`.
    StopParticles { target: Target },
    /// Set the intensity of the target's `PointLight` or `SpotLight`.
    SetLight {
        target: Target,
        #[reflect(@Range { min: 0.0, max: 10000.0 })]
        intensity: f32,
    },
    /// Add `amount` (negative: subtract) to a [`GameVars`] value.
    AddVar { var: String, amount: f64 },
    /// Set a [`GameVars`] value.
    SetVar { var: String, value: f64 },
    /// Switch another `Interactable` on or off (a lever arms a door).
    SetEnabled { target: Target, enabled: bool },
    /// Play a sound once at the interaction point (audio module).
    PlaySound {
        clip: AssetRef,
        #[reflect(@Range { min: 0.0, max: 4.0 }, @Step(0.05))]
        volume: f32,
    },
    /// Script signal `name` with `{ entity, interactor }`, for
    /// `events.on(name, ...)` in scripts.
    Signal { name: String },
    /// Call `function(self, interactor)` on the target's script, in the next
    /// script update (script module).
    CallScript { target: Target, function: String },
    /// Set any reflected field: `path` as in scene commands
    /// (`PointLight.color`), `value` as JSON text (`[1, 0.5, 0.2]`).
    SetField {
        target: Target,
        path: String,
        value: String,
    },
    /// Win the game (`GameRules`: the win screen shows).
    Win,
    /// Lose the game (`GameRules`: the lose screen shows).
    Lose,
    /// Send the player back to the last checkpoint (or where it started);
    /// with `GameRules.lives`, it costs a life.
    Respawn,
    /// Make this interactable's position (plus its offset) the place the
    /// player respawns.
    SetCheckpoint,
    /// Switch to another scene (a name such as `level2`, or a path).
    SwitchScene { scene: String },
    /// A one-shot particle burst at the interaction point.
    Burst {
        style: BurstStyle,
        #[reflect(@Range { min: 1.0, max: 1000.0 })]
        count: u32,
    },
    /// Hurt the target's `Health` (default target: whoever used it). Ignored
    /// while it is invulnerable after a hit.
    Damage {
        target: Target,
        #[reflect(@Range { min: 0.0, max: 10000.0 }, @Step(1.0))]
        amount: f32,
    },
    /// Give the target's `Health` back, up to its maximum (default target:
    /// whoever used it).
    Heal {
        target: Target,
        #[reflect(@Range { min: 0.0, max: 10000.0 }, @Step(1.0))]
        amount: f32,
    },
    /// Remove a spawned copy (a `Spawner`'s, or a script's `transient`
    /// spawn). Anything saved in the scene is hidden instead, so save games
    /// and Restart bring it back.
    Despawn { target: Target },
}

/// The look of an [`InteractAction::Burst`].
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum BurstStyle {
    /// Golden sparks (pickups).
    #[default]
    Sparkle,
    /// Grey dust puffs (landing, breaking).
    Dust,
    /// Orange flames rising (lighting, checkpoints).
    Flame,
}

impl Default for InteractAction {
    fn default() -> Self {
        Self::Show {
            target: Target::This,
            collide: true,
        }
    }
}

/// `collide: true` for the visibility actions, a lit light, full volume.
impl VariantDefaults for InteractAction {
    fn variant_default(variant: &str) -> Option<Self> {
        let target = Target::This;
        Some(match variant {
            "Show" => Self::Show {
                target,
                collide: true,
            },
            "Hide" => Self::Hide {
                target,
                collide: true,
            },
            "Toggle" => Self::Toggle {
                target,
                collide: true,
            },
            "SetLight" => Self::SetLight {
                target,
                intensity: 25.0,
            },
            "AddVar" => Self::AddVar {
                var: String::new(),
                amount: 1.0,
            },
            "SetEnabled" => Self::SetEnabled {
                target,
                enabled: true,
            },
            "PlaySound" => Self::PlaySound {
                clip: AssetRef::default(),
                volume: 1.0,
            },
            "Burst" => Self::Burst {
                style: BurstStyle::Sparkle,
                count: 24,
            },
            "Damage" => Self::Damage {
                target: Target::Interactor,
                amount: 1.0,
            },
            "Heal" => Self::Heal {
                target: Target::Interactor,
                amount: 1.0,
            },
            _ => return None,
        })
    }
}

impl InteractAction {
    /// Actions whose result is state on the target (applied again on load to
    /// prefab contents, which save games do not keep).
    fn is_state(&self) -> bool {
        matches!(
            self,
            Self::Show { .. }
                | Self::Hide { .. }
                | Self::Toggle { .. }
                | Self::StartParticles { .. }
                | Self::StopParticles { .. }
                | Self::SetLight { .. }
                | Self::SetEnabled { .. }
                | Self::SetField { .. }
        )
    }

    fn target(&self) -> Option<&Target> {
        match self {
            Self::Show { target, .. }
            | Self::Hide { target, .. }
            | Self::Toggle { target, .. }
            | Self::StartParticles { target }
            | Self::StopParticles { target }
            | Self::SetLight { target, .. }
            | Self::SetEnabled { target, .. }
            | Self::CallScript { target, .. }
            | Self::SetField { target, .. }
            | Self::Damage { target, .. }
            | Self::Heal { target, .. }
            | Self::Despawn { target } => Some(target),
            Self::AddVar { .. }
            | Self::SetVar { .. }
            | Self::PlaySound { .. }
            | Self::Signal { .. }
            | Self::Win
            | Self::Lose
            | Self::Respawn
            | Self::SetCheckpoint
            | Self::SwitchScene { .. }
            | Self::Burst { .. } => None,
        }
    }
}

/// Marks the player: the entity whose distance to interactables counts.
/// Without one, the first `KinematicCharacter` is the interactor.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Interactor;

/// Named numbers for puzzle and progress state (`embers`, `gate_open`);
/// unset names read 0. A resource of playing worlds: it is kept across scene
/// switches, saved in save games (hook `"vars"`), not stored in scene files.
/// Scripts use the `vars` global; live tools read it with `world_query`
/// (`include_resources`).
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default, Clone, Debug, PartialEq, @NotSaved)]
#[type_path = "box_engine"]
pub struct GameVars {
    /// The values by name.
    pub values: BTreeMap<String, f64>,
    /// Bumped by every change, for code that polls.
    #[reflect(ignore)]
    revision: u64,
}

impl GameVars {
    /// The value of `name`, 0 when unset.
    pub fn get(&self, name: &str) -> f64 {
        self.values.get(name).copied().unwrap_or(0.0)
    }

    /// Sets `name` to `value`.
    pub fn set(&mut self, name: &str, value: f64) {
        if self.values.get(name) != Some(&value) {
            self.values.insert(name.to_string(), value);
            self.revision += 1;
        }
    }

    /// Adds `amount` to `name` and returns the new value.
    pub fn add(&mut self, name: &str, amount: f64) -> f64 {
        let v = self.get(name) + amount;
        self.set(name, v);
        v
    }

    /// Changes since the resource was created.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Sent each time an [`Interactable`] is used, after its actions ran.
/// Scripts: `events.on("Interacted", function(e) ... end)`, with `e.entity`
/// and `e.interactor` uids (`world.entity(uid)`).
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Interacted {
    /// The interactable's uid.
    pub entity: Option<Uid>,
    /// The interactor's uid.
    pub interactor: Option<Uid>,
}

/// Requests from code (`interact.use` in scripts) to use an interactable as
/// if the player pressed its action: conditions are checked, range is not.
#[derive(Debug, Default)]
pub struct InteractRequests {
    pending: Vec<Entity>,
    /// Action lists to run as if `owner` were an interactable used by the
    /// interactor (`Health.on_damage`, a spawner's copies).
    actions: Vec<(Entity, Option<Entity>, Vec<InteractAction>)>,
}

impl InteractRequests {
    pub fn use_entity(&mut self, entity: Entity) {
        self.pending.push(entity);
    }

    /// Runs `actions` for `owner` (`This` targets it, `Interactor` targets
    /// `interactor`, else the player) in the current or next interaction
    /// pass. Conditions and cooldowns do not apply.
    pub fn run_actions(
        &mut self,
        owner: Entity,
        interactor: Option<Entity>,
        actions: Vec<InteractAction>,
    ) {
        self.actions.push((owner, interactor, actions));
    }
}

/// The interaction the prompt shows this frame, for code and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InteractFocus {
    /// The focused interactable.
    pub entity: Option<Entity>,
    /// Its conditions fail (the blocked prompt shows, if any).
    pub blocked: bool,
    /// The prompt text as shown, without the key (empty: no prompt).
    pub text: String,
    /// The key label shown before the text.
    pub key: String,
}

/// Runtime marker: re-apply this interactable's state actions to prefab
/// contents once they exist (it was loaded with `uses > 0`).
struct RestoreEffects {
    frames: u32,
}

/// Registers the interaction types.
pub fn register_interact_types(registry: &mut Registry) {
    for r in [
        registry.register::<Interactable>(),
        registry.register::<Interactor>(),
        registry.register_resource::<GameVars>(),
        registry.register_event::<Interacted>(),
    ] {
        if let Err(e) = r {
            tracing::error!("interaction type registration failed: {e}");
        }
    }
}

/// The default `interact` binding.
pub fn default_interact_actions() -> ActionMap {
    ActionMap::new().button(
        INTERACT_ACTION,
        [
            Binding::Key(KeyCode::E),
            Binding::Gamepad(box_core::input::GamepadButton::North),
        ],
    )
}

/// Interactions as a plugin: the `interact` system, [`GameVars`] with its
/// save hook, the [`Interacted`] event and the `interact` action.
pub struct InteractPlugin;

impl Plugin for InteractPlugin {
    fn build(&self, app: &mut App) {
        app.world.resource_or_default::<GameVars>();
        app.world.resource_or_default::<InteractRequests>();
        app.world.resource_or_default::<InteractFocus>();
        app.add_event::<Interacted>();
        app.world
            .resource_or_default::<ActionMap>()
            .merge_missing(&default_interact_actions());
        app.world.resource_or_default::<SaveHooks>().add(
            VARS_SAVE_HOOK,
            |w| {
                w.get_resource::<GameVars>()
                    .map_or(json!({}), |v| json!(v.values))
            },
            |w, json| {
                let values: BTreeMap<String, f64> =
                    serde_json::from_value(json.clone()).unwrap_or_default();
                let vars = w.resource_or_default::<GameVars>();
                vars.values = values;
                vars.revision += 1;
            },
        );
        app.on_add::<Interactable>(|w, e| {
            if w.get::<Interactable>(e).is_some_and(|i| i.uses > 0) {
                let _ = w.insert_one(e, RestoreEffects { frames: 0 });
            }
        });
        let mut state = InteractState::default();
        app.system(Stage::Update, move |w: &mut World| state.run(w))
            .label("interact")
            .after("ui_input");
    }
}

#[derive(Default)]
struct InteractState {
    /// Touch interactables the interactor is inside (fired already).
    inside: HashSet<Entity>,
    /// Game time of each interactable's last use.
    last_use: HashMap<Entity, f64>,
    prompt: Option<Prompt>,
    /// (owner, action index) pairs whose missing target was reported.
    warned: HashSet<(Entity, usize)>,
    /// The guessed-interactor warning was logged.
    #[cfg(feature = "physics")]
    warned_ambiguous: bool,
}

struct Prompt {
    root: NodeId,
    key: NodeId,
    text: NodeId,
    /// What it shows: (entity, offset, key, text); `None` while hidden.
    shown: Option<(Entity, Vec3, String, String)>,
}

impl InteractState {
    /// The interaction system: picks the focused interactable, shows its
    /// prompt, fires presses, touches and [`InteractRequests`], and restores
    /// loaded state.
    fn run(&mut self, world: &mut World) {
        ensure_action(world);
        self.restore(world);
        let now = world.get_resource::<Time>().map_or(0.0, |t| t.elapsed_f64);
        let captured = world.get_resource::<Ui>().is_some_and(Ui::captures_input);

        let requests = world
            .get_resource_mut::<InteractRequests>()
            .map(|r| std::mem::take(&mut r.pending))
            .unwrap_or_default();
        let interactor = find_interactor(world);
        self.run_queued(world);
        #[cfg(feature = "physics")]
        if let Some(guess) = interactor
            && !self.warned_ambiguous
            && world.query::<&Interactable>().iter().next().is_some()
            && interactor_is_ambiguous(world)
        {
            self.warned_ambiguous = true;
            tracing::warn!(
                target: "box_engine",
                "several characters could use Interactables; using {} (add an `Interactor` component to the player)",
                label(world, guess)
            );
        }
        for e in requests {
            if world.contains(e) {
                self.try_fire(world, e, interactor, now, true);
            }
        }

        let Some(player) = interactor else {
            self.show(world, None);
            return;
        };
        let at = world
            .get::<GlobalTransform>(player)
            .map(|g| g.0.translation.into())
            .unwrap_or(Vec3::ZERO);
        let mut nearest: Option<(f32, Entity)> = None;
        let mut touches = Vec::new();
        let mut still_inside = HashSet::new();
        {
            let mut q = world.query::<(Entity, &Interactable, &GlobalTransform)>();
            for (e, i, g) in q.iter() {
                if !i.available() || e == player {
                    continue;
                }
                let point = Vec3::from(g.0.translation) + i.offset;
                let d = point.distance(at);
                if d > i.radius {
                    continue;
                }
                match i.trigger {
                    InteractTrigger::Press => {
                        if nearest.is_none_or(|(best, _)| d < best) {
                            nearest = Some((d, e));
                        }
                    }
                    InteractTrigger::Touch => {
                        still_inside.insert(e);
                        if !self.inside.contains(&e) {
                            touches.push(e);
                        }
                    }
                }
            }
        }
        self.inside.retain(|e| still_inside.contains(e));
        // Hidden interactables cannot be used.
        let nearest = nearest.filter(|(_, e)| !is_hidden(world, *e));
        for e in touches {
            if is_hidden(world, e) {
                continue;
            }
            if self.try_fire(world, e, Some(player), now, false) {
                self.inside.insert(e);
            }
        }

        let focus = if captured {
            None
        } else {
            nearest.map(|(_, e)| e)
        };
        if let Some(e) = focus {
            let action = world
                .get::<Interactable>(e)
                .map(|i| i.action.clone())
                .unwrap_or_default();
            let blocked = !conditions_hold(world, e);
            let pressed = world
                .get_resource::<Actions>()
                .is_some_and(|a| a.pressed(&action));
            if pressed && !blocked {
                self.try_fire(world, e, Some(player), now, false);
            }
        }
        // The focus may have been used up by the press.
        let focus = focus.filter(|e| world.get::<Interactable>(*e).is_some_and(|i| i.available()));
        self.show(world, focus);
        // Actions queued by this pass's actions (a hit's `on_damage`).
        self.run_queued(world);
    }

    /// Runs [`InteractRequests::run_actions`] lists. Lists queued while
    /// running are run too, up to 8 rounds (a loop of actions that queue
    /// each other stops there).
    fn run_queued(&mut self, world: &mut World) {
        for _ in 0..8 {
            let queued = world
                .get_resource_mut::<InteractRequests>()
                .map(|r| std::mem::take(&mut r.actions))
                .unwrap_or_default();
            if queued.is_empty() {
                return;
            }
            for (owner, interactor, actions) in queued {
                if !world.contains(owner) {
                    continue;
                }
                for (index, action) in actions.iter().enumerate() {
                    self.apply(world, owner, interactor, index, action, false);
                }
            }
        }
    }

    /// Runs `e`'s actions when its conditions hold and its cooldown passed.
    fn try_fire(
        &mut self,
        world: &mut World,
        e: Entity,
        interactor: Option<Entity>,
        now: f64,
        requested: bool,
    ) -> bool {
        let Some(i) = world.get::<Interactable>(e).map(|i| (*i).clone()) else {
            if requested {
                tracing::warn!(target: "box_engine", "interact.use: {} has no Interactable", label(world, e));
            }
            return false;
        };
        if !i.available() {
            return false;
        }
        if !conditions_hold(world, e) {
            return false;
        }
        if let Some(last) = self.last_use.get(&e)
            && now - last < i.cooldown as f64
        {
            return false;
        }
        self.last_use.insert(e, now);
        for (index, action) in i.actions.iter().enumerate() {
            self.apply(world, e, interactor, index, action, false);
        }
        if let Some(c) = world.component_mut::<Interactable>(e) {
            c.uses = c.uses.saturating_add(1);
        }
        let entity_uid = world.get::<Uid>(e).map(|u| *u);
        let interactor_uid = interactor.and_then(|p| world.get::<Uid>(p).map(|u| *u));
        world.send(Interacted {
            entity: entity_uid,
            interactor: interactor_uid,
        });
        #[cfg(feature = "script")]
        if world.has::<box_script::Script>(e) {
            world
                .resource_or_default::<box_script::ScriptCalls>()
                .notify(e, "on_interact", vec![entity_json(interactor_uid)]);
        }
        tracing::debug!(target: "box_engine", "{} used", label(world, e));
        true
    }

    /// Applies one action. `restoring`: re-applying state on load, only to
    /// targets inside prefab contents.
    fn apply(
        &mut self,
        world: &mut World,
        owner: Entity,
        interactor: Option<Entity>,
        index: usize,
        action: &InteractAction,
        restoring: bool,
    ) {
        let target = match action.target() {
            Some(t) => match resolve_for(world, owner, interactor, t) {
                Some(e) => Some(e),
                None => {
                    if !restoring && self.warned.insert((owner, index)) {
                        tracing::warn!(
                            target: "box_engine",
                            "Interactable on {}: action {index} ({}) has no target: {}",
                            label(world, owner),
                            variant_name(action),
                            describe(t)
                        );
                    }
                    return;
                }
            },
            None => None,
        };
        if restoring && !target.is_some_and(|t| is_generated(world, t)) {
            return;
        }
        let interactor_uid = interactor.and_then(|p| world.get::<Uid>(p).map(|u| *u));
        match action {
            InteractAction::Show { collide, .. } => set_shown(world, target, true, *collide),
            InteractAction::Hide { collide, .. } => set_shown(world, target, false, *collide),
            InteractAction::Toggle { collide, .. } => {
                if let Some(t) = target {
                    let shown = !world.get::<Visible>(t).is_some_and(|v| !v.0);
                    set_shown(world, target, !shown, *collide);
                }
            }
            InteractAction::StartParticles { .. } | InteractAction::StopParticles { .. } => {
                let on = matches!(action, InteractAction::StartParticles { .. });
                if let Some(t) = target {
                    match world.component_mut::<ParticleEmitter>(t) {
                        Some(p) => p.playing = on,
                        None => self.warn_component(world, owner, index, t, "ParticleEmitter"),
                    }
                }
            }
            InteractAction::SetLight { intensity, .. } => {
                if let Some(t) = target {
                    let mut done = false;
                    if let Some(l) = world.component_mut::<PointLight>(t) {
                        l.intensity = *intensity;
                        done = true;
                    }
                    if let Some(l) = world.component_mut::<SpotLight>(t) {
                        l.intensity = *intensity;
                        done = true;
                    }
                    if !done {
                        self.warn_component(world, owner, index, t, "PointLight or SpotLight");
                    }
                }
            }
            InteractAction::AddVar { var, amount } => {
                world.resource_or_default::<GameVars>().add(var, *amount);
            }
            InteractAction::SetVar { var, value } => {
                world.resource_or_default::<GameVars>().set(var, *value);
            }
            InteractAction::SetEnabled { enabled, .. } => {
                if let Some(t) = target {
                    let mut done = false;
                    if let Some(i) = world.component_mut::<Interactable>(t) {
                        i.enabled = *enabled;
                        done = true;
                    }
                    done |= crate::behaviours::set_enabled(world, t, *enabled);
                    if !done {
                        self.warn_component(world, owner, index, t, "Interactable or behaviour");
                    }
                }
            }
            InteractAction::PlaySound { clip, volume } => {
                #[cfg(feature = "audio")]
                {
                    let at = interaction_point(world, owner);
                    let mut sound = box_audio::PlaySound::new("").at(at);
                    sound.clip = clip.clone();
                    sound.volume = *volume;
                    world.send(sound);
                }
                #[cfg(not(feature = "audio"))]
                {
                    let _ = (clip, volume);
                    if self.warned.insert((owner, index)) {
                        tracing::warn!(target: "box_engine", "PlaySound needs the `audio` module, which this build leaves out");
                    }
                }
            }
            InteractAction::Signal { name } => {
                #[cfg(feature = "script")]
                {
                    let uid = world.get::<Uid>(owner).map(|u| *u);
                    world.resource_or_default::<box_script::ScriptSignals>().send(
                        name.clone(),
                        json!({ "entity": entity_json(uid), "interactor": entity_json(interactor_uid) }),
                    );
                }
                #[cfg(not(feature = "script"))]
                let _ = name;
            }
            InteractAction::CallScript { function, .. } => {
                #[cfg(feature = "script")]
                if let Some(t) = target {
                    world.resource_or_default::<box_script::ScriptCalls>().call(
                        t,
                        function.clone(),
                        vec![entity_json(interactor_uid)],
                    );
                }
                #[cfg(not(feature = "script"))]
                {
                    let _ = function;
                    if self.warned.insert((owner, index)) {
                        tracing::warn!(target: "box_engine", "CallScript needs the `script` module, which this build leaves out");
                    }
                }
            }
            InteractAction::SetField { path, value, .. } => {
                if let Some(t) = target
                    && let Err(e) = set_field_json(world, t, path, value)
                    && self.warned.insert((owner, index))
                {
                    tracing::warn!(
                        target: "box_engine",
                        "Interactable on {}: action {index} (SetField {path}): {e}",
                        label(world, owner)
                    );
                }
            }
            InteractAction::Win => crate::behaviours::win(world),
            InteractAction::Lose => crate::behaviours::lose(world),
            InteractAction::Respawn => {
                if let Some(p) = interactor.or_else(|| find_interactor(world)) {
                    crate::behaviours::respawn(world, p);
                }
            }
            InteractAction::SetCheckpoint => {
                let at = interaction_point(world, owner);
                crate::behaviours::set_checkpoint(world, at);
            }
            InteractAction::SwitchScene { scene } => {
                crate::behaviours::switch_scene(world, scene);
            }
            InteractAction::Burst { style, count } => {
                let at = interaction_point(world, owner);
                crate::behaviours::burst(world, at, *style, *count);
            }
            InteractAction::Damage { amount, .. } => {
                if let Some(t) = target {
                    if world.has::<crate::behaviours::Health>(t) {
                        // The source is whoever used it, else the owner (an
                        // enemy's own touch, a spawned hazard).
                        let source = if Some(t) == interactor {
                            Some(owner)
                        } else {
                            interactor.or(Some(owner))
                        };
                        crate::behaviours::damage(world, t, *amount, source);
                    } else if !is_player_without_health(world, t) {
                        self.warn_component(world, owner, index, t, "Health");
                    }
                }
            }
            InteractAction::Heal { amount, .. } => {
                if let Some(t) = target {
                    if world.has::<crate::behaviours::Health>(t) {
                        crate::behaviours::heal(world, t, *amount);
                    } else {
                        self.warn_component(world, owner, index, t, "Health");
                    }
                }
            }
            InteractAction::Despawn { .. } => {
                if let Some(t) = target {
                    despawn_or_hide(world, t);
                }
            }
        }
        let _ = interactor_uid;
    }

    fn warn_component(
        &mut self,
        world: &World,
        owner: Entity,
        index: usize,
        t: Entity,
        what: &str,
    ) {
        if self.warned.insert((owner, index)) {
            tracing::warn!(
                target: "box_engine",
                "Interactable on {}: action {index} targets {}, which has no {what}",
                label(world, owner),
                label(world, t)
            );
        }
    }

    /// Re-applies state actions of interactables loaded with `uses > 0` to
    /// their targets inside prefab contents, once those exist (they are
    /// spawned after the scene or save loads). Gives up after 10 s of frames.
    fn restore(&mut self, world: &mut World) {
        let pending: Vec<Entity> = world
            .query::<(Entity, &RestoreEffects)>()
            .iter()
            .map(|(e, _)| e)
            .collect();
        for e in pending {
            let Some(i) = world.get::<Interactable>(e).map(|i| (*i).clone()) else {
                let _ = world.remove_one::<RestoreEffects>(e);
                continue;
            };
            let missing = i
                .actions
                .iter()
                .filter(|a| a.is_state())
                .filter_map(|a| a.target())
                .any(|t| resolve(world, e, t).is_none());
            let frames = world.get::<RestoreEffects>(e).map_or(0, |r| r.frames);
            if missing && frames < 600 {
                if let Some(r) = world.component_mut::<RestoreEffects>(e) {
                    r.frames += 1;
                }
                continue;
            }
            let _ = world.remove_one::<RestoreEffects>(e);
            for (index, action) in i.actions.iter().enumerate() {
                if !action.is_state() {
                    continue;
                }
                let times = if matches!(action, InteractAction::Toggle { .. }) {
                    i.uses % 2
                } else {
                    1
                };
                for _ in 0..times {
                    self.apply(world, e, None, index, action, true);
                }
            }
        }
    }

    /// Shows the prompt for `focus` (or hides it). Touches the UI only when
    /// what it shows changes.
    fn show(&mut self, world: &mut World, focus: Option<Entity>) {
        let want = focus.and_then(|e| prompt_for(world, e).map(|(k, t, o)| (e, o, k, t)));
        let next = InteractFocus {
            entity: focus,
            blocked: focus.is_some_and(|e| !conditions_hold(world, e)),
            text: want.as_ref().map(|w| w.3.clone()).unwrap_or_default(),
            key: want.as_ref().map(|w| w.2.clone()).unwrap_or_default(),
        };
        let f = world.resource_or_default::<InteractFocus>();
        if *f != next {
            *f = next;
        }
        let Some(ui) = world.get_resource_mut::<Ui>() else {
            return;
        };
        if let Some(p) = &self.prompt
            && !ui.contains(p.root)
        {
            self.prompt = None;
        }
        if want.is_none() && self.prompt.as_ref().is_none_or(|p| p.shown.is_none()) {
            return;
        }
        let p = self.prompt.get_or_insert_with(|| {
            let root = ui.add(
                None,
                UiNode::row()
                    .with_class("prompt")
                    .at_point(Vec3::ZERO)
                    .with_layer(5)
                    .hidden(),
            );
            let key = ui.add(Some(root), UiNode::text("E").with_class("prompt_key"));
            let text = ui.add(Some(root), UiNode::text("").with_class("prompt_text"));
            Prompt {
                root,
                key,
                text,
                shown: None,
            }
        });
        if p.shown == want {
            return;
        }
        match &want {
            Some((e, offset, key, text)) => {
                let moved = p
                    .shown
                    .as_ref()
                    .is_none_or(|(se, so, _, _)| se != e || so != offset);
                if moved {
                    ui.anchor_to_entity(p.root, *e, *offset);
                }
                ui.set_text(p.key, key.clone());
                ui.set_visible(p.key, !key.is_empty());
                ui.set_text(p.text, text.clone());
                ui.set_visible(p.root, true);
            }
            None => {
                ui.set_visible(p.root, false);
            }
        }
        p.shown = want;
    }
}

/// Whether every condition of `e`'s `requires` holds now.
pub fn conditions_hold(world: &World, e: Entity) -> bool {
    let Some(i) = world.get::<Interactable>(e) else {
        return false;
    };
    i.requires.iter().all(|c| c.holds_in(world))
}

/// The interactable's origin plus its offset (where sounds and bursts play,
/// and the checkpoint it sets).
pub fn interaction_point(world: &World, e: Entity) -> Vec3 {
    world
        .get::<GlobalTransform>(e)
        .map(|g| Vec3::from(g.0.translation))
        .unwrap_or(Vec3::ZERO)
        + world
            .get::<Interactable>(e)
            .map_or(Vec3::ZERO, |i| i.offset)
}

/// (key label, text, offset) of `e`'s prompt now; `None` when there is
/// nothing to show (blocked without a blocked prompt).
fn prompt_for(world: &World, e: Entity) -> Option<(String, String, Vec3)> {
    let ok = conditions_hold(world, e);
    let i = world.get::<Interactable>(e)?;
    let strings = world.get_resource::<box_ui::Strings>();
    let pick = |text: &str, key: &str| -> String {
        if !key.is_empty()
            && let Some(s) = strings
        {
            return s.get(key);
        }
        text.to_string()
    };
    let text = if ok {
        pick(&i.prompt, &i.prompt_key)
    } else {
        pick(&i.blocked_prompt, &i.blocked_prompt_key)
    };
    if text.is_empty() {
        return None;
    }
    let key = if ok {
        key_label(world, &i.action)
    } else {
        String::new()
    };
    Some((key, text, i.offset))
}

/// The label of `action`'s first keyboard binding ("E"), else its first
/// gamepad button ("North"), else the action name.
pub fn key_label(world: &World, action: &str) -> String {
    let Some(ActionKind::Button { bindings }) = world
        .get_resource::<ActionMap>()
        .and_then(|m| m.get(action))
    else {
        return action.to_string();
    };
    let key = bindings.iter().find_map(|b| match b {
        Binding::Key(k) => Some(format!("{k:?}")),
        _ => None,
    });
    key.or_else(|| {
        bindings.iter().find_map(|b| match b {
            Binding::Gamepad(g) => Some(format!("{g:?}")),
            Binding::Mouse(m) => Some(format!("{m:?}")),
            Binding::Key(_) => None,
        })
    })
    .unwrap_or_else(|| action.to_string())
}

/// Adds the `interact` action when a game replaced the action map after the
/// plugin was built (cheap when present: one lookup).
fn ensure_action(world: &mut World) {
    let map = world.resource_or_default::<ActionMap>();
    if map.get(INTERACT_ACTION).is_none() {
        map.merge_missing(&default_interact_actions());
    }
}

/// The player: the first [`Interactor`]; without one, the
/// `KinematicCharacter` named `Player`, else the first `KinematicCharacter`.
pub fn find_interactor(world: &World) -> Option<Entity> {
    if let Some((e, _)) = world.query::<(Entity, &Interactor)>().iter().next() {
        return Some(e);
    }
    #[cfg(feature = "physics")]
    {
        if let Some((e, _)) = world
            .query::<(Entity, &crate::behaviours::PlayerController)>()
            .iter()
            .next()
        {
            return Some(e);
        }
        let characters: Vec<Entity> = world
            .query::<(Entity, &box_physics::KinematicCharacter)>()
            .iter()
            .map(|(e, _)| e)
            .collect();
        let named = characters
            .iter()
            .copied()
            .find(|e| world.get::<Name>(*e).is_some_and(|n| n.0 == "Player"));
        if let Some(e) = named.or(characters.first().copied()) {
            return Some(e);
        }
    }
    None
}

/// True when the interactor was guessed among several characters (no
/// `Interactor`, none named `Player`).
#[cfg(feature = "physics")]
fn interactor_is_ambiguous(world: &World) -> bool {
    if world.query::<&Interactor>().iter().next().is_some()
        || world
            .query::<&crate::behaviours::PlayerController>()
            .iter()
            .next()
            .is_some()
    {
        return false;
    }
    let mut q = world.query::<(&box_physics::KinematicCharacter, Option<&Name>)>();
    let all: Vec<bool> = q
        .iter()
        .map(|(_, n)| n.is_some_and(|n| n.0 == "Player"))
        .collect();
    all.len() > 1 && !all.contains(&true)
}

/// The entity `target` names for the interactable `owner` (`Interactor`:
/// the player).
pub fn resolve(world: &World, owner: Entity, target: &Target) -> Option<Entity> {
    resolve_for(world, owner, None, target)
}

/// [`resolve`] with the entity that used it, for `Target::Interactor`.
pub fn resolve_for(
    world: &World,
    owner: Entity,
    interactor: Option<Entity>,
    target: &Target,
) -> Option<Entity> {
    match target {
        Target::This => Some(owner),
        Target::Interactor => interactor.or_else(|| find_interactor(world)),
        Target::Entity(uid) => uid.and_then(|u| world.entity_by_uid(u)),
        Target::Child(name) => {
            let mut stack = world.children(owner);
            while let Some(e) = stack.pop() {
                if world.get::<Name>(e).is_some_and(|n| n.0 == *name) {
                    return Some(e);
                }
                stack.extend(world.children(e));
            }
            None
        }
    }
}

fn describe(t: &Target) -> String {
    match t {
        Target::This => "itself".into(),
        Target::Child(n) => format!("no descendant named `{n}`"),
        Target::Entity(Some(u)) => format!("no entity with uid {u}"),
        Target::Entity(None) => "no entity chosen".into(),
        Target::Interactor => "no player (add a PlayerController or an Interactor)".into(),
    }
}

fn variant_name(a: &InteractAction) -> &'static str {
    match a {
        InteractAction::Show { .. } => "Show",
        InteractAction::Hide { .. } => "Hide",
        InteractAction::Toggle { .. } => "Toggle",
        InteractAction::StartParticles { .. } => "StartParticles",
        InteractAction::StopParticles { .. } => "StopParticles",
        InteractAction::SetLight { .. } => "SetLight",
        InteractAction::AddVar { .. } => "AddVar",
        InteractAction::SetVar { .. } => "SetVar",
        InteractAction::SetEnabled { .. } => "SetEnabled",
        InteractAction::PlaySound { .. } => "PlaySound",
        InteractAction::Signal { .. } => "Signal",
        InteractAction::CallScript { .. } => "CallScript",
        InteractAction::SetField { .. } => "SetField",
        InteractAction::Win => "Win",
        InteractAction::Lose => "Lose",
        InteractAction::Respawn => "Respawn",
        InteractAction::SetCheckpoint => "SetCheckpoint",
        InteractAction::SwitchScene { .. } => "SwitchScene",
        InteractAction::Burst { .. } => "Burst",
        InteractAction::Damage { .. } => "Damage",
        InteractAction::Heal { .. } => "Heal",
        InteractAction::Despawn { .. } => "Despawn",
    }
}

/// A player without `Health` hit by `Damage` respawns instead (the hazard
/// still works); returns true when that happened.
fn is_player_without_health(world: &mut World, t: Entity) -> bool {
    if find_interactor(world) != Some(t) {
        return false;
    }
    crate::behaviours::respawn(world, t);
    true
}

/// Despawns `t` when it is runtime-only (`Transient` or below one), else
/// hides it with its colliders, so saved entities come back on load.
pub fn despawn_or_hide(world: &mut World, t: Entity) {
    if box_core::visibility::is_transient(world, t) {
        world.defer().despawn(t);
    } else {
        set_shown(world, Some(t), false, true);
    }
}

/// `Visible` and (with `collide` and the physics module) `Collidable` on
/// `target`.
fn set_shown(world: &mut World, target: Option<Entity>, shown: bool, collide: bool) {
    let Some(t) = target else { return };
    let _ = world.insert_one(t, Visible(shown));
    #[cfg(feature = "physics")]
    if collide {
        let _ = world.insert_one(t, box_physics::Collidable(shown));
    }
    #[cfg(not(feature = "physics"))]
    let _ = collide;
}

/// Sets the reflected field at `path` (`Component.field`) on `e` from JSON
/// text.
pub fn set_field_json(world: &mut World, e: Entity, path: &str, value: &str) -> Result<(), String> {
    let json: Json =
        serde_json::from_str(value).map_err(|err| format!("value `{value}` is not JSON: {err}"))?;
    let path = world.field_path(path).map_err(|err| err.to_string())?;
    let current = world.get_field(e, &path).map_err(|err| err.to_string())?;
    let info = current
        .get_represented_type_info()
        .ok_or_else(|| format!("cannot set `{path}`: unknown field type"))?;
    let v = world
        .registry()
        .value_from_json(info.type_id(), &json)
        .map_err(|err| err.to_string())?;
    world
        .set_field(e, &path, v.as_ref())
        .map_err(|err| err.to_string())
}

#[cfg(feature = "script")]
fn entity_json(uid: Option<Uid>) -> Json {
    uid.map_or(Json::Null, |u| json!({ "$entity": u.to_string() }))
}

fn label(world: &World, e: Entity) -> String {
    match (
        world.get::<Name>(e).map(|n| n.0.clone()),
        world.get::<Uid>(e).map(|u| *u),
    ) {
        (Some(n), Some(u)) => format!("`{n}` ({u})"),
        (Some(n), None) => format!("`{n}`"),
        (None, Some(u)) => u.to_string(),
        (None, None) => format!("entity {}", e.id()),
    }
}
