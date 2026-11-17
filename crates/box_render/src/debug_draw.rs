//! Immediate-mode debug lines.

use std::f32::consts::TAU;

use box_core::color::Color;
use box_core::math::Aabb;
use glam::Vec3;

use crate::uniforms::LineVertex;

const SPHERE_SEGMENTS: usize = 32;

/// Where a debug line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineLayer {
    /// In the HDR scene, depth tested. Colours go through exposure and tone
    /// mapping like everything else.
    Scene,
    /// In the HDR scene, on top of everything.
    SceneOverlay,
    /// After tone mapping, depth tested against the scene. Colours are
    /// display-referred: linear values that appear exactly as given whatever
    /// the [`Environment`](crate::Environment), alpha blended. For editor
    /// grids and selection outlines.
    Display,
    /// After tone mapping, on top of everything (gizmos).
    DisplayOverlay,
}

/// Debug line resource. Shapes queued here are drawn by the next
/// [`Renderer::render`](crate::Renderer::render),
/// [`Renderer::capture`](crate::Renderer::capture) or
/// [`Renderer::render_to`](crate::Renderer::render_to), then cleared.
///
/// The plain methods ([`line`](Self::line), [`aabb`](Self::aabb), ...) draw
/// into the HDR scene ([`LineLayer::Scene`]); the `*_on` variants take a
/// [`LineLayer`].
#[derive(Clone, Debug, Default)]
pub struct DebugDraw {
    /// Vertex pairs per [`LineLayer`], indexed by `layer as usize`.
    pub(crate) layers: [Vec<LineVertex>; 4],
}

impl DebugDraw {
    /// A depth-tested line in the HDR scene.
    pub fn line(&mut self, a: Vec3, b: Vec3, color: Color) {
        self.line_on(LineLayer::Scene, a, b, color);
    }

    /// A line in the HDR scene drawn on top of everything (no depth test).
    pub fn line_overlay(&mut self, a: Vec3, b: Vec3, color: Color) {
        self.line_on(LineLayer::SceneOverlay, a, b, color);
    }

    pub fn line_on(&mut self, layer: LineLayer, a: Vec3, b: Vec3, color: Color) {
        push(&mut self.layers[layer as usize], a, b, color);
    }

    /// The twelve edges of a box.
    pub fn aabb(&mut self, aabb: &Aabb, color: Color) {
        self.aabb_on(LineLayer::Scene, aabb, color);
    }

    pub fn aabb_on(&mut self, layer: LineLayer, aabb: &Aabb, color: Color) {
        if aabb.is_empty() {
            return;
        }
        let (n, x) = (aabb.min, aabb.max);
        let c = |i: u32| {
            Vec3::new(
                if i & 1 == 0 { n.x } else { x.x },
                if i & 2 == 0 { n.y } else { x.y },
                if i & 4 == 0 { n.z } else { x.z },
            )
        };
        for (a, b) in [
            (0, 1),
            (2, 3),
            (4, 5),
            (6, 7),
            (0, 2),
            (1, 3),
            (4, 6),
            (5, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            self.line_on(layer, c(a), c(b), color);
        }
    }

    /// Three great circles.
    pub fn sphere(&mut self, center: Vec3, radius: f32, color: Color) {
        let point = |axis: usize, t: f32| {
            let (s, c) = (t.sin() * radius, t.cos() * radius);
            center
                + match axis {
                    0 => Vec3::new(0.0, c, s),
                    1 => Vec3::new(c, 0.0, s),
                    _ => Vec3::new(c, s, 0.0),
                }
        };
        for axis in 0..3 {
            for i in 0..SPHERE_SEGMENTS {
                let t0 = i as f32 / SPHERE_SEGMENTS as f32 * TAU;
                let t1 = (i + 1) as f32 / SPHERE_SEGMENTS as f32 * TAU;
                self.line(point(axis, t0), point(axis, t1), color);
            }
        }
    }

    /// A square grid on the XZ plane at `center.y`.
    pub fn grid(&mut self, center: Vec3, half_extent: f32, spacing: f32, color: Color) {
        self.grid_on(LineLayer::Scene, center, half_extent, spacing, color);
    }

    pub fn grid_on(
        &mut self,
        layer: LineLayer,
        center: Vec3,
        half_extent: f32,
        spacing: f32,
        color: Color,
    ) {
        if spacing <= 0.0 || half_extent <= 0.0 {
            return;
        }
        let steps = (half_extent / spacing).floor() as i32;
        for i in -steps..=steps {
            let o = i as f32 * spacing;
            self.line_on(
                layer,
                center + Vec3::new(o, 0.0, -half_extent),
                center + Vec3::new(o, 0.0, half_extent),
                color,
            );
            self.line_on(
                layer,
                center + Vec3::new(-half_extent, 0.0, o),
                center + Vec3::new(half_extent, 0.0, o),
                color,
            );
        }
    }

    /// X (red), Y (green) and Z (blue) axes, drawn as overlay.
    pub fn axes(&mut self, origin: Vec3, length: f32) {
        self.line_overlay(origin, origin + Vec3::X * length, Color::RED);
        self.line_overlay(origin, origin + Vec3::Y * length, Color::GREEN);
        self.line_overlay(origin, origin + Vec3::Z * length, Color::BLUE);
    }

    /// Number of queued lines in all layers.
    pub fn line_count(&self) -> usize {
        self.layers.iter().map(Vec::len).sum::<usize>() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.layers.iter().all(Vec::is_empty)
    }

    /// Drops all queued lines (the renderer does this after each frame).
    pub fn clear(&mut self) {
        for l in &mut self.layers {
            l.clear();
        }
    }
}

fn push(list: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: Color) {
    let color = color.to_array();
    list.push(LineVertex {
        position: a.to_array(),
        color,
    });
    list.push(LineVertex {
        position: b.to_array(),
        color,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_emit_expected_line_counts() {
        let mut d = DebugDraw::default();
        d.aabb(&Aabb::new(Vec3::ZERO, Vec3::ONE), Color::WHITE);
        assert_eq!(d.line_count(), 12);
        d.axes(Vec3::ZERO, 1.0);
        assert_eq!(d.layers[LineLayer::SceneOverlay as usize].len(), 6);
        d.grid_on(LineLayer::Display, Vec3::ZERO, 1.0, 1.0, Color::WHITE);
        assert_eq!(d.layers[LineLayer::Display as usize].len(), 12);
        d.clear();
        d.grid(Vec3::ZERO, 2.0, 1.0, Color::WHITE);
        assert_eq!(d.line_count(), 10);
        d.clear();
        d.sphere(Vec3::ZERO, 1.0, Color::WHITE);
        assert_eq!(d.line_count(), 3 * SPHERE_SEGMENTS);
        assert!(!d.is_empty());
    }
}
