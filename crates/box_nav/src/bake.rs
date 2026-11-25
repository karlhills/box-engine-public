//! Baking one tile with rerecast: voxelise the triangles, keep what an agent
//! can stand on (slope, step, headroom, distance from walls), and turn the
//! walkable regions into convex polygons with a height-detail mesh.

use glam::{UVec3, Vec3, Vec3A};
use rerecast::{
    AreaType, ConfigBuilder, DetailNavmesh, HeightfieldBuilder, PolygonNavmesh, RegionId, TriMesh,
};

use crate::NavConfig;
use crate::mesh::{NavEdge, NavPoly, NavTile, TileCoord};

/// Why a tile could not be baked.
#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    /// The config has a size that is zero, negative or not a number.
    #[error("navigation settings are invalid: {0}")]
    Config(String),
    /// A step of the Recast pipeline failed.
    #[error("navmesh tile {tile:?}: {step} failed: {message}")]
    Recast {
        tile: TileCoord,
        step: &'static str,
        message: String,
    },
}

fn check(cfg: &NavConfig) -> Result<(), BakeError> {
    let fields = [
        ("agent_radius", cfg.agent_radius),
        ("agent_height", cfg.agent_height),
        ("cell_size", cfg.cell_size),
        ("tile_size", cfg.tile_size),
    ];
    for (name, v) in fields {
        if !(v.is_finite() && v > 0.0) {
            return Err(BakeError::Config(format!("{name} is {v}")));
        }
    }
    if !(cfg.max_slope.is_finite() && cfg.max_slope > 0.0 && cfg.max_slope < 1.55) {
        return Err(BakeError::Config(format!(
            "max_slope is {} rad (0 to 1.55)",
            cfg.max_slope
        )));
    }
    if !(cfg.step_height.is_finite() && cfg.step_height >= 0.0) {
        return Err(BakeError::Config(format!(
            "step_height is {}",
            cfg.step_height
        )));
    }
    Ok(())
}

/// Bakes tile `tile` from world-space triangles (each `[a, b, c]`). Pass the
/// triangles within [`NavConfig::tile_margin`] of the tile's square; others
/// are ignored. Returns `Ok(None)` when nothing in the tile is walkable.
pub fn bake_tile(
    triangles: &[[Vec3; 3]],
    tile: TileCoord,
    cfg: &NavConfig,
) -> Result<Option<NavTile>, BakeError> {
    check(cfg)?;
    if triangles.is_empty() {
        return Ok(None);
    }
    let err = |step: &'static str, e: &dyn std::fmt::Debug| BakeError::Recast {
        tile,
        step,
        message: format!("{e:?}"),
    };
    let cells = cfg.tile_cells();
    let config = ConfigBuilder {
        cell_size_fraction: cfg.agent_radius / cfg.cell_size,
        // Half-cell heights: steps and kerbs resolve finely.
        cell_height_fraction: 2.0 * cfg.agent_radius / cfg.cell_size,
        agent_height: cfg.agent_height,
        agent_radius: cfg.agent_radius,
        walkable_climb: cfg.step_height,
        walkable_slope_angle: cfg.max_slope,
        tile_size: cells,
        tiling: true,
        ..Default::default()
    }
    .build();
    let cs = config.cell_size;
    let border = config.border_size;
    let (rect_min, _) = cfg.tile_rect(tile);
    let pad = border as f32 * cs;
    let field = (cells + 2 * border) as f32 * cs;

    // The triangles near the tile, as one trimesh.
    let mut mesh = TriMesh::default();
    let (lo_x, lo_z) = (rect_min.x - pad, rect_min.y - pad);
    let (hi_x, hi_z) = (lo_x + field, lo_z + field);
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for t in triangles {
        let tmin = t[0].min(t[1]).min(t[2]);
        let tmax = t[0].max(t[1]).max(t[2]);
        if tmax.x < lo_x || tmin.x > hi_x || tmax.z < lo_z || tmin.z > hi_z {
            continue;
        }
        if !t.iter().all(|v| v.is_finite()) {
            continue;
        }
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(t.iter().map(|v| Vec3A::from(*v)));
        mesh.indices.push(UVec3::new(base, base + 1, base + 2));
        mesh.area_types.push(AreaType::NOT_WALKABLE);
        min_y = min_y.min(tmin.y);
        max_y = max_y.max(tmax.y);
    }
    if mesh.indices.is_empty() {
        return Ok(None);
    }
    mesh.mark_walkable_triangles(config.walkable_slope_angle);
    if !mesh.area_types.contains(&AreaType::DEFAULT_WALKABLE) {
        return Ok(None);
    }

    let aabb = rerecast::Aabb3d {
        min: Vec3::new(lo_x, min_y - 1.0, lo_z),
        max: Vec3::new(hi_x, max_y + cfg.agent_height + 1.0, hi_z),
    };
    let mut hf = HeightfieldBuilder {
        aabb,
        cell_size: cs,
        cell_height: config.cell_height,
    }
    .build()
    .map_err(|e| err("heightfield", &e))?;
    hf.rasterize_triangles(&mesh, config.walkable_climb)
        .map_err(|e| err("rasterize", &e))?;
    hf.filter_low_hanging_walkable_obstacles(config.walkable_climb);
    hf.filter_ledge_spans(config.walkable_height, config.walkable_climb);
    hf.filter_walkable_low_height_spans(config.walkable_height);
    let mut chf = hf
        .into_compact(config.walkable_height, config.walkable_climb)
        .map_err(|e| err("compact heightfield", &e))?;
    chf.erode_walkable_area(config.walkable_radius);
    chf.build_distance_field();
    chf.build_regions(border, config.min_region_area, config.merge_region_area)
        .map_err(|e| err("regions", &e))?;
    let contours = chf.build_contours(
        config.max_simplification_error,
        config.max_edge_len,
        config.contour_flags,
    );
    let poly = contours
        .into_polygon_mesh(config.max_vertices_per_polygon)
        .map_err(|e| err("polygons", &e))?;
    if poly.polygon_count() == 0 {
        return Ok(None);
    }
    let detail = DetailNavmesh::new(
        &poly,
        &chf,
        config.detail_sample_dist,
        config.detail_sample_max_error,
    )
    .map_err(|e| err("detail mesh", &e))?;
    Ok(Some(to_tile(tile, &poly, &detail)))
}

/// Converts rerecast's meshes into a [`NavTile`] with world-space vertices.
fn to_tile(coord: TileCoord, poly: &PolygonNavmesh, detail: &DetailNavmesh) -> NavTile {
    let nvp = poly.max_vertices_per_polygon as usize;
    let orig = poly.aabb.min;
    let vertices: Vec<Vec3> = poly
        .vertices
        .iter()
        .map(|v| {
            Vec3::new(
                orig.x + v.x as f32 * poly.cell_size,
                orig.y + v.y as f32 * poly.cell_height,
                orig.z + v.z as f32 * poly.cell_size,
            )
        })
        .collect();
    let border = RegionId::BORDER_REGION.bits();
    let mut polys = Vec::with_capacity(poly.polygon_count());
    let mut detail_vertices = Vec::new();
    let mut detail_triangles = Vec::new();
    for i in 0..poly.polygon_count() {
        if poly.areas[i] == AreaType::NOT_WALKABLE {
            continue;
        }
        let p = &poly.polygons[i * nvp..(i + 1) * nvp];
        let n = &poly.polygon_neighbors[i * nvp..(i + 1) * nvp];
        let mut verts = Vec::with_capacity(nvp);
        let mut edges = Vec::with_capacity(nvp);
        for j in 0..nvp {
            if p[j] == PolygonNavmesh::NO_INDEX {
                break;
            }
            verts.push(p[j]);
            edges.push(match n[j] {
                PolygonNavmesh::NO_CONNECTION => NavEdge::Wall,
                v if v & border != 0 => NavEdge::Portal((v & 0xf) as u8),
                v => NavEdge::Inner(v),
            });
        }
        // Height detail for this polygon, as triangles into the tile's
        // detail vertices.
        let start = detail_triangles.len() as u32;
        if let Some(sub) = detail.meshes.get(i) {
            let base = detail_vertices.len() as u32;
            let vs =
                &detail.vertices[sub.base_vertex_index as usize..][..sub.vertex_count as usize];
            detail_vertices.extend_from_slice(vs);
            let ts = &detail.triangles[sub.base_triangle_index as usize..]
                [..sub.triangle_count as usize];
            for t in ts {
                detail_triangles.push([base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]);
            }
        }
        let end = detail_triangles.len() as u32;
        polys.push(NavPoly::new(verts, edges, start..end, &vertices));
    }
    // Polygons left out (not walkable) shift indices: remap inner links.
    let kept: Vec<usize> = (0..poly.polygon_count())
        .filter(|i| poly.areas[*i] != AreaType::NOT_WALKABLE)
        .collect();
    if kept.len() != poly.polygon_count() {
        let mut remap = vec![u16::MAX; poly.polygon_count()];
        for (new, old) in kept.iter().enumerate() {
            remap[*old] = new as u16;
        }
        for p in &mut polys {
            for e in &mut p.edges {
                if let NavEdge::Inner(i) = e {
                    *e = match remap.get(*i as usize) {
                        Some(&r) if r != u16::MAX => NavEdge::Inner(r),
                        _ => NavEdge::Wall,
                    };
                }
            }
        }
    }
    NavTile::new(coord, vertices, polys, detail_vertices, detail_triangles)
}
