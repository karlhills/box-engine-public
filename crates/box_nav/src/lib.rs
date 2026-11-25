//! Navigation for box-engine (docs/NAVIGATION.md, D21): where characters
//! that are not the player can walk, and how they get somewhere.
//!
//! - [`bake_tile`] turns world-space triangles (the static colliders) into
//!   one [`NavTile`] of walkable convex polygons with
//!   [rerecast](https://crates.io/crates/rerecast), a Rust port of Recast.
//!   Tiles are squares of [`NavConfig::tile_size`] metres, so a change
//!   rebuilds only the tiles it touched.
//! - [`NavMesh`] holds the tiles, links neighbouring tiles' polygons, and
//!   answers queries: [`NavMesh::find_path`] (A* over polygons, then the
//!   funnel algorithm for the shortest corridor path),
//!   [`NavMesh::closest_point`], [`NavMesh::is_walkable_line`],
//!   [`NavMesh::random_point_near`] and [`NavMesh::height_at`].
//!
//! No world access and no engine types: inputs are triangles and points, so
//! everything here runs and is tested without a GPU or an app. The same
//! triangles and config give the same mesh and paths every time.

mod bake;
mod mesh;
mod query;

pub use bake::{BakeError, bake_tile};
pub use mesh::{NavEdge, NavMesh, NavPoly, NavTile, PolyRef, TileCoord};
pub use query::{NavPath, PathStatus};

use glam::{Vec2, Vec3};

/// What counts as walkable, for one agent size, and how finely to sample.
/// Defaults match `KinematicCharacter`'s (0.4 m radius, 1.8 m tall, about
/// 53° slopes, 0.3 m steps), so paths only lead where the character can go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavConfig {
    /// Agent radius: walkable areas keep this far from walls.
    pub agent_radius: f32,
    /// Agent height: lower ceilings block.
    pub agent_height: f32,
    /// Steepest walkable slope, in radians.
    pub max_slope: f32,
    /// Highest step climbed (stairs, kerbs).
    pub step_height: f32,
    /// Voxel size on the ground plane; smaller is finer and slower.
    pub cell_size: f32,
    /// Edge of a square tile, in metres.
    pub tile_size: f32,
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            agent_radius: 0.4,
            agent_height: 1.8,
            max_slope: 0.6_f32.acos(),
            step_height: 0.3,
            cell_size: 0.15,
            tile_size: 32.0,
        }
    }
}

impl NavConfig {
    /// Voxels along a tile's edge.
    pub fn tile_cells(&self) -> u16 {
        (self.tile_size / self.cell_size).round().clamp(8.0, 4000.0) as u16
    }

    /// The tile edge actually used (a whole number of cells).
    pub fn tile_edge(&self) -> f32 {
        self.tile_cells() as f32 * self.cell_size
    }

    /// The tile holding `p` (on the ground plane).
    pub fn tile_of(&self, p: Vec3) -> TileCoord {
        let e = self.tile_edge();
        TileCoord {
            x: (p.x / e).floor() as i32,
            z: (p.z / e).floor() as i32,
        }
    }

    /// The tile's square on the ground plane: (min, max) of x and z.
    pub fn tile_rect(&self, t: TileCoord) -> (Vec2, Vec2) {
        let e = self.tile_edge();
        let min = Vec2::new(t.x as f32 * e, t.z as f32 * e);
        (min, min + Vec2::splat(e))
    }

    /// Every tile a box (min, max) touches on the ground plane.
    pub fn tiles_in(&self, min: Vec3, max: Vec3) -> impl Iterator<Item = TileCoord> + use<> {
        let a = self.tile_of(min);
        let b = self.tile_of(max);
        (a.z..=b.z).flat_map(move |z| (a.x..=b.x).map(move |x| TileCoord { x, z }))
    }

    /// How far triangles outside a tile still matter to it (erosion and
    /// the border Recast needs), in metres.
    pub fn tile_margin(&self) -> f32 {
        (self.agent_radius / self.cell_size).ceil() * self.cell_size + 3.0 * self.cell_size
    }
}
