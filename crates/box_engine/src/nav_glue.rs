//! Navigation in apps (docs/NAVIGATION.md, D21; `navigation` feature): the
//! navmesh of a world, baked automatically from its static colliders.
//!
//! - [`NavSettings`] (scene resource, optional): the agent size and the
//!   sampling; without it the defaults match `KinematicCharacter`'s.
//! - [`Navigation`] (runtime resource): the [`NavMesh`] and the bake state.
//!   [`update_navigation`] (PostUpdate, after `sync_physics`) stamps the
//!   tiles touched by every added, moved or removed static collider (a push
//!   per change) and flushes them at most every [`FLUSH_FRAMES`] frames:
//!   each dirty tile's triangles are gathered on the main thread and baked
//!   on a worker thread; finished tiles are swapped in at the start of a
//!   later frame. Cost per frame without changes: one empty-list check.
//! - Queries: [`nav_path`], [`nav_closest`], [`nav_reachable`],
//!   [`nav_random_near`].
//!
//! Baked: colliders that are not triggers, not characters, not on a
//! moving body (a non-`Fixed` `RigidBody`) and not on a `Mover`. Moving
//! things are obstacles agents bump into, not holes in the mesh.

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{Receiver, Sender, channel};

use box_core::reflect::{Range, Reflect, ReflectDefault, ReflectResource, Step, Unit};
use box_core::world::{Entity, World};
use box_nav::{NavConfig, NavMesh, NavPath, NavTile, TileCoord, bake_tile};
use box_physics::{Collider, KinematicCharacter, Physics};
use glam::Vec3;

pub use box_nav;

/// Frames between two flushes of dirty tiles (a quarter second at 60 fps),
/// so a drag or a burst of changes bakes each tile once.
pub const FLUSH_FRAMES: u32 = 15;

/// How characters that are not the player find their way: the size of the
/// agent the walkable area is computed for and how finely it is sampled. A
/// scene resource (Scene Settings); without it the defaults apply, which
/// match the default `KinematicCharacter`.
#[derive(Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct NavSettings {
    /// Off: no navmesh; enemies walk straight at their target.
    pub enabled: bool,
    /// Agents keep this far from walls.
    #[reflect(@Range { min: 0.05, max: 5.0 }, @Unit("m"), @Step(0.05))]
    pub agent_radius: f32,
    /// Ceilings lower than this block.
    #[reflect(@Range { min: 0.1, max: 10.0 }, @Unit("m"), @Step(0.05))]
    pub agent_height: f32,
    /// Steepest walkable slope.
    #[reflect(@Range { min: 0.0, max: 1.5 }, @Unit("rad"), @Step(0.01))]
    pub max_slope: f32,
    /// Highest step agents climb (stairs, kerbs).
    #[reflect(@Range { min: 0.0, max: 2.0 }, @Unit("m"), @Step(0.01))]
    pub step_height: f32,
    /// Sampling size: smaller follows walls more closely and bakes slower.
    #[reflect(@Range { min: 0.05, max: 1.0 }, @Unit("m"), @Step(0.01))]
    pub cell_size: f32,
    /// Edge of a tile; a change rebuilds the tiles it touches.
    #[reflect(@Range { min: 4.0, max: 256.0 }, @Unit("m"), @Step(1.0))]
    pub tile_size: f32,
}

impl Default for NavSettings {
    fn default() -> Self {
        let c = NavConfig::default();
        Self {
            enabled: true,
            agent_radius: c.agent_radius,
            agent_height: c.agent_height,
            max_slope: c.max_slope,
            step_height: c.step_height,
            cell_size: c.cell_size,
            tile_size: c.tile_size,
        }
    }
}

impl NavSettings {
    /// The bake config these settings describe.
    pub fn config(&self) -> NavConfig {
        NavConfig {
            agent_radius: self.agent_radius,
            agent_height: self.agent_height,
            max_slope: self.max_slope,
            step_height: self.step_height,
            cell_size: self.cell_size,
            tile_size: self.tile_size,
        }
    }
}

/// Registers [`NavSettings`].
pub fn register_navigation_types(registry: &mut box_core::reflect::Registry) {
    if let Err(e) = registry.register_resource::<NavSettings>() {
        tracing::error!("navigation type registration failed: {e}");
    }
    if let Err(e) = registry.register_yielding::<crate::nav_agent::NavAgent>() {
        tracing::error!("navigation type registration failed: {e}");
    }
    if let Err(e) = registry.register_yielding::<crate::behaviours::Chaser>() {
        tracing::error!("navigation type registration failed: {e}");
    }
    crate::behaviours::presets::register_navigation_presets(registry);
}

/// A finished bake: the tile, or none when nothing there is walkable.
struct Baked {
    coord: TileCoord,
    generation: u64,
    tile: Option<NavTile>,
}

/// The world's navmesh and its bake state (runtime; never saved).
pub struct Navigation {
    mesh: NavMesh,
    enabled: bool,
    /// Tiles waiting for the next flush.
    dirty: BTreeSet<TileCoord>,
    /// Bakes running, by tile, with the generation they started at.
    running: HashMap<TileCoord, u64>,
    /// Bumped by every flush; a result older than the tile's latest start
    /// is dropped.
    generation: u64,
    frames_since_flush: u32,
    sender: Sender<Baked>,
    receiver: Receiver<Baked>,
    /// Bake on the calling thread (headless tests, deterministic replays):
    /// tiles are ready in the frame they are flushed, and flushes are not
    /// throttled.
    pub inline: bool,
    /// Tiles baked so far, for tests and reports.
    pub tiles_baked: u64,
    /// Whether every collider was scanned once (a resource made after the
    /// colliders were synced missed their change regions).
    scanned: bool,
}

impl Default for Navigation {
    fn default() -> Self {
        let (sender, receiver) = channel();
        Self {
            mesh: NavMesh::new(NavConfig::default()),
            enabled: true,
            dirty: BTreeSet::new(),
            running: HashMap::new(),
            generation: 0,
            frames_since_flush: FLUSH_FRAMES,
            sender,
            receiver,
            inline: false,
            tiles_baked: 0,
            scanned: false,
        }
    }
}

impl Navigation {
    /// Bakes inline (see [`inline`](Self::inline)).
    pub fn inline() -> Self {
        Self {
            inline: true,
            ..Self::default()
        }
    }

    pub fn mesh(&self) -> &NavMesh {
        &self.mesh
    }

    /// True while tiles are waiting or baking: paths may change soon.
    pub fn is_baking(&self) -> bool {
        !self.dirty.is_empty() || !self.running.is_empty()
    }
}

/// Whether a collider belongs in the navmesh: solid, not a character, not
/// on a moving body or a `Mover`.
fn is_static(world: &World, e: Entity, c: &Collider) -> bool {
    if c.is_trigger || world.has::<KinematicCharacter>(e) {
        return false;
    }
    if world.has::<crate::behaviours::Mover>(e) {
        return false;
    }
    #[cfg(feature = "dynamics")]
    if let Some(b) = world.get::<box_physics::RigidBody>(e)
        && b.kind != box_physics::BodyKind::Fixed
    {
        return false;
    }
    true
}

/// The `navigation` system (PostUpdate, after `sync_physics`).
pub fn update_navigation(world: &mut World) {
    if !world.contains_resource::<Physics>() {
        return;
    }
    let settings = world
        .get_resource::<NavSettings>()
        .cloned()
        .unwrap_or_default();
    let regions = world
        .get_resource_mut::<Physics>()
        .map(Physics::take_changed_regions)
        .unwrap_or_default();
    world.resource_or_default::<Navigation>();
    world.resource_scope::<Navigation, _>(|world, nav| {
        // Settings changed (or switched on/off): start again from every
        // collider.
        let config = settings.config();
        if !settings.enabled {
            if nav.enabled || !nav.mesh.is_empty() {
                nav.mesh.clear();
                nav.dirty.clear();
                nav.running.clear();
                nav.enabled = false;
            }
            return;
        }
        if !nav.enabled || !nav.scanned || *nav.mesh.config() != config {
            nav.scanned = true;
            nav.enabled = true;
            nav.mesh = NavMesh::new(config);
            nav.running.clear();
            nav.dirty.clear();
            if let Some(physics) = world.get_resource::<Physics>() {
                let mut all = Vec::new();
                physics.triangles_in(
                    Vec3::splat(f32::NEG_INFINITY),
                    Vec3::splat(f32::INFINITY),
                    |e, c| is_static(world, e, c),
                    &mut all,
                );
                let (lo, hi) = bounds(&all);
                if lo.is_finite() && hi.is_finite() {
                    nav.dirty.extend(config.tiles_in(lo, hi));
                }
            }
        }
        let margin = Vec3::new(config.tile_margin(), 0.0, config.tile_margin());
        for r in regions {
            // Moving things (characters, movers, bodies) are not baked, so
            // their moves change nothing. A removed collider's entity may be
            // gone: its old place is rebuilt.
            let skip = r.entity.is_some_and(|e| {
                world.contains(e)
                    && world
                        .get::<Collider>(e)
                        .is_some_and(|c| !is_static(world, e, &c))
            });
            if !skip && r.min.is_finite() && r.max.is_finite() {
                nav.dirty
                    .extend(config.tiles_in(r.min - margin, r.max + margin));
            }
        }
        // Finished bakes.
        while let Ok(b) = nav.receiver.try_recv() {
            if nav.running.get(&b.coord) == Some(&b.generation) {
                nav.running.remove(&b.coord);
                nav.mesh.set_tile(b.coord, b.tile);
                nav.tiles_baked += 1;
            }
        }
        nav.frames_since_flush = nav.frames_since_flush.saturating_add(1);
        if nav.dirty.is_empty() || (!nav.inline && nav.frames_since_flush < FLUSH_FRAMES) {
            return;
        }
        nav.frames_since_flush = 0;
        flush(world, nav, &config);
    });
}

/// Gathers each dirty tile's triangles and bakes it (inline, or on one
/// worker thread for the batch).
fn flush(world: &World, nav: &mut Navigation, config: &box_nav::NavConfig) {
    let Some(physics) = world.get_resource::<Physics>() else {
        return;
    };
    nav.generation += 1;
    let generation = nav.generation;
    let tiles: Vec<TileCoord> = std::mem::take(&mut nav.dirty).into_iter().collect();
    let mut jobs = Vec::with_capacity(tiles.len());
    for coord in tiles {
        let (min, max) = config.tile_rect(coord);
        let m = config.tile_margin();
        let mut tris = Vec::new();
        physics.triangles_in(
            Vec3::new(min.x - m, f32::NEG_INFINITY, min.y - m),
            Vec3::new(max.x + m, f32::INFINITY, max.y + m),
            |e, c| is_static(world, e, c),
            &mut tris,
        );
        nav.running.insert(coord, generation);
        jobs.push((coord, tris));
    }
    let config = *config;
    let bake = move |coord: TileCoord, tris: Vec<[Vec3; 3]>| {
        let tile = match bake_tile(&tris, coord, &config) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(target: "box_engine", "navigation: {e}");
                None
            }
        };
        Baked {
            coord,
            generation,
            tile,
        }
    };
    if nav.inline {
        for (coord, tris) in jobs {
            let b = bake(coord, tris);
            nav.running.remove(&b.coord);
            nav.mesh.set_tile(b.coord, b.tile);
            nav.tiles_baked += 1;
        }
        return;
    }
    let sender = nav.sender.clone();
    let spawned = std::thread::Builder::new()
        .name("navmesh bake".into())
        .spawn(move || {
            for (coord, tris) in jobs {
                let b = box_core::panic_guard::catch(|| bake(coord, tris));
                if let Ok(b) = b
                    && sender.send(b).is_err()
                {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(target: "box_engine", "navigation: cannot start a bake thread: {e}");
    }
}

fn bounds(tris: &[[Vec3; 3]]) -> (Vec3, Vec3) {
    tris.iter().flatten().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(lo, hi), v| (lo.min(*v), hi.max(*v)),
    )
}

/// Waits for running bakes and bakes what is dirty, so the navmesh matches
/// the colliders now (headless tools, `box nav`, tests). Runs
/// [`update_navigation`] first.
pub fn settle_navigation(world: &mut World) {
    let inline = world.get_resource::<Navigation>().is_some_and(|n| n.inline);
    if let Some(n) = world.get_resource_mut::<Navigation>() {
        n.inline = true;
    }
    update_navigation(world);
    // Results of bakes started on worker threads before.
    for _ in 0..2000 {
        let busy = world
            .get_resource::<Navigation>()
            .is_some_and(|n| !n.running.is_empty());
        if !busy {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        update_navigation(world);
    }
    if let Some(n) = world.get_resource_mut::<Navigation>() {
        n.inline = inline;
    }
}

/// The world's navmesh, if it has one.
pub fn nav_mesh(world: &World) -> Option<&NavMesh> {
    world.get_resource::<Navigation>().map(Navigation::mesh)
}

/// The path from `from` to `to` (feet positions), or `None` without a
/// navmesh.
pub fn nav_path(world: &World, from: Vec3, to: Vec3) -> Option<NavPath> {
    Some(nav_mesh(world)?.find_path(from, to))
}

/// The nearest point on the navmesh.
pub fn nav_closest(world: &World, p: Vec3) -> Option<Vec3> {
    nav_mesh(world)?.closest_point(p)
}

/// Whether `to` can be reached from `from` along the navmesh.
pub fn nav_reachable(world: &World, from: Vec3, to: Vec3) -> bool {
    nav_path(world, from, to).is_some_and(|p| p.is_complete())
}

/// A random point on the navmesh within `radius` of `center`, reachable
/// from it (the same `seed`, the same point).
pub fn nav_random_near(world: &World, center: Vec3, radius: f32, seed: u64) -> Option<Vec3> {
    nav_mesh(world)?.random_point_near(center, radius, seed)
}

/// For an editor's edit world (View ▸ Navigation) and tools that do not
/// play: syncs the colliders quietly ([`box_physics::sync_physics_quiet`])
/// and updates the navmesh as Play does (dirty tiles, throttled, baked on a
/// worker). Run it only while the navmesh is wanted: it costs nothing more
/// than a collider sync when nothing changed.
pub fn update_edit_world(world: &mut World) {
    world.resource_or_default::<Navigation>();
    box_physics::sync_physics_quiet(world);
    update_navigation(world);
}

/// Colours of [`draw_navmesh`]: walls and drops, and the edges between
/// polygons, linear RGBA.
pub const NAV_EDGE_COLOR: [f32; 4] = [0.0, 0.85, 0.75, 0.95];
pub const NAV_INNER_COLOR: [f32; 4] = [0.0, 0.85, 0.75, 0.3];

/// Draws the navmesh into `dd` on the post-tonemap layer (colours do not
/// depend on exposure), 5 cm above the walkable surface: the outline of
/// every walkable area (walls, drops) bright, the edges between polygons
/// faint. Cost: two lines per polygon edge, only while called.
pub fn draw_navmesh(mesh: &NavMesh, dd: &mut box_render::DebugDraw) {
    use box_core::color::Color;
    use box_nav::NavEdge;
    use box_render::LineLayer;
    let [r, g, b, a] = NAV_EDGE_COLOR;
    let edge = Color::linear_rgba(r, g, b, a);
    let [r, g, b, a] = NAV_INNER_COLOR;
    let inner = Color::linear_rgba(r, g, b, a);
    let lift = Vec3::Y * 0.05;
    for tile in mesh.tiles() {
        for (i, poly) in tile.polys.iter().enumerate() {
            let n = poly.verts.len();
            for j in 0..n {
                let p = tile.vertices[poly.verts[j] as usize] + lift;
                let q = tile.vertices[poly.verts[(j + 1) % n] as usize] + lift;
                let c = match poly.edges[j] {
                    NavEdge::Wall => edge,
                    // Inner edges are shared by two polygons: draw once.
                    NavEdge::Inner(k) if (k as usize) < i => continue,
                    NavEdge::Inner(_) | NavEdge::Portal(_) => inner,
                };
                dd.line_on(LineLayer::Display, p, q, c);
            }
        }
    }
}

/// Adds the navigation system and resource.
pub struct NavigationPlugin;

impl box_core::app::Plugin for NavigationPlugin {
    fn build(&self, app: &mut box_core::app::App) {
        app.world.resource_or_default::<Navigation>();
        // Added after `PhysicsPlugin`, so it runs after `sync_physics`.
        app.system(box_core::app::Stage::PostUpdate, update_navigation)
            .label("navigation");
        app.on_add::<crate::nav_agent::NavAgent>(crate::nav_agent::on_agent_added);
        // Agents set their velocity like the player's input does: after
        // movers and the player, before the characters move.
        app.system(
            box_core::app::Stage::FixedUpdate,
            crate::nav_agent::drive_nav_agents,
        )
        .label("nav_agents")
        .after("player_input")
        .before("move_characters");
    }
}
