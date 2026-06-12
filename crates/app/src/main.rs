// Release 模式下不显示控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(test)]
mod tests;

mod app;
mod bootstrap;
mod commands;
mod config;
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
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
            .with_min_inner_size([960.0, 640.0]),
        persist_window: true,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode: eframe::wgpu::PresentMode::Immediate,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "硬件调试工作台",
        options,
        Box::new(|cc| Ok(Box::new(WorkbenchApp::new(cc)))),
    )
}
