//! Fake editor data: a 10,000-entity tree, a console log, an asset list.

use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Group,
    Mesh,
    Light,
    Camera,
    Prefab,
}

pub struct Entity {
    pub name: String,
    pub name_lower: String,
    pub kind: Kind,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
    pub visible: bool,
}

pub struct Scene {
    pub entities: Vec<Entity>,
    pub roots: Vec<u32>,
}

/// Small deterministic PRNG so captures are reproducible.
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    pub fn range(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

impl Scene {
    fn add(&mut self, name: String, kind: Kind, parent: Option<u32>) -> u32 {
        let id = self.entities.len() as u32;
        self.entities.push(Entity {
            name_lower: name.to_lowercase(),
            name,
            kind,
            parent,
            children: Vec::new(),
            visible: true,
        });
        match parent {
            Some(p) => self.entities[p as usize].children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    pub fn generate(target: usize) -> Self {
        let mut s = Scene {
            entities: Vec::with_capacity(target),
            roots: Vec::new(),
        };
        let mut rng = Rng::new(7);

        let env = s.add("Environment".into(), Kind::Group, None);
        s.add("Sun".into(), Kind::Light, Some(env));
        s.add("Sky Dome".into(), Kind::Mesh, Some(env));
        s.add("Height Fog".into(), Kind::Group, Some(env));
        s.add("Ocean".into(), Kind::Mesh, Some(env));
        let cams = s.add("Cameras".into(), Kind::Group, None);
        s.add("Main Camera".into(), Kind::Camera, Some(cams));
        s.add("Overview Camera".into(), Kind::Camera, Some(cams));
        s.add("Player Spawn".into(), Kind::Prefab, None);

        let districts = [
            "Harbor District",
            "Old Town",
            "Market Square",
            "Cliffside",
            "Lighthouse Point",
            "Canal Quarter",
            "Upper Terraces",
            "Fishing Docks",
        ];
        let parts = [
            "Foundation",
            "Walls",
            "Roof",
            "Chimney",
            "Door",
            "Window Array",
            "Balcony",
            "Awning",
            "Stairs",
            "Gutter",
        ];
        let props = [
            "Crate",
            "Barrel",
            "Lamp Post",
            "Bench",
            "Planter",
            "Rope Coil",
            "Net Rack",
            "Cart",
            "Sign",
            "Bollard",
        ];
        let mut pass = 0;
        'outer: loop {
            pass += 1;
            for d in districts {
                let name = if pass == 1 {
                    d.to_string()
                } else {
                    format!(
                        "{d} {}",
                        ["", "", "East", "West", "North", "South"][pass.min(5)]
                    )
                };
                let dist = s.add(name, Kind::Group, None);
                let n_blocks = 6 + rng.range(6);
                for b in 0..n_blocks {
                    let block = s.add(format!("Block {:02}", b + 1), Kind::Group, Some(dist));
                    let n_buildings = 3 + rng.range(5);
                    for h in 0..n_buildings {
                        let style = ["Townhouse", "Warehouse", "Cottage", "Tower", "Shopfront"]
                            [rng.range(5) as usize];
                        let bld = s.add(
                            format!("{style}_{:03}", h + 1 + b * 10),
                            Kind::Prefab,
                            Some(block),
                        );
                        let n_parts = 4 + rng.range(6) as usize;
                        for p in parts.iter().take(n_parts) {
                            let part = s.add((*p).into(), Kind::Mesh, Some(bld));
                            if *p == "Window Array" {
                                for r in 0..(1 + rng.range(3)) {
                                    let row =
                                        s.add(format!("Floor {}", r + 1), Kind::Group, Some(part));
                                    for w in 0..(2 + rng.range(3)) {
                                        s.add(
                                            format!("Window {:02}", w + 1),
                                            Kind::Mesh,
                                            Some(row),
                                        );
                                    }
                                }
                            }
                        }
                        if rng.range(4) == 0 {
                            s.add("Interior Light".into(), Kind::Light, Some(bld));
                        }
                    }
                    let props_grp = s.add("Props".into(), Kind::Group, Some(block));
                    for i in 0..(3 + rng.range(8)) {
                        let p = props[rng.range(props.len() as u32) as usize];
                        s.add(format!("{p} {:02}", i + 1), Kind::Mesh, Some(props_grp));
                    }
                    if s.entities.len() >= target {
                        break 'outer;
                    }
                }
            }
        }
        // Trim to exactly `target`. Later ids are always descendants-or-later-siblings, so
        // dropping the tail keeps the tree consistent once dangling child ids are removed.
        s.entities.truncate(target);
        let n = target as u32;
        for e in &mut s.entities {
            e.children.retain(|&c| c < n);
        }
        s.roots.retain(|&r| r < n);
        // A few hidden entities so the visibility state has something to show.
        for i in (40..s.entities.len()).step_by(97) {
            s.entities[i].visible = false;
        }
        s
    }
}

#[derive(Clone, Copy)]
pub struct Row {
    pub id: u32,
    pub depth: u16,
}

/// Flattens the visible part of the tree into rows, the input to `ScrollArea::show_rows`.
/// Without a filter, children of expanded nodes are visible. With a filter, matches and
/// their ancestors are shown, fully expanded.
pub fn flatten(scene: &Scene, expanded: &HashSet<u32>, filter: &str, out: &mut Vec<Row>) {
    out.clear();
    if filter.is_empty() {
        let mut stack: Vec<(u32, u16)> = scene.roots.iter().rev().map(|&r| (r, 0)).collect();
        while let Some((id, depth)) = stack.pop() {
            out.push(Row { id, depth });
            if expanded.contains(&id) {
                for &c in scene.entities[id as usize].children.iter().rev() {
                    stack.push((c, depth + 1));
                }
            }
        }
    } else {
        let f = filter.to_lowercase();
        let n = scene.entities.len();
        let mut keep = vec![false; n];
        for (i, e) in scene.entities.iter().enumerate() {
            if e.name_lower.contains(&f) {
                let mut cur = Some(i as u32);
                while let Some(c) = cur {
                    if keep[c as usize] {
                        break;
                    }
                    keep[c as usize] = true;
                    cur = scene.entities[c as usize].parent;
                }
            }
        }
        let mut stack: Vec<(u32, u16)> = scene
            .roots
            .iter()
            .rev()
            .filter(|r| keep[**r as usize])
            .map(|&r| (r, 0))
            .collect();
        while let Some((id, depth)) = stack.pop() {
            out.push(Row { id, depth });
            for &c in scene.entities[id as usize].children.iter().rev() {
                if keep[c as usize] {
                    stack.push((c, depth + 1));
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

pub struct LogLine {
    pub time: String,
    pub level: Level,
    pub source: &'static str,
    pub text: String,
}

pub fn generate_log(n: usize) -> Vec<LogLine> {
    let mut rng = Rng::new(3);
    let infos = [
        (
            "assets",
            "Imported meshes/townhouse_a.glb (4 meshes, 12,480 tris) in 38 ms",
        ),
        (
            "render",
            "Shadow cascades rebuilt: 4 x 2048, split lambda 0.72",
        ),
        (
            "scene",
            "Loaded scenes/harbor_district.scene (10,000 entities)",
        ),
        ("shader", "Recompiled shaders/lit.wgsl in 11 ms"),
        (
            "assets",
            "Texture textures/brick_weathered_albedo.png: 2048x2048, 8 mips",
        ),
        ("editor", "Layout restored from .box/layout.json"),
        (
            "procgen",
            "Window Array regenerated: 24 instances, seed 1187",
        ),
        ("render", "Pipeline cache warm: 37 pipelines"),
        ("assets", "Material M_Roof_Slate updated, 112 users"),
        (
            "scene",
            "Autosave written to .box/recovery/harbor_district.scene",
        ),
    ];
    let warns = [
        (
            "assets",
            "Texture textures/sail_canvas.png is not a power of two (1000x750)",
        ),
        (
            "render",
            "Light 'Interior Light' exceeds 16 shadow-casting lights; shadows disabled",
        ),
        (
            "procgen",
            "Roof generator: footprint concave, falling back to flat roof",
        ),
        (
            "scene",
            "Entity 'Bollard 07' has a non-uniform negative scale",
        ),
    ];
    let errors = [
        (
            "shader",
            "shaders/water.wgsl:142:17 unknown identifier 'foam_mask'",
        ),
        (
            "assets",
            "Failed to decode textures/flag_harbor.ktx2: unsupported format BC6H",
        ),
    ];
    (0..n)
        .map(|i| {
            let r = rng.range(100);
            let (level, (source, text)) = if r < 6 {
                (
                    Level::Error,
                    errors[rng.range(errors.len() as u32) as usize],
                )
            } else if r < 20 {
                (Level::Warn, warns[rng.range(warns.len() as u32) as usize])
            } else {
                (Level::Info, infos[rng.range(infos.len() as u32) as usize])
            };
            let secs = 3 + i * 2 + rng.range(2) as usize;
            LogLine {
                time: format!(
                    "{:02}:{:02}:{:02}.{:03}",
                    14,
                    (12 + secs / 60) % 60,
                    secs % 60,
                    rng.range(1000)
                ),
                level,
                source,
                text: text.to_string(),
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Folder,
    Mesh,
    Material,
    Texture,
    Scene,
}

pub struct Asset {
    pub name: &'static str,
    pub kind: AssetKind,
    pub tint: [u8; 3],
}

pub fn assets() -> Vec<Asset> {
    use AssetKind::*;
    let a = |name, kind, tint| Asset { name, kind, tint };
    vec![
        a("meshes", Folder, [0; 3]),
        a("textures", Folder, [0; 3]),
        a("scenes", Folder, [0; 3]),
        a("M_Brick_Weathered", Material, [0xa0, 0x5a, 0x48]),
        a("M_Plaster_Warm", Material, [0xd8, 0xc8, 0xa8]),
        a("M_Roof_Slate", Material, [0x4a, 0x55, 0x60]),
        a("M_Timber_Dark", Material, [0x5a, 0x40, 0x30]),
        a("M_Window_Glass", Material, [0x9c, 0xbc, 0xcc]),
        a("M_Sea_Stylized", Material, [0x3c, 0x7c, 0x8c]),
        a("townhouse_a", Mesh, [0; 3]),
        a("warehouse_b", Mesh, [0; 3]),
        a("lamp_post", Mesh, [0; 3]),
        a("crate_small", Mesh, [0; 3]),
        a("brick_albedo", Texture, [0; 3]),
        a("slate_normal", Texture, [0; 3]),
        a("harbor_district", Scene, [0; 3]),
    ]
}
