// Release 模式下不显示控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bootstrap;
mod commands;
mod config;
mod keymap;
mod panels_ops;
mod replay_task;
mod state;
mod tick;
mod ui;
pub(crate) use bootstrap::*;

use app::WorkbenchApp;
use eframe::egui;

const APP_ICON_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/app-icon-256.png"
));

/// 加载应用图标，用于运行时窗口标题栏。
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).expect("embedded app icon PNG should be valid")
}

fn main() -> eframe::Result<()> {
    // 启动时检查并应用待更新（在 eframe 启动前，exe 尚未被锁定）
    let exe_path = std::env::current_exe().unwrap_or_default();
    if let Ok(true) = tool_updater::apply_pending_update(&exe_path) {
        // 更新已应用，启动新版本后退出
        let _ = std::process::Command::new(&exe_path).spawn();
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("hardware-workbench")
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
            .with_icon(app_icon()),
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
