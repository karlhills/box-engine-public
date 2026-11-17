//! Renderers for generated meshes: every entity the generator runtime
//! ([`box_proc::runtime`]) manages gets a [`MeshRenderer`] drawing its
//! generated mesh, with one material per slot.
//!
//! **Materials by slot** (see `box_proc::Procedural`): slot `i` draws
//! `Procedural::materials[i]` when it names an asset (resolved through the
//! asset lookup and server like a `MaterialRef`: the entity's own material
//! while it loads, the placeholder when it cannot be loaded); any other
//! slot draws the entity's own material (`MaterialRef`, else inline
//! `ShaderMaterial`, else inline `StandardMaterial`, else the default). The
//! inline material asset is owned here and freed with the entity.
//!
//! `Primitive` is a generator too, so this module draws primitives as well.
//! Its own `cast_shadows` field is honoured (together with
//! `Procedural::cast_shadows`, which a primitive entity usually lacks).
//!
//! An entity with a generator and a `MeshAsset` keeps the renderer the
//! `MeshAsset` builds; its generated mesh is not drawn (one warning). A
//! `Primitive` wins over a `MeshAsset` instead, as it always has
//! (`asset_glue` skips such entities).
//!
//! Cost when nothing changed: one query over generator entities comparing
//! the inline material, the material references and the renderer with the
//! cached copy. Links are resolved again only when references, the asset
//! lookup's generation or the server's finished imports change.

use std::collections::{HashMap, HashSet};

use box_core::asset_ref::AssetRef;
use box_core::assets::{Assets, Handle};
use box_core::mesh::MeshData;
use box_core::mesh_asset::{MaterialRef, MeshAsset};
use box_core::primitive::Primitive;
use box_core::world::{Entity, World};
use box_proc::{ProcState, Procedural};
use box_render::{MaterialHandle, MeshRenderer, ShaderMaterial, StandardMaterial};

use crate::scene_glue::{
    Link, LinkState, SceneMaterial, add_material, drawn_material, existing_placeholder,
    link_placeholder, link_stamp, material_exists, remove_material, resolve_link, write_material,
};

/// What the glue last built for a generator entity. Runtime only.
#[derive(Debug)]
struct ProcRender {
    mesh: Option<Handle<MeshData>>,
    slot_count: usize,
    cast_shadows: bool,
    material: SceneMaterial,
    /// The asset built from the inline material (owned).
    material_handle: MaterialHandle,
    /// The entity's `MaterialRef`, as last resolved.
    link: Option<Link>,
    /// `Procedural::materials` as last seen, and their resolved links.
    slots: Vec<AssetRef>,
    slot_links: Vec<Option<Link>>,
}

/// Resource: inline material assets owned by the glue, by entity (freed on
/// despawn), and entities already warned about a second mesh source.
#[derive(Default)]
struct OwnedProcMaterials {
    materials: HashMap<Entity, MaterialHandle>,
    warned: HashSet<Entity>,
}

struct Work {
    entity: Entity,
    /// New inline material (`Some` when it changed or on create).
    material: Option<SceneMaterial>,
    /// `Some` when the `MaterialRef` link changed (`Some(None)`: removed).
    link: Option<Option<Link>>,
    /// `Some` when the slot references or their links changed.
    slots: Option<(Vec<AssetRef>, Vec<Option<Link>>)>,
    create: bool,
}

/// The materials a generator entity draws, by slot.
fn drawn_slots(
    owned: MaterialHandle,
    link: Option<&Link>,
    slot_links: &[Option<Link>],
    count: usize,
    placeholder: Option<Handle<StandardMaterial>>,
) -> Vec<MaterialHandle> {
    let base = drawn_material(owned, link, placeholder);
    (0..count.max(1))
        .map(|i| match slot_links.get(i).and_then(Option::as_ref) {
            Some(l) => drawn_material(base, Some(l), placeholder),
            None => base,
        })
        .collect()
}

/// `Procedural::cast_shadows` and a `Primitive`'s own flag (both default on).
pub(crate) fn casts_shadows(
    procedural: Option<&Procedural>,
    primitive: Option<&Primitive>,
) -> bool {
    procedural.is_none_or(|p| p.cast_shadows) && primitive.is_none_or(|p| p.cast_shadows)
}

fn needs_placeholder(link: Option<&Link>, slot_links: &[Option<Link>]) -> bool {
    link.into_iter()
        .chain(slot_links.iter().flatten())
        .any(|l| l.state == LinkState::Missing)
}

fn resolve_slots(
    world: &World,
    refs: &[AssetRef],
    stamp: (u64, u64),
    old: &[Option<Link>],
) -> Vec<Option<Link>> {
    refs.iter()
        .enumerate()
        .map(|(i, r)| {
            r.is_some().then(|| {
                let previous = old.get(i).and_then(Option::as_ref);
                resolve_link(world, r, stamp, previous)
            })
        })
        .collect()
}

/// Keep the renderer of every generator entity in step with its generated
/// mesh and materials (see the [module docs](self)). Called by
/// [`sync_scene_meshes`](crate::sync_scene_meshes) right after the
/// generator runtime's flush.
pub fn sync_procedural_renderers(world: &mut World) {
    let (work, conflicts) = collect_work(world);
    let orphans = collect_orphans(world);
    if work.is_empty() && orphans.is_empty() && conflicts.is_empty() {
        return;
    }
    world.resource_or_default::<Assets<StandardMaterial>>();
    world.resource_or_default::<Assets<ShaderMaterial>>();
    world.resource_or_default::<OwnedProcMaterials>();
    for e in conflicts {
        if world.resource_mut::<OwnedProcMaterials>().warned.insert(e) {
            let who = world
                .get::<box_core::identity::Name>(e)
                .map_or_else(|| format!("{e:?}"), |n| n.0.clone());
            tracing::warn!(
                "`{who}` has a generator and also a MeshAsset; its generated mesh is not drawn (remove one of them)"
            );
        }
    }
    for item in work {
        apply(world, item);
    }
    for (entity, render) in orphans {
        release(world, entity, render);
    }
}

type Row<'w> = (
    Entity,
    &'w ProcState,
    Option<&'w Procedural>,
    Option<&'w StandardMaterial>,
    Option<&'w ShaderMaterial>,
    Option<&'w MaterialRef>,
    Option<&'w ProcRender>,
    Option<&'w MeshRenderer>,
    Option<&'w Primitive>,
    Option<&'w MeshAsset>,
);

fn collect_work(world: &World) -> (Vec<Work>, Vec<Entity>) {
    let mut work = Vec::new();
    let mut conflicts = Vec::new();
    let mut stamp = None;
    let placeholder = existing_placeholder(world);
    let mut query = world.query::<Row<'_>>();
    for row in query.iter() {
        let (entity, state, procedural, standard, shader, linked, cached, renderer, prim, asset) =
            row;
        if asset.is_some() && prim.is_none() {
            if cached.is_none() {
                conflicts.push(entity);
            }
            continue;
        }
        let source = linked.map(|m| &m.material).filter(|r| r.is_some());
        let refs: &[AssetRef] = procedural.map_or(&[], |p| p.materials.as_slice());
        let any_slot_ref = refs.iter().any(AssetRef::is_some);
        let Some(cached) = cached else {
            let s = (source.is_some() || any_slot_ref)
                .then(|| *stamp.get_or_insert_with(|| link_stamp(world)));
            let link = source.zip(s).map(|(r, s)| resolve_link(world, r, s, None));
            let slot_links = s.map_or_else(Vec::new, |s| resolve_slots(world, refs, s, &[]));
            work.push(Work {
                entity,
                material: Some(SceneMaterial::of(standard, shader)),
                link: Some(link),
                slots: Some((refs.to_vec(), slot_links)),
                create: true,
            });
            continue;
        };
        let link = match (source, &cached.link) {
            (None, None) => None,
            (None, Some(_)) => Some(None),
            (Some(r), old) => {
                let s = *stamp.get_or_insert_with(|| link_stamp(world));
                match old {
                    Some(l) if l.source == *r && l.stamp == s => None,
                    old => Some(Some(resolve_link(world, r, s, old.as_ref()))),
                }
            }
        };
        let slots = if cached.slots != refs {
            let s = stamp.get_or_insert_with(|| link_stamp(world));
            Some((
                refs.to_vec(),
                resolve_slots(world, refs, *s, &cached.slot_links),
            ))
        } else if any_slot_ref {
            let s = *stamp.get_or_insert_with(|| link_stamp(world));
            let stale = cached.slot_links.iter().flatten().any(|l| l.stamp != s);
            stale.then(|| {
                (
                    refs.to_vec(),
                    resolve_slots(world, refs, s, &cached.slot_links),
                )
            })
        } else {
            None
        };
        let material_changed = !cached.material.matches(standard, shader);
        let cast_shadows = casts_shadows(procedural, prim);
        let current_link = match &link {
            Some(l) => l.as_ref(),
            None => cached.link.as_ref(),
        };
        let current_slots = slots.as_ref().map_or(&cached.slot_links, |(_, l)| l);
        let expected = drawn_slots(
            cached.material_handle,
            current_link,
            current_slots,
            state.slot_count(),
            placeholder,
        );
        let renderer_ok = match state.mesh() {
            Some(mesh) => renderer.is_some_and(|r| {
                r.mesh == mesh && r.materials == expected && r.cast_shadows == cast_shadows
            }),
            None => renderer.is_none_or(|r| Some(r.mesh) != cached.mesh),
        };
        if material_changed
            || link.is_some()
            || slots.is_some()
            || !renderer_ok
            || cached.mesh != state.mesh()
            || cached.cast_shadows != cast_shadows
            || cached.slot_count != state.slot_count()
        {
            work.push(Work {
                entity,
                material: material_changed.then(|| SceneMaterial::of(standard, shader)),
                link,
                slots,
                create: false,
            });
        }
    }
    (work, conflicts)
}

fn collect_orphans(world: &World) -> Vec<(Entity, Option<ProcRenderParts>)> {
    let mut out: Vec<(Entity, Option<ProcRenderParts>)> = world
        .query::<box_core::world::Without<(Entity, &ProcRender), &ProcState>>()
        .iter()
        .map(|(e, r)| (e, Some((r.mesh, r.material_handle))))
        .collect();
    if let Some(owned) = world.get_resource::<OwnedProcMaterials>() {
        let live = world.query::<&ProcRender>().iter().len();
        if owned.materials.len() > live {
            out.extend(
                owned
                    .materials
                    .keys()
                    .filter(|e| !world.contains(**e))
                    .map(|e| (*e, None)),
            );
        }
    }
    out
}

/// `(mesh drawn, owned inline material)` of a released entity.
type ProcRenderParts = (Option<Handle<MeshData>>, MaterialHandle);

fn release(world: &mut World, entity: Entity, parts: Option<ProcRenderParts>) {
    let owned = world.resource_mut::<OwnedProcMaterials>();
    owned.warned.remove(&entity);
    let material = owned.materials.remove(&entity);
    if let Some((mesh, _)) = parts
        && world.contains(entity)
    {
        let _ = world.remove_one::<ProcRender>(entity);
        let ours = world
            .get::<MeshRenderer>(entity)
            .is_some_and(|r| Some(r.mesh) == mesh);
        if ours {
            let _ = world.remove_one::<MeshRenderer>(entity);
        }
    }
    if let Some(m) = material.or(parts.map(|p| p.1)) {
        remove_material(world, m);
    }
}

fn apply(world: &mut World, item: Work) {
    let Work {
        entity,
        material,
        link,
        slots,
        create,
    } = item;
    let Some((mesh, slot_count)) = world
        .get::<ProcState>(entity)
        .map(|s| (s.mesh(), s.slot_count()))
    else {
        return;
    };
    let cast_shadows = casts_shadows(
        world.get::<Procedural>(entity).as_deref(),
        world.get::<Primitive>(entity).as_deref(),
    );
    let mut render = if create {
        let material = material.clone().unwrap_or(SceneMaterial::Standard(None));
        let handle = add_material(world, &material);
        let previous = world
            .resource_mut::<OwnedProcMaterials>()
            .materials
            .insert(entity, handle);
        if let Some(old) = previous {
            remove_material(world, old);
        }
        ProcRender {
            mesh: None,
            slot_count,
            cast_shadows,
            material,
            material_handle: handle,
            link: None,
            slots: Vec::new(),
            slot_links: Vec::new(),
        }
    } else {
        let Ok(r) = world.remove_one::<ProcRender>(entity) else {
            return;
        };
        r
    };
    if !create {
        if !material_exists(world, render.material_handle) {
            tracing::warn!(
                ?entity,
                "generator material asset was removed; rebuilding it"
            );
            let m = material.clone().unwrap_or_else(|| render.material.clone());
            render.material_handle = add_material(world, &m);
            render.material = m;
        } else if let Some(m) = material {
            render.material_handle = write_material(world, render.material_handle, &m);
            render.material = m;
        }
        world
            .resource_mut::<OwnedProcMaterials>()
            .materials
            .insert(entity, render.material_handle);
    }
    if let Some(l) = link {
        render.link = l;
    }
    if let Some((refs, links)) = slots {
        render.slots = refs;
        render.slot_links = links;
    }
    let placeholder = if needs_placeholder(render.link.as_ref(), &render.slot_links) {
        Some(link_placeholder(world))
    } else {
        existing_placeholder(world)
    };
    let materials = drawn_slots(
        render.material_handle,
        render.link.as_ref(),
        &render.slot_links,
        slot_count,
        placeholder,
    );
    let old_mesh = render.mesh;
    match mesh {
        Some(m) => {
            let renderer = MeshRenderer {
                mesh: m,
                materials,
                cast_shadows,
            };
            let _ = world.insert_one(entity, renderer);
        }
        None => {
            let ours = world
                .get::<MeshRenderer>(entity)
                .is_some_and(|r| old_mesh.is_some() && Some(r.mesh) == old_mesh);
            if ours {
                let _ = world.remove_one::<MeshRenderer>(entity);
            }
        }
    }
    render.mesh = mesh;
    render.slot_count = slot_count;
    render.cast_shadows = cast_shadows;
    let _ = world.insert_one(entity, render);
}
