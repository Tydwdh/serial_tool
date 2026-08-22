//! 右上角临时通知覆盖层。
//!
//! 动画和交互思路参考 `egui-notify`（Copyright (c) 2022-2023 ItsEthra，MIT）。
//! 本实现为适配 egui 0.35 和本项目 `NotificationQueue` 的独立移植；完整许可见
//! 仓库根目录的 `THIRD_PARTY_NOTICES.md`。

use std::collections::VecDeque;

use eframe::egui::{self, Align, Align2, Frame, Id, Layout, Order, RichText, Stroke};
use egui_material_icons::icons::{ICON_CLOSE, ICON_ERROR, ICON_INFO, ICON_WARNING};

use crate::state::{NotificationQueue, StatusLevel};
use tool_panels::design;
use tool_panels::theme;

const MAX_TOASTS: usize = 5;
const TOAST_WIDTH: f32 = 340.0;
const VIEW_MARGIN: f32 = 14.0;
const TOAST_GAP: f32 = 8.0;
const ANIMATION_SECONDS: f32 = 0.18;

#[derive(Default)]
pub(crate) struct ToastOverlay {
    last_notification_id: u64,
    toasts: VecDeque<Toast>,
}

struct Toast {
    id: u64,
    level: StatusLevel,
    text: String,
    /// 使用绝对截止时间，窗口最小化、挂起或掉帧时倒计时仍按真实时间推进。
    deadline_ms: Option<u64>,
    lifetime_ms: Option<u64>,
    last_update_ms: u64,
    visibility: f32,
    dismissing: bool,
}

impl ToastOverlay {
    pub(crate) fn show(&mut self, ctx: &egui::Context, notifications: &mut NotificationQueue) {
        self.collect_new_notifications(notifications);

        let dt = ctx.input(|input| input.stable_dt).min(0.1);
        let focused = ctx.input(|input| input.viewport().focused.unwrap_or(true));
        let now_ms = tool_application::tool_core::now_timestamp_ms();
        let mut y = VIEW_MARGIN;
        let mut needs_repaint = false;

        for toast in &mut self.toasts {
            let elapsed_ms = now_ms.saturating_sub(toast.last_update_ms);
            toast.last_update_ms = now_ms;
            if toast.deadline_ms.is_some_and(|deadline| now_ms >= deadline) {
                toast.dismissing = true;
            }

            toast.visibility = if toast.dismissing {
                (toast.visibility - dt / ANIMATION_SECONDS).max(0.0)
            } else {
                (toast.visibility + dt / ANIMATION_SECONDS).min(1.0)
            };

            let slide = (1.0 - ease_out_cubic(toast.visibility)) * (TOAST_WIDTH + VIEW_MARGIN);
            let area = egui::Area::new(Id::new(("app-toast", toast.id)))
                .order(Order::Foreground)
                .anchor(Align2::RIGHT_TOP, egui::vec2(slide - VIEW_MARGIN, y));

            let shown = area.show(ctx, |ui| {
                ui.set_width(TOAST_WIDTH);
                render_toast(ui, toast, now_ms)
            });
            y += shown.response.rect.height() + TOAST_GAP;

            if shown.response.hovered() && focused && !toast.dismissing {
                // 正常绘制期间悬停会暂停倒计时。限制单帧补偿量，避免窗口最小化后
                // 恢复时因指针仍停在原位置而把整段后台时间错误地补回去。
                let hover_pause_ms = elapsed_ms.min(250);
                if let Some(deadline) = &mut toast.deadline_ms {
                    *deadline = deadline.saturating_add(hover_pause_ms);
                }
                needs_repaint = true;
            } else if toast.deadline_ms.is_some() && !toast.dismissing {
                needs_repaint = true;
            }

            if toast.visibility > 0.0 && (toast.dismissing || toast.visibility < 1.0) {
                needs_repaint = true;
            }
        }

        self.toasts
            .retain(|toast| !(toast.dismissing && toast.visibility <= 0.0));
        if needs_repaint {
            ctx.request_repaint();
        }
    }

    fn collect_new_notifications(&mut self, notifications: &mut NotificationQueue) {
        for notification in notifications.current() {
            if notification.id <= self.last_notification_id {
                continue;
            }
            self.last_notification_id = notification.id;
            let deadline_ms = notification.deadline_ms;
            let lifetime_ms = notification.level.ttl_ms();
            let now_ms = tool_application::tool_core::now_timestamp_ms();

            // 同样的通知重复到达时，只刷新原卡片及倒计时，避免连续错误堆叠。
            if let Some(toast) =
                self.toasts.iter_mut().rev().find(|toast| {
                    toast.level == notification.level && toast.text == notification.text
                })
            {
                toast.deadline_ms = deadline_ms;
                toast.lifetime_ms = lifetime_ms;
                toast.last_update_ms = now_ms;
                toast.dismissing = false;
                continue;
            }

            self.toasts.push_back(Toast {
                id: notification.id,
                level: notification.level,
                text: notification.text,
                deadline_ms,
                lifetime_ms,
                last_update_ms: now_ms,
                visibility: 0.0,
                dismissing: false,
            });
        }

        while self.toasts.len() > MAX_TOASTS {
            self.toasts.pop_front();
        }
    }
}

fn render_toast(ui: &mut egui::Ui, toast: &mut Toast, now_ms: u64) {
    let (accent, icon, title) = match toast.level {
        StatusLevel::Info => (theme::blue(), ICON_INFO, "提示"),
        StatusLevel::Warn => (theme::yellow(), ICON_WARNING, "警告"),
        StatusLevel::Error => (theme::red(), ICON_ERROR, "错误"),
    };
    let mut close = false;
    let frame = Frame::new()
        .fill(theme::bg_secondary())
        .stroke(Stroke::new(1.0, accent.gamma_multiply(0.7)))
        .corner_radius(7.0)
        .inner_margin(egui::Margin::symmetric(12, 10));

    frame.show(ui, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(design::icon_only(icon, accent, 18.0));
                ui.label(RichText::new(title).color(accent).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if design::icon_button(ui, ICON_CLOSE, "关闭通知").clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(3.0);
            ui.add(
                egui::Label::new(RichText::new(&toast.text).color(theme::text_primary())).wrap(),
            );

            if let Some(fraction) = toast.remaining_fraction(now_ms) {
                ui.add_space(8.0);
                let rect = ui.allocate_space(egui::vec2(ui.available_width(), 2.0)).1;
                ui.painter().rect_filled(rect, 1.0, theme::bg_tertiary());
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(rect.width() * fraction, rect.height()),
                    ),
                    1.0,
                    accent,
                );
            }
        });
    });
    if close {
        toast.dismissing = true;
    }
}

impl Toast {
    fn remaining_fraction(&self, now_ms: u64) -> Option<f32> {
        let deadline_ms = self.deadline_ms?;
        let lifetime_ms = self.lifetime_ms?.max(1);
        Some((deadline_ms.saturating_sub(now_ms) as f32 / lifetime_ms as f32).clamp(0.0, 1.0))
    }
}

fn ease_out_cubic(value: f32) -> f32 {
    1.0 - (1.0 - value).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StatusLevel;

    #[test]
    fn repeated_notifications_reset_the_existing_toast() {
        let mut queue = NotificationQueue::new();
        let mut overlay = ToastOverlay::default();
        queue.push("serial", StatusLevel::Error, "access denied");
        overlay.collect_new_notifications(&mut queue);
        queue.push("serial", StatusLevel::Error, "access denied");
        overlay.collect_new_notifications(&mut queue);

        assert_eq!(overlay.toasts.len(), 1);
        assert_eq!(overlay.toasts.back().unwrap().text, "access denied");
    }

    #[test]
    fn toast_progress_uses_the_absolute_deadline() {
        let toast = Toast {
            id: 1,
            level: StatusLevel::Info,
            text: "test".to_owned(),
            deadline_ms: Some(20_000),
            lifetime_ms: Some(10_000),
            last_update_ms: 10_000,
            visibility: 1.0,
            dismissing: false,
        };

        assert_eq!(toast.remaining_fraction(10_000), Some(1.0));
        assert_eq!(toast.remaining_fraction(15_000), Some(0.5));
        assert_eq!(toast.remaining_fraction(25_000), Some(0.0));
    }
}
