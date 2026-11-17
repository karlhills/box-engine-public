//! Reset, copy and paste of inspector values (DX review E13). No UI here;
//! `panes::inspector` draws the menus and markers.
//!
//! - **Defaults** come from the registry: the component's (or resource's)
//!   `default_value`, and inside it the value at the field path. A field of
//!   an enum variant the default does not have has no default.
//! - **Clipboard** text is JSON, so values travel between editor instances
//!   (and can be read by people and agents):
//!   `{"box": "value", "type": "<type path>", "value": <scene JSON>}` for a
//!   field, `{"box": "component", "component": "<name>", "value": ...}` for a
//!   whole component. Pasting checks the type: a value pastes only into a
//!   field of the same type, a component only into the same component.
//! - **Edits** (reset, paste) go through the edit stack as one undo step,
//!   for every selected entity at once.

use box_core::identity::Uid;
use box_core::reflect::bevy_reflect::ReflectPath;
use box_core::reflect::{PartialReflect, ReflectRef, Registry, Value, clone_value};
use serde_json::{Value as Json, json};

use crate::state::EditorState;

/// The full type path of a value (its represented type for dynamic values).
pub fn type_path_of(v: &dyn PartialReflect) -> &str {
    v.get_represented_type_info()
        .map_or_else(|| v.reflect_type_path(), |i| i.type_path())
}

/// The default value of a registered component or resource by name.
pub fn target_default(registry: &Registry, name: &str) -> Option<Value> {
    registry
        .component(name)
        .and_then(|c| c.default_value())
        .or_else(|| registry.resource(name).and_then(|r| r.default_value()))
}

/// The default of the field at `path` (`"StandardMaterial.roughness"`, or a
/// whole component `"Transform"`), from the registry's default value of its
/// component or resource. `None` without a default (no `Default`, or a field
/// of an enum variant the default does not have).
pub fn field_default(registry: &Registry, path: &str) -> Option<Value> {
    let split = path.find(['.', '[']).unwrap_or(path.len());
    let (name, rest) = path.split_at(split);
    let root = target_default(registry, name)?;
    if rest.is_empty() {
        return Some(root);
    }
    let field = rest.reflect_element(root.as_ref()).ok()?;
    clone_value(field).ok()
}

/// The field at `path` (`Component.field…`, or just `Component` for the
/// whole value) inside `root`, the value of component or resource
/// `Component`.
pub fn field_of(root: &dyn PartialReflect, path: &str) -> Option<Value> {
    let split = path.find(['.', '[']).unwrap_or(path.len());
    let rest = &path[split..];
    if rest.is_empty() {
        return clone_value(root).ok();
    }
    let field = rest.reflect_element(root).ok()?;
    clone_value(field).ok()
}

/// Whether `value` (the field at `path`) differs from its default; `None`
/// without a default. Enum rows compare their variant only (their fields
/// have rows of their own).
pub fn differs_from_default(
    registry: &Registry,
    path: &str,
    value: &dyn PartialReflect,
) -> Option<bool> {
    let default = field_default(registry, path)?;
    Some(match (value.reflect_ref(), default.reflect_ref()) {
        (ReflectRef::Enum(a), ReflectRef::Enum(b)) => a.variant_name() != b.variant_name(),
        _ => value.reflect_partial_eq(default.as_ref()) != Some(true),
    })
}

/// What the clipboard holds, parsed.
#[derive(Clone, Debug, PartialEq)]
pub enum Clip {
    /// A field value of the type with this path.
    Value { type_path: String, value: Json },
    /// A whole component.
    Component { component: String, value: Json },
}

impl Clip {
    /// Parses clipboard text; `None` for anything that is not box JSON.
    pub fn parse(text: &str) -> Option<Self> {
        let j: Json = serde_json::from_str(text.trim()).ok()?;
        let value = j.get("value")?.clone();
        match j.get("box")?.as_str()? {
            "value" => Some(Self::Value {
                type_path: j.get("type")?.as_str()?.to_owned(),
                value,
            }),
            "component" => Some(Self::Component {
                component: j.get("component")?.as_str()?.to_owned(),
                value,
            }),
            _ => None,
        }
    }

    /// A short description for menus: `Vec3`, `Standard Material`.
    pub fn describe(&self) -> String {
        match self {
            Self::Value { type_path, .. } => type_path
                .rsplit("::")
                .next()
                .unwrap_or(type_path)
                .to_owned(),
            Self::Component { component, .. } => crate::inspect::friendly_name(component),
        }
    }
}

/// Clipboard text for a field value.
pub fn copy_value_text(registry: &Registry, value: &dyn PartialReflect) -> Result<String, String> {
    let json = registry.value_to_json(value).map_err(|e| e.to_string())?;
    Ok(json!({"box": "value", "type": type_path_of(value), "value": json}).to_string())
}

/// Clipboard text for a whole component.
pub fn copy_component_text(
    registry: &Registry,
    component: &str,
    value: &dyn PartialReflect,
) -> Result<String, String> {
    let json = registry.value_to_json(value).map_err(|e| e.to_string())?;
    Ok(json!({"box": "component", "component": component, "value": json}).to_string())
}

/// The clipboard's value as a value for a field currently holding `like`:
/// refused (with the reason) unless the clipboard holds a value of the same
/// type.
pub fn paste_value(
    registry: &Registry,
    clip: &Clip,
    like: &dyn PartialReflect,
) -> Result<Value, String> {
    let Clip::Value { type_path, value } = clip else {
        return Err("the clipboard holds a component, not a value".into());
    };
    let want = type_path_of(like);
    if type_path != want {
        return Err(format!(
            "the clipboard holds a {}, not a {}",
            short(type_path),
            short(want)
        ));
    }
    let ty = like
        .get_represented_type_info()
        .map(|i| i.type_id())
        .ok_or_else(|| format!("{want} cannot be pasted"))?;
    registry
        .value_from_json(ty, value)
        .map_err(|e| e.to_string())
}

/// The clipboard's component: its registered name and value. With
/// `expect`, refused unless it is that component.
pub fn paste_component(
    registry: &Registry,
    clip: &Clip,
    expect: Option<&str>,
) -> Result<(&'static str, Value), String> {
    let Clip::Component { component, value } = clip else {
        return Err("the clipboard holds a value, not a component".into());
    };
    let ty = registry
        .component(component)
        .ok_or_else(|| format!("{component} is not a registered component"))?;
    if let Some(e) = expect
        && e != ty.name()
    {
        return Err(format!(
            "the clipboard holds a {}, not a {}",
            crate::inspect::friendly_name(ty.name()),
            crate::inspect::friendly_name(e)
        ));
    }
    let type_id = registry
        .type_registry()
        .get_with_type_path(ty.type_path())
        .map(|r| r.type_id())
        .ok_or_else(|| format!("{component} cannot be pasted"))?;
    let v = registry
        .value_from_json(type_id, value)
        .map_err(|e| e.to_string())?;
    Ok((ty.name(), v))
}

fn short(type_path: &str) -> &str {
    type_path.rsplit("::").next().unwrap_or(type_path)
}

/// The editor's clipboard: the text last copied here, and the system
/// clipboard's text as last read by the windowed runner (which copies are
/// written to through egui). Pasting prefers the system text, so values
/// copied in another editor instance paste here.
#[derive(Clone, Debug, Default)]
pub struct Clipboard {
    pub(crate) copied: Option<String>,
    /// The system clipboard, when the runner can read it.
    pub(crate) system: Option<String>,
    /// The inspector showed paste targets this frame: the windowed runner
    /// refreshes [`Self::system`] (at most twice a second).
    pub(crate) wanted: bool,
}

impl Clipboard {
    /// The text a paste uses.
    pub fn text(&self) -> Option<&str> {
        self.system.as_deref().or(self.copied.as_deref())
    }

    /// The parsed clip, if the clipboard holds box JSON.
    pub fn clip(&self) -> Option<Clip> {
        self.text().and_then(Clip::parse)
    }

    /// Sets the system clipboard text read by the runner.
    pub fn set_system_text(&mut self, text: Option<String>) {
        self.system = text;
    }

    /// Whether the runner should read the system clipboard (clears the flag).
    pub fn take_wanted(&mut self) -> bool {
        std::mem::take(&mut self.wanted)
    }
}

/// `"Cube: Reset Roughness"`, `"3 entities: Paste Translation"`,
/// `"Environment: Reset Exposure"`.
pub(crate) fn verb_label(subject: Option<&str>, verb: &str, path: &str) -> String {
    let field = match path.contains(['.', '[']) {
        true => crate::inspect::field_label(path),
        // A whole component: `Standard Material`.
        false => crate::inspect::friendly_name(path),
    };
    match subject {
        Some(s) => format!("{s}: {verb} {field}"),
        None => match path.split_once('.') {
            Some((resource, _)) => format!("{resource}: {verb} {field}"),
            None => format!("{verb} {field}"),
        },
    }
}

impl EditorState {
    /// Remembers `text` as copied (egui writes the system clipboard).
    pub fn copy_to_clipboard(&mut self, text: String) {
        self.clipboard.system = None;
        self.clipboard.copied = Some(text);
    }

    /// The editor's clipboard.
    pub fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    /// The clipboard, mutably (the windowed runner sets the system text).
    pub fn clipboard_mut(&mut self) -> &mut Clipboard {
        &mut self.clipboard
    }

    /// The label subject of `uids`: a name, "3 entities", or `None` (the
    /// scene's resources).
    pub(crate) fn subject_of(&self, uids: &[Uid]) -> Option<String> {
        match uids {
            [] => None,
            [u] => self.world.entity_by_uid(*u).map(|e| self.display_name(e)),
            many => Some(format!("{} entities", many.len())),
        }
    }

    /// Sets the field `path` to `value` on every entity in `uids` (on the
    /// scene's resource when empty) as one undo step labelled with `verb`
    /// (`"Cube: Reset Roughness"`). Returns whether it was applied.
    pub fn set_field_step(
        &mut self,
        uids: &[Uid],
        path: &str,
        value: &dyn PartialReflect,
        verb: &str,
    ) -> bool {
        let label = verb_label(self.subject_of(uids).as_deref(), verb, path);
        self.begin_edit(label);
        let ok = if uids.is_empty() {
            self.set_resource_field(path, value)
        } else {
            self.set_field_on(uids, path, value, None)
        };
        self.finish_edit();
        ok
    }

    /// Resets the field (or whole component) `path` of every entity in
    /// `uids` (of the scene's resource when empty) to its registry default,
    /// as one undo step. Returns false when it has no default.
    pub fn reset_field(&mut self, uids: &[Uid], path: &str) -> bool {
        let Some(default) = field_default(self.world.registry(), path) else {
            tracing::warn!("{path} has no default value");
            return false;
        };
        self.set_field_step(uids, path, default.as_ref(), "Reset")
    }

    /// Pastes the clipboard's value into the field `path` of every entity in
    /// `uids` (the scene's resource when empty), as one undo step. Refused,
    /// with a warning, unless the clipboard holds a value of the field's type.
    pub fn paste_field(&mut self, uids: &[Uid], path: &str) -> bool {
        let like = self.field_value(uids, path);
        let clip = self.clipboard.clip();
        let result = match (clip, like) {
            (None, _) => Err("the clipboard holds no value".to_owned()),
            (_, None) => Err(format!("{path} cannot be read")),
            (Some(clip), Some(like)) => paste_value(self.world.registry(), &clip, like.as_ref()),
        };
        match result {
            Ok(v) => self.set_field_step(uids, path, v.as_ref(), "Paste"),
            Err(e) => {
                tracing::warn!(
                    "cannot paste into {}: {e}",
                    crate::inspect::field_label(path)
                );
                false
            }
        }
    }

    /// The value at `path` of the primary of `uids` (the scene's resource
    /// when empty).
    pub(crate) fn field_value(&self, uids: &[Uid], path: &str) -> Option<Value> {
        let p = self.world.field_path(path).ok()?;
        match uids.last() {
            None => self.world.get_resource_field(&p).ok(),
            Some(u) => {
                let e = self.world.entity_by_uid(*u)?;
                self.world.get_field(e, &p).ok()
            }
        }
    }

    /// Copies the value at `path` (of the primary of `uids`) to the
    /// clipboard as text; returns it for egui's clipboard.
    pub fn copy_field(&mut self, uids: &[Uid], path: &str) -> Option<String> {
        let v = self.field_value(uids, path)?;
        match copy_value_text(self.world.registry(), v.as_ref()) {
            Ok(text) => {
                self.copy_to_clipboard(text.clone());
                Some(text)
            }
            Err(e) => {
                tracing::warn!("cannot copy {path}: {e}");
                None
            }
        }
    }

    /// Copies the component (or resource) `name` of the primary of `uids`.
    pub fn copy_component(&mut self, uids: &[Uid], name: &str) -> Option<String> {
        let v = self.field_value(uids, name)?;
        let text = match uids.is_empty() {
            false => copy_component_text(self.world.registry(), name, v.as_ref()),
            // Resources travel as a value of their type.
            true => copy_value_text(self.world.registry(), v.as_ref()),
        };
        match text {
            Ok(text) => {
                self.copy_to_clipboard(text.clone());
                Some(text)
            }
            Err(e) => {
                tracing::warn!("cannot copy {name}: {e}");
                None
            }
        }
    }

    /// Pastes the clipboard's component over `name` on every entity in
    /// `uids` (all its values), as one undo step. Refused unless the
    /// clipboard holds that component. For a resource (`uids` empty) the
    /// clipboard must hold a value of its type.
    pub fn paste_component_values(&mut self, uids: &[Uid], name: &str) -> bool {
        let Some(clip) = self.clipboard.clip() else {
            tracing::warn!("cannot paste {name}: the clipboard holds no component");
            return false;
        };
        if uids.is_empty() {
            return self.paste_field(uids, name);
        }
        match paste_component(self.world.registry(), &clip, Some(name)) {
            Ok((name, v)) => self.set_field_step(uids, name, v.as_ref(), "Paste"),
            Err(e) => {
                tracing::warn!(
                    "cannot paste into {}: {e}",
                    crate::inspect::friendly_name(name)
                );
                false
            }
        }
    }

    /// The clipboard's component, when the entities in `uids` can get it
    /// from Paste as New Component: registered and missing on some of them.
    pub fn pastable_new_component(&self, uids: &[Uid]) -> Option<&'static str> {
        let clip = self.clipboard.clip()?;
        let Clip::Component { component, .. } = &clip else {
            return None;
        };
        let name = self.world.registry().component(component)?.name();
        if name == "Uid" || name == "Name" {
            return None;
        }
        uids.iter()
            .filter_map(|u| self.world.entity_by_uid(*u))
            .any(|e| !self.world.component_names(e).contains(&name))
            .then_some(name)
    }

    /// Adds the clipboard's component, with its values, to every entity in
    /// `uids` that lacks it, as one undo step.
    pub fn paste_as_new_component(&mut self, uids: &[Uid]) -> bool {
        let Some(clip) = self.clipboard.clip() else {
            return false;
        };
        let (name, value) = match paste_component(self.world.registry(), &clip, None) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("cannot paste a component: {e}");
                return false;
            }
        };
        let missing: Vec<Uid> = uids
            .iter()
            .copied()
            .filter(|u| {
                self.world
                    .entity_by_uid(*u)
                    .is_some_and(|e| !self.world.component_names(e).contains(&name))
            })
            .collect();
        if missing.is_empty() {
            return false;
        }
        let subject = self.subject_of(&missing).unwrap_or_default();
        self.begin_edit(format!(
            "{subject}: Paste {}",
            crate::inspect::friendly_name(name)
        ));
        let mut ok = true;
        for u in missing {
            let r = self
                .edits
                .add_component(&mut self.world, u, name, Some(value.as_ref()));
            ok &= self.report(&format!("add {name}"), r).is_some();
        }
        self.finish_edit();
        self.changed(true);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::prelude::{Shape, Transform};
    use glam::Vec3;

    #[test]
    fn field_of_reads_a_field_or_the_whole_value() {
        use box_core::transform::Transform;
        let t = Transform::from_xyz(1.0, 2.0, 3.0);
        let y = field_of(&t, "Transform.translation.y").unwrap();
        assert_eq!(y.try_downcast_ref::<f32>(), Some(&2.0));
        let whole = field_of(&t, "Transform").unwrap();
        assert_eq!(whole.try_downcast_ref::<Transform>(), Some(&t));
        assert!(field_of(&t, "Transform.nope").is_none());
    }

    #[test]
    fn defaults_come_from_the_registry_by_path() {
        let r = box_engine::engine_registry();
        let d = field_default(&r, "StandardMaterial.roughness").unwrap();
        assert_eq!(d.try_downcast_ref::<f32>(), Some(&0.5));
        let t = field_default(&r, "Transform.scale").unwrap();
        assert_eq!(t.try_downcast_ref::<Vec3>(), Some(&Vec3::ONE));
        assert!(field_default(&r, "Transform").is_some());
        // The default Primitive is a cuboid: a sphere's radius has none.
        assert!(field_default(&r, "Primitive.shape.size").is_some());
        assert!(field_default(&r, "Primitive.shape.radius").is_none());
        assert!(field_default(&r, "Environment.exposure").is_some());
        assert_eq!(
            differs_from_default(&r, "StandardMaterial.roughness", &0.5f32),
            Some(false)
        );
        assert_eq!(
            differs_from_default(&r, "StandardMaterial.roughness", &0.2f32),
            Some(true)
        );
        let sphere = Shape::Sphere {
            radius: 1.0,
            sectors: 8,
            stacks: 4,
        };
        assert_eq!(
            differs_from_default(&r, "Primitive.shape", &sphere),
            Some(true)
        );
        assert_eq!(
            differs_from_default(&r, "Transform", &Transform::default()),
            Some(false)
        );
        assert_eq!(
            differs_from_default(&r, "Transform.translation", &Vec3::Y),
            Some(true),
            "vectors are compared by value"
        );
    }

    #[test]
    fn clipboard_text_round_trips_and_checks_types() {
        let r = box_engine::engine_registry();
        let text = copy_value_text(&r, &Vec3::new(1.0, 2.0, 3.0)).unwrap();
        let clip = Clip::parse(&text).unwrap();
        assert_eq!(clip.describe(), "Vec3");
        let v = paste_value(&r, &clip, &Vec3::ZERO).unwrap();
        assert_eq!(
            <Vec3 as box_core::reflect::bevy_reflect::FromReflect>::from_reflect(v.as_ref()),
            Some(Vec3::new(1.0, 2.0, 3.0))
        );
        let err = paste_value(&r, &clip, &0.5f32).unwrap_err();
        assert!(err.contains("Vec3") && err.contains("f32"), "{err}");
        assert!(Clip::parse("hello").is_none());
        assert!(Clip::parse("{\"box\": \"value\"}").is_none());

        let m = box_core::material::StandardMaterial {
            roughness: 0.25,
            ..Default::default()
        };
        let text = copy_component_text(&r, "StandardMaterial", &m).unwrap();
        let clip = Clip::parse(&text).unwrap();
        let (name, v) = paste_component(&r, &clip, Some("StandardMaterial")).unwrap();
        assert_eq!(name, "StandardMaterial");
        let back = <box_core::material::StandardMaterial as box_core::reflect::bevy_reflect::FromReflect>::from_reflect(v.as_ref()).unwrap();
        assert_eq!(back, m);
        assert!(paste_component(&r, &clip, Some("Transform")).is_err());
        assert!(paste_value(&r, &clip, &0.5f32).is_err());
    }
}
