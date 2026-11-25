//! Add ▸ Gameplay: the registry's presets (box_core::preset), placed at the
//! viewport centre on the surface with the `add_preset` command, as one
//! undo step. Also the scene settings a scene can add (`GameRules`,
//! `HudSettings`).

use box_core::edit::{Command, apply_commands};
use box_core::identity::Uid;
use box_core::math::Aabb;

use crate::state::EditorState;

/// A preset as menus list it.
#[derive(Clone, Debug)]
pub struct PresetEntry {
    pub name: String,
    pub title: String,
    pub category: String,
    pub description: String,
}

impl EditorState {
    /// The registered presets, by category then registration name.
    pub fn preset_entries(&self) -> Vec<PresetEntry> {
        let mut out: Vec<PresetEntry> = self
            .world
            .registry()
            .presets()
            .map(|p| PresetEntry {
                name: p.name.clone(),
                title: p.title.clone(),
                category: if p.category.is_empty() {
                    "Presets".into()
                } else {
                    p.category.clone()
                },
                description: p.description.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.category.cmp(&b.category).then(a.title.cmp(&b.title)));
        out
    }

    /// Places the preset `name` on the surface at the viewport centre and
    /// selects its root. One undo step. `None` (logged) when it fails.
    pub fn add_preset(&mut self, name: &str) -> Option<Uid> {
        self.finish_edit();
        let Some(title) = self.world.registry().preset(name).map(|p| p.title.clone()) else {
            tracing::warn!("cannot add `{name}`: it is not a registered preset");
            return None;
        };
        let at = self.place(Aabb::EMPTY, 0.0);
        let command = Command::AddPreset {
            preset: name.to_string(),
            at: Some(at.to_array()),
            name: None,
            parent: None,
        };
        let r = apply_commands(
            &mut self.world,
            &mut self.edits,
            &[command],
            &format!("Add {title}"),
        )
        .map_err(|e| *e.source);
        let created = self.report(&format!("add {title}"), r)?;
        self.changed(true);
        let root = created.first().copied();
        self.select(root);
        root
    }

    /// Saved scene resources the scene does not have yet (`GameRules`,
    /// `HudSettings`), by name: what Scene Settings can add.
    pub fn missing_scene_resources(&self) -> Vec<&'static str> {
        let registry = self.world.registry().clone();
        registry
            .resources()
            .filter(|r| r.is_saved() && !r.is_settings() && !r.is_hidden())
            .map(|r| r.name())
            .filter(|n| self.world.get_resource_reflect(n).is_err())
            .collect()
    }

    /// Adds the scene resource `name` with its defaults (one undo step).
    pub fn add_scene_resource(&mut self, name: &str) -> bool {
        self.finish_edit();
        let command = Command::SetResource {
            path: name.to_string(),
            value: serde_json::json!({}),
        };
        let r = apply_commands(
            &mut self.world,
            &mut self.edits,
            &[command],
            &format!("Add {name}"),
        )
        .map_err(|e| *e.source);
        let ok = self.report(&format!("add {name}"), r).is_some();
        if ok {
            self.changed(false);
        }
        ok
    }
}
