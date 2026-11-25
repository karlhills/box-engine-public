//! Viewport gizmos for the selected behaviours (docs/BEHAVIOURS.md): an
//! `Interactable`'s reach around its interaction point, a `Mover`'s path, a
//! `Spawner`'s area, a `Chaser`'s sight and give-up distances and patrol.
//! Drawn on the post-tonemap overlay layer for the selection only; a few
//! dozen lines each, only while selected.

use box_core::color::Color;
use box_core::transform::{GlobalTransform, compute_global};
use box_core::world::{Entity, World};
use box_engine::behaviours::{Chaser, Mover, MoverMode, Spawner};
use box_engine::interact::Interactable;
use box_render::{DebugDraw, LineLayer};
use glam::Vec3;

use crate::state::EditorState;

const SEGMENTS: usize = 48;
const LAYER: LineLayer = LineLayer::DisplayOverlay;

/// Colours (sRGB): reach in green, paths in yellow, spawn areas in violet,
/// sight in orange, give-up in faint orange.
const REACH: [u8; 3] = [0x4c, 0xd9, 0x7b];
const PATH: [u8; 3] = [0xf2, 0xc9, 0x4c];
const AREA: [u8; 3] = [0xb0, 0x7c, 0xf2];
const SIGHT: [u8; 3] = [0xf2, 0x8c, 0x3c];

fn color([r, g, b]: [u8; 3], alpha: f32) -> Color {
    Color::srgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0).with_alpha(alpha)
}

fn ring(dd: &mut DebugDraw, centre: Vec3, radius: f32, c: Color) {
    if radius <= 0.0 {
        return;
    }
    let at = |i: usize| {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        centre + Vec3::new(a.cos(), 0.0, a.sin()) * radius
    };
    for i in 0..SEGMENTS {
        dd.line_on(LAYER, at(i), at(i + 1), c);
    }
}

fn cross(dd: &mut DebugDraw, p: Vec3, size: f32, c: Color) {
    dd.line_on(LAYER, p - Vec3::X * size, p + Vec3::X * size, c);
    dd.line_on(LAYER, p - Vec3::Z * size, p + Vec3::Z * size, c);
}

/// A point given in `e`'s parent space, in world space.
fn in_parent(world: &World, e: Entity, local: Vec3) -> Vec3 {
    match world.parent(e) {
        Some(p) => compute_global(world, p).transform_point3(local),
        None => local,
    }
}

/// Draws the gizmos of every selected entity's behaviours.
pub(crate) fn draw_behaviour_gizmos(state: &EditorState, dd: &mut DebugDraw) {
    let world = &state.world;
    for &uid in &state.selection {
        let Some(e) = world.entity_by_uid(uid) else {
            continue;
        };
        let Some(pos) = world
            .get::<GlobalTransform>(e)
            .map(|g| Vec3::from(g.0.translation))
        else {
            continue;
        };
        if let Some(i) = world.get::<Interactable>(e) {
            let p = pos + i.offset;
            ring(dd, p, i.radius, color(REACH, 0.9));
            cross(dd, p, 0.15, color(REACH, 0.9));
        }
        if let Some(m) = world.get::<Mover>(e) {
            // Points are offsets from where it starts, in its parent's axes.
            let start = world
                .get::<box_core::transform::Transform>(e)
                .map_or(Vec3::ZERO, |t| t.translation);
            let mut pts: Vec<Vec3> = m
                .points
                .iter()
                .map(|p| in_parent(world, e, start + *p))
                .collect();
            if m.mode == MoverMode::Loop
                && let Some(first) = pts.first().copied()
            {
                pts.push(first);
            }
            for w in pts.windows(2) {
                dd.line_on(LAYER, w[0], w[1], color(PATH, 0.95));
            }
            for p in &pts {
                cross(dd, *p, 0.2, color(PATH, 0.95));
            }
        }
        if let Some(s) = world.get::<Spawner>(e) {
            ring(dd, pos, s.radius.max(0.25), color(AREA, 0.9));
        }
        if let Some(c) = world.get::<Chaser>(e) {
            ring(dd, pos, c.sight, color(SIGHT, 0.9));
            ring(dd, pos, c.give_up, color(SIGHT, 0.35));
            if !c.patrol.is_empty() {
                let start = world
                    .get::<box_core::transform::Transform>(e)
                    .map_or(Vec3::ZERO, |t| t.translation);
                let mut pts: Vec<Vec3> = c
                    .patrol
                    .iter()
                    .map(|p| in_parent(world, e, start + *p))
                    .collect();
                if let Some(first) = pts.first().copied() {
                    pts.push(first);
                }
                for w in pts.windows(2) {
                    dd.line_on(LAYER, w[0], w[1], color(PATH, 0.95));
                }
                for p in &pts {
                    cross(dd, *p, 0.2, color(PATH, 0.95));
                }
            }
        }
    }
}
