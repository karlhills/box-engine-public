//! Scatter placement (`box_proc::scatter`): determinism, chunk locality of
//! paint strokes, slope and height limits, the minimum distance across
//! chunk borders, density, area and mask semantics, and the saved form.

use box_core::pmath;
use std::collections::HashMap;

use box_core::asset_ref::AssetRef;
use box_core::mesh::MeshData;
use box_core::reflect::Registry;
use box_proc::scatter::{
    ChunkCoord, Scatter, ScatterInstance, ScatterPlan, ScatterSpecies, ScatterSurface,
    instances_hash, place_all,
};
use glam::{Affine3A, Vec2, Vec3};

fn species() -> Vec<ScatterSpecies> {
    vec![
        ScatterSpecies {
            weight: 3.0,
            tint_jitter: 0.2,
            ..ScatterSpecies::mesh(AssetRef::path("a.glb#Mesh/A"), Vec::new())
        },
        ScatterSpecies {
            align_to_normal: 1.0,
            ..ScatterSpecies::mesh(AssetRef::path("b.glb#Mesh/B"), Vec::new())
        },
    ]
}

fn square(size: f32) -> Vec<Vec2> {
    let h = size * 0.5;
    vec![
        Vec2::new(-h, -h),
        Vec2::new(h, -h),
        Vec2::new(h, h),
        Vec2::new(-h, h),
    ]
}

fn meadow() -> Scatter {
    Scatter {
        species: species(),
        density: 1.5,
        min_distance: 0.4,
        max_slope: 1.571,
        area: square(40.0),
        seed: 9,
        ..Scatter::default()
    }
}

fn all(s: &Scatter, surface: &ScatterSurface) -> Vec<ScatterInstance> {
    place_all(s, surface)
        .into_iter()
        .flat_map(|(_, v)| v)
        .collect()
}

/// A height grid with a ramp (x > 4: rising at 45°) and a plateau.
fn ramp_mesh() -> MeshData {
    let n = 40;
    let size = 40.0;
    let h = |x: f32| if x > 4.0 { (x - 4.0).min(8.0) } else { 0.0 };
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let x = -size / 2.0 + i as f32 * size / n as f32;
            let z = -size / 2.0 + j as f32 * size / n as f32;
            positions.push(Vec3::new(x, h(x), z));
        }
    }
    let mut indices = Vec::new();
    let row = n + 1;
    for j in 0..n {
        for i in 0..n {
            let a = (j * row + i) as u32;
            let b = a + 1;
            let c = a + row as u32;
            let d = c + 1;
            // Facing up (+Y): a, c, b and b, c, d.
            indices.extend([a, c, b, b, c, d]);
        }
    }
    MeshData {
        positions,
        indices,
        ..MeshData::default()
    }
}

#[test]
fn the_same_parameters_give_the_same_instances() {
    let s = meadow();
    let a = all(&s, &ScatterSurface::Flat);
    let b = all(&s.clone(), &ScatterSurface::Flat);
    assert!(a.len() > 1000, "{}", a.len());
    assert_eq!(instances_hash(&a), instances_hash(&b));
    // Chunk by chunk too: the same keys and the same instances.
    let plan = ScatterPlan::new(&s);
    for c in plan.chunks() {
        let again = ScatterPlan::new(&s);
        assert_eq!(
            plan.chunk_key(c, &ScatterSurface::Flat),
            again.chunk_key(c, &ScatterSurface::Flat)
        );
        assert_eq!(
            plan.place(c, &ScatterSurface::Flat),
            again.place(c, &ScatterSurface::Flat)
        );
    }
    // Another seed: another arrangement.
    let other = all(
        &Scatter {
            seed: 10,
            ..s.clone()
        },
        &ScatterSurface::Flat,
    );
    assert_ne!(instances_hash(&a), instances_hash(&other));
    // Every instance stands on the plane, inside the area, with a valid
    // species and tint.
    for x in &a {
        let p = x.transform.translation;
        assert_eq!(p.y, 0.0);
        assert!(p.x.abs() <= 20.0 && p.z.abs() <= 20.0, "{p}");
        assert!((x.species as usize) < s.species.len());
        if x.species == 1 {
            assert_eq!(x.tint, 0, "species 1 has no tint jitter");
        }
    }
    // Ids are unique.
    let mut ids: Vec<u64> = a.iter().map(|x| x.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), a.len());
}

#[test]
fn the_minimum_distance_holds_across_chunk_borders() {
    let s = Scatter {
        min_distance: 0.9,
        density: 0.6,
        ..meadow()
    };
    let a = all(&s, &ScatterSurface::Flat);
    assert!(a.len() > 300);
    // A grid of cells of the minimum distance: neighbours only.
    let r = s.min_distance;
    let mut grid: HashMap<(i32, i32), Vec<Vec2>> = HashMap::new();
    for x in &a {
        let p = Vec2::new(x.transform.translation.x, x.transform.translation.z);
        grid.entry(((p.x / r).floor() as i32, (p.y / r).floor() as i32))
            .or_default()
            .push(p);
    }
    let mut closest = f32::INFINITY;
    for (&(i, j), points) in &grid {
        for p in points {
            for dj in -1..=1 {
                for di in -1..=1 {
                    for q in grid.get(&(i + di, j + dj)).into_iter().flatten() {
                        if p != q {
                            closest = closest.min(p.distance(*q));
                        }
                    }
                }
            }
        }
    }
    assert!(closest >= r, "two instances {closest} m apart, minimum {r}");
    // And the chunk grid really was crossed: instances in many chunks.
    assert!(ScatterPlan::new(&s).chunk_count() >= 25);
}

#[test]
fn density_is_close_to_the_target() {
    for (density, r) in [(0.5, 0.5), (1.5, 0.4), (3.0, 0.0), (0.2, 1.2)] {
        let s = Scatter {
            density,
            min_distance: r,
            ..meadow()
        };
        let n = all(&s, &ScatterSurface::Flat).len() as f32;
        let measured = n / (40.0 * 40.0);
        assert!(
            (measured - density).abs() / density < 0.15,
            "density {density}, min distance {r}: measured {measured}"
        );
        assert!(ScatterPlan::new(&s).warnings().is_empty());
    }
    // Asking for more than the minimum distance allows warns.
    let s = Scatter {
        density: 5.0,
        min_distance: 0.6,
        ..meadow()
    };
    assert!(
        ScatterPlan::new(&s)
            .warnings()
            .iter()
            .any(|w| w.contains("min distance")),
    );
}

#[test]
fn slope_and_height_limits_are_respected() {
    let mesh = ramp_mesh();
    let s = Scatter {
        max_slope: 0.5,
        ..meadow()
    };
    let surface = ScatterSurface::from_mesh(&mesh, Affine3A::IDENTITY, s.mask.chunk_size());
    let a = all(&s, &surface);
    let flat = a.iter().filter(|x| x.transform.translation.x < 4.0).count();
    let ramp = a
        .iter()
        .filter(|x| (4.2..11.8).contains(&x.transform.translation.x))
        .count();
    let plateau = a
        .iter()
        .filter(|x| x.transform.translation.x > 12.2)
        .count();
    assert!(
        flat > 500 && plateau > 100,
        "flat {flat}, plateau {plateau}"
    );
    assert_eq!(ramp, 0, "the 45° ramp is steeper than the limit");
    for x in &a {
        let p = x.transform.translation;
        let hit = surface.sample(Vec2::new(p.x, p.z)).unwrap();
        assert!((hit.height - p.y).abs() < 1e-4, "on the ground");
        assert!(hit.normal.y >= pmath::cos(0.5f32) - 1e-5);
    }
    // No limit: the ramp is covered, and species 1 leans with it.
    let s2 = Scatter {
        max_slope: 1.571,
        ..s.clone()
    };
    let b = all(&s2, &surface);
    let on_ramp: Vec<&ScatterInstance> = b
        .iter()
        .filter(|x| (4.5..11.5).contains(&x.transform.translation.x))
        .collect();
    assert!(on_ramp.len() > 100);
    for x in on_ramp.iter().filter(|x| x.species == 1) {
        let up = x.transform.rotation * Vec3::Y;
        assert!(up.x < -0.6, "aligned to the ramp's normal: {up}");
    }
    // Height limits: only the plateau (8 m up).
    let s3 = Scatter {
        min_height: 7.5,
        max_height: 8.5,
        ..s2
    };
    let c = all(&s3, &surface);
    assert!(c.len() > 100);
    assert!(
        c.iter()
            .all(|x| (7.5..=8.5).contains(&x.transform.translation.y))
    );
}

#[test]
fn a_paint_stroke_changes_only_the_chunks_it_touches() {
    let mut s = Scatter {
        area: Vec::new(),
        ..meadow()
    };
    // A painted region of several chunks.
    for k in 0..=20 {
        let p = Vec2::new(-12.0 + k as f32, 3.0);
        s.mask.paint(&[], p, 3.0, 1.0, false);
    }
    let before = place_all(&s, &ScatterSurface::Flat);
    let plan = ScatterPlan::new(&s);
    let keys: HashMap<ChunkCoord, u64> = plan
        .chunks()
        .map(|c| (c, plan.chunk_key(c, &ScatterSurface::Flat).unwrap()))
        .collect();
    assert!(keys.len() >= 4, "{}", keys.len());

    // One dab in the middle of chunk (0, 0).
    let mut after = s.clone();
    let touched = after.mask.paint(&[], Vec2::new(4.0, 6.0), 1.0, 1.0, true);
    assert_eq!(touched, vec![(0, 0)]);
    let plan2 = ScatterPlan::new(&after);
    let mut changed = Vec::new();
    for c in plan2.chunks() {
        if keys.get(&c) != plan2.chunk_key(c, &ScatterSurface::Flat).as_ref() {
            changed.push(c);
        }
    }
    assert_eq!(changed, touched, "only the touched chunk's key changed");
    let after_all = place_all(&after, &ScatterSurface::Flat);
    for ((c, a), (c2, b)) in before.iter().zip(&after_all) {
        assert_eq!(c, c2);
        if *c == (0, 0) {
            assert!(b.len() < a.len(), "erasing removed instances");
            // Erasing only removes: what is left is unchanged.
            assert!(b.iter().all(|x| a.contains(x)));
        } else {
            assert_eq!(a, b, "chunk {c:?} is untouched");
        }
    }
    // Painting more only adds: the old instances stay where they were.
    let mut more = s.clone();
    more.mask.paint(&[], Vec2::new(-6.0, 3.0), 3.0, 1.0, false);
    let grown = all(&more, &ScatterSurface::Flat);
    for x in before.iter().flat_map(|(_, v)| v) {
        assert!(grown.contains(x));
    }
}

#[test]
fn a_painted_tile_overrides_the_area_and_erasing_clears() {
    let s = meadow();
    let n = all(&s, &ScatterSurface::Flat).len();
    // Erase a hole in the area's middle and paint outside it.
    let mut edited = s.clone();
    for _ in 0..10 {
        edited
            .mask
            .paint(&s.area, Vec2::new(0.0, 0.0), 4.0, 1.0, true);
        edited
            .mask
            .paint(&s.area, Vec2::new(26.0, 0.0), 3.0, 1.0, false);
    }
    let a = all(&edited, &ScatterSurface::Flat);
    let near = |x: &ScatterInstance, c: Vec2, r: f32| {
        Vec2::new(x.transform.translation.x, x.transform.translation.z).distance(c) < r
    };
    assert!(!a.iter().any(|x| near(x, Vec2::ZERO, 2.0)), "a hole");
    assert!(
        a.iter()
            .filter(|x| near(x, Vec2::new(26.0, 0.0), 2.5))
            .count()
            > 10
    );
    assert!(
        a.iter()
            .filter(|x| near(x, Vec2::new(-10.0, 0.0), 3.0))
            .count()
            > 10
    );
    assert!(a.len() < n + 200);
    // Painting back what the area gives drops the tiles again.
    let mut back = edited.clone();
    for _ in 0..30 {
        back.mask
            .paint(&s.area, Vec2::new(0.0, 0.0), 6.0, 1.0, false);
        back.mask
            .paint(&s.area, Vec2::new(26.0, 0.0), 5.0, 1.0, true);
    }
    assert!(back.mask.tiles.is_empty(), "{:?}", back.mask.tiles.len());
    assert_eq!(
        instances_hash(&all(&back, &ScatterSurface::Flat)),
        instances_hash(&all(&s, &ScatterSurface::Flat))
    );
}

#[test]
fn a_ground_edit_changes_only_the_chunks_under_it() {
    let s = meadow();
    let mesh = ramp_mesh();
    let size = s.mask.chunk_size();
    let a = ScatterSurface::from_mesh(&mesh, Affine3A::IDENTITY, size);
    // Raise one vertex near (-16, -16).
    let mut bumped = mesh.clone();
    let i = bumped
        .positions
        .iter()
        .position(|p| p.x == -16.0 && p.z == -16.0)
        .unwrap();
    bumped.positions[i].y += 0.5;
    let b = ScatterSurface::from_mesh(&bumped, Affine3A::IDENTITY, size);
    let plan = ScatterPlan::new(&s);
    let changed: Vec<ChunkCoord> = plan
        .chunks()
        .filter(|&c| plan.chunk_key(c, &a) != plan.chunk_key(c, &b))
        .collect();
    assert!(!changed.is_empty() && changed.len() <= 4, "{changed:?}");
    assert!(
        changed.iter().all(|c| c.0 <= -2 && c.1 <= -2),
        "{changed:?}"
    );
    // Moving the ground changes every chunk it covers.
    let moved = ScatterSurface::from_mesh(&mesh, Affine3A::from_translation(Vec3::Y), size);
    assert!(
        plan.chunks()
            .all(|c| plan.chunk_key(c, &a) != plan.chunk_key(c, &moved))
    );
}

#[test]
fn bad_values_warn_and_never_panic() {
    let s = Scatter {
        density: f32::NAN,
        min_distance: f32::INFINITY,
        max_slope: -3.0,
        area: vec![Vec2::ZERO, Vec2::X],
        species: vec![ScatterSpecies {
            scale: Vec2::new(-1.0, f32::NAN),
            weight: f32::NAN,
            ..ScatterSpecies::default()
        }],
        ..Scatter::default()
    };
    let plan = ScatterPlan::new(&s);
    assert!(plan.warnings().len() >= 3, "{:?}", plan.warnings());
    assert_eq!(plan.chunk_count(), 0);
    let mut m = Scatter::default().mask;
    assert!(
        m.paint(&[], Vec2::new(f32::NAN, 0.0), 1.0, 1.0, false)
            .is_empty()
    );
    assert!(m.paint(&[], Vec2::ZERO, -1.0, 1.0, false).is_empty());
    // Damaged tiles count as unpainted.
    let mut damaged = meadow();
    damaged
        .mask
        .paint(&[], Vec2::new(30.0, 0.0), 2.0, 1.0, false);
    damaged.mask.tiles[0].cells.truncate(10);
    let plan = ScatterPlan::new(&damaged);
    assert!(plan.warnings().iter().any(|w| w.contains("damaged")));
}

#[test]
fn the_saved_form_is_the_intent_only() {
    let mut registry = Registry::with_core_types();
    box_proc::register_proc_types(&mut registry);
    let mut s = meadow();
    s.mask.paint(&[], Vec2::new(30.0, 0.0), 2.0, 1.0, false);
    let json = registry.value_to_json(&s).unwrap();
    let mut keys: Vec<&String> = json.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "area",
            "density",
            "mask",
            "max_height",
            "max_slope",
            "min_distance",
            "min_height",
            "seed",
            "species",
            "surface"
        ]
    );
    let tiles = json["mask"]["tiles"].as_array().unwrap();
    assert!(!tiles.is_empty());
    assert_eq!(tiles[0]["cells"].as_str().unwrap().len(), 512);
    let back = registry
        .value_from_json(std::any::TypeId::of::<Scatter>(), &json)
        .unwrap();
    let back = <Scatter as bevy_reflect::FromReflect>::from_reflect(back.as_ref()).unwrap();
    assert_eq!(back, s);
}
