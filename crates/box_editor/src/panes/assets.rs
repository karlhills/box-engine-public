//! Assets pane: the project's `assets/` folder as a folder tree, a
//! breadcrumb and a grid of tiles with thumbnails (`crate::assets` holds the
//! state and operations).
//!
//! - Click selects (the inspector shows import settings or the material),
//!   double-click opens: a folder, a `.scene.json` (as the scene, through
//!   the unsaved-changes prompt), or a glTF's sub-assets (also its chevron).
//! - F2, double-click on the name or the context menu renames inline (the
//!   suffix is kept); dragging a file onto a folder (tile or tree row) moves
//!   it. Both go through `AssetServer::rename` (file and `.meta` together).
//! - Dragging a model, mesh sub-asset or scene onto the viewport places it,
//!   a material onto an entity (viewport or hierarchy) assigns it, an asset
//!   onto a matching inspector reference sets it.
//! - Files dropped from the operating system are copied into the folder
//!   shown.
//!
//! The browser imports the files it shows (once each), so they get a state
//! badge (loading, failed with the error on hover), sub-assets and, for
//! images, a thumbnail: a downscaled copy of the imported `ImageData` made
//! once per asset version (at most [`THUMBS_PER_FRAME`] per frame) and kept
//! as an egui texture. Models, materials and scenes show a type icon.

use std::collections::HashMap;

use box_assets::{AssetServer, LoadState};
use box_core::asset_ref::{AssetId, AssetRef};
use box_core::assets::Assets;
use box_core::image::ImageData;
use box_core::world::World;
use egui::{
    Align2, Color32, ColorImage, CornerRadius, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureId, Ui, pos2, vec2,
};

use crate::assets::{
    AssetDrag, AssetItem, FileKind, RefType, file_name, parent_folder, split_name,
};
use crate::state::EditorState;
use crate::theme::{Tokens, tokens};
use crate::widgets;

/// Thumbnail size in pixels (the longer side).
pub const THUMB_PX: u32 = 128;
/// New thumbnails made per frame at most, so opening a big folder never
/// stalls a frame.
pub const THUMBS_PER_FRAME: u32 = 8;

const TILE_W: f32 = 92.0;
const TILE_H: f32 = 100.0;
const ICON: f32 = 60.0;
const TREE_W: f32 = 150.0;

struct Thumb {
    handle: u64,
    version: u64,
    texture: TextureHandle,
}

/// Image thumbnails as egui textures, keyed by asset and regenerated only
/// when the image's `Assets` version changes.
#[derive(Default)]
pub(crate) struct Thumbnails {
    map: HashMap<(AssetId, Option<String>), Thumb>,
    frame: u64,
    made: u32,
}

impl Thumbnails {
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// The thumbnail of an image asset (file or glTF texture), once it is
    /// imported. Stale thumbnails are shown until the budget allows a new one.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        world: &World,
        server: &AssetServer,
        item: &AssetItem,
    ) -> Option<TextureId> {
        let frame = ctx.cumulative_frame_nr();
        if frame != self.frame {
            self.frame = frame;
            self.made = 0;
        }
        let mut r = AssetRef::id(item.id);
        r.label = item.label.clone();
        let handle = server.get_handle::<ImageData>(&r)?;
        let images = world.get_resource::<Assets<ImageData>>()?;
        let version = images.version(handle)?;
        let key = (item.id, item.label.clone());
        if let Some(t) = self.map.get(&key)
            && ((t.handle == handle.id() && t.version == version) || self.made >= THUMBS_PER_FRAME)
        {
            return Some(t.texture.id());
        }
        if self.made >= THUMBS_PER_FRAME {
            ctx.request_repaint();
            return None;
        }
        let image = images.get(handle)?;
        self.made += 1;
        let texture = ctx.load_texture(
            format!("thumb-{}-{}", item.id, item.label.as_deref().unwrap_or("")),
            thumbnail_image(image, THUMB_PX),
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        self.map.insert(
            key,
            Thumb {
                handle: handle.id(),
                version,
                texture,
            },
        );
        Some(id)
    }
}

/// A downscaled copy of `image` whose longer side is at most `max` pixels:
/// the smallest mip level at least that large, box-filtered further when the
/// image has no mips.
pub fn thumbnail_image(image: &ImageData, max: u32) -> ColorImage {
    let mut level = 0;
    for l in 0..image.mips.len() {
        let (w, h) = image.level_size(l);
        if w.max(h) >= max {
            level = l;
        } else {
            break;
        }
    }
    let (w, h) = image.level_size(level);
    let src = &image.mips[level];
    let f = w.max(h).div_ceil(max).max(1);
    let (tw, th) = ((w / f).max(1), (h / f).max(1));
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for y in 0..th {
        for x in 0..tw {
            let mut sum = [0u32; 4];
            let mut n = 0;
            for sy in y * f..((y + 1) * f).min(h) {
                for sx in x * f..((x + 1) * f).min(w) {
                    let i = ((sy * w + sx) * 4) as usize;
                    if let Some(px) = src.get(i..i + 4) {
                        for c in 0..4 {
                            sum[c] += px[c] as u32;
                        }
                        n += 1;
                    }
                }
            }
            let n = n.max(1);
            out.extend(sum.map(|s| (s / n) as u8));
        }
    }
    ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &out)
}

/// The thumbnail of `item` (images only), through the editor state.
pub(crate) fn thumbnail(
    state: &mut EditorState,
    ctx: &egui::Context,
    item: &AssetItem,
) -> Option<TextureId> {
    let server = state.asset_server()?.clone();
    let mut thumbs = std::mem::take(&mut state.assets.thumbs);
    let id = thumbs.get(ctx, state.edit_world(), &server, item);
    state.assets.thumbs = thumbs;
    id
}

/// A file or sub-asset's state for its badge.
fn item_state(server: &AssetServer, item: &AssetItem) -> Option<LoadState> {
    match &item.label {
        None => server.file_state(item.id),
        Some(_) => {
            let mut r = AssetRef::id(item.id);
            r.label = item.label.clone();
            server.state(&r)
        }
    }
}

enum Tile {
    Folder(String),
    File {
        id: AssetId,
        path: String,
    },
    Sub {
        item: AssetItem,
        type_name: &'static str,
    },
    Broken {
        path: String,
        message: String,
    },
}

enum Action {
    SetFolder(String),
    Select(Option<AssetItem>),
    Open(AssetItem),
    StartRename(AssetId),
    Rename(AssetId, String),
    CancelRename,
    MoveTo(AssetId, String),
    NewFolder,
    NewMaterial,
    NewScene,
    /// Ask to move the file to the project's trash.
    Trash(AssetId),
    Reimport(AssetId),
    Drag(AssetDrag),
}

pub fn assets_ui(ui: &mut Ui, state: &mut EditorState) {
    let t = tokens(ui.ctx());
    let pane = ui.max_rect();
    if state.asset_server().is_none() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No project open").color(t.text));
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Open or create a project (File menu) to browse its assets")
                    .font(t.small())
                    .color(t.text_weak),
            );
        });
        return;
    }
    // The browser owns F2 and Escape after a click in it.
    let press = ui.input(|i| {
        i.pointer
            .any_pressed()
            .then(|| i.pointer.interact_pos())
            .flatten()
    });
    if let Some(p) = press {
        state.assets.active = pane.contains(p);
    }
    state.refresh_listing();

    let mut actions = Vec::new();
    top_bar(ui, state, &t, &mut actions);
    let body = ui.available_rect_before_wrap();
    let tree = Rect::from_min_max(body.min, pos2(body.left() + TREE_W, body.bottom()));
    let grid = Rect::from_min_max(pos2(tree.right() + 1.0, body.top()), body.max);
    ui.painter().vline(
        tree.right() + 0.5,
        body.y_range(),
        Stroke::new(1.0, t.border),
    );
    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(tree));
    folder_tree(&mut tui, state, &t, &mut actions);
    let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(grid));
    tile_grid(&mut gui, state, &t, &mut actions);

    if state.assets.active
        && state.assets.renaming.is_none()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::F2))
        && let Some(AssetItem { id, label: None }) = state.assets.selected.clone()
    {
        actions.push(Action::StartRename(id));
    }
    apply(state, actions);
}

fn apply(state: &mut EditorState, actions: Vec<Action>) {
    for a in actions {
        let result = match a {
            Action::SetFolder(f) => {
                state.assets.folder = f;
                state.assets.filter.clear();
                Ok(())
            }
            Action::Select(item) => {
                state.select_asset(item);
                Ok(())
            }
            Action::Open(item) => {
                state.open_asset(&item);
                Ok(())
            }
            Action::StartRename(id) => {
                let stem = state
                    .asset_server()
                    .and_then(|s| s.path_of(id))
                    .map(|p| split_name(file_name(&p)).0.to_string());
                state.assets.renaming = stem.map(|s| (id, s));
                Ok(())
            }
            Action::Rename(id, stem) => {
                state.assets.renaming = None;
                state.rename_asset_stem(id, &stem)
            }
            Action::CancelRename => {
                state.assets.renaming = None;
                Ok(())
            }
            Action::MoveTo(id, folder) => state.move_asset_to_folder(id, &folder),
            Action::NewFolder => {
                let parent = state.assets.folder.clone();
                state.new_asset_folder(&parent).map(|f| {
                    state.assets.folder = f;
                })
            }
            Action::NewMaterial => {
                let folder = state.assets.folder.clone();
                state.new_material_asset(&folder).map(|_| ())
            }
            Action::NewScene => {
                let folder = state.assets.folder.clone();
                state.new_scene_asset(&folder).map(|_| ())
            }
            Action::Trash(id) => {
                state.requests.delete_asset = Some(id);
                Ok(())
            }
            Action::Reimport(id) => match state.asset_server() {
                Some(s) => s.reimport(id).map_err(Into::into),
                None => Ok(()),
            },
            Action::Drag(d) => {
                state.assets.drag = Some(d);
                Ok(())
            }
        };
        if let Err(e) = result {
            tracing::error!("assets: {e:#}");
        }
    }
}

fn top_bar(ui: &mut Ui, state: &mut EditorState, t: &Tokens, actions: &mut Vec<Action>) {
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    ui.painter().hline(
        row.x_range(),
        row.bottom() - 0.5,
        Stroke::new(1.0, t.border),
    );
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row.shrink2(vec2(8.0, 5.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 2.0;
    // Breadcrumb: assets › models › props.
    let folder = state.assets.folder.clone();
    let mut crumbs = vec![(String::new(), "assets".to_string())];
    let mut acc = String::new();
    for part in folder.split('/').filter(|p| !p.is_empty()) {
        acc = if acc.is_empty() {
            part.to_string()
        } else {
            format!("{acc}/{part}")
        };
        crumbs.push((acc.clone(), part.to_string()));
    }
    let last = crumbs.len() - 1;
    for (i, (path, name)) in crumbs.into_iter().enumerate() {
        if i > 0 {
            bar.label(egui::RichText::new("›").color(t.text_faint));
        }
        let r = widgets::text_button(&mut bar, &name, 22.0, i == last);
        if r.clicked() && i != last {
            actions.push(Action::SetFolder(path.clone()));
        }
        drop_onto_folder(&bar, state, r.rect, &path, t, actions);
    }
    // Right side: New menu and search.
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        egui::containers::menu::MenuButton::from_button(
            egui::Button::new("New").min_size(vec2(44.0, 22.0)),
        )
        .ui(ui, |ui| {
            if ui.button("Folder").clicked() {
                actions.push(Action::NewFolder);
            }
            if ui.button("Material").clicked() {
                actions.push(Action::NewMaterial);
            }
            if ui.button("Scene").clicked() {
                actions.push(Action::NewScene);
            }
        });
        ui.add_space(6.0);
        let w = (ui.available_width() - 8.0).clamp(60.0, 220.0);
        ui.add(
            egui::TextEdit::singleline(&mut state.assets.filter)
                .hint_text("Search assets")
                .font(t.body())
                .desired_width(w),
        );
    });
}

/// A drop target for files dragged onto a folder: highlights while hovered
/// and moves the file on release.
fn drop_onto_folder(
    ui: &Ui,
    state: &EditorState,
    rect: Rect,
    folder: &str,
    t: &Tokens,
    actions: &mut Vec<Action>,
) {
    let Some(drag) = &state.assets.drag else {
        return;
    };
    if drag.item.label.is_some() {
        return;
    }
    let (pos, released) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released()));
    if !pos.is_some_and(|p| rect.contains(p)) {
        return;
    }
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        CornerRadius::same(t.radius),
        Stroke::new(1.5, t.accent),
        StrokeKind::Inside,
    );
    if released {
        actions.push(Action::MoveTo(drag.item.id, folder.to_string()));
    }
}

fn folder_tree(ui: &mut Ui, state: &mut EditorState, t: &Tokens, actions: &mut Vec<Action>) {
    let folders: Vec<String> = state.assets.listing.folders.iter().cloned().collect();
    let rows: Vec<(String, String, usize)> =
        std::iter::once((String::new(), "assets".to_string(), 0))
            .chain(folders.into_iter().map(|f| {
                let depth = f.matches('/').count() + 1;
                let name = file_name(&f).to_string();
                (f, name, depth)
            }))
            .collect();
    egui::ScrollArea::vertical()
        .id_salt("asset_tree")
        .auto_shrink([false, false])
        .show_rows(ui, t.row_h, rows.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (path, name, depth) in &rows[range] {
                let w = ui.available_width();
                let (rect, resp) = ui.allocate_exact_size(vec2(w, t.row_h), Sense::click());
                let on = *path == state.assets.folder && state.assets.filter.is_empty();
                if on {
                    ui.painter().rect_filled(rect, 0.0, t.accent_bg);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 0.0, t.hover);
                }
                let x = rect.left() + 10.0 + *depth as f32 * 12.0;
                folder_glyph(
                    ui.painter(),
                    pos2(x + 6.0, rect.center().y),
                    6.0,
                    t.text_weak,
                );
                ui.painter().text(
                    pos2(x + 16.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    t.body(),
                    if on { t.accent_text } else { t.text },
                );
                if resp.clicked() {
                    actions.push(Action::SetFolder(path.clone()));
                }
                drop_onto_folder(ui, state, rect, path, t, actions);
            }
        });
}

/// The tiles for the current folder, or for every match of the filter.
fn tiles(state: &EditorState) -> Vec<Tile> {
    let l = &state.assets.listing;
    let filter = state.assets.filter.trim().to_lowercase();
    let folder = state.assets.folder.as_str();
    let mut out = Vec::new();
    let in_view = |path: &str| {
        if filter.is_empty() {
            parent_folder(path) == folder
        } else {
            file_name(path).to_lowercase().contains(&filter)
        }
    };
    if filter.is_empty() {
        for f in crate::assets::subfolders(&l.folders, folder) {
            out.push(Tile::Folder(f.to_string()));
        }
    }
    let server = state.asset_server();
    for (id, path) in &l.files {
        if !in_view(path) {
            continue;
        }
        out.push(Tile::File {
            id: *id,
            path: path.clone(),
        });
        if state.assets.expanded.contains(id)
            && let Some(server) = server
        {
            for sub in server.sub_assets(*id) {
                if let Some(label) = &sub.label {
                    out.push(Tile::Sub {
                        item: AssetItem::sub(*id, label),
                        type_name: sub.type_name,
                    });
                }
            }
        }
    }
    for (path, message) in &l.broken {
        if in_view(path) {
            out.push(Tile::Broken {
                path: path.clone(),
                message: message.clone(),
            });
        }
    }
    out
}

fn tile_grid(ui: &mut Ui, state: &mut EditorState, t: &Tokens, actions: &mut Vec<Action>) {
    let tiles = tiles(state);
    let area = ui.max_rect();
    // A click on empty space clears the asset selection.
    let bg = ui.interact(area, ui.scope_id().with("asset_grid_bg"), Sense::click());
    bg.context_menu(|ui| {
        if ui.button("New Folder").clicked() {
            actions.push(Action::NewFolder);
        }
        if ui.button("New Material").clicked() {
            actions.push(Action::NewMaterial);
        }
        if ui.button("New Scene").clicked() {
            actions.push(Action::NewScene);
        }
    });
    if bg.clicked() {
        actions.push(Action::Select(None));
    }
    if tiles.is_empty() {
        let msg = if state.assets.filter.trim().is_empty() {
            "Empty folder · drop files here to add them"
        } else {
            "No matching assets"
        };
        ui.painter().text(
            area.center(),
            Align2::CENTER_CENTER,
            msg,
            t.body(),
            t.text_weak,
        );
        return;
    }
    let cols = ((area.width() - 12.0) / TILE_W).floor().max(1.0) as usize;
    let rows = tiles.len().div_ceil(cols);
    egui::ScrollArea::vertical()
        .id_salt("asset_grid")
        .auto_shrink([false, false])
        .show_rows(ui, TILE_H, rows, |ui, range| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            for row in range {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), TILE_H), Sense::hover());
                for c in 0..cols {
                    let Some(tile) = tiles.get(row * cols + c) else {
                        break;
                    };
                    let r = Rect::from_min_size(
                        pos2(rect.left() + 6.0 + c as f32 * TILE_W, rect.top()),
                        vec2(TILE_W, TILE_H),
                    );
                    tile_ui(ui, state, tile, r, t, actions);
                }
            }
        });
}

fn tile_ui(
    ui: &mut Ui,
    state: &mut EditorState,
    tile: &Tile,
    rect: Rect,
    t: &Tokens,
    actions: &mut Vec<Action>,
) {
    let inner = rect.shrink(3.0);
    let (key, name): (String, String) = match tile {
        Tile::Folder(f) => (format!("d:{f}"), file_name(f).to_string()),
        Tile::File { path, .. } => (format!("f:{path}"), file_name(path).to_string()),
        Tile::Sub { item, .. } => (
            format!("s:{}#{}", item.id, item.label.as_deref().unwrap_or("")),
            item.label.clone().unwrap_or_default(),
        ),
        Tile::Broken { path, .. } => (format!("b:{path}"), file_name(path).to_string()),
    };
    let resp = ui.interact(inner, ui.scope_id().with(&key), Sense::click_and_drag());
    let item = match tile {
        Tile::File { id, .. } => Some(AssetItem::file(*id)),
        Tile::Sub { item, .. } => Some(item.clone()),
        _ => None,
    };
    let selected = item.is_some() && state.assets.selected == item;
    let p = ui.painter().clone();
    let sub = matches!(tile, Tile::Sub { .. });
    if selected {
        p.rect_filled(inner, CornerRadius::same(t.radius + 1), t.accent_bg);
    } else if resp.hovered() {
        p.rect_filled(inner, CornerRadius::same(t.radius + 1), t.hover);
    } else if sub {
        p.rect_filled(inner, CornerRadius::same(t.radius + 1), t.surface_header);
    }
    let icon = Rect::from_center_size(
        pos2(inner.center().x, inner.top() + 6.0 + ICON / 2.0),
        vec2(ICON, ICON),
    );
    let label_rect = Rect::from_min_max(pos2(inner.left(), icon.bottom() + 4.0), inner.max);

    // Icon or thumbnail.
    match tile {
        Tile::Folder(_) => folder_glyph(&p, icon.center(), 20.0, t.text_weak),
        Tile::File { path, .. } => {
            let item = AssetItem::file(match tile {
                Tile::File { id, .. } => *id,
                _ => unreachable!(),
            });
            let kind = FileKind::of(path);
            match (kind, thumbnail(state, ui.ctx(), &item)) {
                (FileKind::Image, Some(tex)) => paint_thumb(&p, icon, tex, ui.ctx()),
                _ => type_icon(&p, icon, kind.tag(), kind_color(kind, t), t),
            }
        }
        Tile::Sub { item, type_name } => {
            let ty = RefType::of_type_name(type_name);
            match (ty, thumbnail(state, ui.ctx(), item)) {
                (RefType::Image, Some(tex)) => paint_thumb(&p, icon.shrink(6.0), tex, ui.ctx()),
                _ => {
                    let tag = match ty {
                        RefType::Mesh => "MESH",
                        RefType::Material => "MAT",
                        RefType::Image => "TEX",
                        RefType::Scene => "SCN",
                        RefType::Shader => "WGSL",
                        RefType::Audio => "SND",
                        RefType::Any => "?",
                    };
                    let color = match ty {
                        RefType::Mesh | RefType::Scene => kind_color(FileKind::Model, t),
                        RefType::Material => kind_color(FileKind::Material, t),
                        RefType::Image => kind_color(FileKind::Image, t),
                        RefType::Shader => kind_color(FileKind::Shader, t),
                        RefType::Audio => kind_color(FileKind::Audio, t),
                        RefType::Any => t.text_faint,
                    };
                    type_icon(&p, icon.shrink(2.0), tag, color, t);
                }
            }
        }
        Tile::Broken { .. } => type_icon(&p, icon, "META", t.error, t),
    }

    // State badge.
    let badge = pos2(icon.right() - 4.0, icon.top() + 4.0);
    let state_now = match (tile, state.asset_server()) {
        (Tile::Broken { message, .. }, _) => Some(LoadState::Failed(message.clone())),
        (_, Some(server)) => item.as_ref().and_then(|i| item_state(server, i)),
        _ => None,
    };
    match &state_now {
        Some(LoadState::Failed(message)) => {
            p.circle_filled(badge, 7.0, t.error);
            p.text(
                badge,
                Align2::CENTER_CENTER,
                "!",
                t.strong_small(),
                t.bg_app,
            );
            let hit = Rect::from_center_size(badge, vec2(16.0, 16.0));
            let _ = ui
                .interact(hit, ui.scope_id().with((&key, "err")), Sense::hover())
                .on_hover_text(message);
            let _ = resp.clone().on_hover_text(message);
        }
        Some(LoadState::Loading) => {
            p.circle_filled(badge, 5.0, t.warn);
            let _ = resp.clone().on_hover_text("Importing…");
        }
        _ => {}
    }

    // Name, or the inline rename field.
    let renaming = match (tile, &state.assets.renaming) {
        (Tile::File { id, .. }, Some((rid, _))) if rid == id => Some(*id),
        _ => None,
    };
    if let Some(id) = renaming {
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(label_rect.shrink2(vec2(2.0, 0.0))));
        let buf = &mut state.assets.renaming.as_mut().expect("renaming").1;
        let r = child.add(
            egui::TextEdit::singleline(buf)
                .font(t.small())
                .desired_width(label_rect.width() - 4.0),
        );
        r.request_focus();
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            actions.push(Action::CancelRename);
        } else if r.lost_focus() {
            actions.push(Action::Rename(id, buf.clone()));
        }
    } else {
        let color = if selected { t.accent_text } else { t.text };
        let mut job = egui::text::LayoutJob::single_section(
            name.clone(),
            egui::TextFormat {
                font_id: t.small(),
                color,
                ..Default::default()
            },
        );
        job.wrap = egui::text::TextWrapping {
            max_width: label_rect.width() - 6.0,
            max_rows: 2,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        job.halign = egui::Align::Center;
        let g = ui.painter().layout_job(job);
        p.galley(pos2(label_rect.center().x, label_rect.top()), g, color);
    }

    // Expand chevron for models.
    if let Tile::File { id, path } = tile
        && FileKind::of(path) == FileKind::Model
    {
        let c = pos2(icon.left() + 4.0, icon.bottom() - 4.0);
        let hit = Rect::from_center_size(c, vec2(16.0, 16.0));
        let open = state.assets.expanded.contains(id);
        let cr = ui.interact(hit, ui.scope_id().with((&key, "chev")), Sense::click());
        p.circle_filled(c, 7.0, if cr.hovered() { t.field_hover } else { t.field });
        widgets::chevron(&p, c, open, t.text);
        if cr.clicked() {
            actions.push(Action::Open(AssetItem::file(*id)));
        }
    }

    // Interaction.
    let on_label = resp
        .interact_pointer_pos()
        .is_some_and(|p| label_rect.contains(p));
    match tile {
        Tile::Folder(f) => {
            if resp.double_clicked() {
                actions.push(Action::SetFolder(f.clone()));
            }
            drop_onto_folder(ui, state, inner, f, t, actions);
        }
        Tile::File { id, .. }
        | Tile::Sub {
            item: AssetItem { id, .. },
            ..
        } => {
            let item = item.clone().expect("file or sub-asset");
            if resp.double_clicked() {
                if on_label && item.label.is_none() {
                    actions.push(Action::StartRename(*id));
                } else {
                    actions.push(Action::Open(item.clone()));
                }
            } else if resp.clicked() {
                actions.push(Action::Select(Some(item.clone())));
            }
            if resp.drag_started() {
                let ty = state.item_type(&item);
                actions.push(Action::Drag(AssetDrag {
                    name: name.clone(),
                    item: item.clone(),
                    ty,
                }));
            }
            if item.label.is_none() {
                let id = *id;
                resp.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        actions.push(Action::StartRename(id));
                    }
                    if ui.button("Reimport").clicked() {
                        actions.push(Action::Reimport(id));
                    }
                    ui.separator();
                    let del = crate::shortcuts::text(ui.ctx(), crate::shortcuts::Command::Delete);
                    if ui
                        .add(egui::Button::new("Move to Trash…").shortcut_text(del))
                        .on_hover_text("Moves the file and its .meta to <project>/.box/trash/")
                        .clicked()
                    {
                        actions.push(Action::Trash(id));
                    }
                });
            }
            // Import what is shown, once.
            state.request_file(*id);
        }
        Tile::Broken { .. } => {}
    }
}

fn paint_thumb(p: &egui::Painter, area: Rect, tex: TextureId, ctx: &egui::Context) {
    let size = ctx
        .tex_manager()
        .read()
        .meta(tex)
        .map_or([1, 1], |m| m.size);
    let aspect = size[0] as f32 / size[1].max(1) as f32;
    let s = if aspect >= 1.0 {
        vec2(area.width(), area.width() / aspect)
    } else {
        vec2(area.height() * aspect, area.height())
    };
    let r = Rect::from_center_size(area.center(), s);
    p.image(
        tex,
        r,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// A rounded square with a short type tag.
pub(crate) fn type_icon(p: &egui::Painter, r: Rect, tag: &str, color: Color32, t: &Tokens) {
    let r = Rect::from_center_size(r.center(), vec2(r.width() * 0.78, r.height() * 0.78));
    p.rect_filled(r, CornerRadius::same(6), color.gamma_multiply(0.22));
    p.rect_stroke(
        r,
        CornerRadius::same(6),
        Stroke::new(1.0, color.gamma_multiply(0.7)),
        StrokeKind::Inside,
    );
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        tag,
        t.strong_small(),
        color,
    );
}

pub(crate) fn kind_color(kind: FileKind, t: &Tokens) -> Color32 {
    match kind {
        FileKind::Image => t.axis_y,
        FileKind::Model => t.axis_z,
        FileKind::Material => t.warn,
        FileKind::Scene => t.accent,
        FileKind::Shader => t.axis_x,
        FileKind::Audio => t.text_strong,
        FileKind::Other => t.text_faint,
    }
}

fn folder_glyph(p: &egui::Painter, c: egui::Pos2, s: f32, color: Color32) {
    let body = Rect::from_center_size(c + vec2(0.0, s * 0.1), vec2(s * 2.0, s * 1.4));
    let tab = Rect::from_min_size(body.left_top() - vec2(0.0, s * 0.3), vec2(s * 0.9, s * 0.4));
    p.rect_filled(tab, CornerRadius::same(1), color.gamma_multiply(0.8));
    p.rect_filled(body, CornerRadius::same(2), color.gamma_multiply(0.8));
}

/// Paints the dragged asset's name next to the pointer (above every pane).
pub(crate) fn drag_ghost(ctx: &egui::Context, state: &EditorState) {
    let Some(drag) = &state.assets.drag else {
        return;
    };
    let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) else {
        return;
    };
    let t = tokens(ctx);
    let p = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("asset_drag"),
    ));
    widgets::overlay_pill(
        &p,
        pos + vec2(14.0, 10.0),
        Align2::LEFT_TOP,
        format!("{} · {}", drag.name, drag.ty.label()),
        &t,
    );
}
