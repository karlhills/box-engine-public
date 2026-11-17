//! `WallGen`: the generator on a wall's derived part. A rectangular
//! section swept along the wall's polyline (`box_proc::sweep`), with both
//! end joints and any bevel fills as parameters written by the `walls`
//! rule. It never reads other entities: every neighbour fact arrives as a
//! parameter, so its cache key (the parameter hash) changes exactly when a
//! junction changes.

use box_engine::box_proc::extrude::{ExtrudeParams, extrude_into};
use box_engine::box_proc::mesh_builder::{MeshBuilder, PlaneFrame};
use box_engine::box_proc::poly::Polygon;
use box_engine::box_proc::spline::{Orientation, Path};
use box_engine::box_proc::sweep::{JointEnd, JointPlane, Profile, SweepParams, sweep_into};
use box_engine::prelude::*;

/// The sweep's mitre limit (corners stretching the section by more than
/// this factor are bevelled). The rule uses the same value for junctions
/// between walls.
pub const MITRE_LIMIT: f32 = 4.0;

/// How one end of a wall finishes (owner-local). The reflected mirror of
/// `box_proc::sweep::JointEnd` that the rule writes.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Default, Clone)]
pub enum WallEnd {
    /// A free end: square, capped.
    #[default]
    Cap,
    /// Square, no cap (something continues in line and covers it).
    Open,
    /// Mitred with a neighbouring wall of the same section.
    /// `neighbour_dir`: the neighbour's segment at the shared point in this
    /// wall's direction of travel (see `JointEnd::Mitre`).
    Mitre { neighbour_dir: Vec3 },
    /// Cut by the vertical plane through `point` with `normal` (a
    /// neighbour's face line), capped or not.
    Butt {
        point: Vec3,
        normal: Vec3,
        cap: bool,
    },
}

impl WallEnd {
    pub fn joint(self) -> JointEnd {
        match self {
            WallEnd::Cap => JointEnd::Cap,
            WallEnd::Open => JointEnd::Open,
            WallEnd::Mitre { neighbour_dir } => JointEnd::Mitre { neighbour_dir },
            WallEnd::Butt { point, normal, cap } => JointEnd::Butt {
                plane: JointPlane::new(point, normal),
                cap,
            },
        }
    }

    /// A short name for tests and messages.
    pub fn kind(self) -> &'static str {
        match self {
            WallEnd::Cap => "cap",
            WallEnd::Open => "open",
            WallEnd::Mitre { .. } => "mitre",
            WallEnd::Butt { cap: true, .. } => "butt-owner",
            WallEnd::Butt { cap: false, .. } => "butt",
        }
    }
}

/// One wall run, as the `walls` rule derives it from a `WallPath` and its
/// neighbours. Slots: the faces (`Wall`), the top (`Top`) and end caps
/// (`Ends`).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct WallGen {
    /// Centreline at the base (owner-local), ends already snapped to their
    /// junction points.
    pub points: Vec<Vec3>,
    #[reflect(@Unit("m"))]
    pub height: f32,
    #[reflect(@Unit("m"))]
    pub thickness: f32,
    pub start: WallEnd,
    pub end: WallEnd,
    /// Bevel fills at junctions sharper than the mitre limit: triangles in
    /// plan (three points each, owner-local), extruded to `height`.
    pub fills: Vec<Vec3>,
}

impl Default for WallGen {
    fn default() -> Self {
        Self {
            points: vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
            height: 2.5,
            thickness: 0.3,
            start: WallEnd::Cap,
            end: WallEnd::Cap,
            fills: Vec::new(),
        }
    }
}

/// Slot indices of [`WallGen::MATERIAL_SLOTS`].
const WALL: u32 = 0;
const TOP: u32 = 1;
const ENDS: u32 = 2;

impl Generator for WallGen {
    const VERSION: u32 = 1;
    const MATERIAL_SLOTS: &'static [&'static str] = &["Wall", "Top", "Ends"];
    const CATEGORY: &'static str = "Walls";

    fn generate(&self, _ctx: &GenContext) -> GenOutput {
        let mut warnings = Vec::new();
        let height = positive("height", self.height, 2.5, &mut warnings);
        let thickness = positive("thickness", self.thickness, 0.3, &mut warnings);
        let path = Path::from_polyline(&self.points, false, Orientation::Up(Vec3::Y));
        if path.is_empty() {
            warnings.push("a wall needs at least two distinct points".into());
            return GenOutput {
                warnings,
                ..GenOutput::empty()
            };
        }
        let mut mb = MeshBuilder::new().with_tangents();
        // Edges: bottom, right, top, left.
        let profile = Profile::rect(thickness, height).with_slots(vec![WALL, WALL, TOP, WALL]);
        sweep_into(
            &mut mb,
            &path,
            &SweepParams {
                profile,
                start: self.start.joint(),
                end: self.end.joint(),
                miter_limit: MITRE_LIMIT,
                cap_slot: ENDS,
                uv_scale: Vec2::ONE,
            },
        );
        for tri in self.fills.as_chunks::<3>().0 {
            fill_into(&mut mb, *tri, height);
        }
        let built = mb.build();
        warnings.extend(built.warnings);
        GenOutput {
            mesh: Some(built.mesh),
            warnings,
            ..GenOutput::default()
        }
    }
}

/// A plan triangle extruded upward from its first point's height.
fn fill_into(mb: &mut MeshBuilder, tri: [Vec3; 3], height: f32) {
    if tri.iter().any(|p| !p.is_finite()) {
        mb.warn("bevel fill with non-finite points skipped");
        return;
    }
    // PLAN maps (x, y) to (x, 0, -y).
    let outline: Vec<Vec2> = tri.iter().map(|p| Vec2::new(p.x, -p.z)).collect();
    let area = box_engine::box_proc::poly::signed_area(&outline);
    if area.abs() < 1e-6 {
        return;
    }
    let mut outline = outline;
    if area < 0.0 {
        outline.reverse();
    }
    extrude_into(
        mb,
        &Polygon::new(outline),
        &ExtrudeParams {
            depth: height,
            frame: PlaneFrame::PLAN.translated(Vec3::new(0.0, tri[0].y, 0.0)),
            side_slot: WALL,
            cap_slot: TOP,
            ..ExtrudeParams::default()
        },
    );
}

/// `v` when finite and above 0, else `fallback` with a warning.
pub fn positive(name: &str, v: f32, fallback: f32, warnings: &mut Vec<String>) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        warnings.push(format!("{name} must be above 0; using {fallback}"));
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use box_engine::box_proc::mesh_builder::{edge_report, signed_volume};

    #[test]
    fn a_free_wall_is_a_closed_box_in_three_slots() {
        let out = WallGen::default().generate(&GenContext::new(0));
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        let m = out.mesh.unwrap();
        assert!(edge_report(&m, 1e-5).is_closed_manifold());
        assert!((signed_volume(&m) - 4.0 * 0.3 * 2.5).abs() < 1e-4);
        let slots: Vec<u32> = m.groups.iter().map(|g| g.material_slot).collect();
        assert_eq!(slots, [WALL, TOP, ENDS]);
    }

    #[test]
    fn bad_parameters_degrade_with_warnings() {
        let out = WallGen {
            points: vec![Vec3::ONE],
            height: f32::NAN,
            ..WallGen::default()
        }
        .generate(&GenContext::new(0));
        assert!(out.mesh.is_none());
        assert_eq!(out.warnings.len(), 2, "{:?}", out.warnings);
    }

    #[test]
    fn a_fill_adds_a_closed_prism() {
        let plain = WallGen::default()
            .generate(&GenContext::new(0))
            .mesh
            .unwrap();
        let filled = WallGen {
            fills: vec![
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.15),
                Vec3::new(4.1, 0.0, 0.0),
            ],
            ..WallGen::default()
        }
        .generate(&GenContext::new(0))
        .mesh
        .unwrap();
        let prism = 0.5 * 0.15 * 0.1 * 2.5;
        assert!((signed_volume(&filled) - signed_volume(&plain) - prism).abs() < 1e-4);
    }
}
