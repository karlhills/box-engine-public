//! The `terrain` rule: every [`Terrain`] becomes chunk parts
//! (`box_proc::terrain::terrain_parts`) drawn with the standard material
//! (vertex colours carry the layers) and, with `Terrain::collide`, a mesh
//! collider each. Added to every app by [`crate::app_with_world`], so it
//! runs in the editor's edit world, in Play, the player and `box render`.

use box_core::color::Color;
use box_core::material::StandardMaterial;
use box_core::prelude::*;
use box_proc::Spline;
use box_proc::rule::{RuleAppExt, RuleInput, RuleOutput};
use box_proc::terrain::{Terrain, terrain_parts};
use box_proc::terrain_path::{TERRAIN_PATH_RULE, terrain_path_rule};

/// The rule's name in the rule list.
pub const TERRAIN_RULE: &str = "terrain";

/// The material of terrain chunks: white, so the layer colours show as
/// they are; matte.
fn chunk_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        roughness: 0.95,
        ..StandardMaterial::default()
    }
}

fn terrain_rule(input: &RuleInput<Terrain>) -> RuleOutput {
    let mut out = RuleOutput::default();
    for intent in input.intents {
        let (parts, warnings) = terrain_parts(intent);
        for w in warnings {
            out.warn(intent.uid, w);
        }
        for part in parts {
            let part = part.with(chunk_material());
            #[cfg(feature = "physics")]
            let part = if intent.value.collide {
                part.with(box_physics::Collider::new(box_physics::ColliderShape::Mesh))
            } else {
                part
            };
            out.push(part);
        }
    }
    out
}

/// The label of the system that updates the ground and moves
/// `FollowGround` entities onto it (`PostUpdate`, before the rules).
pub const GROUND_SYSTEM: &str = "box_engine::ground";

/// Updates the world's `Ground` from its terrains and moves `FollowGround`
/// entities onto it. Change-driven: a compare per terrain and per follower
/// when nothing moved.
pub fn update_ground(world: &mut World) {
    box_proc::ground::update_ground(world);
    box_proc::ground::follow_ground(world);
}

/// Adds the `terrain` rule and the ground (`Ground` resource,
/// `FollowGround`), in every app and in an editor's edit world.
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_rule(TERRAIN_RULE, terrain_rule);
        app.add_rule(TERRAIN_PATH_RULE, terrain_path_rule)
            .rule_reads::<Terrain>(TERRAIN_PATH_RULE)
            .rule_reads::<Spline>(TERRAIN_PATH_RULE);
        app.system(Stage::PostUpdate, update_ground)
            .label(GROUND_SYSTEM)
            .before(box_proc::rule::RULES_SYSTEM);
    }
}
