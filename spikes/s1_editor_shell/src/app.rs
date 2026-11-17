//! Editor shell: state, docked panes, toolbar, layout persistence.

use crate::scene::{self, AssetKind, Level, Row, Scene};
use crate::theme::{self, tokens};
use crate::widgets as w;
use egui::text::LayoutJob;
use egui::{
    Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, TextFormat,
    Ui, UiBuilder, pos2, vec2,
};
use egui_tiles::{Behavior, Container, Linear, LinearDir, Tabs, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Viewport,
    Hierarchy,
    Inspector,
    Assets,
    Console,
}

pub const LAYOUT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct LayoutFile {
    pub version: u32,
    pub tree: Tree<Pane>,
}

pub fn save_layout(path: &std::path::Path, tree: &Tree<Pane>) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = LayoutFile {
        version: LAYOUT_VERSION,
        tree: tree.clone(),
    };
    std::fs::write(path, serde_json::to_string_pretty(&file)?)?;
    Ok(())
}

pub fn load_layout(path: &std::path::Path) -> anyhow::Result<Tree<Pane>> {
    let file: LayoutFile = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    anyhow::ensure!(
        file.version == LAYOUT_VERSION,
        "layout version {} != {}",
        file.version,
        LAYOUT_VERSION
    );
    // Every panel must be present exactly once, or we fall back to the default.
    for p in [
        Pane::Viewport,
        Pane::Hierarchy,
        Pane::Inspector,
        Pane::Assets,
        Pane::Console,
    ] {
        anyhow::ensure!(
            file.tree.tiles.find_pane(&p).is_some(),
            "missing pane {p:?}"
        );
    }
    Ok(file.tree)
}

fn tabs(tiles: &mut Tiles<Pane>, panes: &[Pane], active: usize) -> TileId {
    let ids: Vec<TileId> = panes.iter().map(|p| tiles.insert_pane(*p)).collect();
    let mut t = Tabs::new(ids.clone());
    t.set_active(ids[active]);
    tiles.insert_container(t)
}

fn linear(tiles: &mut Tiles<Pane>, dir: LinearDir, kids: &[(TileId, f32)]) -> TileId {
    let mut l = Linear::new(dir, kids.iter().map(|k| k.0).collect());
    for (id, share) in kids {
        l.shares.set_share(*id, *share);
    }
    tiles.insert_container(l)
}

/// docs/EDITOR.md section 3: hierarchy | viewport over assets+console, inspector full height.
pub fn default_layout() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let hier = tabs(&mut tiles, &[Pane::Hierarchy], 0);
    let vp = tabs(&mut tiles, &[Pane::Viewport], 0);
    let bottom = tabs(&mut tiles, &[Pane::Assets, Pane::Console], 1);
    let insp = tabs(&mut tiles, &[Pane::Inspector], 0);
    let top = linear(
        &mut tiles,
        LinearDir::Horizontal,
        &[(hier, 0.26), (vp, 0.74)],
    );
    let left = linear(
        &mut tiles,
        LinearDir::Vertical,
        &[(top, 0.73), (bottom, 0.27)],
    );
    let root = linear(
        &mut tiles,
        LinearDir::Horizontal,
        &[(left, 0.765), (insp, 0.235)],
    );
    Tree::new(egui::Id::unique("dock"), root, tiles)
}

/// An alternative arrangement: hierarchy over assets on the left, viewport over console in
/// the middle, inspector on the right.
pub fn alt_layout() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let hier = tabs(&mut tiles, &[Pane::Hierarchy], 0);
    let assets = tabs(&mut tiles, &[Pane::Assets], 0);
    let vp = tabs(&mut tiles, &[Pane::Viewport], 0);
    let console = tabs(&mut tiles, &[Pane::Console], 0);
    let insp = tabs(&mut tiles, &[Pane::Inspector], 0);
    let left = linear(
        &mut tiles,
        LinearDir::Vertical,
        &[(hier, 0.62), (assets, 0.38)],
    );
    let mid = linear(
        &mut tiles,
        LinearDir::Vertical,
        &[(vp, 0.78), (console, 0.22)],
    );
    let root = linear(
        &mut tiles,
        LinearDir::Horizontal,
        &[(left, 0.22), (mid, 0.54), (insp, 0.24)],
    );
    Tree::new(egui::Id::unique("dock"), root, tiles)
}

pub struct Inspector {
    name: String,
    enabled: bool,
    pos: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 3],
    mesh: usize,
    material: usize,
    cast: bool,
    receive: bool,
    lod: f32,
    tint: Color32,
    rough: f32,
    metal: f32,
    emission: f32,
    pattern: usize,
    cols: i32,
    rows: i32,
    spacing: f32,
    seed: i32,
    inset: f32,
    open: [bool; 6],
    extra: Vec<(&'static str, bool)>,
    collider: usize,
    light_col: Color32,
    intensity: f32,
    angle: f32,
    light_shadows: bool,
    cascades: usize,
}

const MESHES: [&str; 4] = ["townhouse_a", "warehouse_b", "lamp_post", "crate_small"];
const MATERIALS: [&str; 5] = [
    "M_Plaster_Warm",
    "M_Brick_Weathered",
    "M_Roof_Slate",
    "M_Timber_Dark",
    "M_Window_Glass",
];
const PATTERNS: [&str; 3] = ["Grid", "Staggered", "Random"];
const ADDABLE: [&str; 4] = ["Collider", "Audio Source", "Script", "Point Light"];

pub struct Editor {
    pub scene: Scene,
    pub expanded: HashSet<u32>,
    pub rows: Vec<Row>,
    pub rows_dirty: bool,
    pub filter: String,
    pub selected: Option<u32>,
    pub hier_scroll_to: Option<f32>,
    pub scroll_to_selected: bool,
    log: Vec<scene::LogLine>,
    log_show: [bool; 3],
    log_filter: String,
    log_rows: Vec<usize>,
    log_dirty: bool,
    assets: Vec<scene::Asset>,
    selected_asset: Option<usize>,
    insp: Inspector,
    tool: usize,
    space: usize,
    snap: bool,
    pub playing: bool,
    paused: bool,
    pub viewport_request: [u32; 2],
    pub viewport_tex: Option<egui::TextureId>,
    pub cam_yaw: f32,
    /// Whether the viewport stand-in should outline its "selected" building.
    pub outline_selected: bool,
    pub cam_pitch: f32,
    pub frame_ms: f32,
    pub layout_action: Option<LayoutAction>,
    pub quit: bool,
    /// Cost of the last hierarchy flatten, for the bench report.
    pub last_flatten_ms: f32,
}

#[derive(Clone, Copy)]
pub enum LayoutAction {
    Save,
    Reset,
    Alt,
}

impl Editor {
    pub fn new() -> Self {
        let scene = Scene::generate(10_000);
        let mut ed = Self {
            scene,
            expanded: HashSet::new(),
            rows: Vec::new(),
            rows_dirty: true,
            filter: String::new(),
            selected: None,
            hier_scroll_to: None,
            scroll_to_selected: false,
            log: scene::generate_log(400),
            log_show: [true; 3],
            log_filter: String::new(),
            log_rows: Vec::new(),
            log_dirty: true,
            assets: scene::assets(),
            selected_asset: Some(4),
            insp: Inspector {
                name: String::new(),
                enabled: true,
                pos: [0.0; 3],
                rot: [0.0; 3],
                scale: [1.0; 3],
                mesh: 0,
                material: 0,
                cast: true,
                receive: true,
                lod: 1.0,
                tint: Color32::from_rgb(0xd8, 0xc8, 0xa8),
                rough: 0.72,
                metal: 0.0,
                emission: 0.0,
                pattern: 0,
                cols: 4,
                rows: 3,
                spacing: 1.2,
                seed: 1187,
                inset: 0.08,
                open: [true, true, true, true, false, true],
                extra: Vec::new(),
                collider: 0,
                light_col: Color32::from_rgb(0xff, 0xf1, 0xdc),
                intensity: 32.0,
                angle: 0.53,
                light_shadows: true,
                cascades: 3,
            },
            tool: 0,
            space: 0,
            snap: true,
            playing: false,
            paused: false,
            viewport_request: [0, 0],
            viewport_tex: None,
            cam_yaw: 0.55,
            outline_selected: false,
            cam_pitch: 0.42,
            frame_ms: 0.0,
            layout_action: None,
            quit: false,
            last_flatten_ms: 0.0,
        };
        ed.expanded.insert(ed.scene.roots[0]);
        ed
    }

    pub fn entity_count(&self) -> usize {
        self.scene.entities.len()
    }

    pub fn find(&self, name: &str) -> Option<u32> {
        self.scene
            .entities
            .iter()
            .position(|e| e.name == name)
            .map(|i| i as u32)
    }

    /// Expands all ancestors of `id` and selects it.
    pub fn reveal_and_select(&mut self, id: u32) {
        let mut cur = self.scene.entities[id as usize].parent;
        while let Some(p) = cur {
            self.expanded.insert(p);
            cur = self.scene.entities[p as usize].parent;
        }
        self.select(id);
        self.rows_dirty = true;
        self.scroll_to_selected = true;
    }

    pub fn select(&mut self, id: u32) {
        self.selected = Some(id);
        let e = &self.scene.entities[id as usize];
        self.insp.name = e.name.clone();
        let mut r = scene::Rng::new(id as u64 + 11);
        let f = |r: &mut scene::Rng, s: f32| (r.range(2000) as f32 / 1000.0 - 1.0) * s;
        self.insp.pos = [f(&mut r, 24.0), f(&mut r, 1.5).abs(), f(&mut r, 24.0)];
        self.insp.rot = [0.0, (r.range(8) as f32) * 45.0, 0.0];
        self.insp.scale = [1.0, 1.0 + (r.range(4) as f32) * 0.25, 1.0];
        self.insp.enabled = e.visible;
        self.outline_selected = e.kind == scene::Kind::Prefab;
    }

    pub fn set_expand_all(&mut self, on: bool) {
        self.expanded.clear();
        if on {
            for (i, e) in self.scene.entities.iter().enumerate() {
                if !e.children.is_empty() {
                    self.expanded.insert(i as u32);
                }
            }
        }
        self.rows_dirty = true;
    }

    fn refresh_rows(&mut self) {
        if self.rows_dirty {
            let t0 = std::time::Instant::now();
            scene::flatten(&self.scene, &self.expanded, &self.filter, &mut self.rows);
            self.last_flatten_ms = t0.elapsed().as_secs_f32() * 1000.0;
            self.rows_dirty = false;
            if self.scroll_to_selected
                && let Some(sel) = self.selected
                && let Some(i) = self.rows.iter().position(|r| r.id == sel)
            {
                self.hier_scroll_to = Some((i as f32 - 6.0).max(0.0) * tokens().row_h);
            }
            self.scroll_to_selected = false;
        }
    }

    // ------------------------------------------------------------------ toolbar

    pub fn toolbar(&mut self, ui: &mut Ui) {
        let t = tokens();
        let full = ui.max_rect();
        let h = 26.0;
        let row = full.shrink2(vec2(0.0, ((full.height() - h) / 2.0).floor()));
        let mut lui = ui.new_child(
            UiBuilder::new()
                .max_rect(row)
                .layout(Layout::left_to_right(Align::Center)),
        );
        {
            let ui = &mut lui;
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().interact_size.y = h;
            ui.add_space(4.0);
            egui::MenuBar::new().ui(ui, |ui| {
                ui.spacing_mut().button_padding = vec2(8.0, 4.0);
                ui.spacing_mut().interact_size.y = h;
                ui.menu_button(
                    egui::RichText::new("harbor_district").font(theme::strong()),
                    |ui| {
                        if ui.button("Save Layout").clicked() {
                            self.layout_action = Some(LayoutAction::Save);
                        }
                        if ui.button("Reset Layout").clicked() {
                            self.layout_action = Some(LayoutAction::Reset);
                        }
                        if ui.button("Alternative Layout").clicked() {
                            self.layout_action = Some(LayoutAction::Alt);
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            self.quit = true;
                        }
                    },
                );
                ui.menu_button("Edit", |ui| {
                    if ui.button("Select None").clicked() {
                        self.selected = None;
                    }
                    if ui.button("Clear Search").clicked() {
                        self.filter.clear();
                        self.rows_dirty = true;
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Expand All").clicked() {
                        self.set_expand_all(true);
                    }
                    if ui.button("Collapse All").clicked() {
                        self.set_expand_all(false);
                    }
                    if ui.button("Reset Camera").clicked() {
                        self.cam_yaw = 0.55;
                        self.cam_pitch = 0.42;
                    }
                });
                ui.add_space(10.0);
                vsep(ui, h - 8.0);
                ui.add_space(10.0);
                w::segmented(ui, &mut self.tool, &["Move", "Rotate", "Scale"], h);
                ui.add_space(6.0);
                let space = if self.space == 0 { "Global" } else { "Local" };
                if w::text_button(ui, space, h, false)
                    .on_hover_text("Transform space")
                    .clicked()
                {
                    self.space = 1 - self.space;
                }
                ui.add_space(2.0);
                let snap = self.snap;
                if w::tool_button(ui, "Snap to grid", vec2(h, h), false, true, |p, r, c| {
                    w::magnet_glyph(p, r, if snap { tokens().accent } else { c })
                })
                .clicked()
                {
                    self.snap = !self.snap;
                }
                let label_color = if self.snap { t.text } else { t.text_faint };
                ui.label(
                    egui::RichText::new("0.5 m")
                        .font(theme::mono())
                        .color(label_color),
                );
            });
        }

        // Play controls, centred on the window.
        let bw = 30.0;
        let centre = Rect::from_center_size(full.center(), vec2(bw * 3.0 + 8.0, h));
        let mut cui = ui.new_child(
            UiBuilder::new()
                .max_rect(centre)
                .layout(Layout::left_to_right(Align::Center)),
        );
        cui.spacing_mut().item_spacing.x = 4.0;
        let playing = self.playing;
        if w::tool_button(
            &mut cui,
            "Play",
            vec2(bw, h),
            playing && !self.paused,
            true,
            w::play_glyph,
        )
        .clicked()
        {
            self.playing = true;
            self.paused = false;
        }
        if w::tool_button(
            &mut cui,
            "Pause",
            vec2(bw, h),
            self.paused,
            playing,
            w::pause_glyph,
        )
        .clicked()
            && playing
        {
            self.paused = !self.paused;
        }
        if w::tool_button(&mut cui, "Stop", vec2(bw, h), false, playing, w::stop_glyph).clicked() {
            self.playing = false;
            self.paused = false;
        }

        // Status, right-aligned.
        let right = Rect::from_min_max(pos2(centre.right() + 20.0, full.top()), full.max);
        let mut rui = ui.new_child(
            UiBuilder::new()
                .max_rect(right)
                .layout(Layout::right_to_left(Align::Center)),
        );
        rui.add_space(t.pad_x + 2.0);
        rui.spacing_mut().item_spacing.x = 14.0;
        rui.label(
            egui::RichText::new(format!("{:>5.2} ms", self.frame_ms))
                .font(theme::mono())
                .color(t.text_weak),
        );
        rui.label(
            egui::RichText::new(format!("{} entities", thousands(self.entity_count())))
                .font(theme::body())
                .color(t.text_weak),
        );
        let (state, col) = if self.playing {
            (
                if self.paused { "Paused" } else { "Playing" },
                t.text_strong,
            )
        } else {
            ("Editing · saved", t.text_weak)
        };
        rui.label(egui::RichText::new(state).font(theme::body()).color(col));
    }

    // ---------------------------------------------------------------- hierarchy

    fn hierarchy(&mut self, ui: &mut Ui) {
        let t = tokens();
        let width = ui.available_width();

        // Search field.
        let (row, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
        let field = row.shrink2(vec2(8.0, 6.0));
        let focused_id = ui.scope_id().with("hier_search");
        let has_focus = ui.memory(|m| m.has_focus(focused_id));
        ui.painter()
            .rect_filled(field, CornerRadius::same(t.radius + 1), t.field);
        if has_focus {
            ui.painter().rect_stroke(
                field,
                CornerRadius::same(t.radius + 1),
                Stroke::new(1.0, t.accent),
                egui::StrokeKind::Inside,
            );
        }
        w::search_glyph(
            ui.painter(),
            pos2(field.left() + 12.0, field.center().y),
            t.text_weak,
        );
        let inner = Rect::from_min_max(
            pos2(field.left() + 24.0, field.top()),
            pos2(field.right() - 22.0, field.bottom()),
        );
        let resp = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .id(focused_id)
                            .frame(Frame::NONE)
                            .hint_text("Search entities")
                            .font(theme::body())
                            .desired_width(inner.width()),
                    )
                },
            )
            .inner;
        if resp.changed() {
            self.rows_dirty = true;
        }
        if !self.filter.is_empty() {
            let c = pos2(field.right() - 11.0, field.center().y);
            let r = ui.interact(
                Rect::from_center_size(c, vec2(16.0, 16.0)),
                ui.scope_id().with("clear"),
                Sense::click(),
            );
            let col = if r.hovered() {
                t.text_strong
            } else {
                t.text_weak
            };
            let s = 3.5;
            ui.painter()
                .line_segment([c + vec2(-s, -s), c + vec2(s, s)], Stroke::new(1.2, col));
            ui.painter()
                .line_segment([c + vec2(-s, s), c + vec2(s, -s)], Stroke::new(1.2, col));
            if r.clicked() {
                self.filter.clear();
                self.rows_dirty = true;
            }
        }

        self.refresh_rows();

        // Match count line when filtering.
        if !self.filter.is_empty() {
            let f = self.filter.to_lowercase();
            let matches = self
                .rows
                .iter()
                .filter(|r| self.scene.entities[r.id as usize].name_lower.contains(&f))
                .count();
            let (r, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
            ui.painter().text(
                pos2(r.left() + t.pad_x, r.center().y - 2.0),
                Align2::LEFT_CENTER,
                format!("{} matches", thousands(matches)),
                theme::small(),
                t.text_weak,
            );
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("hierarchy")
            .auto_shrink([false, false]);
        if let Some(off) = self.hier_scroll_to.take() {
            area = area.vertical_scroll_offset(off);
        }
        let n = self.rows.len();
        area.show_rows(ui, t.row_h, n, |ui, range| {
            for i in range {
                let row = self.rows[i];
                self.hierarchy_row(ui, row);
            }
        });
    }

    fn hierarchy_row(&mut self, ui: &mut Ui, row: Row) {
        let t = tokens();
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), t.row_h), Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let id = row.id;
        let e = &self.scene.entities[id as usize];
        let selected = self.selected == Some(id);
        let filtering = !self.filter.is_empty();
        let has_children = !e.children.is_empty();
        let open = filtering || self.expanded.contains(&id);
        let x0 = rect.left() + 8.0 + row.depth as f32 * 14.0;
        let cy = rect.center().y;
        let p = ui.painter();

        if selected {
            p.rect_filled(rect, 0.0, t.accent_bg);
        } else if resp.hovered() {
            p.rect_filled(rect, 0.0, t.hover);
        }
        if !selected {
            for d in 1..=row.depth {
                let x = rect.left() + 8.0 + (d as f32 - 1.0) * 14.0 + 5.0;
                p.vline(x.round() + 0.5, rect.y_range(), Stroke::new(1.0, t.guide));
            }
        }
        let fg_weak = if selected { t.accent_text } else { t.text_weak };
        if has_children {
            w::chevron(p, pos2(x0 + 5.0, cy), open, fg_weak);
        }
        let glyph_col = if selected {
            t.accent_text
        } else if e.visible {
            t.text_weak
        } else {
            t.text_faint
        };
        w::kind_glyph(p, pos2(x0 + 20.0, cy), e.kind, glyph_col);

        let filter_lower = self.filter.to_lowercase();
        let is_match = filtering && e.name_lower.contains(&filter_lower);
        let name_col = if selected {
            t.accent_text
        } else if !e.visible {
            t.text_faint
        } else if filtering && !is_match {
            // ancestors shown only for context
            t.text_weak
        } else {
            t.text
        };
        let mut job = LayoutJob::default();
        let fmt = |c: Color32| TextFormat {
            font_id: theme::body(),
            color: c,
            ..Default::default()
        };
        if is_match && let Some(start) = e.name_lower.find(&filter_lower) {
            let end = start + self.filter.len();
            job.append(&e.name[..start], 0.0, fmt(name_col));
            job.append(
                &e.name[start..end],
                0.0,
                TextFormat {
                    font_id: theme::strong(),
                    color: if selected {
                        t.accent_text
                    } else {
                        t.text_strong
                    },
                    ..Default::default()
                },
            );
            job.append(&e.name[end..], 0.0, fmt(name_col));
        } else {
            job.append(&e.name, 0.0, fmt(name_col));
        }
        let galley = ui.painter().layout_job(job);
        let text_pos = pos2(x0 + 32.0, cy - galley.size().y * 0.5);
        let right_reserved = 44.0;
        ui.painter()
            .with_clip_rect(Rect::from_min_max(
                rect.min,
                pos2(rect.right() - right_reserved, rect.bottom()),
            ))
            .galley(text_pos, galley, name_col);

        let p = ui.painter();
        if !e.visible {
            w::eye_off_glyph(p, pos2(rect.right() - 16.0, cy), fg_weak);
        } else if has_children && !open {
            p.text(
                pos2(rect.right() - 12.0, cy),
                Align2::RIGHT_CENTER,
                e.children.len().to_string(),
                theme::mono(),
                if selected {
                    t.accent_text
                } else {
                    t.text_faint
                },
            );
        }

        let chevron_hit = has_children
            && !filtering
            && resp
                .interact_pointer_pos()
                .is_some_and(|pp| pp.x < x0 + 13.0 && pp.x > x0 - 4.0);
        if resp.double_clicked() && has_children && !filtering {
            self.toggle(id);
        } else if resp.clicked() {
            if chevron_hit {
                self.toggle(id);
            } else {
                self.select(id);
            }
        }
    }

    fn toggle(&mut self, id: u32) {
        if !self.expanded.remove(&id) {
            self.expanded.insert(id);
        }
        self.rows_dirty = true;
    }

    // ---------------------------------------------------------------- inspector

    fn inspector(&mut self, ui: &mut Ui) {
        let t = tokens();
        let Some(sel) = self.selected else {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("Nothing selected").color(t.text_weak));
            });
            return;
        };
        let kind = self.scene.entities[sel as usize].kind;
        egui::ScrollArea::vertical()
            .id_salt("inspector")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add_space(8.0);
                // Header: enabled + name.
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                let mut hui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(r.shrink2(vec2(t.pad_x + 2.0, 1.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                );
                let ins = &mut self.insp;
                w::checkbox(&mut hui, &mut ins.enabled);
                hui.add_space(4.0);
                hui.add(
                    egui::TextEdit::singleline(&mut ins.name)
                        .font(theme::strong())
                        .margin(Margin::symmetric(6, 4))
                        .desired_width(hui.available_width()),
                );
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
                let kind_name = match kind {
                    scene::Kind::Group => "Group",
                    scene::Kind::Mesh => "Mesh",
                    scene::Kind::Light => "Light",
                    scene::Kind::Camera => "Camera",
                    scene::Kind::Prefab => "Prefab instance · townhouse_a",
                };
                ui.painter().text(
                    pos2(r.left() + t.pad_x + 2.0 + 14.0 + 4.0 + 6.0, r.center().y),
                    Align2::LEFT_CENTER,
                    kind_name,
                    theme::small(),
                    t.text_weak,
                );
                ui.painter().text(
                    pos2(r.right() - t.pad_x - 2.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    format!("uid {:08x}", 0x5f3a_0000u32 + sel * 37),
                    theme::mono(),
                    t.text_faint,
                );
                ui.add_space(8.0);

                let ins = &mut self.insp;
                let mut open = ins.open;
                w::section(ui, "Transform", &mut open[0], |ui| {
                    w::property_row(ui, "Position", |ui| w::vec3(ui, &mut ins.pos, 0.05, 2, ""));
                    w::property_row(ui, "Rotation", |ui| w::vec3(ui, &mut ins.rot, 0.5, 1, "°"));
                    w::property_row(ui, "Scale", |ui| w::vec3(ui, &mut ins.scale, 0.01, 2, ""));
                });
                let renderable = matches!(kind, scene::Kind::Mesh | scene::Kind::Prefab);
                if kind == scene::Kind::Light {
                    w::section(ui, "Directional Light", &mut open[5], |ui| {
                        w::property_row(ui, "Colour", |ui| colour(ui, &mut ins.light_col));
                        w::property_row(ui, "Intensity", |ui| {
                            w::slider(ui, &mut ins.intensity, 0.0..=120.0, 1, " klx")
                        });
                        w::property_row(ui, "Angular Size", |ui| {
                            w::slider(ui, &mut ins.angle, 0.0..=5.0, 2, "°")
                        });
                        w::property_row(ui, "Cast Shadows", |ui| {
                            w::checkbox(ui, &mut ins.light_shadows)
                        });
                        w::property_row(ui, "Cascades", |ui| {
                            combo(ui, "cascades", &mut ins.cascades, &["1", "2", "3", "4"])
                        });
                    });
                }
                if renderable {
                    w::section(ui, "Mesh Renderer", &mut open[1], |ui| {
                        w::property_row(ui, "Mesh", |ui| combo(ui, "mesh", &mut ins.mesh, &MESHES));
                        w::property_row(ui, "Material", |ui| {
                            let c = scene::assets()
                                .into_iter()
                                .find(|a| a.name == MATERIALS[ins.material])
                                .map(|a| Color32::from_rgb(a.tint[0], a.tint[1], a.tint[2]))
                                .unwrap_or(t.text_faint);
                            let (sw, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            ui.painter().rect_filled(sw, CornerRadius::same(2), c);
                            combo(ui, "material", &mut ins.material, &MATERIALS);
                        });
                        w::property_row(ui, "Cast Shadows", |ui| w::checkbox(ui, &mut ins.cast));
                        w::property_row(ui, "Receive Shadows", |ui| {
                            w::checkbox(ui, &mut ins.receive)
                        });
                        w::property_row(ui, "LOD Bias", |ui| {
                            w::slider(ui, &mut ins.lod, 0.0..=2.0, 2, "")
                        });
                    });
                    w::section(ui, "Material Overrides", &mut open[2], |ui| {
                        w::property_row(ui, "Tint", |ui| colour(ui, &mut ins.tint));
                        w::property_row(ui, "Roughness", |ui| {
                            w::slider(ui, &mut ins.rough, 0.0..=1.0, 2, "")
                        });
                        w::property_row(ui, "Metallic", |ui| {
                            w::slider(ui, &mut ins.metal, 0.0..=1.0, 2, "")
                        });
                        w::property_row(ui, "Emission", |ui| {
                            w::slider(ui, &mut ins.emission, 0.0..=10.0, 1, " nt")
                        });
                    });
                }
                if kind == scene::Kind::Prefab {
                    w::section(ui, "Window Array", &mut open[3], |ui| {
                        w::property_row(ui, "Pattern", |ui| {
                            combo(ui, "pattern", &mut ins.pattern, &PATTERNS)
                        });
                        w::property_row(ui, "Columns × Rows", |ui| {
                            let half = (ui.available_width() - 4.0) / 2.0;
                            let h = ui.available_height();
                            ui.add(
                                egui::DragValue::new(&mut ins.cols)
                                    .range(1..=32)
                                    .min_size(vec2(half, h)),
                            );
                            ui.add(
                                egui::DragValue::new(&mut ins.rows)
                                    .range(1..=32)
                                    .min_size(vec2(half, h)),
                            );
                        });
                        w::property_row(ui, "Spacing", |ui| {
                            w::slider(ui, &mut ins.spacing, 0.2..=3.0, 2, " m")
                        });
                        w::property_row(ui, "Inset", |ui| {
                            w::slider(ui, &mut ins.inset, 0.0..=0.3, 2, " m")
                        });
                        w::property_row(ui, "Seed", |ui| {
                            let h = ui.available_height();
                            let wdt = ui.available_width();
                            ui.add(egui::DragValue::new(&mut ins.seed).min_size(vec2(wdt, h)));
                        });
                    });
                }
                if renderable {
                    w::section(ui, "Collider", &mut open[4], |ui| {
                        w::property_row(ui, "Shape", |ui| {
                            combo(
                                ui,
                                "collider",
                                &mut ins.collider,
                                &["Box", "Convex Hull", "Mesh"],
                            );
                        });
                    });
                }
                for (name, o) in ins.extra.iter_mut() {
                    w::section(ui, name, o, |ui| {
                        w::property_row(ui, "Enabled", |ui| {
                            ui.label(egui::RichText::new("Defaults").color(t.text_faint));
                        });
                    });
                }
                ins.open = open;

                // Add Component
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter()
                    .hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, t.border));
                ui.add_space(10.0);
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                let mut bui =
                    ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(t.pad_x + 2.0, 0.0))));
                bui.spacing_mut().button_padding = vec2(8.0, 4.0);
                let bw = bui.available_width();
                egui::containers::menu::MenuButton::from_button(
                    egui::Button::new("Add Component").min_size(vec2(bw, 26.0)),
                )
                .ui(&mut bui, |ui| {
                    for c in ADDABLE {
                        if ui.button(c).clicked() && !ins.extra.iter().any(|e| e.0 == c) {
                            ins.extra.push((c, true));
                        }
                    }
                });
                ui.add_space(12.0);
            });
    }

    // ------------------------------------------------------------------ console

    fn console(&mut self, ui: &mut Ui) {
        let t = tokens();
        let width = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
        let counts = [Level::Info, Level::Warn, Level::Error]
            .map(|l| self.log.iter().filter(|x| x.level == l).count());
        let mut bui = ui.new_child(
            UiBuilder::new()
                .max_rect(bar.shrink2(vec2(t.pad_x - 2.0, 5.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        bui.spacing_mut().item_spacing.x = 2.0;
        for (i, (label, col)) in [
            ("Info", t.text_weak),
            ("Warnings", t.warn),
            ("Errors", t.error),
        ]
        .into_iter()
        .enumerate()
        {
            if chip(&mut bui, label, counts[i], col, self.log_show[i]).clicked() {
                self.log_show[i] = !self.log_show[i];
                self.log_dirty = true;
            }
        }
        bui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("Clear").color(t.text)).frame(false))
                .clicked()
            {
                self.log.clear();
                self.log_dirty = true;
            }
            ui.add_space(6.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.log_filter)
                    .hint_text("Filter")
                    .font(theme::body())
                    .margin(Margin::symmetric(8, 3))
                    .desired_width(180.0),
            );
            if r.changed() {
                self.log_dirty = true;
            }
        });

        if self.log_dirty {
            let f = self.log_filter.to_lowercase();
            self.log_rows = (0..self.log.len())
                .filter(|&i| {
                    let l = &self.log[i];
                    let li = match l.level {
                        Level::Info => 0,
                        Level::Warn => 1,
                        Level::Error => 2,
                    };
                    self.log_show[li] && (f.is_empty() || l.text.to_lowercase().contains(&f))
                })
                .collect();
            self.log_dirty = false;
        }

        let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
        ui.painter()
            .hline(r.x_range(), r.top(), Stroke::new(1.0, t.border));

        ui.spacing_mut().item_spacing.y = 0.0;
        let row_h = 19.0;
        let mono = theme::mono();
        egui::ScrollArea::vertical()
            .id_salt("console")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_rows(ui, row_h, self.log_rows.len(), |ui, range| {
                for i in range {
                    let l = &self.log[self.log_rows[i]];
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    let p = ui.painter();
                    if resp.hovered() {
                        p.rect_filled(rect, 0.0, t.hover);
                    }
                    let cy = rect.center().y;
                    let mut x = rect.left() + t.pad_x + 2.0;
                    p.text(
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        &l.time,
                        mono.clone(),
                        t.text_faint,
                    );
                    x += 96.0;
                    let (lv, lc) = match l.level {
                        Level::Info => ("info", t.text_faint),
                        Level::Warn => ("warn", t.warn),
                        Level::Error => ("error", t.error),
                    };
                    p.text(pos2(x, cy), Align2::LEFT_CENTER, lv, mono.clone(), lc);
                    x += 46.0;
                    p.text(
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        l.source,
                        mono.clone(),
                        t.text_weak,
                    );
                    x += 64.0;
                    let msg_col = match l.level {
                        Level::Error => Color32::from_rgb(0xe8, 0xb4, 0xae),
                        _ => t.text,
                    };
                    p.with_clip_rect(rect).text(
                        pos2(x, cy),
                        Align2::LEFT_CENTER,
                        &l.text,
                        mono.clone(),
                        msg_col,
                    );
                }
            });
    }

    // ------------------------------------------------------------------- assets

    fn assets(&mut self, ui: &mut Ui) {
        let t = tokens();
        let width = ui.available_width();
        let (bar, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
        let p = ui.painter();
        let x = bar.left() + t.pad_x + 2.0;
        let g = p.layout_no_wrap("assets  /  ".into(), theme::body(), t.text_weak);
        let gw = g.size().x;
        p.galley(pos2(x, bar.center().y - g.size().y / 2.0), g, t.text_weak);
        p.text(
            pos2(x + gw, bar.center().y),
            Align2::LEFT_CENTER,
            "harbor_district",
            theme::body(),
            t.text,
        );
        p.text(
            pos2(bar.right() - t.pad_x - 2.0, bar.center().y),
            Align2::RIGHT_CENTER,
            format!("{} items", self.assets.len()),
            theme::small(),
            t.text_faint,
        );
        let (r, _) = ui.allocate_exact_size(vec2(width, 1.0), Sense::hover());
        ui.painter()
            .hline(r.x_range(), r.top(), Stroke::new(1.0, t.border));

        egui::ScrollArea::vertical()
            .id_salt("assets")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let tile = vec2(84.0, 86.0);
                let pad = 10.0;
                let cols = (((ui.available_width() - pad) / tile.x).floor() as usize).max(1);
                let rows = self.assets.len().div_ceil(cols);
                let (area, _) = ui.allocate_exact_size(
                    vec2(ui.available_width(), rows as f32 * tile.y + pad * 2.0),
                    Sense::hover(),
                );
                for (i, a) in self.assets.iter().enumerate() {
                    let (c, r) = (i % cols, i / cols);
                    let rect = Rect::from_min_size(
                        area.min + vec2(pad + c as f32 * tile.x, pad + r as f32 * tile.y),
                        tile - vec2(6.0, 6.0),
                    );
                    let resp = ui.interact(rect, ui.scope_id().with(("asset", i)), Sense::click());
                    if resp.clicked() {
                        self.selected_asset = Some(i);
                    }
                    let sel = self.selected_asset == Some(i);
                    let p = ui.painter();
                    if sel {
                        p.rect_filled(rect, CornerRadius::same(4), t.accent_bg);
                    } else if resp.hovered() {
                        p.rect_filled(rect, CornerRadius::same(4), t.hover);
                    }
                    let thumb = Rect::from_center_size(
                        pos2(rect.center().x, rect.top() + 30.0),
                        vec2(48.0, 48.0),
                    );
                    asset_thumb(p, thumb, a);
                    let name_col = if sel { t.accent_text } else { t.text };
                    let mut job = LayoutJob::single_section(
                        a.name.to_string(),
                        TextFormat {
                            font_id: theme::small(),
                            color: name_col,
                            ..Default::default()
                        },
                    );
                    job.wrap = egui::text::TextWrapping {
                        max_width: rect.width() - 8.0,
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    };
                    let galley = p.layout_job(job);
                    let tp = pos2(
                        rect.center().x - galley.size().x / 2.0,
                        thumb.bottom() + 7.0,
                    );
                    p.galley(tp, galley, name_col);
                }
            });
    }

    // ----------------------------------------------------------------- viewport

    fn viewport(&mut self, ui: &mut Ui) {
        let t = tokens();
        let rect = ui.max_rect();
        let ppp = ui.ctx().pixels_per_point();
        self.viewport_request = [
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        ];
        let resp = ui.interact(
            rect,
            ui.scope_id().with("viewport"),
            Sense::click_and_drag(),
        );
        if resp.dragged() {
            let d = resp.drag_delta();
            self.cam_yaw -= d.x * 0.008;
            self.cam_pitch = (self.cam_pitch + d.y * 0.006).clamp(0.08, 1.3);
        }
        let p = ui.painter_at(rect);
        match self.viewport_tex {
            Some(id) => {
                p.image(
                    id,
                    rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                p.rect_filled(rect, 0.0, Color32::BLACK);
            }
        }
        // Overlays.
        let pill =
            |p: &egui::Painter, pos: egui::Pos2, align: Align2, text: String, col: Color32| {
                let g = p.layout_no_wrap(text, theme::small(), col);
                let size = g.size() + vec2(14.0, 8.0);
                let r = align.anchor_size(pos, size);
                p.rect_filled(r, CornerRadius::same(4), Color32::from_black_alpha(110));
                p.galley(r.min + vec2(7.0, 4.0), g, col);
            };
        let ov = Color32::from_rgb(0xe6, 0xe6, 0xea);
        pill(
            &p,
            rect.left_top() + vec2(10.0, 10.0),
            Align2::LEFT_TOP,
            "Perspective  ·  Lit  ·  Grid".into(),
            ov,
        );
        if self.playing {
            pill(
                &p,
                rect.center_top() + vec2(0.0, 10.0),
                Align2::CENTER_TOP,
                if self.paused {
                    "Paused".into()
                } else {
                    "Playing · Esc releases input".into()
                },
                ov,
            );
        }
        pill(
            &p,
            rect.left_bottom() + vec2(10.0, -10.0),
            Align2::LEFT_BOTTOM,
            match self.selected {
                Some(id) => format!("{}  ·  1 selected", self.scene.entities[id as usize].name),
                None => "Nothing selected".into(),
            },
            Color32::from_rgb(0xc0, 0xc0, 0xc6),
        );
        // Axis gizmo, top-right.
        let c = rect.right_top() + vec2(-36.0, 38.0);
        p.circle_filled(c, 26.0, Color32::from_black_alpha(70));
        let (sy, cy) = self.cam_yaw.sin_cos();
        let (sp, cp) = self.cam_pitch.sin_cos();
        let proj = |v: [f32; 3]| {
            // camera right = (cy, 0, -sy), up = (-sp*sy, cp, -sp*cy)
            let x = v[0] * cy - v[2] * sy;
            let y = -sp * sy * v[0] + cp * v[1] - sp * cy * v[2];
            vec2(x, -y) * 17.0
        };
        let axes = [
            ([1.0, 0.0, 0.0], t.axis_x, "X"),
            ([0.0, 1.0, 0.0], t.axis_y, "Y"),
            ([0.0, 0.0, 1.0], t.axis_z, "Z"),
        ];
        for (v, col, label) in axes {
            let e = c + proj(v);
            p.line_segment([c, e], Stroke::new(1.6, col));
            p.circle_filled(e, 6.5, col);
            p.text(
                e,
                Align2::CENTER_CENTER,
                label,
                theme::strong_small(),
                Color32::from_rgb(0x14, 0x14, 0x16),
            );
        }
        let _ = t;
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

fn vsep(ui: &mut Ui, h: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(1.0, h), Sense::hover());
    ui.painter()
        .vline(r.center().x, r.y_range(), Stroke::new(1.0, tokens().border));
}

pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn combo(ui: &mut Ui, id: &str, sel: &mut usize, items: &[&str]) {
    let w = ui.available_width();
    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(items[*sel])
        .show_ui(ui, |ui| {
            for (i, it) in items.iter().enumerate() {
                ui.selectable_value(sel, i, *it);
            }
        });
}

fn colour(ui: &mut Ui, c: &mut Color32) {
    let t = tokens();
    ui.spacing_mut().interact_size.x = 44.0;
    egui::color_picker::color_edit_button_srgba(ui, c, egui::color_picker::Alpha::Opaque);
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()))
            .font(theme::mono())
            .color(t.text),
    );
}

fn chip(ui: &mut Ui, label: &str, count: usize, dot: Color32, on: bool) -> egui::Response {
    let t = tokens();
    let font = theme::body();
    let g = ui
        .painter()
        .layout_no_wrap(label.into(), font.clone(), t.text);
    let cg = ui
        .painter()
        .layout_no_wrap(count.to_string(), theme::mono(), t.text_weak);
    let size = vec2(g.size().x + cg.size().x + 32.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.field);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(t.radius), t.hover);
    }
    let alpha = if on { 1.0 } else { 0.45 };
    p.circle_filled(
        pos2(rect.left() + 10.0, rect.center().y),
        3.0,
        dot.gamma_multiply(alpha),
    );
    let tc = if on { t.text_strong } else { t.text_weak };
    p.galley(
        pos2(rect.left() + 18.0, rect.center().y - g.size().y / 2.0),
        g,
        tc,
    );
    let gx = rect.right() - 8.0 - cg.size().x;
    p.galley(
        pos2(gx, rect.center().y - cg.size().y / 2.0),
        cg,
        if on { t.text_weak } else { t.text_faint },
    );
    resp
}

fn asset_thumb(p: &egui::Painter, r: Rect, a: &scene::Asset) {
    let t = tokens();
    p.rect_filled(r, CornerRadius::same(4), t.field);
    let c = r.center();
    let st = Stroke::new(1.2, t.text_weak);
    match a.kind {
        AssetKind::Folder => {
            let body = Rect::from_center_size(c + vec2(0.0, 2.0), vec2(26.0, 18.0));
            p.rect_filled(
                Rect::from_min_size(body.left_top() + vec2(0.0, -4.0), vec2(11.0, 6.0)),
                CornerRadius::same(2),
                Color32::from_rgb(0x55, 0x56, 0x5e),
            );
            p.rect_filled(
                body,
                CornerRadius::same(2),
                Color32::from_rgb(0x55, 0x56, 0x5e),
            );
        }
        AssetKind::Material => {
            let col = Color32::from_rgb(a.tint[0], a.tint[1], a.tint[2]);
            p.circle_filled(c, 15.0, col);
        }
        AssetKind::Mesh => {
            let s = 11.0;
            let pts = vec![
                c + vec2(0.0, -s),
                c + vec2(s * 0.9, -s * 0.5),
                c + vec2(s * 0.9, s * 0.5),
                c + vec2(0.0, s),
                c + vec2(-s * 0.9, s * 0.5),
                c + vec2(-s * 0.9, -s * 0.5),
            ];
            p.add(egui::Shape::closed_line(pts, st));
            p.line_segment([c + vec2(-s * 0.9, -s * 0.5), c], st);
            p.line_segment([c + vec2(s * 0.9, -s * 0.5), c], st);
            p.line_segment([c, c + vec2(0.0, s)], st);
        }
        AssetKind::Texture => {
            let n = 4;
            let cell = 6.0;
            let o = c - vec2(cell * n as f32 / 2.0, cell * n as f32 / 2.0);
            for i in 0..n {
                for j in 0..n {
                    let col = if (i + j) % 2 == 0 {
                        Color32::from_rgb(0x6a, 0x6a, 0x72)
                    } else {
                        Color32::from_rgb(0x3a, 0x3a, 0x40)
                    };
                    p.rect_filled(
                        Rect::from_min_size(
                            o + vec2(i as f32 * cell, j as f32 * cell),
                            vec2(cell, cell),
                        ),
                        0.0,
                        col,
                    );
                }
            }
        }
        AssetKind::Scene => {
            let rr = Rect::from_center_size(c, vec2(26.0, 18.0));
            p.rect_stroke(rr, CornerRadius::same(2), st, egui::StrokeKind::Middle);
            p.line_segment(
                [rr.left_bottom() + vec2(3.0, -3.0), c + vec2(-2.0, 0.0)],
                st,
            );
            p.line_segment([c + vec2(-2.0, 0.0), c + vec2(3.0, 4.0)], st);
            p.line_segment([c + vec2(3.0, 4.0), rr.right_top() + vec2(-4.0, 5.0)], st);
        }
    }
}

// --------------------------------------------------------------------- docking

pub struct Shell {
    pub ed: Editor,
    pub tree: Tree<Pane>,
}

struct Dock<'a> {
    ed: &'a mut Editor,
}

impl Behavior<Pane> for Dock<'_> {
    fn pane_ui(
        &mut self,
        ui: &mut Ui,
        _tile_id: TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        let t = tokens();
        ui.painter().rect_filled(ui.max_rect(), 0.0, t.surface);
        ui.set_clip_rect(ui.max_rect());
        match pane {
            Pane::Viewport => self.ed.viewport(ui),
            Pane::Hierarchy => self.ed.hierarchy(ui),
            Pane::Inspector => self.ed.inspector(ui),
            Pane::Assets => self.ed.assets(ui),
            Pane::Console => self.ed.console(ui),
        }
        egui_tiles::UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Viewport => "Viewport",
            Pane::Hierarchy => "Hierarchy",
            Pane::Inspector => "Inspector",
            Pane::Assets => "Assets",
            Pane::Console => "Console",
        }
        .into()
    }

    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<Pane>,
        ui: &mut Ui,
        id: egui::Id,
        tile_id: TileId,
        state: &egui_tiles::TabState,
    ) -> egui::Response {
        let t = tokens();
        let title = self.tab_title_for_tile(tiles, tile_id).text().to_string();
        let font = if state.active {
            theme::strong()
        } else {
            theme::body()
        };
        let galley = ui.painter().layout_no_wrap(title.clone(), font, t.text);
        let pad = 12.0;
        let (_, rect) = ui.allocate_space(vec2(galley.size().x + pad * 2.0, ui.available_height()));
        let resp = ui
            .interact(rect, id, Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Default);
        if ui.is_rect_visible(rect) && !state.is_being_dragged {
            let p = ui.painter();
            if state.active {
                p.rect_filled(
                    rect,
                    CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 0,
                        se: 0,
                    },
                    t.surface,
                );
            } else if resp.hovered() {
                p.rect_filled(rect, 0.0, t.hover);
            }
            let col = if state.active {
                t.text_strong
            } else if resp.hovered() {
                t.text
            } else {
                t.text_weak
            };
            p.galley(
                pos2(rect.left() + pad, rect.center().y - galley.size().y / 2.0),
                galley,
                col,
            );
        }
        self.on_tab_button(tiles, tile_id, resp)
    }

    fn top_bar_right_ui(
        &mut self,
        tiles: &Tiles<Pane>,
        ui: &mut Ui,
        _tile_id: TileId,
        tabs: &Tabs,
        _scroll_offset: &mut f32,
    ) {
        let t = tokens();
        let Some(active) = tabs.active.and_then(|a| tiles.get_pane(&a)) else {
            return;
        };
        ui.add_space(t.pad_x);
        if active == &Pane::Viewport {
            let [w, h] = self.ed.viewport_request;
            ui.label(
                egui::RichText::new(format!("{w} × {h}"))
                    .font(theme::mono())
                    .color(t.text_faint),
            );
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        tokens().tab_h
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> Color32 {
        tokens().surface_header
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        1.0
    }

    fn min_size(&self) -> f32 {
        80.0
    }

    fn resize_stroke(&self, _style: &egui::Style, state: egui_tiles::ResizeState) -> Stroke {
        let t = tokens();
        match state {
            egui_tiles::ResizeState::Idle => Stroke::new(1.0, t.divider),
            egui_tiles::ResizeState::Hovering => Stroke::new(1.0, t.accent_bg),
            egui_tiles::ResizeState::Dragging => Stroke::new(2.0, t.accent),
        }
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }

    fn drag_preview_color(&self, _visuals: &egui::Visuals) -> Color32 {
        tokens().accent.gamma_multiply(0.18)
    }

    fn drag_preview_stroke(&self, _visuals: &egui::Visuals) -> Stroke {
        Stroke::new(1.0, tokens().accent)
    }

    fn dragged_overlay_color(&self, _visuals: &egui::Visuals) -> Color32 {
        Color32::from_black_alpha(90)
    }

    fn drag_ui(&mut self, tiles: &Tiles<Pane>, ui: &mut Ui, tile_id: TileId) {
        let t = tokens();
        Frame::NONE
            .fill(t.field_active)
            .corner_radius(CornerRadius::same(t.radius))
            .inner_margin(Margin::symmetric(10, 5))
            .show(ui, |ui| {
                let title = self.tab_title_for_tile(tiles, tile_id);
                ui.label(title);
            });
    }
}

impl Shell {
    pub fn new(tree: Tree<Pane>) -> Self {
        Self {
            ed: Editor::new(),
            tree,
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let t = tokens();
        egui::Panel::top("toolbar")
            .exact_size(t.toolbar_h)
            .show_separator_line(false)
            .resizable(false)
            .frame(Frame::NONE.fill(t.surface_header))
            .show(ui, |ui| {
                self.ed.toolbar(ui);
                let r = ui.max_rect();
                ui.painter()
                    .hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.divider));
            });
        egui::CentralPanel::no_frame()
            .frame(Frame::NONE.fill(t.divider))
            .show(ui, |ui| {
                let mut dock = Dock { ed: &mut self.ed };
                self.tree.ui(&mut dock, ui);
            });
        if let Some(a) = self.ed.layout_action.take() {
            match a {
                LayoutAction::Save => {
                    let path = std::path::Path::new(".box/layout.json");
                    if let Err(e) = save_layout(path, &self.tree) {
                        eprintln!("layout save failed: {e}");
                    }
                }
                LayoutAction::Reset => self.tree = default_layout(),
                LayoutAction::Alt => self.tree = alt_layout(),
            }
        }
    }

    /// Finds a pane's tile and makes it the active tab.
    pub fn activate(&mut self, pane: Pane) {
        if let Some(id) = self.tree.tiles.find_pane(&pane)
            && let Some(parent) = self.tree.tiles.parent_of(id)
            && let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(parent)
        {
            tabs.set_active(id);
        }
    }
}
