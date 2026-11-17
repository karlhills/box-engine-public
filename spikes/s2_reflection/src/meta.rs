//! Inspector metadata attached to fields with `#[reflect(@value)]` custom
//! attributes, plus doc comments (feature `reflect_documentation`).
//!
//! bevy_reflect has no `#[reflect(range = .., unit = ..)]` key/value syntax; a
//! custom attribute is any reflectable value, looked up later by its type.

use bevy_reflect::{NamedField, Reflect, TypeInfo};

use crate::registry::Registry;

/// Inclusive numeric range for sliders and clamping.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
}

/// Display unit ("m", "lm", "deg").
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Unit(pub &'static str);

/// Drag step for numeric widgets.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Step(pub f64);

/// Everything the inspector needs to draw one field row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldMeta {
    pub name: String,
    pub type_path: String,
    /// The field's doc comment, used as the tooltip.
    pub doc: Option<String>,
    pub range: Option<Range>,
    pub unit: Option<&'static str>,
    pub step: Option<f64>,
}

fn field_meta(field: &NamedField) -> FieldMeta {
    FieldMeta {
        name: field.name().to_string(),
        type_path: field.type_path().to_string(),
        doc: field.docs().map(|d| d.trim().to_string()),
        range: field.get_attribute::<Range>().copied(),
        unit: field.get_attribute::<Unit>().map(|u| u.0),
        step: field.get_attribute::<Step>().map(|s| s.0),
    }
}

/// Metadata for every named field of a registered component, in declaration order.
pub fn component_fields(registry: &Registry, component: &str) -> Option<Vec<FieldMeta>> {
    let reg = registry.registration(component)?;
    match reg.type_info() {
        TypeInfo::Struct(info) => Some(info.iter().map(field_meta).collect()),
        // Tuple structs, enums: no named fields to annotate in this spike.
        _ => Some(Vec::new()),
    }
}

/// Metadata for a top-level field of a component (`"PointLight.intensity"`).
pub fn field(registry: &Registry, path: &str) -> Option<FieldMeta> {
    let (component, field) = path.split_once('.')?;
    let field = field.split(['.', '[']).next()?;
    component_fields(registry, component)?
        .into_iter()
        .find(|f| f.name == field)
}

/// The type-level doc comment.
pub fn component_doc(registry: &Registry, component: &str) -> Option<String> {
    registry
        .registration(component)?
        .type_info()
        .docs()
        .map(|d| d.trim().to_string())
}
