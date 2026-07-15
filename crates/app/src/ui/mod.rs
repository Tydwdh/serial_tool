mod activity_bar;
mod bottom_panel;
pub(crate) mod command_palette;
mod contributions;
mod device_panel;
pub(crate) mod dock;
mod layout_buttons;
pub(crate) mod popups;
mod settings_panel;
mod status_bar;
mod top_bar;

use crate::app::WorkbenchApp;
use crate::bootstrap::{ACTIVITY_BAR_WIDTH, BOTTOM_PANEL_MIN};
use crate::ui::dock::{DockResizeState, DockResizeTarget};
use eframe::egui;
use tool_panels::{DockArea, theme};

const RIGHT_DOCK_MIN: f32 = 220.0;

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.dock_drag.bottom_rect = None;
        self.dock_drag.right_rect = None;
        self.dock_drag.left_rect = None;
        self.dock_drag.bottom_tab_rects.clear();
        self.dock_drag.right_tab_rects.clear();

        egui::Panel::top("top-bar").show(ui, |ui| {
            self.top_bar(ui);
        });

        if self.panels.dock.activity_bar_visible {
            let bar = egui::Panel::left("activity-bar")
                .resizable(false)
                .exact_size(ACTIVITY_BAR_WIDTH)
                .show(ui, |ui| {
                    self.activity_bar(ui);
                });
            self.dock_drag.left_rect = Some(bar.response.rect);
        }

        // 固定状态栏：永远贴在窗口最底部，不参与 bottom-dock resize。
        egui::Panel::bottom("status-bar")
            .resizable(false)
            .exact_size(26.0)
            .show_separator_line(true)
            .show(ui, |ui| {
                self.status_bar(ui);
            });

        if self.panels.dock.right_visible {
            let shown = egui::Panel::right("right-dock-v2")
                .resizable(false)
                .exact_size(self.panels.dock.right_size.max(RIGHT_DOCK_MIN))
                .frame(egui::Frame::NONE)
                .show_separator_line(true)
                .show(ui, |ui| {
                    self.dock_stack_ui(ui, DockArea::Right);
                });

            self.dock_drag.right_rect = Some(shown.response.rect);
        }

        if self.panels.dock.bottom_visible {
            let shown = egui::Panel::bottom("bottom-dock-v3")
                .resizable(false)
                .exact_size(self.panels.dock.bottom_size.max(BOTTOM_PANEL_MIN))
                .frame(egui::Frame::NONE)
                .show_separator_line(false)
                .show(ui, |ui| {
                    self.dock_stack_ui(ui, DockArea::Bottom);
                });
            self.dock_drag.bottom_rect = Some(shown.response.rect);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG_DEEP))
            .show(ui, |ui| {
                egui::Frame::default()
                    .fill(theme::BG_DEEP)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        self.dock_stack_ui(ui, DockArea::Center);
                    });
            });

        let layout = dock::DockLayoutRects::from_drag(&self.dock_drag);
        self.paint_dock_drop_overlay(ctx);
        self.paint_bottom_dock_separator(ctx, &layout);
        self.dock_resize_overlays(ctx, &layout);
    }

    fn dock_resize_overlays(&mut self, ctx: &egui::Context, layout: &dock::DockLayoutRects) {
        if layout.bottom_rect != egui::Rect::NOTHING {
            self.bottom_dock_resize_handle(ctx, layout.bottom_resize);
        }
        if layout.right_rect != egui::Rect::NOTHING {
            self.right_dock_resize_handle(ctx, layout.right_resize);
        }
    }

    fn paint_bottom_dock_separator(&self, ctx: &egui::Context, layout: &dock::DockLayoutRects) {
        let rect = layout.bottom_separator;
        if rect.width() <= 1.0 {
            return;
        }

        // 用 Order::Middle 而非 Foreground：分隔线只需浮在普通面板之上，
        // 但必须低于 popup / 窗口等 Foreground 层，否则会盖住历史记录等弹出层。
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("bottom-dock-separator"),
        ));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top()),
            ],
            egui::Stroke::new(1.0, theme::SEPARATOR),
        );
    }

    fn bottom_dock_resize_handle(&mut self, ctx: &egui::Context, handle_rect: egui::Rect) {
        if handle_rect.width() <= 1.0 {
            return;
        }

        let area_id = egui::Id::new("bottom-dock-resize-overlay");

        egui::Area::new(area_id)
            .order(egui::Order::Middle)
            .fixed_pos(handle_rect.min)
            .movable(false)
            .show(ctx, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(handle_rect.size(), egui::Sense::drag());
                let paint_rect = egui::Rect::from_min_size(rect.min, handle_rect.size());
                let active = self
                    .dock_drag
                    .resize
                    .is_some_and(|state| state.target == DockResizeTarget::Bottom);
                let response = response
                    .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                    .on_hover_text("拖动调整底部面板高度");
                paint_dock_resize_handle(ui, paint_rect, response.hovered() || active, false);

                if response.drag_started()
                    && let Some(origin) = response.interact_pointer_pos()
                {
                    self.dock_drag.resize = Some(DockResizeState {
                        target: DockResizeTarget::Bottom,
                        origin,
                        start_size: self.panels.dock.bottom_size,
                    });
                }
                if let Some(state) = self.dock_drag.resize
                    && state.target == DockResizeTarget::Bottom
                {
                    let (primary_down, pointer_pos, viewport_h) = ui.ctx().input(|input| {
                        (
                            input.pointer.primary_down(),
                            input.pointer.interact_pos().or(input.pointer.hover_pos()),
                            input.viewport_rect().height(),
                        )
                    });
                    if primary_down {
                        if let Some(pos) = pointer_pos {
                            let max_h = (viewport_h - 120.0).max(BOTTOM_PANEL_MIN);
                            self.panels.dock.bottom_size = (state.start_size
                                - (pos.y - state.origin.y))
                                .clamp(BOTTOM_PANEL_MIN, max_h);
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                            ui.ctx().request_repaint();
                        }
                    } else {
                        self.dock_drag.resize = None;
                    }
                }
            });
    }

    fn right_dock_resize_handle(&mut self, ctx: &egui::Context, handle_rect: egui::Rect) {
        if handle_rect.height() <= 1.0 {
            return;
        }

        let area_id = egui::Id::new("right-dock-resize-overlay");

        egui::Area::new(area_id)
            .order(egui::Order::Middle)
            .fixed_pos(handle_rect.min)
            .movable(false)
            .show(ctx, |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(handle_rect.size(), egui::Sense::drag());
                let paint_rect = egui::Rect::from_min_size(rect.min, handle_rect.size());
                let active = self
                    .dock_drag
                    .resize
                    .is_some_and(|state| state.target == DockResizeTarget::Right);
                let response = response
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
                    .on_hover_text("拖动调整右侧面板宽度");
                paint_dock_resize_handle(ui, paint_rect, response.hovered() || active, true);

                if response.drag_started()
                    && let Some(origin) = response.interact_pointer_pos()
                {
                    self.dock_drag.resize = Some(DockResizeState {
                        target: DockResizeTarget::Right,
                        origin,
                        start_size: self.panels.dock.right_size,
                    });
                }
                if let Some(state) = self.dock_drag.resize
                    && state.target == DockResizeTarget::Right
                {
                    let (primary_down, pointer_pos, viewport_w) = ui.ctx().input(|input| {
                        (
                            input.pointer.primary_down(),
                            input.pointer.interact_pos().or(input.pointer.hover_pos()),
                            input.viewport_rect().width(),
                        )
                    });
                    if primary_down {
                        if let Some(pos) = pointer_pos {
                            let max_w =
                                (viewport_w - ACTIVITY_BAR_WIDTH - 240.0).max(RIGHT_DOCK_MIN);
                            self.panels.dock.right_size = (state.start_size
                                - (pos.x - state.origin.x))
                                .clamp(RIGHT_DOCK_MIN, max_w);
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            ui.ctx().request_repaint();
                        }
                    } else {
                        self.dock_drag.resize = None;
                    }
                }
            });
    }
}

fn paint_dock_resize_handle(ui: &egui::Ui, rect: egui::Rect, active: bool, vertical: bool) {
    if !active {
        return;
    }

    let line = if vertical {
        egui::Rect::from_center_size(rect.center(), egui::vec2(1.0, rect.height()))
    } else {
        egui::Rect::from_center_size(rect.center(), egui::vec2(rect.width(), 1.0))
    };
    ui.painter().rect_filled(line, 0.0, theme::SEPARATOR_STRONG);
}

pub(crate) fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    // 常用档位（含高速）：点选直接设置。不在列表的值（如自定义 3_000_000）
    // 仍显示在 selected_text，下方 TextEdit 提供自由输入入口。
    let rates = [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600", "128000",
        "256000", "512000", "1000000", "2000000", "3000000",
    ];

    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(baud.clone())
        .show_ui(ui, |ui| {
            for rate in rates {
                ui.selectable_value(baud, rate.to_owned(), rate);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("自定义");
                ui.add(
                    egui::TextEdit::singleline(baud)
                        .desired_width(80.0)
                        .hint_text("bps"),
                );
            });
        });
}
