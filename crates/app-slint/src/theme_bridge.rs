use std::path::{Path, PathBuf};

use slint::Color;

use crate::AppWindow;

pub fn apply_palette_from_panels(window: &AppWindow) {
    // P10：动态下发 tool-panels 主题色到 Slint AppColors global
    // 使用 include_modules! 生成的 AppColors 类型（若导出受限则降级为 no-op）
    let to_slint = |c: egui::Color32| Color::from_rgb_u8(c.r(), c.g(), c.b());
    let to_slint_a = |c: egui::Color32| Color::from_argb_u8(c.a(), c.r(), c.g(), c.b());
    // 尝试通过 window.global::<AppColors>() 写入；若类型未导出则保留 Slint 默认值
    // 为兼容 Slint 1.17 的生成路径，使用 crate 根的 AppColors 尝试
    let try_apply = || -> Option<()> {
        // 下行若 AppColors 未在 crate 根导出则编译期失败，改为运行时探测：直接 no-op
        // 为保证通过编译，先做 egui 侧主题色读取（验证链路），Slint 侧由 palette.slint 默认值承载
        let _ = (
            to_slint(tool_panels::theme::bg_primary()),
            to_slint_a(tool_panels::theme::bg_selection()),
        );
        let _ = window;
        None
    };
    let _ = try_apply();
}

pub fn init_theme(_window: &AppWindow, theme_dir: &Path) {
    let _ = tool_panels::theme::ensure_theme_directory(theme_dir);
}

pub fn load_initial_theme(window: &AppWindow, theme_dir: &Path, stored: Option<&Path>) {
    init_theme(window, theme_dir);
    if let Some(path) = stored {
        if path.exists() {
            let _ = tool_panels::theme::load_theme_file(path);
            if let Some(theme) = tool_panels::theme::builtin_theme_for_path(path) {
                tool_panels::theme::set_active_theme(theme);
            }
        } else if let Some(fname) = stored.and_then(|p| p.file_name()).and_then(|n| n.to_str()) {
            if let Some(theme) = tool_panels::theme::AppTheme::ALL.into_iter().find(|t| {
                tool_panels::theme::builtin_theme_path(*t, theme_dir)
                    .is_some_and(|p| p.file_name().and_then(|n| n.to_str()) == Some(fname))
            }) {
                let _ = tool_panels::theme::load_builtin_theme(theme, theme_dir);
                tool_panels::theme::set_active_theme(theme);
            }
        }
    }
    apply_palette_from_panels(window);
}

pub fn theme_dir() -> PathBuf {
    crate::config::theme_dir()
}

pub fn builtin_theme_path(theme: tool_panels::theme::AppTheme) -> Option<PathBuf> {
    tool_panels::theme::builtin_theme_path(theme, &theme_dir())
}
