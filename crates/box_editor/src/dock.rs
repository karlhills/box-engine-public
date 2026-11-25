//! `egui_tiles` behaviour: pane dispatch plus the S1 tab look (custom tabs,
//! 1 px dividers, accent resize strokes and drag preview).

use egui::{Color32, CornerRadius, Frame, Margin, Sense, Stroke, Ui, pos2, vec2};
use egui_tiles::{Behavior, TabState, Tabs, TileId, Tiles, UiResponse};

use crate::layout::Pane;
use crate::panes;
use crate::state::EditorState;
use crate::theme::{Tokens, tokens};

pub(crate) struct Dock<'a> {
    pub state: &'a mut EditorState,
    /// The active theme, for the callbacks that get no `Ui`.
    pub tokens: Tokens,
}

impl Behavior<Pane> for Dock<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let t = tokens(ui.ctx());
        // egui_tiles panes do not paint their own background. During Play
        // the panes show the throwaway play world: tinted.
        let fill = if self.state.is_playing() {
            t.surface_play
        } else {
            t.surface
        };
        ui.painter().rect_filled(ui.max_rect(), 0.0, fill);
        ui.set_clip_rect(ui.max_rect());
        match pane {
            Pane::Viewport => panes::viewport_ui(ui, self.state),
            Pane::Hierarchy => panes::hierarchy_ui(ui, self.state),
            Pane::Inspector => panes::inspector_ui(ui, self.state),
            Pane::Console => panes::console_ui(ui, self.state),
            Pane::Assets => panes::assets_ui(ui, self.state),
            Pane::Script => crate::script_editor::script_ui(ui, self.state),
        }
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn tab_ui(
        &mut self,
        tiles: &mut Tiles<Pane>,
        ui: &mut Ui,
        id: egui::Id,
        tile_id: TileId,
        state: &TabState,
    ) -> egui::Response {
        let t = tokens(ui.ctx());
        let title = self.tab_title_for_tile(tiles, tile_id).text().to_string();
        let font = if state.active { t.strong() } else { t.body() };
        let galley = ui.painter().layout_no_wrap(title, font, t.text);
        let pad = 12.0;
        // Unseen warnings and errors on the Console tab.
        let unseen = match tiles.get_pane(&tile_id) {
            Some(Pane::Console) => self.state.console.unseen(),
            _ => [0; 2],
        };
        let badge = crate::feedback::badge_width(unseen, ui, &t);
        let badge_gap = if badge > 0.0 { 4.0 } else { 0.0 };
        let (_, rect) = ui.allocate_space(vec2(
            galley.size().x + pad * 2.0 + badge + badge_gap,
            ui.available_height(),
        ));
        let resp = ui
            .interact(rect, id, Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Default);
        if ui.is_rect_visible(rect) && !state.is_being_dragged {
            let p = ui.painter();
            if state.active {
                p.rect_filled(rect, 0.0, t.surface);
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
            let title_w = galley.size().x;
            p.galley(
                pos2(rect.left() + pad, rect.center().y - galley.size().y / 2.0),
                galley,
                col,
            );
            if badge > 0.0 {
                let x = rect.left() + pad + title_w + 6.0;
                crate::feedback::paint_badge(p, unseen, x, rect.center().y, &t);
            }
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
        let t = tokens(ui.ctx());
        let Some(active) = tabs.active.and_then(|a| tiles.get_pane(&a)) else {
            return;
        };
        ui.add_space(t.pad_x);
        if active == &Pane::Viewport {
            panes::viewport::header_ui(ui, self.state);
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        self.tokens.tab_h
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> Color32 {
        self.tokens.surface_header
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        1.0
    }

    fn min_size(&self) -> f32 {
        80.0
    }

    fn resize_stroke(&self, _style: &egui::Style, state: egui_tiles::ResizeState) -> Stroke {
        let t = self.tokens;
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
        self.tokens.accent.gamma_multiply(0.18)
    }

    fn drag_preview_stroke(&self, _visuals: &egui::Visuals) -> Stroke {
        Stroke::new(1.0, self.tokens.accent)
    }

    fn dragged_overlay_color(&self, _visuals: &egui::Visuals) -> Color32 {
        Color32::from_black_alpha(90)
    }

    fn drag_ui(&mut self, tiles: &Tiles<Pane>, ui: &mut Ui, tile_id: TileId) {
        let t = tokens(ui.ctx());
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
