//! Samples of the procedural guide (`docs/guide/procedural.md`): a
//! generator ([`Cairn`]) and a typed rule ([`cairn_rows`]) written in Rust.

use box_engine::box_proc::derive::Part;
use box_engine::box_proc::mesh_builder::MeshBuilder;
use box_engine::box_proc::rule::{RuleAppExt, RuleInput, RuleOutput};
use box_engine::prelude::*;

/// A cairn: a stack of stones, each smaller than the one below, shifted
/// and turned at random (seeded by `Procedural::seed`). Material slots:
/// Stone, and Top for the last stone.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct Cairn {
    /// Number of stones.
    #[reflect(@Range { min: 1.0, max: 32.0 })]
    pub stones: u32,
    /// Width of the bottom stone.
    #[reflect(@Range { min: 0.05, max: 5.0 }, @Unit("m"), @Step(0.01))]
    pub size: f32,
    /// How far a stone may shift sideways, as a fraction of its width.
    #[reflect(@Range { min: 0.0, max: 0.5 }, @Step(0.01))]
    pub jitter: f32,
}

impl Default for Cairn {
    fn default() -> Self {
        Self {
            stones: 5,
            size: 0.6,
            jitter: 0.15,
        }
    }
}

/// The most stones one cairn builds.
pub const MAX_STONES: u32 = 32;

impl Generator for Cairn {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Stone", "Top"];
    const CATEGORY: &'static str = "Garden";

    fn generate(&self, ctx: &GenContext) -> GenOutput {
        // Validate and degrade with a warning; never panic.
        let mut warnings = Vec::new();
        let size = if self.size.is_finite() && self.size > 0.0 {
            self.size
        } else {
            warnings.push("size must be above 0; using 0.6".to_string());
            0.6
        };
        let stones = if self.stones > MAX_STONES {
            warnings.push(format!(
                "{} stones is too many; building {MAX_STONES}",
                self.stones
            ));
            MAX_STONES
        } else {
            self.stones.max(1)
        };
        let jitter = if self.jitter.is_finite() {
            self.jitter.clamp(0.0, 0.5)
        } else {
            0.0
        };

        let rng = ctx.rng();
        let mut mesh = MeshBuilder::new();
        let mut y = 0.0;
        for i in 0..stones {
            // One random stream per stone: adding a stone keeps the others.
            let mut r = rng.fork(u64::from(i));
            let width = size * (1.0 - 0.6 * i as f32 / stones as f32);
            let height = width * r.range(0.35, 0.5);
            let shift = Vec3::new(r.range(-jitter, jitter), 0.0, r.range(-jitter, jitter)) * width;
            let turn = Quat::from_rotation_y(r.range(0.0, std::f32::consts::TAU));
            let slot = if i + 1 == stones { 1 } else { 0 };
            let stone = slab(Vec3::new(width, height, width * 0.8), slot);
            let at = shift + Vec3::new(0.0, y + height / 2.0, 0.0);
            mesh.append(&stone, Affine3A::from_rotation_translation(turn, at));
            y += height;
        }
        let mut out = GenOutput::mesh(mesh.finish());
        out.warnings = warnings;
        out
    }
}

/// A box of `size` whose triangles all draw material slot `slot`.
fn slab(size: Vec3, slot: u32) -> MeshData {
    let mut m = MeshData::cuboid(size);
    m.groups = vec![MaterialGroup {
        first_index: 0,
        index_count: m.indices.len() as u32,
        material_slot: slot,
    }];
    m
}

/// Saved intent: a row of cairns along the entity's +X axis. The
/// `cairn_rows` rule turns it into one generated `Cairn` per position.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct CairnRow {
    /// Number of cairns.
    #[reflect(@Range { min: 0.0, max: 100.0 })]
    pub count: u32,
    /// Distance between two cairns.
    #[reflect(@Range { min: 0.1, max: 20.0 }, @Unit("m"), @Step(0.01))]
    pub spacing: f32,
    /// Width of each cairn's bottom stone.
    #[reflect(@Range { min: 0.05, max: 5.0 }, @Unit("m"), @Step(0.01))]
    pub size: f32,
}

impl Default for CairnRow {
    fn default() -> Self {
        Self {
            count: 4,
            spacing: 1.5,
            size: 0.6,
        }
    }
}

/// The `cairn_rows` rule: a pure function from every `CairnRow` to parts.
/// Each part is a `Cairn` with its own seed, keyed `cairn<i>`.
pub fn cairn_rows(input: &RuleInput<CairnRow>) -> RuleOutput {
    let mut out = RuleOutput::default();
    for row in input.intents {
        let r = &row.value;
        if r.spacing < r.size {
            out.warn(
                row.uid,
                "the cairns overlap: spacing is less than their size",
            );
        }
        for i in 0..r.count.min(100) {
            let cairn = Cairn {
                size: r.size,
                ..Cairn::default()
            };
            out.push(
                Part::new(row.key(format!("cairn{i}")))
                    .at(Transform::from_xyz(i as f32 * r.spacing, 0.0, 0.0))
                    .with(cairn)
                    .with(Procedural::with_seed(u64::from(i)))
                    .with(StandardMaterial::color(Color::srgb(0.55, 0.53, 0.5))),
            );
        }
    }
    out
}

/// Registers the procedural samples.
pub struct ProceduralSamplesPlugin;

impl Plugin for ProceduralSamplesPlugin {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(CairnRow,)>();
        // Errors are kept and logged by `Game` with the plugin's name.
        let _ = registry.register_generator::<Cairn>();
    }

    fn build(&self, app: &mut App) {
        app.add_rule("cairn_rows", cairn_rows);
    }
}
