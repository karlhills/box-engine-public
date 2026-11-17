//! The inspector's field model: rows built by walking a reflected component
//! or resource value with the registry's field metadata. No UI here, so the
//! model is tested headlessly; `panes::inspector` draws it.

use box_core::asset_ref::AssetRef;
use box_core::color::Color;
use box_core::identity::{Name, Uid};
use box_core::prelude::{Entity, World};
use box_core::reflect::bevy_reflect::enums::{DynamicEnum, DynamicVariant, VariantInfo};
use box_core::reflect::bevy_reflect::structs::DynamicStruct;
use box_core::reflect::bevy_reflect::tuple::DynamicTuple;
use box_core::reflect::{
    FieldMeta, PartialReflect, ReflectDefault, ReflectRef, Registry, TypeInfo, Value, clone_value,
};
use glam::{EulerRot, IVec2, IVec3, Quat, UVec2, UVec3, Vec2, Vec3, Vec4};

/// How a row is edited.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    F32,
    F64,
    /// Any primitive integer; `signed` and the bit width pick the clamp range.
    Int {
        signed: bool,
        bits: u32,
    },
    Bool,
    String,
    Vec2,
    Vec3,
    Vec4,
    UVec2,
    UVec3,
    IVec2,
    IVec3,
    /// Shown as Euler angles in degrees.
    Quat,
    Color,
    /// A combo of the variants; the current variant's fields follow as child rows.
    Enum {
        variants: Vec<&'static str>,
        current: String,
    },
    /// A heading; its fields follow as child rows.
    Struct,
    /// An asset reference (`AssetRef`), edited with the asset picker.
    AssetRef,
    /// A list of asset references (material slots); its items follow as
    /// [`FieldKind::AssetRef`] child rows (`path[i]`).
    AssetList {
        len: usize,
    },
    /// Any other list (a spline's points): its items follow as child rows
    /// (`path[i]`, headings for structs); items are added (a copy of the
    /// last) and removed (the last) as a whole-list edit.
    List {
        len: usize,
    },
    /// Shown read-only (maps, arrays, opaque types).
    ReadOnly(String),
}

/// Which parts of a row differ across a multi-selection ([`shared_sections`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mixed {
    /// Every selected entity has the same value (or one entity is selected).
    #[default]
    Same,
    /// The values differ.
    Whole,
    /// For vectors and rotations: a bit per differing lane (X = bit 0).
    Lanes(u8),
}

impl Mixed {
    pub fn any(self) -> bool {
        self != Self::Same
    }

    /// Lane `i` differs (any lane of a non-vector value that differs).
    pub fn lane(self, i: usize) -> bool {
        match self {
            Self::Same => false,
            Self::Whole => true,
            Self::Lanes(mask) => mask & (1 << i) != 0,
        }
    }

    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Same, m) | (m, Self::Same) => m,
            (Self::Lanes(a), Self::Lanes(b)) => Self::Lanes(a | b),
            _ => Self::Whole,
        }
    }
}

/// One inspector row.
#[derive(Debug)]
pub struct FieldRow {
    /// Full field path, e.g. `"Primitive.shape.radius"`.
    pub path: String,
    pub label: String,
    pub depth: usize,
    pub kind: FieldKind,
    pub meta: Option<FieldMeta>,
    /// The current value (an owned copy); the primary selection's in a
    /// multi-selection.
    pub value: Value,
    /// Whether the selected entities disagree about this value.
    pub mixed: Mixed,
}

/// The Euler angles a rotation field shows, remembered with the quaternion
/// they produced. Quaternion to Euler is not stable (X past 90 degrees comes
/// back as X below 90 with Y and Z turned by 180), so while the rotation is
/// the one last set from these angles the angles themselves are shown; they
/// are recomputed only when the rotation changed from outside (gizmo, undo,
/// another entity).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EulerEdit {
    pub rotation: Quat,
    /// Degrees about X, Y and Z (applied in YXZ order).
    pub degrees: [f32; 3],
}

impl EulerEdit {
    /// The rotation for typed or dragged angles.
    pub fn from_degrees(degrees: [f32; 3]) -> Self {
        let rotation = Quat::from_euler(
            EulerRot::YXZ,
            degrees[1].to_radians(),
            degrees[0].to_radians(),
            degrees[2].to_radians(),
        )
        .normalize();
        Self { rotation, degrees }
    }

    /// The angles to show for `rotation`: the remembered ones while they
    /// still describe it, otherwise a fresh decomposition.
    pub fn degrees_for(remembered: Option<Self>, rotation: Quat) -> [f32; 3] {
        if let Some(r) = remembered
            && (r.rotation.abs_diff_eq(rotation, 1e-6) || r.rotation.abs_diff_eq(-rotation, 1e-6))
        {
            return r.degrees;
        }
        let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
        // Round away float noise (and "-0.0") for display.
        let clean = |a: f32| {
            let d = (a.to_degrees() * 1000.0).round() / 1000.0;
            if d == 0.0 { 0.0 } else { d }
        };
        [clean(x), clean(y), clean(z)]
    }
}

/// The readable name of a field path for undo labels:
/// `Transform.translation` -> `Translation`,
/// `DirectionalLight.shadow.depth_bias` -> `Shadow depth bias`,
/// `MeshAsset.materials[0]` -> `Materials [0]`; a whole component is its
/// name.
pub fn field_label(path: &str) -> String {
    let Some((component, rest)) = path.split_once('.') else {
        return path.to_owned();
    };
    if rest.is_empty() {
        return component.to_owned();
    }
    let words = rest.replace('[', " [").replace(['.', '_'], " ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => component.to_owned(),
    }
}

/// The undo label of an inspector edit of `path` on `subject` (an entity's
/// name, "3 entities", or `None` for a scene resource): `Cube: Translation`,
/// `Rename Cube`, `Environment: Exposure`. Every inspector label comes from
/// here.
pub fn edit_label(subject: Option<&str>, path: &str) -> String {
    match subject {
        Some(s) if path == "Name" || path.starts_with("Name.") => format!("Rename {s}"),
        Some(s) => format!("{s}: {}", field_label(path)),
        None => match path.split_once('.') {
            Some((resource, _)) => format!("{resource}: {}", field_label(path)),
            None => format!("Set {path}"),
        },
    }
}

/// The lanes of a vector value as `f32`s (rotations as Euler degrees, as
/// the inspector shows them); `None` for other values.
pub fn lanes_of(v: &dyn PartialReflect) -> Option<Vec<f32>> {
    if let Some(v) = v.try_downcast_ref::<Vec2>() {
        Some(v.to_array().to_vec())
    } else if let Some(v) = v.try_downcast_ref::<Vec3>() {
        Some(v.to_array().to_vec())
    } else if let Some(v) = v.try_downcast_ref::<Vec4>() {
        Some(v.to_array().to_vec())
    } else if let Some(v) = v.try_downcast_ref::<UVec2>() {
        Some(v.as_vec2().to_array().to_vec())
    } else if let Some(v) = v.try_downcast_ref::<UVec3>() {
        Some(v.as_vec3().to_array().to_vec())
    } else if let Some(v) = v.try_downcast_ref::<IVec2>() {
        Some(v.as_vec2().to_array().to_vec())
    } else if let Some(v) = v.try_downcast_ref::<IVec3>() {
        Some(v.as_vec3().to_array().to_vec())
    } else {
        v.try_downcast_ref::<Quat>()
            .map(|q| EulerEdit::degrees_for(None, *q).to_vec())
    }
}

/// `like` with the lanes in `mask` replaced by those of `new` (same type as
/// `like`; integer lanes are rounded). `None` if `like` is not a vector
/// value or `new` is too short.
pub fn with_lanes(like: &dyn PartialReflect, mask: u8, new: &[f32]) -> Option<Value> {
    let mut lanes = lanes_of(like)?;
    if new.len() < lanes.len() {
        return None;
    }
    for (i, lane) in lanes.iter_mut().enumerate() {
        if mask & (1 << i) != 0 {
            *lane = new[i];
        }
    }
    let l = &lanes;
    let i = |k: usize| l[k].round() as i32;
    let u = |k: usize| l[k].round().max(0.0) as u32;
    Some(if like.try_downcast_ref::<Vec2>().is_some() {
        Box::new(Vec2::from_slice(l))
    } else if like.try_downcast_ref::<Vec3>().is_some() {
        Box::new(Vec3::from_slice(l))
    } else if like.try_downcast_ref::<Vec4>().is_some() {
        Box::new(Vec4::from_slice(l))
    } else if like.try_downcast_ref::<UVec2>().is_some() {
        Box::new(UVec2::new(u(0), u(1)))
    } else if like.try_downcast_ref::<UVec3>().is_some() {
        Box::new(UVec3::new(u(0), u(1), u(2)))
    } else if like.try_downcast_ref::<IVec2>().is_some() {
        Box::new(IVec2::new(i(0), i(1)))
    } else if like.try_downcast_ref::<IVec3>().is_some() {
        Box::new(IVec3::new(i(0), i(1), i(2)))
    } else {
        Box::new(EulerEdit::from_degrees([l[0], l[1], l[2]]).rotation)
    })
}

/// How `b` (the same field of another entity) differs from `row`'s value.
fn compare(row: &FieldRow, b: &dyn PartialReflect) -> Mixed {
    let a = row.value.as_ref();
    match &row.kind {
        FieldKind::Struct => Mixed::Same,
        FieldKind::Enum { current, .. } => match b.reflect_ref() {
            ReflectRef::Enum(e) if e.variant_name() == current => Mixed::Same,
            _ => Mixed::Whole,
        },
        kind => match (lanes_of(a), lanes_of(b)) {
            (Some(x), Some(y)) if x.len() == y.len() => {
                // Rotations compare as shown (degrees rounded to 0.001).
                let tolerance = if *kind == FieldKind::Quat { 1e-3 } else { 0.0 };
                let mask = x
                    .iter()
                    .zip(&y)
                    .enumerate()
                    .filter(|(_, (p, q))| (*p - *q).abs() > tolerance)
                    .fold(0u8, |m, (i, _)| m | (1 << i));
                if mask == 0 {
                    Mixed::Same
                } else {
                    Mixed::Lanes(mask)
                }
            }
            _ => match a.reflect_partial_eq(b) {
                Some(true) => Mixed::Same,
                _ => Mixed::Whole,
            },
        },
    }
}

/// The sections for a selection (`entities`, the primary last): the
/// components every entity has, in the primary's order, with the primary's
/// rows marked [`Mixed`] where the entities disagree. A row some entity
/// lacks (a field of another enum variant) is left out, and so are the
/// fields under an enum whose variant differs. One entity: the same as
/// [`entity_sections`].
///
/// Cost: the primary's rows as for one entity, then per other entity one
/// whole-component comparison per shared component, and per-field reads
/// only for components whose values differ.
pub fn shared_sections(world: &World, entities: &[Entity]) -> Vec<Section> {
    let Some((&primary, others)) = entities.split_last() else {
        return Vec::new();
    };
    let mut sections = entity_sections(world, primary);
    if others.is_empty() {
        return sections;
    }
    sections.retain(|s| {
        others
            .iter()
            .all(|e| world.get_reflect(*e, s.component).is_ok())
    });
    for s in &mut sections {
        let Ok(mine) = world.get_reflect(primary, s.component) else {
            continue;
        };
        // Entities whose whole component differs from the primary's.
        let differ: Vec<Entity> = others
            .iter()
            .copied()
            .filter(|e| {
                world.get_reflect(*e, s.component).is_ok_and(|theirs| {
                    mine.as_partial_reflect()
                        .reflect_partial_eq(theirs.as_partial_reflect())
                        != Some(true)
                })
            })
            .collect();
        if differ.is_empty() {
            continue;
        }
        let mut skipped: Vec<String> = Vec::new();
        s.rows = std::mem::take(&mut s.rows)
            .into_iter()
            .filter_map(|mut row| {
                if skipped.iter().any(|p| row.path.starts_with(p.as_str())) {
                    return None;
                }
                let path = world.field_path(&row.path).ok()?;
                let mut mixed = Mixed::Same;
                for e in &differ {
                    match world.get_field(*e, &path) {
                        Ok(v) => mixed = mixed.union(compare(&row, v.as_ref())),
                        Err(_) => {
                            skipped.push(format!("{}.", row.path));
                            return None;
                        }
                    }
                }
                if matches!(row.kind, FieldKind::Enum { .. }) && mixed.any() {
                    skipped.push(format!("{}.", row.path));
                }
                row.mixed = mixed;
                Some(row)
            })
            .collect();
    }
    sections
}

/// Components that can be added to a selection: registered, not hidden,
/// with a default value, and missing on at least one of `entities`.
pub fn addable_to_any(world: &World, entities: &[Entity]) -> Vec<&'static str> {
    world
        .registry()
        .components()
        .filter(|c| !c.is_hidden() && c.default_value().is_some())
        .map(|c| c.name())
        .filter(|n| *n != "Uid" && *n != "Name")
        .filter(|n| {
            entities
                .iter()
                .any(|e| !world.component_names(*e).contains(n))
        })
        .collect()
}

/// A component section of the inspector.
#[derive(Debug)]
pub struct Section {
    pub component: &'static str,
    pub doc: Option<&'static str>,
    pub rows: Vec<FieldRow>,
}

/// Component names shown for an entity: `Transform` first, then the rest by
/// name. `Uid` and `Name` (edited in the header) and `@Hidden` types are left out.
pub fn shown_components(world: &World, e: Entity) -> Vec<&'static str> {
    let registry = world.registry();
    let mut names: Vec<&'static str> = world
        .component_names(e)
        .into_iter()
        .filter(|n| *n != "Uid" && *n != "Name")
        .filter(|n| registry.component(n).is_some_and(|c| !c.is_hidden()))
        .collect();
    names.sort_by_key(|n| (*n != "Transform", *n));
    names
}

/// Components that can be added to `e`: registered, not hidden, with a
/// default value, and not already present.
pub fn addable_components(world: &World, e: Entity) -> Vec<&'static str> {
    let present = world.component_names(e);
    world
        .registry()
        .components()
        .filter(|c| !c.is_hidden() && c.default_value().is_some())
        .map(|c| c.name())
        .filter(|n| !present.contains(n) && *n != "Uid" && *n != "Name")
        .collect()
}

/// The sections for an entity.
pub fn entity_sections(world: &World, e: Entity) -> Vec<Section> {
    let registry = world.registry().clone();
    shown_components(world, e)
        .into_iter()
        .filter_map(|name| {
            let value = world.get_reflect(e, name).ok()?;
            let value = clone_value(value.as_partial_reflect()).ok()?;
            Some(Section {
                component: name,
                doc: registry.component(name).and_then(|c| c.doc()),
                rows: rows(&registry, name, value.as_ref()),
            })
        })
        .collect()
}

/// The sections for the world's registered, visible resources.
pub fn resource_sections(world: &World) -> Vec<Section> {
    let registry = world.registry().clone();
    world
        .resource_names()
        .into_iter()
        .filter(|n| {
            // Settings resources are project settings (Game ▸ Project
            // Settings), not scene settings.
            registry
                .resource(n)
                .is_some_and(|r| !r.is_hidden() && !r.is_settings())
        })
        .filter_map(|name| {
            let value = world.get_resource_reflect(name).ok()?;
            let value = clone_value(value.as_partial_reflect()).ok()?;
            Some(Section {
                component: name,
                doc: registry.resource(name).and_then(|r| r.doc()),
                rows: rows(&registry, name, value.as_ref()),
            })
        })
        .collect()
}

/// Rows for the fields of a component or resource value (`root` is its name).
pub fn rows(registry: &Registry, root: &str, value: &dyn PartialReflect) -> Vec<FieldRow> {
    let mut out = Vec::new();
    // A component that is itself a leaf (e.g. a tuple struct of one f32) still
    // shows its fields; walk children of the top-level value.
    children(registry, root, value, 0, &mut out);
    out
}

fn meta_for(registry: &Registry, path: &str) -> Option<FieldMeta> {
    let p = box_core::reflect::FieldPath::parse(registry, path).ok()?;
    registry.field_meta(&p)
}

fn children(
    registry: &Registry,
    path: &str,
    value: &dyn PartialReflect,
    depth: usize,
    out: &mut Vec<FieldRow>,
) {
    match value.reflect_ref() {
        ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                if let (Some(name), Some(f)) = (s.name_at(i), s.field_at(i)) {
                    walk(registry, &format!("{path}.{name}"), name, f, depth, out);
                }
            }
        }
        ReflectRef::TupleStruct(s) => {
            for i in 0..s.field_len() {
                if let Some(f) = s.field(i) {
                    walk(
                        registry,
                        &format!("{path}.{i}"),
                        &i.to_string(),
                        f,
                        depth,
                        out,
                    );
                }
            }
        }
        ReflectRef::Enum(e) => {
            for i in 0..e.field_len() {
                let Some(f) = e.field_at(i) else { continue };
                let name = e
                    .name_at(i)
                    .map(str::to_owned)
                    .unwrap_or_else(|| i.to_string());
                walk(registry, &format!("{path}.{name}"), &name, f, depth, out);
            }
        }
        _ => {}
    }
}

fn leaf_kind(v: &dyn PartialReflect) -> Option<FieldKind> {
    macro_rules! is {
        ($t:ty) => {
            v.try_downcast_ref::<$t>().is_some()
        };
    }
    Some(if is!(f32) {
        FieldKind::F32
    } else if is!(f64) {
        FieldKind::F64
    } else if is!(bool) {
        FieldKind::Bool
    } else if is!(String) {
        FieldKind::String
    } else if is!(Vec2) {
        FieldKind::Vec2
    } else if is!(Vec3) {
        FieldKind::Vec3
    } else if is!(Vec4) {
        FieldKind::Vec4
    } else if is!(UVec2) {
        FieldKind::UVec2
    } else if is!(UVec3) {
        FieldKind::UVec3
    } else if is!(IVec2) {
        FieldKind::IVec2
    } else if is!(IVec3) {
        FieldKind::IVec3
    } else if is!(Quat) {
        FieldKind::Quat
    } else if is!(Color) {
        FieldKind::Color
    } else if is!(u8) {
        FieldKind::Int {
            signed: false,
            bits: 8,
        }
    } else if is!(u16) {
        FieldKind::Int {
            signed: false,
            bits: 16,
        }
    } else if is!(u32) {
        FieldKind::Int {
            signed: false,
            bits: 32,
        }
    } else if is!(u64) || is!(usize) {
        FieldKind::Int {
            signed: false,
            bits: 64,
        }
    } else if is!(i8) {
        FieldKind::Int {
            signed: true,
            bits: 8,
        }
    } else if is!(i16) {
        FieldKind::Int {
            signed: true,
            bits: 16,
        }
    } else if is!(i32) {
        FieldKind::Int {
            signed: true,
            bits: 32,
        }
    } else if is!(i64) || is!(isize) {
        FieldKind::Int {
            signed: true,
            bits: 64,
        }
    } else if is!(Uid) {
        FieldKind::ReadOnly(
            v.try_downcast_ref::<Uid>()
                .map(|u| u.to_string())
                .unwrap_or_default(),
        )
    } else if is!(Name) {
        FieldKind::String
    } else if is!(AssetRef) {
        FieldKind::AssetRef
    } else {
        return None;
    })
}

fn walk(
    registry: &Registry,
    path: &str,
    label: &str,
    value: &dyn PartialReflect,
    depth: usize,
    out: &mut Vec<FieldRow>,
) {
    let meta = meta_for(registry, path);
    if meta.as_ref().is_some_and(|m| m.hidden) {
        return;
    }
    let Ok(owned) = clone_value(value) else {
        return;
    };
    let kind = match leaf_kind(value) {
        Some(k) => k,
        None if is_asset_list(value) => match value.reflect_ref() {
            ReflectRef::List(l) => FieldKind::AssetList { len: l.len() },
            _ => FieldKind::ReadOnly(String::new()),
        },
        None => match value.reflect_ref() {
            ReflectRef::Struct(_) | ReflectRef::TupleStruct(_) => FieldKind::Struct,
            ReflectRef::Enum(e) => FieldKind::Enum {
                variants: e
                    .get_represented_enum_info()
                    .map(|i| i.variant_names().to_vec())
                    .unwrap_or_default(),
                current: e.variant_name().to_owned(),
            },
            ReflectRef::List(l) => FieldKind::List { len: l.len() },
            ReflectRef::Array(a) => FieldKind::ReadOnly(format!("{} items", a.len())),
            ReflectRef::Map(m) => FieldKind::ReadOnly(format!("{} entries", m.len())),
            _ => FieldKind::ReadOnly(value.reflect_short_type_path().to_owned()),
        },
    };
    let nested = matches!(kind, FieldKind::Struct | FieldKind::Enum { .. });
    let list = matches!(kind, FieldKind::AssetList { .. });
    let items = matches!(kind, FieldKind::List { .. });
    out.push(FieldRow {
        path: path.to_owned(),
        label: label.to_owned(),
        depth,
        kind,
        meta,
        value: owned,
        mixed: Mixed::Same,
    });
    if nested {
        children(registry, path, value, depth + 1, out);
    }
    if items && let ReflectRef::List(l) = value.reflect_ref() {
        // `points` -> `point 0`, `point 1`, ...
        let item = singular(label);
        for i in 0..l.len() {
            if let Some(v) = l.get(i) {
                walk(
                    registry,
                    &format!("{path}[{i}]"),
                    &format!("{item} {i}"),
                    v,
                    depth + 1,
                    out,
                );
            }
        }
    }
    if list && let ReflectRef::List(l) = value.reflect_ref() {
        for i in 0..l.len() {
            let Some(item) = l.get(i).and_then(|v| clone_value(v).ok()) else {
                continue;
            };
            out.push(FieldRow {
                path: format!("{path}[{i}]"),
                label: format!("slot {i}"),
                depth: depth + 1,
                kind: FieldKind::AssetRef,
                meta: None,
                value: item,
                mixed: Mixed::Same,
            });
        }
    }
}

/// Words whose singular and plural are the same, or that end in `s`
/// without being plurals.
const INVARIANT: &[&str] = &[
    "species", "series", "news", "physics", "status", "canvas", "alias", "bias", "lens", "gas",
    "radius", "focus", "axis", "basis", "analysis", "chassis",
];

/// Irregular plurals: (plural, singular).
const IRREGULAR: &[(&str, &str)] = &[
    ("vertices", "vertex"),
    ("indices", "index"),
    ("matrices", "matrix"),
    ("children", "child"),
    ("leaves", "leaf"),
    ("halves", "half"),
    ("shelves", "shelf"),
    ("people", "person"),
    ("mice", "mouse"),
    ("caches", "cache"),
    ("axes", "axis"),
];

/// The singular of a list field's name, for its item labels:
/// `points` -> `point`, `entries` -> `entry`, `boxes` -> `box`,
/// `edge_slots` -> `edge_slot`, `species` -> `species`,
/// `vertices` -> `vertex`. Only the last word changes; a name that is not
/// a recognisable plural is kept as it is.
pub fn singular(name: &str) -> String {
    let (head, word) = match name.rfind('_') {
        Some(i) => name.split_at(i + 1),
        None => ("", name),
    };
    let lower = word.to_ascii_lowercase();
    let one = if INVARIANT.contains(&lower.as_str()) {
        word.to_owned()
    } else if let Some((_, s)) = IRREGULAR.iter().find(|(p, _)| *p == lower) {
        (*s).to_owned()
    } else if let Some(stem) = lower.strip_suffix("ies")
        && stem.len() >= 2
    {
        format!("{}y", &word[..stem.len()])
    } else if ["sses", "xes", "ches", "shes"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        word[..word.len() - 2].to_owned()
    } else if lower.len() > 2
        && lower.ends_with('s')
        && !["ss", "us", "is"].iter().any(|e| lower.ends_with(e))
    {
        word[..word.len() - 1].to_owned()
    } else {
        word.to_owned()
    };
    format!("{head}{one}")
}

/// A list whose items are `AssetRef`s (such as `MeshAsset::materials`).
fn is_asset_list(value: &dyn PartialReflect) -> bool {
    match value.get_represented_type_info() {
        Some(TypeInfo::List(info)) => info.item_ty().id() == std::any::TypeId::of::<AssetRef>(),
        _ => false,
    }
}

/// A value of the enum `info` switched to `variant`. Fields that exist in the
/// current variant with the same name and type are kept; others take the
/// enum's `VariantDefaults` for that variant, else a sensible default (1 for
/// lengths, 16 for counts, the type's default otherwise), clamped into their
/// declared range.
pub fn switch_variant(
    registry: &Registry,
    current: &dyn PartialReflect,
    variant: &str,
) -> Option<Value> {
    let ReflectRef::Enum(cur) = current.reflect_ref() else {
        return None;
    };
    let info = cur.get_represented_enum_info()?;
    let type_info: &'static TypeInfo = current.get_represented_type_info()?;
    let keep = |name: &str, ty: std::any::TypeId| -> Option<Value> {
        let f = cur.field(name)?;
        (f.get_represented_type_info()?.type_id() == ty)
            .then(|| clone_value(f).ok())
            .flatten()
    };
    let dynamic = match info.variant(variant)? {
        VariantInfo::Unit(_) => DynamicVariant::Unit,
        VariantInfo::Struct(s) => {
            let mut d = DynamicStruct::default();
            for f in s.iter() {
                let v = keep(f.name(), f.type_id())
                    .or_else(|| {
                        registry.variant_field_default(type_info.type_id(), variant, f.name())
                    })
                    .or_else(|| {
                        let range = f
                            .custom_attributes()
                            .get::<box_core::reflect::Range>()
                            .copied();
                        default_value(registry, f.type_info()?, range)
                    })?;
                d.insert_boxed(f.name(), v);
            }
            DynamicVariant::Struct(d)
        }
        VariantInfo::Tuple(t) => {
            let mut d = DynamicTuple::default();
            for f in t.iter() {
                d.insert_boxed(default_value(registry, f.type_info()?, None)?);
            }
            DynamicVariant::Tuple(d)
        }
    };
    let mut e = DynamicEnum::new(variant, dynamic);
    e.set_represented_type(Some(type_info));
    Some(Box::new(e))
}

/// `list` with a copy of its last item appended (an empty list gets the
/// item type's default). `None` when `list` is not a list.
pub fn list_with_item_added(registry: &Registry, list: &dyn PartialReflect) -> Option<Value> {
    let ReflectRef::List(l) = list.reflect_ref() else {
        return None;
    };
    let mut d = l.to_dynamic_list().ok()?;
    let item = match l.len().checked_sub(1).and_then(|i| l.get(i)) {
        Some(last) => clone_value(last).ok()?,
        None => {
            let Some(TypeInfo::List(info)) = list.get_represented_type_info() else {
                return None;
            };
            default_value(registry, info.item_info()?, None)?
        }
    };
    d.push_box(item);
    d.set_represented_type(list.get_represented_type_info());
    Some(Box::new(d))
}

/// `list` without its last item. `None` when `list` is not a list or empty.
pub fn list_with_last_removed(list: &dyn PartialReflect) -> Option<Value> {
    use box_core::reflect::bevy_reflect::list::List as _;
    let ReflectRef::List(l) = list.reflect_ref() else {
        return None;
    };
    let mut d = l.to_dynamic_list().ok()?;
    d.pop()?;
    d.set_represented_type(list.get_represented_type_info());
    Some(Box::new(d))
}

fn default_value(
    registry: &Registry,
    info: &'static TypeInfo,
    range: Option<box_core::reflect::Range>,
) -> Option<Value> {
    let clamp = |v: f64| match range {
        Some(r) => v.clamp(r.min, r.max),
        None => v,
    };
    let id = info.type_id();
    let any = |v: Box<dyn PartialReflect>| Some(v);
    if id == std::any::TypeId::of::<f32>() {
        return any(Box::new(clamp(1.0) as f32));
    }
    if id == std::any::TypeId::of::<f64>() {
        return any(Box::new(clamp(1.0)));
    }
    if id == std::any::TypeId::of::<u32>() {
        return any(Box::new(clamp(16.0) as u32));
    }
    if id == std::any::TypeId::of::<Vec3>() {
        return any(Box::new(Vec3::splat(clamp(1.0) as f32)));
    }
    if id == std::any::TypeId::of::<Vec2>() {
        return any(Box::new(Vec2::splat(clamp(1.0) as f32)));
    }
    if id == std::any::TypeId::of::<UVec2>() {
        return any(Box::new(UVec2::splat(clamp(1.0) as u32)));
    }
    let reg = registry.type_registry().get(id)?;
    reg.data::<ReflectDefault>()
        .map(|d| d.default().into_partial_reflect())
}

// ----- component names and categories (Add Component, section headers) -----

/// A component type name as shown: `StandardMaterial` -> `Standard
/// Material`, `UVOffset` -> `UV Offset`.
pub fn friendly_name(type_name: &str) -> String {
    let chars: Vec<char> = type_name.chars().collect();
    let mut out = String::with_capacity(type_name.len() + 4);
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            // A new word: after a lowercase letter or digit, or the last
            // capital of an acronym followed by lowercase (`UVOffset`).
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(*c);
    }
    out
}

/// Where a component is listed in Add Component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComponentCategory {
    Rendering,
    Lights,
    Physics,
    AssetsPrefabs,
    /// The game's own types (not in the engine registry).
    Gameplay,
    Other,
}

impl ComponentCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rendering => "Rendering",
            Self::Lights => "Lights",
            Self::Physics => "Physics",
            Self::AssetsPrefabs => "Assets & Prefabs",
            Self::Gameplay => "Gameplay",
            Self::Other => "Other",
        }
    }
}

/// The category of a registered component type.
pub fn component_category(name: &str) -> ComponentCategory {
    match name {
        "Primitive" | "StandardMaterial" | "ShaderMaterial" | "Camera" | "ParticleEmitter" => {
            ComponentCategory::Rendering
        }
        "DirectionalLight" | "PointLight" | "SpotLight" | "AmbientVolume" => {
            ComponentCategory::Lights
        }
        "MeshAsset" | "PrefabInstance" => ComponentCategory::AssetsPrefabs,
        "Script" => ComponentCategory::Gameplay,
        n if box_engine::shared_engine_registry().component(n).is_none() => {
            ComponentCategory::Gameplay
        }
        n if n.contains("Collider") || n.contains("Body") || n.contains("Joint") => {
            ComponentCategory::Physics
        }
        _ => ComponentCategory::Other,
    }
}

/// One entry of the Add Component list.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentChoice {
    pub name: &'static str,
    pub label: String,
    pub category: ComponentCategory,
}

/// The `addable` components matching `query` (case-insensitive, in the
/// friendly name, the type name or the doc comment), by category and then
/// name; within a search, names that start with the query come first in
/// each category.
pub fn component_choices(
    registry: &Registry,
    addable: &[&'static str],
    query: &str,
) -> Vec<ComponentChoice> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<(bool, ComponentChoice)> = addable
        .iter()
        .filter_map(|name| {
            let label = friendly_name(name);
            let doc = registry
                .component(name)
                .and_then(|c| c.doc())
                .unwrap_or_default()
                .to_lowercase();
            let lower = label.to_lowercase();
            let hit = q.is_empty()
                || lower.contains(&q)
                || name.to_lowercase().contains(&q)
                || doc.contains(&q);
            let prefix =
                !q.is_empty() && (lower.starts_with(&q) || name.to_lowercase().starts_with(&q));
            hit.then(|| {
                (
                    prefix,
                    ComponentChoice {
                        name,
                        label,
                        category: component_category(name),
                    },
                )
            })
        })
        .collect();
    out.sort_by(|(pa, a), (pb, b)| (a.category, !pa, &a.label).cmp(&(b.category, !pb, &b.label)));
    out.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn undo_labels_are_readable() {
        assert_eq!(
            edit_label(Some("Cube"), "Transform.translation"),
            "Cube: Translation"
        );
        assert_eq!(
            edit_label(Some("3 entities"), "StandardMaterial.base_color"),
            "3 entities: Base color"
        );
        assert_eq!(edit_label(Some("Cube"), "Name"), "Rename Cube");
        assert_eq!(
            edit_label(None, "Environment.exposure"),
            "Environment: Exposure"
        );
        assert_eq!(
            field_label("DirectionalLight.shadow.depth_bias"),
            "Shadow depth bias"
        );
        assert_eq!(field_label("MeshAsset.materials[0]"), "Materials [0]");
        assert_eq!(field_label("Primitive"), "Primitive");
    }

    use super::*;
    use box_core::prelude::{Primitive, Shape, Transform};

    #[test]
    fn transform_and_primitive_rows() {
        let mut world = World::new();
        let e = world
            .spawn_saved(
                Uid::from_u128(1),
                "Ball",
                (
                    Transform::default(),
                    Primitive::new(Shape::Sphere {
                        radius: 2.0,
                        sectors: 8,
                        stacks: 4,
                    }),
                ),
            )
            .unwrap();
        let sections = entity_sections(&world, e);
        let names: Vec<_> = sections.iter().map(|s| s.component).collect();
        assert_eq!(names, ["Transform", "Primitive"]);
        let t: Vec<(&str, &FieldKind)> = sections[0]
            .rows
            .iter()
            .map(|r| (r.path.as_str(), &r.kind))
            .collect();
        assert_eq!(
            t,
            [
                ("Transform.translation", &FieldKind::Vec3),
                ("Transform.rotation", &FieldKind::Quat),
                ("Transform.scale", &FieldKind::Vec3)
            ]
        );
        assert_eq!(sections[0].rows[0].meta.as_ref().unwrap().unit, Some("m"));
        let p: Vec<&str> = sections[1].rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            p,
            [
                "Primitive.shape",
                "Primitive.shape.radius",
                "Primitive.shape.sectors",
                "Primitive.shape.stacks",
                "Primitive.cast_shadows"
            ]
        );
        let radius = &sections[1].rows[1];
        assert_eq!(radius.depth, 1);
        assert_eq!(radius.value.try_downcast_ref::<f32>(), Some(&2.0));
        assert!(radius.meta.as_ref().unwrap().range.is_some());
        assert!(
            addable_components(&world, e)
                .iter()
                .all(|c| *c != "Transform" && *c != "Primitive")
        );
    }

    #[test]
    fn switching_variant_keeps_matching_fields_and_defaults_the_rest() {
        let mut world = World::new();
        let uid = Uid::from_u128(2);
        let e = world
            .spawn_saved(
                uid,
                "Can",
                (Primitive::new(Shape::Sphere {
                    radius: 2.0,
                    sectors: 8,
                    stacks: 4,
                }),),
            )
            .unwrap();
        let path = world.field_path("Primitive.shape").unwrap();
        let current = world.get_field(e, &path).unwrap();
        let v = switch_variant(world.registry(), current.as_ref(), "Cylinder").unwrap();
        world.set_field(e, &path, v.as_ref()).unwrap();
        let shape = world.get::<Primitive>(e).unwrap().shape.clone();
        assert_eq!(
            shape,
            Shape::Cylinder {
                radius: 2.0,
                height: 1.0,
                segments: 32
            }
        );
    }
}
