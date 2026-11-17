//! The `material` (`.material.json`), `shader` (`.wgsl`), `scene`
//! (`.scene.json`), `animgraph` (`.animgraph.json`) and `json` (any other
//! `.json`) importers.

use std::any::TypeId;
use std::sync::OnceLock;

use bevy_reflect::FromReflect;
use box_core::animation::AnimationGraph;
use box_core::asset_ref::AssetRef;
use box_core::material::StandardMaterial;
use box_core::reflect::Registry;
use box_core::shader_material::{ShaderMaterial, ShaderSource, parse_material_header};

use crate::error::ImportError;
use crate::importer::{ImportContext, Importer};
use crate::index::normalize_rel;
use crate::scene_asset::SceneAsset;

/// box_core's registry, for reading and writing core components as JSON.
pub(crate) fn core_registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::with_core_types)
}

/// `.material.json` → a material (the file's main asset): a
/// [`ShaderMaterial`] when the object has a `"shader"` key, otherwise a
/// [`StandardMaterial`].
///
/// The file is the reflected JSON form of the material component, as a scene
/// stores it. Texture and shader paths are relative to `assets/`; path-only
/// references get the referenced file's id filled in when it is indexed.
///
/// ```json
/// {
///   "base_color": [1.0, 1.0, 1.0, 1.0],
///   "roughness": 0.8,
///   "metallic": 0.0,
///   "emissive": [0.0, 0.0, 0.0, 1.0],
///   "base_color_texture": "textures/stone.png"
/// }
/// ```
///
/// A custom shader material (parameters by name from the shader's
/// `@material` header; missing ones use the header defaults):
///
/// ```json
/// {
///   "shader": "shaders/stripes.wgsl",
///   "params": [{ "name": "frequency", "value": { "Float": 12.0 } }]
/// }
/// ```
pub struct MaterialImporter;

impl Importer for MaterialImporter {
    fn name(&self) -> &str {
        "material"
    }

    fn extensions(&self) -> &[&str] {
        &["material.json"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let json: serde_json::Value = serde_json::from_slice(ctx.bytes())
            .map_err(|e| ImportError::Decode(format!("invalid JSON: {e}")))?;
        if json.get("shader").is_some() {
            let json = read_file_value::<ShaderMaterial>(ctx, "ShaderMaterial", &json)?;
            let value = core_registry()
                .value_from_json(TypeId::of::<ShaderMaterial>(), &json)
                .map_err(|e| ImportError::Decode(format!("not a ShaderMaterial: {e}")))?;
            let mut material = ShaderMaterial::from_reflect(value.as_ref())
                .ok_or_else(|| ImportError::Decode("not a ShaderMaterial".into()))?;
            if material.shader.is_none() {
                return Err(ImportError::Decode("\"shader\" names no file".into()));
            }
            fill_ref(ctx, &mut material.shader, "shader")?;
            ctx.add(None, material);
            return Ok(());
        }
        let json = read_file_value::<StandardMaterial>(ctx, "StandardMaterial", &json)?;
        let value = core_registry()
            .value_from_json(TypeId::of::<StandardMaterial>(), &json)
            .map_err(|e| ImportError::Decode(format!("not a StandardMaterial: {e}")))?;
        let mut material = StandardMaterial::from_reflect(value.as_ref())
            .ok_or_else(|| ImportError::Decode("not a StandardMaterial".into()))?;
        for texture in [
            &mut material.base_color_texture,
            &mut material.normal_texture,
            &mut material.orm_texture,
            &mut material.emissive_texture,
        ] {
            fill_ref(ctx, texture, "texture")?;
        }
        ctx.add(None, material);
        Ok(())
    }
}

/// A file holding one value of type `T` (named `name` for migrations),
/// ready to deserialize: upgraded through the type's steps
/// (`box_core::migrate`), without `"$version"`, and without top-level fields
/// `T` does not have (warned about; the editor writes them back when it
/// saves the file).
fn read_file_value<T: 'static>(
    ctx: &ImportContext,
    name: &str,
    json: &serde_json::Value,
) -> Result<serde_json::Value, ImportError> {
    let box_core::migrate::ValueFile {
        value,
        unknown: extra,
        ..
    } = box_core::migrate::read_value_file(core_registry(), name, TypeId::of::<T>(), json)
        .map_err(|e| ImportError::Decode(e.to_string()))?;
    if !extra.is_empty() {
        let names: Vec<&str> = extra.keys().map(String::as_str).collect();
        tracing::warn!(
            "{}: `{name}` has no field{} `{}` (kept in the file, ignored)",
            ctx.path(),
            if names.len() == 1 { "" } else { "s" },
            names.join("`, `")
        );
    }
    Ok(value)
}

/// Normalizes a path reference inside a material file and fills in the
/// referenced file's id when it is indexed (`what` names it in messages).
fn fill_ref(ctx: &ImportContext, r: &mut AssetRef, what: &str) -> Result<(), ImportError> {
    let Some(path) = r.path.clone() else {
        return Ok(());
    };
    let path = normalize_rel(&path).ok_or_else(|| {
        ImportError::Decode(format!(
            "{what} path `{path}` is not inside the assets folder"
        ))
    })?;
    if r.id.is_none() {
        r.id = ctx.asset_ref(&path).id;
        if r.id.is_none() {
            tracing::warn!("{}: {what} `{path}` not found", ctx.path());
        }
    }
    r.path = Some(path);
    Ok(())
}

/// `.wgsl` → [`ShaderSource`] (the file's main asset): a material shader
/// (see `box_core::shader_material`). The `@material` parameter header is
/// validated at import, so a broken header fails the import (the last good
/// version stays in use on hot reload) with `file:line` in the message; the
/// rest of the WGSL is compiled by the renderer, which keeps the previous
/// pipeline and reports the error when it fails.
pub struct ShaderImporter;

impl Importer for ShaderImporter {
    fn name(&self) -> &str {
        "shader"
    }

    fn extensions(&self) -> &[&str] {
        &["wgsl"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let text = std::str::from_utf8(ctx.bytes())
            .map_err(|e| ImportError::Decode(format!("not UTF-8 text: {e}")))?;
        parse_material_header(text).map_err(|e| {
            ImportError::Decode(format!(
                "{}:{}: bad @material header: {}",
                ctx.path(),
                e.line,
                e.message
            ))
        })?;
        let source = ShaderSource::new(ctx.path(), text);
        ctx.add(None, source);
        Ok(())
    }
}

/// `.scene.json` → [`SceneAsset`] (the file's main asset): the parsed,
/// version-checked JSON tree. Component data is not checked against a
/// registry here; that happens when the scene is loaded or instantiated.
pub struct SceneImporter;

impl Importer for SceneImporter {
    fn name(&self) -> &str {
        "scene"
    }

    fn extensions(&self) -> &[&str] {
        &["scene.json"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let json: serde_json::Value = serde_json::from_slice(ctx.bytes())
            .map_err(|e| ImportError::Decode(format!("invalid JSON: {e}")))?;
        let scene = SceneAsset::from_json(json)?;
        ctx.add(None, scene);
        Ok(())
    }
}

/// A parsed JSON data file (the `json` importer's asset): project settings
/// (`settings/<Type>.json`), input action maps (`settings/input.actions.json`)
/// and other data a game reads. Hot reload replaces the value like any asset.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonAsset(pub serde_json::Value);

/// `.json` → [`JsonAsset`] (the file's main asset): any JSON file without a
/// more specific importer (`.material.json` and `.scene.json` have their
/// own). Only the syntax is checked; what the data means is up to its
/// reader (`box_engine::settings` for settings files).
pub struct JsonImporter;

impl Importer for JsonImporter {
    fn name(&self) -> &str {
        "json"
    }

    fn extensions(&self) -> &[&str] {
        &["json"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let json: serde_json::Value = serde_json::from_slice(ctx.bytes())
            .map_err(|e| ImportError::Decode(format!("invalid JSON: {e}")))?;
        ctx.add(None, JsonAsset(json));
        Ok(())
    }
}

/// `.animgraph.json` → [`AnimationGraph`] (the file's main asset): the
/// reflected JSON form of an animation state machine graph, as an
/// `AnimationStateMachine` stores it inline in a scene. Clip paths are
/// relative to `assets/`; path-only references get the file's id filled in.
///
/// ```json
/// {
///   "states": [
///     { "name": "Idle", "clip": "models/hero.glb#Animation/Idle", "speed": 1.0, "looping": true },
///     { "name": "Walk", "clip": "models/hero.glb#Animation/Walk", "speed": 1.0, "looping": true }
///   ],
///   "transitions": [
///     { "from": "Idle", "to": "Walk", "duration": 0.2, "exit_time": 0.0,
///       "conditions": [{ "parameter": "speed", "test": { "Greater": 0.5 } }] }
///   ],
///   "start": "Idle"
/// }
/// ```
pub struct AnimationGraphImporter;

impl Importer for AnimationGraphImporter {
    fn name(&self) -> &str {
        "animgraph"
    }

    fn extensions(&self) -> &[&str] {
        &["animgraph.json"]
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let json: serde_json::Value = serde_json::from_slice(ctx.bytes())
            .map_err(|e| ImportError::Decode(format!("invalid JSON: {e}")))?;
        let json = read_file_value::<AnimationGraph>(ctx, "AnimationGraph", &json)?;
        let value = core_registry()
            .value_from_json(TypeId::of::<AnimationGraph>(), &json)
            .map_err(|e| ImportError::Decode(format!("not an AnimationGraph: {e}")))?;
        let mut graph = AnimationGraph::from_reflect(value.as_ref())
            .ok_or_else(|| ImportError::Decode("not an AnimationGraph".into()))?;
        for state in &mut graph.states {
            fill_ref(ctx, &mut state.clip, "clip")?;
        }
        for problem in graph.problems() {
            tracing::warn!("{}: {problem}", ctx.path());
        }
        ctx.add(None, graph);
        Ok(())
    }
}
