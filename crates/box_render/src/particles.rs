//! CPU billboard particles: the [`ParticleEmitter`] component, its runtime
//! [`ParticleState`], and [`simulate_particles`], the system that advances
//! them. The renderer draws every emitter's live particles as camera-facing
//! quads, instanced (particle_render.rs).
//!
//! Simulation runs once per frame in `PostUpdate`, after transform
//! propagation, with `Time::delta` (so time scale and pause apply): emitters
//! read the transforms of this frame, so a trail on a projectile moved in
//! `Update` or `FixedUpdate` starts exactly at the projectile. Particles
//! are visual only and need no fixed step; with equal frame times (headless
//! runs use 1/60 s) a seeded emitter gives the same particles every run.

use box_core::asset_ref::AssetRef;
use box_core::color::Color;
use box_core::math::Aabb;
use box_core::reflect::{
    Logarithmic, Range, Reflect, ReflectComponent, ReflectDefault, Step, Unit,
};
use box_core::time::Time;
use box_core::transform::GlobalTransform;
use box_core::world::{Entity, World};
use glam::{Affine3A, Quat, UVec2, Vec2, Vec3, Vec4};

/// Most particles one emitter can hold (`max_particles` is clamped to it).
pub const MAX_PARTICLES_PER_EMITTER: u32 = 100_000;

/// How particles are blended over the scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ParticleBlend {
    /// Alpha blending (smoke, dust); sorted back to front within the emitter.
    #[default]
    Alpha,
    /// Added to the scene (fire, sparks, magic); order does not matter.
    Additive,
}

/// The space particles live in once emitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub enum ParticleSpace {
    /// Particles stay where they were emitted when the emitter moves
    /// (smoke, trails).
    #[default]
    World,
    /// Particles move with the emitter (a flame on a torch being carried).
    Local,
}

/// A burst of `count` particles `time` seconds into each emission cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ParticleBurst {
    #[reflect(@Range { min: 0.0, max: 600.0 }, @Unit("s"), @Step(0.01))]
    pub time: f32,
    #[reflect(@Range { min: 0.0, max: 100000.0 })]
    pub count: u32,
}

/// A value at a point `t` (0 birth .. 1 death) of a particle's life;
/// values between keys are interpolated linearly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct CurveKey {
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub t: f32,
    pub value: f32,
}

/// A colour (with alpha) at a point `t` (0 birth .. 1 death) of a
/// particle's life; colours between keys are interpolated linearly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct GradientKey {
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub t: f32,
    /// Linear colour; alpha is opacity (alpha blending) or brightness
    /// (additive).
    pub color: Color,
}

/// Emits camera-facing particles from the entity's position, simulated on
/// the CPU and drawn instanced in the transparent pass.
///
/// Particles are born at `rate` per second (plus `rate_over_distance` per
/// metre the emitter moves, for trails, plus `bursts`) while `playing`,
/// within `spawn_radius` of the emitter, moving at `speed` inside a cone
/// of `cone_angle` around `direction`. They fall with `gravity`, slow with
/// `drag`, spin, change size and colour over their life and die after
/// `lifetime`. With `looping` off the emitter emits for one `duration`
/// and, with `despawn_when_finished`, despawns once its last particle
/// dies: a one-shot burst for a hit is [`ParticleEmitter::burst`].
/// Clearing `playing` stops emission (live particles finish); setting it
/// again restarts a finished emitter. A `seed` gives the same particles
/// every run for the same frame times.
#[derive(Clone, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct ParticleEmitter {
    /// Emitting new particles.
    pub playing: bool,
    /// Start a new cycle (bursts again) after `duration`; otherwise stop
    /// emitting then.
    pub looping: bool,
    /// Length of one emission cycle.
    #[reflect(@Range { min: 0.01, max: 600.0 }, @Unit("s"), @Step(0.1))]
    pub duration: f32,
    /// Particles per second.
    #[reflect(@Range { min: 0.0, max: 100000.0 }, @Unit("1/s"), @Step(1.0))]
    pub rate: f32,
    /// Particles per metre the emitter moves (trails behind moving things).
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("1/m"), @Step(0.5))]
    pub rate_over_distance: f32,
    /// Bursts within each cycle.
    pub bursts: Vec<ParticleBurst>,
    /// Most live particles; no new ones are born beyond it.
    #[reflect(@Range { min: 1.0, max: 100000.0 })]
    pub max_particles: u32,
    /// Shortest and longest life (each particle picks one at random).
    #[reflect(@Range { min: 0.01, max: 120.0 }, @Unit("s"), @Step(0.05))]
    pub lifetime: Vec2,
    /// Slowest and fastest initial speed.
    #[reflect(@Range { min: 0.0, max: 500.0 }, @Unit("m/s"), @Step(0.1))]
    pub speed: Vec2,
    /// Emission direction in the entity's space (normalised when used).
    pub direction: Vec3,
    /// Half-angle of the cone around `direction` velocities are picked in
    /// (0 straight, π every direction).
    #[reflect(@Range { min: 0.0, max: 3.15 }, @Unit("rad"), @Step(0.01))]
    pub cone_angle: f32,
    /// Particles are born at random within this distance of the emitter.
    #[reflect(@Range { min: 0.0, max: 100.0 }, @Unit("m"), @Step(0.01))]
    pub spawn_radius: f32,
    /// Acceleration in world space (sparks: `(0, -9.81, 0)`; smoke rises
    /// with a small positive y).
    #[reflect(@Unit("m/s²"), @Step(0.1))]
    pub gravity: Vec3,
    /// Air resistance: velocity shrinks by `exp(-drag * dt)`.
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("1/s"), @Step(0.05))]
    pub drag: f32,
    /// Size (m, the quad's width) over life. Empty: 0.1 m.
    pub size: Vec<CurveKey>,
    /// Each particle's size is scaled by a random factor in
    /// `1 ± size_random`.
    #[reflect(@Range { min: 0.0, max: 1.0 }, @Step(0.01))]
    pub size_random: f32,
    /// Colour and alpha over life. Empty: white.
    pub color: Vec<GradientKey>,
    /// Brightness multiplier for the colour (above 1 glows and blooms).
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Step(0.1), @Logarithmic)]
    pub intensity: f32,
    /// Slowest and fastest spin (rad/s, either direction at random).
    #[reflect(@Range { min: 0.0, max: 50.0 }, @Unit("rad/s"), @Step(0.1))]
    pub spin: Vec2,
    /// Start each particle at a random rotation.
    pub random_rotation: bool,
    /// Image (RGBA, alpha is coverage); none draws a soft round dot.
    pub texture: AssetRef,
    /// Columns and rows of animation frames in `texture`, played over
    /// each particle's life; (1, 1) is a single image.
    pub atlas: UVec2,
    /// How particles are blended over the scene: `Alpha` (smoke, dust) or
    /// `Additive` (fire, sparks, magic).
    pub blend: ParticleBlend,
    /// Soft particles: fade out within this distance of opaque surfaces
    /// behind them (m; 0 off). Uses the depth prepass.
    #[reflect(@Range { min: 0.0, max: 10.0 }, @Unit("m"), @Step(0.01))]
    pub soft_distance: f32,
    /// Lit by the ambient light (and ambient volumes), the sun and local
    /// lights, unshadowed; otherwise the colour is emitted as is (fire).
    pub lit: bool,
    /// Where emitted particles live: `World` (they stay put when the
    /// emitter moves) or `Local` (they move with it).
    pub space: ParticleSpace,
    /// Random seed: equal seeds and frame times give equal particles.
    pub seed: u64,
    /// Despawn the entity when it has finished (not looping, emission over,
    /// no particles left).
    pub despawn_when_finished: bool,
}

impl Default for ParticleEmitter {
    fn default() -> Self {
        Self {
            playing: true,
            looping: true,
            duration: 5.0,
            rate: 20.0,
            rate_over_distance: 0.0,
            bursts: Vec::new(),
            max_particles: 1000,
            lifetime: Vec2::new(1.0, 2.0),
            speed: Vec2::new(0.5, 1.0),
            direction: Vec3::Y,
            cone_angle: 0.3,
            spawn_radius: 0.0,
            gravity: Vec3::ZERO,
            drag: 0.0,
            size: vec![
                CurveKey { t: 0.0, value: 0.1 },
                CurveKey { t: 1.0, value: 0.1 },
            ],
            size_random: 0.0,
            color: vec![
                GradientKey {
                    t: 0.0,
                    color: Color::WHITE,
                },
                GradientKey {
                    t: 1.0,
                    color: Color::WHITE.with_alpha(0.0),
                },
            ],
            intensity: 1.0,
            spin: Vec2::ZERO,
            random_rotation: false,
            texture: AssetRef::NONE,
            atlas: UVec2::ONE,
            blend: ParticleBlend::Alpha,
            soft_distance: 0.0,
            lit: false,
            space: ParticleSpace::World,
            seed: 1,
            despawn_when_finished: false,
        }
    }
}

impl ParticleEmitter {
    /// A one-shot burst of `count` particles that despawns its entity when
    /// the last one dies (hits, explosions). Adjust speed, colour and so on
    /// on the result.
    pub fn burst(count: u32) -> Self {
        Self {
            looping: false,
            duration: 0.01,
            rate: 0.0,
            bursts: vec![ParticleBurst { time: 0.0, count }],
            max_particles: count.max(1),
            cone_angle: std::f32::consts::PI,
            despawn_when_finished: true,
            ..Default::default()
        }
    }

    /// Size at life fraction `t`.
    pub fn size_at(&self, t: f32) -> f32 {
        eval_curve(&self.size, t)
    }

    /// Colour (with alpha) at life fraction `t`, without `intensity`.
    pub fn color_at(&self, t: f32) -> Vec4 {
        eval_gradient(&self.color, t)
    }
}

fn eval_curve(keys: &[CurveKey], t: f32) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return 0.1;
    };
    if t <= first.t {
        return first.value;
    }
    for w in keys.windows(2) {
        if t <= w[1].t {
            let span = (w[1].t - w[0].t).max(1e-6);
            return w[0].value + (w[1].value - w[0].value) * ((t - w[0].t) / span);
        }
    }
    last.value
}

fn eval_gradient(keys: &[GradientKey], t: f32) -> Vec4 {
    let v = |c: Color| Vec4::from_array(c.to_array());
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return Vec4::ONE;
    };
    if t <= first.t {
        return v(first.color);
    }
    for w in keys.windows(2) {
        if t <= w[1].t {
            let span = (w[1].t - w[0].t).max(1e-6);
            return v(w[0].color).lerp(v(w[1].color), (t - w[0].t) / span);
        }
    }
    v(last.color)
}

/// One live particle. Position and velocity are in world space, or in the
/// emitter's space with [`ParticleSpace::Local`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub lifetime: f32,
    /// Size multiplier (from `size_random`).
    pub size_scale: f32,
    pub rotation: f32,
    pub spin: f32,
}

/// A small, fast, seedable generator (PCG32).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rng {
    state: u64,
}

impl Rng {
    const MUL: u64 = 6364136223846793005;
    const INC: u64 = 1442695040888963407;

    fn new(seed: u64) -> Self {
        let mut r = Self { state: 0 };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }

    fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(Self::MUL).wrapping_add(Self::INC);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in 0..1.
    fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    fn range(&mut self, r: Vec2) -> f32 {
        r.x + (r.y - r.x) * self.f32()
    }
}

/// Runtime state of a [`ParticleEmitter`]: its live particles and emission
/// clock. Added by [`simulate_particles`]; never saved.
#[derive(Clone, Debug)]
pub struct ParticleState {
    particles: Vec<Particle>,
    rng: Rng,
    /// Seconds into the current emission cycle.
    time: f32,
    rate_carry: f32,
    distance_carry: f32,
    /// Not looping and the cycle is over.
    emission_over: bool,
    was_playing: bool,
    last_position: Option<Vec3>,
    /// World bounds of the particles (padded by their size).
    bounds: Aabb,
    /// The emitter's transform at the last step (local-space particles).
    transform: Affine3A,
}

impl ParticleState {
    pub fn new(seed: u64) -> Self {
        Self {
            particles: Vec::new(),
            rng: Rng::new(seed),
            time: 0.0,
            rate_carry: 0.0,
            distance_carry: 0.0,
            emission_over: false,
            was_playing: true,
            last_position: None,
            bounds: Aabb::EMPTY,
            transform: Affine3A::IDENTITY,
        }
    }

    /// Live particles.
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Not looping, the emission is over and every particle has died.
    pub fn is_finished(&self) -> bool {
        self.emission_over && self.particles.is_empty()
    }

    /// World bounds of the live particles, padded by their size.
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    /// The emitter's transform at the last step.
    pub fn transform(&self) -> &Affine3A {
        &self.transform
    }

    /// Clears every particle and starts the emission cycle again with
    /// `seed`.
    pub fn restart(&mut self, seed: u64) {
        *self = Self {
            transform: self.transform,
            ..Self::new(seed)
        };
    }

    /// Advances the emitter by `dt` seconds with the emitter's transform
    /// `transform`.
    pub fn step(&mut self, e: &ParticleEmitter, transform: &Affine3A, dt: f32) {
        let dt = dt.max(0.0);
        let local = e.space == ParticleSpace::Local;
        if e.playing && !self.was_playing && self.emission_over {
            self.time = 0.0;
            self.emission_over = false;
        }
        self.was_playing = e.playing;
        let now = Vec3::from(transform.translation);
        let before = self.last_position.unwrap_or(now);
        self.last_position = Some(now);
        self.transform = *transform;

        // Age, move and retire the live particles.
        let gravity = if local {
            transform.matrix3.inverse() * e.gravity
        } else {
            e.gravity
        };
        let damping = (-e.drag.max(0.0) * dt).exp();
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.age += dt;
            if p.age >= p.lifetime {
                self.particles.swap_remove(i);
                continue;
            }
            p.velocity = (p.velocity + gravity * dt) * damping;
            p.position += p.velocity * dt;
            p.rotation += p.spin * dt;
            i += 1;
        }

        // Emission: rate and distance spread over the frame (born earlier
        // in it, along the emitter's path), bursts at their time.
        if e.playing && !self.emission_over {
            self.rate_carry += e.rate.max(0.0) * dt;
            self.distance_carry += e.rate_over_distance.max(0.0) * (now - before).length();
            let spread = (self.rate_carry.floor() + self.distance_carry.floor()) as u32;
            self.rate_carry = self.rate_carry.fract();
            self.distance_carry = self.distance_carry.fract();
            for k in 0..spread {
                let f = (k as f32 + 0.5) / spread as f32;
                self.spawn(e, transform, before.lerp(now, f), (1.0 - f) * dt, gravity);
            }
            // Bursts timed in [start, end) of this frame, cycle by cycle.
            let duration = e.duration.max(0.01);
            let (mut start, mut end) = (self.time, self.time + dt);
            for _ in 0..1000 {
                let stop = end.min(duration);
                for b in &e.bursts {
                    if b.time >= start && b.time < stop {
                        for _ in 0..b.count {
                            self.spawn(e, transform, now, 0.0, gravity);
                        }
                    }
                }
                if end < duration {
                    break;
                }
                if !e.looping {
                    self.emission_over = true;
                    end = duration;
                    break;
                }
                start = 0.0;
                end -= duration;
            }
            self.time = end;
        }

        // Bounds for culling and sorting.
        let pad = e.size.iter().map(|k| k.value).fold(0.0f32, f32::max)
            * (1.0 + e.size_random.max(0.0))
            * std::f32::consts::FRAC_1_SQRT_2;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for p in &self.particles {
            let w = if local {
                transform.transform_point3(p.position)
            } else {
                p.position
            };
            min = min.min(w);
            max = max.max(w);
        }
        self.bounds = if self.particles.is_empty() {
            Aabb::EMPTY
        } else {
            Aabb::new(min - Vec3::splat(pad), max + Vec3::splat(pad))
        };
    }

    /// Spawns one particle at world position `at`, `age` seconds old.
    fn spawn(
        &mut self,
        e: &ParticleEmitter,
        transform: &Affine3A,
        at: Vec3,
        age: f32,
        gravity: Vec3,
    ) {
        let r = &mut self.rng;
        // Draw every random number even when over the cap, so the stream
        // does not depend on how many particles are alive.
        let offset = random_in_ball(r) * e.spawn_radius.max(0.0);
        let dir = random_in_cone(r, e.cone_angle);
        let speed = r.range(e.speed);
        let lifetime = r.range(e.lifetime).max(0.01);
        let size_scale = 1.0 + e.size_random.clamp(0.0, 1.0) * (2.0 * r.f32() - 1.0);
        let rotation = if e.random_rotation {
            r.f32() * std::f32::consts::TAU
        } else {
            0.0
        };
        let spin = r.range(e.spin) * if r.f32() < 0.5 { -1.0 } else { 1.0 };
        if self.particles.len() >= e.max_particles.clamp(1, MAX_PARTICLES_PER_EMITTER) as usize
            || age >= lifetime
        {
            return;
        }
        let axis = e.direction.try_normalize().unwrap_or(Vec3::Y);
        let local_dir = Quat::from_rotation_arc(Vec3::Y, axis) * dir;
        let (position, velocity) = match e.space {
            ParticleSpace::World => (
                at + offset,
                (transform.matrix3 * local_dir).normalize_or_zero() * speed,
            ),
            ParticleSpace::Local => {
                let inv = transform.inverse();
                (inv.transform_point3(at) + offset, local_dir * speed)
            }
        };
        // Catch up on the part of the frame it was already alive for.
        let velocity = velocity + gravity * age;
        self.particles.push(Particle {
            position: position + velocity * age,
            velocity,
            age,
            lifetime,
            size_scale,
            rotation: rotation + spin * age,
            spin,
        });
    }
}

/// A uniform random point in the unit ball.
fn random_in_ball(r: &mut Rng) -> Vec3 {
    let d = random_in_cone(r, std::f32::consts::PI);
    d * r.f32().cbrt()
}

/// A uniform random unit vector within `angle` of +Y.
fn random_in_cone(r: &mut Rng, angle: f32) -> Vec3 {
    let cos_max = angle.clamp(0.0, std::f32::consts::PI).cos();
    let cos = 1.0 - r.f32() * (1.0 - cos_max);
    let sin = (1.0 - cos * cos).max(0.0).sqrt();
    let phi = r.f32() * std::f32::consts::TAU;
    Vec3::new(sin * phi.cos(), cos, sin * phi.sin())
}

/// Advances every [`ParticleEmitter`] by `Time::delta`: adds its
/// [`ParticleState`] on its first frame and despawns finished one-shot
/// emitters (`despawn_when_finished`). An emitter hidden by `Visible(false)`
/// (its own or an ancestor's) is not simulated and loses its particles;
/// shown again, it starts afresh. Runs in `PostUpdate` after transform
/// propagation (box_engine's `app_with_world` adds it).
pub fn simulate_particles(world: &mut World) {
    profiling::scope!("simulate_particles");
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    let world: &World = world;
    let hidden = box_core::visibility::hidden_entities(world);
    type Emitters<'a> = (
        Entity,
        &'a ParticleEmitter,
        &'a GlobalTransform,
        Option<&'a mut ParticleState>,
    );
    for (entity, emitter, gt, state) in world.query::<Emitters>().iter() {
        if hidden.contains(&entity) {
            if state.is_some() {
                world.defer().remove::<ParticleState>(entity);
            }
            continue;
        }
        let finished = match state {
            Some(state) => {
                state.step(emitter, &gt.0, dt);
                state.is_finished()
            }
            None => {
                // First frame: a fresh state, stepped by this frame.
                let mut state = ParticleState::new(emitter.seed);
                state.step(emitter, &gt.0, dt);
                let finished = state.is_finished();
                world.defer().insert(entity, (state,));
                finished
            }
        };
        if finished && emitter.despawn_when_finished {
            world.defer().despawn(entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(e: &ParticleEmitter, frames: usize, at: impl Fn(usize) -> Vec3) -> ParticleState {
        let mut s = ParticleState::new(e.seed);
        for f in 0..frames {
            s.step(e, &Affine3A::from_translation(at(f)), 1.0 / 60.0);
        }
        s
    }

    #[test]
    fn equal_seeds_give_equal_particles_and_other_seeds_differ() {
        let e = ParticleEmitter {
            rate: 200.0,
            spawn_radius: 0.3,
            cone_angle: 0.8,
            size_random: 0.5,
            random_rotation: true,
            spin: Vec2::new(0.5, 2.0),
            gravity: Vec3::new(0.0, -2.0, 0.0),
            drag: 0.5,
            seed: 7,
            ..Default::default()
        };
        let a = run(&e, 120, |_| Vec3::ZERO);
        let b = run(&e, 120, |_| Vec3::ZERO);
        assert!(a.len() > 100);
        assert_eq!(a.particles(), b.particles());
        let c = run(
            &ParticleEmitter {
                seed: 8,
                ..e.clone()
            },
            120,
            |_| Vec3::ZERO,
        );
        assert_ne!(a.particles(), c.particles());
    }

    #[test]
    fn rate_lifetime_and_the_cap_bound_the_count() {
        // 100/s for 1 s lives: about 100 alive in steady state.
        let e = ParticleEmitter {
            rate: 100.0,
            lifetime: Vec2::splat(1.0),
            ..Default::default()
        };
        let s = run(&e, 180, |_| Vec3::ZERO);
        assert!((99..=101).contains(&s.len()), "{}", s.len());
        assert!(s.particles().iter().all(|p| p.age < 1.0));
        let capped = run(
            &ParticleEmitter {
                max_particles: 30,
                ..e.clone()
            },
            180,
            |_| Vec3::ZERO,
        );
        assert_eq!(capped.len(), 30);
        // Stopped: no new particles, the live ones die out.
        let mut s = s;
        let stopped = ParticleEmitter {
            playing: false,
            ..e.clone()
        };
        for _ in 0..61 {
            s.step(&stopped, &Affine3A::IDENTITY, 1.0 / 60.0);
        }
        assert!(s.is_empty());
        assert!(!s.is_finished(), "a looping emitter never finishes");
    }

    #[test]
    fn bursts_fire_once_per_cycle_and_one_shots_finish() {
        let e = ParticleEmitter {
            rate: 0.0,
            duration: 1.0,
            bursts: vec![
                ParticleBurst {
                    time: 0.0,
                    count: 10,
                },
                ParticleBurst {
                    time: 0.5,
                    count: 5,
                },
            ],
            lifetime: Vec2::splat(10.0),
            ..Default::default()
        };
        assert_eq!(run(&e, 1, |_| Vec3::ZERO).len(), 10);
        assert_eq!(run(&e, 31, |_| Vec3::ZERO).len(), 15);
        // Two cycles and a bit: three bursts at 0 (0, 1, 2 s), two at 0.5.
        assert_eq!(run(&e, 125, |_| Vec3::ZERO).len(), 40);

        let hit = ParticleEmitter {
            lifetime: Vec2::splat(0.5),
            ..ParticleEmitter::burst(25)
        };
        let mut s = ParticleState::new(hit.seed);
        s.step(&hit, &Affine3A::IDENTITY, 1.0 / 60.0);
        assert_eq!(s.len(), 25);
        assert!(!s.is_finished());
        for _ in 0..40 {
            s.step(&hit, &Affine3A::IDENTITY, 1.0 / 60.0);
        }
        assert!(s.is_finished());
        // Playing again restarts a finished emitter.
        let off = ParticleEmitter {
            playing: false,
            ..hit.clone()
        };
        s.step(&off, &Affine3A::IDENTITY, 1.0 / 60.0);
        s.step(&hit, &Affine3A::IDENTITY, 1.0 / 60.0);
        assert_eq!(s.len(), 25);
    }

    #[test]
    fn trails_spread_along_the_path_and_local_particles_follow() {
        // 10 particles per metre, moving 0.5 m per frame along +X.
        let e = ParticleEmitter {
            rate: 0.0,
            rate_over_distance: 10.0,
            speed: Vec2::ZERO,
            lifetime: Vec2::splat(5.0),
            ..Default::default()
        };
        let s = run(&e, 11, |f| Vec3::new(f as f32 * 0.5, 0.0, 0.0));
        assert_eq!(s.len(), 50);
        let mut xs: Vec<f32> = s.particles().iter().map(|p| p.position.x).collect();
        xs.sort_by(f32::total_cmp);
        for w in xs.windows(2) {
            assert!((w[1] - w[0] - 0.1).abs() < 1e-3, "evenly spaced: {xs:?}");
        }
        // Local space: the particles move with the emitter.
        let local = ParticleEmitter {
            rate: 60.0,
            speed: Vec2::ZERO,
            space: ParticleSpace::Local,
            ..Default::default()
        };
        let s = run(&local, 30, |f| Vec3::new(f as f32, 0.0, 0.0));
        assert!(s.bounds().min.x > 28.0, "{:?}", s.bounds());
    }

    #[test]
    fn gravity_and_drag_shape_the_motion() {
        let e = ParticleEmitter {
            rate: 0.0,
            bursts: vec![ParticleBurst {
                time: 0.0,
                count: 1,
            }],
            speed: Vec2::splat(0.0),
            gravity: Vec3::new(0.0, -10.0, 0.0),
            lifetime: Vec2::splat(10.0),
            ..Default::default()
        };
        let s = run(&e, 61, |_| Vec3::ZERO);
        let p = s.particles()[0];
        // About 1 s of free fall (semi-implicit Euler): v = -10, y ≈ -5.
        assert!((p.velocity.y + 10.0).abs() < 0.01, "{p:?}");
        assert!((p.position.y + 5.0).abs() < 0.2, "{p:?}");
        let dragged = run(
            &ParticleEmitter {
                drag: 2.0,
                ..e.clone()
            },
            61,
            |_| Vec3::ZERO,
        );
        // Terminal speed g / drag = 5 m/s.
        assert!(dragged.particles()[0].velocity.y > -5.0);
    }

    #[test]
    fn curves_and_gradients_interpolate_and_clamp() {
        let e = ParticleEmitter {
            size: vec![
                CurveKey { t: 0.0, value: 1.0 },
                CurveKey { t: 0.5, value: 3.0 },
                CurveKey { t: 1.0, value: 0.0 },
            ],
            ..Default::default()
        };
        assert_eq!(e.size_at(-1.0), 1.0);
        assert_eq!(e.size_at(0.25), 2.0);
        assert_eq!(e.size_at(0.75), 1.5);
        assert_eq!(e.size_at(2.0), 0.0);
        assert_eq!(e.color_at(0.5), Vec4::new(1.0, 1.0, 1.0, 0.5));
        assert_eq!(ParticleEmitter { size: vec![], ..e }.size_at(0.3), 0.1);
    }

    #[test]
    fn the_system_adds_state_and_despawns_finished_one_shots() {
        let mut world = World::new();
        world.insert_resource(Time {
            delta: 1.0 / 60.0,
            ..Default::default()
        });
        let hit = ParticleEmitter {
            lifetime: Vec2::splat(0.1),
            ..ParticleEmitter::burst(5)
        };
        let e = world.spawn((hit, GlobalTransform::default()));
        simulate_particles(&mut world);
        world.apply_deferred();
        assert_eq!(world.get::<ParticleState>(e).unwrap().len(), 5);
        for _ in 0..10 {
            simulate_particles(&mut world);
            world.apply_deferred();
        }
        assert!(!world.contains(e), "despawned when finished");
    }
}
