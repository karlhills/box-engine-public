//! A procedural low-poly humanoid with a 14-joint skeleton and four
//! authored clips (`Idle`, `Walk`, `Run`, `Jump`), written as a skinned
//! `.glb`. It is the engine's own animation test character: no downloaded
//! or third-party assets, and the glTF importer is tested on a real file.
//!
//! The character stands on the origin facing −Z (the engine's forward),
//! 1.8 m tall, arms down. Limbs are box tubes whose joint-side rings are
//! weighted half to the parent bone, so elbows and knees bend smoothly.
//! Material slot 0 (`Body`) is vertex-coloured clothing; slot 1 (`Skin`)
//! is the head and hands. Idle's chest uses cubic-spline keys and its head
//! glances use step keys; everything else is linear, so all three glTF
//! interpolation modes are exercised. [`combat_character_glb`] adds
//! `Attack` (a one-armed slash) and `Hit` (a flinch) for action games; the
//! plain [`character_glb`] file is unchanged by them.

use box_core::pmath;
use std::f32::consts::{PI, TAU};

use box_core::animation::{AnimationClip, Channel, Interpolation, Keyframes};
use box_core::color::Color;
use box_core::material::StandardMaterial;
use box_core::mesh::{MaterialGroup, MeshData};
use glam::{Quat, Vec3};

use crate::glb::{Glb, GlbError, GlbJoint, GlbMaterial, write_skinned_glb};

/// Joint names, in skin order (glTF node names).
pub const JOINTS: [&str; 14] = [
    "Hips",
    "Spine",
    "Chest",
    "Head",
    "UpperArm.L",
    "LowerArm.L",
    "UpperArm.R",
    "LowerArm.R",
    "UpperLeg.L",
    "LowerLeg.L",
    "Foot.L",
    "UpperLeg.R",
    "LowerLeg.R",
    "Foot.R",
];

/// Clip names and durations (seconds).
pub const CLIPS: [(&str, f32); 4] = [("Idle", 2.0), ("Walk", 1.0), ("Run", 0.6), ("Jump", 1.0)];

const HIPS_Y: f32 = 0.95;

/// (parent, local bind translation) per joint.
fn skeleton() -> [(Option<usize>, Vec3); 14] {
    [
        (None, Vec3::new(0.0, HIPS_Y, 0.0)),
        (Some(0), Vec3::new(0.0, 0.10, 0.0)),
        (Some(1), Vec3::new(0.0, 0.20, 0.0)),
        (Some(2), Vec3::new(0.0, 0.25, 0.0)),
        (Some(2), Vec3::new(0.25, 0.20, 0.0)),
        (Some(4), Vec3::new(0.0, -0.28, 0.0)),
        (Some(2), Vec3::new(-0.25, 0.20, 0.0)),
        (Some(6), Vec3::new(0.0, -0.28, 0.0)),
        (Some(0), Vec3::new(0.10, -0.05, 0.0)),
        (Some(8), Vec3::new(0.0, -0.40, 0.0)),
        (Some(9), Vec3::new(0.0, -0.42, 0.0)),
        (Some(0), Vec3::new(-0.10, -0.05, 0.0)),
        (Some(11), Vec3::new(0.0, -0.40, 0.0)),
        (Some(12), Vec3::new(0.0, -0.42, 0.0)),
    ]
}

/// The joint's bind position in model space.
fn bind_position(joint: usize) -> Vec3 {
    let s = skeleton();
    let mut p = Vec3::ZERO;
    let mut j = Some(joint);
    while let Some(i) = j {
        p += s[i].1;
        j = s[i].0;
    }
    p
}

/// Skin weights of a ring: the bone, blended with `partner` by its weight.
#[derive(Clone, Copy)]
struct Weights {
    bone: u16,
    partner: Option<(u16, f32)>,
}

impl Weights {
    fn rigid(bone: usize) -> Self {
        Self {
            bone: bone as u16,
            partner: None,
        }
    }

    fn blend(bone: usize, partner: usize, w: f32) -> Self {
        Self {
            bone: bone as u16,
            partner: Some((partner as u16, w)),
        }
    }

    fn joints(&self) -> ([u16; 4], [f32; 4]) {
        match self.partner {
            Some((p, w)) => ([self.bone, p, 0, 0], [1.0 - w, w, 0.0, 0.0]),
            None => ([self.bone, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]),
        }
    }
}

#[derive(Default)]
struct Builder {
    mesh: MeshData,
    /// Indices per material slot.
    groups: [Vec<u32>; 2],
}

impl Builder {
    fn vertex(&mut self, p: Vec3, n: Vec3, color: [f32; 4], w: Weights) -> u32 {
        let (j, wt) = w.joints();
        self.mesh.positions.push(p);
        self.mesh.normals.push(n);
        self.mesh.colors.push(color);
        self.mesh.joints.push(j);
        self.mesh.weights.push(wt);
        self.mesh.positions.len() as u32 - 1
    }

    /// A quad facing `n` (corners in order around it, either direction).
    fn quad(&mut self, slot: usize, corners: [(Vec3, Weights); 4], n: Vec3, color: [f32; 4]) {
        let [a, b, c, d] = corners.map(|(p, w)| self.vertex(p, n, color, w));
        let (pa, pb, pc) = (corners[0].0, corners[1].0, corners[2].0);
        let idx = if (pb - pa).cross(pc - pa).dot(n) >= 0.0 {
            [a, b, c, a, c, d]
        } else {
            [a, c, b, a, d, c]
        };
        self.groups[slot].extend(idx);
    }

    /// A vertical box tube from `top` down to `bottom` (y), centred on
    /// `centre` (x, z), with a middle ring: rings take `top_w`, `bone`,
    /// `bottom_w` weights.
    #[allow(clippy::too_many_arguments)]
    fn tube(
        &mut self,
        slot: usize,
        centre: (f32, f32),
        (top, bottom): (f32, f32),
        (hx, hz): (f32, f32),
        color: [f32; 4],
        bone: usize,
        top_w: Weights,
        bottom_w: Weights,
    ) {
        let rings = [
            (top, top_w),
            ((top + bottom) * 0.5, Weights::rigid(bone)),
            (bottom, bottom_w),
        ];
        let (cx, cz) = centre;
        let at = |sx: f32, sz: f32, y: f32| Vec3::new(cx + sx * hx, y, cz + sz * hz);
        // Sides: normal and the two corner signs (x, z) of its edge.
        let sides = [
            (Vec3::X, [(1.0, -1.0), (1.0, 1.0)]),
            (Vec3::NEG_X, [(-1.0, 1.0), (-1.0, -1.0)]),
            (Vec3::Z, [(1.0, 1.0), (-1.0, 1.0)]),
            (Vec3::NEG_Z, [(-1.0, -1.0), (1.0, -1.0)]),
        ];
        for (n, [(ax, az), (bx, bz)]) in sides {
            for k in 0..2 {
                let (y0, w0) = rings[k];
                let (y1, w1) = rings[k + 1];
                self.quad(
                    slot,
                    [
                        (at(ax, az, y0), w0),
                        (at(bx, bz, y0), w0),
                        (at(bx, bz, y1), w1),
                        (at(ax, az, y1), w1),
                    ],
                    n,
                    color,
                );
            }
        }
        for (y, w, n) in [(top, top_w, Vec3::Y), (bottom, bottom_w, Vec3::NEG_Y)] {
            self.quad(
                slot,
                [
                    (at(-1.0, -1.0, y), w),
                    (at(1.0, -1.0, y), w),
                    (at(1.0, 1.0, y), w),
                    (at(-1.0, 1.0, y), w),
                ],
                n,
                color,
            );
        }
    }

    /// An axis-aligned box rigidly weighted to `bone`.
    fn cuboid(&mut self, slot: usize, min: Vec3, max: Vec3, color: [f32; 4], bone: usize) {
        let w = Weights::rigid(bone);
        let c = (min + max) * 0.5;
        let h = (max - min) * 0.5;
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let mut n = Vec3::ZERO;
                n[axis] = sign;
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let corner = |su: f32, sv: f32| {
                    let mut p = c + n * h;
                    p[u] += su * h[u];
                    p[v] += sv * h[v];
                    (p, w)
                };
                self.quad(
                    slot,
                    [
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ],
                    n,
                    color,
                );
            }
        }
    }

    fn finish(mut self) -> MeshData {
        for (slot, idx) in self.groups.iter().enumerate() {
            self.mesh.groups.push(MaterialGroup {
                first_index: self.mesh.indices.len() as u32,
                index_count: idx.len() as u32,
                material_slot: slot as u32,
            });
            self.mesh.indices.extend(idx);
        }
        self.mesh
    }
}

/// The character's mesh in bind pose (joints and weights set).
pub fn mesh() -> MeshData {
    const BODY: usize = 0;
    const SKIN: usize = 1;
    let shirt = Color::srgb(0.22, 0.45, 0.78).to_array();
    let belt = Color::srgb(0.30, 0.22, 0.15).to_array();
    let pants = Color::srgb(0.24, 0.26, 0.32).to_array();
    let shoes = Color::srgb(0.12, 0.10, 0.09).to_array();
    let white = [1.0; 4];
    let dark = Color::srgb(0.08, 0.08, 0.1).to_array();
    let mut b = Builder::default();
    let r = Weights::rigid;
    // Torso: pelvis (Hips), abdomen (Spine), chest (Chest).
    b.tube(
        BODY,
        (0.0, 0.0),
        (1.06, 0.88),
        (0.16, 0.10),
        belt,
        0,
        Weights::blend(0, 1, 0.5),
        r(0),
    );
    b.tube(
        BODY,
        (0.0, 0.0),
        (1.25, 1.06),
        (0.15, 0.095),
        shirt,
        1,
        Weights::blend(1, 2, 0.5),
        Weights::blend(1, 0, 0.5),
    );
    b.tube(
        BODY,
        (0.0, 0.0),
        (1.49, 1.25),
        (0.19, 0.11),
        shirt,
        2,
        r(2),
        Weights::blend(2, 1, 0.5),
    );
    // Neck and head (Head), with a nose and eyes on the front (−Z).
    b.tube(
        SKIN,
        (0.0, 0.0),
        (1.57, 1.48),
        (0.05, 0.05),
        white,
        3,
        r(3),
        Weights::blend(3, 2, 0.5),
    );
    b.cuboid(
        SKIN,
        Vec3::new(-0.11, 1.56, -0.12),
        Vec3::new(0.11, 1.80, 0.11),
        white,
        3,
    );
    b.cuboid(
        SKIN,
        Vec3::new(-0.02, 1.64, -0.16),
        Vec3::new(0.02, 1.69, -0.12),
        white,
        3,
    );
    for x in [-0.05, 0.05] {
        b.cuboid(
            BODY,
            Vec3::new(x - 0.018, 1.71, -0.125),
            Vec3::new(x + 0.018, 1.735, -0.118),
            dark,
            3,
        );
    }
    // Arms: upper (sleeve), lower, hand.
    for (upper, lower) in [(4, 5), (6, 7)] {
        let s = bind_position(upper);
        let e = bind_position(lower);
        let c = (s.x, s.z);
        b.tube(
            BODY,
            c,
            (s.y + 0.03, e.y),
            (0.055, 0.06),
            shirt,
            upper,
            Weights::blend(upper, 2, 0.3),
            Weights::blend(upper, lower, 0.5),
        );
        b.tube(
            SKIN,
            c,
            (e.y, e.y - 0.25),
            (0.045, 0.05),
            white,
            lower,
            Weights::blend(lower, upper, 0.5),
            r(lower),
        );
        b.cuboid(
            SKIN,
            Vec3::new(c.0 - 0.04, e.y - 0.36, c.1 - 0.05),
            Vec3::new(c.0 + 0.04, e.y - 0.25, c.1 + 0.05),
            white,
            lower,
        );
    }
    // Legs: thigh, shin, shoe.
    for (upper, lower, foot) in [(8, 9, 10), (11, 12, 13)] {
        let h = bind_position(upper);
        let k = bind_position(lower);
        let a = bind_position(foot);
        let c = (h.x, h.z);
        b.tube(
            BODY,
            c,
            (h.y + 0.04, k.y),
            (0.075, 0.08),
            pants,
            upper,
            Weights::blend(upper, 0, 0.5),
            Weights::blend(upper, lower, 0.5),
        );
        b.tube(
            BODY,
            c,
            (k.y, a.y + 0.02),
            (0.06, 0.065),
            pants,
            lower,
            Weights::blend(lower, upper, 0.5),
            Weights::blend(lower, foot, 0.3),
        );
        b.cuboid(
            BODY,
            Vec3::new(c.0 - 0.06, 0.0, c.1 - 0.17),
            Vec3::new(c.0 + 0.06, a.y + 0.03, c.1 + 0.06),
            shoes,
            foot,
        );
    }
    b.finish()
}

fn rot_x(a: f32) -> Quat {
    Quat::from_rotation_x(a)
}

/// A linear channel sampling `f` at `keys + 1` evenly spaced times over
/// `duration` (the last equals the first for cycles).
fn sampled<T>(
    target: &str,
    duration: f32,
    keys: usize,
    f: impl Fn(f32) -> T,
    wrap: impl Fn(Vec<T>) -> Keyframes,
) -> Channel {
    let times: Vec<f32> = (0..=keys)
        .map(|i| duration * i as f32 / keys as f32)
        .collect();
    let values = times.iter().map(|&t| f(t / duration)).collect();
    Channel {
        target: target.into(),
        interpolation: Interpolation::Linear,
        times,
        values: wrap(values),
    }
}

fn rotation(target: &str, duration: f32, keys: usize, f: impl Fn(f32) -> Quat) -> Channel {
    sampled(target, duration, keys, f, Keyframes::Rotation)
}

fn hips(duration: f32, keys: usize, y: impl Fn(f32) -> f32) -> Channel {
    sampled(
        "Hips",
        duration,
        keys,
        |u| Vec3::new(0.0, y(u), 0.0),
        Keyframes::Translation,
    )
}

/// A walking or running cycle (phase u in 0..1).
fn gait(
    duration: f32,
    legs: f32,
    knees: f32,
    arms: f32,
    elbows: f32,
    lean: f32,
    bob: f32,
) -> AnimationClip {
    let k = 16;
    let phase = |u: f32| TAU * u;
    let knee = move |p: f32| -(0.1 + knees * pmath::cos(p).max(0.0));
    AnimationClip::new(vec![
        hips(duration, k, move |u| {
            HIPS_Y - bob + bob * pmath::cos(2.0 * phase(u))
        }),
        rotation("Hips", duration, k, move |u| {
            Quat::from_rotation_y(0.08 * pmath::sin(phase(u)))
        }),
        rotation("Spine", duration, k, move |_| rot_x(-lean)),
        rotation("Chest", duration, k, move |u| {
            Quat::from_rotation_y(-0.12 * pmath::sin(phase(u)))
        }),
        rotation("UpperLeg.L", duration, k, move |u| {
            rot_x(legs * pmath::sin(phase(u)))
        }),
        rotation("UpperLeg.R", duration, k, move |u| {
            rot_x(-legs * pmath::sin(phase(u)))
        }),
        rotation("LowerLeg.L", duration, k, move |u| rot_x(knee(phase(u)))),
        rotation("LowerLeg.R", duration, k, move |u| {
            rot_x(knee(phase(u) + PI))
        }),
        rotation("Foot.L", duration, k, move |u| {
            rot_x(0.25 * pmath::sin(phase(u) + 0.5))
        }),
        rotation("Foot.R", duration, k, move |u| {
            rot_x(0.25 * pmath::sin(phase(u) + PI + 0.5))
        }),
        rotation("UpperArm.L", duration, k, move |u| {
            rot_x(-arms * pmath::sin(phase(u))) * Quat::from_rotation_z(0.06)
        }),
        rotation("UpperArm.R", duration, k, move |u| {
            rot_x(arms * pmath::sin(phase(u))) * Quat::from_rotation_z(-0.06)
        }),
        rotation("LowerArm.L", duration, k, move |u| {
            rot_x(elbows + 0.15 * pmath::sin(phase(u)))
        }),
        rotation("LowerArm.R", duration, k, move |u| {
            rot_x(elbows - 0.15 * pmath::sin(phase(u)))
        }),
    ])
}

fn idle() -> AnimationClip {
    let d = CLIPS[0].1;
    let breathe = |a: f32| rot_x(a);
    let zero = Quat::from_xyzw(0.0, 0.0, 0.0, 0.0);
    AnimationClip::new(vec![
        hips(d, 8, |u| HIPS_Y - 0.006 + 0.006 * pmath::cos(TAU * u)),
        // Cubic spline, zero tangents: a smooth in-and-out breath.
        Channel {
            target: "Chest".into(),
            interpolation: Interpolation::CubicSpline,
            times: vec![0.0, d * 0.5, d],
            values: Keyframes::Rotation(vec![
                zero,
                breathe(0.03),
                zero,
                zero,
                breathe(-0.03),
                zero,
                zero,
                breathe(0.03),
                zero,
            ]),
        },
        // Step keys: quick glances left and right.
        Channel {
            target: "Head".into(),
            interpolation: Interpolation::Step,
            times: vec![0.0, 0.7, 1.3, 1.8],
            values: Keyframes::Rotation(vec![
                Quat::IDENTITY,
                Quat::from_rotation_y(0.35),
                Quat::from_rotation_y(-0.25),
                Quat::IDENTITY,
            ]),
        },
        rotation("UpperArm.L", d, 8, |u| {
            Quat::from_rotation_z(0.08 + 0.02 * pmath::sin(TAU * u))
        }),
        rotation("UpperArm.R", d, 8, |u| {
            Quat::from_rotation_z(-0.08 - 0.02 * pmath::sin(TAU * u))
        }),
        rotation("LowerArm.L", d, 1, |_| rot_x(0.12)),
        rotation("LowerArm.R", d, 1, |_| rot_x(0.12)),
    ])
}

fn jump() -> AnimationClip {
    let times = vec![0.0, 0.15, 0.3, 0.5, 0.7, 0.85, 1.0];
    let channel = |target: &str, values: Keyframes| Channel {
        target: target.into(),
        interpolation: Interpolation::Linear,
        times: times.clone(),
        values,
    };
    let rx = |a: [f32; 7]| Keyframes::Rotation(a.map(rot_x).to_vec());
    let thigh = [0.0, 0.8, -0.1, 0.9, 0.5, 0.8, 0.0];
    let shin = [0.0, -1.4, -0.05, -1.5, -0.6, -1.4, 0.0];
    let foot = [0.0, 0.5, -0.4, 0.4, 0.1, 0.5, 0.0];
    let arms = [0.0, -0.7, 2.7, 2.3, 1.2, 0.3, 0.0];
    let elbows = [0.1, 0.3, 0.2, 0.6, 0.4, 0.3, 0.1];
    AnimationClip::new(vec![
        channel(
            "Hips",
            Keyframes::Translation(
                [0.0, -0.17, 0.05, 0.0, 0.0, -0.15, 0.0]
                    .map(|y| Vec3::new(0.0, HIPS_Y + y, 0.0))
                    .to_vec(),
            ),
        ),
        channel("Spine", rx([0.0, -0.35, 0.1, -0.1, 0.0, -0.3, 0.0])),
        channel("UpperLeg.L", rx(thigh)),
        channel("UpperLeg.R", rx(thigh)),
        channel("LowerLeg.L", rx(shin)),
        channel("LowerLeg.R", rx(shin)),
        channel("Foot.L", rx(foot)),
        channel("Foot.R", rx(foot)),
        channel("UpperArm.L", rx(arms)),
        channel("UpperArm.R", rx(arms)),
        channel("LowerArm.L", rx(elbows)),
        channel("LowerArm.R", rx(elbows)),
    ])
}

/// The four clips, in [`CLIPS`] order.
pub fn clips() -> Vec<(&'static str, AnimationClip)> {
    vec![
        ("Idle", idle()),
        ("Walk", gait(CLIPS[1].1, 0.5, 0.7, 0.4, 0.25, 0.04, 0.025)),
        ("Run", gait(CLIPS[2].1, 0.85, 1.3, 0.8, 1.3, 0.2, 0.05)),
        ("Jump", jump()),
    ]
}

/// Combat clip names and durations (seconds), added by
/// [`combat_character_glb`].
pub const COMBAT_CLIPS: [(&str, f32); 2] = [("Attack", 0.45), ("Hit", 0.4)];

/// A linear clip of rotations about X (and Y for `twist`) at shared key
/// times: `(joint, angles)` per channel.
fn keyed(times: &[f32], x: &[(&str, &[f32])], twist: &[(&str, &[f32])]) -> AnimationClip {
    let channel = |target: &str, values: Vec<Quat>| Channel {
        target: target.into(),
        interpolation: Interpolation::Linear,
        times: times.to_vec(),
        values: Keyframes::Rotation(values),
    };
    let mut channels: Vec<Channel> = x
        .iter()
        .map(|(j, a)| channel(j, a.iter().map(|&a| rot_x(a)).collect()))
        .collect();
    channels.extend(
        twist
            .iter()
            .map(|(j, a)| channel(j, a.iter().map(|&a| Quat::from_rotation_y(a)).collect())),
    );
    AnimationClip::new(channels)
}

/// A one-armed slash: wind up to the right, sweep across to the left.
fn attack() -> AnimationClip {
    let d = COMBAT_CLIPS[0].1;
    let times = [0.0, d * 0.25, d * 0.55, d];
    keyed(
        &times,
        &[
            ("Spine", &[0.0, -0.08, 0.18, 0.0]),
            ("UpperArm.R", &[0.0, 1.9, 1.2, 0.0]),
            ("LowerArm.R", &[0.12, 0.9, 0.15, 0.12]),
            ("UpperArm.L", &[0.0, -0.3, 0.4, 0.0]),
            ("UpperLeg.L", &[0.0, 0.35, 0.35, 0.0]),
            ("LowerLeg.L", &[0.0, -0.4, -0.4, 0.0]),
        ],
        &[("Chest", &[0.0, -0.6, 0.7, 0.0])],
    )
}

/// A flinch: thrown back, arms up, then recovering.
fn hit() -> AnimationClip {
    let d = COMBAT_CLIPS[1].1;
    let times = [0.0, d * 0.3, d];
    keyed(
        &times,
        &[
            ("Spine", &[0.0, 0.35, 0.0]),
            ("Head", &[0.0, 0.3, 0.0]),
            ("UpperArm.L", &[0.0, 0.7, 0.0]),
            ("UpperArm.R", &[0.0, 0.7, 0.0]),
            ("LowerArm.L", &[0.12, 1.0, 0.12]),
            ("LowerArm.R", &[0.12, 1.0, 0.12]),
        ],
        &[],
    )
}

/// The two combat clips, in [`COMBAT_CLIPS`] order.
pub fn combat_clips() -> Vec<(&'static str, AnimationClip)> {
    vec![("Attack", attack()), ("Hit", hit())]
}

/// The character as a `.glb` (mesh `Body`, root node `Character`). The
/// same bytes every time.
pub fn character_glb() -> Result<Glb, GlbError> {
    write_character(&clips())
}

/// The character with the combat clips too ([`CLIPS`] then
/// [`COMBAT_CLIPS`]), for action games (the M8 game).
pub fn combat_character_glb() -> Result<Glb, GlbError> {
    let mut all = clips();
    all.extend(combat_clips());
    write_character(&all)
}

fn write_character(clips: &[(&'static str, AnimationClip)]) -> Result<Glb, GlbError> {
    let joints: Vec<GlbJoint<'_>> = skeleton()
        .iter()
        .zip(JOINTS)
        .map(|(&(parent, translation), name)| GlbJoint {
            name,
            parent,
            translation,
            rotation: Quat::IDENTITY,
        })
        .collect();
    let body = StandardMaterial {
        roughness: 0.75,
        ..StandardMaterial::default()
    };
    let skin = StandardMaterial {
        base_color: Color::srgb(0.93, 0.72, 0.56),
        roughness: 0.6,
        ..StandardMaterial::default()
    };
    let materials = [
        GlbMaterial {
            name: "Body",
            material: &body,
        },
        GlbMaterial {
            name: "Skin",
            material: &skin,
        },
    ];
    let clips: Vec<(&str, &AnimationClip)> = clips.iter().map(|(n, c)| (*n, c)).collect();
    write_skinned_glb(&mesh(), "Body", "Character", &materials, &joints, &clips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_is_valid_and_fully_weighted() {
        let m = mesh();
        m.validate().unwrap();
        assert_eq!(m.groups.len(), 2);
        for (j, w) in m.joints.iter().zip(&m.weights) {
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            assert!(j.iter().all(|&j| (j as usize) < JOINTS.len()));
        }
        let b = m.bounds();
        assert!(
            b.min.y.abs() < 1e-6 && (b.max.y - 1.8).abs() < 1e-6,
            "{b:?}"
        );
    }

    #[test]
    fn clips_have_the_documented_durations_and_valid_targets() {
        for ((name, clip), (expected, duration)) in clips().iter().zip(CLIPS) {
            assert_eq!(*name, expected);
            assert!((clip.duration - duration).abs() < 1e-6, "{name}");
            for t in clip.targets() {
                assert!(JOINTS.contains(&t), "{name}: {t}");
            }
        }
    }

    #[test]
    fn combat_clips_have_the_documented_durations_and_valid_targets() {
        for ((name, clip), (expected, duration)) in combat_clips().iter().zip(COMBAT_CLIPS) {
            assert_eq!(*name, expected);
            assert!((clip.duration - duration).abs() < 1e-6, "{name}");
            for t in clip.targets() {
                assert!(JOINTS.contains(&t), "{name}: {t}");
            }
        }
        assert!(combat_character_glb().unwrap().bytes.len() > character_glb().unwrap().bytes.len());
    }

    #[test]
    fn the_file_is_deterministic() {
        assert_eq!(
            character_glb().unwrap().bytes,
            character_glb().unwrap().bytes
        );
    }
}
