//! [`Overrides`]: manual changes to derived parts, saved on their owner
//! (docs/PROCEDURAL_DIRECTION.md §5.4).
//!
//! A rule decides every part from the owner's intent; an override lets the
//! user change one detail anyway (this wall's colour, that corner's height)
//! or hide a part. Overrides are ordinary saved data on the owner, so undo,
//! save, reload, the CLI and MCP handle them like any field.
//!
//! **Format.** One [`PartOverride`] per part, addressed by the rule's name
//! and the part's role (its [`PartKey`] without the owner uid, which is the
//! owner itself): `hidden`, and a list of [`FieldOverride`]s, each a
//! reflected field path starting with the component's short name
//! (`WallGen.height`, `StandardMaterial.base_color`) and the value as JSON
//! text in the scene-file representation of that field (`"3.2"`,
//! `"\"Open\""`, `"[1.0,0.5,0.2,1.0]"`).
//!
//! **Application.** [`crate::derive::reconcile`] applies an owner's
//! overrides after the rule's output and before anything hashes it: a
//! patched field is just another parameter, so only that part regenerates,
//! and removing the override restores the rule's value. A hidden part is
//! not spawned (its key still counts as existing). An override whose part
//! the rule no longer makes, or whose field does not exist, is kept and
//! shown as a warning on the owner; it is never dropped.
//!
//! **Change gate.** `add_rule` rules re-run when an intent's `Overrides`
//! changes (the owner is the intent in every rule so far). A hand-written
//! system that calls `reconcile` re-runs it itself.
//!
//! ```
//! use box_proc::overrides::Overrides;
//!
//! let mut o = Overrides::default();
//! o.set_field("walls", "wall", "WallGen.height", "3.5");
//! o.set_hidden("walls", "cap", true);
//! assert_eq!(o.field("walls", "wall", "WallGen.height"), Some("3.5"));
//! assert!(o.is_hidden("walls", "cap"));
//! o.reset_field("walls", "wall", "WallGen.height");
//! o.set_hidden("walls", "cap", false);
//! assert!(o.is_empty()); // empty entries are dropped
//! ```
//!
//! [`PartKey`]: box_core::derived::PartKey

use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault};

/// Saved on the owner of derived parts: per-part field values and hidden
/// parts that replace what the rules decided. Edit it through the
/// inspector of a part (Cmd+Alt+click it in the viewport), or as data.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Overrides {
    /// One entry per overridden part.
    pub parts: Vec<PartOverride>,
}

/// The overrides of one part.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct PartOverride {
    /// The rule that makes the part (`walls`).
    pub rule: String,
    /// The part's role: its key without the owner uid (`wall`, `end0`).
    pub part: String,
    /// The part is not built.
    pub hidden: bool,
    /// Field values that replace the rule's.
    pub fields: Vec<FieldOverride>,
}

/// One overridden field of a part.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct FieldOverride {
    /// Reflected field path, starting with the component's short name:
    /// `WallGen.height`.
    pub path: String,
    /// The value as JSON text, in the scene-file representation of the
    /// field: `3.5`, `"Open"`, `[1.0, 0.5, 0.2, 1.0]`.
    pub value: String,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// The overrides of `rule`'s part `part`.
    pub fn part(&self, rule: &str, part: &str) -> Option<&PartOverride> {
        self.parts.iter().find(|p| p.rule == rule && p.part == part)
    }

    fn part_mut(&mut self, rule: &str, part: &str) -> &mut PartOverride {
        let i = match self
            .parts
            .iter()
            .position(|p| p.rule == rule && p.part == part)
        {
            Some(i) => i,
            None => {
                self.parts.push(PartOverride {
                    rule: rule.to_owned(),
                    part: part.to_owned(),
                    ..PartOverride::default()
                });
                self.parts.len() - 1
            }
        };
        &mut self.parts[i]
    }

    /// The overridden value (JSON text) of `path` on the part, if any.
    pub fn field(&self, rule: &str, part: &str, path: &str) -> Option<&str> {
        self.part(rule, part)?
            .fields
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.value.as_str())
    }

    /// Overrides `path` on the part with `value` (JSON text).
    pub fn set_field(&mut self, rule: &str, part: &str, path: &str, value: impl Into<String>) {
        let value = value.into();
        let p = self.part_mut(rule, part);
        match p.fields.iter_mut().find(|f| f.path == path) {
            Some(f) => f.value = value,
            None => p.fields.push(FieldOverride {
                path: path.to_owned(),
                value,
            }),
        }
    }

    /// Removes the override of `path` (and of fields below it) on the
    /// part. True if one was removed.
    pub fn reset_field(&mut self, rule: &str, part: &str, path: &str) -> bool {
        let Some(p) = self
            .parts
            .iter_mut()
            .find(|p| p.rule == rule && p.part == part)
        else {
            return false;
        };
        let before = p.fields.len();
        p.fields.retain(|f| !covers(path, &f.path));
        let removed = p.fields.len() != before;
        self.prune();
        removed
    }

    /// Whether the part is hidden.
    pub fn is_hidden(&self, rule: &str, part: &str) -> bool {
        self.part(rule, part).is_some_and(|p| p.hidden)
    }

    /// Hides or shows the part.
    pub fn set_hidden(&mut self, rule: &str, part: &str, hidden: bool) {
        self.part_mut(rule, part).hidden = hidden;
        self.prune();
    }

    /// Removes every override of the part. True if it had any.
    pub fn reset_part(&mut self, rule: &str, part: &str) -> bool {
        let before = self.parts.len();
        self.parts.retain(|p| !(p.rule == rule && p.part == part));
        self.parts.len() != before
    }

    /// Drops entries that override nothing.
    pub fn prune(&mut self) {
        self.parts.retain(|p| p.hidden || !p.fields.is_empty());
    }
}

/// `outer` is `inner` or a path above it (`WallGen` covers
/// `WallGen.height`, `a.b` covers `a.b[2]` but not `a.bc`).
pub fn covers(outer: &str, inner: &str) -> bool {
    inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with(['.', '[']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_cover_their_fields_only() {
        assert!(covers("WallGen", "WallGen.height"));
        assert!(covers("A.points", "A.points[2]"));
        assert!(covers("A.b", "A.b"));
        assert!(!covers("A.b", "A.bc"));
        assert!(!covers("A.b.c", "A.b"));
    }

    #[test]
    fn set_reset_and_prune() {
        let mut o = Overrides::default();
        o.set_field("r", "p", "C.x", "1");
        o.set_field("r", "p", "C.x", "2");
        o.set_field("r", "p", "C.y", "3");
        assert_eq!(o.parts.len(), 1);
        assert_eq!(o.field("r", "p", "C.x"), Some("2"));
        assert!(o.reset_field("r", "p", "C"));
        assert!(o.is_empty());
        o.set_hidden("r", "q", true);
        assert!(o.reset_part("r", "q"));
        assert!(!o.reset_part("r", "q"));
    }
}
