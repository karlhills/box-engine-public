//! Animation asset glue, run by [`sync_assets`](crate::asset_glue::sync_assets):
//!
//! - every [`SkinBinding`] entity gets a runtime [`SkinnedMesh`]: the skin
//!   asset's handle and its joint uids resolved to entities. Rebuilt when
//!   the binding changes, when a joint entity is despawned (a prefab
//!   reload), or, while something could not be resolved, when the asset
//!   lookup changes;
//! - every clip an [`AnimationPlayer`] or [`AnimationStateMachine`] names
//!   (inline graph states, graph files and their states) is requested from
//!   the server once, so the [`AssetLookup`] resolves it for the players.
//!
//! Per frame: one query over `SkinBinding` entities (a component compare
//! and a liveness check per joint), and one hash lookup per clip
//! reference of each player and state machine.

use std::collections::HashSet;

use box_assets::AssetServer;
use box_core::animation::{
    AnimationClip, AnimationGraph, AnimationPlayer, AnimationStateMachine, Skin, SkinBinding,
    SkinnedMesh,
};
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::world::{Entity, Without, World};

/// Resource: references already requested, and messages already logged.
#[derive(Default)]
struct AnimationGlue {
    requested: HashSet<AssetRef>,
    graphs_generation: u64,
    logged: HashSet<String>,
}

/// What the glue last built for a `SkinBinding` entity. Runtime only.
#[derive(Debug)]
struct SkinBindingState {
    source: SkinBinding,
    built: Option<SkinnedMesh>,
    /// The skin or a joint could not be resolved.
    incomplete: bool,
    lookup_generation: u64,
}

pub(crate) fn sync_animation(world: &mut World, server: Option<&AssetServer>) {
    profiling::scope!("sync_animation");
    let mut glue = world.remove_resource::<AnimationGlue>().unwrap_or_default();
    sync_skins(world, server, &mut glue);
    if let Some(server) = server {
        request_clips(world, server, &mut glue);
    }
    world.insert_resource(glue);
}

fn sync_skins(world: &mut World, server: Option<&AssetServer>, glue: &mut AnimationGlue) {
    let lookup_generation = world
        .get_resource::<AssetLookup>()
        .map_or(0, AssetLookup::generation);
    let mut todo: Vec<(Entity, SkinBinding)> = Vec::new();
    for (e, binding, state, skinned) in world
        .query::<(
            Entity,
            &SkinBinding,
            Option<&SkinBindingState>,
            Option<&SkinnedMesh>,
        )>()
        .iter()
    {
        let rebuild = match state {
            None => true,
            Some(s) => {
                s.source != *binding
                    || skinned != s.built.as_ref()
                    || (s.incomplete && s.lookup_generation != lookup_generation)
                    || skinned.is_some_and(|m| m.joints.iter().any(|&j| !world.contains(j)))
            }
        };
        if rebuild {
            todo.push((e, binding.clone()));
        }
    }
    // SkinBinding removed: remove what the glue built.
    let removed: Vec<(Entity, Option<SkinnedMesh>)> = world
        .query::<Without<(Entity, &SkinBindingState), &SkinBinding>>()
        .iter()
        .map(|(e, s)| (e, s.built.clone()))
        .collect();
    for (e, built) in removed {
        let _ = world.remove_one::<SkinBindingState>(e);
        if built.is_some() && world.get::<SkinnedMesh>(e).map(|m| (*m).clone()) == built {
            let _ = world.remove_one::<SkinnedMesh>(e);
        }
    }
    for (e, binding) in todo {
        let (built, incomplete) = resolve(world, server, glue, &binding);
        match &built {
            Some(m) => {
                let _ = world.insert_one(e, m.clone());
            }
            None => {
                let _ = world.remove_one::<SkinnedMesh>(e);
            }
        }
        let _ = world.insert_one(
            e,
            SkinBindingState {
                source: binding,
                built,
                incomplete,
                lookup_generation,
            },
        );
    }
}

/// The runtime skin for `binding` and whether something was unresolved.
/// Missing joints stay at the mesh's bind pose (the entity itself is used).
fn resolve(
    world: &World,
    server: Option<&AssetServer>,
    glue: &mut AnimationGlue,
    binding: &SkinBinding,
) -> (Option<SkinnedMesh>, bool) {
    if binding.skin.is_none() {
        return (None, false);
    }
    let skin: Option<Handle<Skin>> = match server {
        Some(server) => match server.load::<Skin>(&binding.skin) {
            Ok(h) => Some(h),
            Err(err) => {
                log_once(glue, format!("skin {}: {err}", binding.skin));
                None
            }
        },
        None => world
            .get_resource::<AssetLookup>()
            .and_then(|l| l.get::<Skin>(&binding.skin)),
    };
    let Some(skin) = skin else {
        return (None, true);
    };
    let mut incomplete = false;
    let mut joints = Vec::with_capacity(binding.joints.len());
    for uid in &binding.joints {
        match world.entity_by_uid(*uid) {
            Some(e) => joints.push(e),
            None => {
                incomplete = true;
                log_once(
                    glue,
                    format!(
                        "skin {}: joint {uid} is not in the world; that joint stays in bind pose",
                        binding.skin
                    ),
                );
                joints.push(Entity::DANGLING);
            }
        }
    }
    (Some(SkinnedMesh { skin, joints }), incomplete)
}

fn log_once(glue: &mut AnimationGlue, message: String) {
    if glue.logged.insert(message.clone()) {
        tracing::warn!(target: "box_engine::animation", "{message}");
    }
}

fn request_clips(world: &World, server: &AssetServer, glue: &mut AnimationGlue) {
    let mut wanted: Vec<AssetRef> = Vec::new();
    let want = |r: &AssetRef, wanted: &mut Vec<AssetRef>| {
        if r.is_some() && !glue.requested.contains(r) {
            wanted.push(r.clone());
        }
    };
    for p in world.query::<&AnimationPlayer>().iter() {
        want(&p.clip, &mut wanted);
    }
    let mut graphs: Vec<AssetRef> = Vec::new();
    for m in world.query::<&AnimationStateMachine>().iter() {
        if m.source.is_some() {
            if !glue.requested.contains(&m.source) {
                graphs.push(m.source.clone());
            }
        } else {
            for s in &m.graph.states {
                want(&s.clip, &mut wanted);
            }
        }
    }
    // States of loaded graph files (scanned when the store changes).
    if let Some(store) = world.get_resource::<Assets<AnimationGraph>>()
        && store.generation() != glue.graphs_generation
    {
        glue.graphs_generation = store.generation();
        for (_, _, g) in store.iter() {
            for s in &g.states {
                want(&s.clip, &mut wanted);
            }
        }
    }
    for r in graphs {
        if let Err(err) = server.load::<AnimationGraph>(&r) {
            log_once(glue, format!("animation graph {r}: {err}"));
        }
        glue.requested.insert(r);
    }
    for r in wanted {
        if glue.requested.contains(&r) {
            continue;
        }
        if let Err(err) = server.load::<AnimationClip>(&r) {
            log_once(glue, format!("animation clip {r}: {err}"));
        }
        glue.requested.insert(r);
    }
}
