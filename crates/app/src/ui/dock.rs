use eframe::egui;
use tool_panels::{DockArea, PanelKind, theme};

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;

impl WorkbenchApp {
    pub(crate) fn dock_stack_ui(&mut self, ui: &mut egui::Ui, area: DockArea) {
        let tabs = self.panels.dock.stack(area).tabs.clone();

        if tabs.is_empty() {
            self.empty_dock_ui(ui, area);
            return;
        }

        // Center 是主工作区，不画 dock tab，不参与拖拽
        if matches!(area, DockArea::Bottom | DockArea::Right) {
            self.dock_tab_bar(ui, area, &tabs);
            ui.separator();
        }

        let active = self.panels.dock.stack(area).active_or_first();

        if let Some(kind) = active {
            self.dock_panel_body(ui, area, kind);
        }
    }
    fn dock_tab_bar(&mut self, ui: &mut egui::Ui, area: DockArea, tabs: &[PanelKind]) {
        let pointer = ui.ctx().pointer_latest_pos();
        let mut tab_rects: Vec<(PanelKind, egui::Rect)> = Vec::with_capacity(tabs.len());

        ui.horizontal(|ui| {
            for kind in tabs {
                let active = self.panels.dock.stack(area).active.as_ref() == Some(kind);
                let dragging = self.dock_dragging_panel.as_ref() == Some(kind);
                let title = self.panel_title(kind);

                let width = (title.chars().count() as f32 * 14.0 + 28.0).clamp(64.0, 180.0);

                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click_and_drag());

                let response = response.on_hover_text("拖动调整位置，右键移动到其他区域");

                tab_rects.push((kind.clone(), rect));

                if response.clicked() {
                    self.panels.dock.stack_mut(area).active = Some(kind.clone());
                }

                if response.drag_started() {
                    self.dock_dragging_panel = Some(kind.clone());
                }

                if response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    ui.ctx().request_repaint();
                }

                let bg = if dragging {
                    theme::BG_TERTIARY
                } else if active {
                    theme::BG_SELECTION
                } else if response.hovered() {
                    theme::WIDGET_HOVER
                } else {
                    theme::BG_SECONDARY
                };

                let fg = if active {
                    theme::TEXT_WHITE
                } else {
                    theme::TEXT_PRIMARY
                };

                ui.painter().rect_filled(rect, 4.0, bg);
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0, theme::BORDER_LIGHT),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &title,
                    egui::FontId::proportional(13.0),
                    fg,
                );

                response.context_menu(|ui| {
                    match area {
                        DockArea::Bottom => {
                            if ui.button("移到右侧").clicked() {
                                self.panels.dock.move_panel(kind.clone(), DockArea::Right);
                                self.panels.dock.right_visible = true;
                                self.panels.sync_tabs_from_dock();
                                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
                                ui.close();
                            }
                        }
                        DockArea::Right => {
                            if ui.button("移到底部").clicked() {
                                self.panels.dock.move_panel(kind.clone(), DockArea::Bottom);
                                self.panels.sync_tabs_from_dock();
                                self.set_bottom_visible(true);
                                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
                                ui.close();
                            }
                        }
                        DockArea::Center => {}
                    }

                    if ui.button("关闭").clicked() {
                        self.panels.dock.stack_mut(area).close(kind);
                        if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
                        ui.close();
                    }
                });
            }

            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match area {
                    DockArea::Bottom => {
                        if ui.small_button("×").on_hover_text("隐藏底部面板").clicked() {
                            self.panels.dock.bottom_visible = false;
                            self.bottom_panel_visible = false;
                            if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
                        }
                    }
                    DockArea::Right => {
                        if ui
                            .small_button("×")
                            .on_hover_text("隐藏右侧停靠区")
                            .clicked()
                        {
                            self.panels.dock.right_visible = false;
                            if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
                        }
                    }
                    DockArea::Center => {}
                },
            );
        });

        let insert_index = if self.dock_dragging_panel.is_some() {
            pointer.and_then(|pos| horizontal_insert_index_from_pointer(&tab_rects, pos))
        } else {
            None
        };

        if let Some(index) = insert_index {
            paint_dock_insert_line(ui, &tab_rects, index);
        }

        // 只在"释放在当前 tab bar 上"时处理同区域重排。
        // 不要无条件 take()，否则跨区域 drop overlay 没机会处理。
        if ui.input(|i| i.pointer.any_released())
            && let Some(kind) = self.dock_dragging_panel.clone()
            && self.panels.dock.stack(area).contains(&kind)
            && let Some(insert_index) = insert_index
        {
            self.dock_dragging_panel = None;

            if self
                .panels
                .dock
                .stack_mut(area)
                .reorder(&kind, insert_index)
                && let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
        }
    }
    fn dock_panel_body(&mut self, ui: &mut egui::Ui, area: DockArea, kind: PanelKind) {
        match kind {
            PanelKind::Devices => {
                egui::ScrollArea::vertical().show(ui, |ui| self.device_panel(ui));
            }
            PanelKind::Replay => {
                egui::ScrollArea::vertical().show(ui, |ui| self.replay_panel.ui(ui));
            }
            PanelKind::Plugins => {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some((msg, is_error)) = self.plugins_panel.ui(ui, &mut self.plugin_manager) {
                        let level = if is_error { StatusLevel::Error } else { StatusLevel::Info };
                        self.set_status_force(level, msg);
                    }
                });
            }
            PanelKind::Settings => {
                egui::ScrollArea::vertical().show(ui, |ui| self.settings_panel(ui));
            }
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
            PanelKind::Sender => match area {
                DockArea::Right => self.send_panel_vertical(ui),
                DockArea::Bottom => self.send_panel_horizontal(ui),
                DockArea::Center => {
                    ui.colored_label(theme::YELLOW, "发送器不支持放在主工作区，已自动移到底部");
                    self.panels.dock.move_panel(PanelKind::Sender, DockArea::Bottom);
                    self.panels.dock.bottom_visible = true;
                    self.set_bottom_visible(true);
                    self.panels.sync_tabs_from_dock();
                }
            },
            PanelKind::Logs => self.bottom_log_panel.ui(ui),
            PanelKind::Dynamic(id) => {
                if self.detached_dynamic_panels.contains(&id) {
                    ui.label("已弹出到独立窗口");
                } else if self.dynamic_panels.contains(&id) {
                    egui::ScrollArea::vertical().show(ui, |ui| self.dynamic_panels.ui_body(ui, &id));
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
                        self.panels.sync_tabs_from_dock();
                    }
                }
                DockArea::Bottom => {
                    ui.label("底部面板为空");
                }
                DockArea::Right => {
                    ui.label("右侧停靠区为空");
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
        let Some(kind) = self.dock_dragging_panel.clone() else {
            return;
        };

        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let released = ctx.input(|i| i.pointer.any_released());
        let esc_pressed = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        // ESC 取消拖拽
        if esc_pressed {
            self.dock_dragging_panel = None;
            return;
        }

        let Some(pos) = ctx.pointer_latest_pos() else {
            if !primary_down {
                self.dock_dragging_panel = None;
            }
            return;
        };

        let right_hit = self.right_dock_rect.is_some_and(|rect| rect.contains(pos));

        let bottom_hit = self.bottom_dock_rect.is_some_and(|rect| rect.contains(pos));

        if right_hit {
            if let Some(rect) = self.right_dock_rect {
                paint_real_dock_hover(ctx, rect, "右侧");
            }
        } else if bottom_hit && let Some(rect) = self.bottom_dock_rect {
            paint_real_dock_hover(ctx, rect, "底部");
        }

        if released {
            if right_hit {
                self.panels.dock.move_panel(kind, DockArea::Right);
                self.panels.dock.right_visible = true;
                self.panels.sync_tabs_from_dock();
                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
            } else if bottom_hit {
                self.panels.dock.move_panel(kind, DockArea::Bottom);
                self.panels.sync_tabs_from_dock();
                self.set_bottom_visible(true);
                if let Err(e) = self.save_config() { log::warn!("save_config failed: {e}") };
            }

            self.dock_dragging_panel = None;
        } else if !primary_down {
            self.dock_dragging_panel = None;
        }
    }
}

fn horizontal_insert_index_from_pointer(
    rects: &[(PanelKind, egui::Rect)],
    pos: egui::Pos2,
) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }

    let top = rects
        .iter()
        .map(|(_, rect)| rect.top())
        .fold(f32::INFINITY, f32::min);

    let bottom = rects
        .iter()
        .map(|(_, rect)| rect.bottom())
        .fold(f32::NEG_INFINITY, f32::max);

    if pos.y < top - 8.0 || pos.y > bottom + 8.0 {
        return None;
    }

    for (index, (_, rect)) in rects.iter().enumerate() {
        if pos.x < rect.center().x {
            return Some(index);
        }
    }

    Some(rects.len())
}

fn paint_dock_insert_line(ui: &egui::Ui, rects: &[(PanelKind, egui::Rect)], index: usize) {
    if rects.is_empty() {
        return;
    }

    let x = if index >= rects.len() {
        rects.last().unwrap().1.right() + 3.0
    } else {
        rects[index].1.left() - 3.0
    };

    let top = rects
        .iter()
        .map(|(_, rect)| rect.top())
        .fold(f32::INFINITY, f32::min);

    let bottom = rects
        .iter()
        .map(|(_, rect)| rect.bottom())
        .fold(f32::NEG_INFINITY, f32::max);

    ui.painter().line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(2.0, theme::BLUE),
    );
}

fn paint_real_dock_hover(ctx: &egui::Context, rect: egui::Rect, label: &str) {
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(("real-dock-hover", label)),
    ));

    painter.rect_stroke(
        rect.shrink(2.0),
        4.0,
        egui::Stroke::new(2.0, theme::BLUE),
        egui::StrokeKind::Inside,
    );
}
