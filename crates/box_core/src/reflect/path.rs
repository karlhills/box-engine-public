//! `"Component.field.path"` strings, parsed once.

use std::fmt;

use bevy_reflect::ParsedPath;

use super::registry::Kind;
use super::{ReflectError, Registry};

/// A path to a component or resource, or to a field inside one:
/// `"Transform"`, `"Transform.translation.x"`, `"Npc.tags[0]"`,
/// `"Environment.fog.density"`.
///
/// The first segment is a registered short type name; the rest uses
/// bevy_reflect path syntax (`.field`, `.0`, `[3]`). Parsing checks the type
/// name and the syntax, not that the field exists.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FieldPath {
    text: String,
    target: &'static str,
    resource: bool,
    /// Byte offset of the field part in `text` (after the separating `.`).
    field_start: usize,
    parsed: ParsedPath,
}

impl FieldPath {
    pub fn parse(registry: &Registry, text: &str) -> Result<Self, ReflectError> {
        let split = text.find(['.', '[']).unwrap_or(text.len());
        let (name, rest) = text.split_at(split);
        let (target, kind) = registry
            .kind_of(name)
            .ok_or_else(|| ReflectError::UnknownType(name.to_string()))?;
        let parsed = if rest.is_empty() {
            ParsedPath::empty()
        } else {
            ParsedPath::parse(rest).map_err(|e| ReflectError::Path {
                path: text.to_string(),
                message: e.to_string(),
            })?
        };
        if !rest.is_empty() && parsed.0.is_empty() {
            return Err(ReflectError::Path {
                path: text.to_string(),
                message: "empty field path".into(),
            });
        }
        let field_start = if rest.starts_with('.') {
            split + 1
        } else {
            split
        };
        Ok(Self {
            text: text.to_string(),
            target,
            resource: kind == Kind::Resource,
            field_start,
            parsed,
        })
    }

    /// The component or resource name.
    pub fn target(&self) -> &'static str {
        self.target
    }

    /// True if the path names a resource rather than a component.
    pub fn is_resource(&self) -> bool {
        self.resource
    }

    /// The field part without the type name (`"translation.x"`), or `""` for
    /// the whole component.
    pub fn field(&self) -> &str {
        &self.text[self.field_start..]
    }

    /// True if the path names the whole component or resource.
    pub fn is_whole(&self) -> bool {
        self.parsed.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn parsed(&self) -> &ParsedPath {
        &self.parsed
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}
