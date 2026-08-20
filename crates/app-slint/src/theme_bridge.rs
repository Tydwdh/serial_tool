use std::path::{Path, PathBuf};

use crate::AppWindow;

/// P1 阶段：调色板先由 Slint 端默认值驱动，Rust 动态写入在 P2 再完整打通
/// 保留函数签名以兼容 main.rs 定时调用，当前为 no-op。
pub fn apply_palette_from_panels(_window: &AppWindow) {
    // TODO(P2): 通过 AppColors global 动态写入 tool-panels 主题色
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
