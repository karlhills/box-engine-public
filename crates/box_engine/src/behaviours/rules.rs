//! Game rules: what winning and losing mean for a scene ([`GameRules`]),
//! where the game stands ([`GameState`]), the built-in pause and end screens,
//! and the built-in HUD ([`HudSettings`]).
//!
//! Everything here is off unless the scene has a `GameRules` resource
//! (menus, win and lose) or a `HudSettings` resource (the HUD), so games
//! that build their own menus are not affected.

use std::collections::BTreeSet;

use box_core::action::Actions;
use box_core::app::AppExit;
use box_core::color::Color;
use box_core::event::{EventCursor, Events};
use box_core::identity::Name;
use box_core::input::CursorOptions;
use box_core::interpolate::Interpolated;
use box_core::reflect::{NotSaved, Range, Reflect, ReflectDefault, ReflectResource, Step, Unit};
use box_core::time::Time;
use box_core::transform::{GlobalTransform, Transform};
use box_core::visibility::Transient;
use box_core::world::{Entity, World};
use box_render::{GradientKey, ParticleBlend, ParticleEmitter};
use box_ui::{NodeId, Ui, UiEvent, UiEventKind, UiNode};
use glam::{Vec2, Vec3};
use serde_json::{Value as Json, json};

use crate::interact::{
    BurstStyle, Condition, GameVars, Interactable, find_interactor, group_progress,
};
use crate::scenes::{SceneInstanceId, Scenes};

/// The input action that opens the pause menu: Escape, P, gamepad Start.
pub const PAUSE_ACTION: &str = "pause";

/// Save hook name of [`GameState`].
pub const GAME_SAVE_HOOK: &str = "game";

/// What winning and losing mean in this scene. A scene resource (edit it in
/// Scene Settings, or with `set_resource`); a scene without it has no
/// built-in win, lose, pause or end screens.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct GameRules {
    /// The game is won as soon as all of these hold (a `Win` action wins
    /// too). Empty: only a `Win` action wins.
    pub win: Vec<Condition>,
    /// The game is lost as soon as any of these holds (a `Lose` action, the
    /// time limit and running out of lives lose too).
    pub lose: Vec<Condition>,
    /// Seconds to win in; 0: no limit. The HUD counts down.
    #[reflect(@Range { min: 0.0, max: 36000.0 }, @Unit("s"), @Step(1.0))]
    pub time_limit: f32,
    /// Respawns the player has (falling off, hazards); the game is lost
    /// when they run out. 0: unlimited.
    #[reflect(@Range { min: 0.0, max: 99.0 })]
    pub lives: u32,
    /// Built-in pause menu (the `pause` action: Escape, P, gamepad Start)
    /// and win and lose screens. Off for games with their own menus.
    pub menus: bool,
    /// The scene the win screen's Next button opens (a name such as
    /// `level2`). Empty: no Next button.
    pub next_scene: String,
    /// Title of the win screen.
    pub win_title: String,
    /// Title of the lose screen.
    pub lose_title: String,
}

impl Default for GameRules {
    fn default() -> Self {
        Self {
            win: Vec::new(),
            lose: Vec::new(),
            time_limit: 0.0,
            lives: 0,
            menus: true,
            next_scene: String::new(),
            win_title: "You win!".into(),
            lose_title: "Game over".into(),
        }
    }
}

/// Where the game stands.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum GamePhase {
    /// The game runs.
    #[default]
    Playing,
    /// The pause menu is open; game time is stopped.
    Paused,
    /// Won: the win screen is open.
    Won,
    /// Lost: the lose screen is open.
    Lost,
}

/// The state of the current game: phase, time, lives, checkpoint. A runtime
/// resource of playing worlds (never in scene files; save games keep it
/// through the `"game"` hook). Live tools read it with `world_query`.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default, Clone, Debug, PartialEq, @NotSaved)]
#[type_path = "box_engine"]
pub struct GameState {
    /// Playing, paused, won or lost.
    pub phase: GamePhase,
    /// Game seconds since the scene started (stops while paused).
    pub elapsed: f64,
    /// Lives left when `GameRules.lives` is set.
    pub lives_left: u32,
    /// Where the player respawns; `None`: where it started.
    pub checkpoint: Option<Vec3>,
    /// How many times the player respawned.
    pub respawns: u32,
}

/// A line of the built-in HUD.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct HudVar {
    /// The `GameVars` name.
    pub var: String,
    /// Shown before the value; empty: the name, capitalised.
    pub label: String,
}

/// The built-in HUD: a panel in the top-left corner with game vars, group
/// progress, lives and the time left. A scene resource; without it there is
/// no built-in HUD. Styled by the theme's `hud` style.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct HudSettings {
    /// Off: no HUD.
    pub enabled: bool,
    /// The vars to show, in order. Empty: every var that has a value.
    pub vars: Vec<HudVar>,
    /// Show each interactable group's progress (`Pickups 3 / 12`).
    pub groups: bool,
    /// Show lives left (when `GameRules.lives` is set).
    pub lives: bool,
    /// Show the time left (when `GameRules.time_limit` is set).
    pub timer: bool,
    /// Show the player's health (when it has a `Health`).
    pub health: bool,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            vars: Vec::new(),
            groups: true,
            lives: true,
            timer: true,
            health: true,
        }
    }
}

/// Runtime: where an interactor was first seen, its respawn point without a
/// checkpoint.
#[derive(Clone, Copy, Debug)]
pub struct SpawnPoint(pub Vec3);

/// Wins the game (no effect unless it is being played).
pub fn win(world: &mut World) {
    set_phase_from_playing(world, GamePhase::Won);
}

/// Loses the game (no effect unless it is being played).
pub fn lose(world: &mut World) {
    set_phase_from_playing(world, GamePhase::Lost);
}

fn set_phase_from_playing(world: &mut World, phase: GamePhase) {
    let state = world.resource_or_default::<GameState>();
    if state.phase == GamePhase::Playing {
        state.phase = phase;
        tracing::info!(target: "box_engine", "game {}", if phase == GamePhase::Won { "won" } else { "lost" });
    }
}

/// Where respawns go now: the checkpoint, else where the player started.
pub fn respawn_point(world: &World, player: Entity) -> Option<Vec3> {
    world
        .get_resource::<GameState>()
        .and_then(|s| s.checkpoint)
        .or_else(|| world.get::<SpawnPoint>(player).map(|s| s.0))
}

/// Sends `player` back to the checkpoint (or its start). With
/// `GameRules.lives`, it costs a life, and the last one loses the game.
pub fn respawn(world: &mut World, player: Entity) {
    let lives = world.get_resource::<GameRules>().map_or(0, |r| r.lives);
    {
        let state = world.resource_or_default::<GameState>();
        if state.phase != GamePhase::Playing {
            return;
        }
        state.respawns += 1;
        if lives > 0 {
            state.lives_left = state.lives_left.saturating_sub(1);
            if state.lives_left == 0 {
                state.phase = GamePhase::Lost;
                tracing::info!(target: "box_engine", "game lost: no lives left");
                return;
            }
        }
    }
    super::health::refill(world, player);
    let Some(at) = respawn_point(world, player) else {
        return;
    };
    teleport(world, player, at);
}

/// Moves an entity at once (no interpolated slide), stopping a character.
pub fn teleport(world: &mut World, e: Entity, at: Vec3) {
    if let Some(t) = world.component_mut::<Transform>(e) {
        t.translation = at;
    }
    #[cfg(feature = "physics")]
    if let Some(c) = world.component_mut::<box_physics::KinematicCharacter>(e) {
        c.velocity = Vec3::ZERO;
        c.grounded = false;
        c.ground_entity = None;
        c.ground_pose = None;
    }
    if world.has::<Interpolated>(e) {
        Interpolated::reset(world, e);
    }
}

/// Makes `at` the respawn point.
pub fn set_checkpoint(world: &mut World, at: Vec3) {
    let state = world.resource_or_default::<GameState>();
    if state.checkpoint != Some(at) {
        state.checkpoint = Some(at);
        tracing::info!(target: "box_engine", "checkpoint at [{:.1}, {:.1}, {:.1}]", at.x, at.y, at.z);
    }
}

/// Asks for a scene switch at the start of the next Update.
pub fn switch_scene(world: &mut World, scene: &str) {
    if scene.is_empty() {
        tracing::warn!(target: "box_engine", "SwitchScene: no scene given");
        return;
    }
    match world.get_resource_mut::<Scenes>() {
        Some(s) => {
            s.switch(scene);
        }
        None => {
            tracing::warn!(target: "box_engine", "SwitchScene `{scene}`: this app has no Scenes resource")
        }
    }
}

/// A one-shot particle burst at `at` (a runtime entity that despawns itself).
pub fn burst(world: &mut World, at: Vec3, style: BurstStyle, count: u32) {
    let mut p = ParticleEmitter::burst(count.max(1));
    let key = |t: f32, c: Color| GradientKey { t, color: c };
    match style {
        BurstStyle::Sparkle => {
            p.speed = Vec2::new(1.5, 3.0);
            p.lifetime = Vec2::new(0.4, 0.8);
            p.gravity = Vec3::new(0.0, -6.0, 0.0);
            p.color = vec![
                key(0.0, Color::linear_rgba(1.0, 0.75, 0.2, 1.0)),
                key(1.0, Color::linear_rgba(1.0, 0.4, 0.05, 0.0)),
            ];
            p.intensity = 6.0;
            p.blend = ParticleBlend::Additive;
        }
        BurstStyle::Dust => {
            p.speed = Vec2::new(0.5, 1.5);
            p.lifetime = Vec2::new(0.6, 1.2);
            p.gravity = Vec3::new(0.0, -1.0, 0.0);
            p.drag = 2.0;
            p.color = vec![
                key(0.0, Color::linear_rgba(0.55, 0.5, 0.45, 0.6)),
                key(1.0, Color::linear_rgba(0.55, 0.5, 0.45, 0.0)),
            ];
            p.blend = ParticleBlend::Alpha;
        }
        BurstStyle::Flame => {
            p.speed = Vec2::new(1.0, 4.0);
            p.lifetime = Vec2::new(0.5, 1.0);
            p.gravity = Vec3::new(0.0, 2.0, 0.0);
            p.color = vec![
                key(0.0, Color::linear_rgba(1.0, 0.8, 0.3, 1.0)),
                key(1.0, Color::linear_rgba(1.0, 0.3, 0.05, 0.0)),
            ];
            p.intensity = 6.0;
            p.blend = ParticleBlend::Additive;
        }
    }
    world.spawn((
        Name::new("Burst"),
        Transform::from_translation(at),
        p,
        Transient,
    ));
}

/// The `"game"` save hook.
pub(crate) fn save_game_state(world: &World) -> Json {
    let Some(s) = world.get_resource::<GameState>() else {
        return json!({});
    };
    json!({
        "elapsed": s.elapsed,
        "lives_left": s.lives_left,
        "checkpoint": s.checkpoint.map(|c| [c.x, c.y, c.z]),
        "respawns": s.respawns,
    })
}

pub(crate) fn load_game_state(world: &mut World, j: &Json) {
    let state = world.resource_or_default::<GameState>();
    state.phase = GamePhase::Playing;
    state.elapsed = j["elapsed"].as_f64().unwrap_or(0.0);
    state.lives_left = j["lives_left"].as_u64().unwrap_or(0) as u32;
    state.respawns = j["respawns"].as_u64().unwrap_or(0) as u32;
    state.checkpoint = j["checkpoint"].as_array().and_then(|a| {
        let f = |i: usize| a.get(i).and_then(Json::as_f64).map(|x| x as f32);
        Some(Vec3::new(f(0)?, f(1)?, f(2)?))
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuKind {
    Pause,
    Won,
    Lost,
}

/// The rules system's state (one per app).
#[derive(Default)]
pub(crate) struct RulesSystem {
    menu: Option<(MenuKind, NodeId)>,
    hud: Option<(NodeId, Vec<NodeId>, Vec<String>)>,
    ui_events: EventCursor<UiEvent>,
    /// The scene snapshot Restart goes back to.
    restart: Option<Json>,
    /// The main scene instance the snapshot belongs to (`Some(None)`: taken,
    /// no `Scenes`).
    snapshot_of: Option<Option<SceneInstanceId>>,
    /// This system paused game time (and will resume it).
    paused_time: bool,
}

impl RulesSystem {
    /// Update, after interactions: time, win and lose, pause, menus, HUD.
    pub(crate) fn run(&mut self, world: &mut World) {
        world.resource_or_default::<GameState>();
        record_spawn_points(world);
        let rules = world.get_resource::<GameRules>().cloned();
        if let Some(rules) = &rules {
            self.track_scene(world, rules);
            self.tick(world, rules);
            self.handle_ui(world, rules);
            self.sync_menu(world, rules);
        } else {
            self.close_menu(world);
        }
        self.sync_hud(world, rules.as_ref());
    }

    /// A new main scene: take the Restart snapshot and start the state over
    /// (lives from the rules, no checkpoint).
    fn track_scene(&mut self, world: &mut World, rules: &GameRules) {
        let main = world.get_resource::<Scenes>().and_then(Scenes::main);
        if self.snapshot_of == Some(main) {
            return;
        }
        if world
            .get_resource::<Scenes>()
            .is_some_and(Scenes::has_pending)
        {
            return;
        }
        let first = self.snapshot_of.is_none();
        self.snapshot_of = Some(main);
        {
            let state = world.resource_or_default::<GameState>();
            if !first || *state == GameState::default() {
                *state = GameState {
                    lives_left: rules.lives,
                    ..Default::default()
                };
            }
        }
        match crate::savegame::snapshot(world) {
            Ok(s) => self.restart = Some(s),
            Err(e) => {
                self.restart = None;
                tracing::warn!(target: "box_engine", "Restart will not work in this scene: {e}");
            }
        }
    }

    fn tick(&mut self, world: &mut World, rules: &GameRules) {
        let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta) as f64;
        let phase = world.resource_or_default::<GameState>().phase;
        if phase == GamePhase::Playing {
            let elapsed = {
                let s = world.resource_or_default::<GameState>();
                s.elapsed += dt;
                s.elapsed
            };
            if !rules.win.is_empty() && rules.win.iter().all(|c| c.holds_in(world)) {
                win(world);
            } else if (rules.time_limit > 0.0 && elapsed >= rules.time_limit as f64)
                || rules.lose.iter().any(|c| c.holds_in(world))
            {
                lose(world);
            }
        }
        if !rules.menus {
            return;
        }
        if super::ensure_game_actions(world) {
            return;
        }
        let pressed = world
            .get_resource::<Actions>()
            .is_some_and(|a| a.pressed(PAUSE_ACTION));
        if pressed {
            let s = world.resource_or_default::<GameState>();
            s.phase = match s.phase {
                GamePhase::Playing => GamePhase::Paused,
                GamePhase::Paused => GamePhase::Playing,
                other => other,
            };
        }
    }

    fn handle_ui(&mut self, world: &mut World, rules: &GameRules) {
        let mut clicks = Vec::new();
        if let Some(events) = world.get_resource::<Events<UiEvent>>() {
            for e in self.ui_events.read(events) {
                // Back is sent for the active modal root.
                let in_menu = self.menu.is_some_and(|(_, root)| root == e.node);
                if e.kind == UiEventKind::Back && in_menu {
                    clicks.push("game_resume".to_string());
                } else if e.kind == UiEventKind::Click
                    && let Some(id) = &e.id
                    && id.starts_with("game_")
                {
                    clicks.push(id.clone());
                }
            }
        }
        for id in clicks {
            match id.as_str() {
                "game_resume" => {
                    let s = world.resource_or_default::<GameState>();
                    if s.phase == GamePhase::Paused {
                        s.phase = GamePhase::Playing;
                    }
                }
                "game_restart" => self.restart(world),
                "game_next" => {
                    if !rules.next_scene.is_empty() {
                        switch_scene(world, &rules.next_scene);
                        world.resource_or_default::<GameState>().phase = GamePhase::Playing;
                    }
                }
                "game_quit" => world.insert_resource(AppExit(true)),
                _ => {}
            }
        }
    }

    /// Back to how the scene was when it started (entities, vars, state).
    fn restart(&mut self, world: &mut World) {
        let Some(snapshot) = self.restart.clone() else {
            tracing::warn!(target: "box_engine", "Restart: no snapshot of this scene");
            return;
        };
        match crate::savegame::restore(world, &snapshot) {
            Ok(_) => {
                world.resource_or_default::<GameState>().phase = GamePhase::Playing;
                tracing::info!(target: "box_engine", "game restarted");
            }
            Err(e) => tracing::warn!(target: "box_engine", "Restart failed: {e}"),
        }
    }

    fn sync_menu(&mut self, world: &mut World, rules: &GameRules) {
        let phase = world.resource_or_default::<GameState>().phase;
        let want = match phase {
            _ if !rules.menus => None,
            GamePhase::Playing => None,
            GamePhase::Paused => Some(MenuKind::Pause),
            GamePhase::Won => Some(MenuKind::Won),
            GamePhase::Lost => Some(MenuKind::Lost),
        };
        // Game time stops while a menu or end screen is open.
        let stop = phase != GamePhase::Playing;
        if let Some(t) = world.get_resource_mut::<Time>() {
            if stop && !t.paused {
                t.paused = true;
                self.paused_time = true;
            } else if !stop && self.paused_time {
                t.paused = false;
                self.paused_time = false;
            }
        }
        if self.menu.map(|(k, _)| k) == want
            && self.menu.is_none_or(|(_, root)| {
                world
                    .get_resource::<Ui>()
                    .is_some_and(|ui| ui.contains(root))
            })
        {
            return;
        }
        self.close_menu(world);
        let Some(kind) = want else {
            return;
        };
        if let Some(c) = world.get_resource_mut::<CursorOptions>() {
            c.release();
        }
        let Some(ui) = world.get_resource_mut::<Ui>() else {
            return;
        };
        let title = match kind {
            MenuKind::Pause => "Paused",
            MenuKind::Won => rules.win_title.as_str(),
            MenuKind::Lost => rules.lose_title.as_str(),
        };
        let root = ui.add(
            None,
            UiNode::panel()
                .modal()
                .anchored([0.5, 0.5], [0.0, 0.0])
                .with_layer(20)
                .with_id("game_menu"),
        );
        ui.add(Some(root), UiNode::text(title).with_class("title"));
        let mut buttons: Vec<(&str, &str)> = Vec::new();
        match kind {
            MenuKind::Pause => {
                buttons.extend([("game_resume", "Resume"), ("game_restart", "Restart")])
            }
            MenuKind::Won => {
                if !rules.next_scene.is_empty() {
                    buttons.push(("game_next", "Next"));
                }
                buttons.push(("game_restart", "Play again"));
            }
            MenuKind::Lost => buttons.push(("game_restart", "Try again")),
        }
        buttons.push(("game_quit", "Quit"));
        for (i, (id, text)) in buttons.into_iter().enumerate() {
            let mut b = UiNode::button(id, "").with_text(text);
            if i == 0 {
                b = b.autofocus();
            }
            ui.add(Some(root), b);
        }
        self.menu = Some((kind, root));
    }

    fn close_menu(&mut self, world: &mut World) {
        if let Some((_, root)) = self.menu.take()
            && let Some(ui) = world.get_resource_mut::<Ui>()
        {
            ui.remove(root);
        }
        if self.paused_time
            && let Some(t) = world.get_resource_mut::<Time>()
        {
            t.paused = false;
            self.paused_time = false;
        }
    }

    /// The HUD lines now; touches the UI only when one changes.
    fn sync_hud(&mut self, world: &mut World, rules: Option<&GameRules>) {
        let settings = world
            .get_resource::<HudSettings>()
            .filter(|h| h.enabled)
            .cloned();
        let Some(settings) = settings else {
            if let Some((root, _, _)) = self.hud.take()
                && let Some(ui) = world.get_resource_mut::<Ui>()
            {
                ui.remove(root);
            }
            return;
        };
        let lines = hud_lines(world, &settings, rules);
        let Some(ui) = world.get_resource_mut::<Ui>() else {
            return;
        };
        if let Some((root, nodes, shown)) = &mut self.hud
            && ui.contains(*root)
            && nodes.len() == lines.len()
        {
            for ((node, old), new) in nodes.iter().zip(shown.iter_mut()).zip(&lines) {
                if old != new {
                    ui.set_text(*node, new.clone());
                    *old = new.clone();
                }
            }
            return;
        }
        if let Some((root, _, _)) = self.hud.take() {
            ui.remove(root);
        }
        let root = ui.add(
            None,
            UiNode::column()
                .with_class("hud")
                .anchored([0.0, 0.0], [24.0, 24.0])
                .with_id("game_hud")
                .with_layer(5),
        );
        let nodes = lines
            .iter()
            .map(|l| ui.add(Some(root), UiNode::text(l.clone())))
            .collect();
        if lines.is_empty() {
            ui.set_visible(root, false);
        }
        self.hud = Some((root, nodes, lines));
    }
}

/// The HUD's text lines.
pub fn hud_lines(world: &World, settings: &HudSettings, rules: Option<&GameRules>) -> Vec<String> {
    let mut lines = Vec::new();
    let vars = world.get_resource::<GameVars>();
    if settings.vars.is_empty() {
        if let Some(v) = vars {
            for (name, value) in &v.values {
                lines.push(format!("{} {}", label_of(name), number(*value)));
            }
        }
    } else {
        for h in &settings.vars {
            let value = vars.map_or(0.0, |v| v.get(&h.var));
            let label = if h.label.is_empty() {
                label_of(&h.var)
            } else {
                h.label.clone()
            };
            lines.push(format!("{label} {}", number(value)));
        }
    }
    if settings.groups {
        let groups: BTreeSet<String> = world
            .query::<&Interactable>()
            .iter()
            .filter(|i| !i.group.is_empty())
            .map(|i| i.group.clone())
            .collect();
        for g in groups {
            let (used, total) = group_progress(world, &g);
            // Only spawned copies or a spawner's template: nothing to count.
            if total == 0 {
                continue;
            }
            lines.push(format!("{} {used} / {total}", label_of(&g)));
        }
    }
    if settings.health
        && let Some(h) = find_interactor(world).and_then(|p| world.get::<super::Health>(p))
    {
        lines.push(format!(
            "Health {} / {}",
            number(h.current as f64),
            number(h.max as f64)
        ));
    }
    let state = world.get_resource::<GameState>();
    if let Some(r) = rules {
        if settings.lives && r.lives > 0 {
            lines.push(format!("Lives {}", state.map_or(r.lives, |s| s.lives_left)));
        }
        if settings.timer && r.time_limit > 0.0 {
            let left = (r.time_limit as f64 - state.map_or(0.0, |s| s.elapsed))
                .max(0.0)
                .ceil() as u64;
            lines.push(format!("Time {}:{:02}", left / 60, left % 60));
        }
    }
    lines
}

/// `red_keys` → `Red keys`.
fn label_of(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut c = spaced.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Gives the interactor a [`SpawnPoint`] where it is first seen.
fn record_spawn_points(world: &mut World) {
    let Some(p) = find_interactor(world) else {
        return;
    };
    if world.has::<SpawnPoint>(p) {
        return;
    }
    let at = world
        .get::<Transform>(p)
        .map(|t| t.translation)
        .or_else(|| {
            world
                .get::<GlobalTransform>(p)
                .map(|g| g.0.translation.into())
        });
    if let Some(at) = at {
        let _ = world.insert_one(p, SpawnPoint(at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_numbers() {
        assert_eq!(label_of("red_keys"), "Red keys");
        assert_eq!(label_of("score"), "Score");
        assert_eq!(number(3.0), "3");
        assert_eq!(number(2.5), "2.5");
    }
}
