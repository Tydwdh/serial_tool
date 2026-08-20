#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod state;
mod theme_bridge;

slint::include_modules!();

use std::sync::Arc;

use slint::TimerMode;

fn main() -> Result<(), slint::PlatformError> {
    let _ = env_logger::try_init();

    let app_state = Arc::new(app::AppState::load());
    let theme_dir = app_state.theme_dir.clone();
    let stored_theme = app_state.theme_path();

    let window = AppWindow::new()?;

    // 主题：初始化 + 写入 Palette global
    theme_bridge::load_initial_theme(&window, &theme_dir, stored_theme.as_deref());

    // 状态栏：定时轮询 NotificationQueue（TTL 5/8/15s）
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        let timer = slint::Timer::default();
        timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                if let Some(w) = win_weak.upgrade() {
                    let (text, level) = app::poll_status_text(&st);
                    w.set_status_text(text.into());
                    w.set_status_level(level.into());
                    theme_bridge::apply_palette_from_panels(&w);
                }
            },
        );
        std::mem::forget(timer);
    }

    // TX 模拟：计数 + 回显 + 通知
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        window.on_tx_send(move |text| {
            if let Some(w) = win_weak.upgrade() {
                let trimmed = text.trim().to_string();
                let count = w.get_launch_count() + 1;
                w.set_launch_count(count);
                if trimmed.is_empty() {
                    st.push_status(state::StatusLevel::Warn, format!("第 {count} 次：输入为空"));
                    w.set_rx_preview(
                        format!("{}\n[提示 {count}] 输入为空，未发送", w.get_rx_preview()).into(),
                    );
                } else {
                    st.push_status(
                        state::StatusLevel::Info,
                        format!("第 {count} 次发送(模拟)：{trimmed}"),
                    );
                    w.set_rx_preview(
                        format!("{}\n[TX 模拟 {count}] {trimmed}", w.get_rx_preview()).into(),
                    );
                }
                w.set_log_preview(format!("模拟发送 #{count}").into());
            }
        });
    }

    // 打开配置目录
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        window.on_open_config_folder(move || {
            let dir = crate::config::config_path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| theme_dir.clone());
            let msg = if dir.exists() {
                format!("配置目录：{}", dir.display())
            } else {
                format!("配置目录（尚未创建）：{}", dir.display())
            };
            st.push_status(state::StatusLevel::Info, msg.clone());
            if let Some(w) = win_weak.upgrade() {
                w.set_status_text(msg.into());
            }
            let _ = open::that(&dir);
        });
    }

    // 主题切换（P1：下拉选择内置/自定义主题并持久化）
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        window.on_change_theme(move |theme_file| {
            let path = if theme_file.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(theme_file.to_string()))
            };
            if let Some(p) = path.as_deref() {
                if p.exists() {
                    let _ = tool_panels::theme::load_theme_file(p);
                    if let Some(t) = tool_panels::theme::builtin_theme_for_path(p) {
                        tool_panels::theme::set_active_theme(t);
                    }
                    let rel = p
                        .strip_prefix(&st.theme_dir)
                        .unwrap_or(p)
                        .display()
                        .to_string();
                    let mut cfg = st.build_snapshot();
                    cfg.theme_path = Some(rel);
                    let _ = crate::config::save_config_snapshot(&cfg);
                    st.push_status(state::StatusLevel::Info, format!("已切换主题：{}", p.display()));
                }
            }
            if let Some(w) = win_weak.upgrade() {
                theme_bridge::apply_palette_from_panels(&w);
            }
        });
    }

    if app_state.config_migrated {
        let _ = app_state.save();
    }

    window.run()
}
