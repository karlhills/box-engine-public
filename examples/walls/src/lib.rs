//! Walls: neighbour-aware walls and fences from saved paths, the M5
//! acceptance scenario for derived entities (docs/PROCEDURAL_DIRECTION.md
//! section 7).
//!
//! - **Saved intent:** [`WallPath`] and [`FencePath`] ([`intent`]). Nothing
//!   else this crate defines is ever saved.
//! - **Rules:** `walls` ([`plan`]) and `fences` ([`fence`]), pure functions
//!   from every path to one derived part per path, added with
//!   `app.add_rule`. Both use the same path-end adjacency ([`adjacency`]);
//!   walls resolve junctions as mitres, butts and bevels, fences as shared
//!   corner posts.
//! - **Generators:** [`WallGen`] (a rectangular section swept along the
//!   polyline with joint-plane ends, `box_proc::sweep`) and [`FenceGen`]
//!   (posts placed with `divide_size` / `place_bays`, rails as sweeps).
//! - **Snapping:** ends connect within 10 cm. Hysteresis (let go only beyond
//!   15 cm) is per gesture ([`snap::Snapper`], from `box_proc::snap`), never
//!   saved. The editor's wall and fence drawing tools are in `editor`
//!   (feature `editor`).
//!
//! Scenes are data in `assets/scenes`, generated from the JSON edit commands
//! in `authoring/` ([`authoring`]).

pub mod adjacency;
pub mod authoring;
#[cfg(feature = "editor")]
pub mod editor;
pub mod fence;
pub mod intent;
pub mod plan;
pub mod snap;
pub mod wall_gen;

use box_engine::box_proc::rule::RuleAppExt;
use box_engine::prelude::*;

pub use fence::FenceGen;
pub use intent::{FencePath, WallPath};
pub use wall_gen::{WallEnd, WallGen};

/// Registers the intent components and generators and adds both rules.
pub struct WallsPlugin;

impl Plugin for WallsPlugin {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(WallPath, FencePath)>();
        let _ = registry.register_generator::<WallGen>();
        let _ = registry.register_generator::<FenceGen>();
    }

    fn build(&self, app: &mut App) {
        app.add_rule(plan::RULE, plan::plan);
        app.add_rule(fence::RULE, fence::plan);
        app.rule_reads::<WallPath>(fence::RULE);
    }
}
