//! Terrain (world building W2): a heightfield ground shaped by saved,
//! editable stamps, drawn and collided as chunks.
//!
//! **Saved intent.** [`Terrain`] is a reflected component: the area (size
//! and cell size, centred on its entity), a base height, an ordered list of
//! [`TerrainStamp`]s (hills, ridges, basins, flats, flattened paths, noise),
//! the colours of its layers and an edge skirt. Heights are a pure function
//! of those ([`Terrain::height_at`]), so ground queries are exact and cost a
//! few stamp evaluations, and nothing else needs saving.
//!
//! **Chunks.** The `terrain` rule (box_engine adds it to every app) cuts a
//! terrain into chunks of [`CHUNK_CELLS`] × `CHUNK_CELLS` cells, each a
//! derived part with a [`TerrainChunk`] generator that holds only the
//! stamps that reach it ([`terrain_parts`]). A stamp edit changes the
//! parameters of the chunks it touches and no others, so the generator
//! runtime regenerates only those (stamp, then throttled flush), and their
//! colliders follow (box_engine gives each chunk a `Collider { shape: Mesh }`
//! when [`Terrain::collide`] is on; the chunk hands physics its height grid,
//! so the collider is a heightfield of exactly the drawn surface).
//!
//! **Shading.** Each vertex gets a colour from the layers: grass, rock on
//! slopes steeper than [`TerrainLayers::rock_slope`], dirt under flattened
//! paths, and a low-frequency brightness variation. The part's standard
//! material multiplies it. [`Terrain::show_walkability`] tints ground
//! steeper than [`Terrain::walk_slope`] red and ground within 5° of it
//! orange, to see where a character cannot walk (ledges of other geometry,
//! such as stairs, show in `box walk`).
//!
//! Output is bit-identical on every platform (D17): heights use
//! `box_core::pmath` and integer hashing only.

use std::f32::consts::PI;

use bevy_reflect::Reflect;
use box_core::color::Color;
use box_core::mesh::MeshData;
use box_core::mesh_asset::HeightGrid;
use box_core::pmath;
use box_core::reflect::{
    Range, ReflectComponent, ReflectDefault, ReflectVariantDefaults, Step, Unit, VariantDefaults,
};
use glam::{Vec2, Vec3};

use crate::derive::Part;
use crate::generator::{ColliderHint, GenContext, GenOutput, Generator};
use crate::rule::Intent;

/// Cells along one side of a chunk.
pub const CHUNK_CELLS: u32 = 32;
/// Most cells along one side of a terrain (larger terrains are cut, with a
/// warning).
pub const MAX_CELLS: u32 = 4096;

/// One shaping operation of a [`Terrain`], applied in list order. Positions
/// are in the terrain's own space (x and z, metres, centred on its entity);
/// heights are metres.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq, VariantDefaults)]
#[type_path = "box_engine"]
pub enum TerrainStamp {
    /// A round hill (a negative height makes a dip), smooth all the way
    /// down.
    Hill {
        /// Where the top is.
        center: Vec2,
        /// Distance from the top to the foot.
        #[reflect(@Range { min: 0.1, max: 2000.0 }, @Unit("m"))]
        radius: f32,
        /// Height of the top above the ground under it.
        #[reflect(@Range { min: -500.0, max: 500.0 }, @Unit("m"))]
        height: f32,
    },
    /// A ridge (or, with a negative height, a valley) along a line of
    /// points.
    Ridge {
        /// The crest line (x, z).
        points: Vec<Vec2>,
        /// Full width at the foot.
        #[reflect(@Range { min: 0.1, max: 2000.0 }, @Unit("m"))]
        width: f32,
        /// Height of the crest.
        #[reflect(@Range { min: -500.0, max: 500.0 }, @Unit("m"))]
        height: f32,
    },
    /// A round hollow for a pond or lake.
    Basin {
        /// The deepest point.
        center: Vec2,
        /// Distance from the deepest point to the rim.
        #[reflect(@Range { min: 0.1, max: 2000.0 }, @Unit("m"))]
        radius: f32,
        /// How far the middle goes down.
        #[reflect(@Range { min: 0.0, max: 500.0 }, @Unit("m"))]
        depth: f32,
    },
    /// A flat round area at `height` (a building pad, a plaza), blending
    /// into the ground around it over `bank`.
    Flatten {
        /// The middle of the flat area.
        center: Vec2,
        /// Radius of the flat area.
        #[reflect(@Range { min: 0.0, max: 2000.0 }, @Unit("m"))]
        radius: f32,
        /// Its height.
        #[reflect(@Range { min: -500.0, max: 500.0 }, @Unit("m"))]
        height: f32,
        /// Width of the slope to the ground around it.
        #[reflect(@Range { min: 0.0, max: 200.0 }, @Unit("m"))]
        bank: f32,
    },
    /// A walkable strip along points (x, height, z): its side profile is
    /// the points' heights, `width` wide, blending into the ground over
    /// `bank` on each side and painted with the dirt layer. A path, a road,
    /// a ramp up to a shrine.
    FlattenPath {
        /// The centre line: x, height, z per point.
        points: Vec<Vec3>,
        /// Width of the level strip.
        #[reflect(@Range { min: 0.1, max: 200.0 }, @Unit("m"))]
        width: f32,
        /// Width of the slope to the ground on each side.
        #[reflect(@Range { min: 0.0, max: 200.0 }, @Unit("m"))]
        bank: f32,
    },
    /// Rolling unevenness over the whole terrain: `amplitude` metres high,
    /// features about `scale` metres across, `octaves` finer layers.
    Noise {
        /// Another pattern with the same settings.
        seed: u32,
        /// Size of the largest features.
        #[reflect(@Range { min: 0.5, max: 2000.0 }, @Unit("m"))]
        scale: f32,
        /// Height of the largest features.
        #[reflect(@Range { min: 0.0, max: 500.0 }, @Unit("m"))]
        amplitude: f32,
        /// Layers of finer detail, each half as high.
        #[reflect(@Range { min: 1.0, max: 8.0 })]
        octaves: u32,
    },
}

impl Default for TerrainStamp {
    fn default() -> Self {
        Self::Hill {
            center: Vec2::ZERO,
            radius: 12.0,
            height: 3.0,
        }
    }
}

/// A 12 m, 3 m hill; a 4 m wide, 2 m ridge; a 6 m, 1.5 m deep basin; an
/// 8 m flat with a 4 m bank; a 3 m path with 1.5 m banks; 1 m noise of
/// 24 m features in 3 octaves.
impl VariantDefaults for TerrainStamp {
    fn variant_default(variant: &str) -> Option<Self> {
        Some(match variant {
            "Hill" => Self::default(),
            "Ridge" => Self::Ridge {
                points: Vec::new(),
                width: 4.0,
                height: 2.0,
            },
            "Basin" => Self::Basin {
                center: Vec2::ZERO,
                radius: 6.0,
                depth: 1.5,
            },
            "Flatten" => Self::Flatten {
                center: Vec2::ZERO,
                radius: 8.0,
                height: 0.0,
                bank: 4.0,
            },
            "FlattenPath" => Self::FlattenPath {
                points: Vec::new(),
                width: 3.0,
                bank: 1.5,
            },
            "Noise" => Self::Noise {
                seed: 1,
                scale: 24.0,
                amplitude: 1.0,
                octaves: 3,
            },
            _ => return None,
        })
    }
}

/// The colours of a terrain's layers (linear) and where each shows.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct TerrainLayers {
    /// Flat and gently sloping ground.
    pub grass: Color,
    /// Under flattened paths.
    pub dirt: Color,
    /// Steep ground.
    pub rock: Color,
    /// Slope where rock starts to show, degrees.
    #[reflect(@Range { min: 0.0, max: 90.0 }, @Unit("°"))]
    pub rock_slope: f32,
    /// Brightness variation over the ground (0 is none).
    #[reflect(@Range { min: 0.0, max: 0.5 }, @Step(0.01))]
    pub variation: f32,
}

impl Default for TerrainLayers {
    fn default() -> Self {
        Self {
            grass: Color::hex(0x7f9a4e),
            dirt: Color::hex(0xb08a5e),
            rock: Color::hex(0x8a8178),
            rock_slope: 32.0,
            variation: 0.12,
        }
    }
}

/// Heightfield ground shaped by [`TerrainStamp`]s and drawn in chunks (see
/// the module docs). The terrain covers `size` metres on X and Z, centred
/// on its entity.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Terrain {
    /// Extent on X and Z.
    #[reflect(@Range { min: 1.0, max: 8192.0 }, @Unit("m"))]
    pub size: Vec2,
    /// Distance between height samples: smaller is smoother and heavier.
    #[reflect(@Range { min: 0.1, max: 16.0 }, @Unit("m"), @Step(0.05))]
    pub cell_size: f32,
    /// Height of the ground before any stamp.
    #[reflect(@Unit("m"))]
    pub base_height: f32,
    /// Shaping operations, applied in order.
    pub stamps: Vec<TerrainStamp>,
    /// Layer colours and where each shows.
    pub layers: TerrainLayers,
    /// Depth of the wall hanging down from the terrain's edges, so the
    /// world does not end in a paper-thin edge (0: none).
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m"))]
    pub skirt: f32,
    /// Give the chunks mesh colliders (characters walk on it, rays hit it).
    pub collide: bool,
    /// Tint ground a character cannot walk: steeper than `walk_slope` red,
    /// within 5° of it orange.
    pub show_walkability: bool,
    /// Steepest walkable slope for `show_walkability`, degrees (match the
    /// character's `max_slope`).
    #[reflect(@Range { min: 0.0, max: 90.0 }, @Unit("°"))]
    pub walk_slope: f32,
}

impl Default for Terrain {
    fn default() -> Self {
        Self {
            size: Vec2::splat(64.0),
            cell_size: 1.0,
            base_height: 0.0,
            stamps: Vec::new(),
            layers: TerrainLayers::default(),
            skirt: 2.0,
            collide: true,
            show_walkability: false,
            walk_slope: 53.0,
        }
    }
}

/// Height and dirt weight of the ground at a point, from `stamps`.
fn sample(base: f32, stamps: &[TerrainStamp], p: Vec2) -> (f32, f32) {
    let mut h = base;
    let mut dirt: f32 = 0.0;
    for s in stamps {
        match s {
            TerrainStamp::Hill {
                center,
                radius,
                height,
            } => h += height * bump(p.distance(*center) / radius.max(1e-3)),
            TerrainStamp::Basin {
                center,
                radius,
                depth,
            } => h -= depth * bump(p.distance(*center) / radius.max(1e-3)),
            TerrainStamp::Ridge {
                points,
                width,
                height,
            } => {
                if let Some((d, _)) = polyline_distance(points.iter().copied(), p) {
                    h += height * bump(d / (width * 0.5).max(1e-3));
                }
            }
            TerrainStamp::Flatten {
                center,
                radius,
                height,
                bank,
            } => {
                let w = plateau(p.distance(*center), *radius, *bank);
                h += (height - h) * w;
            }
            TerrainStamp::FlattenPath {
                points,
                width,
                bank,
            } => {
                let flat = points.iter().map(|q| Vec2::new(q.x, q.z));
                if let Some((d, (i, t))) = polyline_distance(flat, p) {
                    let y = if points.len() == 1 {
                        points[0].y
                    } else {
                        points[i].y + (points[i + 1].y - points[i].y) * t
                    };
                    let w = plateau(d, width * 0.5, *bank);
                    h += (y - h) * w;
                    dirt = dirt.max(plateau(d, width * 0.5, bank.min(width * 0.25)));
                }
            }
            TerrainStamp::Noise {
                seed,
                scale,
                amplitude,
                octaves,
            } => h += amplitude * fbm(p / scale.max(1e-3), *seed, (*octaves).clamp(1, 8)),
        }
    }
    (h, dirt)
}

/// 1 at the centre to 0 at `u = 1`, smooth at both ends (a raised cosine).
fn bump(u: f32) -> f32 {
    if u >= 1.0 {
        0.0
    } else {
        0.5 * (1.0 + pmath::cos(PI * u))
    }
}

/// 1 within `inner`, falling to 0 over `bank` beyond it.
fn plateau(d: f32, inner: f32, bank: f32) -> f32 {
    if d <= inner {
        1.0
    } else if bank <= 0.0 {
        0.0
    } else {
        bump((d - inner) / bank)
    }
}

/// Distance from `p` to a polyline, and the segment index and position
/// along it of the closest point. `None` without points.
fn polyline_distance(points: impl Iterator<Item = Vec2>, p: Vec2) -> Option<(f32, (usize, f32))> {
    let pts: Vec<Vec2> = points.collect();
    match pts.len() {
        0 => None,
        1 => Some((p.distance(pts[0]), (0, 0.0))),
        _ => {
            let mut best = (f32::INFINITY, (0, 0.0));
            for (i, w) in pts.windows(2).enumerate() {
                let (a, b) = (w[0], w[1]);
                let ab = b - a;
                let len2 = ab.length_squared();
                let t = if len2 > 0.0 {
                    ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let d = p.distance(a + ab * t);
                if d < best.0 {
                    best = (d, (i, t));
                }
            }
            Some(best)
        }
    }
}

/// Integer hash of a lattice point to -1..1.
fn lattice(x: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (z as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 8_388_607.5 - 1.0
}

/// Smooth value noise, about -1..1.
fn value_noise(p: Vec2, seed: u32) -> f32 {
    let (fx, fz) = (p.x.floor(), p.y.floor());
    let (x, z) = (fx as i32, fz as i32);
    let (tx, tz) = (p.x - fx, p.y - fz);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (s(tx), s(tz));
    let a = lattice(x, z, seed);
    let b = lattice(x + 1, z, seed);
    let c = lattice(x, z + 1, seed);
    let d = lattice(x + 1, z + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sz
}

/// Octaves of value noise, normalised to about -1..1.
fn fbm(p: Vec2, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 1.0, 0.0, p);
    for o in 0..octaves {
        sum += amp * value_noise(q, seed.wrapping_add(o.wrapping_mul(7919)));
        norm += amp;
        amp *= 0.5;
        q = q * 2.03 + Vec2::new(17.3, -9.1);
    }
    sum / norm
}

/// The area a stamp changes, on X and Z (`None`: everywhere).
fn stamp_bounds(s: &TerrainStamp) -> Option<(Vec2, Vec2)> {
    let around = |c: Vec2, r: f32| Some((c - Vec2::splat(r), c + Vec2::splat(r)));
    let along = |pts: &mut dyn Iterator<Item = Vec2>, r: f32| {
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for p in pts {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo.x <= hi.x).then(|| (lo - Vec2::splat(r), hi + Vec2::splat(r)))
    };
    match s {
        TerrainStamp::Hill { center, radius, .. } | TerrainStamp::Basin { center, radius, .. } => {
            around(*center, *radius)
        }
        TerrainStamp::Flatten {
            center,
            radius,
            bank,
            ..
        } => around(*center, radius + bank),
        TerrainStamp::Ridge { points, width, .. } => {
            along(&mut points.iter().copied(), width * 0.5)
        }
        TerrainStamp::FlattenPath {
            points,
            width,
            bank,
        } => along(
            &mut points.iter().map(|p| Vec2::new(p.x, p.z)),
            width * 0.5 + bank,
        ),
        TerrainStamp::Noise { .. } => None,
    }
}

impl Terrain {
    /// The ground height at `p` (terrain space x, z), from the stamps. The
    /// drawn surface interpolates these at the cell corners.
    pub fn height_at(&self, p: Vec2) -> f32 {
        sample(self.base_height, &self.stamps, p).0
    }

    /// The surface normal at `p` (terrain space), from heights one cell
    /// apart.
    pub fn normal_at(&self, p: Vec2) -> Vec3 {
        normal(self.base_height, &self.stamps, p, self.cell_size.max(0.01))
    }

    /// The height of the drawn (and collided) surface at `p` (terrain
    /// space x, z): the cell corners' heights interpolated over the
    /// triangle that holds `p`, split as the chunks split it. `None`
    /// outside the terrain's area. Differs from [`height_at`](Self::height_at)
    /// inside cells by the drawing's error (small at a fine `cell_size`).
    pub fn surface_height(&self, p: Vec2) -> Option<f32> {
        if self.cell_size.is_nan() || self.cell_size <= 0.0 || !p.is_finite() {
            return None;
        }
        let (cx, cz, _) = self.cells();
        let cell = self.cell_size.max(0.01);
        let half = Vec2::new(cx as f32, cz as f32) * cell * 0.5;
        let g = (p + half) / cell;
        if g.x < 0.0 || g.y < 0.0 || g.x > cx as f32 || g.y > cz as f32 {
            return None;
        }
        let i = (g.x as u32).min(cx - 1);
        let j = (g.y as u32).min(cz - 1);
        let (u, v) = (g.x - i as f32, g.y - j as f32);
        // Corner positions computed exactly as the chunk computes them, so
        // the split matches the drawn mesh bit for bit.
        let corner = |i: u32, j: u32| {
            let (kx, kz) = (i.min(cx - 1) / CHUNK_CELLS, j.min(cz - 1) / CHUNK_CELLS);
            let origin =
                Vec2::new((kx * CHUNK_CELLS) as f32, (kz * CHUNK_CELLS) as f32) * cell - half;
            let local = Vec2::new((i - kx * CHUNK_CELLS) as f32, (j - kz * CHUNK_CELLS) as f32);
            self.height_at(origin + local * cell)
        };
        let (ha, hb, hc, hd) = (
            corner(i, j),
            corner(i + 1, j),
            corner(i, j + 1),
            corner(i + 1, j + 1),
        );
        Some(if uses_main_diagonal(ha, hb, hc, hd) {
            // Split along a–d.
            if v >= u {
                ha + (hd - hc) * u + (hc - ha) * v
            } else {
                ha + (hb - ha) * u + (hd - hb) * v
            }
        } else if u + v <= 1.0 {
            ha + (hb - ha) * u + (hc - ha) * v
        } else {
            hd + (hc - hd) * (1.0 - u) + (hb - hd) * (1.0 - v)
        })
    }

    /// The terrain's area on X and Z in terrain space (centred on its
    /// entity), as drawn.
    pub fn extent(&self) -> (Vec2, Vec2) {
        let (cx, cz, _) = self.cells();
        let half = Vec2::new(cx as f32, cz as f32) * self.cell_size.max(0.01) * 0.5;
        (-half, half)
    }

    /// The areas (terrain space x, z) where heights may differ between
    /// `self` and `other`: the areas of stamps that differ, or the whole of
    /// both areas when the base, size, cell size or a stamp without bounds
    /// (noise) differs. Empty when the heights are the same.
    pub fn height_changes(&self, other: &Terrain) -> Vec<(Vec2, Vec2)> {
        let whole = || {
            let (a, b) = (self.extent(), other.extent());
            vec![(a.0.min(b.0), a.1.max(b.1))]
        };
        if self.base_height != other.base_height
            || self.cell_size != other.cell_size
            || self.size != other.size
        {
            return whole();
        }
        let mut out = Vec::new();
        for k in 0..self.stamps.len().max(other.stamps.len()) {
            let (a, b) = (self.stamps.get(k), other.stamps.get(k));
            if a == b {
                continue;
            }
            for s in [a, b].into_iter().flatten() {
                match stamp_bounds(s) {
                    Some(r) if !out.contains(&r) => out.push(r),
                    Some(_) => {}
                    None => return whole(),
                }
            }
        }
        out
    }

    /// Cells along X and Z (each at least 1, at most [`MAX_CELLS`]) and
    /// whether the size had to be cut.
    pub fn cells(&self) -> (u32, u32, bool) {
        let cell = self.cell_size.max(0.01);
        let n = |s: f32| (s.max(0.0) / cell).round().max(1.0);
        let (x, z) = (n(self.size.x), n(self.size.y));
        let cut = x > MAX_CELLS as f32 || z > MAX_CELLS as f32;
        (
            x.min(MAX_CELLS as f32) as u32,
            z.min(MAX_CELLS as f32) as u32,
            cut,
        )
    }
}

fn normal(base: f32, stamps: &[TerrainStamp], p: Vec2, e: f32) -> Vec3 {
    let h = |q: Vec2| sample(base, stamps, q).0;
    let dx = h(p + Vec2::X * e) - h(p - Vec2::X * e);
    let dz = h(p + Vec2::Y * e) - h(p - Vec2::Y * e);
    Vec3::new(-dx, 2.0 * e, -dz).normalize_or(Vec3::Y)
}

/// Whether a terrain cell is split along its a–d diagonal.
pub(crate) fn uses_main_diagonal(ha: f32, hb: f32, hc: f32, hd: f32) -> bool {
    (ha - hd).abs() <= (hb - hc).abs()
}

/// One chunk of a [`Terrain`]: generated by the `terrain` rule as a part
/// of the terrain's entity, never saved. It holds the stamps that reach
/// it, so editing a stamp regenerates only the chunks under it.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq, @box_core::reflect::Hidden)]
#[type_path = "box_engine"]
pub struct TerrainChunk {
    /// Terrain-space corner (x, z) of the chunk's first cell.
    pub origin: Vec2,
    /// Cells along X.
    pub cells_x: u32,
    /// Cells along Z.
    pub cells_z: u32,
    /// Distance between height samples.
    pub cell_size: f32,
    /// The terrain's base height.
    pub base_height: f32,
    /// The terrain's stamps that change this chunk, in order.
    pub stamps: Vec<TerrainStamp>,
    /// The terrain's layer colours.
    pub layers: TerrainLayers,
    /// Skirt depth.
    pub skirt: f32,
    /// Which edges get a skirt: bits -X, +X, -Z, +Z.
    pub skirt_edges: u32,
    /// The terrain's walkability tint.
    pub show_walkability: bool,
    /// The terrain's walkable slope, degrees.
    pub walk_slope: f32,
}

impl Default for TerrainChunk {
    fn default() -> Self {
        let t = Terrain::default();
        Self {
            origin: Vec2::ZERO,
            cells_x: CHUNK_CELLS,
            cells_z: CHUNK_CELLS,
            cell_size: 1.0,
            base_height: 0.0,
            stamps: Vec::new(),
            layers: t.layers,
            skirt: 0.0,
            skirt_edges: 0,
            show_walkability: false,
            walk_slope: t.walk_slope,
        }
    }
}

/// Edge bits of [`TerrainChunk::skirt_edges`].
const EDGE_NEG_X: u32 = 1;
const EDGE_POS_X: u32 = 2;
const EDGE_NEG_Z: u32 = 4;
const EDGE_POS_Z: u32 = 8;

impl TerrainChunk {
    /// Position, normal and dirt weight of a top-surface grid vertex.
    pub(crate) fn surface_vertex(&self, i: usize, j: usize) -> (Vec3, Vec3, f32) {
        let p = self.origin + Vec2::new(i as f32, j as f32) * self.cell_size;
        let (height, dirt) = sample(self.base_height, &self.stamps, p);
        let n = normal(self.base_height, &self.stamps, p, self.cell_size);
        (Vec3::new(p.x, height, p.y), n, dirt)
    }

    fn vertex_color(&self, p: Vec2, n: Vec3, dirt: f32) -> [f32; 4] {
        let l = &self.layers;
        let mix = |a: Color, b: Color, t: f32| {
            Color::linear(
                a.r + (b.r - a.r) * t,
                a.g + (b.g - a.g) * t,
                a.b + (b.b - a.b) * t,
            )
        };
        // Rock from a few degrees below `rock_slope` to a few above.
        let cos_at = |deg: f32| pmath::cos(deg.clamp(0.0, 90.0) * PI / 180.0);
        let (lo, hi) = (cos_at(l.rock_slope - 6.0), cos_at(l.rock_slope + 6.0));
        let rock = ((lo - n.y) / (lo - hi).max(1e-4)).clamp(0.0, 1.0);
        let mut c = mix(l.grass, l.dirt, dirt.clamp(0.0, 1.0));
        c = mix(c, l.rock, rock);
        let v = 1.0 + l.variation * fbm(p / 9.0, 0x7e11, 2);
        let mut rgb = [c.r * v, c.g * v, c.b * v];
        if self.show_walkability {
            if n.y < cos_at(self.walk_slope) {
                rgb = [0.9, 0.08, 0.05];
            } else if n.y < cos_at(self.walk_slope - 5.0) {
                rgb = [1.0, 0.45, 0.02];
            }
        }
        [rgb[0], rgb[1], rgb[2], 1.0]
    }
}

impl Generator for TerrainChunk {
    const VERSION: u32 = 1;
    const CATEGORY: &'static str = "Terrain";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let cell = self.cell_size;
        if cell.is_nan() || cell <= 0.0 || self.cells_x == 0 || self.cells_z == 0 {
            return GenOutput::empty().warn("a terrain chunk needs cells and a cell size above 0");
        }
        let (nx, nz) = (self.cells_x as usize + 1, self.cells_z as usize + 1);
        let mut mesh = MeshData::default();
        let mut heights = Vec::with_capacity(nx * nz);
        for j in 0..nz {
            for i in 0..nx {
                let (p, n, dirt) = self.surface_vertex(i, j);
                mesh.positions.push(p);
                mesh.normals.push(n);
                mesh.colors
                    .push(self.vertex_color(Vec2::new(p.x, p.z), n, dirt));
                heights.push(p.y);
            }
        }
        let uvs: Vec<Vec2> = mesh.positions.iter().map(|p| Vec2::new(p.x, p.z)).collect();
        let mut main_diagonal = Vec::with_capacity((nx - 1) * (nz - 1));
        for j in 0..nz - 1 {
            for i in 0..nx - 1 {
                let a = (j * nx + i) as u32;
                let (b, c, d) = (a + 1, a + nx as u32, a + nx as u32 + 1);
                // Split along the diagonal that follows the surface better.
                let (ha, hb, hc, hd) = (
                    heights[a as usize],
                    heights[b as usize],
                    heights[c as usize],
                    heights[d as usize],
                );
                let main = uses_main_diagonal(ha, hb, hc, hd);
                main_diagonal.push(main);
                if main {
                    mesh.indices.extend_from_slice(&[a, c, d, a, d, b]);
                } else {
                    mesh.indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
        }
        let mut uvs = uvs;
        if self.skirt > 0.0 && self.skirt_edges != 0 {
            let edges: [(u32, Vec<usize>, Vec3); 4] = [
                (EDGE_NEG_X, (0..nz).map(|j| j * nx).collect(), Vec3::NEG_X),
                (
                    EDGE_POS_X,
                    (0..nz).rev().map(|j| j * nx + nx - 1).collect(),
                    Vec3::X,
                ),
                (EDGE_NEG_Z, (0..nx).rev().collect(), Vec3::NEG_Z),
                (
                    EDGE_POS_Z,
                    (0..nx).map(|i| (nz - 1) * nx + i).collect(),
                    Vec3::Z,
                ),
            ];
            for (bit, line, outward) in edges {
                if self.skirt_edges & bit == 0 {
                    continue;
                }
                let start = mesh.positions.len() as u32;
                for &v in &line {
                    let top = mesh.positions[v];
                    let color = mesh.colors[v];
                    for y in [top.y, top.y - self.skirt] {
                        mesh.positions.push(Vec3::new(top.x, y, top.z));
                        mesh.normals.push(outward);
                        let shade = if y < top.y { 0.55 } else { 0.8 };
                        mesh.colors.push([
                            color[0] * shade,
                            color[1] * shade,
                            color[2] * shade,
                            1.0,
                        ]);
                        uvs.push(Vec2::new(top.x + top.z, y));
                    }
                }
                for k in 0..line.len() as u32 - 1 {
                    let (t0, b0, t1, b1) = (
                        start + 2 * k,
                        start + 2 * k + 1,
                        start + 2 * k + 2,
                        start + 2 * k + 3,
                    );
                    mesh.indices.extend_from_slice(&[t0, b0, b1, t0, b1, t1]);
                }
            }
        }
        mesh.uvs = vec![uvs];
        // The same surface for physics (the skirt is only for looks).
        let grid = HeightGrid {
            origin: self.origin,
            cell_size: cell,
            columns: nx as u32,
            rows: nz as u32,
            heights,
            main_diagonal,
        };
        GenOutput::mesh(mesh).with_collider(ColliderHint::Heights(std::sync::Arc::new(grid)))
    }
}

/// The chunk parts of one terrain (the `terrain` rule's output for it):
/// one [`TerrainChunk`] per chunk, keyed `chunk/<x>_<z>`, each holding the
/// stamps whose area reaches it. Warnings for a cut size are returned
/// with them.
pub fn terrain_parts(intent: &Intent<Terrain>) -> (Vec<Part>, Vec<String>) {
    let t = &intent.value;
    let mut warnings = Vec::new();
    if t.cell_size.is_nan()
        || t.cell_size <= 0.0
        || !t.size.is_finite()
        || t.size.min_element() <= 0.0
    {
        warnings.push("terrain size and cell size must be above 0".to_string());
        return (Vec::new(), warnings);
    }
    if t.cells().2 {
        warnings.push(format!(
            "terrain cut to {MAX_CELLS} cells per side: raise cell_size or shrink size"
        ));
    }
    let parts = t
        .chunks()
        .into_iter()
        .map(|((kx, kz), chunk)| Part::new(intent.key(format!("chunk/{kx}_{kz}"))).with(chunk))
        .collect();
    (parts, warnings)
}

impl Terrain {
    /// The terrain's chunks with their (x, z) chunk coordinates, each
    /// holding the stamps that reach it (what [`terrain_parts`] makes
    /// parts of). Empty for a size or cell size not above 0.
    pub fn chunks(&self) -> Vec<((u32, u32), TerrainChunk)> {
        self.chunks_matching(|_, _| true)
    }

    /// Builds the selected chunks without collecting stamps for other chunks.
    pub(crate) fn chunks_matching(
        &self,
        include: impl Fn(u32, u32) -> bool,
    ) -> Vec<((u32, u32), TerrainChunk)> {
        let t = self;
        if t.cell_size.is_nan()
            || t.cell_size <= 0.0
            || !t.size.is_finite()
            || t.size.min_element() <= 0.0
        {
            return Vec::new();
        }
        let (cx, cz, _) = t.cells();
        let cell = t.cell_size.max(0.01);
        let half = Vec2::new(cx as f32, cz as f32) * cell * 0.5;
        let chunks = |n: u32| n.div_ceil(CHUNK_CELLS);
        let bounds: Vec<Option<(Vec2, Vec2)>> = t.stamps.iter().map(stamp_bounds).collect();
        let mut parts = Vec::new();
        for kz in 0..chunks(cz) {
            for kx in 0..chunks(cx) {
                if !include(kx, kz) {
                    continue;
                }
                let cells_x = CHUNK_CELLS.min(cx - kx * CHUNK_CELLS);
                let cells_z = CHUNK_CELLS.min(cz - kz * CHUNK_CELLS);
                let origin =
                    Vec2::new((kx * CHUNK_CELLS) as f32, (kz * CHUNK_CELLS) as f32) * cell - half;
                let size = Vec2::new(cells_x as f32, cells_z as f32) * cell;
                // One cell of margin: normals and ledges look one cell out.
                let (lo, hi) = (
                    origin - Vec2::splat(cell),
                    origin + size + Vec2::splat(cell),
                );
                let stamps: Vec<TerrainStamp> = t
                    .stamps
                    .iter()
                    .zip(&bounds)
                    .filter(|(_, b)| {
                        b.is_none_or(|(a, c)| {
                            a.x <= hi.x && c.x >= lo.x && a.y <= hi.y && c.y >= lo.y
                        })
                    })
                    .map(|(s, _)| s.clone())
                    .collect();
                let mut edges = 0;
                if kx == 0 {
                    edges |= EDGE_NEG_X;
                }
                if kx + 1 == chunks(cx) {
                    edges |= EDGE_POS_X;
                }
                if kz == 0 {
                    edges |= EDGE_NEG_Z;
                }
                if kz + 1 == chunks(cz) {
                    edges |= EDGE_POS_Z;
                }
                let chunk = TerrainChunk {
                    origin,
                    cells_x,
                    cells_z,
                    cell_size: cell,
                    base_height: t.base_height,
                    stamps,
                    layers: t.layers.clone(),
                    skirt: t.skirt.max(0.0),
                    skirt_edges: edges,
                    show_walkability: t.show_walkability,
                    walk_slope: t.walk_slope,
                };
                parts.push(((kx, kz), chunk));
            }
        }
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hill() -> Terrain {
        Terrain {
            size: Vec2::splat(64.0),
            stamps: vec![TerrainStamp::Hill {
                center: Vec2::new(8.0, 0.0),
                radius: 10.0,
                height: 4.0,
            }],
            ..Terrain::default()
        }
    }

    #[test]
    fn stamps_shape_heights() {
        let t = hill();
        assert!((t.height_at(Vec2::new(8.0, 0.0)) - 4.0).abs() < 1e-5);
        assert_eq!(t.height_at(Vec2::new(30.0, 0.0)), 0.0);
        let mut t = t;
        t.stamps.push(TerrainStamp::Flatten {
            center: Vec2::new(8.0, 0.0),
            radius: 2.0,
            height: 1.0,
            bank: 2.0,
        });
        assert!(
            (t.height_at(Vec2::new(9.0, 0.0)) - 1.0).abs() < 1e-5,
            "flattened"
        );
        let path = TerrainStamp::FlattenPath {
            points: vec![Vec3::new(-20.0, 0.5, -10.0), Vec3::new(-20.0, 1.5, 10.0)],
            width: 2.0,
            bank: 1.0,
        };
        t.stamps.push(path);
        assert!(
            (t.height_at(Vec2::new(-20.0, 0.0)) - 1.0).abs() < 1e-5,
            "follows the path"
        );
        let (_, dirt) = sample(t.base_height, &t.stamps, Vec2::new(-20.0, 0.0));
        assert_eq!(dirt, 1.0);
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a = fbm(Vec2::new(3.7, -1.2), 5, 4);
        assert_eq!(a, fbm(Vec2::new(3.7, -1.2), 5, 4));
        assert_ne!(a, fbm(Vec2::new(3.7, -1.2), 6, 4));
        for i in 0..200 {
            let v = fbm(Vec2::new(i as f32 * 0.37, i as f32 * -0.11), 9, 3);
            assert!((-1.0..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn chunks_hold_only_the_stamps_that_reach_them() {
        let t = hill();
        let intent = Intent {
            entity: box_core::world::Entity::DANGLING,
            uid: box_core::identity::Uid::new(),
            value: t,
            transform: glam::Affine3A::IDENTITY,
        };
        let (parts, warnings) = terrain_parts(&intent);
        assert!(warnings.is_empty());
        assert_eq!(parts.len(), 4, "64 m at 1 m cells: 2 x 2 chunks");
    }

    #[test]
    fn chunk_edges_match_their_neighbours() {
        let t = Terrain {
            stamps: vec![
                TerrainStamp::Hill {
                    center: Vec2::ZERO,
                    radius: 20.0,
                    height: 5.0,
                },
                TerrainStamp::Noise {
                    seed: 3,
                    scale: 10.0,
                    amplitude: 1.0,
                    octaves: 3,
                },
            ],
            ..Terrain::default()
        };
        let ctx = GenContext::new(0);
        let chunk = |origin: Vec2| TerrainChunk {
            origin,
            stamps: t.stamps.clone(),
            ..TerrainChunk::default()
        };
        let a = chunk(Vec2::new(-32.0, -32.0)).generate(&ctx).mesh.unwrap();
        let b = chunk(Vec2::new(0.0, -32.0)).generate(&ctx).mesh.unwrap();
        // a's last column is b's first: same positions and normals.
        for j in 0..=32 {
            let ia = j * 33 + 32;
            let ib = j * 33;
            assert_eq!(a.positions[ia], b.positions[ib]);
            assert_eq!(a.normals[ia], b.normals[ib]);
        }
    }
}
