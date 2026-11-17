//! The saved [`Script`] component and its property values.

use bevy_reflect::Reflect;
use box_core::asset_ref::AssetRef;
use box_core::color::Color;
use box_core::identity::Uid;
use box_core::reflect::{ReflectComponent, ReflectDefault};
use glam::Vec3;

/// Runs a Luau script (a `.luau` file under `assets/`) on this entity while
/// the game plays (the player, Play mode; never while editing). The script's
/// declared properties appear in the inspector; values that differ from the
/// script's defaults are stored here and saved with the scene.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Script {
    /// The `.luau` file (relative to `assets/`).
    pub source: AssetRef,
    /// Property values by name, overriding the script's declared defaults.
    /// A property missing here, or holding a value of the wrong type, uses
    /// the default; values for properties the script no longer declares are
    /// kept but unused.
    #[reflect(default)]
    pub properties: Vec<ScriptProperty>,
}

impl Script {
    /// A script file (relative to `assets/`) with its default properties.
    pub fn new(path: &str) -> Self {
        Self {
            source: AssetRef::path(path),
            properties: Vec::new(),
        }
    }

    /// Overrides property `name` (builder form).
    pub fn with(mut self, name: &str, value: ScriptValue) -> Self {
        self.set(name, value);
        self
    }

    /// The stored value of property `name`, if it is overridden.
    pub fn get(&self, name: &str) -> Option<&ScriptValue> {
        self.properties
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.value)
    }

    /// Overrides property `name`.
    pub fn set(&mut self, name: &str, value: ScriptValue) {
        match self.properties.iter_mut().find(|p| p.name == name) {
            Some(p) => p.value = value,
            None => self.properties.push(ScriptProperty {
                name: name.to_owned(),
                value,
            }),
        }
    }

    /// Drops the override of property `name` (back to the script default).
    pub fn reset(&mut self, name: &str) {
        self.properties.retain(|p| p.name != name);
    }
}

/// One overridden script property.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ScriptProperty {
    /// The name the script declares it under.
    pub name: String,
    pub value: ScriptValue,
}

/// A script property value. Colours are linear; an entity is referenced by
/// its uid, an asset by reference.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ScriptValue {
    Number(f64),
    Integer(i64),
    Boolean(bool),
    String(String),
    Vector(Vec3),
    Color(Color),
    Entity(Option<Uid>),
    Asset(AssetRef),
}

impl Default for ScriptValue {
    fn default() -> Self {
        Self::Number(0.0)
    }
}

impl ScriptValue {
    /// The property type this value belongs to.
    pub fn ty(&self) -> PropType {
        match self {
            Self::Number(_) => PropType::Number,
            Self::Integer(_) => PropType::Integer,
            Self::Boolean(_) => PropType::Boolean,
            Self::String(_) => PropType::String,
            Self::Vector(_) => PropType::Vector,
            Self::Color(_) => PropType::Color,
            Self::Entity(_) => PropType::Entity,
            Self::Asset(_) => PropType::Asset,
        }
    }
}

/// The type of a declared script property (`type = "number"` in the
/// script's `properties` table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropType {
    Number,
    Integer,
    Boolean,
    String,
    Vector,
    Color,
    Entity,
    Asset,
}

impl PropType {
    /// Every type, in the order the docs list them.
    pub const ALL: [PropType; 8] = [
        Self::Number,
        Self::Integer,
        Self::Boolean,
        Self::String,
        Self::Vector,
        Self::Color,
        Self::Entity,
        Self::Asset,
    ];

    /// The name scripts use (`"number"`, `"vector"`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::String => "string",
            Self::Vector => "vector",
            Self::Color => "color",
            Self::Entity => "entity",
            Self::Asset => "asset",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The value used when a declaration gives no default.
    pub fn zero(self) -> ScriptValue {
        match self {
            Self::Number => ScriptValue::Number(0.0),
            Self::Integer => ScriptValue::Integer(0),
            Self::Boolean => ScriptValue::Boolean(false),
            Self::String => ScriptValue::String(String::new()),
            Self::Vector => ScriptValue::Vector(Vec3::ZERO),
            Self::Color => ScriptValue::Color(Color::WHITE),
            Self::Entity => ScriptValue::Entity(None),
            Self::Asset => ScriptValue::Asset(AssetRef::NONE),
        }
    }

    /// The Luau type of the value scripts see in `self.props`.
    pub fn luau_type(self) -> &'static str {
        match self {
            Self::Number | Self::Integer => "number",
            Self::Boolean => "boolean",
            Self::String | Self::Asset => "string",
            Self::Vector => "vector",
            Self::Color => "Color",
            Self::Entity => "Entity?",
        }
    }
}

/// One property a script declares: name, type, default and inspector hints.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDecl {
    pub name: String,
    pub ty: PropType,
    pub default: ScriptValue,
    /// Slider range (`min`/`max`), for numbers.
    pub range: Option<(f64, f64)>,
    /// Drag step, for numbers.
    pub step: Option<f64>,
    /// Display unit (`"m/s"`).
    pub unit: Option<String>,
    /// Tooltip.
    pub doc: Option<String>,
}

impl PropertyDecl {
    /// The value an instance uses: `stored` when it has this property's
    /// type, else the default.
    pub fn effective(&self, stored: Option<&ScriptValue>) -> ScriptValue {
        match stored {
            Some(v) if v.ty() == self.ty => v.clone(),
            // Whole numbers typed into a number property and the reverse.
            Some(ScriptValue::Integer(i)) if self.ty == PropType::Number => {
                ScriptValue::Number(*i as f64)
            }
            _ => self.default.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_are_set_read_and_reset() {
        let mut s = Script::new("scripts/spin.luau").with("speed", ScriptValue::Number(2.0));
        assert_eq!(s.get("speed"), Some(&ScriptValue::Number(2.0)));
        s.set("speed", ScriptValue::Number(3.0));
        assert_eq!(s.properties.len(), 1);
        s.reset("speed");
        assert!(s.get("speed").is_none());
        let decl = PropertyDecl {
            name: "speed".into(),
            ty: PropType::Number,
            default: ScriptValue::Number(1.0),
            range: None,
            step: None,
            unit: None,
            doc: None,
        };
        assert_eq!(
            decl.effective(Some(&ScriptValue::Boolean(true))),
            ScriptValue::Number(1.0)
        );
        assert_eq!(
            decl.effective(Some(&ScriptValue::Integer(4))),
            ScriptValue::Number(4.0)
        );
        for t in PropType::ALL {
            assert_eq!(PropType::parse(t.name()), Some(t));
            assert_eq!(t.zero().ty(), t);
        }
    }
}
