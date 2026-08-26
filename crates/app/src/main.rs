// Release 模式下不显示控制台窗口
#![cfg_attr(
    all(windows, not(target_arch = "wasm32")),
    windows_subsystem = "windows"
)]

#[cfg(not(target_arch = "wasm32"))]
mod app;
mod bootstrap;
#[cfg(not(target_arch = "wasm32"))]
mod command_registry;
#[cfg(not(target_arch = "wasm32"))]
mod commands;
#[cfg(not(target_arch = "wasm32"))]
mod config;
#[cfg(not(target_arch = "wasm32"))]
mod keymap;
mod panel_registry;
#[cfg(not(target_arch = "wasm32"))]
mod panels_ops;
#[cfg(not(target_arch = "wasm32"))]
mod perf;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod replay_task;
#[cfg(not(target_arch = "wasm32"))]
mod runtime;
mod shared_keymap;
mod shared_settings;
mod shared_shell;
#[cfg(not(target_arch = "wasm32"))]
mod state;
#[cfg(not(target_arch = "wasm32"))]
mod ui;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
mod web_perf;
#[cfg(target_arch = "wasm32")]
mod web_plugin_host;
mod workbench_app;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use bootstrap::*;

#[cfg(not(target_arch = "wasm32"))]
use app::WorkbenchApp;
#[cfg(not(target_arch = "wasm32"))]
use eframe::egui;

#[cfg(not(target_arch = "wasm32"))]
const APP_ICON_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/app-icon-256.png"
));

/// 加载应用图标，用于运行时窗口标题栏。
#[cfg(not(target_arch = "wasm32"))]
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(APP_ICON_PNG).expect("embedded app icon PNG should be valid")
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    {
        let mut args = std::env::args_os();
        let _program = args.next();
        if args.next().as_deref() == Some(std::ffi::OsStr::new("--apply-pending-update")) {
            let Some(target_exe) = args.next() else {
                eprintln!("missing target exe for --apply-pending-update");
                std::process::exit(2);
            };
            if let Err(error) = tool_updater::run_update_helper(std::path::Path::new(&target_exe)) {
                eprintln!("{error}");
                std::process::exit(2);
            }
            return Ok(());
        }
    }

    if std::env::args().any(|arg| arg == "--check-update-once") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");
        match rt.block_on(tool_updater::update_info::fetch_update_info(
            tool_updater::UPDATE_JSON_URL,
        )) {
            Ok(info) => {
                println!(
                    "update check ok: version={} date={} download_url={}",
                    info.version, info.date, info.download_url
                );
                return Ok(());
            }
            Err(err) => {
                eprintln!("update check failed: {err}");
                std::process::exit(2);
            }
        }
    }

    #[cfg(windows)]
    {
        // 启动时检查并应用待更新（在 eframe 启动前，exe 尚未被锁定）
        let exe_path = std::env::current_exe().unwrap_or_default();
        match tool_updater::apply_pending_update(&exe_path) {
            Ok(true) => {
                // 兼容旧版遗留标记：更新已应用，启动新版本后退出
                let _ = std::process::Command::new(&exe_path).spawn();
                return Ok(());
            }
            Ok(false) => {}
            Err(error) => eprintln!("apply pending update failed: {error}"),
        }
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("hardware-workbench")
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
            .with_icon(app_icon()),
        persist_window: true,
        // 阻止 persist_window 恢复最大化：winit 在 Windows 上最大化窗口
        // 会导致短暂消失再出现。保留位置和大小记忆，最大化由用户手动触发。
        window_builder: Some(Box::new(|builder| builder.with_maximized(false))),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "硬件调试工作台",
        options,
        Box::new(|cc| Ok(Box::new(WorkbenchApp::new(cc)))),
    )
}

/// The browser entry point is exported from [`web`] and called by the small
/// JavaScript bootstrap after the canvas has been inserted into the page.
#[cfg(target_arch = "wasm32")]
fn main() {}
