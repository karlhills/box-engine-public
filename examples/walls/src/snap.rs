//! Snapping with hysteresis for whatever drags a wall or fence point: an
//! end connects within [`SNAP`] (10 cm) and lets go only beyond
//! [`SNAP_RELEASE`] (15 cm). The `Snapper` began here in M5 and moved to
//! the engine in M6 ([`box_engine::box_proc::snap`]), where the editor's
//! path tool uses it; this module re-exports it.
//!
//! The rules' fixed 10 cm test ([`crate::adjacency::SNAP`]) is the same
//! distance, so a snapped (coincident) end always connects after undo,
//! redo, save and reload.

pub use box_engine::box_proc::snap::{
    SNAP, SNAP_RELEASE, SnapHit, SnapSegment, SnapTargets, Snapper,
};
