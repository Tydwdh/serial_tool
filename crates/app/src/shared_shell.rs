//! Shared egui/egui_tiles shell used by Native and Web.

use crate::panel_registry::PanelRegistry;
use eframe::egui;
use egui_tiles::{
    Behavior, Container, ContainerKind, DragPreview, EditAction, ResizeState,
    SimplificationOptions, TabState, Tile, TileId, Tiles, UiResponse,
};
use tool_panels::{PanelId, PanelManager, theme};

pub(crate) trait DockHost {
    fn panels(&mut self) -> &mut PanelManager;
    fn panels_ref(&self) -> &PanelManager;
    fn panel_registry(&self) -> &PanelRegistry;
    fn render_panel(&mut self, ui: &mut egui::Ui, id: &PanelId);
    fn mark_layout_dirty(&mut self);
}

/// Shared application chrome. Native and Web provide only the contents of the
/// two bars; their frame, sizing, ids and central margins stay identical.
pub(crate) trait AppShellHost: DockHost {
    fn render_top_bar(&mut self, ui: &mut egui::Ui);
    fn render_status_bar(&mut self, ui: &mut egui::Ui);

    fn after_dock(&mut self, _ui: &egui::Ui) {}
}

pub(crate) fn show_shell<H: AppShellHost>(host: &mut H, ui: &mut egui::Ui) {
    egui::Panel::top("top-bar")
        .frame(
            egui::Frame::new()
                .fill(theme::bg_secondary())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .inner_margin(egui::Margin::symmetric(10, 7)),
        )
        .show(ui, |ui| host.render_top_bar(ui));

    // Fixed status bar: it must not participate in the resizable bottom dock.
    egui::Panel::bottom("status-bar")
        .resizable(false)
        .exact_size(30.0)
        .show_separator_line(true)
        .show(ui, |ui| host.render_status_bar(ui));

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::bg_deep()))
        .show(ui, |ui| {
            egui::Frame::default()
                .fill(theme::bg_deep())
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| show_dock(host, ui));
        });
    host.after_dock(ui);
}

pub(crate) fn show_dock<H: DockHost>(host: &mut H, ui: &mut egui::Ui) {
    let original_layout = host.panels().ensure_tiles_layout().clone();
    let mut layout = original_layout.clone();
    {
        let mut behavior = SharedTiles { host };
        layout.tree.ui(&mut behavior, ui);
    }
    if layout.reconcile_plugin_groups() {
        host.mark_layout_dirty();
    }
    if host.panels().tiles.as_ref() == Some(&original_layout) {
        host.panels().tiles = Some(layout);
    }
}

struct SharedTiles<'a, H> {
    host: &'a mut H,
}

impl<H: DockHost> SharedTiles<'_, H> {
    fn title(&self, id: &PanelId) -> String {
        self.host.panel_registry().title(id)
    }

    fn icon(&self, id: &PanelId) -> egui_material_icons::MaterialIcon {
        self.host.panel_registry().icon(id)
    }
}

impl<H: DockHost> Behavior<PanelId> for SharedTiles<'_, H> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut PanelId) -> UiResponse {
        self.host.render_panel(ui, pane);
        UiResponse::None
    }

    fn tab_hover_cursor_icon(&self) -> egui::CursorIcon {
        egui::CursorIcon::Default
    }

    fn tab_title_for_pane(&mut self, pane: &PanelId) -> egui::WidgetText {
        format!("{} {}", self.icon(pane).codepoint, self.title(pane)).into()
    }

    fn tab_title_for_tile(&mut self, tiles: &Tiles<PanelId>, tile_id: TileId) -> egui::WidgetText {
        if let Some(plugin_id) = self.host.panels().plugin_group_id(tile_id) {
            return format!(
                "{} {plugin_id}",
                egui_material_icons::icons::ICON_CABLE.codepoint
            )
            .into();
        }
        match tiles.get(tile_id) {
            Some(Tile::Pane(pane)) => self.tab_title_for_pane(pane),
            Some(Tile::Container(Container::Tabs(tabs))) => {
                if let Some(Tile::Pane(pane)) = tabs
                    .children
                    .iter()
                    .find_map(|child_id| tiles.get(*child_id))
                    .filter(|_| tabs.children.len() == 1)
                {
                    self.tab_title_for_pane(pane)
                } else {
                    format!("标签组（{}）", tabs.children.len()).into()
                }
            }
            Some(Tile::Container(container)) => format!("{:?}", container.kind()).into(),
            None => "MISSING TILE".into(),
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        36.0
    }

    fn tab_title_spacing(&self, _visuals: &egui::Visuals) -> f32 {
        12.0
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::tab_bar_bg()
    }

    fn tab_bg_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<PanelId>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::tab_active_bg()
        } else {
            theme::tab_inactive_bg()
        }
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<PanelId>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Stroke {
        if state.active {
            let color = theme::tab_active_outline();
            if color.a() == 0 {
                egui::Stroke::NONE
            } else {
                egui::Stroke::new(1.0, color)
            }
        } else {
            egui::Stroke::NONE
        }
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        let color = theme::tab_bar_outline();
        if color.a() == 0 {
            egui::Stroke::NONE
        } else {
            egui::Stroke::new(1.0, color)
        }
    }

    fn tab_text_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<PanelId>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::tab_active_text()
        } else {
            theme::tab_inactive_text()
        }
    }

    fn retain_pane(&mut self, pane: &PanelId) -> bool {
        self.host.panel_registry().contains(pane)
    }

    fn on_edit(&mut self, _action: EditAction) {
        self.host.mark_layout_dirty();
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        5.0
    }

    fn resize_stroke(&self, _style: &egui::Style, state: ResizeState) -> egui::Stroke {
        match state {
            ResizeState::Idle => egui::Stroke::new(2.0, theme::separator_strong()),
            ResizeState::Hovering => egui::Stroke::new(3.0, theme::blue()),
            ResizeState::Dragging => egui::Stroke::new(3.0, theme::cyan()),
        }
    }

    fn paint_drag_preview(
        &self,
        _visuals: &egui::Visuals,
        painter: &egui::Painter,
        preview: DragPreview,
    ) {
        let (label, color) = match preview.insertion_kind {
            ContainerKind::Tabs => {
                let label = self
                    .host
                    .panels_ref()
                    .tiles
                    .as_ref()
                    .and_then(|layout| layout.tree.tiles.get(preview.target_tile_id))
                    .map(|target| match target {
                        Tile::Pane(pane) => format!("与「{}」组成标签组", self.title(pane)),
                        Tile::Container(Container::Tabs(tabs)) => {
                            format!("加入当前标签组（{} 个标签）", tabs.children.len())
                        }
                        Tile::Container(_) => "将目标区域合并为标签组".to_owned(),
                    })
                    .unwrap_or_else(|| "合并为标签".to_owned());
                (label, theme::purple())
            }
            ContainerKind::Horizontal => {
                let center = preview.parent_rect.unwrap_or(preview.target_rect).center();
                let label = if preview.preview_rect.center().x <= center.x {
                    "左侧拆分"
                } else {
                    "右侧拆分"
                };
                (label.to_owned(), theme::blue())
            }
            ContainerKind::Vertical => {
                let center = preview.parent_rect.unwrap_or(preview.target_rect).center();
                let label = if preview.preview_rect.center().y <= center.y {
                    "上方拆分"
                } else {
                    "下方拆分"
                };
                (label.to_owned(), theme::cyan())
            }
            ContainerKind::Grid => ("加入网格".to_owned(), theme::green()),
        };

        painter.rect(
            preview.preview_rect,
            4.0,
            color.linear_multiply(0.20),
            egui::Stroke::new(2.0, color),
            egui::StrokeKind::Inside,
        );
        let badge_center = preview.parent_rect.unwrap_or(preview.target_rect).center();
        let badge_width = (label.chars().count() as f32 * 16.0 + 32.0).clamp(132.0, 300.0);
        let badge_rect = egui::Rect::from_center_size(badge_center, egui::vec2(badge_width, 30.0));
        painter.rect(
            badge_rect,
            6.0,
            theme::bg_primary(),
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Inside,
        );
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(16.0),
            theme::text_white(),
        );
    }

    fn min_size(&self) -> f32 {
        120.0
    }

    fn is_container_kind_allowed(&self, kind: ContainerKind) -> bool {
        kind != ContainerKind::Vertical
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            prune_empty_tabs: true,
            prune_empty_containers: true,
            prune_single_child_tabs: false,
            ..Default::default()
        }
    }
}
