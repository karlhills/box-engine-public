//! Spline paths on a terrain. Each chunk gets the union of the lane
//! footprints that reach it, so overlaps and intersections have one
//! surface. The union is cut along the terrain's own triangle edges.

use std::collections::{BTreeMap, HashMap};
use std::f32::consts::TAU;

use bevy_reflect::Reflect;
use box_core::identity::Uid;
use box_core::material::StandardMaterial;
use box_core::pmath;
use box_core::reflect::{Hidden, Range, ReflectComponent, ReflectDefault, Unit};
use glam::{Mat2, Vec2, Vec3, Vec3Swizzles};

use crate::derive::Part;
use crate::generator::{GenContext, GenOutput, Generator};
use crate::mesh_builder::{MeshBuilder, Vertex};
use crate::poly::{self, Polygon};
use crate::procedural::Procedural;
use crate::rule::{Intent, RuleInput, RuleOutput};
use crate::spline::{Sampling, Spline};
use crate::terrain::{CHUNK_CELLS, Terrain, TerrainChunk, uses_main_diagonal};

pub const TERRAIN_PATH_RULE: &str = "terrain_paths";

const CURVE_TOLERANCE: f32 = 0.025;
const CAP_SEGMENTS: usize = 16;
// Vertices lie directly over the drawn terrain triangles. This lift only
// separates depth values. The terrain still provides all collision.
const SURFACE_LIFT: f32 = 0.01;

/// A dirt path following the XZ course of the `Spline` on this entity.
/// Paths on the same terrain merge wherever their footprints touch.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct TerrainPath {
    /// Full width in world metres.
    #[reflect(@Range { min: 0.1, max: 100.0 }, @Unit("m"))]
    pub width: f32,
    /// Terrain to follow. Leave empty when the scene has exactly one terrain.
    pub terrain: Option<Uid>,
}

impl Default for TerrainPath {
    fn default() -> Self {
        Self {
            width: 2.0,
            terrain: None,
        }
    }
}

/// Converts lane intents to merged path meshes owned by their terrains.
pub fn terrain_path_rule(input: &RuleInput<TerrainPath>) -> RuleOutput {
    let mut out = RuleOutput::default();
    if input.intents.is_empty() {
        return out;
    }
    let terrains = input.read::<Terrain>();
    let terrain_by_uid: BTreeMap<_, _> = terrains.iter().map(|t| (t.uid, t)).collect();
    let splines: HashMap<_, _> = input.read::<Spline>().iter().map(|s| (s.uid, s)).collect();
    let mut paths: Vec<_> = input.intents.iter().collect();
    paths.sort_unstable_by_key(|path| path.uid);
    let mut footprints: BTreeMap<Uid, Vec<Polygon>> = BTreeMap::new();
    for path in paths {
        let terrain = match path.value.terrain {
            Some(uid) => terrain_by_uid.get(&uid).copied(),
            None if terrains.len() == 1 => terrains.first(),
            None => None,
        };
        let Some(terrain) = terrain else {
            out.warn(
                path.uid,
                "set TerrainPath.terrain to a terrain entity in this scene",
            );
            continue;
        };
        let Some(spline) = splines.get(&path.uid) else {
            out.warn(path.uid, "TerrainPath needs a Spline on the same entity");
            continue;
        };
        let tr = terrain.transform;
        let plane = Mat2::from_cols(tr.matrix3.x_axis.xz(), tr.matrix3.z_axis.xz());
        let up = tr.matrix3.y_axis;
        if !plane.is_finite()
            || !tr.translation.is_finite()
            || !up.is_finite()
            || up.y <= 0.0
            || up.xz().length() > 1e-5 * up.y
            || plane.determinant().abs() < 1e-8
        {
            out.warn(
                path.uid,
                "TerrainPath needs upright terrain with a nonzero scale",
            );
            continue;
        }
        if !path.value.width.is_finite() || !(0.1..=100.0).contains(&path.value.width) {
            out.warn(path.uid, "TerrainPath.width must be between 0.1 and 100 m");
            continue;
        }
        let mut ribbons = footprint(spline, path.value.width / 2.0);
        if ribbons.is_empty() {
            out.warn(
                path.uid,
                "TerrainPath needs at least two distinct points in XZ",
            );
            continue;
        }
        let inverse = plane.inverse();
        for p in ribbons.iter_mut().flat_map(|poly| &mut poly.outer) {
            *p = inverse * (*p - tr.translation.xz());
        }
        footprints.entry(terrain.uid).or_default().extend(ribbons);
    }
    for (uid, polygons) in footprints {
        add_chunks(terrain_by_uid[&uid], polygons, &mut out);
    }
    out
}

fn footprint(intent: &Intent<Spline>, radius: f32) -> Vec<Polygon> {
    let mut spline = intent.value.clone();
    for p in &mut spline.points {
        let flat = |p: Vec3| Vec3::new(p.x, 0.0, p.z);
        p.position = flat(intent.transform.transform_point3(p.position));
        p.in_tangent = p
            .in_tangent
            .map(|v| flat(intent.transform.transform_vector3(v)));
        p.out_tangent = p
            .out_tangent
            .map(|v| flat(intent.transform.transform_vector3(v)));
    }
    let mut points: Vec<_> = spline
        .curve()
        .sample(Sampling::Adaptive {
            tolerance: CURVE_TOLERANCE,
            max_step: f32::MAX,
        })
        .into_iter()
        .map(|p| p.position.xz())
        .filter(|p| p.is_finite() && p.abs().max_element() <= poly::MAX_COORD)
        .collect();
    points.dedup_by(|a, b| a.distance_squared(*b) < 1e-8);
    if points.len() < 2 {
        return Vec::new();
    }
    // Round caps also fill the outside of each bend and let paths with a
    // shared endpoint merge regardless of angle or width.
    let ring: Vec<_> = (0..CAP_SEGMENTS)
        .map(|i| {
            let (sin, cos) = pmath::sin_cos(TAU * i as f32 / CAP_SEGMENTS as f32);
            Vec2::new(cos, sin) * radius
        })
        .collect();
    let mut polygons: Vec<_> = points
        .iter()
        .map(|&center| polygon(ring.iter().map(|p| center + p).collect()))
        .collect();
    if spline.closed {
        points.push(points[0]);
    }
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if let Some(side) = (b - a).perp().try_normalize() {
            let side = side * radius;
            polygons.push(polygon(vec![a - side, b - side, b + side, a + side]));
        }
    }
    polygons
}

fn polygon(outer: Vec<Vec2>) -> Polygon {
    Polygon {
        outer,
        holes: Vec::new(),
    }
}

fn add_chunks(terrain: &Intent<Terrain>, polygons: Vec<Polygon>, out: &mut RuleOutput) {
    let t = &terrain.value;
    if t.show_walkability
        || !t.cell_size.is_finite()
        || t.cell_size <= 0.0
        || !t.size.is_finite()
        || t.size.min_element() <= 0.0
    {
        return;
    }
    let (start, end) = t.extent();
    let chunk_width = t.cell_size.max(0.01) * CHUNK_CELLS as f32;
    let (cx, cz, _) = t.cells();
    let (max_x, max_z) = ((cx - 1) / CHUNK_CELLS, (cz - 1) / CHUNK_CELLS);
    let cell = |p: Vec2| {
        ((p - start) / chunk_width)
            .floor()
            .max(Vec2::ZERO)
            .as_uvec2()
    };
    let mut bins: HashMap<_, Vec<_>> = HashMap::new();
    for polygon in &polygons {
        let Some((lo, hi)) = polygon.bounds() else {
            continue;
        };
        if hi.cmplt(start).any() || lo.cmpgt(end).any() {
            continue;
        }
        let (lo, hi) = (cell(lo), cell(hi));
        for z in lo.y.min(max_z)..=hi.y.min(max_z) {
            for x in lo.x.min(max_x)..=hi.x.min(max_x) {
                bins.entry((x, z)).or_default().push(polygon);
            }
        }
    }
    for ((x, z), chunk) in t.chunks_matching(|x, z| bins.contains_key(&(x, z))) {
        let polygons = &bins[&(x, z)];
        let lo = chunk.origin;
        let hi = lo + Vec2::new(chunk.cells_x as f32, chunk.cells_z as f32) * chunk.cell_size;
        // Clip first to keep the union local to the chunk, including the
        // precision of the boolean kernel for a large world.
        let polygons: Vec<_> = polygons
            .iter()
            .map(|p| polygon(clip_rect(&p.outer, lo, hi)))
            .collect();
        let surface = poly::triangulate_all(&poly::make_valid(&polygons));
        if surface.indices.is_empty() {
            continue;
        }
        let triangles = surface
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|tri| tri.map(|i| surface.points[i as usize]))
            .collect();
        let material = StandardMaterial {
            base_color: chunk.layers.dirt,
            roughness: 0.95,
            ..StandardMaterial::default()
        };
        out.push(
            Part::new(terrain.key(format!("path/{x}_{z}")))
                .with(TerrainPathChunk { chunk, triangles })
                .with(material)
                .with(Procedural {
                    cast_shadows: false,
                    ..Procedural::default()
                }),
        );
    }
}

/// Runtime mesh parameters of one path chunk, owned by the terrain.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq, @Hidden)]
#[type_path = "box_engine"]
pub(crate) struct TerrainPathChunk {
    /// Terrain cell grid and stamps used to drape the path.
    chunk: TerrainChunk,
    /// Joined path footprint triangles in terrain-local XZ coordinates.
    triangles: Vec<[Vec2; 3]>,
}

impl Generator for TerrainPathChunk {
    const VERSION: u32 = 1;
    const CATEGORY: &'static str = "Terrain";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let chunk = &self.chunk;
        let nx = chunk.cells_x.min(CHUNK_CELLS) as usize;
        let nz = chunk.cells_z.min(CHUNK_CELLS) as usize;
        if self.triangles.is_empty()
            || nx == 0
            || nz == 0
            || chunk.cell_size <= 0.0
            || !chunk.cell_size.is_finite()
            || !chunk.origin.is_finite()
        {
            return GenOutput::empty();
        }
        let grid: Vec<_> = (0..=nz)
            .flat_map(|z| (0..=nx).map(move |x| chunk.surface_vertex(x, z)))
            .collect();
        let vertex = |x, z| {
            let (p, n, _) = grid[z * (nx + 1) + x];
            (p, n)
        };
        let cells = |min: f32, max: f32, axis: usize, n: usize| {
            let index = |p| (((p - chunk.origin[axis]) / chunk.cell_size) as usize).min(n - 1);
            index(min)..=index(max)
        };
        let mut mesh = MeshBuilder::new();
        for triangle in &self.triangles {
            let z_min = triangle[0].y.min(triangle[1].y.min(triangle[2].y));
            let z_max = triangle[0].y.max(triangle[1].y.max(triangle[2].y));
            for z in cells(z_min, z_max, 1, nz) {
                let row = clip(triangle, |p| p.y - vertex(0, z).0.z);
                let row = clip(&row, |p| vertex(0, z + 1).0.z - p.y);
                if row.len() < 3 {
                    continue;
                }
                let (x_min, x_max) = row
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                        (lo.min(p.x), hi.max(p.x))
                    });
                for x in cells(x_min, x_max, 0, nx) {
                    let [a, b, c, d] = [
                        vertex(x, z),
                        vertex(x + 1, z),
                        vertex(x, z + 1),
                        vertex(x + 1, z + 1),
                    ];
                    let patch = clip(&row, |p| p.x - a.0.x);
                    let patch = clip(&patch, |p| b.0.x - p.x);
                    if uses_main_diagonal(a.0.y, b.0.y, c.0.y, d.0.y) {
                        emit_split(
                            &mut mesh,
                            &patch,
                            (a.0.xz(), d.0.xz()),
                            [a, d, c],
                            [a, b, d],
                        );
                    } else {
                        emit_split(
                            &mut mesh,
                            &patch,
                            (b.0.xz(), c.0.xz()),
                            [a, b, c],
                            [b, d, c],
                        );
                    }
                }
            }
        }
        let mesh = mesh.finish();
        if mesh.indices.is_empty() {
            GenOutput::empty()
        } else {
            GenOutput::mesh(mesh)
        }
    }
}

fn clip_rect(points: &[Vec2], lo: Vec2, hi: Vec2) -> Vec<Vec2> {
    let points = clip(points, |p| p.x - lo.x);
    let points = clip(&points, |p| hi.x - p.x);
    let points = clip(&points, |p| p.y - lo.y);
    clip(&points, |p| hi.y - p.y)
}

/// Clip a convex polygon to the nonnegative side of a line.
fn clip(points: &[Vec2], distance: impl Fn(Vec2) -> f32) -> Vec<Vec2> {
    let Some(&last) = points.last() else {
        return Vec::new();
    };
    let (mut a, mut da) = (last, distance(last));
    let mut clipped = Vec::with_capacity(points.len() + 1);
    for &b in points {
        let db = distance(b);
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            let p = a.as_dvec2().lerp(
                b.as_dvec2(),
                f64::from(da) / (f64::from(da) - f64::from(db)),
            );
            clipped.push(p.as_vec2());
        }
        if db >= 0.0 {
            clipped.push(b);
        }
        (a, da) = (b, db);
    }
    clipped
}

fn emit_split(
    mesh: &mut MeshBuilder,
    patch: &[Vec2],
    (a, b): (Vec2, Vec2),
    left: [(Vec3, Vec3); 3],
    right: [(Vec3, Vec3); 3],
) {
    let side = |p| (b - a).perp_dot(p - a);
    emit_patch(mesh, &clip(patch, side), left);
    emit_patch(mesh, &clip(patch, |p| -side(p)), right);
}

fn emit_patch(
    mesh: &mut MeshBuilder,
    patch: &[Vec2],
    [(a, na), (b, nb), (c, nc)]: [(Vec3, Vec3); 3],
) {
    if patch.len() < 3 {
        return;
    }
    let (ab, ac) = ((b - a).xz(), (c - a).xz());
    let area = ab.perp_dot(ac);
    if area <= 0.0 {
        return;
    }
    let indices: Vec<_> = patch
        .iter()
        .map(|&p| {
            let ap = p - a.xz();
            let u = ap.perp_dot(ac) / area;
            let v = ab.perp_dot(ap) / area;
            let height = a.y + (b.y - a.y) * u + (c.y - a.y) * v;
            let normal = (na + (nb - na) * u + (nc - na) * v).normalize_or(Vec3::Y);
            mesh.vertex(
                0,
                Vertex::new(Vec3::new(p.x, height + SURFACE_LIFT, p.y), normal, p),
            )
        })
        .collect();
    for pair in indices[1..].windows(2) {
        mesh.triangle_indices(0, [indices[0], pair[1], pair[0]]);
    }
}
