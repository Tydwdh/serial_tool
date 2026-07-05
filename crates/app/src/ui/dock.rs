use eframe::egui;
use tool_panels::{DockArea, PanelKind, theme};

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;

/// 停靠区标签拖拽状态：跨区域移动面板时的临时拖拽数据。
#[derive(Default)]
pub(crate) struct DockDragState {
    /// 当前正在拖拽的面板种类。None 表示没有拖拽进行中。
    pub(crate) dragging_panel: Option<PanelKind>,
    /// 本帧各停靠区的屏幕矩形（用于碰撞检测拖拽落点）。
    pub(crate) bottom_rect: Option<egui::Rect>,
    pub(crate) right_rect: Option<egui::Rect>,
    pub(crate) left_rect: Option<egui::Rect>,
    /// 本帧各停靠区标签栏中每个标签的屏幕矩形（用于计算插入位置）。
    pub(crate) bottom_tab_rects: Vec<(PanelKind, egui::Rect)>,
    pub(crate) right_tab_rects: Vec<(PanelKind, egui::Rect)>,
}

impl WorkbenchApp {
    pub(super) fn dock_stack_ui(&mut self, ui: &mut egui::Ui, area: DockArea) {
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
            if area == DockArea::Center {
                self.panels.sync_active_tab_from_center();
            }
            self.dock_panel_body(ui, area, kind);
        }
    }
    fn dock_tab_bar(&mut self, ui: &mut egui::Ui, area: DockArea, tabs: &[PanelKind]) {
        let pointer = ui.ctx().pointer_latest_pos();
        let mut tab_rects: Vec<(PanelKind, egui::Rect)> = Vec::with_capacity(tabs.len());

        ui.horizontal(|ui| {
            for kind in tabs {
                let active = self.panels.dock.stack(area).active.as_ref() == Some(kind);
                let dragging = self.dock_drag.dragging_panel.as_ref() == Some(kind);
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
                    self.dock_drag.dragging_panel = Some(kind.clone());
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
                // 活动标签顶部绘制 2px 强调线：让 active/inactive 不止依赖背景色、
                // 对色觉障碍用户也更友好。
                if active {
                    let accent_rect = egui::Rect::from_min_size(
                        rect.left_top(),
                        egui::vec2(rect.width(), 2.0),
                    );
                    ui.painter()
                        .rect_filled(accent_rect, 0.0, theme::BLUE);
                }
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
                            if ui.button("移到主工作区").clicked() {
                                self.panels.dock.move_panel(kind.clone(), DockArea::Center);
                                self.panels.sync_tabs_from_dock();
                                if let Err(e) = self.save_config() {
                                    log::warn!("save_config failed: {e}")
                                };
                                ui.close();
                            }
                            if ui.button("移到右侧").clicked() {
                                self.panels.dock.move_panel(kind.clone(), DockArea::Right);
                                self.panels.dock.right_visible = true;
                                self.panels.sync_tabs_from_dock();
                                if let Err(e) = self.save_config() {
                                    log::warn!("save_config failed: {e}")
                                };
                                ui.close();
                            }
                        }
                        DockArea::Right => {
                            if ui.button("移到主工作区").clicked() {
                                self.panels.dock.move_panel(kind.clone(), DockArea::Center);
                                self.panels.sync_tabs_from_dock();
                                if let Err(e) = self.save_config() {
                                    log::warn!("save_config failed: {e}")
                                };
                                ui.close();
                            }
                            if ui.button("移到底部").clicked() {
                                self.panels.dock.move_panel(kind.clone(), DockArea::Bottom);
                                self.panels.sync_tabs_from_dock();
                                self.set_bottom_visible(true);
                                if let Err(e) = self.save_config() {
                                    log::warn!("save_config failed: {e}")
                                };
                                ui.close();
                            }
                        }
                        DockArea::Center => {}
                    }

                    if ui.button("关闭").clicked() {
                        self.panels.dock.stack_mut(area).close(kind);
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
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
                            if let Err(e) = self.save_config() {
                                log::warn!("save_config failed: {e}")
                            };
                        }
                    }
                    DockArea::Right => {
                        if ui
                            .small_button("×")
                            .on_hover_text("隐藏右侧停靠区")
                            .clicked()
                        {
                            self.panels.dock.right_visible = false;
                            if let Err(e) = self.save_config() {
                                log::warn!("save_config failed: {e}")
                            };
                        }
                    }
                    DockArea::Center => {}
                },
            );
        });

        let insert_index = if self.dock_drag.dragging_panel.is_some() {
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
            && let Some(kind) = self.dock_drag.dragging_panel.clone()
            && self.panels.dock.stack(area).contains(&kind)
            && let Some(insert_index) = insert_index
        {
            self.dock_drag.dragging_panel = None;

            if self
                .panels
                .dock
                .stack_mut(area)
                .reorder(&kind, insert_index)
                && let Err(e) = self.save_config()
            {
                log::warn!("save_config failed: {e}")
            };
        }

        // 保存本帧的标签矩形，供 paint_dock_drop_overlay 跨区拖拽时计算插入位置。
        match area {
            DockArea::Bottom => self.dock_drag.bottom_tab_rects = tab_rects,
            DockArea::Right => self.dock_drag.right_tab_rects = tab_rects,
            DockArea::Center => {}
        }
    }
    fn dock_panel_body(&mut self, ui: &mut egui::Ui, area: DockArea, kind: PanelKind) {
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
                        if ui.button("关闭悬浮窗口并回到底部").clicked() {
                            self.popups.terminal_open = false;
                        }
                    });
                } else {
                    self.terminal_panel.ui(ui);
                }
            }
            PanelKind::Sender => match area {
                DockArea::Right => self.send_panel_vertical(ui),
                DockArea::Bottom | DockArea::Center => self.send_panel_horizontal(ui),
            },
            PanelKind::Logs => self.bottom_log_panel.ui(ui),
            PanelKind::Dynamic(id) => {
                if self.detached_dynamic_panels.contains(&id) {
                    ui.label("已弹出到独立窗口");
                } else if self.dynamic_panels.contains(&id) {
                    egui::ScrollArea::vertical()
                        .show(ui, |ui| self.dynamic_panels.ui_body(ui, &id));
                } else {
                    ui.colored_label(theme::RED, format!("动态面板不存在：{id}"));
                }
            }
        }
    }

    /// 处理插件面板产生的事件（状态反馈 / 刷新市场 / 安装插件）。
    fn handle_plugin_panel_events(&mut self, events: Vec<tool_panels::PluginPanelEvent>) {
        for event in events {
            use tool_panels::PluginPanelEvent as Ev;
            match event {
                Ev::Status(msg, is_error) => {
                    let level = if is_error {
                        StatusLevel::Error
                    } else {
                        StatusLevel::Info
                    };
                    self.set_status_force(level, msg);
                }
                Ev::RefreshMarket => {
                    self.start_marketplace_refresh();
                }
                Ev::InstallPlugin(id) => {
                    // 从面板缓存的 registry 中查找对应条目，转交后台安装。
                    match self.plugins_panel.find_market_plugin(&id) {
                        Some(entry) => self.start_marketplace_install(entry),
                        None => {
                            self.set_status(
                                StatusLevel::Warn,
                                format!("找不到插件 {id} 的市场条目，请先刷新市场"),
                            );
                        }
                    }
                }
                Ev::UninstallPlugin(id) => {
                    self.uninstall_plugin(&id);
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

    pub(super) fn panel_title(&self, kind: &PanelKind) -> String {
        match kind {
            PanelKind::Dynamic(id) => self.dynamic_panels.title(id).unwrap_or(id).to_owned(),
            _ => kind.title(),
        }
    }

    pub(super) fn paint_dock_drop_overlay(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.dock_drag.dragging_panel.clone() else {
            return;
        };

        let primary_down = ctx.input(|i| i.pointer.primary_down());
        let released = ctx.input(|i| i.pointer.any_released());
        let esc_pressed = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        // ESC 取消拖拽
        if esc_pressed {
            self.dock_drag.dragging_panel = None;
            return;
        }

        let Some(pos) = ctx.pointer_latest_pos() else {
            if !primary_down {
                self.dock_drag.dragging_panel = None;
            }
            return;
        };

        let left_hit = self.dock_drag.left_rect.is_some_and(|rect| rect.contains(pos));

        let right_hit = self.dock_drag.right_rect.is_some_and(|rect| rect.contains(pos));

        let bottom_hit = self.dock_drag.bottom_rect.is_some_and(|rect| rect.contains(pos));

        if left_hit {
            if let Some(rect) = self.dock_drag.left_rect {
                paint_real_dock_hover(ctx, rect, "主工作区");
            }
        } else if right_hit {
            if let Some(rect) = self.dock_drag.right_rect {
                paint_real_dock_hover(ctx, rect, "右侧");
            }
            // 在标签栏中绘制插入线
            let rects = &self.dock_drag.right_tab_rects;
            if let Some(insert_idx) = horizontal_insert_index_from_pointer(rects, pos) {
                paint_dock_insert_line_at(ctx, rects, insert_idx, false);
            }
        } else if bottom_hit && let Some(rect) = self.dock_drag.bottom_rect {
            paint_real_dock_hover(ctx, rect, "底部");
            // 在标签栏中绘制插入线
            let rects = &self.dock_drag.bottom_tab_rects;
            if let Some(insert_idx) = horizontal_insert_index_from_pointer(rects, pos) {
                paint_dock_insert_line_at(ctx, rects, insert_idx, false);
            }
        }

        if released {
            if left_hit {
                self.panels.dock.move_panel(kind, DockArea::Center);
                self.panels.sync_tabs_from_dock();
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            } else if right_hit {
                // 根据指针在目标标签栏中的位置计算插入索引（非末尾追加）
                let rects = &self.dock_drag.right_tab_rects;
                let index = horizontal_insert_index_from_pointer(rects, pos)
                    .unwrap_or(rects.len());
                self.panels.dock.insert_panel_at(kind, DockArea::Right, index);
                self.panels.dock.right_visible = true;
                self.panels.sync_tabs_from_dock();
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            } else if bottom_hit {
                // 根据指针在目标标签栏中的位置计算插入索引（非末尾追加）
                let rects = &self.dock_drag.bottom_tab_rects;
                let index = horizontal_insert_index_from_pointer(rects, pos)
                    .unwrap_or(rects.len());
                self.panels.dock.insert_panel_at(kind, DockArea::Bottom, index);
                self.panels.sync_tabs_from_dock();
                self.set_bottom_visible(true);
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            }

            self.dock_drag.dragging_panel = None;
        } else if !primary_down {
            self.dock_drag.dragging_panel = None;
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

fn paint_dock_insert_line_at(
    ctx: &egui::Context,
    rects: &[(PanelKind, egui::Rect)],
    index: usize,
    is_vertical: bool,
) {
    if rects.is_empty() {
        return;
    }

    let (x, top, bottom) = if is_vertical {
        let y = if index >= rects.len() {
            rects.last().expect("rects is non-empty").1.bottom() + 3.0
        } else {
            rects[index].1.top() - 3.0
        };
        let left = rects.iter().map(|(_, r)| r.left()).fold(f32::INFINITY, f32::min);
        let right = rects.iter().map(|(_, r)| r.right()).fold(f32::NEG_INFINITY, f32::max);
        (left, y, right)
    } else {
        let x = if index >= rects.len() {
            rects.last().expect("rects is non-empty").1.right() + 3.0
        } else {
            rects[index].1.left() - 3.0
        };
        let top = rects.iter().map(|(_, r)| r.top()).fold(f32::INFINITY, f32::min);
        let bottom = rects.iter().map(|(_, r)| r.bottom()).fold(f32::NEG_INFINITY, f32::max);
        (x, top, bottom)
    };

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new(("dock-insert-line", index)),
    ));

    painter.line_segment(
        [egui::pos2(x, top), egui::pos2(x, bottom)],
        egui::Stroke::new(2.0, theme::BLUE),
    );
}

fn paint_dock_insert_line(ui: &egui::Ui, rects: &[(PanelKind, egui::Rect)], index: usize) {
    if rects.is_empty() {
        return;
    }

    let x = if index >= rects.len() {
        // SAFETY: rects is non-empty (guarded above), and last() always returns Some for non-empty vecs
        rects.last().expect("rects is non-empty").1.right() + 3.0
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
