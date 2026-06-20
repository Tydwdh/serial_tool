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

/// 生成应用图标（程序化 32x32 RGBA，芯片/终端风格）。
fn app_icon() -> egui::IconData {
    // 简单的硬件芯片图标：深色背景 + 中央矩形 + 引脚
    let size = 32;
    let mut rgba = vec![0u8; size * size * 4];

    let set_pixel = |rgba: &mut Vec<u8>, x: usize, y: usize, r: u8, g: u8, b: u8, a: u8| {
        let idx = (y * size + x) * 4;
        rgba[idx] = r;
        rgba[idx + 1] = g;
        rgba[idx + 2] = b;
        rgba[idx + 3] = a;
    };

    let bg = (28, 32, 38); // 深色背景
    let chip = (70, 130, 190); // 芯片蓝色
    let pin = (145, 154, 168); // 引脚灰色
    let accent = (137, 180, 108); // 绿色点缀

    // 背景
    for y in 0..size {
        for x in 0..size {
            set_pixel(&mut rgba, x, y, bg.0, bg.1, bg.2, 255);
        }
    }

    // 芯片主体 (8..24, 8..24)
    for y in 8..24 {
        for x in 8..24 {
            set_pixel(&mut rgba, x, y, chip.0, chip.1, chip.2, 255);
        }
    }

    // 芯片内部细节
    for y in 12..20 {
        for x in 12..20 {
            let shade = if (x + y) % 2 == 0 { 50 } else { 90 };
            set_pixel(&mut rgba, x, y, shade, shade + 60, shade + 110, 255);
        }
    }

    // 左侧引脚
    for y in &[10, 13, 16, 19, 22] {
        for x in 4..8 {
            set_pixel(&mut rgba, x, *y, pin.0, pin.1, pin.2, 255);
        }
    }
    // 右侧引脚
    for y in &[10, 13, 16, 19, 22] {
        for x in 24..28 {
            set_pixel(&mut rgba, x, *y, pin.0, pin.1, pin.2, 255);
        }
    }
    // 顶部引脚
    for x in &[10, 13, 16, 19, 22] {
        for y in 4..8 {
            set_pixel(&mut rgba, *x, y, pin.0, pin.1, pin.2, 255);
        }
    }
    // 底部引脚
    for x in &[10, 13, 16, 19, 22] {
        for y in 24..28 {
            set_pixel(&mut rgba, *x, y, pin.0, pin.1, pin.2, 255);
        }
    }

    // 绿色 LED 点
    set_pixel(&mut rgba, 10, 10, accent.0, accent.1, accent.2, 255);
    set_pixel(&mut rgba, 21, 21, accent.0, accent.1, accent.2, 255);

    egui::IconData {
        rgba,
        width: size as _,
        height: size as _,
    }
}

fn main() -> eframe::Result<()> {
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
