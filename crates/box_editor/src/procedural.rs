//! Procedural objects in the editor: Add ▸ `<category>` ▸ `<generator>`
//! for every registered generator (engine and game types alike, listed
//! from the registry), and Edit ▸ Bake to Mesh.

use box_core::identity::Uid;
use box_core::math::Aabb;
use box_core::reflect::{PartialReflect, Value};
use box_core::transform::Transform;
use box_core::world::Placement;
use box_engine::box_proc::{GenContext, GeneratorRegistry, ProcState};

use crate::state::EditorState;

/// A generator the Add menus offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratorEntry {
    /// The component's short name.
    pub name: &'static str,
    pub label: String,
    pub category: &'static str,
    pub doc: Option<&'static str>,
}

/// A generator the Add menus list under another name, created together with
/// a component it reads on its own entity.
struct Preset {
    generator: &'static str,
    label: &'static str,
    doc: &'static str,
    /// Added with its default value.
    companion: &'static str,
}

/// Add ▸ Shapes ▸ Spline is a `Sweep` reading the entity's own `Spline`
/// (a curved tube to start from; Edit Spline shapes it).
const PRESETS: &[Preset] = &[Preset {
    generator: "Sweep",
    label: "Spline",
    doc: "A tube along an editable spline: shape it with the Edit Spline tool, \
          change its profile (rectangle, circle, your own) in the inspector",
    companion: "Spline",
}];

fn preset(generator: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.generator == generator)
}

impl EditorState {
    /// Every registered generator, ordered by category, then label, except
    /// `Primitive`: the Add menus offer it once per shape (Cube, Sphere,
    /// ...; [`NewEntity`](crate::state::NewEntity)).
    pub fn generator_entries(&self) -> Vec<GeneratorEntry> {
        let registry = self.world.registry();
        let mut out: Vec<GeneratorEntry> = registry
            .generators()
            .into_iter()
            .filter(|g| g.name() != "Primitive")
            .map(|g| match preset(g.name()) {
                Some(p) if registry.component(p.companion).is_some() => GeneratorEntry {
                    name: g.name(),
                    label: p.label.to_string(),
                    category: g.category(),
                    doc: Some(p.doc),
                },
                _ => GeneratorEntry {
                    name: g.name(),
                    label: g.label().to_string(),
                    category: g.category(),
                    doc: g.doc(),
                },
            })
            .collect();
        out.sort_by(|a, b| (a.category, &a.label).cmp(&(b.category, &b.label)));
        out
    }

    /// Add ▸ `<generator>`: a new root entity with the generator's default
    /// parameters, a `Procedural` and a default `StandardMaterial` (one undo
    /// step), selected. It is placed like a primitive (see
    /// [`EditorState::placement`]) by the bounds of its default output,
    /// generated once here. `None` (logged) when `name` is not a registered
    /// generator. Add ▸ Shapes ▸ Spline (the `Sweep` generator) also gets a
    /// default `Spline` on the entity.
    pub fn add_generator(&mut self, name: &str) -> Option<Uid> {
        self.finish_edit();
        let registry = self.world.registry().clone();
        let Some(generator) = registry.generator(name) else {
            tracing::warn!("cannot add `{name}`: it is not a registered generator");
            return None;
        };
        let Some(params) = generator.default_value() else {
            tracing::warn!("cannot add `{name}`: it has no default value (#[reflect(Default)])");
            return None;
        };
        let preset = preset(name).filter(|p| registry.component(p.companion).is_some());
        let label = preset.map_or_else(|| generator.label().to_string(), |p| p.label.to_string());
        // Generated once here for its bounds, so the new object rests on
        // the surface and beside existing meshes, as primitives do. A
        // generator that reads inputs (a Sweep and its Spline) has none
        // yet and may give no mesh: it is placed like an empty.
        let bounds = generator
            .reflect()
            .generate(params.as_ref(), &GenContext::new(0))
            .and_then(|out| out.mesh)
            .map_or(Aabb::EMPTY, |m| m.bounds());
        let lift = if bounds.is_empty() {
            0.0
        } else {
            -bounds.min.y
        };
        let mut owned: Vec<(&'static str, Value)> = vec![
            (
                "Transform",
                Box::new(Transform::from_translation(self.place(bounds, lift))),
            ),
            (generator.name(), params),
        ];
        let companion = preset.map(|p| p.companion);
        for extra in companion
            .into_iter()
            .chain(["Procedural", "StandardMaterial"])
        {
            if let Some(v) = registry.component(extra).and_then(|c| c.default_value()) {
                owned.push((extra, v));
            }
        }
        let components: Vec<(&str, &dyn PartialReflect)> =
            owned.iter().map(|(n, v)| (*n, v.as_ref())).collect();
        let uid = Uid::new();
        let entity_name = self.unique_name(&label);
        let r = self.edits.spawn(
            &mut self.world,
            uid,
            &entity_name,
            Placement::last(None),
            &components,
        );
        self.report(&format!("add {label}"), r)?;
        self.changed(true);
        self.select(Some(uid));
        Some(uid)
    }

    /// Why Bake to Mesh is unavailable, if it is.
    pub(crate) fn bake_availability(&self) -> Result<(), String> {
        if self.is_playing() {
            return Err("Stop Play first".into());
        }
        if self.project.is_none() || self.asset_server().is_none() {
            return Err("Open a project first (the mesh is saved under assets/baked/)".into());
        }
        let Some(e) = self.primary_entity() else {
            return Err("Select a procedural object first".into());
        };
        // The runtime state outlives a removed generator until the next
        // frame, so the component itself is checked.
        let state = self.world.get::<ProcState>(e);
        match state {
            Some(s) if !self.world.component_names(e).contains(&s.generator()) => {
                Err("The selection is not a procedural object".into())
            }
            Some(s) if s.mesh().is_some() || s.is_dirty() => Ok(()),
            Some(_) => Err("The generator produced no mesh".into()),
            None => Err("The selection is not a procedural object".into()),
        }
    }

    /// Edit ▸ Bake to Mesh: writes the primary selection's generated mesh to
    /// `assets/baked/<name>.glb` and replaces its generator with a
    /// `MeshAsset` drawing that file (one undo step; see
    /// `box_engine::bake`). Returns the new file's path.
    pub fn bake_selection(&mut self) -> Option<String> {
        if let Err(why) = self.bake_availability() {
            tracing::warn!("Bake to Mesh: {why}");
            return None;
        }
        self.finish_edit();
        let uid = *self.selection.first()?;
        let server = self.asset_server()?.clone();
        let path = box_engine::bake::bake_path(&self.world, &server, uid);
        match box_engine::bake::bake_to_mesh(&mut self.world, &mut self.edits, &server, uid, &path)
        {
            Ok(baked) => {
                self.changed(false);
                self.notify(&format!("Baked to {}", baked.path));
                Some(baked.path)
            }
            Err(e) => {
                tracing::warn!("Bake to Mesh: {e}");
                None
            }
        }
    }
}
