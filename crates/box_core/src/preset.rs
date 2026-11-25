//! Presets: named recipes of saved entities (a Pickup, a Moving Platform)
//! that the editor's Add menu, `box scene apply` (`add_preset`) and MCP
//! place the same way. A preset is a small entity tree in the scene-file
//! representation; placing it spawns fresh entities through the
//! [`edit`](crate::edit) layer, so it is one undo step like any command.
//!
//! In component values, the string `"$uid:<entity name>"` stands for the
//! uid of the preset entity with that name (a player's `camera` field), and
//! is replaced when the preset is placed.
//!
//! ```json
//! { "name": "pickup", "title": "Pickup", "category": "Gameplay",
//!   "description": "A spinning coin the player collects by touching it.",
//!   "entities": [
//!     { "name": "Pickup", "components": { "Spinner": {}, "Primitive": { "shape": "Sphere" } } }
//!   ] }
//! ```

use serde::Deserialize;
use serde_json::{Map, Value as Json};

use crate::edit::Command;
use crate::identity::Uid;
use crate::world::World;

/// A named recipe of entities, registered with
/// [`Registry::register_preset`](crate::reflect::Registry::register_preset).
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    /// The id commands use (`"pickup"`).
    pub name: String,
    /// What menus show (`"Pickup"`).
    pub title: String,
    /// Menu group (`"Gameplay"`).
    #[serde(default)]
    pub category: String,
    /// One sentence for tooltips and agents.
    #[serde(default)]
    pub description: String,
    /// The entities; the first is the root, placed where asked. Others name
    /// their `parent` among the preset's entities (none: a root of its own).
    pub entities: Vec<PresetEntity>,
}

/// One entity of a [`Preset`].
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetEntity {
    pub name: String,
    /// Name of its parent within the preset.
    #[serde(default)]
    pub parent: Option<String>,
    /// Components in the scene-file representation (fields left out take
    /// their defaults).
    #[serde(default)]
    pub components: Map<String, Json>,
}

impl Preset {
    /// Parses a preset from JSON text.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let p: Preset = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if p.entities.is_empty() {
            return Err(format!("preset `{}` has no entities", p.name));
        }
        for e in &p.entities[1..] {
            if let Some(parent) = &e.parent
                && !p.entities.iter().any(|o| &o.name == parent)
            {
                return Err(format!(
                    "preset `{}`: `{}` names parent `{parent}`, which is not in the preset",
                    p.name, e.name
                ));
            }
        }
        Ok(p)
    }

    /// Whether any of its entities has the component (or names the resource)
    /// `name`.
    pub fn uses(&self, name: &str) -> bool {
        self.entities
            .iter()
            .any(|e| e.components.contains_key(name))
    }

    /// The spawn commands that place it: fresh uids, every entity without a
    /// parent moved by `at` (its own translation added), the first under
    /// `parent` and named `name` or the preset's root name made unique in
    /// `world` ("Pickup 2").
    pub fn spawn_commands(
        &self,
        world: &World,
        at: Option<[f32; 3]>,
        name: Option<&str>,
        parent: Option<&str>,
    ) -> Vec<Command> {
        let uids: Vec<(String, Uid)> = self
            .entities
            .iter()
            .map(|e| (e.name.clone(), Uid::new()))
            .collect();
        let uid_of = |n: &str| uids.iter().find(|(m, _)| m == n).map(|(_, u)| *u);
        let mut names: Vec<String> = Vec::new();
        let mut commands = Vec::new();
        for (i, e) in self.entities.iter().enumerate() {
            let mut components = e.components.clone();
            for v in components.values_mut() {
                replace_uids(v, &uid_of);
            }
            let entity_name = if i == 0 {
                match name {
                    Some(n) => n.to_string(),
                    None => unique_name(world, &e.name),
                }
            } else {
                e.name.clone()
            };
            let parent = if i == 0 {
                parent.map(str::to_string)
            } else {
                e.parent.as_deref().and_then(uid_of).map(|u| u.to_string())
            };
            if e.parent.is_none()
                && let Some(at) = at
            {
                place_root(&mut components, at);
            }
            names.push(entity_name.clone());
            commands.push(Command::Spawn {
                uid: Some(uids[i].1),
                name: entity_name,
                parent,
                index: None,
                components,
            });
        }
        commands
    }
}

/// The script version of a behaviour component (Convert to Script, the
/// `eject_behaviour` command): a `.luau` file that does what the component
/// does, and how the component's fields become the script's properties.
#[derive(Clone, Copy, Debug)]
pub struct BehaviourScript {
    /// The component's short name (`"Mover"`).
    pub component: &'static str,
    /// Where the script goes, relative to `assets/`
    /// (`"scripts/behaviours/mover.luau"`). Every converted copy uses the
    /// same file; each keeps its own values as properties.
    pub path: &'static str,
    /// The script's text, written when the project lacks the file (an
    /// existing file, perhaps edited, is kept).
    pub source: &'static str,
    /// The component value (scene JSON) to the script's property records
    /// (`[{ "name": "speed", "value": { "Number": 2.0 } }]`); an error says
    /// why this copy cannot be converted.
    pub properties: fn(&Json) -> Result<Vec<Json>, String>,
}

/// `name`, or `name 2`, `name 3`... whichever no entity in `world` has.
pub fn unique_name(world: &World, name: &str) -> String {
    let taken = |n: &str| world.find_by_name(n).is_some();
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{name} {i}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| name.to_string())
}

/// Adds `at` to the root's `Transform.translation` (creating it).
fn place_root(components: &mut Map<String, Json>, at: [f32; 3]) {
    let t = components
        .entry("Transform")
        .or_insert_with(|| Json::Object(Map::new()));
    let Some(t) = t.as_object_mut() else {
        return;
    };
    let base: [f64; 3] = t
        .get("translation")
        .and_then(Json::as_array)
        .map(|a| {
            let f = |i: usize| a.get(i).and_then(Json::as_f64).unwrap_or(0.0);
            [f(0), f(1), f(2)]
        })
        .unwrap_or([0.0; 3]);
    t.insert(
        "translation".into(),
        serde_json::json!([
            base[0] + at[0] as f64,
            base[1] + at[1] as f64,
            base[2] + at[2] as f64
        ]),
    );
}

fn replace_uids(v: &mut Json, uid_of: &dyn Fn(&str) -> Option<Uid>) {
    match v {
        Json::String(s) => {
            if let Some(n) = s.strip_prefix("$uid:")
                && let Some(u) = uid_of(n)
            {
                *s = u.to_string();
            }
        }
        Json::Array(a) => a.iter_mut().for_each(|x| replace_uids(x, uid_of)),
        Json::Object(o) => o.values_mut().for_each(|x| replace_uids(x, uid_of)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_expands_with_uid_links() {
        let p = Preset::from_json(
            r#"{ "name": "pair", "title": "Pair", "entities": [
                { "name": "A", "components": { "Transform": { "translation": [0, 1, 0] } } },
                { "name": "B", "parent": "A", "components": { "Link": { "to": "$uid:A" } } }
            ] }"#,
        )
        .unwrap();
        let world = World::new();
        let cmds = p.spawn_commands(&world, Some([2.0, 0.0, 3.0]), None, None);
        assert_eq!(cmds.len(), 2);
        let Command::Spawn {
            uid: Some(a),
            components,
            ..
        } = &cmds[0]
        else {
            panic!()
        };
        assert_eq!(
            components["Transform"]["translation"],
            serde_json::json!([2.0, 1.0, 3.0])
        );
        let Command::Spawn {
            parent, components, ..
        } = &cmds[1]
        else {
            panic!()
        };
        assert_eq!(parent.as_deref(), Some(a.to_string().as_str()));
        assert_eq!(components["Link"]["to"], serde_json::json!(a.to_string()));
    }

    mod engine {
        use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};
        #[derive(Reflect, Default, Clone)]
        #[reflect(Component, Default)]
        pub struct Mover {
            pub speed: f32,
        }
    }
    mod game {
        use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};
        #[derive(Reflect, Default, Clone)]
        #[reflect(Component, Default)]
        pub struct Mover {
            pub lanes: u32,
        }
    }

    #[test]
    fn a_game_type_takes_over_a_yielding_name_and_its_presets_go() {
        let mut r = crate::reflect::Registry::new();
        r.register_yielding::<engine::Mover>().unwrap();
        r.register_preset(
            Preset::from_json(
                r#"{ "name": "lift", "title": "Lift", "entities": [ { "name": "Lift", "components": { "Mover": {} } } ] }"#,
            )
            .unwrap(),
        );
        r.register::<game::Mover>().unwrap();
        let c = r.component("Mover").unwrap();
        assert_eq!(c.type_id(), std::any::TypeId::of::<game::Mover>());
        assert!(
            r.preset("lift").is_none(),
            "the preset needed the engine's Mover"
        );
        // Only once: a third type with the name is an error again.
        mod other {
            use crate::reflect::{Reflect, ReflectComponent, ReflectDefault};
            #[derive(Reflect, Default, Clone)]
            #[reflect(Component, Default)]
            pub struct Mover;
        }
        assert!(r.register::<other::Mover>().is_err());
    }

    #[test]
    fn rejects_unknown_parent() {
        let e = Preset::from_json(
            r#"{ "name": "x", "title": "X", "entities": [ { "name": "A" }, { "name": "B", "parent": "C" } ] }"#,
        )
        .unwrap_err();
        assert!(e.contains("parent `C`"), "{e}");
    }
}
