//! box_arch: architectural construction for box-engine (M6,
//! docs/PROCEDURAL.md section 9). An optional plugin: add [`ArchPlugin`]
//! to a game (`Game::new().with_plugin(ArchPlugin)`); it is not part of the
//! engine's default plugins.
//!
//! Generators (listed under Add ▸ Architecture):
//!
//! - [`Wall`]: a solid wall along a polyline with arcs or a spline, with
//!   explicit junctions at its ends ([`WallEnd`]), openings ([`Opening`]:
//!   doors, windows, passages with flat, round or pointed heads) cut in 2D
//!   elevation with reveals, panels and sills, optional plinth, cornice
//!   and coping bands, and a top that can follow a roof's underside.
//! - [`Roof`]: closed-form gable, hip and shed roofs over rectangles, cone
//!   and dome roofs over circles; [`Roof::underside`] gives the underside
//!   as data for the walls under it.
//! - [`Platform`]: a slab from a footprint polygon, with a lip or trim.
//! - [`Stair`]: a straight flight between two points.
//!
//! Plus [`Anchor`] (decorations that stay on a generator's surface,
//! resolved by [`anchor::resolve_anchors`]) and the [`junction`] helpers
//! that compute wall end parameters for mitres, butts and T junctions.
//!
//! Everything is generic: no rooms, door placement rules or world documents.
//! Those belong to game rules, which choose junctions and ownership and
//! write the results into these generators' parameters (spike S6).
//!
//! Every generator bakes ambient occlusion into vertex colours (strength
//! `ao`, 0 disables it), emits UV set 0 in metres and UV set 1 as
//! weathering data (walls: height above the foot, depth below the top).

pub mod anchor;
mod clip;
pub mod consts;
pub mod junction;
mod mesh;
pub mod platform;
pub mod roof;
pub mod stair;
pub mod underside;
pub mod wall;

pub use anchor::{
    ANCHORS_SYSTEM, Anchor, AnchorSurface, AnchorTargets, Anchorable, WallSide, resolve_anchors,
};
pub use platform::{Footprint, Platform, PlatformEdge};
pub use roof::{RidgeAxis, Roof, RoofEnd, RoofShape};
pub use stair::{Stair, StairLayout, StairStyle};
pub use underside::{RoofUnderside, UndersideSurface};
pub use wall::{Opening, OpeningKind, OpeningTop, Wall, WallEnd, WallPath, WallPoint};

use box_core::app::{App, Plugin, Stage};
use box_core::reflect::Registry;
use box_proc::generator::GeneratorRegistry;

/// Registers the architectural generators and [`Anchor`], and resolves
/// anchors every frame (in `PostUpdate`, after generation; also in an
/// editor's edit world, through `App::add_edit_world_system`).
#[derive(Clone, Copy, Debug, Default)]
pub struct ArchPlugin;

impl Plugin for ArchPlugin {
    fn name(&self) -> &str {
        "box_arch"
    }

    fn register(&self, registry: &mut Registry) {
        let results = [
            registry.register_generator::<Wall>(),
            registry.register_generator::<Roof>(),
            registry.register_generator::<Platform>(),
            registry.register_generator::<Stair>(),
            registry.register::<Anchor>(),
        ];
        for r in results {
            if let Err(e) = r {
                tracing::error!(target: "box_arch", "registration failed: {e}");
            }
        }
    }

    fn build(&self, app: &mut App) {
        // Keep targets a game plugin built earlier may have added.
        app.world.resource_or_default::<AnchorTargets>();
        // Also in an editor's edit world, so decorations follow walls while
        // editing (it is change-driven and reads only saved data).
        app.add_edit_world_system(Stage::PostUpdate, ANCHORS_SYSTEM, |w| {
            resolve_anchors(w);
        });
    }
}
