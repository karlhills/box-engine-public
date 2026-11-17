//! Core runtime for box-engine.
//!
//! Contains the app loop, the [`World`](world::World) (entities, components and
//! resources), transforms and the parent/child hierarchy, time, input state,
//! CPU mesh data and simple asset stores, plus the headless data model used by
//! the editor, the CLI and the runtime: reflection ([`reflect`]), persistent
//! identity ([`identity`]), scene files ([`scene`]) and the undoable command
//! layer ([`edit`]). Nothing in this crate touches the GPU or the windowing
//! system.

pub mod action;
pub mod animation;
pub mod app;
pub mod asset_ref;
pub mod assets;
pub mod color;
pub mod derived;
pub mod edit;
pub mod event;
mod hooks;
pub mod identity;
pub mod image;
pub mod input;
pub mod interpolate;
pub mod material;
pub mod math;
pub mod mesh;
pub mod mesh_asset;
pub mod migrate;
pub mod panic_guard;
pub mod pmath;
pub mod prefab;
pub mod primitive;
pub mod reflect;
pub mod scene;
mod schedule;
pub mod shader_material;
pub mod time;
pub mod timer;
pub mod transform;
pub mod tween;
pub mod visibility;
pub mod world;

pub use glam;

/// Register box_core's scene types: `Uid`, `Name`, `Transform`, `MeshAsset`,
/// `MaterialRef`, `StandardMaterial`, `ShaderMaterial`, `PrefabInstance`, `Interpolated`,
/// `AnimationPlayer`, `AnimationStateMachine`, `SkinBinding` and `Primitive` (and, through their fields, `Color`, glam vectors and the
/// primitive `Shape`). `GlobalTransform`, `Parent` and `Children` are derived
/// data and are never registered or saved.
pub fn register_core_types(registry: &mut reflect::Registry) {
    // These names are unique within box_core, so registration cannot fail.
    let results = [
        registry.register::<identity::Uid>(),
        registry.register::<identity::Name>(),
        registry.register::<transform::Transform>(),
        registry.register::<primitive::Primitive>(),
        registry.register::<material::StandardMaterial>(),
        registry.register::<mesh_asset::MeshAsset>(),
        registry.register::<mesh_asset::MaterialRef>(),
        registry.register::<shader_material::ShaderMaterial>(),
        registry.register::<prefab::PrefabInstance>(),
        registry.register::<interpolate::Interpolated>(),
        registry.register::<animation::AnimationPlayer>(),
        registry.register::<animation::AnimationStateMachine>(),
        registry.register::<animation::SkinBinding>(),
        registry.register::<visibility::Visible>(),
        registry.register::<visibility::Transient>(),
    ];
    debug_assert!(results.iter().all(Result::is_ok), "{results:?}");
    registry.register_value::<color::Color>();
    registry.register_value::<asset_ref::AssetId>();
    registry.register_value::<asset_ref::AssetRef>();
    registry.register_value::<material::AlphaMode>();
    registry.register_value::<shader_material::ShaderParamValue>();
}

pub mod prelude {
    pub use crate::action::{
        ActionKind, ActionMap, Actions, ActionsPlugin, Axis1Binding, Axis2Binding, Binding,
        GamepadStick,
    };
    pub use crate::animation::{
        AnimationClip, AnimationGraph, AnimationGraphState, AnimationPlayer, AnimationPlayerState,
        AnimationStateMachine, AnimationStateMachineState, AnimationTransition, ConditionTest,
        ParameterValue, SkinBinding, SkinnedMesh,
    };
    pub use crate::app::{App, AppExit, FixedTime, Plugin, Stage, SystemBuilder};
    pub use crate::asset_ref::{AssetId, AssetRef};
    pub use crate::assets::{AssetLookup, Assets, Handle};
    pub use crate::color::Color;
    pub use crate::event::{EventCursor, Events};
    pub use crate::identity::{Name, Uid};
    pub use crate::image::{ColorSpace, ImageData};
    pub use crate::input::{
        CursorGrab, CursorOptions, GamepadAxis, GamepadButton, GamepadId, Input, KeyCode,
        Modifiers, MouseButton,
    };
    pub use crate::interpolate::{Interpolated, interpolated_transform};
    pub use crate::material::{AlphaMode, StandardMaterial, WindSway, WindWeight};
    pub use crate::math::{Aabb, Ray};
    pub use crate::mesh::{MaterialGroup, MeshData};
    pub use crate::mesh_asset::{MaterialRef, MeshAsset};
    pub use crate::prefab::PrefabInstance;
    pub use crate::primitive::{Primitive, Shape};
    pub use crate::reflect::{
        Reflect, ReflectComponent, ReflectDefault, ReflectResource, ReflectVariantDefaults,
        Registry, VariantDefaults,
    };
    pub use crate::time::Time;
    pub use crate::timer::Timer;
    pub use crate::transform::{Children, GlobalTransform, Parent, Transform};
    pub use crate::tween::{Ease, Repeat, Tween, TweenValue};
    pub use crate::visibility::{Transient, Visible};
    pub use crate::world::{Deferred, Entity, World};
    pub use glam::{Affine3A, EulerRot, Mat3, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};
}
