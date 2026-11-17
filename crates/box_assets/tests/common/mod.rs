//! Test helpers: a temporary project made from the fixture corpus, and the
//! generator that writes the binary fixtures (`gen_fixtures.rs`).

#![allow(dead_code)]

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use box_assets::AssetServer;
use box_core::world::World;
use serde_json::{Value, json};

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A copy of the fixture corpus in a fresh temporary folder, removed on drop.
pub struct TempAssets {
    pub dir: PathBuf,
}

impl TempAssets {
    pub fn new(name: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "box_assets_{name}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&fixtures_dir(), &dir);
        Self { dir }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    pub fn server(&self) -> AssetServer {
        AssetServer::new(&self.dir).expect("asset server")
    }
}

impl Drop for TempAssets {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// Updates until idle; panics after 30 s.
pub fn settle(server: &AssetServer, world: &mut World) {
    assert!(
        server.update_until_idle(world, Duration::from_secs(30)),
        "imports did not finish"
    );
}

// ------------------------------------------------------------ generator ----

pub fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

pub fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x * 30) as u8, (y * 30) as u8, 128])
    });
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Jpeg)
        .unwrap();
    out
}

pub fn checker_png() -> Vec<u8> {
    png(4, 4, |x, y| {
        if (x + y) % 2 == 0 {
            [200, 40, 40, 255]
        } else {
            [255, 255, 255, 255]
        }
    })
}

/// Builds glTF JSON plus one binary buffer.
pub struct Gltf {
    pub json: Value,
    pub bin: Vec<u8>,
}

impl Gltf {
    pub fn new() -> Self {
        Self {
            json: json!({
                "asset": {"version": "2.0", "generator": "box_assets fixture generator"},
                "buffers": [], "bufferViews": [], "accessors": [],
                "meshes": [], "materials": [], "textures": [], "images": [],
                "nodes": [], "scenes": []
            }),
            bin: Vec::new(),
        }
    }

    fn push(&mut self, key: &str, v: Value) -> usize {
        let a = self.json[key].as_array_mut().unwrap();
        a.push(v);
        a.len() - 1
    }

    pub fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.push("bufferViews", v)
    }

    pub fn floats(&mut self, data: &[f32], ty: &str, comps: usize) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34962));
        let count = data.len() / comps;
        let mut acc =
            json!({"bufferView": view, "componentType": 5126, "count": count, "type": ty});
        if ty == "VEC3" {
            let mut min = [f32::MAX; 3];
            let mut max = [f32::MIN; 3];
            for v in data.chunks(3) {
                for i in 0..3 {
                    min[i] = min[i].min(v[i]);
                    max[i] = max[i].max(v[i]);
                }
            }
            acc["min"] = json!(min);
            acc["max"] = json!(max);
        }
        self.push("accessors", acc)
    }

    pub fn indices_u16(&mut self, data: &[u16]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.push(
            "accessors",
            json!({"bufferView": view, "componentType": 5123, "count": data.len(), "type": "SCALAR"}),
        )
    }

    pub fn indices_u32(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(34963));
        self.push(
            "accessors",
            json!({"bufferView": view, "componentType": 5125, "count": data.len(), "type": "SCALAR"}),
        )
    }

    pub fn embedded_image(&mut self, png: &[u8]) -> usize {
        let view = self.view(png, None);
        self.push(
            "images",
            json!({"bufferView": view, "mimeType": "image/png"}),
        )
    }

    pub fn uri_image(&mut self, uri: &str) -> usize {
        self.push("images", json!({"uri": uri}))
    }

    pub fn texture(&mut self, image: usize) -> usize {
        self.push("textures", json!({"source": image}))
    }

    pub fn material(&mut self, m: Value) -> usize {
        self.push("materials", m)
    }

    pub fn mesh(&mut self, m: Value) -> usize {
        self.push("meshes", m)
    }

    pub fn node(&mut self, n: Value) -> usize {
        self.push("nodes", n)
    }

    pub fn scene(&mut self, nodes: &[usize]) {
        self.push("scenes", json!({"nodes": nodes}));
        self.json["scene"] = json!(0);
    }

    fn finish_json(&mut self, buffer: Value) -> Value {
        let mut j = self.json.clone();
        j["buffers"] = json!([buffer]);
        // Drop empty arrays (glTF wants them absent or non-empty).
        let obj = j.as_object_mut().unwrap();
        obj.retain(|_, v| !v.as_array().is_some_and(Vec::is_empty));
        j
    }

    pub fn glb(mut self) -> Vec<u8> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let j = self.finish_json(json!({"byteLength": self.bin.len()}));
        let mut js = serde_json::to_vec(&j).unwrap();
        while !js.len().is_multiple_of(4) {
            js.push(b' ');
        }
        let total = 12 + 8 + js.len() + 8 + self.bin.len();
        let mut out = Vec::new();
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((js.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(js);
        out.extend((self.bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(&self.bin);
        out
    }

    /// `.gltf` JSON referencing `bin_uri` (a file name or a data URI).
    pub fn gltf(mut self, bin_uri: &str) -> (Vec<u8>, Vec<u8>) {
        let j = self.finish_json(json!({"byteLength": self.bin.len(), "uri": bin_uri}));
        (serde_json::to_vec_pretty(&j).unwrap(), self.bin)
    }
}

/// 24-vertex unit cube: positions, normals, uv0, u16 indices.
pub fn cube() -> (Vec<f32>, Vec<f32>, Vec<f32>, Vec<u16>) {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1., 0., 0.], [0., 1., 0.], [0., 0., -1.]),
        ([-1., 0., 0.], [0., 1., 0.], [0., 0., 1.]),
        ([0., 1., 0.], [0., 0., -1.], [1., 0., 0.]),
        ([0., -1., 0.], [0., 0., 1.], [1., 0., 0.]),
        ([0., 0., 1.], [0., 1., 0.], [1., 0., 0.]),
        ([0., 0., -1.], [0., 1., 0.], [-1., 0., 0.]),
    ];
    let (mut p, mut n, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
    for (fi, (normal, up, right)) in faces.iter().enumerate() {
        for (k, (su, sv)) in [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
            .iter()
            .enumerate()
        {
            for a in 0..3 {
                p.push(0.5 * (normal[a] + su * right[a] + sv * up[a]));
            }
            n.extend(normal);
            uv.extend([[0., 1.], [1., 1.], [1., 0.], [0., 0.]][k]);
        }
        let b = (fi * 4) as u16;
        idx.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    (p, n, uv, idx)
}

pub fn tower_glb() -> Vec<u8> {
    let mut g = Gltf::new();
    let color = g.embedded_image(&checker_png());
    let mr = g.embedded_image(&png(2, 2, |_, _| [0, 128, 255, 255]));
    let color_tex = g.texture(color);
    let mr_tex = g.texture(mr);
    let stone = g.material(json!({
        "name": "Stone",
        "pbrMetallicRoughness": {
            "baseColorTexture": {"index": color_tex},
            "metallicRoughnessTexture": {"index": mr_tex},
            "roughnessFactor": 0.9, "metallicFactor": 0.0
        }
    }));
    let roof = g.material(json!({
        "name": "Roof Tiles",
        "pbrMetallicRoughness": {"baseColorFactor": [0.6, 0.1, 0.1, 1.0], "roughnessFactor": 0.5, "metallicFactor": 0.25},
        "emissiveFactor": [0.1, 0.0, 0.0],
        "extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 2.0}},
        "alphaMode": "MASK", "alphaCutoff": 0.3, "doubleSided": true
    }));
    g.json["extensionsUsed"] = json!(["KHR_materials_emissive_strength"]);
    let (p, n, uv, idx) = cube();
    let (pa, na, ua, ia) = (
        g.floats(&p, "VEC3", 3),
        g.floats(&n, "VEC3", 3),
        g.floats(&uv, "VEC2", 2),
        g.indices_u16(&idx),
    );
    let base = g.mesh(json!({"name": "Base", "primitives": [{
        "attributes": {"POSITION": pa, "NORMAL": na, "TEXCOORD_0": ua}, "indices": ia, "material": stone
    }]}));
    // Roof: four flat-shaded sides (no normals), plus a bottom quad with
    // normals but no UVs.
    let apex = [0.0f32, 0.5, 0.0];
    let corners = [
        [-0.5f32, 0.0, 0.5],
        [0.5, 0.0, 0.5],
        [0.5, 0.0, -0.5],
        [-0.5, 0.0, -0.5],
    ];
    let mut side_p = Vec::new();
    let mut side_uv = Vec::new();
    for i in 0..4 {
        for v in [corners[i], corners[(i + 1) % 4], apex] {
            side_p.extend(v);
        }
        side_uv.extend([0.0f32, 1.0, 1.0, 1.0, 0.5, 0.0]);
    }
    let sp = g.floats(&side_p, "VEC3", 3);
    let su = g.floats(&side_uv, "VEC2", 2);
    let bottom_p: Vec<f32> = corners.iter().flatten().copied().collect();
    let bp = g.floats(&bottom_p, "VEC3", 3);
    let bn = g.floats(
        &[0., -1., 0., 0., -1., 0., 0., -1., 0., 0., -1., 0.],
        "VEC3",
        3,
    );
    let bi = g.indices_u16(&[0, 2, 1, 0, 3, 2]);
    let roof_mesh = g.mesh(json!({"name": "Roof", "primitives": [
        {"attributes": {"POSITION": sp, "TEXCOORD_0": su}, "material": roof},
        {"attributes": {"POSITION": bp, "NORMAL": bn}, "indices": bi, "material": stone}
    ]}));
    let flag = g.node(json!({"name": "Flag", "scale": [0.5, 0.5, 0.5]}));
    let rot = (std::f32::consts::FRAC_PI_8).sin();
    let roof_node = g.node(json!({
        "name": "Roof", "mesh": roof_mesh, "translation": [0.0, 1.0, 0.0],
        "rotation": [0.0, rot, 0.0, (std::f32::consts::FRAC_PI_8).cos()], "children": [flag]
    }));
    let base_node = g.node(json!({"name": "Base", "mesh": base}));
    let tower = g.node(json!({"name": "Tower", "translation": [0.0, 0.0, -2.0], "children": [base_node, roof_node]}));
    g.scene(&[tower]);
    g.glb()
}

/// `crate.gltf` + `crate.bin`, with external textures `crate_color.png`
/// and `crate_mr.png` (metallic-roughness and occlusion in one image).
pub fn crate_gltf() -> (Vec<u8>, Vec<u8>) {
    let mut g = Gltf::new();
    let color = g.uri_image("crate_color.png");
    let orm = g.uri_image("crate_mr.png");
    let ct = g.texture(color);
    let ot = g.texture(orm);
    let mat = g.material(json!({
        "name": "Crate",
        "pbrMetallicRoughness": {"baseColorTexture": {"index": ct}, "metallicRoughnessTexture": {"index": ot}},
        "occlusionTexture": {"index": ot}
    }));
    let p = [-0.5f32, 0., 0., 0.5, 0., 0., 0.5, 1., 0., -0.5, 1., 0.];
    let n = [0f32, 0., 1., 0., 0., 1., 0., 0., 1., 0., 0., 1.];
    let uv = [0f32, 1., 1., 1., 1., 0., 0., 0.];
    let t = [
        1f32, 0., 0., 1., 1., 0., 0., 1., 1., 0., 0., 1., 1., 0., 0., 1.,
    ];
    let c = [
        1f32, 0., 0., 1., 0., 1., 0., 1., 0., 0., 1., 1., 1., 1., 1., 1.,
    ];
    let (pa, na, ua, ta, ca) = (
        g.floats(&p, "VEC3", 3),
        g.floats(&n, "VEC3", 3),
        g.floats(&uv, "VEC2", 2),
        g.floats(&t, "VEC4", 4),
        g.floats(&c, "VEC4", 4),
    );
    let ia = g.indices_u32(&[0, 1, 2, 0, 2, 3]);
    let m = g.mesh(json!({"name": "Crate", "primitives": [{
        "attributes": {"POSITION": pa, "NORMAL": na, "TEXCOORD_0": ua, "TANGENT": ta, "COLOR_0": ca},
        "indices": ia, "material": mat
    }]}));
    let node = g.node(json!({"name": "Crate", "mesh": m}));
    g.scene(&[node]);
    g.gltf("crate.bin")
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// One triangle with its buffer in a data URI and a base colour texture
/// that does not exist.
pub fn missing_texture_gltf() -> Vec<u8> {
    let mut g = Gltf::new();
    let img = g.uri_image("nope.png");
    let t = g.texture(img);
    let mat = g.material(
        json!({"name": "Ghost", "pbrMetallicRoughness": {"baseColorTexture": {"index": t}}}),
    );
    let pa = g.floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.], "VEC3", 3);
    let m = g.mesh(json!({"primitives": [{"attributes": {"POSITION": pa}, "material": mat}]}));
    let node = g.node(json!({"mesh": m}));
    g.scene(&[node]);
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        b64(&g.bin.clone())
    );
    g.gltf(&uri).0
}

/// An accessor pointing at a buffer view that does not exist.
pub fn bad_buffer_gltf() -> Vec<u8> {
    let mut g = Gltf::new();
    let pa = g.floats(&[0., 0., 0., 1., 0., 0., 0., 1., 0.], "VEC3", 3);
    g.json["accessors"][pa]["bufferView"] = json!(7);
    let m = g.mesh(json!({"primitives": [{"attributes": {"POSITION": pa}}]}));
    let node = g.node(json!({"mesh": m}));
    g.scene(&[node]);
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        b64(&g.bin.clone())
    );
    g.gltf(&uri).0
}

/// Requires Draco.
pub fn draco_gltf() -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "asset": {"version": "2.0"},
        "extensionsUsed": ["KHR_draco_mesh_compression"],
        "extensionsRequired": ["KHR_draco_mesh_compression"],
        "meshes": [{"primitives": [{"attributes": {}, "extensions": {"KHR_draco_mesh_compression": {"bufferView": 0, "attributes": {"POSITION": 0}}}}]}]
    }))
    .unwrap()
}

/// A flat grid of about `triangles` triangles with normals and UVs (no
/// tangents, so import generates them), for timing.
pub fn grid_glb(triangles: usize) -> Vec<u8> {
    let cells = ((triangles as f64 / 2.0).sqrt().ceil()) as u32;
    let verts = cells + 1;
    let (mut p, mut n, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
    for z in 0..verts {
        for x in 0..verts {
            let (u, v) = (x as f32 / cells as f32, z as f32 / cells as f32);
            p.extend([
                u * 100.0 - 50.0,
                (u * 13.0).sin() * (v * 7.0).cos(),
                v * 100.0 - 50.0,
            ]);
            n.extend([0.0f32, 1.0, 0.0]);
            uv.extend([u * 8.0, v * 8.0]);
        }
    }
    for z in 0..cells {
        for x in 0..cells {
            let i = z * verts + x;
            idx.extend([i, i + verts, i + 1, i + 1, i + verts, i + verts + 1]);
        }
    }
    let mut g = Gltf::new();
    let (pa, na, ua, ia) = (
        g.floats(&p, "VEC3", 3),
        g.floats(&n, "VEC3", 3),
        g.floats(&uv, "VEC2", 2),
        g.indices_u32(&idx),
    );
    let m = g.mesh(json!({"name": "Grid", "primitives": [{"attributes": {"POSITION": pa, "NORMAL": na, "TEXCOORD_0": ua}, "indices": ia}]}));
    let node = g.node(json!({"name": "Grid", "mesh": m}));
    g.scene(&[node]);
    g.glb()
}

fn meta(id: u32, importer: &str, settings: Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(&json!({
        "version": 1,
        "id": format!("10000000-0000-4000-8000-{id:012}"),
        "importer": importer,
        "settings": settings
    }))
    .unwrap();
    out.push(b'\n');
    out
}

/// Writes the fixture corpus to `dir` (see `gen_fixtures.rs`).
pub fn write_fixtures(dir: &Path) {
    let w = |rel: &str, bytes: &[u8]| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    let tex = json!({"color_space": "srgb", "mipmaps": true});
    let model = json!({"scale": 1.0, "generate_tangents": true});
    w("textures/checker.png", &checker_png());
    w(
        "textures/checker.png.meta",
        &meta(1, "texture", tex.clone()),
    );
    // photo.jpg has no .meta on purpose: the server creates it.
    w("textures/photo.jpg", &jpeg(8, 8));
    w("models/tower.glb", &tower_glb());
    w("models/tower.glb.meta", &meta(2, "gltf", model.clone()));
    let (crate_json, crate_bin) = crate_gltf();
    w("models/crate.gltf", &crate_json);
    w("models/crate.gltf.meta", &meta(3, "gltf", model.clone()));
    w("models/crate.bin", &crate_bin);
    w(
        "models/crate_color.png",
        &png(2, 2, |x, _| [x as u8 * 255, 100, 50, 255]),
    );
    w(
        "models/crate_color.png.meta",
        &meta(4, "texture", tex.clone()),
    );
    w("models/crate_mr.png", &png(2, 2, |_, _| [255, 200, 0, 255]));
    w(
        "models/crate_mr.png.meta",
        &meta(
            5,
            "texture",
            json!({"color_space": "linear", "mipmaps": true}),
        ),
    );
    w(
        "materials/stone.material.json",
        &serde_json::to_vec_pretty(&json!({
            "base_color": [1.0, 1.0, 1.0, 1.0],
            "roughness": 0.8,
            "metallic": 0.0,
            "emissive": [0.0, 0.0, 0.0, 1.0],
            "base_color_texture": "textures/checker.png",
            "double_sided": true
        }))
        .unwrap(),
    );
    w(
        "materials/stone.material.json.meta",
        &meta(6, "material", json!({})),
    );
    w(
        "scenes/level.scene.json",
        &serde_json::to_vec_pretty(&json!({
            "version": 1,
            "entities": [{
                "uid": "20000000-0000-4000-8000-000000000001",
                "name": "Ground",
                "parent": null,
                "components": {"Transform": {"translation": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]}}
            }]
        }))
        .unwrap(),
    );
    w("scenes/level.scene.json.meta", &meta(7, "scene", json!({})));
    // Malformed files.
    w("bad/truncated.png", &checker_png()[..40]);
    w("bad/truncated.png.meta", &meta(11, "texture", tex.clone()));
    let mut garbage = b"glTF\x02\x00\x00\x00\xff\x00\x00\x00".to_vec();
    garbage.extend((0..200u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
    w("bad/garbage.glb", &garbage);
    w("bad/garbage.glb.meta", &meta(12, "gltf", model.clone()));
    w("bad/bad_buffer.gltf", &bad_buffer_gltf());
    w("bad/bad_buffer.gltf.meta", &meta(13, "gltf", model.clone()));
    w("bad/missing_texture.gltf", &missing_texture_gltf());
    w(
        "bad/missing_texture.gltf.meta",
        &meta(14, "gltf", model.clone()),
    );
    w("bad/draco.gltf", &draco_gltf());
    w("bad/draco.gltf.meta", &meta(15, "gltf", model.clone()));
    w("bad/empty.glb", b"");
    w("bad/empty.glb.meta", &meta(16, "gltf", model));
    w("bad/bad_meta.png", &checker_png());
    w("bad/bad_meta.png.meta", b"{ \"version\": 1, \"id\": ");
    w("bad/wrong.material.json", br#"{ "roughness": "very" }"#);
    w(
        "bad/wrong.material.json.meta",
        &meta(17, "material", json!({})),
    );
}
