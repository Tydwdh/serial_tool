//! 基于 `egui_tiles` 的工作区布局与面板渲染。
//!
//! 布局树只保存 `PanelKind`，所有业务状态仍由 `WorkbenchApp` 持有；因此用户拖拽、
//! 拆分或合并面板不会影响串口、录制和插件运行时状态。

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;
use egui_tiles::{
    Behavior, Container, ContainerKind, DragPreview, EditAction, ResizeState,
    SimplificationOptions, TabState, Tile, TileId, Tiles, UiResponse,
};
use tool_panels::{PanelKind, PluginPanelEvent, theme};

impl WorkbenchApp {
    pub(super) fn tiles_ui(&mut self, ui: &mut egui::Ui) {
        // `Behavior` 需要可变访问应用状态，Tree 同时也需要可变访问。复制轻量布局元数据
        // 可以避免自引用借用；Tree 只包含面板标识和尺寸，不包含面板业务数据。若面板本身
        // 在本帧调用了“重置布局”或“显示/隐藏区域”，优先保留该显式操作，避免被这份渲染副本覆盖。
        let original_layout = self.panels.ensure_tiles_layout().clone();
        let mut layout = original_layout.clone();
        {
            let mut behavior = WorkbenchTiles { app: self };
            layout.tree.ui(&mut behavior, ui);
        }
        if self.panels.tiles.as_ref() == Some(&original_layout) {
            self.panels.tiles = Some(layout);
        }

        // 拖拽/调整大小过程中只标记脏状态，鼠标松开后一次性原子保存，避免每帧写磁盘。
        if self.layout_dirty && !ui.input(|input| input.pointer.primary_down()) {
            self.layout_dirty = false;
            if let Err(error) = self.save_config() {
                log::warn!("save_config failed: {error}");
            }
        }
    }

    fn tile_panel_body(&mut self, ui: &mut egui::Ui, kind: &PanelKind) {
        self.panels.active_tab = kind.clone();
        match kind {
            PanelKind::Devices => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-devices")
                    .show(ui, |ui| self.device_panel(ui));
            }
            PanelKind::Replay => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-replay")
                    .show(ui, |ui| self.replay_panel.ui(ui));
            }
            PanelKind::Plugins => {
                let events = egui::ScrollArea::vertical()
                    .id_salt("scroll-plugins")
                    .show(ui, |ui| self.plugins_panel.ui(ui, &mut self.plugin_manager))
                    .inner;
                self.handle_plugin_panel_events(events);
            }
            PanelKind::Settings => {
                egui::ScrollArea::vertical()
                    .id_salt("scroll-settings")
                    .show(ui, |ui| self.settings_panel(ui));
            }
            PanelKind::Terminal => {
                if self.popups.terminal_open {
                    ui.vertical_centered(|ui| {
                        ui.add_space(24.0);
                        ui.label("接收区已在悬浮窗口中打开");
                        if ui.button("关闭悬浮窗口并回到工作区").clicked() {
                            self.popups.terminal_open = false;
                        }
                    });
                } else {
                    self.terminal_panel.ui(ui);
                }
            }
            PanelKind::Sender => {
                if ui.available_width() < 420.0 {
                    self.send_panel_vertical(ui);
                } else {
                    self.send_panel_horizontal(ui);
                }
            }
            PanelKind::Logs => self.bottom_log_panel.ui(ui),
            PanelKind::Dynamic(id) => {
                if self.detached_dynamic_panels.contains(id) {
                    ui.label("已弹出到独立窗口");
                } else if self.dynamic_panels.contains(id) {
                    egui::ScrollArea::vertical().show(ui, |ui| self.dynamic_panels.ui_body(ui, id));
                } else {
                    ui.colored_label(theme::red(), format!("动态面板不存在：{id}"));
                }
            }
        }
    }

    fn handle_plugin_panel_events(&mut self, events: Vec<PluginPanelEvent>) {
        for event in events {
            match event {
                PluginPanelEvent::Status(message, is_error) => {
                    let level = if is_error {
                        StatusLevel::Error
                    } else {
                        StatusLevel::Info
                    };
                    self.set_status_force(level, message);
                }
                PluginPanelEvent::RefreshMarket => self.start_marketplace_refresh(),
                PluginPanelEvent::InstallPlugin(id) => {
                    match self.plugins_panel.find_market_plugin(&id) {
                        Some(entry) => self.start_marketplace_install(entry),
                        None => self.set_status(
                            StatusLevel::Warn,
                            format!("找不到插件 {id} 的市场条目，请先刷新市场"),
                        ),
                    }
                }
                PluginPanelEvent::UninstallPlugin(id) => self.uninstall_plugin(&id),
            }
        }
    }

    pub(super) fn panel_title(&self, kind: &PanelKind) -> String {
        match kind {
            PanelKind::Dynamic(id) => self.dynamic_panels.title(id).unwrap_or(id).to_owned(),
            _ => kind.title(),
        }
    }
}

struct WorkbenchTiles<'a> {
    app: &'a mut WorkbenchApp,
}

impl Behavior<PanelKind> for WorkbenchTiles<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut PanelKind) -> UiResponse {
        self.app.tile_panel_body(ui, pane);
        UiResponse::None
    }

    fn tab_hover_cursor_icon(&self) -> egui::CursorIcon {
        egui::CursorIcon::Default
    }

    fn tab_title_for_pane(&mut self, pane: &PanelKind) -> egui::WidgetText {
        format!("{} {}", pane.icon(), self.app.panel_title(pane)).into()
    }

    fn tab_title_for_tile(
        &mut self,
        tiles: &Tiles<PanelKind>,
        tile_id: TileId,
    ) -> egui::WidgetText {
        if let Some(plugin_id) = self.app.panels.plugin_group_id(tile_id) {
            return format!("🔌 {plugin_id}").into();
        }
        match tiles.get(tile_id) {
            Some(Tile::Pane(pane)) => self.tab_title_for_pane(pane),
            Some(Tile::Container(container)) => format!("{:?}", container.kind()).into(),
            None => "MISSING TILE".into(),
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        34.0
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
        _tiles: &Tiles<PanelKind>,
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
        _tiles: &Tiles<PanelKind>,
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
        _tiles: &Tiles<PanelKind>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::tab_active_text()
        } else {
            theme::tab_inactive_text()
        }
    }

    fn retain_pane(&mut self, pane: &PanelKind) -> bool {
        pane.dynamic_id()
            .is_none_or(|id| self.app.dynamic_panels.contains(id))
    }

    fn on_edit(&mut self, _action: EditAction) {
        self.app.layout_dirty = true;
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
                    .app
                    .panels
                    .tiles
                    .as_ref()
                    .and_then(|layout| layout.tree.tiles.get(preview.target_tile_id))
                    .map(|target| match target {
                        Tile::Pane(pane) => {
                            format!("与「{}」组成标签组", self.app.panel_title(pane))
                        }
                        Tile::Container(Container::Tabs(tabs)) => {
                            format!("加入当前标签组（{} 个标签）", tabs.children.len())
                        }
                        Tile::Container(_) => "将目标区域合并为标签组".to_owned(),
                    })
                    .unwrap_or_else(|| "合并为标签".to_owned());
                (label, theme::purple())
            }
            ContainerKind::Horizontal => {
                let parent_center = preview.parent_rect.unwrap_or(preview.target_rect).center();
                let label = if preview.target_rect.center().x <= parent_center.x {
                    "左侧拆分"
                } else {
                    "右侧拆分"
                };
                (label.to_owned(), theme::blue())
            }
            ContainerKind::Vertical => {
                let parent_center = preview.parent_rect.unwrap_or(preview.target_rect).center();
                let label = if preview.target_rect.center().y <= parent_center.y {
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

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            // 最后一个标签被移走或关闭后，立即回收空标签页及其父容器，
            // 避免布局中留下无法使用的空白区域。
            prune_empty_tabs: true,
            prune_empty_containers: true,
            prune_single_child_tabs: false,
            ..Default::default()
        }
    }
}
