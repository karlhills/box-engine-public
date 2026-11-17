//! Particle preview while editing (View ▸ Particles: Selected, All, None).
//!
//! The edit world runs no gameplay, but an emitter is hard to tune without
//! seeing it, so the editor steps the previewed emitters' runtime
//! [`ParticleState`] with the UI's frame time. It reads only saved data
//! (`ParticleEmitter`, `GlobalTransform`) and writes only `ParticleState`,
//! which is unregistered: never saved, never in the undo stack, and never
//! copied into Play (the play world is built from the saved scene and runs
//! `simulate_particles` itself). Unlike the game it never despawns
//! (`despawn_when_finished` is ignored), previews an emitter whose
//! `playing` is off as if on (its look is what is being tuned), and starts
//! a finished one-shot emitter again after a short pause.
//!
//! Cost: one `ParticleState::step` per previewed emitter per frame while
//! the viewport is shown and not playing, and a continuous repaint while
//! any emitter is previewed. The default previews only selected emitters,
//! so an idle editor with a torch in the scene does not redraw.

use std::borrow::Cow;
use std::collections::HashMap;

use box_core::prelude::{Entity, GlobalTransform};
use box_render::{Hidden, ParticleEmitter, ParticleState};

use crate::state::EditorState;

/// Which emitters the edit world simulates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParticlePreview {
    /// Emitters of the selected entities.
    #[default]
    Selected,
    All,
    None,
}

impl ParticlePreview {
    pub const ALL: [Self; 3] = [Self::Selected, Self::All, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::Selected => "Selected",
            Self::All => "All",
            Self::None => "None",
        }
    }
}

/// Seconds a finished one-shot emitter rests before its preview restarts.
pub const RESTART_PAUSE: f32 = 0.6;

/// The preview's own bookkeeping: how long each finished emitter has
/// rested.
#[derive(Clone, Debug, Default)]
pub struct PreviewClock {
    rest: HashMap<Entity, f32>,
}

impl EditorState {
    /// Advances the previewed emitters of the edit world by `dt` seconds
    /// (clamped to 0.1 s), adds their `ParticleState` on their first frame
    /// and removes it from emitters no longer previewed (hidden ones too).
    /// Does nothing during Play. Returns true while any emitter is
    /// previewed, so the caller keeps repainting.
    pub fn preview_particles(&mut self, dt: f32) -> bool {
        if self.is_playing() {
            return false;
        }
        let dt = dt.clamp(0.0, 0.1);
        let mode = self.viewport.view.particles;
        let world = &self.world;
        let mut rest = HashMap::new();
        let mut any = false;
        type Emitters<'a> = (
            Entity,
            &'a ParticleEmitter,
            &'a GlobalTransform,
            Option<&'a mut ParticleState>,
        );
        let game_hidden = box_core::visibility::hidden_entities(world);
        for (e, emitter, gt, state) in world.query::<Emitters>().iter() {
            let wanted = !world.has::<Hidden>(e)
                && !game_hidden.contains(&e)
                && match mode {
                    ParticlePreview::All => true,
                    ParticlePreview::Selected => world.uid(e).is_some_and(|u| self.is_selected(u)),
                    ParticlePreview::None => false,
                };
            match (wanted, state) {
                (false, Some(_)) => {
                    world.defer().remove::<ParticleState>(e);
                }
                (false, None) => {}
                (true, state) => {
                    any = true;
                    let emitter = if emitter.playing {
                        Cow::Borrowed(emitter)
                    } else {
                        Cow::Owned(ParticleEmitter {
                            playing: true,
                            ..emitter.clone()
                        })
                    };
                    match state {
                        Some(s) if s.is_finished() => {
                            let rested =
                                self.viewport.particles.rest.get(&e).copied().unwrap_or(0.0) + dt;
                            if rested >= RESTART_PAUSE {
                                s.restart(emitter.seed);
                                s.step(&emitter, &gt.0, dt);
                            } else {
                                rest.insert(e, rested);
                            }
                        }
                        Some(s) => s.step(&emitter, &gt.0, dt),
                        None => {
                            let mut s = ParticleState::new(emitter.seed);
                            s.step(&emitter, &gt.0, dt);
                            world.defer().insert_one(e, s);
                        }
                    }
                }
            }
        }
        self.viewport.particles.rest = rest;
        self.world.apply_deferred();
        any
    }
}
