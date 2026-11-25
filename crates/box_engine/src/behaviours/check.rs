//! Gameplay rules that cannot work, found without playing
//! (docs/BEHAVIOURS.md, B2b): `box check` reports them as warnings, and MCP
//! `check` with it. Each problem says what is wrong and what to do.

use std::collections::BTreeSet;

use box_core::identity::{Name, Uid};
use box_core::world::{Entity, World};

use super::{GameRules, Spawner};
use crate::interact::{Condition, InteractAction, Interactable};

/// One rule that cannot work.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleProblem {
    /// Stable code: `group_empty`, `var_never_set`, `no_camera`,
    /// `switch_scene_empty`, `spawner_empty`, `nav_unreachable`.
    pub code: &'static str,
    pub message: String,
    /// The entity it is about, if any.
    pub entity: Option<Uid>,
    pub entity_name: Option<String>,
}

fn problem(world: &World, e: Option<Entity>, code: &'static str, message: String) -> RuleProblem {
    RuleProblem {
        code,
        message,
        entity: e.and_then(|e| world.uid(e)),
        entity_name: e.and_then(|e| world.get::<Name>(e).map(|n| n.0.clone())),
    }
}

fn label(world: &World, e: Entity) -> String {
    world
        .get::<Name>(e)
        .map_or_else(|| "an entity".to_string(), |n| format!("`{}`", n.0))
}

/// Every action list of the scene with its owner: interactables' actions
/// and health's `on_damage`/`on_zero`.
fn action_lists(world: &World) -> Vec<(Entity, Vec<InteractAction>)> {
    let mut out = Vec::new();
    for (e, i) in world.query::<(Entity, &Interactable)>().iter() {
        out.push((e, i.actions.clone()));
    }
    for (e, h) in world.query::<(Entity, &super::Health)>().iter() {
        out.push((e, h.on_damage.clone()));
        out.push((e, h.on_zero.clone()));
    }
    out
}

/// Conditions of the scene with their owner (`None`: the scene's rules).
fn conditions(world: &World) -> Vec<(Option<Entity>, Condition)> {
    let mut out = Vec::new();
    if let Some(r) = world.get_resource::<GameRules>() {
        for c in r.win.iter().chain(&r.lose) {
            out.push((None, c.clone()));
        }
    }
    for (e, i) in world.query::<(Entity, &Interactable)>().iter() {
        for c in &i.requires {
            out.push((Some(e), c.clone()));
        }
    }
    out
}

/// The rules of a scene (not yet playing) that cannot work.
pub fn rule_problems(world: &World) -> Vec<RuleProblem> {
    let mut out = Vec::new();
    let lists = action_lists(world);

    // Groups that nothing belongs to.
    let groups: BTreeSet<String> = world
        .query::<&Interactable>()
        .iter()
        .map(|i| i.group.clone())
        .collect();
    // Vars something writes (actions); scripts may write any.
    let scripted = has_component_named(world, "Script");
    let mut written = BTreeSet::new();
    for (_, list) in &lists {
        for a in list {
            if let InteractAction::AddVar { var, .. } | InteractAction::SetVar { var, .. } = a {
                written.insert(var.clone());
            }
        }
    }
    for (owner, c) in conditions(world) {
        let whose = match owner {
            Some(e) => format!("{}'s `requires`", label(world, e)),
            None => "the scene's GameRules".to_string(),
        };
        match &c {
            Condition::GroupDone { group } if !groups.contains(group) => {
                out.push(problem(
                    world,
                    owner,
                    "group_empty",
                    format!(
                        "{whose} waits for every interactable of group `{group}`, but no Interactable has `group: \"{group}\"`, so it never holds; set the group on the pickups (the Pickup preset uses `pickups`) or fix the name"
                    ),
                ));
            }
            Condition::AtLeast { var, value } | Condition::Equals { var, value }
                if *value != 0.0 && !scripted && !written.contains(var) =>
            {
                out.push(problem(
                    world,
                    owner,
                    "var_never_set",
                    format!(
                        "{whose} needs the game variable `{var}`, but no AddVar or SetVar action sets it, so it stays 0; add an action that changes `{var}` (the Pickup preset adds to `score`) or fix the name"
                    ),
                ));
            }
            _ => {}
        }
    }

    // Scene switches without a scene.
    for (e, list) in &lists {
        if list
            .iter()
            .any(|a| matches!(a, InteractAction::SwitchScene { scene } if scene.trim().is_empty()))
        {
            out.push(problem(
                world,
                Some(*e),
                "switch_scene_empty",
                format!(
                    "{} has a SwitchScene action with no scene; type the scene's name (such as `level2`) into it",
                    label(world, *e)
                ),
            ));
        }
    }

    // Spawners with nothing to copy.
    for (e, s) in world.query::<(Entity, &Spawner)>().iter() {
        let template = match s.template {
            Some(u) => world.entity_by_uid(u),
            None => world.children(e).into_iter().next(),
        };
        if template.is_none() {
            out.push(problem(
                world,
                Some(e),
                "spawner_empty",
                format!(
                    "Spawner {} has nothing to copy: put the thing to spawn under it as a child, or choose an entity in `template`",
                    label(world, e)
                ),
            ));
        }
    }

    // A player without a camera to see it through.
    #[cfg(feature = "physics")]
    if let Some((p, _)) = world
        .query::<(Entity, &super::PlayerController)>()
        .iter()
        .next()
        && world.query::<&box_render::Camera>().iter().next().is_none()
    {
        out.push(problem(
            world,
            Some(p),
            "no_camera",
            format!(
                "{} is a player but the scene has no camera, so nothing is drawn in Play; place the Player preset (it brings a follow camera) or add a Camera with FollowCamera",
                label(world, p)
            ),
        ));
    }
    out
}

fn has_component_named(world: &World, name: &str) -> bool {
    world
        .query::<Entity>()
        .iter()
        .any(|e| world.component_names(e).contains(&name))
}

/// Enemies that cannot reach the player along the navmesh. Needs a world
/// whose navmesh is built (`nav_glue::settle_navigation`).
#[cfg(feature = "navigation")]
pub fn reachability_problems(world: &World) -> Vec<RuleProblem> {
    use box_core::transform::Transform;
    use box_physics::KinematicCharacter;
    let feet = |e: Entity| -> Option<glam::Vec3> {
        let t = world.get::<Transform>(e)?.translation;
        let h = world
            .get::<KinematicCharacter>(e)
            .map_or(0.92, |k| k.half_height() + k.skin);
        Some(t - glam::Vec3::Y * h)
    };
    let Some(player) = crate::interact::find_interactor(world) else {
        return Vec::new();
    };
    let Some(target) = feet(player) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (e, _) in world.query::<(Entity, &super::Chaser)>().iter() {
        if super::is_spawn_template(world, e) {
            continue;
        }
        let Some(from) = feet(e) else { continue };
        if !crate::nav_glue::nav_reachable(world, from, target) {
            out.push(problem(
                world,
                Some(e),
                "nav_unreachable",
                format!(
                    "enemy {} cannot walk to where {} starts: no walkable way between them (a wall, a gap, a drop or a bridge hidden at the start); check with `box nav` or View ▸ Navigation",
                    label(world, e),
                    label(world, player)
                ),
            ));
        }
    }
    out
}
