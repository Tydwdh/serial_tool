//! 主循环 tick 逻辑：把原 `tick.rs` 按职责拆成多个子模块。
//!
//! `tick_pre_ui` / `tick_post_ui` 是两个编排入口（在 [crate::app] 的 `eframe::App::update`
//! 中调用），按顺序调度各子模块的 `tick_*` 方法。各子模块仍以 `impl WorkbenchApp`
//! 的 inherent impl 形式分散实现，Rust 允许 inherent impl 跨文件。

mod autosave;
mod keys;
mod periodic_send;
mod plugin;
mod port_refresh;
mod replay;
mod timing;
mod update;

use crate::app::WorkbenchApp;
use eframe::egui;

impl WorkbenchApp {
    pub(crate) fn tick_pre_ui(&mut self, ctx: &egui::Context) {
        self.clear_status_if_expired();
        self.tick_recorder_status();
        self.tick_terminal_maximize();
        self.tick_replay(ctx);
        self.tick_plugin_lifecycle();
        self.handle_keys(ctx);
        self.flush_pending_action();
        self.tick_key_recording(ctx);
        self.tick_port_refresh(ctx);
        self.tick_periodic_send(ctx);
        self.tick_auto_save(ctx);
        self.tick_update();
    }

    pub(crate) fn tick_post_ui(&mut self, ctx: &egui::Context) {
        self.bottom_log_panel.ingest_pending();
        self.detached_dynamic_panel_viewports(ctx);
        self.send_popup(ctx);
        self.terminal_popup(ctx);
    }
}
