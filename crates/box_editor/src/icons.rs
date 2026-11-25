//! Viewport icons and helpers for entities that draw nothing themselves.
//!
//! Lights, ambient volumes, particle emitters, cameras, empties (and groups
//! of them) and prefab instances whose content has not loaded get a small
//! icon of constant screen size, painted with egui over the viewport image
//! (so it is in every editor capture). An entity gets one when it is a
//! light, an ambient volume, an emitter or a camera, or when no mesh is
//! rendered at or below it; instanced prefab content gets none (its root
//! stands for it). Icons are clicked by distance in points and win over
//! meshes behind them; the marquee hits an icon's disc. The View menu turns
//! them off ([`ViewOptions::icons`]).
//!
//! Selected entities also get helpers, drawn as display-referred overlay
//! lines: a camera's frustum (its field of view, near plane and the
//! viewport's aspect), a directional light's direction, a point light's
//! range (three circles), a spot light's cone, an ambient volume's box, a
//! particle emitter's emission cone and spawn sphere and collider
//! wireframes (for the selection, every
//! collider or none: [`ColliderDisplay`]).
//!
//! Cost: one pass over the saved entities per frame while the viewport is
//! shown (classification, one projection and one disc each; measured in
//! `tests/viewport.rs`), nothing for unselected helpers.

use std::collections::HashSet;

use box_core::color::Color;
use box_core::identity::Uid;
use box_core::prefab::PrefabInstance;
use box_core::prelude::{Entity, GlobalTransform, Primitive, Shape, World};
use box_physics::{Collider, ColliderShape, Dynamics, Joint, JointKind};
use box_render::{
    AmbientVolume, Camera, DebugDraw, DirectionalLight, LineLayer, MeshRenderer, ParticleEmitter,
    PointLight, SpotLight,
};
use egui::{Color32, Painter, Pos2, pos2, vec2};
use glam::{Affine3A, Quat, Vec2, Vec3};

use crate::gizmo::GizmoView;
use crate::state::EditorState;
use crate::theme::Tokens;

/// Icon disc radius in points.
pub const ICON_RADIUS: f32 = 11.0;
/// A click within this many points of an icon's centre picks it.
pub const PICK_RADIUS: f32 = 12.0;
const CIRCLE_SEGMENTS: usize = 48;

/// Which collider wireframes the viewport draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColliderDisplay {
    /// Colliders of the selected entities.
    #[default]
    Selected,
    All,
    None,
}

impl ColliderDisplay {
    pub const ALL: [Self; 3] = [Self::Selected, Self::All, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::Selected => "Selected",
            Self::All => "All",
            Self::None => "None",
        }
    }
}

/// View menu toggles of the viewport (for the session).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    /// Icons for lights, cameras, empties and unloaded prefabs.
    pub icons: bool,
    pub colliders: ColliderDisplay,
    /// Contact points between rigid bodies, while playing.
    pub contacts: bool,
    /// Sound source ranges, and lines to the listener while playing.
    pub audio: bool,
    /// The navmesh: where enemies can walk (View ▸ Navigation).
    pub nav: bool,
    /// Which particle emitters simulate while editing.
    pub particles: crate::particle_preview::ParticlePreview,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            icons: true,
            colliders: ColliderDisplay::Selected,
            contacts: false,
            audio: false,
            nav: false,
            particles: Default::default(),
        }
    }
}

/// What an icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconKind {
    /// A directional light.
    Sun,
    PointLight,
    SpotLight,
    /// An `AmbientVolume` (a box of ambient light).
    AmbientVolume,
    /// A `ParticleEmitter`.
    Particles,
    Camera,
    /// An empty, or a group without meshes below it.
    Empty,
    /// A prefab instance without loaded meshes.
    Prefab,
}

/// An icon placed on the viewport this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportIcon {
    pub uid: Uid,
    pub entity: Entity,
    pub kind: IconKind,
    /// Centre in viewport points (origin top-left).
    pub pos: Vec2,
    /// Distance in front of the camera.
    pub depth: f32,
}

/// Per-frame viewport overlay data the pane computes and paints.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    /// Visible icons, farthest first (the paint order).
    pub icons: Vec<ViewportIcon>,
    /// The icon under the pointer.
    pub hover: Option<Uid>,
    /// Show the fly speed until this time (egui seconds).
    pub fly_pill_until: f64,
}

/// The saved entities that get an icon, with their kind and world origin.
pub fn icon_entities(world: &World) -> Vec<(Entity, Uid, IconKind, Vec3)> {
    // Entities with a rendered mesh at or below them.
    let mut covered: HashSet<Entity> = HashSet::new();
    for (e, _) in world.query::<(Entity, &MeshRenderer)>().iter() {
        let mut cur = Some(e);
        while let Some(c) = cur {
            if !covered.insert(c) {
                break;
            }
            cur = world.parent(c);
        }
    }
    let mut out = Vec::new();
    for (e, uid, gt) in world.query::<(Entity, &Uid, &GlobalTransform)>().iter() {
        let kind = if world.has::<DirectionalLight>(e) {
            IconKind::Sun
        } else if world.has::<PointLight>(e) {
            IconKind::PointLight
        } else if world.has::<SpotLight>(e) {
            IconKind::SpotLight
        } else if world.has::<AmbientVolume>(e) {
            IconKind::AmbientVolume
        } else if world.has::<ParticleEmitter>(e) {
            IconKind::Particles
        } else if world.has::<Camera>(e) {
            IconKind::Camera
        } else if covered.contains(&e) {
            continue;
        } else if world.has::<PrefabInstance>(e) {
            IconKind::Prefab
        } else {
            IconKind::Empty
        };
        if box_core::prefab::is_generated(world, e) {
            continue;
        }
        out.push((e, *uid, kind, Vec3::from(gt.0.translation)));
    }
    out
}

/// Fills `out` with the icons visible through `view`, farthest first.
pub fn collect(world: &World, view: &GizmoView, out: &mut Vec<ViewportIcon>) {
    out.clear();
    let vp = view.view_proj();
    let margin = Vec2::splat(ICON_RADIUS);
    for (entity, uid, kind, p) in icon_entities(world) {
        let clip = vp * p.extend(1.0);
        if clip.w <= 1e-3 {
            continue;
        }
        let Some(pos) = view.clip_to_points(clip) else {
            continue;
        };
        if pos.cmplt(-margin).any() || pos.cmpgt(view.size + margin).any() {
            continue;
        }
        out.push(ViewportIcon {
            uid,
            entity,
            kind,
            pos,
            depth: clip.w,
        });
    }
    out.sort_by(|a, b| b.depth.total_cmp(&a.depth));
}

/// The icon at `cursor` (viewport points): the nearest to the camera among
/// those within [`PICK_RADIUS`].
pub fn pick(icons: &[ViewportIcon], cursor: Vec2) -> Option<Uid> {
    icons
        .iter()
        .filter(|i| i.pos.distance(cursor) <= PICK_RADIUS)
        .min_by(|a, b| a.depth.total_cmp(&b.depth))
        .map(|i| i.uid)
}

/// True if the disc `centre ± radius` meets the rectangle `min..max`.
pub fn disc_meets_rect(centre: Vec2, radius: f32, min: Vec2, max: Vec2) -> bool {
    centre.clamp(min, max).distance(centre) <= radius
}

/// Paints the icons over the viewport whose top-left corner is `origin`.
/// Selected icons are filled with the accent (the primary's lighter, as its
/// gizmo sits on it), the hovered one is darker with a light ring.
///
/// The glyphs are rasterised once per egui context into a small texture
/// ([`atlas`]); every icon is three textured quads (disc, ring, glyph) in
/// one mesh, so a thousand icons cost a few thousand vertices and no path
/// tessellation.
pub fn paint(p: &Painter, origin: Pos2, state: &EditorState, t: &Tokens) {
    let overlay = &state.viewport.overlay;
    if overlay.icons.is_empty() {
        return;
    }
    let texture = atlas(p.ctx());
    let mut mesh = egui::Mesh::with_texture(texture.id());
    let primary = state.primary();
    let half = vec2(CELL_POINTS, CELL_POINTS) * 0.5;
    for icon in &overlay.icons {
        let c = origin + vec2(icon.pos.x, icon.pos.y);
        let rect = egui::Rect::from_center_size(c, half * 2.0);
        let selected = state.is_selected(icon.uid);
        let hovered = overlay.hover == Some(icon.uid);
        let (fill, glyph) = if Some(icon.uid) == primary {
            (t.accent.gamma_multiply(0.55), t.accent_text)
        } else if selected {
            (t.accent.gamma_multiply(0.85), t.accent_text)
        } else if hovered {
            (Color32::from_black_alpha(190), t.text_strong)
        } else {
            (t.overlay_bg, t.overlay_text)
        };
        let ring = if hovered || selected {
            t.overlay_text
        } else {
            Color32::from_white_alpha(40)
        };
        for (cell, color) in [
            (Cell::Disc, fill),
            (Cell::Ring, ring),
            (Cell::Glyph(icon.kind), glyph),
        ] {
            mesh.add_rect_with_uv(rect, cell.uv(), color);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Size of an atlas cell in points (the disc and its ring fit inside).
const CELL_POINTS: f32 = 26.0;
/// Atlas cell size in texels (enough for 2.5 pixels per point).
const CELL_PX: usize = 64;
const CELLS: usize = 10;

/// A cell of the icon atlas.
#[derive(Clone, Copy)]
enum Cell {
    Disc,
    Ring,
    Glyph(IconKind),
}

impl Cell {
    fn index(self) -> usize {
        match self {
            Self::Disc => 0,
            Self::Ring => 1,
            Self::Glyph(IconKind::Sun) => 2,
            Self::Glyph(IconKind::PointLight) => 3,
            Self::Glyph(IconKind::Camera) => 4,
            Self::Glyph(IconKind::Empty) => 5,
            Self::Glyph(IconKind::Prefab) => 6,
            Self::Glyph(IconKind::SpotLight) => 7,
            Self::Glyph(IconKind::AmbientVolume) => 8,
            Self::Glyph(IconKind::Particles) => 9,
        }
    }

    fn uv(self) -> egui::Rect {
        let w = 1.0 / CELLS as f32;
        let x = self.index() as f32 * w;
        egui::Rect::from_min_max(pos2(x, 0.0), pos2(x + w, 1.0))
    }
}

/// The icon atlas of this egui context (built on first use).
fn atlas(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::unique("box_editor_icon_atlas");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let t = ctx.load_texture(
        "viewport icons",
        atlas_image(),
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Distance-field primitives of the glyphs, in points about the cell centre
/// (+Y down). Stroke widths are full widths.
enum Prim {
    Disc(Vec2, f32),
    Ring(Vec2, f32, f32),
    Segment(Vec2, Vec2, f32),
    /// Rounded rectangle outline: centre, half size, corner radius, width.
    RectStroke(Vec2, Vec2, f32, f32),
    /// A filled convex polygon, counter-clockwise on screen (+Y down).
    Convex(Vec<Vec2>),
}

impl Prim {
    fn distance(&self, p: Vec2) -> f32 {
        match self {
            Self::Disc(c, r) => p.distance(*c) - r,
            Self::Ring(c, r, w) => (p.distance(*c) - r).abs() - w * 0.5,
            Self::Segment(a, b, w) => {
                let ab = *b - *a;
                let t = ((p - *a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
                p.distance(*a + ab * t) - w * 0.5
            }
            Self::RectStroke(c, half, r, w) => {
                let q = (p - *c).abs() - *half + Vec2::splat(*r);
                let d = q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r;
                d.abs() - w * 0.5
            }
            Self::Convex(pts) => {
                let n = pts.len();
                (0..n)
                    .map(|i| {
                        let (a, b) = (pts[i], pts[(i + 1) % n]);
                        let normal = Vec2::new(b.y - a.y, a.x - b.x).normalize_or_zero();
                        (p - a).dot(normal)
                    })
                    .fold(f32::NEG_INFINITY, f32::max)
            }
        }
    }
}

fn cell_prims(cell: usize) -> Vec<Prim> {
    use Prim::*;
    let v = Vec2::new;
    const W: f32 = 1.5;
    match cell {
        0 => vec![Disc(Vec2::ZERO, ICON_RADIUS)],
        1 => vec![Ring(Vec2::ZERO, ICON_RADIUS, 1.0)],
        // Sun: a disc and eight rays.
        2 => {
            let mut p = vec![Disc(Vec2::ZERO, 3.4)];
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let d = v(a.cos(), a.sin());
                p.push(Segment(d * 5.4, d * 7.6, W));
            }
            p
        }
        // Point light: a bulb on a base.
        3 => vec![
            Ring(v(0.0, -1.6), 4.4, W),
            Disc(v(0.0, -1.6), 1.6),
            Segment(v(-2.2, 4.2), v(2.2, 4.2), W),
            Segment(v(-1.5, 6.4), v(1.5, 6.4), W),
        ],
        // Camera: a body and a lens.
        4 => vec![
            RectStroke(v(-2.25, 0.0), v(4.75, 4.0), 1.0, W),
            Convex(vec![v(3.3, 0.0), v(7.2, -3.6), v(7.2, 3.6)]),
        ],
        // Empty: a cross through a small circle.
        5 => vec![
            Segment(v(-6.5, 0.0), v(6.5, 0.0), W),
            Segment(v(0.0, -6.5), v(0.0, 6.5), W),
            Ring(Vec2::ZERO, 3.2, W),
        ],
        // Spot light: a lamp head and its cone of light.
        7 => vec![
            Convex(vec![
                v(-2.6, -6.8),
                v(2.6, -6.8),
                v(3.6, -2.6),
                v(-3.6, -2.6),
            ]),
            Segment(v(-3.2, -1.4), v(-6.4, 6.2), W),
            Segment(v(3.2, -1.4), v(6.4, 6.2), W),
            Segment(v(0.0, -0.6), v(0.0, 6.6), W),
        ],
        // Ambient volume: a box (corner brackets) with the light it holds.
        8 => {
            let mut p = vec![Disc(Vec2::ZERO, 2.4)];
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let corner = v(6.4 * sx, 6.4 * sy);
                p.push(Segment(corner, corner - v(3.4 * sx, 0.0), W));
                p.push(Segment(corner, corner - v(0.0, 3.4 * sy), W));
            }
            for k in 0..4 {
                let a = k as f32 * std::f32::consts::FRAC_PI_2 + std::f32::consts::FRAC_PI_4;
                let d = v(a.cos(), a.sin());
                p.push(Segment(d * 3.6, d * 4.8, 1.2));
            }
            p
        }
        // Particle emitter: dots fanning up from the emitter.
        9 => vec![
            Disc(v(0.0, 5.6), 1.6),
            Disc(v(-4.6, -3.4), 1.5),
            Disc(v(0.0, -5.6), 1.8),
            Disc(v(4.6, -3.4), 1.5),
            Disc(v(-2.2, 0.6), 1.2),
            Disc(v(2.4, 0.2), 1.2),
        ],
        // Prefab: an isometric box.
        _ => {
            let s = 6.2;
            let hex = [
                v(0.0, -s),
                v(s * 0.9, -s * 0.5),
                v(s * 0.9, s * 0.5),
                v(0.0, s),
                v(-s * 0.9, s * 0.5),
                v(-s * 0.9, -s * 0.5),
            ];
            let mut p: Vec<Prim> = (0..6)
                .map(|i| Segment(hex[i], hex[(i + 1) % 6], W))
                .collect();
            for end in [hex[1], hex[5], hex[3]] {
                p.push(Segment(Vec2::ZERO, end, W));
            }
            p
        }
    }
}

/// White glyphs with anti-aliased coverage in alpha (premultiplied), one
/// cell per [`Cell`], side by side.
fn atlas_image() -> egui::ColorImage {
    let (w, h) = (CELL_PX * CELLS, CELL_PX);
    let px_per_point = CELL_PX as f32 / CELL_POINTS;
    let mut pixels = vec![Color32::TRANSPARENT; w * h];
    for cell in 0..CELLS {
        let prims = cell_prims(cell);
        for y in 0..h {
            for x in 0..CELL_PX {
                let p = (Vec2::new(x as f32, y as f32) + 0.5 - CELL_PX as f32 * 0.5) / px_per_point;
                let d = prims
                    .iter()
                    .map(|s| s.distance(p))
                    .fold(f32::INFINITY, f32::min);
                let a = ((0.5 - d * px_per_point).clamp(0.0, 1.0) * 255.0).round() as u8;
                pixels[y * w + cell * CELL_PX + x] = Color32::from_rgba_premultiplied(a, a, a, a);
            }
        }
    }
    egui::ColorImage::new([w, h], pixels)
}

// ----- helpers for selected entities (debug lines) -----

/// Colour of collider wireframes (display-referred).
const COLLIDER: Color = Color::linear(0.25, 0.85, 1.0);

/// Queues the helpers: frustum, light direction and range, and joint anchor
/// and axis for the selected entities, collider wireframes as
/// [`ViewOptions::colliders`] says, and rigid-body contact points while
/// playing if [`ViewOptions::contacts`] is on.
pub(crate) fn draw_helpers(state: &EditorState, dd: &mut DebugDraw, accent: Color) {
    let Some(view) = state.gizmo_view() else {
        return;
    };
    let world = &state.world;
    let layer = LineLayer::DisplayOverlay;
    let primary = state.primary();
    for &uid in &state.selection {
        let Some(e) = world.entity_by_uid(uid) else {
            continue;
        };
        let Some(gt) = world.get::<GlobalTransform>(e).map(|g| g.0) else {
            continue;
        };
        let color = if Some(uid) == primary {
            accent
        } else {
            accent.with_alpha(0.55)
        };
        let (_, rot, pos) = gt.to_scale_rotation_translation();
        if let Some(cam) = world.get::<Camera>(e) {
            frustum(
                dd,
                layer,
                pos,
                rot,
                &cam,
                view.size.x / view.size.y.max(1.0),
                color,
            );
        }
        if world.has::<DirectionalLight>(e) {
            sun_arrow(dd, layer, &view, pos, rot, color);
        }
        if let Some(light) = world.get::<PointLight>(e) {
            let r = light.range.max(0.0);
            for (a, b) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
                circle(dd, layer, pos, a * r, b * r, color);
            }
        }
        if let Some(light) = world.get::<SpotLight>(e) {
            spot_cone(dd, layer, pos, rot, &light, color);
        }
        if let Some(volume) = world.get::<AmbientVolume>(e) {
            let corner = |k: usize| {
                let h = volume.half_extents;
                gt.transform_point3(Vec3::new(
                    if k & 1 == 0 { -h.x } else { h.x },
                    if k & 2 == 0 { -h.y } else { h.y },
                    if k & 4 == 0 { -h.z } else { h.z },
                ))
            };
            for (a, b) in crate::marquee::BOX_EDGES {
                dd.line_on(layer, corner(a), corner(b), color);
            }
        }
        if let Some(emitter) = world.get::<ParticleEmitter>(e) {
            emission_cone(dd, layer, &view, &gt, &emitter, color);
        }
        if let Some(j) = world.get::<Joint>(e) {
            joint(dd, layer, world, &gt, &j, color);
        }
        if state.viewport.view.colliders == ColliderDisplay::Selected
            && let Some(c) = world.get::<Collider>(e)
        {
            collider(
                dd,
                layer,
                &gt,
                &c,
                world.get::<Primitive>(e).as_deref(),
                COLLIDER,
            );
        }
    }
    if state.viewport.view.colliders == ColliderDisplay::All {
        for (e, c, gt) in world
            .query::<(Entity, &Collider, &GlobalTransform)>()
            .iter()
        {
            let selected = world.uid(e).is_some_and(|u| state.is_selected(u));
            let color = if selected {
                COLLIDER
            } else {
                COLLIDER.with_alpha(0.55)
            };
            collider(
                dd,
                layer,
                &gt.0,
                c,
                world.get::<Primitive>(e).as_deref(),
                color,
            );
        }
    }
    if state.viewport.view.contacts
        && state.is_playing()
        && let Some(dynamics) = world.get_resource::<Dynamics>()
    {
        for c in dynamics.contact_points() {
            cross(dd, layer, c.point, 0.04, CONTACT);
            dd.line_on(layer, c.point, c.point + c.normal * 0.25, CONTACT);
        }
    }
}

/// Colour of contact points and joint helpers (display-referred).
const CONTACT: Color = Color::linear(1.0, 0.35, 0.2);

/// A small three-axis cross of half-size `r` at `p`.
fn cross(dd: &mut DebugDraw, layer: LineLayer, p: Vec3, r: f32, color: Color) {
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        dd.line_on(layer, p - axis * r, p + axis * r, color);
    }
}

/// A joint's anchor, its hinge or slide axis, and a line to the connected
/// body (or a mark on the world anchor).
fn joint(
    dd: &mut DebugDraw,
    layer: LineLayer,
    world: &World,
    gt: &Affine3A,
    j: &Joint,
    color: Color,
) {
    let anchor = gt.transform_point3(if j.anchor.is_finite() {
        j.anchor
    } else {
        Vec3::ZERO
    });
    cross(dd, layer, anchor, 0.08, CONTACT);
    if matches!(j.kind, JointKind::Hinge | JointKind::Prismatic) {
        let axis = gt.transform_vector3(j.axis).normalize_or_zero() * 0.5;
        dd.line_on(layer, anchor - axis, anchor + axis, CONTACT);
    }
    let other = j
        .connected
        .and_then(|u| world.entity_by_uid(u))
        .and_then(|o| {
            world
                .get::<GlobalTransform>(o)
                .map(|g| g.0.translation.into())
        });
    if let Some(other) = other {
        dd.line_on(layer, anchor, other, color.with_alpha(0.55));
    }
}

/// A circle about `centre` through `centre + a` and `centre + b` (a and b
/// perpendicular, of equal length).
fn circle(dd: &mut DebugDraw, layer: LineLayer, centre: Vec3, a: Vec3, b: Vec3, color: Color) {
    arc(dd, layer, centre, a, b, 0.0, std::f32::consts::TAU, color);
}

#[allow(clippy::too_many_arguments)]
fn arc(
    dd: &mut DebugDraw,
    layer: LineLayer,
    centre: Vec3,
    a: Vec3,
    b: Vec3,
    from: f32,
    to: f32,
    color: Color,
) {
    let n = ((to - from).abs() / std::f32::consts::TAU * CIRCLE_SEGMENTS as f32).ceil() as usize;
    let n = n.max(4);
    let at = |k: usize| {
        let t = from + (to - from) * k as f32 / n as f32;
        centre + a * t.cos() + b * t.sin()
    };
    for k in 0..n {
        dd.line_on(layer, at(k), at(k + 1), color);
    }
}

/// The camera's view pyramid: the near plane, a plane a little further out
/// (cameras have no far plane) and a triangle on its top edge for "up".
fn frustum(
    dd: &mut DebugDraw,
    layer: LineLayer,
    pos: Vec3,
    rot: Quat,
    cam: &Camera,
    aspect: f32,
    color: Color,
) {
    let tan = (cam.fov_y.clamp(0.01, 3.0) * 0.5).tan();
    let near = cam.near.max(1e-3);
    let far = (near * 4.0).max(2.0);
    let rect = |d: f32| {
        let (h, w) = (tan * d, tan * d * aspect);
        [(-w, -h), (w, -h), (w, h), (-w, h)].map(|(x, y)| pos + rot * Vec3::new(x, y, -d))
    };
    let (n, f) = (rect(near), rect(far));
    for k in 0..4 {
        dd.line_on(layer, n[k], n[(k + 1) % 4], color);
        dd.line_on(layer, f[k], f[(k + 1) % 4], color);
        dd.line_on(layer, pos, f[k], color);
    }
    let h = tan * far;
    let apex = pos + rot * Vec3::new(0.0, h * 1.35, -far);
    dd.line_on(layer, f[2], apex, color);
    dd.line_on(layer, f[3], apex, color);
}

/// A directional light's direction: an arrow of constant screen size from
/// its origin, with a ring and parallel rays.
fn sun_arrow(
    dd: &mut DebugDraw,
    layer: LineLayer,
    view: &GizmoView,
    pos: Vec3,
    rot: Quat,
    color: Color,
) {
    let wpp = view.world_per_point(pos);
    let (dir, u, v) = (rot * Vec3::NEG_Z, rot * Vec3::X, rot * Vec3::Y);
    let len = wpp * 80.0;
    let r = wpp * 16.0;
    let tip = pos + dir * len;
    dd.line_on(layer, pos, tip, color);
    let base = tip - dir * wpp * 12.0;
    for side in [u, -u, v, -v] {
        dd.line_on(layer, tip, base + side * wpp * 5.0, color);
    }
    circle(dd, layer, pos, u * r, v * r, color);
    for side in [u, -u, v, -v] {
        let s = pos + side * r;
        dd.line_on(layer, s, s + dir * len * 0.45, color);
    }
}

/// A spot light's cone: its outer angle out to its range (four edges and
/// the rim) and the inner angle's rim.
fn spot_cone(
    dd: &mut DebugDraw,
    layer: LineLayer,
    pos: Vec3,
    rot: Quat,
    light: &SpotLight,
    color: Color,
) {
    let range = light.range.max(0.0);
    let (dir, u, v) = (rot * Vec3::NEG_Z, rot * Vec3::X, rot * Vec3::Y);
    let outer = light.outer_angle.clamp(0.0, std::f32::consts::FRAC_PI_2);
    let inner = light.inner_angle.clamp(0.0, outer);
    let rim = |angle: f32| (pos + dir * range * angle.cos(), range * angle.sin());
    let (centre, r) = rim(outer);
    circle(dd, layer, centre, u * r, v * r, color);
    for side in [u, -u, v, -v] {
        dd.line_on(layer, pos, centre + side * r, color);
    }
    if inner > 1e-3 && inner < outer - 1e-3 {
        let (centre, r) = rim(inner);
        circle(
            dd,
            layer,
            centre,
            u * r,
            v * r,
            color.with_alpha(color.a * 0.5),
        );
    }
}

/// A particle emitter's emission: its cone (`direction` and `cone_angle`
/// in the entity's space) drawn as a rim and four edges of constant screen
/// length, and the `spawn_radius` sphere (three circles) when it has one.
fn emission_cone(
    dd: &mut DebugDraw,
    layer: LineLayer,
    view: &GizmoView,
    gt: &Affine3A,
    e: &ParticleEmitter,
    color: Color,
) {
    let pos = Vec3::from(gt.translation);
    let axis = gt
        .transform_vector3(e.direction)
        .try_normalize()
        .unwrap_or(Vec3::Y);
    let len = view.world_per_point(pos) * 70.0;
    let angle = e.cone_angle.clamp(0.0, std::f32::consts::PI);
    let rot = Quat::from_rotation_arc(Vec3::Y, axis);
    let (u, v) = (rot * Vec3::X, rot * Vec3::Z);
    // The rim at `len` from the origin; a wide cone (over 90°) opens
    // backwards, a full sphere has no rim.
    let (centre, r) = (pos + axis * len * angle.cos(), len * angle.sin());
    if angle < std::f32::consts::PI - 1e-3 {
        circle(dd, layer, centre, u * r, v * r, color);
    }
    for side in [u, -u, v, -v] {
        dd.line_on(layer, pos, centre + side * r, color);
    }
    dd.line_on(layer, pos, pos + axis * len, color);
    let radius = e.spawn_radius.max(0.0) * gt.matrix3.x_axis.length();
    if radius > 0.0 {
        let faint = color.with_alpha(color.a * 0.5);
        for (a, b) in [(Vec3::X, Vec3::Y), (Vec3::Y, Vec3::Z), (Vec3::Z, Vec3::X)] {
            circle(dd, layer, pos, a * radius, b * radius, faint);
        }
    }
}

/// A collider's shape in its entity's world pose, scaled the way
/// `box_physics` builds it.
fn collider(
    dd: &mut DebugDraw,
    layer: LineLayer,
    global: &Affine3A,
    c: &Collider,
    primitive: Option<&Primitive>,
    color: Color,
) {
    let (scale, rot, _) = global.to_scale_rotation_translation();
    let s = scale.abs();
    let centre = global.transform_point3(c.offset);
    let radial = s.x.max(s.z);
    let (x, y, z) = (rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z);
    let shape = match &c.shape {
        ColliderShape::Primitive => match primitive.map(|p| &p.shape) {
            Some(Shape::Cuboid { size }) => ColliderShape::Cuboid { size: *size },
            Some(Shape::Sphere { radius, .. }) => ColliderShape::Sphere { radius: *radius },
            Some(Shape::Cylinder { radius, height, .. }) => ColliderShape::Cylinder {
                radius: *radius,
                height: *height,
            },
            Some(Shape::Capsule {
                radius,
                half_height,
                ..
            }) => ColliderShape::Capsule {
                radius: *radius,
                half_height: *half_height,
            },
            Some(Shape::Plane { size, .. }) => ColliderShape::Cuboid {
                size: Vec3::new(size.x, 0.0, size.y),
            },
            // No primitive: no collider exists.
            None => return,
        },
        other => other.clone(),
    };
    match shape {
        ColliderShape::Cuboid { size } => {
            let h = size.abs() * s * 0.5;
            let corner = |k: usize| {
                centre
                    + x * if k & 1 == 0 { -h.x } else { h.x }
                    + y * if k & 2 == 0 { -h.y } else { h.y }
                    + z * if k & 4 == 0 { -h.z } else { h.z }
            };
            for (a, b) in crate::marquee::BOX_EDGES {
                dd.line_on(layer, corner(a), corner(b), color);
            }
        }
        ColliderShape::Sphere { radius } => {
            let r = radius.abs() * s.max_element();
            for (a, b) in [(x, y), (y, z), (z, x)] {
                circle(dd, layer, centre, a * r, b * r, color);
            }
        }
        ColliderShape::Cylinder { radius, height } => {
            let (r, hh) = (radius.abs() * radial, height.abs() * 0.5 * s.y);
            for k in [-1.0, 1.0] {
                circle(dd, layer, centre + y * hh * k, x * r, z * r, color);
            }
            for side in [x, -x, z, -z] {
                dd.line_on(
                    layer,
                    centre + side * r - y * hh,
                    centre + side * r + y * hh,
                    color,
                );
            }
        }
        ColliderShape::Capsule {
            radius,
            half_height,
        } => {
            let (r, hh) = (radius.abs() * radial, half_height.abs() * s.y);
            use std::f32::consts::PI;
            for k in [-1.0f32, 1.0] {
                let c = centre + y * hh * k;
                circle(dd, layer, c, x * r, z * r, color);
                // Half circles over the ends, in the XY and ZY planes.
                for side in [x, z] {
                    arc(dd, layer, c, side * r, y * r * k, 0.0, PI, color);
                }
            }
            for side in [x, -x, z, -z] {
                dd.line_on(
                    layer,
                    centre + side * r - y * hh,
                    centre + side * r + y * hh,
                    color,
                );
            }
        }
        // A mesh collider is the drawn mesh itself.
        ColliderShape::Primitive | ColliderShape::Mesh | ColliderShape::ConvexHull => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_icon_within_the_radius_is_picked() {
        let icon = |n: u128, x: f32, depth: f32| ViewportIcon {
            uid: Uid::from_u128(n),
            entity: Entity::DANGLING,
            kind: IconKind::Empty,
            pos: Vec2::new(x, 50.0),
            depth,
        };
        let icons = [icon(1, 100.0, 9.0), icon(2, 104.0, 3.0)];
        assert_eq!(
            pick(&icons, Vec2::new(101.0, 50.0)),
            Some(Uid::from_u128(2))
        );
        assert_eq!(pick(&icons, Vec2::new(90.0, 50.0)), Some(Uid::from_u128(1)));
        assert_eq!(pick(&icons, Vec2::new(60.0, 50.0)), None);
    }

    #[test]
    fn discs_and_rectangles() {
        let (min, max) = (Vec2::ZERO, Vec2::splat(10.0));
        assert!(disc_meets_rect(Vec2::new(5.0, 5.0), 1.0, min, max));
        assert!(disc_meets_rect(Vec2::new(15.0, 5.0), 6.0, min, max));
        assert!(!disc_meets_rect(Vec2::new(15.0, 5.0), 4.0, min, max));
        assert!(!disc_meets_rect(Vec2::new(18.0, 18.0), 10.0, min, max));
    }
}
