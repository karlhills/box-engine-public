//! Reflection facade over `bevy_reflect` (decision D3).
//!
//! Game and engine code derive [`Reflect`] and annotate types with
//! `#[reflect(...)]`; everything else (registry, field metadata, paths, value
//! cloning and structural replace) is owned here, so the inspector, scene files,
//! undo and the CLI consume box-engine types rather than `bevy_reflect` internals.
//!
//! ```
//! use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault, Range, Unit};
//!
//! /// An omnidirectional light.
//! #[derive(Reflect, Clone, Default)]
//! #[reflect(Component, Default, Clone)]
//! #[type_path = "game"]
//! pub struct Lamp {
//!     /// Luminous power.
//!     #[reflect(@Range { min: 0.0, max: 100_000.0 }, @Unit("lm"))]
//!     pub intensity: f32,
//! }
//!
//! let mut registry = box_core::reflect::Registry::with_core_types();
//! registry.register::<Lamp>().unwrap();
//! let meta = registry.component("Lamp").unwrap().fields();
//! assert_eq!(meta[0].unit, Some("lm"));
//! ```
//!
//! The derive macro emits paths starting with `bevy_reflect::`. Game crates
//! get that name from `use box_engine::prelude::*` (which re-exports this
//! module's `bevy_reflect`, plus `Range`, `Unit`, `Step`, `Hidden` and
//! `NotSaved`), so they need no `bevy_reflect` dependency. Engine crates
//! depend on it directly (`bevy_reflect.workspace = true`).
//!
//! `#[type_path = "..."]` is optional: the registry and scene files use the
//! short type name, which it does not change. Engine crates set it to
//! `box_engine` to keep full type paths stable across module moves.
//!
//! Attributes understood by box-engine:
//!
//! | Attribute | On | Meaning |
//! |---|---|---|
//! | `#[reflect(Component)]` | type | Storable as a component; required by [`Registry::register`] |
//! | `#[reflect(Resource)]` | type | Storable as a world resource; required by [`Registry::register_resource`] |
//! | `#[reflect(Default)]` | type | Enables "add component" with a default value |
//! | `#[reflect(VariantDefaults)]` | enum | Per-variant defaults for fields a value leaves out ([`VariantDefaults`]) |
//! | `#[reflect(@Range { min, max })]` | field | Slider range and clamping hint |
//! | `#[reflect(@Unit("m"))]` | field | Display unit |
//! | `#[reflect(@Step(0.1))]` | field | Drag step; also the decimals shown |
//! | `#[reflect(@Logarithmic)]` | field | Slider on a logarithmic scale (with `@Range`) |
//! | `#[reflect(@Hidden)]` | type or field | Not shown in the inspector or the add-component menu |
//! | `#[reflect(@NotSaved)]` | type | Never written to scene files |
//!
//! Doc comments on types and fields are kept (feature `reflect_documentation`)
//! and shown as tooltips.

mod dump;
mod meta;
mod path;
mod registry;
mod schema;

use std::any::TypeId;

pub use bevy_reflect;
pub use bevy_reflect::std_traits::ReflectDefault;
pub use bevy_reflect::{
    FromReflect, GetTypeRegistration, PartialReflect, Reflect, ReflectDeserialize, ReflectKind,
    ReflectMut, ReflectRef, ReflectSerialize, TypeInfo, TypePath, Typed,
};

pub use meta::FieldMeta;
pub use path::FieldPath;
pub use registry::{
    ComponentRef, ComponentType, ComponentTypes, EventType, ReflectComponent, ReflectEvent,
    ReflectResource, Registry, ResourceType, VariantCompletion,
};
pub use schema::{Checked, SchemaType};

/// The candidate closest to `name` if it is close enough to be a likely typo
/// (case-insensitive edit distance at most a third of the name's length, and
/// at least 1): `did_you_mean("Coinn", ["Coin", "Door"]) == Some("Coin")`.
pub fn did_you_mean<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let lower = name.to_lowercase();
    let limit = (name.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (edit_distance(&lower, &c.to_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Levenshtein distance over chars.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

use crate::identity::Uid;
use crate::world::Entity;

/// An owned reflected value: a concrete component or field value, or a dynamic
/// one (`DynamicStruct`, ...) as produced by the deserializer or scripts.
pub type Value = Box<dyn PartialReflect>;

/// Inclusive numeric range for sliders and clamping: `#[reflect(@Range { min: 0.0, max: 1.0 })]`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

/// Display unit such as `"m"`, `"lm"` or `"deg"`: `#[reflect(@Unit("m"))]`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Unit(pub &'static str);

/// Drag step for numeric widgets: `#[reflect(@Step(0.1))]`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Step(pub f64);

/// Show a ranged number on a logarithmic slider, for values spanning
/// orders of magnitude such as fog density: `#[reflect(@Logarithmic)]`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Logarithmic;

/// Hide a type or field from the inspector: `#[reflect(@Hidden)]`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Hidden;

/// Keep a component or resource type out of scene files: `#[reflect(@NotSaved)]`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct NotSaved;

/// Per-variant defaults of a reflected enum: the value a variant starts as
/// when a scene file, an edit command, a script or the inspector switches to
/// it without giving every field. `{ "Sphere": { "radius": 0.5 } }` takes
/// `sectors` and `stacks` from `variant_default("Sphere")` instead of each
/// field type's default (0). Enable it with `#[reflect(VariantDefaults)]`
/// next to `#[reflect(Default)]`:
///
/// ```
/// use box_core::reflect::{Reflect, ReflectDefault, ReflectVariantDefaults, VariantDefaults};
///
/// #[derive(Reflect, Clone, Debug, PartialEq)]
/// #[reflect(Default, VariantDefaults)]
/// pub enum Spawner {
///     Ring { radius: f32, count: u32 },
///     Line { length: f32, count: u32 },
/// }
///
/// impl Default for Spawner {
///     fn default() -> Self {
///         Self::Ring { radius: 2.0, count: 8 }
///     }
/// }
///
/// impl VariantDefaults for Spawner {
///     fn variant_default(variant: &str) -> Option<Self> {
///         Some(match variant {
///             "Ring" => Self::default(),
///             "Line" => Self::Line { length: 4.0, count: 8 },
///             _ => return None,
///         })
///     }
/// }
/// ```
///
/// A variant for which it returns `None` (or a type without it) fills each
/// missing field with its type's default, and edit commands warn about it.
pub trait VariantDefaults: Reflect + Sized {
    /// The default value of the variant named `variant` (its Rust name), or
    /// `None` when it has none.
    fn variant_default(variant: &str) -> Option<Self>;
}

/// Type data for [`VariantDefaults`], added by `#[reflect(VariantDefaults)]`.
/// Read it through [`Registry::variant_default`].
#[derive(Clone)]
pub struct ReflectVariantDefaults {
    get: fn(&str) -> Option<Box<dyn Reflect>>,
}

impl ReflectVariantDefaults {
    /// The default value of `variant`, if the type has one.
    pub fn get(&self, variant: &str) -> Option<Box<dyn Reflect>> {
        (self.get)(variant)
    }
}

impl<T: VariantDefaults> bevy_reflect::CreateTypeData<T> for ReflectVariantDefaults {
    fn create_type_data(_: ()) -> Self {
        Self {
            get: |variant| T::variant_default(variant).map(|v| Box::new(v) as Box<dyn Reflect>),
        }
    }
}

/// Errors from the registry and from reflective world access.
#[derive(Debug, thiserror::Error)]
pub enum ReflectError {
    #[error("unknown component or resource type `{0}`")]
    UnknownType(String),
    #[error(
        "`{name}` is already registered as `{existing}`; `{new}` cannot use the same short name"
    )]
    DuplicateName {
        name: String,
        existing: &'static str,
        new: &'static str,
    },
    #[error("`{0}` is registered without #[reflect(Component)]")]
    NotAComponent(String),
    #[error("`{0}` is registered without #[reflect(Resource)]")]
    NotAResource(String),
    #[error("entity {0:?} does not exist")]
    NoEntity(Entity),
    #[error("no entity with uid {0}")]
    NoUid(Uid),
    #[error("uid {0} is already used by another entity")]
    DuplicateUid(Uid),
    #[error("entity has no `{0}` component")]
    Missing(String),
    #[error("entity already has a `{0}` component")]
    AlreadyPresent(String),
    #[error("resource `{0}` is not in the world")]
    MissingResource(String),
    #[error("`{0}` has no default value (#[reflect(Default)] missing)")]
    NoDefault(String),
    #[error("`{0}` is read-only")]
    ReadOnly(String),
    #[error("bad field path `{path}`: {message}")]
    Path { path: String, message: String },
    #[error("value of type `{got}` cannot be stored in `{expected}`")]
    Shape { expected: String, got: String },
    #[error("type `{0}` is not registered")]
    Unregistered(String),
    #[error("cannot convert `{type_path}` from JSON: {message}")]
    Deserialize { type_path: String, message: String },
    #[error("cannot convert `{type_path}` to JSON: {message}")]
    Serialize { type_path: String, message: String },
    #[error("the new parent is the entity itself or one of its descendants")]
    Cycle,
    #[error("cannot copy a value of type `{0}`")]
    Clone(String),
}

/// An owned copy of a reflected value: concrete when the type supports
/// `reflect_clone`, dynamic otherwise (fixed-size arrays have no `reflect_clone`
/// in bevy_reflect 0.20).
pub fn clone_value(value: &dyn PartialReflect) -> Result<Value, ReflectError> {
    match value.reflect_clone() {
        Ok(v) => Ok(v.into_partial_reflect()),
        Err(_) => value
            .to_dynamic()
            .map_err(|_| ReflectError::Clone(value.reflect_type_path().to_string())),
    }
}

/// Make `target` structurally equal to `value`, which may be concrete or dynamic.
///
/// bevy's `try_apply` is not a replace: on a list it never shrinks, so applying
/// `["c"]` over `["a", "b"]` gives `["c", "b"]`. This walks both values,
/// truncating lists on the way, and leaves leaves and enum variant switches to
/// `try_apply`.
pub fn replace(
    target: &mut dyn PartialReflect,
    value: &dyn PartialReflect,
) -> Result<(), ReflectError> {
    // `&'static` type paths, taken before `target` is borrowed mutably below.
    let expected = static_type_path(target);
    let got = static_type_path(value);
    match (target.reflect_mut(), value.reflect_ref()) {
        (ReflectMut::Struct(t), ReflectRef::Struct(v)) => {
            for i in 0..v.field_len() {
                let (Some(name), Some(field)) = (v.name_at(i), v.field_at(i)) else {
                    continue;
                };
                let Some(target_field) = t.field_mut(name) else {
                    return Err(ReflectError::Shape {
                        expected: expected.to_string(),
                        got: format!("{got} (field `{name}` is not in `{expected}`)"),
                    });
                };
                replace(target_field, field)?;
            }
            Ok(())
        }
        (ReflectMut::TupleStruct(t), ReflectRef::TupleStruct(v)) => {
            for i in 0..v.field_len().min(t.field_len()) {
                if let (Some(tf), Some(vf)) = (t.field_mut(i), v.field(i)) {
                    replace(tf, vf)?;
                }
            }
            Ok(())
        }
        (ReflectMut::Tuple(t), ReflectRef::Tuple(v)) => {
            for i in 0..v.field_len().min(t.field_len()) {
                if let (Some(tf), Some(vf)) = (t.field_mut(i), v.field(i)) {
                    replace(tf, vf)?;
                }
            }
            Ok(())
        }
        (ReflectMut::Array(t), ReflectRef::Array(v)) if t.len() == v.len() => {
            for i in 0..v.len() {
                if let (Some(tf), Some(vf)) = (t.get_mut(i), v.get(i)) {
                    replace(tf, vf)?;
                }
            }
            Ok(())
        }
        (ReflectMut::List(t), ReflectRef::List(v)) => {
            while t.len() > v.len() {
                t.pop();
            }
            for i in 0..v.len() {
                let Some(vf) = v.get(i) else { continue };
                if let Some(tf) = t.get_mut(i) {
                    replace(tf, vf)?;
                } else {
                    let copy = vf
                        .to_dynamic()
                        .map_err(|_| ReflectError::Clone(vf.reflect_type_path().to_string()))?;
                    t.push(copy);
                }
            }
            Ok(())
        }
        (ReflectMut::Enum(t), ReflectRef::Enum(v)) if t.variant_name() == v.variant_name() => {
            for i in 0..v.field_len().min(t.field_len()) {
                if let (Some(tf), Some(vf)) = (t.field_at_mut(i), v.field_at(i)) {
                    replace(tf, vf)?;
                }
            }
            Ok(())
        }
        // Different enum variant (built from `value`), opaque leaves, maps, sets.
        _ => target.try_apply(value).map_err(|e| ReflectError::Shape {
            expected: expected.to_string(),
            got: format!("{got} ({e})"),
        }),
    }
}

fn static_type_path(value: &dyn PartialReflect) -> &'static str {
    value
        .get_represented_type_info()
        .map_or("<dynamic value>", TypeInfo::type_path)
}

/// The represented type of a value (the concrete type, or the type a dynamic
/// value stands for).
pub(crate) fn represented_type(value: &dyn PartialReflect) -> Option<TypeId> {
    value.get_represented_type_info().map(TypeInfo::type_id)
}
