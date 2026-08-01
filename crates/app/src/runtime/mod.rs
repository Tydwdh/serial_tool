//! 主循环 tick 逻辑：把原 `tick.rs` 按职责拆成多个子模块。
//!
//! `tick_pre_ui` / `tick_post_ui` 是两个编排入口（在 [crate::app] 的 `eframe::App::update`
//! 中调用），按顺序调度各子模块的 `tick_*` 方法。各子模块仍以 `impl WorkbenchApp`
//! 的 inherent impl 形式分散实现，Rust 允许 inherent impl 跨文件。

mod autosave;
mod keys;
pub(crate) mod marketplace;
pub(crate) mod periodic_send;
mod plugin;
mod port_refresh;
mod replay;
mod timing;
mod update;

use crate::app::WorkbenchApp;
use eframe::egui;

impl WorkbenchApp {
    pub(crate) fn tick_pre_ui(&mut self, ctx: &egui::Context) {
        // 帧级缓存重置：每帧 UI 构建前清空 plugin_summaries_cache，使其在首次
        // ui_contribution_slot 调用时重新计算，同帧后续调用复用。
        self.plugin_summaries_cache = std::cell::OnceCell::new();

        // 通知队列在 status_bar 渲染时自动清理过期消息。
        self.tick_recorder_status();
        self.tick_replay(ctx);
        self.tick_plugin_lifecycle();
        self.sync_marketplace_installed_ids();
        self.handle_keys(ctx);
        self.flush_pending_action();
        self.tick_key_recording(ctx);
        self.tick_port_refresh(ctx);
        self.tick_periodic_send(ctx);
        self.tick_auto_save(ctx);
        self.tick_update();
        self.tick_marketplace();
    }

    pub(crate) fn tick_post_ui(&mut self, ctx: &egui::Context) {
        self.bottom_log_panel.ingest_pending();
        self.process_ui_set_status();
        self.command_palette(ctx);
    }

    /// 处理 Lua 插件通过 ctx.ui.set_status() 推送的状态栏通知。
    fn process_ui_set_status(&mut self) {
        for event in self.ui_set_status_subscription.drain_limited(32) {
            if let tool_core::Payload::Json(payload) = event.payload
                && let Some(msg) = payload.get("message").and_then(|v| v.as_str())
            {
                self.notifications
                    .push("plugin", crate::state::StatusLevel::Warn, msg.to_owned());
            }
        }
    }
}
