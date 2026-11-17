//! Mesh checks shared by the box_arch tests.

#![allow(dead_code)]

use std::collections::HashMap;

use box_core::mesh::MeshData;
use box_proc::generator::GenOutput;
use box_proc::mesh_builder::inward_triangles;
use glam::Vec3;

/// Finite positions, normals, UVs and colours; valid indices and groups;
/// windings that agree with the normals; no triangle the builder had to
/// drop.
pub fn check_output(out: &GenOutput) -> MeshData {
    for w in &out.warnings {
        assert!(
            !w.contains("degenerate") && !w.contains("non-finite") && !w.contains("clamped slots"),
            "builder dropped geometry: {w}"
        );
    }
    let m = out.mesh.clone().unwrap_or_default();
    check_mesh(&m);
    m
}

pub fn check_mesh(m: &MeshData) {
    m.validate().expect("valid mesh");
    assert!(
        m.positions.iter().all(|p| p.is_finite()),
        "finite positions"
    );
    assert!(
        m.normals
            .iter()
            .all(|n| n.is_finite() && (n.length() - 1.0).abs() < 1e-3),
        "unit normals"
    );
    assert!(
        m.colors
            .iter()
            .flatten()
            .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
    );
    for set in &m.uvs {
        assert!(set.iter().all(|uv| uv.is_finite()));
    }
    assert_eq!(
        inward_triangles(m),
        0,
        "triangles wound against their normals"
    );
}

/// The triangles of `m` in the given material slots, as one mesh.
pub fn slots(m: &MeshData, keep: &[u32]) -> MeshData {
    let mut out = MeshData {
        positions: m.positions.clone(),
        normals: m.normals.clone(),
        ..MeshData::default()
    };
    for g in &m.groups {
        if keep.contains(&g.material_slot) {
            let r = g.first_index as usize..(g.first_index + g.index_count) as usize;
            out.indices.extend_from_slice(&m.indices[r]);
        }
    }
    out
}

fn point_segment(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-20)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Open edges that are not one side of a crack narrower than `gap`: an
/// edge whose endpoints both lie within `gap` of another open edge (a
/// sub-millimetre T-junction sliver) or that is itself shorter than `gap`
/// does not count.
pub fn wide_open_edges(edges: &[(Vec3, Vec3)], gap: f32) -> Vec<(Vec3, Vec3)> {
    edges
        .iter()
        .enumerate()
        .filter(|(i, (a, b))| {
            if a.distance(*b) < gap {
                return false;
            }
            !edges.iter().enumerate().any(|(j, (c, d))| {
                j != *i && point_segment(*a, *c, *d) < gap && point_segment(*b, *c, *d) < gap
            })
        })
        .map(|(_, e)| *e)
        .collect()
}

/// Boundary edges (used by one triangle) after merging positions within
/// `tol`, as segment endpoints.
pub fn boundary_edges(m: &MeshData, tol: f32) -> Vec<(Vec3, Vec3)> {
    // Points within `tol` of an earlier point merge with it (neighbouring
    // grid cells are searched, so values straddling a cell boundary merge).
    let q = |p: Vec3| p.to_array().map(|x| (x / tol).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut rep: Vec<Vec3> = Vec::new();
    let pid: Vec<u32> = m
        .positions
        .iter()
        .map(|p| {
            let c = q(*p);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if let Some(ids) = grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]) {
                            for &id in ids {
                                if rep[id as usize].distance(*p) <= tol {
                                    return id;
                                }
                            }
                        }
                    }
                }
            }
            rep.push(*p);
            let id = rep.len() as u32 - 1;
            grid.entry(c).or_default().push(id);
            id
        })
        .collect();
    let mut count: HashMap<(u32, u32), i32> = HashMap::new();
    for t in m.indices.as_chunks::<3>().0 {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| pid[i as usize]);
        if a == b || b == c || a == c {
            continue;
        }
        for (x, y) in [(a, b), (b, c), (c, a)] {
            *count.entry((x.min(y), x.max(y))).or_insert(0) += 1;
        }
    }
    let mut out: Vec<(Vec3, Vec3)> = count
        .into_iter()
        .filter(|(_, n)| *n == 1)
        .map(|((a, b), _)| (rep[a as usize], rep[b as usize]))
        .collect();
    out.sort_by(|a, b| a.0.to_array().partial_cmp(&b.0.to_array()).unwrap());
    out
}
