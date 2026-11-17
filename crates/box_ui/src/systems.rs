//! The UI systems: input at the start of `Update`, layout and paint in
//! `PostUpdate` (after transform propagation, so world-anchored labels
//! follow this frame's transforms).

use box_assets::{FontAsset, JsonAsset};
use box_core::action::Actions;
use box_core::asset_ref::AssetRef;
use box_core::assets::Assets;
use box_core::identity::Uid;
use box_core::image::ImageData;
use box_core::input::Input;
use box_core::time::Time;
use box_core::transform::GlobalTransform;
use box_core::world::{Entity, World};
use box_render::{Camera, Overlay};
use epaint::Vec2;
use glam::Vec3;

use crate::layout::{LayoutInputs, scale_for};
use crate::strings::Strings;
use crate::style::Theme;
use crate::ui::{AnchorTarget, Ui, UiEvent};

/// Resource: the size of the target the UI draws into, in physical pixels,
/// and the window's scale factor. `box_engine` keeps it equal to its
/// `WindowSize` (the window, a capture, or the editor's game view).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiScreen {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
}

impl Default for UiScreen {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            scale_factor: 1.0,
        }
    }
}

/// Start of `Update`: pointer, focus navigation, activation; sends
/// [`UiEvent`]s and sets input capture. Idle cost without UI nodes: one
/// resource lookup.
pub fn ui_input_system(world: &mut World) {
    let Some(mut ui) = world.remove_resource::<Ui>() else {
        return;
    };
    if ui.roots().is_empty() && !ui.nav.is_capturing() && ui.focused.is_none() {
        world.insert_resource(ui);
        return;
    }
    let dt = world
        .get_resource::<Time>()
        .map_or(1.0 / 60.0, |t| t.unscaled_delta);
    world.resource_or_default::<Input>();
    world.resource_or_default::<Actions>();
    let (input, actions) = world.resources_mut::<(Input, Actions)>();
    ui.process_input(input, actions, dt);
    let events = std::mem::take(&mut ui.events);
    world.insert_resource(ui);
    for e in events {
        world.send::<UiEvent>(e);
    }
}

/// `PostUpdate`: theme, fonts, images and strings; world anchors; then
/// layout and paint into the [`Overlay`] when anything changed. Idle cost
/// (nothing changed, no world labels): a handful of compares.
pub fn ui_layout_system(world: &mut World) {
    let Some(mut ui) = world.remove_resource::<Ui>() else {
        return;
    };
    // A modal shown or hidden by game code during Update captures (or
    // frees) game input from the next frame's first fixed step on.
    if (!ui.roots().is_empty() || ui.nav.is_capturing())
        && let Some(actions) = world.get_resource_mut::<Actions>()
    {
        ui.sync_capture(actions);
    }
    if ui.roots().is_empty() {
        if let Some(o) = world.get_resource_mut::<Overlay>() {
            o.clear();
        }
        world.insert_resource(ui);
        return;
    }
    sync_theme(&mut ui, world);
    sync_fonts(&mut ui, world);
    sync_images(&mut ui, world);
    let screen = world
        .get_resource::<UiScreen>()
        .copied()
        .unwrap_or_default();
    let strings_rev = world.get_resource::<Strings>().map_or(0, |s| s.revision());
    let images_gen = world
        .get_resource::<Assets<ImageData>>()
        .map_or(0, |a| a.generation());
    let screen_key = (
        screen.width,
        screen.height,
        (screen.scale_factor * 1000.0).round() as u32,
    );
    if ui.seen.screen != screen_key
        || ui.seen.strings != strings_rev
        || ui.seen.images != images_gen
    {
        ui.seen.screen = screen_key;
        ui.seen.strings = strings_rev;
        ui.seen.images = images_gen;
        ui.dirty = true;
    }
    let (scale, size) = scale_for(
        ui.scale_mode,
        Vec2::new(screen.width.max(1) as f32, screen.height.max(1) as f32),
        screen.scale_factor,
    );
    if scale != ui.scale || size != ui.screen {
        ui.scale = scale;
        ui.screen = size;
        ui.dirty = true;
    }
    project_anchors(&mut ui, world, screen);
    if ui.dirty {
        ui.dirty = false;
        {
            let inputs = LayoutInputs {
                strings: world.get_resource::<Strings>(),
                images: world.get_resource::<Assets<ImageData>>(),
            };
            ui.layout(&inputs);
        }
        let overlay = world.resource_or_default::<Overlay>();
        ui.paint(overlay);
    }
    world.insert_resource(ui);
}

/// Requests the UI's assets now (theme, its fonts once the theme is
/// loaded, images, string tables), so a loading screen or a headless run
/// can wait for them (`box_engine::settle_assets`) before the first frame.
/// The systems do the same lazily.
pub fn request_ui_assets(world: &mut World) {
    crate::strings::update_strings(world);
    let Some(mut ui) = world.remove_resource::<Ui>() else {
        return;
    };
    sync_theme(&mut ui, world);
    sync_fonts(&mut ui, world);
    sync_images(&mut ui, world);
    world.insert_resource(ui);
}

/// Loads the theme file and applies it when its content changes.
fn sync_theme(ui: &mut Ui, world: &mut World) {
    let Some(path) = ui.theme_path.clone() else {
        return;
    };
    let Some(server) = box_assets::loader(world) else {
        ui.warn_once(format!("theme {path}: no asset server in this world"));
        return;
    };
    let handle = match ui.theme_loaded {
        Some((h, _)) => h,
        None => match server.load::<JsonAsset>(&AssetRef::path(&path)) {
            Ok(h) => {
                ui.theme_loaded = Some((h, u64::MAX));
                h
            }
            Err(e) => {
                ui.warn_once(format!("theme {path}: {e}"));
                return;
            }
        },
    };
    let Some(assets) = world.get_resource::<Assets<JsonAsset>>() else {
        return;
    };
    let (Some(json), Some(version)) = (assets.get(handle), assets.version(handle)) else {
        return;
    };
    if ui.theme_loaded.is_some_and(|(_, v)| v == version) {
        return;
    }
    ui.theme_loaded = Some((handle, version));
    match serde_json::from_value::<Theme>(json.0.clone()) {
        Ok(theme) => {
            ui.rebuild_theme(Some(&theme));
            tracing::info!(target: "box_ui", "theme {path} applied");
        }
        Err(e) => tracing::warn!(
            target: "box_ui",
            "theme {path} does not parse ({e}); keeping the last good theme"
        ),
    }
}

/// Loads the theme's font files and rebuilds the font set when they change.
fn sync_fonts(ui: &mut Ui, world: &mut World) {
    // Nothing to do unless the theme's fonts or the font store changed.
    let store_gen = world
        .get_resource::<Assets<FontAsset>>()
        .map_or(0, |a| a.generation());
    let key = (store_gen, ui.theme_revision);
    if ui.seen_fonts_key == Some(key) {
        return;
    }
    ui.seen_fonts_key = Some(key);
    let wanted: Vec<(String, String)> = ui
        .theme
        .fonts
        .iter()
        .map(|(r, p)| (r.clone(), p.clone()))
        .collect();
    ui.font_handles
        .retain(|role, (path, _)| wanted.iter().any(|(r, p)| r == role && p == path));
    if !wanted.is_empty()
        && let Some(server) = box_assets::loader(world)
    {
        for (role, path) in &wanted {
            if ui.font_handles.contains_key(role) {
                continue;
            }
            match server.load::<FontAsset>(&AssetRef::path(path)) {
                Ok(h) => {
                    ui.font_handles.insert(role.clone(), (path.clone(), h));
                }
                Err(e) => ui.warn_once(format!("font {path} (role `{role}`): {e}")),
            }
        }
    }
    let mut assets = std::collections::BTreeMap::new();
    if let Some(store) = world.get_resource::<Assets<FontAsset>>() {
        for (role, (_, h)) in &ui.font_handles {
            if let Some(f) = store.get(*h) {
                assets.insert(role.clone(), f.clone());
            }
        }
    }
    let changed = assets.len() != ui.painter.font_assets.len()
        || assets.iter().any(|(r, f)| {
            ui.painter
                .font_assets
                .get(r)
                .is_none_or(|old| !std::sync::Arc::ptr_eq(&old.bytes, &f.bytes))
        });
    if changed {
        ui.painter.font_assets = assets;
        ui.dirty = true;
    }
    ui.painter.ensure_fonts(&ui.theme);
}

/// Requests image assets of image nodes.
fn sync_images(ui: &mut Ui, world: &mut World) {
    if !std::mem::take(&mut ui.images_pending) {
        return;
    }
    let pending: Vec<(crate::NodeId, String)> = ui
        .all_nodes()
        .filter_map(|(id, n)| {
            (n.image_handle.is_none())
                .then(|| n.image.clone().map(|p| (id, p)))
                .flatten()
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let Some(server) = box_assets::loader(world) else {
        ui.warn_once("image nodes need an asset server in the world".into());
        return;
    };
    for (id, path) in pending {
        match server.load::<ImageData>(&AssetRef::path(&path)) {
            Ok(h) => {
                if let Some(n) = ui.node_mut(id) {
                    n.image_handle = Some(h);
                }
                ui.dirty = true;
            }
            Err(e) => {
                ui.warn_once(format!("image {path}: {e}"));
                if let Some(n) = ui.node_mut(id) {
                    n.image = None;
                }
            }
        }
    }
}

/// Projects world-anchored roots through the first active camera; marks
/// layout dirty when one moved by more than a tenth of a pixel or appeared
/// or disappeared.
fn project_anchors(ui: &mut Ui, world: &World, screen: UiScreen) {
    let anchored: Vec<crate::NodeId> = ui
        .roots()
        .iter()
        .copied()
        .filter(|r| ui.node(*r).is_some_and(|n| n.world.is_some()))
        .collect();
    if anchored.is_empty() {
        return;
    }
    let camera = world
        .query::<(&Camera, &GlobalTransform)>()
        .iter()
        .find(|(c, _)| c.active)
        .map(|(c, g)| (*c, *g));
    let (w, h) = (screen.width.max(1) as f32, screen.height.max(1) as f32);
    let scale = ui.scale;
    for id in anchored {
        let Some(anchor) = ui.node(id).and_then(|n| n.world.clone()) else {
            continue;
        };
        let target = match &anchor.target {
            AnchorTarget::Point(p) => Some(*p),
            AnchorTarget::Entity(e) => entity_position(world, *e),
            AnchorTarget::Named {
                name,
                cached,
                tried_at,
            } => {
                let count = world.entity_count();
                let live = cached.filter(|e| world.contains(*e));
                let e = match live {
                    Some(e) => Some(e),
                    None if *tried_at != Some(count) => resolve_named(world, name),
                    None => None,
                };
                if let Some(n) = ui.node_mut(id)
                    && let Some(a) = &mut n.world
                    && let AnchorTarget::Named {
                        cached, tried_at, ..
                    } = &mut a.target
                {
                    *cached = e;
                    *tried_at = Some(count);
                }
                e.and_then(|e| entity_position(world, e))
            }
        };
        let projected = match (target, camera) {
            (Some(p), Some((cam, g))) => {
                project(p + anchor.offset, &cam, &g, w, h, anchor.max_distance).map(|px| px / scale)
            }
            _ => None,
        };
        let changed = match (anchor.screen, projected) {
            (Some(a), Some(b)) => (a - b).length() * scale > 0.1,
            (None, None) => false,
            _ => true,
        };
        if changed {
            if let Some(n) = ui.node_mut(id)
                && let Some(a) = &mut n.world
            {
                a.screen = projected;
            }
            ui.dirty = true;
        }
    }
}

fn entity_position(world: &World, e: Entity) -> Option<Vec3> {
    world.get::<GlobalTransform>(e).map(|g| g.translation())
}

fn resolve_named(world: &World, name: &str) -> Option<Entity> {
    if let Some(uid) = Uid::parse(name)
        && let Some(e) = world.entity_by_uid(uid)
    {
        return Some(e);
    }
    world.find_by_name(name)
}

/// A world point in target pixels, or `None` behind the camera or beyond
/// `max_distance`.
pub(crate) fn project(
    p: Vec3,
    cam: &Camera,
    g: &GlobalTransform,
    w: f32,
    h: f32,
    max_distance: Option<f32>,
) -> Option<Vec2> {
    let v = g.affine().inverse().transform_point3(p);
    let depth = -v.z;
    if depth <= cam.near.max(1e-4) {
        return None;
    }
    if max_distance.is_some_and(|d| v.length() > d) {
        return None;
    }
    let f = 1.0 / (cam.fov_y * 0.5).tan();
    let aspect = w / h;
    let ndc_x = f / aspect * v.x / depth;
    let ndc_y = f * v.y / depth;
    Some(Vec2::new((ndc_x * 0.5 + 0.5) * w, (0.5 - ndc_y * 0.5) * h))
}
