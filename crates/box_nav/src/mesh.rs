//! The navmesh: tiles of convex walkable polygons, and the links between
//! neighbouring polygons (inside a tile and across tile edges).

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Range;

use glam::{Vec2, Vec3};

use crate::NavConfig;

/// A tile's grid position (tile `x`, `z` covers `x * edge .. (x + 1) * edge`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord {
    pub x: i32,
    pub z: i32,
}

impl TileCoord {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// The neighbour across portal direction `dir` (0: −x, 1: +z, 2: +x,
    /// 3: −z, Recast's order).
    pub fn neighbour(self, dir: u8) -> Self {
        match dir & 3 {
            0 => Self::new(self.x - 1, self.z),
            1 => Self::new(self.x, self.z + 1),
            2 => Self::new(self.x + 1, self.z),
            _ => Self::new(self.x, self.z - 1),
        }
    }
}

/// One polygon, by tile and index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PolyRef {
    pub tile: TileCoord,
    pub index: u16,
}

/// What lies across one edge of a polygon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavEdge {
    /// Nothing walkable: a wall or a drop.
    Wall,
    /// Another polygon of the same tile.
    Inner(u16),
    /// The tile's edge in a direction (0: −x, 1: +z, 2: +x, 3: −z); linked
    /// to the neighbour tile's polygons when that tile is loaded.
    Portal(u8),
}

/// A convex walkable polygon.
#[derive(Clone, Debug, PartialEq)]
pub struct NavPoly {
    /// Indices into the tile's `vertices`, in order around the polygon.
    pub verts: Vec<u16>,
    /// `edges[j]` is the edge from `verts[j]` to `verts[j + 1]`.
    pub edges: Vec<NavEdge>,
    /// Its height-detail triangles in the tile's `detail_triangles`.
    pub detail: Range<u32>,
    /// Bounds of its vertices.
    pub min: Vec3,
    pub max: Vec3,
    /// Average of its vertices.
    pub center: Vec3,
    /// Area on the ground plane, in m².
    pub area: f32,
}

impl NavPoly {
    pub(crate) fn new(
        verts: Vec<u16>,
        edges: Vec<NavEdge>,
        detail: Range<u32>,
        vertices: &[Vec3],
    ) -> Self {
        let pts: Vec<Vec3> = verts.iter().map(|&i| vertices[i as usize]).collect();
        let min = pts
            .iter()
            .copied()
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let max = pts
            .iter()
            .copied()
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        let center = pts.iter().copied().sum::<Vec3>() / pts.len().max(1) as f32;
        let mut area = 0.0;
        for i in 0..pts.len() {
            let a = pts[i];
            let b = pts[(i + 1) % pts.len()];
            area += a.x * b.z - b.x * a.z;
        }
        Self {
            verts,
            edges,
            detail,
            min,
            max,
            center,
            area: (area * 0.5).abs(),
        }
    }
}

/// One baked tile.
#[derive(Clone, Debug, PartialEq)]
pub struct NavTile {
    pub coord: TileCoord,
    /// Polygon vertices, in world space.
    pub vertices: Vec<Vec3>,
    pub polys: Vec<NavPoly>,
    /// Height-detail vertices, in world space.
    pub detail_vertices: Vec<Vec3>,
    /// Height-detail triangles (indices into `detail_vertices`).
    pub detail_triangles: Vec<[u32; 3]>,
    /// Bounds of every polygon.
    pub min: Vec3,
    pub max: Vec3,
}

impl NavTile {
    pub(crate) fn new(
        coord: TileCoord,
        vertices: Vec<Vec3>,
        polys: Vec<NavPoly>,
        detail_vertices: Vec<Vec3>,
        detail_triangles: Vec<[u32; 3]>,
    ) -> Self {
        let min = polys
            .iter()
            .fold(Vec3::splat(f32::INFINITY), |m, p| m.min(p.min));
        let max = polys
            .iter()
            .fold(Vec3::splat(f32::NEG_INFINITY), |m, p| m.max(p.max));
        Self {
            coord,
            vertices,
            polys,
            detail_vertices,
            detail_triangles,
            min,
            max,
        }
    }

    /// The polygon's corner positions.
    pub fn poly_points(&self, p: &NavPoly) -> Vec<Vec3> {
        p.verts.iter().map(|&i| self.vertices[i as usize]).collect()
    }

    /// The polygon's height-detail triangles.
    pub fn detail_tris<'a>(&'a self, p: &NavPoly) -> impl Iterator<Item = [Vec3; 3]> + 'a {
        self.detail_triangles[p.detail.start as usize..p.detail.end as usize]
            .iter()
            .map(|t| {
                [
                    self.detail_vertices[t[0] as usize],
                    self.detail_vertices[t[1] as usize],
                    self.detail_vertices[t[2] as usize],
                ]
            })
    }
}

/// A walkable connection to a neighbouring polygon through the segment
/// `a`–`b` (the shared edge, or the overlap of two tiles' edges).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Link {
    pub to: PolyRef,
    pub a: Vec3,
    pub b: Vec3,
}

/// A small, fast hasher for polygon refs (FxHash's mix): path searches
/// hash thousands of them, where SipHash showed up in the costs.
#[derive(Clone, Copy, Default)]
pub(crate) struct FxHasher(u64);

impl FxHasher {
    #[inline]
    fn add(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.add(*b as u64);
        }
    }
    fn write_u16(&mut self, v: u16) {
        self.add(v as u64);
    }
    fn write_u32(&mut self, v: u32) {
        self.add(v as u64);
    }
    fn write_i32(&mut self, v: i32) {
        self.add(v as u32 as u64);
    }
    fn write_u64(&mut self, v: u64) {
        self.add(v);
    }
    fn write_usize(&mut self, v: usize) {
        self.add(v as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

/// Maps and sets keyed by [`PolyRef`].
pub(crate) type FxBuild = BuildHasherDefault<FxHasher>;
pub(crate) type PolyMap<V> = HashMap<PolyRef, V, FxBuild>;

/// Every loaded tile, and the links between their polygons.
#[derive(Clone, Debug)]
pub struct NavMesh {
    config: NavConfig,
    tiles: BTreeMap<TileCoord, NavTile>,
    links: PolyMap<Vec<Link>>,
    revision: u64,
}

impl NavMesh {
    pub fn new(config: NavConfig) -> Self {
        Self {
            config,
            tiles: BTreeMap::new(),
            links: PolyMap::default(),
            revision: 0,
        }
    }

    pub fn config(&self) -> &NavConfig {
        &self.config
    }

    /// Bumped by every tile change; paths planned before a change may be
    /// stale.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn tiles(&self) -> impl Iterator<Item = &NavTile> {
        self.tiles.values()
    }

    pub fn tile(&self, coord: TileCoord) -> Option<&NavTile> {
        self.tiles.get(&coord)
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Polygons in all tiles.
    pub fn poly_count(&self) -> usize {
        self.tiles.values().map(|t| t.polys.len()).sum()
    }

    /// Adds or replaces the tile at `tile.coord` and relinks it with its
    /// neighbours.
    pub fn insert_tile(&mut self, tile: NavTile) {
        let coord = tile.coord;
        self.tiles.insert(coord, tile);
        self.relink(coord);
    }

    /// Sets the tile at `coord`: `Some` inserts, `None` removes.
    pub fn set_tile(&mut self, coord: TileCoord, tile: Option<NavTile>) {
        match tile {
            Some(t) => {
                debug_assert_eq!(t.coord, coord);
                self.insert_tile(t);
            }
            None => self.remove_tile(coord),
        }
    }

    /// Removes the tile at `coord` (and links into it).
    pub fn remove_tile(&mut self, coord: TileCoord) {
        if self.tiles.remove(&coord).is_some() {
            self.relink(coord);
        }
    }

    /// Removes every tile.
    pub fn clear(&mut self) {
        self.tiles.clear();
        self.links.clear();
        self.revision += 1;
    }

    pub fn poly(&self, r: PolyRef) -> Option<(&NavTile, &NavPoly)> {
        let t = self.tiles.get(&r.tile)?;
        Some((t, t.polys.get(r.index as usize)?))
    }

    pub(crate) fn links(&self, r: PolyRef) -> &[Link] {
        self.links.get(&r).map_or(&[], Vec::as_slice)
    }

    /// Recomputes the links of the tile at `coord` and of its four
    /// neighbours (their portals face it).
    fn relink(&mut self, coord: TileCoord) {
        self.revision += 1;
        let around = [
            coord,
            coord.neighbour(0),
            coord.neighbour(1),
            coord.neighbour(2),
            coord.neighbour(3),
        ];
        self.links.retain(|r, _| !around.contains(&r.tile));
        for c in around {
            let Some(tile) = self.tiles.get(&c) else {
                continue;
            };
            for (i, p) in tile.polys.iter().enumerate() {
                let from = PolyRef {
                    tile: c,
                    index: i as u16,
                };
                let mut out = Vec::new();
                let n = p.verts.len();
                for j in 0..n {
                    let a = tile.vertices[p.verts[j] as usize];
                    let b = tile.vertices[p.verts[(j + 1) % n] as usize];
                    match p.edges[j] {
                        NavEdge::Wall => {}
                        NavEdge::Inner(k) => out.push(Link {
                            to: PolyRef { tile: c, index: k },
                            a,
                            b,
                        }),
                        NavEdge::Portal(dir) => {
                            self.portal_links(c, dir, a, b, &mut out);
                        }
                    }
                }
                if !out.is_empty() {
                    self.links.insert(from, out);
                }
            }
        }
    }

    /// Links across tile `c`'s edge `dir` for the edge `a`–`b`: the
    /// neighbour's polygons whose facing portal overlaps it at a similar
    /// height.
    fn portal_links(&self, c: TileCoord, dir: u8, a: Vec3, b: Vec3, out: &mut Vec<Link>) {
        let other = c.neighbour(dir);
        let Some(t) = self.tiles.get(&other) else {
            return;
        };
        let facing = (dir + 2) & 3;
        // Along z for the x sides, along x for the z sides.
        let along = |v: Vec3| if dir & 1 == 0 { v.z } else { v.x };
        let (a0, a1) = (along(a).min(along(b)), along(a).max(along(b)));
        let tolerance = self.config.step_height + self.config.cell_size;
        for (i, p) in t.polys.iter().enumerate() {
            let n = p.verts.len();
            for j in 0..n {
                if p.edges[j] != NavEdge::Portal(facing) {
                    continue;
                }
                let c0 = t.vertices[p.verts[j] as usize];
                let c1 = t.vertices[p.verts[(j + 1) % n] as usize];
                let (b0, b1) = (along(c0).min(along(c1)), along(c0).max(along(c1)));
                let lo = a0.max(b0);
                let hi = a1.min(b1);
                if hi - lo < 0.01 {
                    continue;
                }
                let at = |s: f32| point_on(a, b, along, s);
                let other_at = |s: f32| point_on(c0, c1, along, s);
                if (at(lo).y - other_at(lo).y).abs() > tolerance
                    || (at(hi).y - other_at(hi).y).abs() > tolerance
                {
                    continue;
                }
                out.push(Link {
                    to: PolyRef {
                        tile: other,
                        index: i as u16,
                    },
                    a: at(lo),
                    b: at(hi),
                });
            }
        }
    }
}

/// The point of segment `a`–`b` whose `along` coordinate is `s`.
fn point_on(a: Vec3, b: Vec3, along: impl Fn(Vec3) -> f32, s: f32) -> Vec3 {
    let (sa, sb) = (along(a), along(b));
    if (sb - sa).abs() < 1e-6 {
        return a;
    }
    a.lerp(b, ((s - sa) / (sb - sa)).clamp(0.0, 1.0))
}

/// The ground-plane part of a point.
pub(crate) fn xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}
