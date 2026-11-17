//! Spike S2: bevy_reflect on top of hecs (through `box_core::World`) for the
//! inspector, scene files, Play-mode cloning and undo.

pub mod components;
pub mod fixtures;
pub mod math;
pub mod meta;
pub mod ops;
pub mod registry;
pub mod scene;
pub mod undo;

pub use registry::Registry;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown component type `{0}`")]
    UnknownComponent(String),
    #[error("two components share the name `{0}`")]
    DuplicateName(String),
    #[error("`{0}` is registered without #[reflect(Component)]")]
    NotAComponent(String),
    #[error("no entity with uid {0}")]
    NoUid(uuid::Uuid),
    #[error("entity no longer exists")]
    NoEntity,
    #[error("entity has no `{0}` component")]
    Missing(String),
    #[error("entity already has a `{0}` component")]
    AlreadyPresent(String),
    #[error("`{0}` has no default value (#[reflect(Default)] missing)")]
    NoDefault(String),
    #[error("bad field path `{path}`: {message}")]
    Path { path: String, message: String },
    #[error("value of type `{got}` cannot be stored in `{expected}`")]
    Shape { expected: String, got: String },
    #[error("reparenting would create a cycle")]
    Cycle,
    #[error("scene: {0}")]
    Scene(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A registry with every spike component registered.
pub fn default_registry() -> Registry {
    use components::*;
    let mut r = Registry::new();
    r.register_component::<Uid>().unwrap();
    r.register_component::<Name>().unwrap();
    r.register_component::<Transform>().unwrap();
    r.register_component::<PointLight>().unwrap();
    r.register_component::<Health>().unwrap();
    r.register_component::<Npc>().unwrap();
    r
}
