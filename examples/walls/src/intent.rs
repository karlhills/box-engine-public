//! The saved intent: what a scene stores for walls and fences. Nothing
//! else of this example is saved; every part, mesh and junction is derived
//! from these two components by the rules.

use box_engine::prelude::*;

/// A wall: a rectangular section swept along a polyline. Ends that meet
/// other walls connect automatically (mitre, butt or bevel).
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct WallPath {
    /// Corner points of the wall's centreline at its base, relative to the
    /// entity. Ends within 10 cm of another wall's end or face connect.
    pub points: Vec<Vec3>,
    /// Height above the points.
    #[reflect(@Range { min: 0.1, max: 20.0 }, @Unit("m"), @Step(0.05))]
    pub height: f32,
    /// Thickness across the centreline.
    #[reflect(@Range { min: 0.02, max: 2.0 }, @Unit("m"), @Step(0.01))]
    pub thickness: f32,
}

impl Default for WallPath {
    fn default() -> Self {
        Self {
            points: vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
            height: 2.5,
            thickness: 0.3,
        }
    }
}

/// A fence: posts along a polyline with rails between them. Fences that
/// meet share one corner post.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default, Clone)]
pub struct FencePath {
    /// Points of the fence line at ground level, relative to the entity.
    /// Every point gets a post.
    pub points: Vec<Vec3>,
    /// Height of the posts.
    #[reflect(@Range { min: 0.2, max: 5.0 }, @Unit("m"), @Step(0.05))]
    pub height: f32,
    /// Target distance between posts; each straight run is divided evenly.
    #[reflect(@Range { min: 0.3, max: 10.0 }, @Unit("m"), @Step(0.05))]
    pub post_spacing: f32,
    /// Width of the square posts.
    #[reflect(@Range { min: 0.03, max: 0.6 }, @Unit("m"), @Step(0.01))]
    pub post_size: f32,
    /// Number of horizontal rails.
    #[reflect(@Range { min: 0.0, max: 8.0 })]
    pub rail_count: u32,
}

impl Default for FencePath {
    fn default() -> Self {
        Self {
            points: vec![Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0)],
            height: 1.1,
            post_spacing: 2.0,
            post_size: 0.14,
            rail_count: 2,
        }
    }
}
