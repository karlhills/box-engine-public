//! Editor-only code (feature `editor`): a command palette action.

use box_editor::actions::Action;
use box_editor::{EditorPlugin, EditorState};

use crate::gameplay::Score;

/// Adds **Reset Score** (category Game) to the command palette.
pub struct GuideEditor;

impl EditorPlugin for GuideEditor {
    fn build(&self, editor: &mut EditorState) {
        editor.register_action(
            Action::new("guide.reset_score", "Game", "Reset Score", |state| {
                // During Play, `state.world` is the play world.
                if let Some(score) = state.world.get_resource_mut::<Score>() {
                    score.points = 0;
                }
                state.notify("Score reset");
            })
            .tooltip("Sets the running game's score back to zero")
            .keywords("points")
            .enabled_when(|state| {
                if state.is_playing() {
                    Ok(())
                } else {
                    Err("Start Play first".into())
                }
            }),
        );
    }
}
