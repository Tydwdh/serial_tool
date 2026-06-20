// Release 模式下不显示控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bootstrap;
mod commands;
mod config;
mod panels_ops;
mod replay_task;
mod state;
mod tick;
mod ui;
pub(crate) use bootstrap::*;

use app::WorkbenchApp;
use eframe::egui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("hardware-workbench")
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT]),
        vsync: true,
        persist_window: true,
        // 阻止 persist_window 恢复最大化：winit 在 Windows 上最大化窗口
        // 会导致短暂消失再出现。保留位置和大小记忆，最大化由用户手动触发。
        window_builder: Some(Box::new(|builder| builder.with_maximized(false))),

        ..Default::default()
    };
    eframe::run_native(
        "硬件调试工作台",
        options,
        Box::new(|cc| Ok(Box::new(WorkbenchApp::new(cc)))),
    )
}
