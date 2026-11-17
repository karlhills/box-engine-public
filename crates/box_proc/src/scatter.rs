//! Scatter (M6): instances of meshes or prefabs placed over an area or a
//! painted mask, deterministic per (seed, chunk, rule parameters, ground).
//!
//! **Saved intent.** [`Scatter`] is a reflected component: an owner-local
//! area polygon on the XZ plane and/or a painted [`ScatterMask`], a species
//! list ([`ScatterSpecies`]: a mesh or prefab asset, weight, scale range,
//! random yaw, align-to-normal, tint jitter), density, minimum distance,
//! slope and height limits, the ground entity and a seed. Nothing else is
//! saved: instances are a cache, rebuilt by the engine glue
//! (`box_engine::scatter_glue`).
//!
//! **Chunks.** The owner's XZ plane is cut into square chunks of 16 × 16
//! mask cells ([`ScatterMask::cell_size`], 0.5 m by default, so 8 m
//! chunks). [`ScatterPlan::place`] places one chunk as a pure function of
//! the parameters, the chunk's mask tile (or how the area covers it) and the
//! ground triangles over it; [`ScatterPlan::chunk_key`] hashes exactly those
//! inputs, so a paint stroke, an area edit or a ground change regenerates
//! only the chunks whose inputs changed.
//!
//! **Placement.** A global grid of candidate cells, one candidate each, at
//! a seeded random position (the seed, the cell coordinates and nothing
//! else), thinned at random to reach the density, then made to respect the
//! minimum distance by priority (a candidate survives when no surviving
//! candidate within the minimum distance has a higher random priority; a
//! Matérn type II process). Both steps look only at candidates, never at
//! the mask or the ground, so the decision for a point is the same whichever
//! chunk computes it, and the minimum distance holds across chunk borders.
//! Survivors are then filtered by the mask weight (a seeded per-candidate
//! draw against the weight, so painting more only ever adds instances), the
//! ground (a point with no ground under it is dropped), the slope and the
//! height limits.
//!
//! **Mask.** A sparse grid of `u8` weights, stored per chunk as a
//! [`MaskTile`] (256 cells as 512 hex digits, one string per painted chunk).
//! A tile overrides the area inside its chunk; chunks without a tile use the
//! area polygon. Painting creates a tile initialised from the area, so
//! erasing inside an area and adding outside it both work, and a tile that
//! returns to what the area gives is dropped. The grid is bounded by the
//! painted region, not by the number of strokes, and a stroke touches only
//! the tiles under it.
//!
//! **Ground.** [`ScatterSurface`]: the owner's XZ plane (y = 0), or the
//! triangles of a mesh in owner space, bucketed for sampling and hashed per
//! chunk. The engine glue builds it from the `Scatter::surface` entity's
//! mesh when that mesh or the relative transform changes.

use box_core::pmath;
use std::collections::HashMap;
use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use bevy_reflect::Reflect;
use box_core::asset_ref::AssetRef;
use box_core::identity::Uid;
use box_core::mesh::MeshData;
use box_core::reflect::{Hidden, Range, ReflectComponent, ReflectDefault, Step, Unit};
use box_core::transform::Transform;
use glam::{Affine3A, Quat, Vec2, Vec3};

use crate::hash::{StableHasher, reflect_hash};
use crate::rng::Rng;

/// Version of the placement algorithm, part of every chunk key. Bump it
/// whenever placement changes for the same parameters.
pub const SCATTER_VERSION: u32 = 1;
/// Mask cells along one side of a chunk (and of a [`MaskTile`]).
pub const TILE_CELLS: usize = 16;
/// Cells in one tile.
pub const TILE_LEN: usize = TILE_CELLS * TILE_CELLS;
/// Brightness variants per species when `tint_jitter` is above zero (each
/// is one material, so instances still batch).
pub const TINT_VARIANTS: u8 = 4;
/// Most instances one chunk places; the density is lowered (with a
/// warning) to stay under it.
pub const MAX_PER_CHUNK: f32 = 8192.0;
/// Most chunks one scatter covers; an area larger than this is cut (with a
/// warning).
pub const MAX_CHUNKS: usize = 65_536;

/// A chunk's coordinates: chunk `(x, z)` covers `[x·s, (x+1)·s) × [z·s, (z+1)·s)`
/// of the owner's XZ plane, `s` = [`ScatterMask::chunk_size`].
pub type ChunkCoord = (i32, i32);

/// Where a species' instances come from.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum SpeciesSource {
    /// A mesh asset (such as `models/bush.glb#Mesh/Bush`) with material
    /// assets by slot; an empty slot draws the default material.
    Mesh {
        mesh: AssetRef,
        materials: Vec<AssetRef>,
    },
    /// A prefab or model scene (such as `models/tree.glb#Scene`): its
    /// `MeshAsset` entities, flattened into one instance each.
    Prefab { scene: AssetRef },
}

impl Default for SpeciesSource {
    fn default() -> Self {
        Self::Mesh {
            mesh: AssetRef::NONE,
            materials: Vec::new(),
        }
    }
}

/// One kind of thing a [`Scatter`] places.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ScatterSpecies {
    /// The mesh or prefab to place.
    pub source: SpeciesSource,
    /// How often this species is chosen, relative to the others.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Step(0.1))]
    pub weight: f32,
    /// Smallest and largest uniform scale.
    #[reflect(@Range { min: 0.01, max: 100.0 }, @Step(0.01))]
    pub scale: Vec2,
    /// Turn each instance by a random angle about its up axis.
    pub random_yaw: bool,
    /// 0: instances stand upright; 1: they lean with the ground.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub align_to_normal: f32,
    /// Random brightness variation, up to this fraction lighter or darker
    /// (standard materials only; four variants, so instances still batch).
    #[reflect(@Range { min: 0.0, max: 0.5 }, @Step(0.01))]
    pub tint_jitter: f32,
    /// Whether the instances cast shadows.
    pub cast_shadows: bool,
}

impl Default for ScatterSpecies {
    fn default() -> Self {
        Self {
            source: SpeciesSource::default(),
            weight: 1.0,
            scale: Vec2::new(0.8, 1.2),
            random_yaw: true,
            align_to_normal: 0.0,
            tint_jitter: 0.0,
            cast_shadows: true,
        }
    }
}

impl ScatterSpecies {
    /// A species drawing the mesh asset `mesh` with `materials`.
    pub fn mesh(mesh: AssetRef, materials: Vec<AssetRef>) -> Self {
        Self {
            source: SpeciesSource::Mesh { mesh, materials },
            ..Self::default()
        }
    }

    /// A species instancing the meshes of the scene asset `scene`.
    pub fn prefab(scene: AssetRef) -> Self {
        Self {
            source: SpeciesSource::Prefab { scene },
            ..Self::default()
        }
    }
}

/// The painted weights of a [`Scatter`]: a sparse grid of `u8` cells,
/// stored per chunk (see the module docs).
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ScatterMask {
    /// Edge of one mask cell; a chunk is 16 × 16 cells. Changing it
    /// reinterprets the painted cells.
    #[reflect(@Range { min: 0.05, max: 100.0 }, @Unit("m"))]
    pub cell_size: f32,
    /// Painted chunks, sorted by (z, x).
    pub tiles: Vec<MaskTile>,
}

impl Default for ScatterMask {
    fn default() -> Self {
        Self {
            cell_size: DEFAULT_CELL,
            tiles: Vec::new(),
        }
    }
}

/// The painted cells of one chunk: 256 weights (row by row along +X, rows
/// along +Z) as 512 hex digits.
#[derive(Clone, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct MaskTile {
    pub x: i32,
    pub z: i32,
    pub cells: String,
}

const DEFAULT_CELL: f32 = 0.5;

/// Decoded tile cells.
pub type TileCells = [u8; TILE_LEN];

impl MaskTile {
    /// The cells, or `None` when `cells` is not 512 hex digits.
    pub fn decode(&self) -> Option<TileCells> {
        let b = self.cells.as_bytes();
        if b.len() != TILE_LEN * 2 {
            return None;
        }
        let mut out = [0u8; TILE_LEN];
        for (i, o) in out.iter_mut().enumerate() {
            *o = (hex(b[2 * i])? << 4) | hex(b[2 * i + 1])?;
        }
        Some(out)
    }

    /// A tile at `(x, z)` holding `cells`.
    pub fn encode(x: i32, z: i32, cells: &TileCells) -> Self {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(TILE_LEN * 2);
        for &c in cells {
            s.push(DIGITS[(c >> 4) as usize] as char);
            s.push(DIGITS[(c & 15) as usize] as char);
        }
        Self { x, z, cells: s }
    }
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl ScatterMask {
    /// The cell size actually used (the default when `cell_size` is not a
    /// usable number).
    pub fn cell(&self) -> f32 {
        if self.cell_size.is_finite() && (0.05..=100.0).contains(&self.cell_size) {
            self.cell_size
        } else {
            DEFAULT_CELL
        }
    }

    /// Edge of one chunk: 16 cells.
    pub fn chunk_size(&self) -> f32 {
        self.cell() * TILE_CELLS as f32
    }

    /// The chunk holding the owner-local XZ point `p`.
    pub fn chunk_of(&self, p: Vec2) -> ChunkCoord {
        let s = self.chunk_size();
        ((p.x / s).floor() as i32, (p.y / s).floor() as i32)
    }

    /// The tile of chunk `c`, if painted.
    pub fn tile(&self, c: ChunkCoord) -> Option<&MaskTile> {
        self.tiles
            .binary_search_by(|t| (t.z, t.x).cmp(&(c.1, c.0)))
            .ok()
            .map(|i| &self.tiles[i])
    }

    /// One brush dab at the owner-local XZ point `center`: cells whose
    /// centre is within `radius` move towards full weight (or towards zero
    /// with `erase`) by `strength` times a smooth falloff. Chunks without a
    /// tile start from what `area` gives. Returns the chunks whose tile
    /// changed (added, edited or dropped), sorted.
    pub fn paint(
        &mut self,
        area: &[Vec2],
        center: Vec2,
        radius: f32,
        strength: f32,
        erase: bool,
    ) -> Vec<ChunkCoord> {
        let strength = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if !(radius.is_finite() && radius > 0.0 && center.is_finite()) || strength == 0.0 {
            return Vec::new();
        }
        let cell = self.cell();
        let s = self.chunk_size();
        let lo = self.chunk_of(center - Vec2::splat(radius));
        let hi = self.chunk_of(center + Vec2::splat(radius));
        let target = if erase { 0.0 } else { 255.0 };
        let mut touched = Vec::new();
        for cz in lo.1..=hi.1 {
            for cx in lo.0..=hi.0 {
                let origin = Vec2::new(cx as f32 * s, cz as f32 * s);
                let area_cells = area_tile(area, origin, cell);
                let mut cells = match self.tile((cx, cz)).and_then(MaskTile::decode) {
                    Some(c) => c,
                    None => area_cells,
                };
                let before = cells;
                for (i, v) in cells.iter_mut().enumerate() {
                    let p = origin
                        + Vec2::new(
                            ((i % TILE_CELLS) as f32 + 0.5) * cell,
                            ((i / TILE_CELLS) as f32 + 0.5) * cell,
                        );
                    let d = p.distance(center) / radius;
                    if d >= 1.0 {
                        continue;
                    }
                    let t = 1.0 - d;
                    let amount = strength * t * t * (3.0 - 2.0 * t);
                    let cur = f32::from(*v);
                    let mut next = cur + (target - cur) * amount;
                    // Always make progress, so repeated dabs saturate.
                    if amount > 0.0 && (next - cur).abs() < 1.0 && cur != target {
                        next = cur + (target - cur).signum();
                    }
                    *v = next.round().clamp(0.0, 255.0) as u8;
                }
                if cells == before && self.tile((cx, cz)).is_some() {
                    continue;
                }
                let changed = self.set_tile((cx, cz), &cells, &area_cells);
                if changed {
                    touched.push((cx, cz));
                }
            }
        }
        touched.sort_unstable_by_key(|c| (c.1, c.0));
        touched
    }

    /// Stores `cells` as chunk `c`'s tile, or drops the tile when the cells
    /// equal what the area gives there. True if the mask changed.
    fn set_tile(&mut self, c: ChunkCoord, cells: &TileCells, area_cells: &TileCells) -> bool {
        let at = self.tiles.binary_search_by(|t| (t.z, t.x).cmp(&(c.1, c.0)));
        if cells == area_cells {
            return match at {
                Ok(i) => {
                    self.tiles.remove(i);
                    true
                }
                Err(_) => false,
            };
        }
        let tile = MaskTile::encode(c.0, c.1, cells);
        match at {
            Ok(i) if self.tiles[i] == tile => false,
            Ok(i) => {
                self.tiles[i] = tile;
                true
            }
            Err(i) => {
                self.tiles.insert(i, tile);
                true
            }
        }
    }
}

/// The cells of the chunk at `origin` as the area gives them (255 where the
/// cell centre is inside the polygon).
fn area_tile(area: &[Vec2], origin: Vec2, cell: f32) -> TileCells {
    let mut out = [0u8; TILE_LEN];
    if area.len() < 3 {
        return out;
    }
    for (i, v) in out.iter_mut().enumerate() {
        let p = origin
            + Vec2::new(
                ((i % TILE_CELLS) as f32 + 0.5) * cell,
                ((i / TILE_CELLS) as f32 + 0.5) * cell,
            );
        if inside(area, p) {
            *v = 255;
        }
    }
    out
}

/// Even-odd point in polygon.
fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) {
            let x = (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x;
            if p.x < x {
                c = !c;
            }
        }
        j = i;
    }
    c
}

/// Places instances of meshes or prefabs (species) over an owner-local area
/// polygon and painted regions, on the ground entity or the owner's XZ
/// plane: deterministic per seed and chunk, and regenerated per chunk when
/// its inputs change. The instances are runtime only.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Scatter {
    /// What to place, chosen by weight.
    pub species: Vec<ScatterSpecies>,
    /// Instances per square metre where the area or the paint is full
    /// (approximate near the most the minimum distance allows).
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("/m²"), @Step(0.01))]
    pub density: f32,
    /// No two instances closer than this.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("m"), @Step(0.01))]
    pub min_distance: f32,
    /// Steepest ground an instance stands on (1.571 = no limit).
    #[reflect(@Range { min: 0.0, max: 1.571 }, @Unit("rad"), @Step(0.01))]
    pub max_slope: f32,
    /// Lowest ground an instance stands on (owner-local Y).
    #[reflect(@Unit("m"))]
    pub min_height: f32,
    /// Highest ground an instance stands on (owner-local Y).
    #[reflect(@Unit("m"))]
    pub max_height: f32,
    /// Owner-local outline on the XZ plane (x, z). Empty: only painted
    /// regions are covered.
    pub area: Vec<Vec2>,
    /// The entity whose mesh the instances stand on. None: the owner's XZ
    /// plane.
    pub surface: Option<Uid>,
    /// Change it for another arrangement with the same settings.
    pub seed: u64,
    /// Painted regions (the Paint Scatter tool writes them).
    #[reflect(@Hidden)]
    pub mask: ScatterMask,
}

impl Default for Scatter {
    fn default() -> Self {
        Self {
            species: Vec::new(),
            density: 2.0,
            min_distance: 0.4,
            max_slope: 0.7,
            min_height: -1000.0,
            max_height: 1000.0,
            area: Vec::new(),
            surface: None,
            seed: 0,
            mask: ScatterMask::default(),
        }
    }
}

/// One placed instance, in the owner's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterInstance {
    /// Stable id: the candidate cell it came from (unique per scatter,
    /// unchanged by edits that keep the instance).
    pub id: u64,
    /// Index into [`Scatter::species`].
    pub species: u32,
    /// Brightness variant, `0..TINT_VARIANTS`.
    pub tint: u8,
    pub transform: Transform,
}

/// The brightness factor of tint variant `variant` for a species with
/// `jitter`.
pub fn tint_factor(jitter: f32, variant: u8) -> f32 {
    let t = (f32::from(variant) + 0.5) / f32::from(TINT_VARIANTS) * 2.0 - 1.0;
    1.0 + jitter.clamp(0.0, 0.5) * t
}

/// What the ground is at a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceHit {
    pub height: f32,
    /// Unit, facing up (owner-local).
    pub normal: Vec3,
}

/// The ground a scatter stands on, in the owner's space (see the module
/// docs).
#[derive(Clone, Debug, Default)]
pub enum ScatterSurface {
    /// The owner's XZ plane, y = 0.
    #[default]
    Flat,
    Mesh(MeshSurface),
}

/// Ground triangles in the owner's space, bucketed for sampling.
#[derive(Clone, Debug)]
pub struct MeshSurface {
    tris: Vec<SurfaceTri>,
    grid: HashMap<ChunkCoord, Vec<u32>>,
    grid_cell: f32,
    chunk_size: f32,
    chunk_hashes: HashMap<ChunkCoord, u64>,
}

#[derive(Clone, Copy, Debug)]
struct SurfaceTri {
    p: [Vec3; 3],
    n: [Vec3; 3],
}

const FLAT_HASH: u64 = 0x666c_6174_2067_726e;
const EMPTY_HASH: u64 = 0x6e6f_2067_726f_756e;

impl ScatterSurface {
    /// The upward-facing triangles of `mesh`, moved into the owner's space
    /// by `to_owner`, bucketed for chunks of `chunk_size`. Downward-facing
    /// and vertical triangles are not ground.
    pub fn from_mesh(mesh: &MeshData, to_owner: Affine3A, chunk_size: f32) -> Self {
        let normal_matrix = to_owner.matrix3.inverse().transpose();
        let has_normals = mesh.normals.len() == mesh.positions.len();
        let grid_cell = chunk_size / 4.0;
        let mut tris = Vec::new();
        let mut grid: HashMap<ChunkCoord, Vec<u32>> = HashMap::new();
        let mut hashers: HashMap<ChunkCoord, StableHasher> = HashMap::new();
        let n = mesh.positions.len();
        for t in mesh.indices.as_chunks::<3>().0 {
            let idx = t.map(|i| i as usize);
            if idx.iter().any(|&i| i >= n) {
                continue;
            }
            let p = [0, 1, 2].map(|k| to_owner.transform_point3(mesh.positions[idx[k]]));
            if !p.iter().all(|v| v.is_finite()) {
                continue;
            }
            let mut ng = (p[1] - p[0]).cross(p[2] - p[0]);
            let n = if has_normals {
                let n = [0, 1, 2]
                    .map(|k| (normal_matrix * mesh.normals[idx[k]]).normalize_or(Vec3::ZERO));
                if ng.dot(n[0] + n[1] + n[2]) < 0.0 {
                    ng = -ng;
                }
                n
            } else {
                [Vec3::ZERO; 3]
            };
            let ng = ng.normalize_or(Vec3::ZERO);
            if ng.y <= 1e-4 {
                continue;
            }
            let n = n.map(|v| if v.y > 0.0 { v } else { ng });
            let index = tris.len() as u32;
            tris.push(SurfaceTri { p, n });
            let (lo, hi) = xz_bounds(&p);
            for (cell, out) in [(grid_cell, true), (chunk_size, false)] {
                let (x0, z0) = ((lo.x / cell).floor() as i32, (lo.y / cell).floor() as i32);
                let (x1, z1) = ((hi.x / cell).floor() as i32, (hi.y / cell).floor() as i32);
                // A sliver spanning a huge range would explode the grid.
                if (x1 - x0) as i64 * (z1 - z0) as i64 > 1 << 20 {
                    continue;
                }
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        if out {
                            grid.entry((x, z)).or_default().push(index);
                        } else {
                            let h = hashers.entry((x, z)).or_default();
                            for v in p {
                                h.write_u32(v.x.to_bits());
                                h.write_u32(v.y.to_bits());
                                h.write_u32(v.z.to_bits());
                            }
                            for v in n {
                                h.write_u32(v.x.to_bits());
                                h.write_u32(v.y.to_bits());
                                h.write_u32(v.z.to_bits());
                            }
                        }
                    }
                }
            }
        }
        Self::Mesh(MeshSurface {
            tris,
            grid,
            grid_cell,
            chunk_size,
            chunk_hashes: hashers.into_iter().map(|(k, h)| (k, h.finish())).collect(),
        })
    }

    /// The chunk size this surface was bucketed for (`None` for the plane).
    pub fn chunk_size(&self) -> Option<f32> {
        match self {
            Self::Flat => None,
            Self::Mesh(m) => Some(m.chunk_size),
        }
    }

    /// A hash of the ground over chunk `c` (its triangles).
    pub fn chunk_hash(&self, c: ChunkCoord) -> u64 {
        match self {
            Self::Flat => FLAT_HASH,
            Self::Mesh(m) => m.chunk_hashes.get(&c).copied().unwrap_or(EMPTY_HASH),
        }
    }

    /// Ground triangles (0 for the plane).
    pub fn triangle_count(&self) -> usize {
        match self {
            Self::Flat => 0,
            Self::Mesh(m) => m.tris.len(),
        }
    }

    /// The highest ground at the owner-local XZ point `p`.
    pub fn sample(&self, p: Vec2) -> Option<SurfaceHit> {
        let m = match self {
            Self::Flat => {
                return Some(SurfaceHit {
                    height: 0.0,
                    normal: Vec3::Y,
                });
            }
            Self::Mesh(m) => m,
        };
        let cell = (
            (p.x / m.grid_cell).floor() as i32,
            (p.y / m.grid_cell).floor() as i32,
        );
        let mut best: Option<SurfaceHit> = None;
        for &i in m.grid.get(&cell)? {
            let t = &m.tris[i as usize];
            let Some(w) = barycentric_xz(&t.p, p) else {
                continue;
            };
            let height = t.p[0].y * w.x + t.p[1].y * w.y + t.p[2].y * w.z;
            if best.is_some_and(|b| b.height >= height) {
                continue;
            }
            let normal = (t.n[0] * w.x + t.n[1] * w.y + t.n[2] * w.z).normalize_or(Vec3::Y);
            best = Some(SurfaceHit { height, normal });
        }
        best
    }

    /// The nearest hit of the owner-space ray `origin + t·dir` (t ≥ 0) with
    /// the ground: (t, normal).
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3)> {
        match self {
            Self::Flat => {
                if dir.y.abs() < 1e-6 {
                    return None;
                }
                let t = -origin.y / dir.y;
                (t >= 0.0).then_some((t, Vec3::Y))
            }
            Self::Mesh(m) => {
                let mut best: Option<(f32, Vec3)> = None;
                for tri in &m.tris {
                    if let Some(t) = ray_triangle(origin, dir, &tri.p)
                        && best.is_none_or(|(b, _)| t < b)
                    {
                        let hit = origin + dir * t;
                        let n = barycentric_xz(&tri.p, Vec2::new(hit.x, hit.z))
                            .map_or((tri.p[1] - tri.p[0]).cross(tri.p[2] - tri.p[0]), |w| {
                                tri.n[0] * w.x + tri.n[1] * w.y + tri.n[2] * w.z
                            });
                        best = Some((t, n.normalize_or(Vec3::Y)));
                    }
                }
                best
            }
        }
    }
}

fn xz_bounds(p: &[Vec3; 3]) -> (Vec2, Vec2) {
    let lo = Vec2::new(
        p[0].x.min(p[1].x).min(p[2].x),
        p[0].z.min(p[1].z).min(p[2].z),
    );
    let hi = Vec2::new(
        p[0].x.max(p[1].x).max(p[2].x),
        p[0].z.max(p[1].z).max(p[2].z),
    );
    (lo, hi)
}

/// Barycentric weights of `p` in the triangle's XZ projection, if inside
/// (with a little tolerance, so shared edges have no gaps).
fn barycentric_xz(t: &[Vec3; 3], p: Vec2) -> Option<Vec3> {
    let a = Vec2::new(t[0].x, t[0].z);
    let b = Vec2::new(t[1].x, t[1].z);
    let c = Vec2::new(t[2].x, t[2].z);
    let d = (b - a).perp_dot(c - a);
    if d.abs() < 1e-12 {
        return None;
    }
    let w1 = (p - a).perp_dot(c - a) / d;
    let w2 = (b - a).perp_dot(p - a) / d;
    let w0 = 1.0 - w1 - w2;
    const EPS: f32 = -1e-5;
    (w0 >= EPS && w1 >= EPS && w2 >= EPS).then_some(Vec3::new(w0, w1, w2))
}

/// Möller–Trumbore, both sides.
fn ray_triangle(o: Vec3, d: Vec3, p: &[Vec3; 3]) -> Option<f32> {
    let e1 = p[1] - p[0];
    let e2 = p[2] - p[0];
    let h = d.cross(e2);
    let a = e1.dot(h);
    if a.abs() < 1e-12 {
        return None;
    }
    let f = 1.0 / a;
    let s = o - p[0];
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = f * d.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * e2.dot(q);
    (t >= 0.0).then_some(t)
}

/// How a chunk's mask weight is decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// Its painted tile.
    Tile,
    /// Wholly inside the area.
    Inside,
    /// Crossed by the area's outline.
    Partial,
}

/// Validated species parameters.
#[derive(Clone, Copy, Debug)]
struct SpeciesParams {
    scale: (f32, f32),
    random_yaw: bool,
    align: f32,
    tinted: bool,
}

/// Everything [`ScatterPlan::place`] needs, computed once per parameter
/// change: validated parameters, the candidate grid, the chunks to fill and
/// warnings.
#[derive(Clone, Debug)]
pub struct ScatterPlan {
    chunk_size: f32,
    mask_cell: f32,
    /// Candidate cell edge.
    cell: f32,
    /// Neighbour cells the minimum-distance test looks at.
    reach: i32,
    r2: f32,
    /// Probability a candidate survives the density thinning.
    p0: f32,
    seed: u64,
    params: u64,
    area: Vec<Vec2>,
    area_hash: u64,
    chunks: Vec<(ChunkCoord, Source)>,
    tiles: HashMap<ChunkCoord, TileCells>,
    tile_hashes: HashMap<ChunkCoord, u64>,
    cdf: Vec<f32>,
    species: Vec<SpeciesParams>,
    cos_slope: f32,
    heights: (f32, f32),
    warnings: Vec<String>,
}

impl ScatterPlan {
    /// Validates `scatter` and lists the chunks it covers. Bad values are
    /// replaced (with a warning), never a panic.
    pub fn new(scatter: &Scatter) -> Self {
        let mut warnings = Vec::new();
        let mask_cell = scatter.mask.cell();
        if mask_cell != scatter.mask.cell_size {
            warnings.push(format!(
                "mask cell size {} is not usable; using {mask_cell} m",
                scatter.mask.cell_size
            ));
        }
        let chunk_size = mask_cell * TILE_CELLS as f32;

        // Species: weights and validated parameters.
        let mut cdf = Vec::with_capacity(scatter.species.len());
        let mut total = 0.0f32;
        let mut species = Vec::with_capacity(scatter.species.len());
        for (i, s) in scatter.species.iter().enumerate() {
            let w = if s.weight.is_finite() && s.weight > 0.0 {
                s.weight
            } else {
                0.0
            };
            total += w;
            cdf.push(total);
            let (a, b) = (s.scale.x, s.scale.y);
            let scale = if a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 {
                (a.min(b), a.max(b))
            } else {
                warnings.push(format!(
                    "species {i}: scale must be two numbers above 0; using 1"
                ));
                (1.0, 1.0)
            };
            let align = if s.align_to_normal.is_finite() {
                s.align_to_normal.clamp(0.0, 1.0)
            } else {
                0.0
            };
            species.push(SpeciesParams {
                scale,
                random_yaw: s.random_yaw,
                align,
                tinted: s.tint_jitter.is_finite() && s.tint_jitter > 0.0,
            });
        }
        if scatter.species.is_empty() {
            warnings.push("no species: add one to the species list".into());
        } else if total <= 0.0 {
            warnings.push("every species has weight 0".into());
        }

        // Density and minimum distance.
        let mut density = if scatter.density.is_finite() {
            scatter.density.max(0.0)
        } else {
            warnings.push("density is not a number; using 0".into());
            0.0
        };
        let cap = MAX_PER_CHUNK / (chunk_size * chunk_size);
        if density > cap {
            warnings.push(format!(
                "density {density} per m² is more than {MAX_PER_CHUNK} per chunk; using {cap:.2}"
            ));
            density = cap;
        }
        let r = if scatter.min_distance.is_finite() {
            scatter.min_distance.max(0.0)
        } else {
            warnings.push("min distance is not a number; using 0".into());
            0.0
        };
        let (mut cell, mut p0, mut reach) = (1.0, 0.0, 0);
        if density > 0.0 {
            let base = 0.7 / density.sqrt();
            let disc = PI * r * r;
            // Wanted instances per disc of the minimum distance.
            let packed = density * disc;
            if r > 0.0 && packed >= 0.02 {
                // One candidate per cell of r/√2: the thinning that leaves
                // `packed` after the distance test, from the measured curve.
                cell = r * FRAC_1_SQRT_2;
                let most = KEPT.last().map_or(1.0, |k| k.1);
                let y = if packed >= most * 0.995 {
                    warnings.push(format!(
                        "density {density} per m² is more than a min distance of {r} m allows \
                         (at most about {:.2} per m²)",
                        most / disc
                    ));
                    TAU
                } else {
                    thinning_for(packed)
                };
                // Candidates per disc: 2π at one per cell.
                p0 = (y / TAU).min(1.0);
                reach = (r / cell).ceil() as i32;
            } else {
                // The distance rarely matters: a jittered grid, thinned.
                cell = base;
                p0 = density * cell * cell;
                reach = if r > 0.0 { (r / cell).ceil() as i32 } else { 0 };
            }
        }

        // Limits.
        let max_slope = if scatter.max_slope.is_finite() {
            scatter.max_slope.clamp(0.0, PI / 2.0)
        } else {
            PI / 2.0
        };
        let cos_slope = if max_slope >= PI / 2.0 - 1e-4 {
            -1.0
        } else {
            pmath::cos(max_slope)
        };
        let lo = if scatter.min_height.is_finite() {
            scatter.min_height
        } else {
            f32::NEG_INFINITY
        };
        let hi = if scatter.max_height.is_finite() {
            scatter.max_height
        } else {
            f32::INFINITY
        };
        if lo > hi {
            warnings.push("min height is above max height: nothing is placed".into());
        }

        // Parameters every chunk depends on.
        let params = {
            let mut s = scatter.clone();
            s.area = Vec::new();
            s.mask.tiles = Vec::new();
            s.surface = None;
            let mut h = StableHasher::new();
            h.write_u32(SCATTER_VERSION);
            h.write_u64(reflect_hash(&s));
            h.finish()
        };

        // The area.
        let area: Vec<Vec2> = if scatter.area.len() >= 3
            && scatter.area.iter().all(|p| p.is_finite())
        {
            scatter.area.clone()
        } else {
            if !scatter.area.is_empty() {
                warnings.push(
                    "the area needs at least 3 finite points; only painted regions are used".into(),
                );
            }
            Vec::new()
        };
        let area_hash = {
            let mut h = StableHasher::new();
            for p in &area {
                h.write_u32(p.x.to_bits());
                h.write_u32(p.y.to_bits());
            }
            h.finish()
        };

        // Chunks: painted tiles, then chunks the area covers.
        let mut tiles = HashMap::new();
        let mut tile_hashes = HashMap::new();
        let mut chunks: Vec<(ChunkCoord, Source)> = Vec::new();
        let mut bad_tiles = 0;
        for t in &scatter.mask.tiles {
            let c = (t.x, t.z);
            if tiles.contains_key(&c) {
                continue;
            }
            let cells = match t.decode() {
                Some(c) => c,
                None => {
                    bad_tiles += 1;
                    [0u8; TILE_LEN]
                }
            };
            let mut h = StableHasher::new();
            h.write(&cells);
            tile_hashes.insert(c, h.finish());
            if cells.iter().any(|&v| v > 0) {
                chunks.push((c, Source::Tile));
            }
            tiles.insert(c, cells);
        }
        if bad_tiles > 0 {
            warnings.push(format!(
                "{bad_tiles} painted tile(s) are damaged (not 512 hex digits) and count as unpainted"
            ));
        }
        if !area.is_empty() {
            let (lo, hi) = area.iter().fold(
                (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
                |(l, h), p| (l.min(*p), h.max(*p)),
            );
            let (x0, z0) = (
                (lo.x / chunk_size).floor() as i64,
                (lo.y / chunk_size).floor() as i64,
            );
            let (x1, z1) = (
                (hi.x / chunk_size).floor() as i64,
                (hi.y / chunk_size).floor() as i64,
            );
            let count = (x1 - x0 + 1) * (z1 - z0 + 1);
            if count > MAX_CHUNKS as i64 {
                warnings.push(format!(
                    "the area covers {count} chunks; only the first {MAX_CHUNKS} are filled"
                ));
            }
            let mut n = 0usize;
            'outer: for z in z0..=z1 {
                for x in x0..=x1 {
                    if n >= MAX_CHUNKS {
                        break 'outer;
                    }
                    n += 1;
                    let c = (x as i32, z as i32);
                    if tiles.contains_key(&c) {
                        continue;
                    }
                    let origin = Vec2::new(c.0 as f32 * chunk_size, c.1 as f32 * chunk_size);
                    match classify(&area, origin, chunk_size) {
                        Some(true) => chunks.push((c, Source::Inside)),
                        None => chunks.push((c, Source::Partial)),
                        Some(false) => {}
                    }
                }
            }
        }
        if total <= 0.0 || density <= 0.0 || lo > hi {
            chunks.clear();
        }
        chunks.sort_unstable_by_key(|(c, _)| (c.1, c.0));

        Self {
            chunk_size,
            mask_cell,
            cell,
            reach,
            r2: r * r,
            p0,
            seed: scatter.seed,
            params,
            area,
            area_hash,
            chunks,
            tiles,
            tile_hashes,
            cdf,
            species,
            cos_slope,
            heights: (lo, hi),
            warnings,
        }
    }

    /// Edge of one chunk.
    pub fn chunk_size(&self) -> f32 {
        self.chunk_size
    }

    /// The chunks to fill, sorted by (z, x).
    pub fn chunks(&self) -> impl Iterator<Item = ChunkCoord> + '_ {
        self.chunks.iter().map(|(c, _)| *c)
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Problems with the parameters, for the owner's warnings.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn source(&self, c: ChunkCoord) -> Option<Source> {
        self.chunks
            .binary_search_by(|(k, _)| (k.1, k.0).cmp(&(c.1, c.0)))
            .ok()
            .map(|i| self.chunks[i].1)
    }

    /// A hash of everything chunk `c`'s instances depend on: the
    /// parameters, the seed, the chunk, its tile (or how the area covers
    /// it) and the ground over it. `None` when the chunk is not filled.
    pub fn chunk_key(&self, c: ChunkCoord, surface: &ScatterSurface) -> Option<u64> {
        let source = self.source(c)?;
        let mut h = StableHasher::new();
        h.write_u64(self.params);
        h.write_i64(i64::from(c.0));
        h.write_i64(i64::from(c.1));
        match source {
            Source::Tile => {
                h.write_str("tile");
                h.write_u64(self.tile_hashes.get(&c).copied().unwrap_or(0));
            }
            Source::Inside => h.write_str("inside"),
            Source::Partial => {
                h.write_str("partial");
                h.write_u64(self.area_hash);
            }
        }
        h.write_u64(surface.chunk_hash(c));
        Some(h.finish())
    }

    /// The instances of chunk `c` (none when it is not filled), in id
    /// order. Pure: the same plan, chunk and ground give the same result.
    pub fn place(&self, c: ChunkCoord, surface: &ScatterSurface) -> Vec<ScatterInstance> {
        let Some(source) = self.source(c) else {
            return Vec::new();
        };
        let s = self.chunk_size;
        let origin = Vec2::new(c.0 as f32 * s, c.1 as f32 * s);
        let cell = self.cell;
        let i0 = (origin.x / cell).floor() as i32;
        let i1 = ((origin.x + s) / cell).floor() as i32;
        let j0 = (origin.y / cell).floor() as i32;
        let j1 = ((origin.y + s) / cell).floor() as i32;
        let reach = self.reach;
        let w = (i1 - i0 + 1 + 2 * reach) as usize;
        let h = (j1 - j0 + 1 + 2 * reach) as usize;
        let mut grid: Vec<Candidate> = Vec::with_capacity(w * h);
        for j in (j0 - reach)..=(j1 + reach) {
            for i in (i0 - reach)..=(i1 + reach) {
                grid.push(self.candidate(i, j));
            }
        }
        let at = |i: i32, j: i32| &grid[(j - j0 + reach) as usize * w + (i - i0 + reach) as usize];
        let tile = self.tiles.get(&c);
        let mut out = Vec::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                let cand = at(i, j);
                if !cand.alive {
                    continue;
                }
                let p = cand.pos;
                if ((p.x / s).floor() as i32, (p.y / s).floor() as i32) != c {
                    continue;
                }
                if reach > 0 {
                    let mut beaten = false;
                    'n: for dj in -reach..=reach {
                        for di in -reach..=reach {
                            if di == 0 && dj == 0 {
                                continue;
                            }
                            let o = at(i + di, j + dj);
                            if o.alive
                                && o.pos.distance_squared(p) < self.r2
                                && (o.priority, (j + dj, i + di)) > (cand.priority, (j, i))
                            {
                                beaten = true;
                                break 'n;
                            }
                        }
                    }
                    if beaten {
                        continue;
                    }
                }
                let weight = match source {
                    Source::Tile => tile.map_or(0.0, |t| {
                        let cx = (((p.x - origin.x) / self.mask_cell) as usize).min(TILE_CELLS - 1);
                        let cz = (((p.y - origin.y) / self.mask_cell) as usize).min(TILE_CELLS - 1);
                        f32::from(t[cz * TILE_CELLS + cx]) / 255.0
                    }),
                    Source::Inside => 1.0,
                    Source::Partial => {
                        if inside(&self.area, p) {
                            1.0
                        } else {
                            0.0
                        }
                    }
                };
                if cand.u_mask >= weight {
                    continue;
                }
                let Some(hit) = surface.sample(p) else {
                    continue;
                };
                if hit.normal.y < self.cos_slope
                    || hit.height < self.heights.0
                    || hit.height > self.heights.1
                {
                    continue;
                }
                let total = *self.cdf.last().unwrap_or(&0.0);
                let pick = cand.u_species * total;
                let Some(species) = self.cdf.iter().position(|&x| pick < x) else {
                    continue;
                };
                let sp = self.species[species];
                let scale = sp.scale.0 + (sp.scale.1 - sp.scale.0) * cand.u_scale;
                let yaw = if sp.random_yaw {
                    Quat::from_rotation_y(cand.u_yaw * TAU)
                } else {
                    Quat::IDENTITY
                };
                let tilt =
                    Quat::IDENTITY.slerp(Quat::from_rotation_arc(Vec3::Y, hit.normal), sp.align);
                let tint = if sp.tinted {
                    ((cand.u_tint * f32::from(TINT_VARIANTS)) as u8).min(TINT_VARIANTS - 1)
                } else {
                    0
                };
                out.push(ScatterInstance {
                    id: cell_id(i, j),
                    species: species as u32,
                    tint,
                    transform: Transform {
                        translation: Vec3::new(p.x, hit.height, p.y),
                        rotation: (tilt * yaw).normalize(),
                        scale: Vec3::splat(scale),
                    },
                });
            }
        }
        out.sort_unstable_by_key(|x| x.id);
        out
    }

    fn candidate(&self, i: i32, j: i32) -> Candidate {
        let mut rng = Rng::new(self.seed).fork(cell_id(i, j));
        let u = rng.next_f32();
        let v = rng.next_f32();
        let pos = Vec2::new((i as f32 + u) * self.cell, (j as f32 + v) * self.cell);
        let alive = rng.next_f32() < self.p0;
        let priority = rng.next_u64();
        Candidate {
            pos,
            alive,
            priority,
            u_mask: rng.next_f32(),
            u_species: rng.next_f32(),
            u_scale: rng.next_f32(),
            u_yaw: rng.next_f32(),
            u_tint: rng.next_f32(),
        }
    }
}

/// Instances left by the minimum-distance test per disc of the minimum
/// distance (π r²), by candidates surviving the thinning per disc, for one
/// candidate per cell of r/√2 (scale-free). Measured over 6 seeds on
/// 160 m × 160 m with r = 1; the last row is no thinning (2π candidates
/// per disc), the most the minimum distance allows.
const KEPT: [(f32, f32); 20] = [
    (0.0, 0.0),
    (0.25, 0.2254),
    (0.5, 0.4049),
    (0.75, 0.5491),
    (1.0, 0.6635),
    (1.25, 0.755),
    (1.5, 0.8288),
    (1.75, 0.8845),
    (2.0, 0.9267),
    (2.25, 0.9589),
    (2.5, 0.9825),
    (2.75, 0.9983),
    (3.0, 1.0117),
    (3.25, 1.0214),
    (3.5, 1.0285),
    (3.75, 1.0319),
    (4.0, 1.0345),
    (4.5, 1.0392),
    (5.0, 1.0401),
    (TAU, 1.0419),
];

/// Surviving candidates per disc that leave `kept` per disc (inverse of
/// [`KEPT`], linear between rows).
fn thinning_for(kept: f32) -> f32 {
    for w in KEPT.windows(2) {
        let ((y0, k0), (y1, k1)) = (w[0], w[1]);
        if kept <= k1 {
            return y0 + (y1 - y0) * ((kept - k0) / (k1 - k0)).clamp(0.0, 1.0);
        }
    }
    TAU
}

/// `(i, j)` packed into one id.
fn cell_id(i: i32, j: i32) -> u64 {
    (u64::from(i as u32) << 32) | u64::from(j as u32)
}

struct Candidate {
    pos: Vec2,
    alive: bool,
    priority: u64,
    u_mask: f32,
    u_species: f32,
    u_scale: f32,
    u_yaw: f32,
    u_tint: f32,
}

/// `Some(true)`: the square at `origin` with edge `s` is inside `poly`;
/// `Some(false)`: outside; `None`: the outline crosses it.
fn classify(poly: &[Vec2], origin: Vec2, s: f32) -> Option<bool> {
    let (lo, hi) = (origin, origin + Vec2::splat(s));
    let n = poly.len();
    for k in 0..n {
        let a = poly[k];
        let b = poly[(k + 1) % n];
        if segment_hits_rect(a, b, lo, hi) {
            return None;
        }
    }
    Some(inside(poly, origin + Vec2::splat(s * 0.5)))
}

/// True if segment `ab` touches the closed rectangle `[lo, hi]`
/// (Liang–Barsky clip).
fn segment_hits_rect(a: Vec2, b: Vec2, lo: Vec2, hi: Vec2) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [
        (-d.x, a.x - lo.x),
        (d.x, hi.x - a.x),
        (-d.y, a.y - lo.y),
        (d.y, hi.y - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// Every chunk's instances (for tests and tools): `(chunk, instances)` in
/// chunk order.
pub fn place_all(
    scatter: &Scatter,
    surface: &ScatterSurface,
) -> Vec<(ChunkCoord, Vec<ScatterInstance>)> {
    let plan = ScatterPlan::new(scatter);
    plan.chunks().map(|c| (c, plan.place(c, surface))).collect()
}

/// A hash of instances (ids, species, tints and transform bits), for
/// determinism tests.
pub fn instances_hash<'a>(instances: impl IntoIterator<Item = &'a ScatterInstance>) -> u64 {
    let mut h = StableHasher::new();
    for x in instances {
        h.write_u64(x.id);
        h.write_u32(x.species);
        h.write_u8(x.tint);
        let t = &x.transform;
        for v in t
            .translation
            .to_array()
            .into_iter()
            .chain(t.rotation.to_array())
            .chain(t.scale.to_array())
        {
            h.write_u32(v.to_bits());
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_round_trip_and_reject_damage() {
        let mut cells = [0u8; TILE_LEN];
        for (i, c) in cells.iter_mut().enumerate() {
            *c = (i * 7) as u8;
        }
        let t = MaskTile::encode(-3, 5, &cells);
        assert_eq!(t.cells.len(), 512);
        assert_eq!(t.decode(), Some(cells));
        let bad = MaskTile {
            cells: "zz".into(),
            ..t
        };
        assert_eq!(bad.decode(), None);
    }

    #[test]
    fn classify_squares_against_a_polygon() {
        let poly = [
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 20.0),
            Vec2::new(0.0, 20.0),
        ];
        assert_eq!(classify(&poly, Vec2::new(4.0, 4.0), 8.0), Some(true));
        assert_eq!(classify(&poly, Vec2::new(30.0, 4.0), 8.0), Some(false));
        assert_eq!(classify(&poly, Vec2::new(16.0, 4.0), 8.0), None);
        // A polygon wholly inside the square crosses it.
        assert_eq!(classify(&poly, Vec2::new(-10.0, -10.0), 40.0), None);
    }

    #[test]
    fn painting_and_erasing_back_drops_the_tile() {
        let mut m = ScatterMask::default();
        let touched = m.paint(&[], Vec2::new(4.0, 4.0), 1.5, 1.0, false);
        assert_eq!(touched, vec![(0, 0)]);
        assert_eq!(m.tiles.len(), 1);
        for _ in 0..40 {
            m.paint(&[], Vec2::new(4.0, 4.0), 2.0, 1.0, true);
        }
        assert!(m.tiles.is_empty(), "erased back to the area: no tile");
    }

    #[test]
    fn flat_surface_and_mesh_surface_sample() {
        let mesh = MeshData::plane(10.0, 10.0);
        let s = ScatterSurface::from_mesh(&mesh, Affine3A::from_translation(Vec3::Y), 8.0);
        let hit = s.sample(Vec2::new(1.0, -2.0)).unwrap();
        assert!((hit.height - 1.0).abs() < 1e-5);
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(s.sample(Vec2::new(20.0, 0.0)).is_none());
        let (t, _) = s.raycast(Vec3::new(0.0, 5.0, 0.0), Vec3::NEG_Y).unwrap();
        assert!((t - 4.0).abs() < 1e-5);
        assert_eq!(ScatterSurface::Flat.sample(Vec2::ZERO).unwrap().height, 0.0);
    }
}
