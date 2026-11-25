//! box_proc: procedural geometry for box-engine (M5).
//!
//! Parameters are the source of truth; meshes are a cache
//! (docs/PROCEDURAL.md). This crate holds the generator runtime (the
//! `Generator` trait, the `Procedural` component, cache keys and the
//! throttled flush) and the geometry building blocks generators use (mesh
//! building, the 2D polygon kernel, extrude, lathe, splines, sweep, split
//! and repeat). It depends on `box_core` only.
//!
//! Runtime modules: [`generator`] (the trait, outputs, registry bridge),
//! [`inputs`] (declared dependencies), [`procedural`] (the saved
//! per-object settings), [`runtime`] (stamp and throttled flush),
//! [`hash`] (cache keys and `mesh_hash`), [`rng`] (seeded randomness),
//! [`golden`] (versioned output fixtures) and [`glb`] (bake to a `.glb`).
//! Derived entities: [`derive`] (reconcile) and [`rule`] (named rules).
//! Scatter: [`scatter`] (the `Scatter` intent and pure chunked placement).
//! Built-in generators: [`builtin`] (`Primitive`, `Lathe`, `Extrude`).

pub mod builtin;
pub mod derive;
pub mod extrude;
pub mod generator;
pub mod glb;
pub mod golden;
pub mod ground;
pub mod hash;
pub mod humanoid;
pub mod inputs;
pub mod lathe;
pub mod mesh_builder;
pub mod overrides;
pub mod poly;
pub mod procedural;
pub mod rng;
pub mod rule;
pub mod runtime;
pub mod scatter;
pub mod snap;
pub mod spline;
pub mod split;
pub mod sweep;
pub mod sweep_gen;
pub mod terrain;
pub mod terrain_path;

pub use builtin::{Extrude, Hole, Lathe, LatheShape, Outline, register_builtin_generators};
pub use generator::{
    ColliderHint, GenContext, GenOutput, Generator, GeneratorInfo, GeneratorRegistry,
    GeneratorType, HintBox, ReflectGenerator,
};
pub use ground::{FollowGround, Ground, GroundSample, GroundView};
pub use hash::mesh_hash;
pub use inputs::{InputId, Inputs};
pub use overrides::{FieldOverride, Overrides, PartOverride};
pub use procedural::{Procedural, register_proc_types};
pub use rng::Rng;
pub use runtime::{
    AppGeneratorExt, FlushStats, ProcClock, ProcPlugin, ProcRuntime, ProcSettings, ProcState,
    flush, pending, settle,
};
pub use scatter::{Scatter, ScatterMask, ScatterSpecies, SpeciesSource};
pub use spline::{ControlPoint, Spline, SplineMode};
pub use sweep_gen::{Sweep, SweepOrientation, SweepProfile};
pub use terrain::{Terrain, TerrainChunk, TerrainLayers, TerrainStamp};
pub use terrain_path::TerrainPath;
