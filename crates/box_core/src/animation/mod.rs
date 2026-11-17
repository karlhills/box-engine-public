//! Skeletal animation (M7): clips, skins, a clip player with crossfades and
//! a minimal state machine.
//!
//! - [`AnimationClip`] (asset): keyframed translation, rotation and scale
//!   channels with step, linear or cubic-spline interpolation, targeting
//!   entities by `Name`. Imported from glTF (`file#Animation/<name>`).
//! - [`Skin`] (asset, `file#Skin/<i>`): inverse bind matrices; the saved
//!   [`SkinBinding`] names the joint entities; the asset glue turns it into
//!   the runtime [`SkinnedMesh`] the renderer skins on the GPU.
//! - [`AnimationPlayer`] (saved component): plays a clip on its entity's
//!   descendants and crossfades when the clip changes.
//! - [`AnimationStateMachine`] (saved component) running an
//!   [`AnimationGraph`] (inline or a `.animgraph.json` asset): states,
//!   transitions on float, bool and trigger parameters.
//!
//! [`AnimationPlugin`] runs [`update_state_machines`] then
//! [`animate_players`] in `PostUpdate` before transform propagation, so
//! parameters set by game code in `Update` (or `FixedUpdate`) show in the
//! same frame. Players write the joints' `Transform`s rather than a matrix
//! palette: joints stay ordinary entities (things can be attached to a
//! hand, gameplay can read a bone), and change-driven propagation computes
//! their world transforms; the renderer builds the palette from those.
//! Property tweens are [`Tween`](crate::tween::Tween).

mod clip;
mod graph;
mod player;
mod skin;

pub use clip::{AnimationClip, Channel, Interpolation, Keyframes, Sample};
pub use graph::{
    AnimationCondition, AnimationGraph, AnimationGraphState, AnimationParameter,
    AnimationStateMachine, AnimationStateMachineState, AnimationTransition, ConditionTest,
    ParameterValue, update_state_machines,
};
pub use player::{AnimationPlayer, AnimationPlayerState, LayerInfo, animate_players};
pub use skin::{Skin, SkinBinding, SkinnedMesh, joint_matrix};

use crate::app::{App, Plugin, Stage};

/// Runs [`update_state_machines`] and [`animate_players`] in `PostUpdate`,
/// before transform propagation. `box_engine` apps include it.
pub struct AnimationPlugin;

impl Plugin for AnimationPlugin {
    fn build(&self, app: &mut App) {
        app.system(Stage::PostUpdate, update_state_machines)
            .before("propagate_transforms");
        app.system(Stage::PostUpdate, animate_players)
            .after("update_state_machines")
            .before("propagate_transforms");
    }
}

#[cfg(test)]
mod tests;
