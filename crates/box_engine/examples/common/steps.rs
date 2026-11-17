//! `Steps`: a small game-defined generator, used by `examples/generator.rs`
//! and the procedural tests (runtime, latency, bake, golden hashes and
//! images). It shows everything a generator needs: reflected parameters
//! with ranges and units, a `Default`, `VERSION`, material slots,
//! validation with warnings, and seeded randomness.

use box_engine::prelude::*;

/// A straight flight of steps climbing towards −Z. Treads draw material
/// slot 0, risers, sides and the back slot 1. `wobble` varies each step's
/// height randomly (seeded by `Procedural::seed`).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct Steps {
    /// Number of steps.
    #[reflect(@Range { min: 1.0, max: 64.0 })]
    pub count: u32,
    /// Width across the flight.
    #[reflect(@Range { min: 0.1, max: 20.0 }, @Unit("m"), @Step(0.01))]
    pub width: f32,
    /// Height of one step.
    #[reflect(@Range { min: 0.01, max: 2.0 }, @Unit("m"), @Step(0.01))]
    pub rise: f32,
    /// Depth of one step.
    #[reflect(@Range { min: 0.01, max: 4.0 }, @Unit("m"), @Step(0.01))]
    pub run: f32,
    /// Random height variation per step.
    #[reflect(@Range { min: 0.0, max: 0.2 }, @Unit("m"), @Step(0.005))]
    pub wobble: f32,
}

impl Default for Steps {
    fn default() -> Self {
        Self {
            count: 6,
            width: 1.6,
            rise: 0.18,
            run: 0.3,
            wobble: 0.0,
        }
    }
}

/// The most steps one flight builds.
pub const MAX_STEPS: u32 = 64;

impl Generator for Steps {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Tread", "Riser"];

    fn generate(&self, ctx: &GenContext) -> GenOutput {
        let mut warnings = Vec::new();
        let mut positive = |name: &str, v: f32, fallback: f32| {
            if v.is_finite() && v > 0.0 {
                v
            } else {
                warnings.push(format!("{name} must be above 0; using {fallback}"));
                fallback
            }
        };
        let width = positive("width", self.width, 1.0);
        let rise = positive("rise", self.rise, 0.18);
        let run = positive("run", self.run, 0.3);
        let count = if self.count > MAX_STEPS {
            warnings.push(format!(
                "{} steps is too many; building {MAX_STEPS}",
                self.count
            ));
            MAX_STEPS
        } else {
            self.count.max(1)
        };
        let wobble = if self.wobble.is_finite() {
            self.wobble.abs()
        } else {
            0.0
        };

        let rng = ctx.rng();
        let mut q = Quads::default();
        let hw = width * 0.5;
        for i in 0..count {
            let jitter = rng.fork(u64::from(i)).range(-wobble, wobble);
            let top = (rise * (i + 1) as f32 + jitter).max(rise * 0.25);
            let (front, back) = (-(i as f32) * run, -((i + 1) as f32) * run);
            let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
            // Tread (+Y).
            q.quad(
                0,
                [
                    p(-hw, top, front),
                    p(hw, top, front),
                    p(hw, top, back),
                    p(-hw, top, back),
                ],
                1.0,
            );
            // Riser (+Z), sides, back (−Z) and bottom.
            q.quad(
                1,
                [
                    p(-hw, 0.0, front),
                    p(hw, 0.0, front),
                    p(hw, top, front),
                    p(-hw, top, front),
                ],
                0.8,
            );
            q.quad(
                1,
                [
                    p(hw, 0.0, front),
                    p(hw, 0.0, back),
                    p(hw, top, back),
                    p(hw, top, front),
                ],
                0.9,
            );
            q.quad(
                1,
                [
                    p(-hw, 0.0, back),
                    p(-hw, 0.0, front),
                    p(-hw, top, front),
                    p(-hw, top, back),
                ],
                0.9,
            );
            q.quad(
                1,
                [
                    p(hw, 0.0, back),
                    p(-hw, 0.0, back),
                    p(-hw, top, back),
                    p(hw, top, back),
                ],
                0.85,
            );
            q.quad(
                1,
                [
                    p(-hw, 0.0, back),
                    p(hw, 0.0, back),
                    p(hw, 0.0, front),
                    p(-hw, 0.0, front),
                ],
                0.7,
            );
        }
        let mut out = GenOutput::mesh(q.finish().with_tangents());
        out.warnings = warnings;
        out
    }
}

/// Quads by material slot, with hard normals, world-space metre UVs and a
/// grey vertex colour for cheap shading contrast.
#[derive(Default)]
struct Quads {
    slots: [Vec<([Vec3; 4], f32)>; 2],
}

impl Quads {
    /// A quad with corners counter-clockwise seen from its front.
    fn quad(&mut self, slot: usize, corners: [Vec3; 4], shade: f32) {
        self.slots[slot].push((corners, shade));
    }

    fn finish(self) -> MeshData {
        let mut m = MeshData {
            uvs: vec![Vec::new()],
            ..MeshData::default()
        };
        for (slot, quads) in self.slots.iter().enumerate() {
            let first = m.indices.len() as u32;
            for (c, shade) in quads {
                let n = (c[1] - c[0]).cross(c[2] - c[0]).normalize_or(Vec3::Y);
                // UV axes in the quad's plane.
                let u_axis = (c[1] - c[0]).normalize_or(Vec3::X);
                let v_axis = n.cross(u_axis);
                let base = m.positions.len() as u32;
                for p in c {
                    m.positions.push(*p);
                    m.normals.push(n);
                    m.uvs[0].push(Vec2::new(p.dot(u_axis), p.dot(v_axis)));
                    m.colors.push([*shade, *shade, *shade, 1.0]);
                }
                m.indices
                    .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
            let count = m.indices.len() as u32 - first;
            if count > 0 {
                m.groups.push(MaterialGroup {
                    first_index: first,
                    index_count: count,
                    material_slot: slot as u32,
                });
            }
        }
        m
    }
}

/// Registers `Steps`: what a game's plugin does in `Plugin::register`.
pub struct StepsPlugin;

impl Plugin for StepsPlugin {
    fn register(&self, registry: &mut Registry) {
        let _ = registry.register_generator::<Steps>();
    }

    fn build(&self, _app: &mut App) {}
}

/// Two flights of steps on a ground plane: a plain one and a longer,
/// wobbly one with its own seed.
pub fn build_steps_scene(world: &mut World) {
    world.spawn((
        Transform::default(),
        Primitive::plane(Vec2::splat(12.0)),
        StandardMaterial {
            base_color: Color::hex(0x9fb08a),
            roughness: 0.95,
            ..Default::default()
        },
    ));
    world.spawn((
        Transform::from_xyz(-1.4, 0.0, 1.5),
        Steps::default(),
        StandardMaterial {
            base_color: Color::hex(0xd8c8a8),
            roughness: 0.8,
            ..Default::default()
        },
    ));
    world.spawn((
        Transform::from_xyz(1.4, 0.0, 1.8).with_rotation(Quat::from_rotation_y(-0.35)),
        Steps {
            count: 10,
            width: 1.2,
            wobble: 0.04,
            ..Steps::default()
        },
        Procedural::with_seed(3),
        StandardMaterial {
            base_color: Color::hex(0xb07a5a),
            roughness: 0.7,
            ..Default::default()
        },
    ));
}

/// A camera framing [`build_steps_scene`].
pub fn steps_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.0, 0.6, 0.0),
        distance: 7.5,
        yaw: 0.6,
        pitch: 0.4,
        ..Default::default()
    }
}
