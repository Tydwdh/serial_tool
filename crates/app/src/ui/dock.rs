use eframe::egui;
use tool_panels::{DockArea, PanelKind, theme};

use crate::app::WorkbenchApp;

impl WorkbenchApp {
    pub(crate) fn dock_stack_ui(&mut self, ui: &mut egui::Ui, area: DockArea) {
        let tabs = self.panels.dock.stack(area).tabs.clone();

        if tabs.is_empty() {
            self.empty_dock_ui(ui, area);
            return;
        }

        let active = self.panels.dock.stack(area).active_or_first();

        if let Some(kind) = active {
            self.dock_panel_body(ui, area, kind);
        }
    }

    fn dock_panel_body(&mut self, ui: &mut egui::Ui, _area: DockArea, kind: PanelKind) {
        match kind {
            PanelKind::Devices => self.device_panel(ui),
            PanelKind::Replay => self.replay_panel.ui(ui),
            PanelKind::Plugins => self.plugins_panel.ui(ui, &mut self.plugin_manager),
            PanelKind::Settings => self.settings_panel(ui),
            PanelKind::Terminal => {
                if self.terminal_popup_open {
                    ui.vertical_centered(|ui| {
                        ui.add_space(24.0);
                        ui.label("接收区已在悬浮窗口中打开");
                        if ui.button("关闭悬浮窗口并回到底部").clicked() {
                            self.terminal_popup_open = false;
                        }
                    });
                } else {
                    self.terminal_panel.ui(ui);
                }
            }
            PanelKind::Logs => self.bottom_log_panel.ui(ui),
            PanelKind::Dynamic(id) => {
                if self.detached_dynamic_panels.contains(&id) {
                    ui.label("已弹出到独立窗口");
                } else if self.dynamic_panels.contains(&id) {
                    self.dynamic_panels.ui_body(ui, &id);
                } else {
                    ui.colored_label(theme::RED, format!("动态面板不存在：{id}"));
                }
            }
        }
    }

    fn empty_dock_ui(&mut self, ui: &mut egui::Ui, area: DockArea) {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);

            match area {
                DockArea::Center => {
                    ui.label("主工作区为空");
                    if ui.button("打开回放").clicked() {
                        self.panels
                            .dock
                            .move_panel(PanelKind::Replay, DockArea::Center);
                    }
                }
                DockArea::Bottom => {
                    ui.label("底部面板为空");
                    if ui.button("打开终端").clicked() {
                        self.panels
                            .dock
                            .move_panel(PanelKind::Terminal, DockArea::Bottom);
                    }
                }
                DockArea::Right => {
                    ui.label("右侧停靠区为空");
                    ui.label("以后可放属性、选中事件详情、图表配置、插件参数");
                }
            }
        });
    }

    pub(crate) fn panel_title(&self, kind: &PanelKind) -> String {
        match kind {
            PanelKind::Dynamic(id) => self.dynamic_panels.title(id).unwrap_or(id).to_owned(),
            _ => kind.title(),
        }
    }

    pub(crate) fn paint_dock_drop_overlay(&mut self, ctx: &egui::Context) {
        // 兜底清理：鼠标在窗口外释放时清除拖拽状态
        if ctx.input(|i| !i.pointer.primary_down()) {
            self.dock_dragging_panel = None;
        }

        let Some(kind) = self.dock_dragging_panel.clone() else {
            return;
        };

        let pointer_released = ctx.input(|i| i.pointer.any_released());
        let pointer_pos = ctx.pointer_latest_pos();

        let Some(pointer_pos) = pointer_pos else {
            return;
        };

        // 绘制拖拽面板名
        let title = self.panel_title(&kind);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("dock-drag-ghost"),
        ));
        let gal = ctx.fonts_mut(|f| {
            f.layout(
                title.clone(),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
                f32::INFINITY,
            )
        });
        let rect = egui::Rect::from_min_size(
            pointer_pos + egui::vec2(12.0, -16.0),
            egui::vec2(gal.size().x + 16.0, 24.0),
        );
        painter.rect_filled(
            rect,
            4.0,
            egui::Color32::from_rgba_premultiplied(40, 70, 110, 220),
        );
        painter.galley(rect.center() - gal.size() * 0.5, gal, egui::Color32::WHITE);

        let screen = ctx.content_rect();
        let center_rect = screen.shrink2(egui::vec2(180.0, 140.0));

        let right_target = egui::Rect::from_min_max(
            egui::pos2(screen.right() - 260.0, screen.top() + 80.0),
            egui::pos2(screen.right() - 80.0, screen.bottom() - 80.0),
        );

        let bottom_target = egui::Rect::from_min_max(
            egui::pos2(screen.left() + 220.0, screen.bottom() - 180.0),
            egui::pos2(screen.right() - 220.0, screen.bottom() - 60.0),
        );

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("dock-drop-overlay"),
        ));

        paint_drop_target(&painter, center_rect, pointer_pos, "主工作区");
        paint_drop_target(&painter, right_target, pointer_pos, "右侧");
        paint_drop_target(&painter, bottom_target, pointer_pos, "底部");

        if pointer_released {
            if right_target.contains(pointer_pos) {
                self.panels.dock.move_panel(kind, DockArea::Right);
                self.panels.dock.right_visible = true;
            } else if bottom_target.contains(pointer_pos) {
                self.panels.dock.move_panel(kind, DockArea::Bottom);
                self.set_bottom_visible(true);
            } else if center_rect.contains(pointer_pos) {
                self.panels.dock.move_panel(kind, DockArea::Center);
            }

            self.dock_dragging_panel = None;
            let _ = self.save_config();
        }
    }
}

fn paint_drop_target(painter: &egui::Painter, rect: egui::Rect, pointer: egui::Pos2, label: &str) {
    let hovered = rect.contains(pointer);

    let fill = if hovered {
        egui::Color32::from_rgba_unmultiplied(80, 140, 220, 80)
    } else {
        egui::Color32::from_rgba_unmultiplied(80, 80, 80, 40)
    };

    let stroke = if hovered {
        egui::Stroke::new(2.0, egui::Color32::from_rgb(110, 170, 255))
    } else {
        egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 120, 120))
    };

    painter.rect_filled(rect, 8.0, fill);
    painter.rect_stroke(rect, 8.0, stroke, egui::StrokeKind::Inside);

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(16.0),
        egui::Color32::WHITE,
    );
}
