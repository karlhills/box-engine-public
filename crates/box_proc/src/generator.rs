//! The [`Generator`] trait, its output ([`GenOutput`]), its context
//! ([`GenContext`]) and the registry bridge ([`ReflectGenerator`],
//! [`GeneratorRegistry`]).
//!
//! A generator is a saved, reflected component holding parameters, plus a
//! pure function from those parameters (and declared inputs) to a mesh.
//! The runtime ([`crate::runtime`]) regenerates it when its parameters, its
//! entity's [`Procedural`](crate::Procedural) seed or an input change.
//!
//! ```
//! use box_core::prelude::*;
//! use box_core::reflect::bevy_reflect;
//! use box_proc::{GenContext, GenOutput, Generator, GeneratorRegistry};
//!
//! /// A flat slab.
//! #[derive(Reflect, Clone, PartialEq, Debug)]
//! #[reflect(Component, Default, Clone)]
//! pub struct Slab {
//!     /// Edge length.
//!     pub size: f32,
//! }
//!
//! impl Default for Slab {
//!     fn default() -> Self {
//!         Self { size: 2.0 }
//!     }
//! }
//!
//! impl Generator for Slab {
//!     const VERSION: u32 = 1;
//!     fn generate(&self, _ctx: &GenContext) -> GenOutput {
//!         if !(self.size > 0.0) {
//!             return GenOutput::empty().warn("size must be above 0");
//!         }
//!         GenOutput::mesh(MeshData::cuboid(Vec3::new(self.size, 0.2, self.size)))
//!     }
//! }
//!
//! let mut registry = Registry::with_core_types();
//! registry.register_generator::<Slab>().unwrap();
//! let slab = registry.generator("Slab").unwrap();
//! assert_eq!(slab.label(), "Slab");
//! assert_eq!(slab.category(), "Procedural");
//! ```

use std::any::TypeId;

use bevy_reflect::{FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypePath, Typed};
use box_core::asset_ref::AssetRef;
use box_core::identity::Uid;
use box_core::mesh::MeshData;
use box_core::reflect::{ComponentType, ReflectError, Registry, Value};
use box_core::world::{Component, Entity, World};
use glam::{Quat, Vec3};

use crate::inputs::{Inputs, ResolvedInput};
use crate::rng::Rng;
use crate::runtime;

/// A procedural object type: saved parameters plus a pure function that
/// turns them into geometry. Implemented by a reflected component:
///
/// - `#[derive(Reflect, Clone, PartialEq)]`, `#[reflect(Component, Default, Clone)]`,
///   a `Default` impl (the parameters a new object starts with) and,
///   for engine and plugin crates, `#[type_path = "..."]`. Field metadata
///   (`@Range`, `@Unit`, `@Step`) and doc comments drive the inspector as
///   for any component.
/// - Registered in the game plugin's `Plugin::register` with
///   `registry.register_generator::<T>()` ([`GeneratorRegistry`]). That is
///   all a game does: the engine's runtime regenerates every registered
///   generator type, and the editor lists it under Add ▸ [`CATEGORY`](Self::CATEGORY).
///
/// `PartialEq` is how the runtime notices edits cheaply: every frame each
/// generator is compared with a cached copy (no hashing, no allocation). A
/// parameter that compares unequal to itself (a NaN) costs one parameter
/// hash per frame for that entity, never a regeneration.
///
/// **Purity.** [`generate`](Self::generate) must be deterministic: no world
/// access, no globals, no time, no iteration over hash maps, and no
/// randomness except [`GenContext::rng`] (seeded from `Procedural::seed`).
/// It validates its parameters and degrades with a warning rather than
/// panicking; a panic is caught anyway and reported as a warning, and the
/// object then draws nothing.
pub trait Generator:
    Component
    + Reflect
    + FromReflect
    + TypePath
    + Typed
    + GetTypeRegistration
    + Default
    + Clone
    + PartialEq
{
    /// The output version. **Bump it whenever the output changes for the
    /// same parameters and seed** (a fix, a new UV layout, more segments by
    /// default). It is part of the cache key, and the golden harness
    /// ([`crate::golden`]) fails when output changes without a bump.
    const VERSION: u32;

    /// Names of the material slots the output's material groups index,
    /// in slot order (`MaterialGroup::material_slot`), e.g.
    /// `&["Wall", "Trim", "Glass"]`. Shown next to `Procedural::materials`.
    const MATERIAL_SLOTS: &'static [&'static str] = &["Surface"];

    /// The editor menu this generator is listed under: Add ▸ `CATEGORY` ▸
    /// its name.
    const CATEGORY: &'static str = "Procedural";

    /// Declare what this generator reads besides its own parameters:
    /// components of other entities by [`Uid`] (a spline) and loaded
    /// assets (a profile). The runtime watches them and regenerates this
    /// object when one changes; [`GenContext::component`] and
    /// [`GenContext::asset`] give the values to `generate`. A missing input
    /// adds a warning; `generate` still runs and decides what to draw.
    ///
    /// Neighbour facts (which walls touch) are not inputs: a game's rule
    /// system writes them into parameters (docs/PROCEDURAL.md section 2).
    fn inputs(&self, _inputs: &mut Inputs) {}

    /// Build the geometry. Pure and deterministic (see the trait docs).
    fn generate(&self, ctx: &GenContext) -> GenOutput;
}

/// What [`Generator::generate`] produces.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenOutput {
    /// The mesh, its material groups indexed by [`Generator::MATERIAL_SLOTS`]
    /// (no groups: everything in slot 0). `None` draws nothing.
    pub mesh: Option<MeshData>,
    /// Problems worth telling the user ("opening too close to the corner,
    /// dropped"). Stored on the entity ([`ProcState::warnings`](crate::ProcState::warnings))
    /// and logged once each time they change (the editor shows them as
    /// toasts and in the Console).
    pub warnings: Vec<String>,
    /// A cheap stand-in collider. Stored on the entity
    /// ([`ProcState::collider_hint`](crate::ProcState::collider_hint)); M5
    /// does not feed it to physics yet (see [`ColliderHint`]).
    pub collider: ColliderHint,
}

impl GenOutput {
    /// Output that draws `mesh`.
    pub fn mesh(mesh: MeshData) -> Self {
        Self {
            mesh: Some(mesh),
            ..Self::default()
        }
    }

    /// Output that draws nothing.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Adds a warning: `GenOutput::mesh(m).warn("steps clamped to 64")`.
    pub fn warn(mut self, message: impl Into<String>) -> Self {
        self.warnings.push(message.into());
        self
    }

    /// Sets the collider hint.
    pub fn with_collider(mut self, collider: ColliderHint) -> Self {
        self.collider = collider;
        self
    }
}

/// A cheap collider shape a generator can suggest for its output, for use
/// while the accurate mesh collider would be too slow to rebuild (dragging)
/// or instead of it.
///
/// **M5 status:** stored on the entity
/// ([`ProcState::collider_hint`](crate::ProcState::collider_hint)) for
/// games to read; the physics glue does not consume it yet. Background
/// collider rebuilds that use it during interaction are scheduled with the
/// background-job pipeline (docs/PROCEDURAL.md section 5, step 2).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ColliderHint {
    /// Use the generated mesh itself.
    #[default]
    FromMesh,
    /// Not solid (decoration).
    None,
    /// Oriented boxes in the entity's local space, e.g. one per wall
    /// segment or stair step.
    Boxes(Vec<HintBox>),
    /// The mesh's surface as a height grid (terrain chunks). Physics uses
    /// it for `Mesh` colliders instead of the triangles
    /// (`box_core::mesh_asset::GeneratedMesh::heights`).
    Heights(std::sync::Arc<box_core::mesh_asset::HeightGrid>),
}

/// One box of [`ColliderHint::Boxes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HintBox {
    pub center: Vec3,
    pub half_extents: Vec3,
    pub rotation: Quat,
}

/// What [`Generator::generate`] may read: the seed and the inputs it
/// declared in [`Generator::inputs`], resolved by the runtime.
pub struct GenContext<'a> {
    seed: u64,
    owner: Option<Uid>,
    inputs: &'a [ResolvedInput<'a>],
}

impl GenContext<'static> {
    /// A context with `seed`, no owner and no inputs (tests, golden
    /// fixtures, tools).
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            owner: None,
            inputs: &[],
        }
    }
}

impl<'a> GenContext<'a> {
    pub(crate) fn with_inputs(
        seed: u64,
        owner: Option<Uid>,
        inputs: &'a [ResolvedInput<'a>],
    ) -> Self {
        Self {
            seed,
            owner,
            inputs,
        }
    }

    /// The entity's `Procedural::seed` (0 without a `Procedural`).
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The `Uid` of the entity being generated (as [`Inputs::owner`]).
    pub fn owner(&self) -> Option<Uid> {
        self.owner
    }

    /// The world transform of the entity `uid`, if it was declared with
    /// [`Inputs::world_transform`] and the entity exists.
    pub fn world_transform(&self, uid: Uid) -> Option<glam::Affine3A> {
        self.inputs
            .iter()
            .find(|i| i.id.is_world_transform(uid))?
            .value
            .as_ref()?
            .downcast_ref::<box_core::transform::GlobalTransform>()
            .map(|g| g.0)
    }

    /// The world's ground, if the generator declared it with
    /// [`Inputs::ground`] and the world has one.
    pub fn ground(&self) -> Option<&crate::ground::Ground> {
        self.inputs
            .iter()
            .find(|i| i.id == crate::inputs::InputId::Ground)?
            .value
            .as_ref()?
            .downcast_ref()
    }

    /// A fresh random generator seeded with [`seed`](Self::seed); the only
    /// source of randomness a generator may use.
    pub fn rng(&self) -> Rng {
        Rng::new(self.seed)
    }

    /// The `C` component of the entity `uid`, if it was declared with
    /// [`Inputs::component`] and exists.
    pub fn component<C: 'static>(&self, uid: Uid) -> Option<&C> {
        self.inputs
            .iter()
            .find(|i| i.id.is_component::<C>(uid))?
            .value
            .as_ref()?
            .downcast_ref()
    }

    /// The loaded asset `asset` of type `A`, if it was declared with
    /// [`Inputs::asset`] and is loaded.
    pub fn asset<A: 'static>(&self, asset: &AssetRef) -> Option<&A> {
        self.inputs
            .iter()
            .find(|i| i.id.is_asset::<A>(asset))?
            .value
            .as_ref()?
            .downcast_ref()
    }
}

/// A generator type's description for menus and tools.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorInfo {
    /// The component's short type name (scene files, field paths).
    pub name: &'static str,
    /// A readable name for menus: `"RoundTower"` becomes `"Round Tower"`.
    pub label: String,
    /// [`Generator::CATEGORY`].
    pub category: &'static str,
    /// [`Generator::VERSION`].
    pub version: u32,
    /// [`Generator::MATERIAL_SLOTS`].
    pub material_slots: &'static [&'static str],
}

/// Type data attached by [`GeneratorRegistry::register_generator`]: a
/// generator type's description and type-erased entry points, so tools
/// (the editor, the CLI) can list generators and run one without the
/// concrete type.
#[derive(Clone)]
pub struct ReflectGenerator {
    info: GeneratorInfo,
    pub(crate) type_id: TypeId,
    pub(crate) has: fn(&World, Entity) -> bool,
    pub(crate) stamp: fn(&mut World, &mut runtime::Stamps),
    pub(crate) lost: fn(&mut World) -> Vec<Entity>,
    pub(crate) forget: fn(&mut World, Entity),
    pub(crate) inputs_on: fn(&World, Entity, &mut Inputs) -> bool,
    pub(crate) params_hash: fn(&World, Entity) -> Option<u64>,
    pub(crate) generate_on: fn(&World, Entity, &GenContext) -> Option<GenOutput>,
    generate_value: fn(&dyn PartialReflect, &GenContext) -> Option<GenOutput>,
}

impl std::fmt::Debug for ReflectGenerator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReflectGenerator")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl ReflectGenerator {
    /// The entry points of `T`.
    pub fn of<T: Generator>() -> Self {
        Self {
            info: GeneratorInfo {
                name: T::short_type_path(),
                label: friendly_name(T::short_type_path()),
                category: T::CATEGORY,
                version: T::VERSION,
                material_slots: T::MATERIAL_SLOTS,
            },
            type_id: TypeId::of::<T>(),
            has: |w, e| w.has::<T>(e),
            stamp: runtime::stamp::<T>,
            lost: runtime::lost::<T>,
            forget: runtime::forget::<T>,
            inputs_on: |w, e, inputs| match w.get::<T>(e) {
                Some(g) => {
                    g.inputs(inputs);
                    true
                }
                None => false,
            },
            params_hash: |w, e| w.get::<T>(e).map(|g| crate::hash::reflect_hash(&*g)),
            generate_on: |w, e, ctx| w.get::<T>(e).map(|g| run_generate(&*g, ctx)),
            generate_value: |value, ctx| {
                // `from_reflect` fills missing fields with defaults, so it
                // would accept a value of another struct type.
                let represented = value.get_represented_type_info().map(|i| i.type_id());
                if represented.is_some_and(|t| t != TypeId::of::<T>()) {
                    return None;
                }
                T::from_reflect(value).map(|g| run_generate(&g, ctx))
            },
        }
    }

    pub fn info(&self) -> &GeneratorInfo {
        &self.info
    }

    /// Runs the generator on a reflected value of its type (concrete or
    /// dynamic, as the deserializer produces). `None` when `value` is not
    /// of this type. Panics are caught and returned as a warning.
    pub fn generate(&self, value: &dyn PartialReflect, ctx: &GenContext) -> Option<GenOutput> {
        (self.generate_value)(value, ctx)
    }
}

/// `generator.generate(ctx)` with panics turned into a warning.
pub(crate) fn run_generate<T: Generator>(generator: &T, ctx: &GenContext) -> GenOutput {
    match box_core::panic_guard::catch(|| generator.generate(ctx)) {
        Ok(out) => out,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            GenOutput::empty().warn(format!(
                "generator panicked: {message} (a bug in `{}`; it should warn and degrade instead)",
                T::short_type_path()
            ))
        }
    }
}

/// `"RoundTower"` → `"Round Tower"`, `"UVSphere"` → `"UV Sphere"`.
pub fn friendly_name(type_name: &str) -> String {
    let chars: Vec<char> = type_name.chars().collect();
    let mut out = String::with_capacity(type_name.len() + 4);
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(*c);
    }
    out
}

/// A registered generator type (see [`GeneratorRegistry::generators`]).
#[derive(Clone, Copy)]
pub struct GeneratorType<'r> {
    component: ComponentType<'r>,
    data: &'r ReflectGenerator,
}

impl<'r> GeneratorType<'r> {
    /// The component's short type name.
    pub fn name(&self) -> &'static str {
        self.component.name()
    }

    /// The menu name ("Round Tower").
    pub fn label(&self) -> &'r str {
        &self.data.info.label
    }

    pub fn category(&self) -> &'static str {
        self.data.info.category
    }

    pub fn version(&self) -> u32 {
        self.data.info.version
    }

    pub fn material_slots(&self) -> &'static [&'static str] {
        self.data.info.material_slots
    }

    /// The type's doc comment (the menu tooltip).
    pub fn doc(&self) -> Option<&'static str> {
        self.component.doc()
    }

    /// The parameters a new object starts with.
    pub fn default_value(&self) -> Option<Value> {
        self.component.default_value()
    }

    pub fn component(&self) -> ComponentType<'r> {
        self.component
    }

    pub fn reflect(&self) -> &'r ReflectGenerator {
        self.data
    }
}

/// Generator registration and queries on the engine's [`Registry`].
pub trait GeneratorRegistry {
    /// Register generator `T`: the component type (saved in scenes, shown
    /// in the inspector) and its [`ReflectGenerator`] type data (listed by
    /// [`generators`](Self::generators), regenerated by the runtime). Call
    /// it from `Plugin::register`. Registering twice is a no-op.
    fn register_generator<T: Generator>(&mut self) -> Result<(), ReflectError>;

    /// Every registered generator type, ordered by category, then label.
    fn generators(&self) -> Vec<GeneratorType<'_>>;

    /// The generator type with this short name.
    fn generator(&self, name: &str) -> Option<GeneratorType<'_>>;
}

impl GeneratorRegistry for Registry {
    fn register_generator<T: Generator>(&mut self) -> Result<(), ReflectError> {
        self.register::<T>()?;
        self.insert_type_data::<T, _>(ReflectGenerator::of::<T>());
        Ok(())
    }

    fn generators(&self) -> Vec<GeneratorType<'_>> {
        let mut out: Vec<GeneratorType<'_>> = self
            .components()
            .filter_map(|component| {
                Some(GeneratorType {
                    component,
                    data: component.data::<ReflectGenerator>()?,
                })
            })
            .collect();
        out.sort_by(|a, b| (a.category(), a.label()).cmp(&(b.category(), b.label())));
        out
    }

    fn generator(&self, name: &str) -> Option<GeneratorType<'_>> {
        let component = self.component(name)?;
        Some(GeneratorType {
            component,
            data: component.data::<ReflectGenerator>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_names_split_words() {
        assert_eq!(friendly_name("RoundTower"), "Round Tower");
        assert_eq!(friendly_name("UVSphere"), "UV Sphere");
        assert_eq!(friendly_name("Lathe"), "Lathe");
    }
}
