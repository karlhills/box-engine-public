//! Material shaders (`ShaderMaterial`): composing a user WGSL file with the
//! engine library, validating it with naga, the parameter layout naga
//! computes, and packing parameter values into the uniform.
//!
//! A composed module is: the engine library (`#include`d: globals, object,
//! lighting, shadows, fog, forward, prepass, wind, material_api), the header rewritten as a
//! plain struct bound as `material` at group 2, the user's file (header lines
//! blanked so line numbers in errors match the file), a pass-through `vertex`
//! hook when the file has none, and the entry points in
//! custom_material.wgsl.

use std::collections::HashMap;

use box_core::assets::Assets;
use box_core::shader_material::{
    ShaderMaterial, ShaderParamDecl, ShaderSource, parse_material_header,
};

use crate::pipelines::{Layouts, PipelineCache, ShaderKey, create_material_pipeline};
use crate::renderer::catch_errors;
use crate::shaders::{ShaderSources, validate_wgsl};

/// File name of the generated part of a composed module (in error messages).
const PRELUDE: &str = "<material prelude>";

/// One parameter and its byte offset in the uniform.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParamSlot {
    pub decl: ShaderParamDecl,
    pub offset: u32,
}

/// A composed and validated material shader.
#[derive(Debug)]
pub(crate) struct ComposedShader {
    pub wgsl: String,
    pub params: Vec<ParamSlot>,
    /// Size of the parameter uniform (a multiple of 16, at least 16).
    pub uniform_size: u64,
    /// The file defines a `vertex` hook (it may move vertices, so the
    /// selection outline draws with the shader's own vertex stage).
    pub has_vertex: bool,
    /// The file calls `scene_depth`: materials using it run the depth
    /// prepass while visible.
    pub reads_scene_depth: bool,
}

/// Checks a material shader file (`.wgsl` defining `fn surface`) the way the
/// renderer composes it, with naga and without a GPU (`box check`). Errors
/// name the file and line, as at runtime.
pub fn check_material_shader(name: &str, text: &str) -> Result<(), String> {
    compose(&ShaderSources::embedded(), &ShaderSource::new(name, text)).map(|_| ())
}

/// Builds and validates the module for `source`. Errors name the user's file
/// and line.
pub(crate) fn compose(
    library: &ShaderSources,
    source: &ShaderSource,
) -> Result<ComposedShader, String> {
    let name = if source.name.is_empty() {
        "material.wgsl"
    } else {
        source.name.as_str()
    };
    // The user's file must not shadow an engine file in the include table.
    let file = if library.contains(name) {
        format!("project:{name}")
    } else {
        name.to_string()
    };
    let header = parse_material_header(&source.text).map_err(|e| format!("{name}: {e}"))?;
    let (struct_name, struct_wgsl, params) = match &header {
        Some(h) => (h.struct_name.clone(), h.wgsl_struct(), h.params.clone()),
        None => (
            "MaterialParams".to_string(),
            "struct MaterialParams {\n    _unused: vec4<f32>,\n};\n".to_string(),
            Vec::new(),
        ),
    };
    let user: String = source
        .text
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if header.as_ref().is_some_and(|h| h.lines.contains(&(i + 1))) {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let code_tokens = identifiers(&user);
    let defines = |f: &str| code_tokens.windows(2).any(|w| w[0] == "fn" && w[1] == f);
    if !defines("surface") {
        return Err(format!(
            "{name}: a material shader must define \
             `fn surface(in: SurfaceInput, p: {struct_name}) -> Surface`"
        ));
    }
    let mut prelude = String::new();
    for lib in [
        "common.wgsl",
        "object.wgsl",
        "lighting.wgsl",
        "shadows.wgsl",
        "fog.wgsl",
        "forward.wgsl",
        "prepass.wgsl",
        "wind.wgsl",
        "material_api.wgsl",
    ] {
        prelude.push_str(&format!("#include \"{lib}\"\n"));
    }
    prelude.push_str(&struct_wgsl);
    if struct_name != "MaterialParams" {
        prelude.push_str(&format!("alias MaterialParams = {struct_name};\n"));
    }
    prelude.push_str("@group(2) @binding(0) var<uniform> material: MaterialParams;\n");
    prelude.push_str(&format!("#include \"{file}\"\n"));
    let has_vertex = defines("vertex");
    let reads_scene_depth = code_tokens.contains(&"scene_depth");
    if !has_vertex {
        prelude.push_str(
            "fn vertex(position: vec3<f32>, in: VertexInput, p: MaterialParams) -> vec3<f32> {\n    \
             return position;\n}\n",
        );
    }
    prelude.push_str("#include \"custom_material.wgsl\"\n");
    // Skinned entry points (RS4).
    prelude.push_str("#include \"skin.wgsl\"\n");
    prelude.push_str("#include \"custom_material_skinned.wgsl\"\n");

    let mut files = library.clone();
    files.set(PRELUDE, &prelude);
    files.set(&file, &user);
    let (wgsl, map) = files.preprocess_mapped(PRELUDE)?;
    let module = validate_wgsl(name, &wgsl, &map)?;
    let (offsets, span) = param_layout(&module)
        .ok_or_else(|| format!("{name}: the `material` binding is not a struct"))?;
    let params = params
        .into_iter()
        .map(|decl| {
            let offset = offsets
                .iter()
                .find(|(n, _)| *n == decl.name)
                .map(|(_, o)| *o)
                .ok_or_else(|| {
                    format!("{name}: parameter `{}` missing from the struct", decl.name)
                })?;
            Ok(ParamSlot { decl, offset })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ComposedShader {
        wgsl,
        params,
        uniform_size: (span as u64).max(16).next_multiple_of(16),
        has_vertex,
        reads_scene_depth,
    })
}

/// Member offsets and the size of the `material` struct, as naga lays it
/// out for the uniform address space.
fn param_layout(module: &naga::Module) -> Option<(Vec<(String, u32)>, u32)> {
    let (_, var) = module
        .global_variables
        .iter()
        .find(|(_, v)| v.name.as_deref() == Some("material"))?;
    match &module.types[var.ty].inner {
        naga::TypeInner::Struct { members, span } => Some((
            members
                .iter()
                .map(|m| (m.name.clone().unwrap_or_default(), m.offset))
                .collect(),
            *span,
        )),
        _ => None,
    }
}

/// Identifiers and keywords of WGSL source, comments removed (enough to find
/// `fn <name>`).
fn identifiers(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in src.lines() {
        let code = line.split("//").next().unwrap_or("");
        out.extend(
            code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .filter(|t| !t.is_empty()),
        );
    }
    out
}

/// The parameter uniform's bytes for `material`: each declared parameter's
/// value (or header default) at its offset, little-endian.
pub(crate) fn pack_params(
    slots: &[ParamSlot],
    uniform_size: u64,
    material: &ShaderMaterial,
    out: &mut Vec<u8>,
) {
    out.clear();
    out.resize(uniform_size as usize, 0);
    for slot in slots {
        let value = material.value_for(&slot.decl);
        if let Some(given) = material.params.iter().find(|p| p.name == slot.decl.name)
            && !slot.decl.ty.accepts(&given.value)
        {
            tracing::warn!(
                "shader parameter `{}` holds {:?}, but the shader declares {}; using the default",
                slot.decl.name,
                given.value,
                slot.decl.ty.wgsl()
            );
        }
        let words = value.words();
        for (i, w) in words.iter().take(slot.decl.ty.components()).enumerate() {
            let at = slot.offset as usize + i * 4;
            if let Some(dst) = out.get_mut(at..at + 4) {
                dst.copy_from_slice(&w.to_le_bytes());
            }
        }
    }
}

/// A material shader that compiled: its module and parameter layout.
pub(crate) struct CompiledShader {
    pub module: wgpu::ShaderModule,
    pub params: Vec<ParamSlot>,
    pub uniform_size: u64,
    /// See [`ComposedShader::has_vertex`].
    pub has_vertex: bool,
    /// See [`ComposedShader::reads_scene_depth`].
    pub reads_scene_depth: bool,
}

/// The compile state of one `ShaderSource` asset.
pub(crate) struct CustomShader {
    /// (asset version, library generation) of the last compile attempt.
    attempted: (u64, u64),
    /// The last version that compiled (kept when a newer one fails).
    pub compiled: Option<CompiledShader>,
    /// Why the current version failed, if it did.
    pub error: Option<String>,
    /// Changes on every compile attempt; materials rebuild when it does.
    pub state: u64,
}

/// Every material shader, by `ShaderSource` asset id. Each version is
/// compiled once, whatever number of materials use it.
#[derive(Default)]
pub(crate) struct CustomShaders {
    shaders: HashMap<u64, CustomShader>,
    next_state: u64,
    /// Bumped when the engine library changes (hot reload): every material
    /// shader is recompiled against it.
    pub library_generation: u64,
}

impl CustomShaders {
    pub(crate) fn get(&self, id: u64) -> Option<&CustomShader> {
        self.shaders.get(&id)
    }

    /// Compiles new and changed shaders and forgets removed ones (with their
    /// pipelines). A version that fails keeps the previous version and its
    /// pipelines; the error is logged once and kept for
    /// `Renderer::shader_error`.
    #[profiling::function]
    pub(crate) fn sync(
        &mut self,
        device: &wgpu::Device,
        layouts: &Layouts,
        library: &ShaderSources,
        pipelines: &mut PipelineCache,
        assets: Option<&Assets<ShaderSource>>,
    ) {
        let Some(assets) = assets else {
            for id in self.shaders.drain().map(|(id, _)| id) {
                pipelines.remove_shader(ShaderKey::Custom(id));
            }
            return;
        };
        for (id, version, source) in assets.iter() {
            let want = (version, self.library_generation);
            if self.shaders.get(&id).is_some_and(|s| s.attempted == want) {
                continue;
            }
            // An empty source is the asset server's placeholder for a file
            // that failed to import (a bad `@material` header): the import
            // error is already reported, so draw the error material without
            // compiling or logging.
            let result = if source.text.is_empty() {
                Err("the shader file failed to import or was deleted (see its import error)".into())
            } else {
                compile(device, layouts, library, source, id, pipelines)
            };
            let placeholder = source.text.is_empty();
            self.next_state += 1;
            let entry = self.shaders.entry(id).or_insert(CustomShader {
                attempted: want,
                compiled: None,
                error: None,
                state: 0,
            });
            entry.attempted = want;
            entry.state = self.next_state;
            match result {
                Ok(compiled) => {
                    tracing::info!("material shader {} compiled", source.name);
                    entry.compiled = Some(compiled);
                    entry.error = None;
                }
                Err(e) if placeholder => {
                    entry.compiled = None;
                    entry.error = Some(e);
                }
                Err(e) => {
                    tracing::error!(
                        "material shader {} failed to compile; {}\n{e}",
                        source.name,
                        if entry.compiled.is_some() {
                            "keeping the previous version"
                        } else {
                            "drawing the error material"
                        }
                    );
                    entry.error = Some(e);
                }
            }
        }
        if self.shaders.len() > assets.len() {
            let gone: Vec<u64> = self
                .shaders
                .keys()
                .copied()
                .filter(|&id| assets.get_by_id(id).is_none())
                .collect();
            for id in gone {
                self.shaders.remove(&id);
                pipelines.remove_shader(ShaderKey::Custom(id));
            }
        }
    }
}

/// Composes `source`, creates its module and rebuilds the existing pipelines
/// of shader `id` with it, all inside error scopes and all or nothing.
fn compile(
    device: &wgpu::Device,
    layouts: &Layouts,
    library: &ShaderSources,
    source: &ShaderSource,
    id: u64,
    pipelines: &mut PipelineCache,
) -> Result<CompiledShader, String> {
    let composed = compose(library, source)?;
    let module = catch_errors(device, || {
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&source.name),
            source: wgpu::ShaderSource::Wgsl(composed.wgsl.as_str().into()),
        })
    })
    .map_err(|e| format!("{}: {e}", source.name))?;
    let keys = pipelines.keys_where(|k| k.shader == ShaderKey::Custom(id));
    let rebuilt = catch_errors(device, || {
        keys.iter()
            .map(|(slot, key)| {
                (
                    *slot,
                    create_material_pipeline(device, layouts, key, &module),
                )
            })
            .collect::<Vec<_>>()
    })
    .map_err(|e| format!("{}: pipeline creation failed: {e}", source.name))?;
    pipelines.replace(rebuilt);
    Ok(CompiledShader {
        module,
        params: composed.params,
        uniform_size: composed.uniform_size,
        has_vertex: composed.has_vertex,
        reads_scene_depth: composed.reads_scene_depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_core::color::Color;
    use box_core::shader_material::ShaderParamValue;

    const STRIPES: &str = "\
import box::lighting;

@material struct Params {
    /// Stripe colour.
    @color tint: vec3f = vec3f(0.9, 0.3, 0.2),
    @range(1.0, 40.0) frequency: f32 = 10.0,
    offset: vec2f,
}

fn surface(in: SurfaceInput, p: Params) -> Surface {
    var s = default_surface(in);
    let stripe = step(0.5, fract(in.uv.x * p.frequency + p.offset.x));
    s.base_color = mix(vec3f(0.9), p.tint, stripe);
    return s;
}
";

    fn library() -> ShaderSources {
        ShaderSources::embedded()
    }

    #[test]
    fn composes_validates_and_lays_out_params() {
        let c = compose(
            &library(),
            &ShaderSource::new("shaders/stripes.wgsl", STRIPES),
        )
        .unwrap();
        // WGSL uniform layout: vec3 at 0, the f32 packs into its tail, vec2
        // aligns to 8; the struct rounds up to 32.
        let offsets: Vec<_> = c
            .params
            .iter()
            .map(|p| (p.decl.name.as_str(), p.offset))
            .collect();
        assert_eq!(offsets, [("tint", 0), ("frequency", 12), ("offset", 16)]);
        assert_eq!(c.uniform_size, 32);
        for entry in [
            "vs_main",
            "fs_main",
            "vs_shadow",
            "vs_shadow_pancake",
            "fs_shadow_mask",
        ] {
            assert!(c.wgsl.contains(&format!("fn {entry}(")), "{entry}");
        }
        // A pass-through vertex hook was added.
        assert!(c.wgsl.contains("fn vertex(position: vec3<f32>"));

        let m = ShaderMaterial::default()
            .with_param(
                "tint",
                ShaderParamValue::Color(Color::linear(0.1, 0.2, 0.3)),
            )
            .with_param("offset", ShaderParamValue::Float(1.0)); // wrong type: default
        let mut bytes = Vec::new();
        pack_params(&c.params, c.uniform_size, &m, &mut bytes);
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!(floats, &[0.1, 0.2, 0.3, 10.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn without_a_header_the_params_type_is_material_params() {
        let src = "fn surface(in: SurfaceInput, p: MaterialParams) -> Surface {\n    \
                   return default_surface(in);\n}\n\
                   fn vertex(position: vec3f, in: VertexInput, p: MaterialParams) -> vec3f {\n    \
                   return position + vec3f(0.0, sin(globals.time.x), 0.0);\n}\n";
        let c = compose(&library(), &ShaderSource::new("plain.wgsl", src)).unwrap();
        assert!(c.params.is_empty());
        assert_eq!(c.uniform_size, 16);
        assert!(
            c.wgsl.contains("sin(globals.time.x)"),
            "the user's hook is kept"
        );
        assert!(
            !c.wgsl.contains("p: MaterialParams) -> vec3<f32> {"),
            "no default hook"
        );
    }

    #[test]
    fn scene_depth_helpers_compose_and_are_detected() {
        let shore = "\
@material struct P { @range(0.0, 4.0) foam: f32 = 0.5 }
fn surface(in: SurfaceInput, p: P) -> Surface {
    var s = default_surface(in);
    let gap = scene_depth(in.frag_coord) - fragment_view_depth(in.frag_coord);
    s.alpha = select(0.5, saturate(gap / p.foam), scene_depth_available());
    return s;
}
";
        let c = compose(&library(), &ShaderSource::new("shore.wgsl", shore)).unwrap();
        assert!(c.reads_scene_depth);
        let plain = compose(
            &library(),
            &ShaderSource::new("shaders/stripes.wgsl", STRIPES),
        )
        .unwrap();
        assert!(!plain.reads_scene_depth);
        // A mention in a comment is not a call.
        let commented = "// scene_depth(in.frag_coord) later\n\
                         fn surface(in: SurfaceInput, p: MaterialParams) -> Surface {\n    \
                         return default_surface(in);\n}\n";
        let c = compose(&library(), &ShaderSource::new("c.wgsl", commented)).unwrap();
        assert!(!c.reads_scene_depth);
    }

    #[test]
    fn errors_name_the_users_file_and_line() {
        let lib = library();
        let err =
            |text: &str| compose(&lib, &ShaderSource::new("shaders/bad.wgsl", text)).unwrap_err();
        // A type error on line 3 of the user's file.
        let e = err(
            "@material struct P { a: f32 }\nfn surface(in: SurfaceInput, p: P) -> Surface {\n    let x: i32 = p.a;\n    return default_surface(in);\n}\n",
        );
        assert!(e.contains("(at shaders/bad.wgsl:3)"), "{e}");
        // Header errors.
        let e = err("@material struct P { a: mat4x4f }\nfn surface() {}");
        assert!(
            e.contains("shaders/bad.wgsl: line 1") && e.contains("unsupported type"),
            "{e}"
        );
        // No surface hook.
        let e = err("fn not_surface() {}\n// fn surface( in a comment does not count\n");
        assert!(e.contains("must define `fn surface"), "{e}");
        // Unknown import.
        let e = err(
            "import box::nope;\nfn surface(in: SurfaceInput, p: MaterialParams) -> Surface { return default_surface(in); }",
        );
        assert!(
            e.contains("shaders/bad.wgsl:1") && e.contains("unknown import"),
            "{e}"
        );
        // A file named like an engine file does not replace it.
        let ok = compose(
            &lib,
            &ShaderSource::new(
                "common.wgsl",
                "fn surface(in: SurfaceInput, p: MaterialParams) -> Surface { return default_surface(in); }",
            ),
        );
        assert!(ok.is_ok(), "{:?}", ok.err());
    }
}
