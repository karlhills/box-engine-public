//! The `gltf` importer: glTF 2.0 (`.gltf` + buffers, `.glb`) → meshes,
//! materials, textures and a scene.
//!
//! Sub-asset labels (stable across re-imports of the same file):
//!
//! | Label | Type | From |
//! |---|---|---|
//! | `Mesh/<name>` | `MeshData` | each glTF mesh; one material group per triangle primitive |
//! | `Material/<name>` | `StandardMaterial` | each glTF material |
//! | `Texture/<i>` | `ImageData` | each image embedded in the file (GLB chunk or data URI), `i` = glTF image index |
//! | `Texture/orm_<mr>_<occ>` | `ImageData` | packed occlusion/roughness/metallic (see below) |
//! | `Skin/<i>` | `Skin` | each skin: joint names and parents, inverse bind matrices |
//! | `Animation/<name>` | `AnimationClip` | each animation: translation, rotation and scale channels by node name |
//! | `Scene` | `SceneAsset` | the default scene (else the first) |
//!
//! `<name>` is the glTF name sanitized (letters, digits, `-`, `_`, `.`; other
//! characters and spaces become `_`) or the element's index when it has no
//! name. When two elements get the same label, the later one (by index) gets
//! `_<index>` appended.
//!
//! Textures: images used as base colour or emissive are sRGB, images used
//! only as normal, metallic-roughness or occlusion maps are linear; all get
//! mips. External images (relative URIs in a `.gltf`) are not copied: the
//! material references the image file itself, which is imported with its own
//! `.meta` settings. When such a file is used only as a linear map and its
//! `.meta` was just created with the default `srgb`, the import sets it to
//! `linear`; a `.meta` that already said `srgb` is kept, with one warning.
//!
//! ORM: the engine's `orm_texture` is occlusion (R), roughness (G),
//! metallic (B), glTF's channel layout. When a material's metallic-roughness
//! and occlusion textures are the same image (and occlusion strength is 1),
//! that image is used as is. Otherwise a packed image is made at import:
//! R = occlusion (strength applied; 1 without occlusion), G and B from the
//! metallic-roughness image (1 without one), at the metallic-roughness
//! image's size.
//!
//! Skins: meshes keep `JOINTS_0` (as `u16`) and `WEIGHTS_0` (normalized to
//! sum to 1); a scene node with a mesh and a skin also gets a `SkinBinding`
//! naming the skin and its joint nodes' entities, which prefab instancing
//! remaps and the asset glue turns into a `SkinnedMesh`. Animation channels
//! target nodes by scene entity name (glTF name, or `Node <index>`).
//!
//! Colliders: with the `colliders` setting, every node with a mesh and no
//! skin also gets `Collider { shape: Mesh }` (box_physics, written by name
//! with the other fields at their defaults), so a level modelled outside
//! the editor collides as drawn once placed as a prefab. A node whose name
//! ends in [`NO_COLLIDER_SUFFIX`] (`-nocol`, any case: foliage, decals,
//! light shafts) is left without one.
//!
//! Not imported: cameras, lights, morph targets,
//! `texCoord` other than 0, normal-map scale, `KHR_texture_transform` and
//! other extensions (ignored, logged at debug level). Draco, meshopt,
//! KTX2/BasisU and quantized attributes are refused with a clear error.

use std::collections::{BTreeSet, HashMap, HashSet};

use box_core::animation::{AnimationClip, Channel, Interpolation, Keyframes, Skin, SkinBinding};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::color::Color;
use box_core::identity::Uid;
use box_core::image::ColorSpace;
use box_core::material::{AlphaMode, StandardMaterial};
use box_core::mesh::{MaterialGroup, MeshData};
use box_core::mesh_asset::MeshAsset;
use box_core::scene::SCENE_VERSION;
use box_core::transform::Transform;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use gltf::accessor::{DataType, Dimensions};
use gltf::animation::util::ReadOutputs;
use gltf::mesh::Mode;
use gltf::{Document, Semantic};
use image::RgbaImage;
use serde_json::{Value as Json, json};

use super::data::core_registry;
use super::texture::{decode_rgba, to_image_data};
use crate::error::ImportError;
use crate::importer::{ImportContext, Importer};
use crate::meta::{self, ColorSpaceSetting, ModelSettings, TextureSettings};
use crate::scene_asset::SceneAsset;

/// glTF 2.0 / GLB importer. See the [module docs](self) for labels and
/// conventions.
pub struct GltfImporter;

impl Importer for GltfImporter {
    fn name(&self) -> &str {
        "gltf"
    }

    fn extensions(&self) -> &[&str] {
        &["gltf", "glb"]
    }

    fn default_settings(&self) -> Json {
        serde_json::to_value(ModelSettings::default()).unwrap_or_default()
    }

    fn import(&self, ctx: &mut ImportContext) -> Result<(), ImportError> {
        let settings: ModelSettings = ctx.settings()?;
        if !(settings.scale.is_finite() && settings.scale > 0.0) {
            return Err(ImportError::Settings(format!(
                "scale must be a finite number above 0, got {}",
                settings.scale
            )));
        }
        let gltf::Gltf { document, blob } = parse(ctx.path(), ctx.bytes())?;
        let buffers = load_buffers(ctx, &document, blob)?;
        let labels = Labels::new(&document);
        let mut images = Images::new(ctx, &document, &buffers);
        let materials = import_materials(ctx, &document, &labels, &mut images);
        let mesh_materials =
            import_meshes(ctx, &document, &buffers, &labels, &materials, &settings)?;
        import_skins(ctx, &document, &buffers, settings.scale)?;
        import_animations(ctx, &document, &buffers, &labels, settings.scale)?;
        import_scene(ctx, &document, &labels, &mesh_materials, &settings)
    }
}

fn parse(path: &str, bytes: &[u8]) -> Result<gltf::Gltf, ImportError> {
    match gltf::Gltf::from_slice(bytes) {
        Ok(g) => {
            check_extensions(path, &g.document)?;
            Ok(g)
        }
        Err(e) => {
            // An unsupported compression extension often fails validation
            // (no plain accessors); report the extension, not the symptom.
            if let Ok(g) = gltf::Gltf::from_slice_without_validation(bytes) {
                check_extensions(path, &g.document)?;
            }
            let m = e.to_string();
            Err(ImportError::Decode(if m.starts_with("invalid glTF") {
                m
            } else {
                format!("invalid glTF: {m}")
            }))
        }
    }
}

const SUPPORTED_EXTENSIONS: &[&str] = &["KHR_materials_emissive_strength"];

fn check_extensions(path: &str, doc: &Document) -> Result<(), ImportError> {
    if let Some(ext) = doc.extensions_required().next() {
        let what = match ext {
            "KHR_draco_mesh_compression" => {
                "Draco-compressed meshes (KHR_draco_mesh_compression); re-export without Draco compression".to_string()
            }
            "EXT_meshopt_compression" | "KHR_meshopt_compression" => {
                format!("meshopt-compressed buffers ({ext}); re-export without meshopt compression")
            }
            "KHR_texture_basisu" => {
                "KTX2/BasisU textures (KHR_texture_basisu); use PNG or JPEG textures".to_string()
            }
            "KHR_mesh_quantization" => {
                "quantized vertex attributes (KHR_mesh_quantization); re-export with float attributes".to_string()
            }
            other if SUPPORTED_EXTENSIONS.contains(&other) => String::new(),
            other => format!("the required glTF extension {other}"),
        };
        if !what.is_empty() {
            return Err(ImportError::Unsupported(what));
        }
    }
    for ext in doc.extensions_used() {
        if !SUPPORTED_EXTENSIONS.contains(&ext) {
            tracing::debug!("{path}: glTF extension {ext} is ignored");
        }
    }
    Ok(())
}

// ------------------------------------------------------------ buffers ----

fn load_buffers(
    ctx: &mut ImportContext,
    doc: &Document,
    mut blob: Option<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, ImportError> {
    let mut out = Vec::new();
    for buffer in doc.buffers() {
        let i = buffer.index();
        let data = match buffer.source() {
            gltf::buffer::Source::Bin => blob.take().ok_or_else(|| {
                ImportError::Decode(format!(
                    "buffer {i} refers to the GLB binary chunk, which is missing"
                ))
            })?,
            gltf::buffer::Source::Uri(uri) => {
                read_uri(ctx, uri).map_err(|e| prefix(e, &format!("buffer {i}")))?
            }
        };
        if data.len() < buffer.length() {
            return Err(ImportError::Decode(format!(
                "buffer {i} has {} bytes, the file declares {}",
                data.len(),
                buffer.length()
            )));
        }
        out.push(data);
    }
    Ok(out)
}

fn prefix(e: ImportError, what: &str) -> ImportError {
    match e {
        ImportError::Io(m) => ImportError::Io(format!("{what}: {m}")),
        ImportError::Decode(m) => ImportError::Decode(format!("{what}: {m}")),
        ImportError::Unsupported(m) => ImportError::Unsupported(format!("{what}: {m}")),
        ImportError::Settings(m) => ImportError::Settings(m),
    }
}

/// The bytes of a data URI or of a file relative to the glTF.
fn read_uri(ctx: &mut ImportContext, uri: &str) -> Result<Vec<u8>, ImportError> {
    if let Some(data) = data_uri(uri) {
        return data;
    }
    let path = external_path(ctx, uri)?;
    ctx.read_dependency(&path)
}

fn data_uri(uri: &str) -> Option<Result<Vec<u8>, ImportError>> {
    let rest = uri.strip_prefix("data:")?;
    let Some((header, data)) = rest.split_once(',') else {
        return Some(Err(ImportError::Decode("malformed data URI".into())));
    };
    if !header.ends_with(";base64") {
        return Some(Err(ImportError::Unsupported(
            "data URIs that are not base64-encoded".into(),
        )));
    }
    Some(
        base64_decode(data).ok_or_else(|| ImportError::Decode("invalid base64 in data URI".into())),
    )
}

/// A relative URI as a path relative to `assets/`.
fn external_path(ctx: &ImportContext, uri: &str) -> Result<String, ImportError> {
    if uri.contains("://") {
        return Err(ImportError::Unsupported(format!(
            "external URI `{uri}` (only relative file paths are supported)"
        )));
    }
    ctx.resolve_relative(&percent_decode(uri))
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ------------------------------------------------------------- labels ----

fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Labels for a list of optionally named elements, deterministic and unique.
fn unique_labels<'a>(prefix: &str, names: impl Iterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut used = HashSet::new();
    let mut out = Vec::new();
    for (i, name) in names.enumerate() {
        let base = name
            .map(sanitize)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| i.to_string());
        let mut label = base.clone();
        if used.contains(&label) {
            label = format!("{base}_{i}");
            while used.contains(&label) {
                label.push('_');
            }
        }
        used.insert(label.clone());
        out.push(format!("{prefix}/{label}"));
    }
    out
}

struct Labels {
    meshes: Vec<String>,
    materials: Vec<String>,
    animations: Vec<String>,
}

impl Labels {
    fn new(doc: &Document) -> Self {
        Self {
            meshes: unique_labels("Mesh", doc.meshes().map(|m| m.name())),
            materials: unique_labels("Material", doc.materials().map(|m| m.name())),
            animations: unique_labels("Animation", doc.animations().map(|a| a.name())),
        }
    }
}

/// A skin's label: `Skin/<index>`.
fn skin_label(index: usize) -> String {
    format!("Skin/{index}")
}

/// The entity name a node gets in the imported scene (its glTF name, or
/// `Node <index>`), which animation channels target.
fn node_name(node: &gltf::Node) -> String {
    node.name()
        .map_or_else(|| format!("Node {}", node.index()), str::to_string)
}

// ------------------------------------------------------------- images ----

enum ImageSource {
    /// Decoded into the sub-asset `label`.
    Embedded { label: String },
    /// An image file next to the glTF (path relative to `assets/`).
    External { path: String },
    /// Could not be decoded or located (already warned).
    Unavailable { label: String },
}

/// A packed ORM image: metallic-roughness image, occlusion image and
/// strength bits.
type OrmKey = (Option<usize>, Option<(usize, u32)>);

struct Images {
    sources: Vec<ImageSource>,
    /// RGBA pixels kept for ORM packing (images used as data maps).
    pixels: Vec<Option<RgbaImage>>,
    /// Packed ORM images made so far: (mr, occ, strength bits) → label.
    packed: HashMap<OrmKey, String>,
}

#[derive(Clone, Copy, Default)]
struct Usage {
    color: bool,
    data: bool,
}

impl Images {
    fn new(ctx: &mut ImportContext, doc: &Document, buffers: &[Vec<u8>]) -> Self {
        let mut usage = vec![Usage::default(); doc.images().len()];
        for m in doc.materials() {
            let pbr = m.pbr_metallic_roughness();
            let mut mark = |i: usize, color: bool| {
                if let Some(u) = usage.get_mut(i) {
                    if color {
                        u.color = true;
                    } else {
                        u.data = true;
                    }
                }
            };
            if let Some(t) = pbr.base_color_texture() {
                mark(t.texture().source().index(), true);
            }
            if let Some(t) = m.emissive_texture() {
                mark(t.texture().source().index(), true);
            }
            if let Some(t) = pbr.metallic_roughness_texture() {
                mark(t.texture().source().index(), false);
            }
            if let Some(t) = m.normal_texture() {
                mark(t.texture().source().index(), false);
            }
            if let Some(t) = m.occlusion_texture() {
                mark(t.texture().source().index(), false);
            }
        }
        let mut sources = Vec::new();
        let mut pixels = Vec::new();
        for image in doc.images() {
            let i = image.index();
            let label = format!("Texture/{i}");
            let u = usage[i];
            let bytes = match image.source() {
                gltf::image::Source::View { view, .. } => {
                    let start = view.offset();
                    buffers
                        .get(view.buffer().index())
                        .and_then(|b| b.get(start..start.checked_add(view.length())?))
                        .map(<[u8]>::to_vec)
                        .ok_or_else(|| ImportError::Decode("buffer view out of range".into()))
                }
                gltf::image::Source::Uri { uri, .. } => match data_uri(uri) {
                    Some(data) => data,
                    None => {
                        let source = match external_path(ctx, uri) {
                            Ok(path) => {
                                check_external_texture(ctx, &path, u);
                                ImageSource::External { path }
                            }
                            Err(e) => {
                                tracing::warn!("{}: image {i}: {e}", ctx.path());
                                ImageSource::Unavailable { label }
                            }
                        };
                        sources.push(source);
                        pixels.push(None);
                        continue;
                    }
                },
            };
            let decoded = bytes.and_then(|b| decode_rgba(&b));
            match decoded {
                Ok(rgba) => {
                    if u.color && u.data {
                        tracing::warn!(
                            "{}: image {i} is used both as colour and as data; imported as sRGB",
                            ctx.path()
                        );
                    }
                    let cs = if !u.color && u.data {
                        ColorSpace::Linear
                    } else {
                        ColorSpace::Srgb
                    };
                    pixels.push(u.data.then(|| rgba.clone()));
                    match to_image_data(rgba, cs, true) {
                        Ok(img) => {
                            ctx.add(Some(label.clone()), img);
                            sources.push(ImageSource::Embedded { label });
                        }
                        Err(e) => {
                            tracing::warn!("{}: image {i}: {e}", ctx.path());
                            sources.push(ImageSource::Unavailable { label });
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("{}: image {i}: {e}", ctx.path());
                    pixels.push(None);
                    sources.push(ImageSource::Unavailable { label });
                }
            }
        }
        Self {
            sources,
            pixels,
            packed: HashMap::new(),
        }
    }

    fn texture_ref(&self, ctx: &ImportContext, image: usize) -> AssetRef {
        match self.sources.get(image) {
            Some(ImageSource::Embedded { label } | ImageSource::Unavailable { label }) => {
                ctx.sub_asset_ref(label)
            }
            Some(ImageSource::External { path }) => ctx.asset_ref(path),
            None => ctx.sub_asset_ref(&format!("Texture/{image}")),
        }
    }

    /// Pixels of `image` for packing (external files are read here).
    fn pixels(&mut self, ctx: &mut ImportContext, image: usize) -> Option<RgbaImage> {
        if let Some(Some(p)) = self.pixels.get(image) {
            return Some(p.clone());
        }
        let Some(ImageSource::External { path }) = self.sources.get(image) else {
            return None;
        };
        let path = path.clone();
        let rgba = ctx
            .read_dependency(&path)
            .and_then(|b| decode_rgba(&b))
            .inspect_err(|e| tracing::warn!("{}: image {image}: {e}", ctx.path()))
            .ok()?;
        if let Some(slot) = self.pixels.get_mut(image) {
            *slot = Some(rgba.clone());
        }
        Some(rgba)
    }

    /// The ORM texture for a material (see the module docs).
    fn orm_ref(
        &mut self,
        ctx: &mut ImportContext,
        mr: Option<usize>,
        occlusion: Option<(usize, f32)>,
    ) -> AssetRef {
        match (mr, occlusion) {
            (None, None) => return AssetRef::NONE,
            (Some(m), Some((o, s))) if m == o && s == 1.0 => return self.texture_ref(ctx, m),
            _ => {}
        }
        let key = (mr, occlusion.map(|(o, s)| (o, s.to_bits())));
        if let Some(label) = self.packed.get(&key) {
            return ctx.sub_asset_ref(label);
        }
        let name = |i: Option<usize>| i.map_or("none".to_string(), |i| i.to_string());
        let mut label = format!("Texture/orm_{}_{}", name(mr), name(occlusion.map(|o| o.0)));
        if occlusion.is_some_and(|(_, s)| s != 1.0) {
            label = format!("{label}_{}", self.packed.len());
        }
        let mr_px = mr.and_then(|m| self.pixels(ctx, m));
        let occ_px = occlusion.and_then(|(o, s)| Some((self.pixels(ctx, o)?, s)));
        if mr.is_some() != mr_px.is_some() || occlusion.is_some() != occ_px.is_some() {
            tracing::warn!(
                "{}: could not build {label}: a source image is unavailable",
                ctx.path()
            );
            return AssetRef::NONE;
        }
        let packed = pack_orm(mr_px.as_ref(), occ_px.as_ref().map(|(p, s)| (p, *s)));
        match to_image_data(packed, ColorSpace::Linear, true) {
            Ok(img) => ctx.add(Some(label.clone()), img),
            Err(e) => {
                tracing::warn!("{}: {label}: {e}", ctx.path());
                return AssetRef::NONE;
            }
        }
        self.packed.insert(key, label.clone());
        ctx.sub_asset_ref(&label)
    }
}

/// An external image used only as a data map (normal, metallic-roughness,
/// occlusion) must be linear. A `.meta` that the asset index created in this
/// process (default settings, nobody chose `srgb`) is switched to `linear`;
/// one that already said `srgb` keeps it, with a warning once per file and
/// process.
fn check_external_texture(ctx: &ImportContext, path: &str, usage: Usage) {
    if !ctx.is_indexed(path) {
        tracing::warn!("{}: external texture `{path}` not found", ctx.path());
        return;
    }
    if !usage.data || usage.color {
        return;
    }
    let abs = ctx.root.join(path);
    let Some(mut sidecar) = meta::read_meta(&abs).ok().flatten() else {
        return;
    };
    let srgb = meta::parse_settings::<TextureSettings>(&sidecar.settings)
        .is_ok_and(|(s, _)| s.color_space == ColorSpaceSetting::Srgb);
    if !srgb {
        return;
    }
    let created = ctx.index.read().is_ok_and(|i| i.created_meta(path));
    if created && let Some(settings) = sidecar.settings.as_object_mut() {
        settings.insert("color_space".into(), "linear".into());
        match meta::write_meta(&abs, &sidecar) {
            Ok(()) => tracing::info!(
                "{}: `{path}` is a normal/ORM map; set \"color_space\": \"linear\" in its new {path}.meta",
                ctx.path()
            ),
            Err(e) => tracing::warn!("{path}.meta: could not set \"color_space\": \"linear\": {e}"),
        }
        return;
    }
    static WARNED: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
    let first = WARNED
        .lock()
        .map_or(true, |mut w| w.insert(path.to_string()));
    if first {
        tracing::warn!(
            "{}: `{path}` is used as a normal/ORM map but imported as sRGB; set \"color_space\": \"linear\" in assets/{path}.meta",
            ctx.path()
        );
    }
}

fn pack_orm(mr: Option<&RgbaImage>, occlusion: Option<(&RgbaImage, f32)>) -> RgbaImage {
    let (w, h) = mr
        .map(RgbaImage::dimensions)
        .or_else(|| occlusion.map(|(o, _)| o.dimensions()))
        .unwrap_or((1, 1));
    RgbaImage::from_fn(w, h, |x, y| {
        let r = occlusion.map_or(255, |(o, s)| {
            let (ow, oh) = o.dimensions();
            let v = f32::from(o.get_pixel(x * ow / w, y * oh / h)[0]) / 255.0;
            ((1.0 + s * (v - 1.0)).clamp(0.0, 1.0) * 255.0).round() as u8
        });
        let (g, b) = mr.map_or((255, 255), |m| {
            let p = m.get_pixel(x, y);
            (p[1], p[2])
        });
        image::Rgba([r, g, b, 255])
    })
}

// ---------------------------------------------------------- materials ----

/// Adds every material; returns their references by glTF material index.
fn import_materials(
    ctx: &mut ImportContext,
    doc: &Document,
    labels: &Labels,
    images: &mut Images,
) -> Vec<AssetRef> {
    let mut refs = Vec::new();
    for m in doc.materials() {
        let label = &labels.materials[refs.len()];
        let pbr = m.pbr_metallic_roughness();
        let tex_coords = [
            pbr.base_color_texture().map(|t| t.tex_coord()),
            pbr.metallic_roughness_texture().map(|t| t.tex_coord()),
            m.normal_texture().map(|t| t.tex_coord()),
            m.occlusion_texture().map(|t| t.tex_coord()),
            m.emissive_texture().map(|t| t.tex_coord()),
        ];
        if tex_coords.iter().flatten().any(|&c| c != 0) {
            tracing::warn!(
                "{}: {label}: textures using UV sets other than 0 are sampled with UV set 0",
                ctx.path()
            );
        }
        if m.normal_texture().is_some_and(|t| t.scale() != 1.0) {
            tracing::debug!("{}: {label}: normal map scale is ignored", ctx.path());
        }
        let [r, g, b, a] = pbr.base_color_factor();
        let [er, eg, eb] = m.emissive_factor();
        let strength = m.emissive_strength().unwrap_or(1.0);
        let tex = |images: &Images, ctx: &ImportContext, t: Option<usize>| {
            t.map_or(AssetRef::NONE, |i| images.texture_ref(ctx, i))
        };
        let base_color_texture = tex(
            images,
            ctx,
            pbr.base_color_texture()
                .map(|t| t.texture().source().index()),
        );
        let normal_texture = tex(
            images,
            ctx,
            m.normal_texture().map(|t| t.texture().source().index()),
        );
        let emissive_texture = tex(
            images,
            ctx,
            m.emissive_texture().map(|t| t.texture().source().index()),
        );
        let orm_texture = images.orm_ref(
            ctx,
            pbr.metallic_roughness_texture()
                .map(|t| t.texture().source().index()),
            m.occlusion_texture()
                .map(|t| (t.texture().source().index(), t.strength())),
        );
        let material = StandardMaterial {
            base_color: Color::linear_rgba(r, g, b, a),
            roughness: pbr.roughness_factor(),
            metallic: pbr.metallic_factor(),
            emissive: Color::linear(er * strength, eg * strength, eb * strength),
            base_color_texture,
            normal_texture,
            orm_texture,
            emissive_texture,
            alpha_mode: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask {
                    cutoff: m.alpha_cutoff().unwrap_or(0.5),
                },
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            double_sided: m.double_sided(),
            ..StandardMaterial::default()
        };
        let label = label.clone();
        refs.push(ctx.sub_asset_ref(&label));
        ctx.add(Some(label), material);
    }
    refs
}

// ------------------------------------------------------------- meshes ----

/// Per-vertex joint indices and weights.
type SkinWeights = (Vec<[u16; 4]>, Vec<[f32; 4]>);

#[derive(Default)]
struct Primitive {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: [Option<Vec<Vec2>>; 4],
    colors: Option<Vec<[f32; 4]>>,
    tangents: Option<Vec<Vec4>>,
    /// `JOINTS_0` and normalized `WEIGHTS_0`, when both are present.
    skin: Option<SkinWeights>,
    indices: Vec<u32>,
    material: AssetRef,
}

/// Adds every mesh; returns each mesh's material references by group slot.
fn import_meshes(
    ctx: &mut ImportContext,
    doc: &Document,
    buffers: &[Vec<u8>],
    labels: &Labels,
    materials: &[AssetRef],
    settings: &ModelSettings,
) -> Result<Vec<Vec<AssetRef>>, ImportError> {
    let mut all = Vec::new();
    for mesh in doc.meshes() {
        let label = &labels.meshes[mesh.index()];
        let mut prims = Vec::new();
        for (pi, prim) in mesh.primitives().enumerate() {
            let what = format!("{label} primitive {pi}");
            if let Some(p) = read_primitive(ctx.path(), &what, &prim, buffers, settings.scale)? {
                let material = prim
                    .material()
                    .index()
                    .and_then(|i| materials.get(i).cloned())
                    .unwrap_or(AssetRef::NONE);
                prims.push(Primitive { material, ..p });
            }
        }
        if prims.is_empty() {
            tracing::warn!("{}: {label} has no triangles", ctx.path());
        }
        let mut data = merge(&prims, label)?;
        if settings.generate_tangents
            && data.tangents.is_empty()
            && !data.uvs.is_empty()
            && !data.indices.is_empty()
        {
            generate_tangents(&mut data);
        }
        data.validate()
            .map_err(|e| ImportError::Decode(format!("{label}: {e}")))?;
        all.push(prims.into_iter().map(|p| p.material).collect());
        ctx.add(Some(label.clone()), data);
    }
    Ok(all)
}

fn check_float(
    prim: &gltf::Primitive,
    semantic: Semantic,
    dims: Dimensions,
    what: &str,
) -> Result<(), ImportError> {
    if let Some(a) = prim.get(&semantic)
        && (a.data_type() != DataType::F32 || a.dimensions() != dims)
    {
        return Err(ImportError::Unsupported(format!(
            "{what}: {semantic:?} stored as {:?} {:?} (quantized attributes); re-export with float attributes",
            a.data_type(),
            a.dimensions()
        )));
    }
    Ok(())
}

fn read_primitive(
    path: &str,
    what: &str,
    prim: &gltf::Primitive,
    buffers: &[Vec<u8>],
    scale: f32,
) -> Result<Option<Primitive>, ImportError> {
    let mode = prim.mode();
    if !matches!(
        mode,
        Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan
    ) {
        tracing::warn!("{path}: {what}: {mode:?} primitives are skipped");
        return Ok(None);
    }
    check_float(prim, Semantic::Positions, Dimensions::Vec3, what)?;
    check_float(prim, Semantic::Normals, Dimensions::Vec3, what)?;
    check_float(prim, Semantic::Tangents, Dimensions::Vec4, what)?;
    let reader = prim.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
    let decode = |m: String| ImportError::Decode(format!("{what}: {m}"));
    let positions: Vec<Vec3> = reader
        .read_positions()
        .ok_or_else(|| decode("missing or unreadable POSITION".into()))?
        .map(|p| Vec3::from(p) * scale)
        .collect();
    let n = positions.len();
    let declared = prim.get(&Semantic::Positions).map_or(0, |a| a.count());
    if n != declared {
        return Err(decode(format!(
            "POSITION has {n} readable values, {declared} declared"
        )));
    }
    if n == 0 {
        return Ok(None);
    }
    let check_len = |name: &str, len: usize| {
        if len == n {
            Ok(())
        } else {
            Err(decode(format!("{name} has {len} values, POSITION has {n}")))
        }
    };
    let normals: Option<Vec<Vec3>> = reader.read_normals().map(|i| i.map(Vec3::from).collect());
    let tangents: Option<Vec<Vec4>> = reader.read_tangents().map(|i| i.map(Vec4::from).collect());
    let mut uvs: [Option<Vec<Vec2>>; 4] = Default::default();
    for (set, uv) in uvs.iter_mut().enumerate() {
        *uv = reader
            .read_tex_coords(set as u32)
            .map(|t| t.into_f32().map(Vec2::from).collect());
    }
    let colors: Option<Vec<[f32; 4]>> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect());
    let joints: Option<Vec<[u16; 4]>> = reader.read_joints(0).map(|j| j.into_u16().collect());
    let weights: Option<Vec<[f32; 4]>> = reader
        .read_weights(0)
        .map(|w| w.into_f32().map(normalize_weights).collect());
    if joints.is_some() != weights.is_some() {
        tracing::warn!("{path}: {what}: JOINTS_0 without WEIGHTS_0 (or the reverse); not skinned");
    }
    let skin = joints.zip(weights);
    if let Some((j, w)) = &skin {
        check_len("JOINTS_0", j.len())?;
        check_len("WEIGHTS_0", w.len())?;
    }
    if let Some(v) = &normals {
        check_len("NORMAL", v.len())?;
    }
    if let Some(v) = &tangents {
        check_len("TANGENT", v.len())?;
    }
    for (set, uv) in uvs.iter().enumerate() {
        if let Some(v) = uv {
            check_len(&format!("TEXCOORD_{set}"), v.len())?;
        }
    }
    if let Some(v) = &colors {
        check_len("COLOR_0", v.len())?;
    }
    let raw: Vec<u32> = match reader.read_indices() {
        Some(i) => i.into_u32().collect(),
        None => {
            if prim.indices().is_some() {
                return Err(decode("indices are unreadable".into()));
            }
            (0..n as u32).collect()
        }
    };
    if let Some(&bad) = raw.iter().find(|&&i| i as usize >= n) {
        return Err(decode(format!("index {bad} out of range ({n} vertices)")));
    }
    let indices = match mode {
        Mode::TriangleStrip => (0..raw.len().saturating_sub(2))
            .flat_map(|i| {
                if i % 2 == 0 {
                    [raw[i], raw[i + 1], raw[i + 2]]
                } else {
                    [raw[i + 1], raw[i], raw[i + 2]]
                }
            })
            .collect(),
        Mode::TriangleFan => (1..raw.len().saturating_sub(1))
            .flat_map(|i| [raw[0], raw[i], raw[i + 1]])
            .collect(),
        _ => {
            if !raw.len().is_multiple_of(3) {
                return Err(decode(format!(
                    "{} indices is not a multiple of 3",
                    raw.len()
                )));
            }
            raw
        }
    };
    let mut p = Primitive {
        positions,
        normals: Vec::new(),
        uvs,
        colors,
        tangents,
        skin,
        indices,
        material: AssetRef::NONE,
    };
    match normals {
        Some(normals) => p.normals = normals,
        None => flat_normals(&mut p),
    }
    Ok(Some(p))
}

/// glTF: without normals, a primitive is flat shaded. Unwelds every
/// triangle and gives it its face normal.
fn flat_normals(p: &mut Primitive) {
    fn gather<T: Copy>(v: &[T], idx: &[u32]) -> Vec<T> {
        idx.iter().map(|&i| v[i as usize]).collect()
    }
    let idx = std::mem::take(&mut p.indices);
    p.positions = gather(&p.positions, &idx);
    for uv in p.uvs.iter_mut().flatten() {
        *uv = gather(uv, &idx);
    }
    if let Some(c) = &mut p.colors {
        *c = gather(c, &idx);
    }
    if let Some((j, w)) = &mut p.skin {
        *j = gather(j, &idx);
        *w = gather(w, &idx);
    }
    // Tangents without normals are meaningless; regenerate them if asked.
    p.tangents = None;
    p.normals = p
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|t| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
            let n = if n == Vec3::ZERO { Vec3::Y } else { n };
            [n; 3]
        })
        .collect();
    p.indices = (0..p.positions.len() as u32).collect();
}

/// Weights scaled to sum to 1; all-zero (or invalid) weights become joint
/// 0 at full weight.
fn normalize_weights(w: [f32; 4]) -> [f32; 4] {
    let w = w.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    let sum: f32 = w.iter().sum();
    if sum > 1e-6 {
        w.map(|v| v / sum)
    } else {
        [1.0, 0.0, 0.0, 0.0]
    }
}

/// One `MeshData` with a material group per primitive. Channels present in
/// some primitives only are filled (UVs with zero, colours with white,
/// skin weights with joint 0 at full weight); tangents are kept only when
/// every primitive has them.
fn merge(prims: &[Primitive], what: &str) -> Result<MeshData, ImportError> {
    let mut data = MeshData::default();
    let uv_sets = (0..4)
        .rev()
        .find(|&s| prims.iter().any(|p| p.uvs[s].is_some()))
        .map_or(0, |s| s + 1);
    let any_colors = prims.iter().any(|p| p.colors.is_some());
    let all_tangents = !prims.is_empty() && prims.iter().all(|p| p.tangents.is_some());
    let any_skin = prims.iter().any(|p| p.skin.is_some());
    data.uvs = vec![Vec::new(); uv_sets];
    let total: usize = prims.iter().map(|p| p.positions.len()).sum();
    if total > u32::MAX as usize {
        return Err(ImportError::Unsupported(format!(
            "{what}: {total} vertices is more than a mesh can index"
        )));
    }
    for (slot, p) in prims.iter().enumerate() {
        let n = p.positions.len();
        let base = data.positions.len() as u32;
        data.positions.extend(&p.positions);
        data.normals.extend(&p.normals);
        for (s, out) in data.uvs.iter_mut().enumerate() {
            match &p.uvs[s] {
                Some(uv) => out.extend(uv),
                None => out.extend(std::iter::repeat_n(Vec2::ZERO, n)),
            }
        }
        if any_colors {
            match &p.colors {
                Some(c) => data.colors.extend(c),
                None => data.colors.extend(std::iter::repeat_n([1.0; 4], n)),
            }
        }
        if all_tangents && let Some(t) = &p.tangents {
            data.tangents.extend(t);
        }
        if any_skin {
            match &p.skin {
                Some((j, w)) => {
                    data.joints.extend(j);
                    data.weights.extend(w);
                }
                None => {
                    data.joints.extend(std::iter::repeat_n([0; 4], n));
                    data.weights
                        .extend(std::iter::repeat_n([1.0, 0.0, 0.0, 0.0], n));
                }
            }
        }
        data.groups.push(MaterialGroup {
            first_index: data.indices.len() as u32,
            index_count: p.indices.len() as u32,
            material_slot: slot as u32,
        });
        data.indices.extend(p.indices.iter().map(|i| i + base));
    }
    Ok(data)
}

struct Mikk<'a> {
    data: &'a MeshData,
    tangents: Vec<Vec4>,
}

impl Mikk<'_> {
    fn vertex(&self, face: usize, vert: usize) -> usize {
        self.data.indices[face * 3 + vert] as usize
    }
}

impl bevy_mikktspace::Geometry for Mikk<'_> {
    fn num_faces(&self) -> usize {
        self.data.indices.len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.data.positions[self.vertex(face, vert)].into()
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.data.normals[self.vertex(face, vert)].into()
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.data.uvs[0][self.vertex(face, vert)].into()
    }

    fn set_tangent(
        &mut self,
        tangent_space: Option<bevy_mikktspace::TangentSpace>,
        face: usize,
        vert: usize,
    ) {
        if let Some(ts) = tangent_space {
            let v = self.vertex(face, vert);
            self.tangents[v] = Vec4::from(ts.tangent_encoded());
        }
    }
}

/// MikkTSpace tangents from normals and UV set 0. Vertices shared by faces
/// with different tangents keep the last one written.
fn generate_tangents(data: &mut MeshData) {
    let mut mikk = Mikk {
        data,
        tangents: vec![Vec4::new(1.0, 0.0, 0.0, 1.0); data.positions.len()],
    };
    // The error type has no variants in bevy_mikktspace 1.0.
    if bevy_mikktspace::generate_tangents(&mut mikk).is_ok() {
        data.tangents = mikk.tangents;
    }
}

// ------------------------------------------------------ skins, clips ----

/// Every skin as `Skin/<i>`: joint names, parent joints and inverse bind
/// matrices (identity when the file has none; translations scaled by the
/// import scale like node translations).
fn import_skins(
    ctx: &mut ImportContext,
    doc: &Document,
    buffers: &[Vec<u8>],
    scale: f32,
) -> Result<(), ImportError> {
    let mut parent_of: HashMap<usize, usize> = HashMap::new();
    for node in doc.nodes() {
        for child in node.children() {
            parent_of.insert(child.index(), node.index());
        }
    }
    for skin in doc.skins() {
        let what = skin_label(skin.index());
        let nodes: Vec<gltf::Node> = skin.joints().collect();
        if nodes.len() > usize::from(u16::MAX) {
            return Err(ImportError::Unsupported(format!(
                "{what}: {} joints (at most {})",
                nodes.len(),
                u16::MAX
            )));
        }
        let reader = skin.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
        let mut inverse_bind_matrices: Vec<Mat4> = match reader.read_inverse_bind_matrices() {
            Some(m) => m.map(|c| Mat4::from_cols_array_2d(&c)).collect(),
            None => vec![Mat4::IDENTITY; nodes.len()],
        };
        if inverse_bind_matrices.len() != nodes.len() {
            return Err(ImportError::Decode(format!(
                "{what}: {} inverse bind matrices for {} joints",
                inverse_bind_matrices.len(),
                nodes.len()
            )));
        }
        if !inverse_bind_matrices.iter().all(Mat4::is_finite) {
            return Err(ImportError::Decode(format!(
                "{what}: an inverse bind matrix is not finite"
            )));
        }
        for m in &mut inverse_bind_matrices {
            m.w_axis = (m.w_axis.truncate() * scale).extend(m.w_axis.w);
        }
        let joint_of: HashMap<usize, u32> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.index(), i as u32))
            .collect();
        let parents = nodes
            .iter()
            .map(|n| {
                let mut p = parent_of.get(&n.index());
                // The nearest ancestor that is a joint of this skin.
                while let Some(&node) = p {
                    if let Some(&j) = joint_of.get(&node) {
                        return Some(j);
                    }
                    p = parent_of.get(&node);
                }
                None
            })
            .collect();
        ctx.add(
            Some(what),
            Skin {
                joint_names: nodes.iter().map(node_name).collect(),
                parents,
                inverse_bind_matrices,
            },
        );
    }
    Ok(())
}

/// Every animation as `Animation/<name>`: translation, rotation and scale
/// channels (morph target weights are skipped), targeting node names.
fn import_animations(
    ctx: &mut ImportContext,
    doc: &Document,
    buffers: &[Vec<u8>],
    labels: &Labels,
    scale: f32,
) -> Result<(), ImportError> {
    let mut name_count: HashMap<String, usize> = HashMap::new();
    for node in doc.nodes() {
        *name_count.entry(node_name(&node)).or_default() += 1;
    }
    let mut warned: HashSet<String> = HashSet::new();
    for anim in doc.animations() {
        let label = &labels.animations[anim.index()];
        let mut channels = Vec::new();
        let mut morphs = false;
        for (ci, channel) in anim.channels().enumerate() {
            let what = format!("{label} channel {ci}");
            let reader = channel.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let decode = |m: &str| ImportError::Decode(format!("{what}: {m}"));
            let times: Vec<f32> = reader
                .read_inputs()
                .ok_or_else(|| decode("unreadable keyframe times"))?
                .collect();
            let values = match reader
                .read_outputs()
                .ok_or_else(|| decode("unreadable keyframe values"))?
            {
                ReadOutputs::Translations(v) => {
                    Keyframes::Translation(v.map(|t| Vec3::from(t) * scale).collect())
                }
                ReadOutputs::Rotations(r) => {
                    Keyframes::Rotation(r.into_f32().map(Quat::from_array).collect())
                }
                ReadOutputs::Scales(v) => Keyframes::Scale(v.map(Vec3::from).collect()),
                ReadOutputs::MorphTargetWeights(_) => {
                    morphs = true;
                    continue;
                }
            };
            let interpolation = match channel.sampler().interpolation() {
                gltf::animation::Interpolation::Step => Interpolation::Step,
                gltf::animation::Interpolation::Linear => Interpolation::Linear,
                gltf::animation::Interpolation::CubicSpline => Interpolation::CubicSpline,
            };
            let target = node_name(&channel.target().node());
            if name_count.get(&target).copied().unwrap_or(0) > 1 && warned.insert(target.clone()) {
                tracing::warn!(
                    "{}: {label} animates `{target}`, a name several nodes share; \
                     it will move the first one found",
                    ctx.path()
                );
            }
            let c = Channel {
                target,
                interpolation,
                times,
                values,
            };
            c.validate().map_err(|e| decode(&e))?;
            channels.push(c);
        }
        if morphs {
            tracing::debug!(
                "{}: {label}: morph target weights are not imported",
                ctx.path()
            );
        }
        ctx.add(Some(label.clone()), AnimationClip::new(channels));
    }
    Ok(())
}

// -------------------------------------------------------------- scene ----

/// A deterministic uid for entity `n` of the scene of asset `id`, so
/// re-imports produce the same scene JSON. Prefab instances remap uids.
fn scene_uid(id: AssetId, n: u64) -> Uid {
    fn mix(mut z: u64) -> u64 {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    let a = id.0.as_u128();
    let hi = mix(a as u64 ^ mix(n));
    let lo = mix((a >> 64) as u64 ^ hi.rotate_left(17) ^ n);
    let bytes = ((u128::from(hi) << 64) | u128::from(lo)).to_le_bytes();
    Uid(uuid::Builder::from_random_bytes(bytes).into_uuid())
}

fn record(
    uid: Uid,
    name: &str,
    parent: Option<Uid>,
    transform: &Transform,
    mesh: Option<&MeshAsset>,
    skin: Option<&SkinBinding>,
    collider: bool,
) -> Result<Json, ImportError> {
    let registry = core_registry();
    let to_json = |v: &dyn bevy_reflect::PartialReflect| {
        registry
            .value_to_json(v)
            .map_err(|e| ImportError::Decode(e.to_string()))
    };
    let mut components = serde_json::Map::new();
    if let Some(m) = mesh {
        components.insert("MeshAsset".into(), to_json(m)?);
    }
    if let Some(s) = skin {
        components.insert("SkinBinding".into(), to_json(s)?);
    }
    if collider {
        // box_physics' `Collider` (by name: this crate does not depend on
        // it); the other fields keep their defaults.
        components.insert("Collider".into(), json!({ "shape": "Mesh" }));
    }
    components.insert("Transform".into(), to_json(transform)?);
    Ok(json!({
        "uid": uid.to_string(),
        "name": name,
        "parent": parent.map(|p| p.to_string()),
        "components": components,
    }))
}

/// A mesh node named `<name>-nocol` (any case) gets no collider from the
/// `colliders` import setting.
pub const NO_COLLIDER_SUFFIX: &str = "-nocol";

fn opts_out_of_collider(name: Option<&str>) -> bool {
    name.is_some_and(|n| {
        n.len() >= NO_COLLIDER_SUFFIX.len()
            && n.is_char_boundary(n.len() - NO_COLLIDER_SUFFIX.len())
            && n[n.len() - NO_COLLIDER_SUFFIX.len()..].eq_ignore_ascii_case(NO_COLLIDER_SUFFIX)
    })
}

fn import_scene(
    ctx: &mut ImportContext,
    doc: &Document,
    labels: &Labels,
    mesh_materials: &[Vec<AssetRef>],
    settings: &ModelSettings,
) -> Result<(), ImportError> {
    let scale = settings.scale;
    let Some(scene) = doc.default_scene().or_else(|| doc.scenes().next()) else {
        return Ok(());
    };
    if doc.scenes().len() > 1 {
        tracing::warn!(
            "{}: has {} scenes; only scene {} is imported",
            ctx.path(),
            doc.scenes().len(),
            scene.index()
        );
    }
    let file = ctx.path().rsplit('/').next().unwrap_or(ctx.path());
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let root = scene_uid(ctx.id(), u64::MAX);
    let mut records = vec![record(
        root,
        stem,
        None,
        &Transform::IDENTITY,
        None,
        None,
        false,
    )?];
    let mut visited = vec![false; doc.nodes().len()];
    let mut stack: Vec<(gltf::Node, Uid)> = scene.nodes().map(|n| (n, root)).collect();
    stack.reverse();
    while let Some((node, parent)) = stack.pop() {
        let i = node.index();
        if std::mem::replace(&mut visited[i], true) {
            tracing::warn!(
                "{}: node {i} appears more than once in the hierarchy; later uses are skipped",
                ctx.path()
            );
            continue;
        }
        let (t, r, s) = node.transform().decomposed();
        let rotation = Quat::from_array(r);
        let rotation = if rotation.is_finite() && rotation.length_squared() > 0.0 {
            rotation.normalize()
        } else {
            Quat::IDENTITY
        };
        let transform = Transform {
            translation: Vec3::from(t) * scale,
            rotation,
            scale: Vec3::from(s),
        };
        let mesh = node.mesh().map(|m| MeshAsset {
            mesh: ctx.sub_asset_ref(&labels.meshes[m.index()]),
            materials: mesh_materials[m.index()].clone(),
            cast_shadows: true,
        });
        // Skinned: the joints are the skin's nodes (their scene entities).
        let skin = match (node.mesh(), node.skin()) {
            (Some(_), Some(skin)) => Some(SkinBinding {
                skin: ctx.sub_asset_ref(&skin_label(skin.index())),
                joints: skin
                    .joints()
                    .map(|j| scene_uid(ctx.id(), j.index() as u64))
                    .collect(),
            }),
            _ => None,
        };
        let name = node_name(&node);
        let uid = scene_uid(ctx.id(), i as u64);
        let collider = settings.colliders
            && mesh.is_some()
            && skin.is_none()
            && !opts_out_of_collider(node.name());
        records.push(record(
            uid,
            &name,
            Some(parent),
            &transform,
            mesh.as_ref(),
            skin.as_ref(),
            collider,
        )?);
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev().map(|c| (c, uid)));
    }
    let scene = SceneAsset {
        json: json!({ "version": SCENE_VERSION, "entities": records }),
    };
    ctx.add(Some("Scene".into()), scene);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_sanitized_and_unique() {
        let names = [
            Some("Roof"),
            None,
            Some("Roof"),
            Some("a/b#c"),
            Some(" "),
            Some("1"),
        ];
        assert_eq!(
            unique_labels("Mesh", names.into_iter()),
            [
                "Mesh/Roof",
                "Mesh/1",
                "Mesh/Roof_2",
                "Mesh/a_b_c",
                "Mesh/4",
                "Mesh/1_5"
            ]
        );
    }

    #[test]
    fn decodes_base64_and_percent_escapes() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
        assert!(base64_decode("a$").is_none());
        assert_eq!(percent_decode("my%20tex.png"), "my tex.png");
        assert_eq!(percent_decode("bad%2"), "bad%2");
        assert_eq!(percent_decode("%"), "%");
    }

    #[test]
    fn merge_makes_one_group_per_primitive_and_fills_channels() {
        let tri = |uv: bool| Primitive {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            normals: vec![Vec3::Z; 3],
            uvs: [uv.then(|| vec![Vec2::ONE; 3]), None, None, None],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        let m = merge(&[tri(true), tri(false)], "m").unwrap();
        m.validate().unwrap();
        assert_eq!(m.groups.len(), 2);
        assert_eq!(m.groups[1].first_index, 3);
        assert_eq!(m.groups[1].material_slot, 1);
        assert_eq!(m.indices[3..], [3, 4, 5]);
        assert_eq!(m.uvs.len(), 1);
        assert_eq!(m.uvs[0][4], Vec2::ZERO);
        assert!(m.colors.is_empty() && m.tangents.is_empty());
    }

    #[test]
    fn packs_orm_from_separate_images() {
        let mr = RgbaImage::from_pixel(2, 2, image::Rgba([0, 100, 200, 255]));
        let occ = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
        let p = pack_orm(Some(&mr), Some((&occ, 0.5)));
        assert_eq!(p.dimensions(), (2, 2));
        assert_eq!(p.get_pixel(1, 1).0, [128, 100, 200, 255]);
        let only_mr = pack_orm(Some(&mr), None);
        assert_eq!(only_mr.get_pixel(0, 0).0, [255, 100, 200, 255]);
    }
}
