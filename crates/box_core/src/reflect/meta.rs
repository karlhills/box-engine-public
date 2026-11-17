//! Inspector metadata read from `TypeInfo`: names, types, doc comments and the
//! `@Range`/`@Unit`/`@Step`/`@Logarithmic`/`@Hidden` field attributes.

use std::borrow::Cow;

use bevy_reflect::access::Access;
use bevy_reflect::attributes::CustomAttributes;
use bevy_reflect::enums::VariantInfo;
use bevy_reflect::{NamedField, OffsetAccess, ParsedPath, TypeInfo, UnnamedField};

use super::{Hidden, Logarithmic, Range, Step, Unit};

/// Everything the inspector needs to draw one field row.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldMeta {
    /// Field name; `"0"`, `"1"`, ... for tuple fields, `"[3]"` for list elements.
    pub name: Cow<'static, str>,
    /// Full type path of the field's type, e.g. `"f32"` or `"glam::Vec3"`.
    pub type_path: &'static str,
    /// The field's doc comment, used as the tooltip.
    pub doc: Option<&'static str>,
    pub range: Option<Range>,
    pub unit: Option<&'static str>,
    pub step: Option<f64>,
    /// `@Logarithmic`: a logarithmic slider.
    pub logarithmic: bool,
    /// `@Hidden`: not shown in the inspector.
    pub hidden: bool,
}

fn doc(d: Option<&'static str>) -> Option<&'static str> {
    d.map(str::trim).filter(|d| !d.is_empty())
}

fn from_attrs(
    name: Cow<'static, str>,
    type_path: &'static str,
    docs: Option<&'static str>,
    attrs: &CustomAttributes,
) -> FieldMeta {
    FieldMeta {
        name,
        type_path,
        doc: doc(docs),
        range: attrs.get::<Range>().copied(),
        unit: attrs.get::<Unit>().map(|u| u.0),
        step: attrs.get::<Step>().map(|s| s.0),
        logarithmic: attrs.contains::<Logarithmic>(),
        hidden: attrs.contains::<Hidden>(),
    }
}

fn named(f: &NamedField) -> FieldMeta {
    from_attrs(
        Cow::Borrowed(f.name()),
        f.type_path(),
        f.docs(),
        f.custom_attributes(),
    )
}

fn unnamed(f: &UnnamedField) -> FieldMeta {
    from_attrs(
        Cow::Owned(f.index().to_string()),
        f.type_path(),
        f.docs(),
        f.custom_attributes(),
    )
}

/// Top-level fields of a struct or tuple struct, in declaration order.
pub(crate) fn fields_of(info: &TypeInfo) -> Vec<FieldMeta> {
    match info {
        TypeInfo::Struct(s) => s.iter().map(named).collect(),
        TypeInfo::TupleStruct(t) => t.iter().map(unnamed).collect(),
        _ => Vec::new(),
    }
}

/// Fields of one enum variant, in declaration order.
pub(crate) fn variant_fields(v: &VariantInfo) -> Vec<FieldMeta> {
    match v {
        VariantInfo::Struct(s) => s.iter().map(named).collect(),
        VariantInfo::Tuple(t) => t.iter().map(unnamed).collect(),
        VariantInfo::Unit(_) => Vec::new(),
    }
}

fn inherit(parent: Option<FieldMeta>, child: FieldMeta) -> FieldMeta {
    let Some(p) = parent else { return child };
    FieldMeta {
        doc: child.doc.or(p.doc),
        range: child.range.or(p.range),
        unit: child.unit.or(p.unit),
        step: child.step.or(p.step),
        logarithmic: child.logarithmic || p.logarithmic,
        hidden: child.hidden || p.hidden,
        ..child
    }
}

/// Walk `path` through `root`'s type information.
pub(crate) fn lookup(root: &'static TypeInfo, path: &ParsedPath) -> Option<FieldMeta> {
    let mut info = Some(root);
    let mut current: Option<FieldMeta> = None;
    for OffsetAccess { access, .. } in &path.0 {
        let ti = info?;
        let (meta, next) = match (access, ti) {
            (Access::Field(name), TypeInfo::Struct(s)) => {
                let f = s.field(name)?;
                (named(f), f.type_info())
            }
            (Access::FieldIndex(i), TypeInfo::Struct(s)) => {
                let f = s.field_at(*i)?;
                (named(f), f.type_info())
            }
            (Access::Field(name), TypeInfo::Enum(e)) => {
                let f = e.iter().find_map(|v| match v {
                    VariantInfo::Struct(sv) => sv.field(name),
                    _ => None,
                })?;
                (named(f), f.type_info())
            }
            (Access::TupleIndex(i), TypeInfo::TupleStruct(t)) => {
                let f = t.field_at(*i)?;
                (unnamed(f), f.type_info())
            }
            (Access::TupleIndex(i), TypeInfo::Tuple(t)) => {
                let f = t.field_at(*i)?;
                (unnamed(f), f.type_info())
            }
            (Access::TupleIndex(i), TypeInfo::Enum(e)) => {
                let f = e.iter().find_map(|v| match v {
                    VariantInfo::Tuple(tv) => tv.field_at(*i),
                    _ => None,
                })?;
                (unnamed(f), f.type_info())
            }
            (Access::ListIndex(i), TypeInfo::List(l)) => {
                (element(&current, *i, l.item_ty().path()), l.item_info())
            }
            (Access::ListIndex(i), TypeInfo::Array(a)) => {
                (element(&current, *i, a.item_ty().path()), a.item_info())
            }
            _ => return None,
        };
        current = Some(inherit(current, meta));
        info = next;
    }
    current
}

/// A readable explanation of the first segment of `path` that names a field
/// `root` (whose short name is `root_name`) does not have:
/// "`Coin` has no field `pionts`; fields: points, spin (did you mean `points`?)".
/// `None` when every named field exists (the problem is elsewhere, such as an
/// index out of bounds or another enum variant).
pub(crate) fn explain_missing_field(
    root: &'static TypeInfo,
    root_name: &str,
    path: &ParsedPath,
) -> Option<String> {
    let mut info = Some(root);
    let mut type_name = root_name.to_string();
    for OffsetAccess { access, .. } in &path.0 {
        let ti = info?;
        let next = match (access, ti) {
            (Access::Field(name), TypeInfo::Struct(s)) => match s.field(name) {
                Some(f) => f.type_info(),
                None => {
                    let fields: Vec<&str> = s.field_names().to_vec();
                    return Some(missing(&type_name, name, &fields));
                }
            },
            (Access::Field(name), TypeInfo::Enum(e)) => {
                let fields: Vec<&str> = e
                    .iter()
                    .filter_map(|v| match v {
                        VariantInfo::Struct(sv) => Some(sv.field_names().to_vec()),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                match e.iter().find_map(|v| match v {
                    VariantInfo::Struct(sv) => sv.field(name),
                    _ => None,
                }) {
                    Some(f) => f.type_info(),
                    None => return Some(missing(&type_name, name, &fields)),
                }
            }
            (Access::TupleIndex(i), TypeInfo::TupleStruct(t)) => t.field_at(*i)?.type_info(),
            (Access::TupleIndex(i), TypeInfo::Tuple(t)) => t.field_at(*i)?.type_info(),
            (Access::ListIndex(_), TypeInfo::List(l)) => l.item_info(),
            (Access::ListIndex(_), TypeInfo::Array(a)) => a.item_info(),
            (Access::FieldIndex(i), TypeInfo::Struct(s)) => s.field_at(*i)?.type_info(),
            _ => return None,
        };
        info = next;
        if let Some(n) = next {
            type_name = n.type_path_table().short_path().to_string();
        }
    }
    None
}

fn missing(type_name: &str, name: &str, fields: &[&str]) -> String {
    let mut message = if fields.is_empty() {
        format!("`{type_name}` has no field `{name}` (it has no named fields)")
    } else {
        format!(
            "`{type_name}` has no field `{name}`; fields: {}",
            fields.join(", ")
        )
    };
    if let Some(guess) = super::did_you_mean(name, fields.iter().copied()) {
        message.push_str(&format!(" (did you mean `{guess}`?)"));
    }
    message
}

fn element(parent: &Option<FieldMeta>, index: usize, type_path: &'static str) -> FieldMeta {
    FieldMeta {
        name: Cow::Owned(format!("[{index}]")),
        type_path,
        doc: None,
        range: None,
        unit: None,
        step: None,
        logarithmic: false,
        hidden: parent.as_ref().is_some_and(|p| p.hidden),
    }
}
