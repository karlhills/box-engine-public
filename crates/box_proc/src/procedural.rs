//! The [`Procedural`] component: per-object settings shared by every
//! generator (seed, materials by slot, shadows).

use box_core::asset_ref::AssetRef;
use box_core::reflect::{Reflect, ReflectComponent, ReflectDefault, Registry};

/// Settings of a procedural object, next to its generator component
/// (`Lathe`, `Wall`, ...). Optional: a generator entity without one uses
/// the defaults (seed 0, the entity's own material, shadows on).
///
/// **Materials.** The output's material groups index the generator's
/// material slots. Slot `i` draws `materials[i]` when that names an asset;
/// a missing or empty entry draws the entity's own material, with the same
/// precedence as a `Primitive`: a `MaterialRef`, else an inline
/// `ShaderMaterial`, else an inline `StandardMaterial`, else the default
/// material. So a one-slot generator with a `StandardMaterial` behaves like
/// a primitive, and a many-slot generator can override slots one by one.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Procedural {
    /// Seed of the generator's random choices; change it for another
    /// variation of the same parameters.
    pub seed: u64,
    /// Material assets by material slot, in the generator's slot order. An
    /// empty entry uses the entity's own material.
    pub materials: Vec<AssetRef>,
    /// Whether the generated mesh casts shadows.
    pub cast_shadows: bool,
}

impl Default for Procedural {
    fn default() -> Self {
        Self {
            seed: 0,
            materials: Vec::new(),
            cast_shadows: true,
        }
    }
}

impl Procedural {
    /// Default settings with `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            seed,
            ..Self::default()
        }
    }
}

/// Register box_proc's saved types: `Procedural`, the `Spline` and
/// `Scatter` components,
/// the `Sweep` generator and the built-in generators (`Primitive`,
/// `Lathe`, `Extrude`; see [`crate::builtin`]).
/// `box_engine::engine_registry` calls it.
pub fn register_proc_types(registry: &mut Registry) {
    use crate::generator::GeneratorRegistry;
    let results = [
        registry.register::<Procedural>(),
        registry.register::<crate::overrides::Overrides>(),
        registry.register::<crate::spline::Spline>(),
        registry.register::<crate::scatter::Scatter>(),
        registry.register_generator::<crate::sweep_gen::Sweep>(),
    ];
    for e in results.into_iter().filter_map(Result::err) {
        tracing::error!("box_proc type registration failed: {e}");
    }
    crate::builtin::register_builtin_generators(registry);
}
