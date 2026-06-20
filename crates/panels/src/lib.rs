mod attitude;
mod chart;
mod dynamic;
mod log;
mod manager;
mod plugins;
mod replay;
mod sender;
mod terminal;
pub mod theme;

pub use attitude::AttitudePanel;
pub use chart::ChartPanel;
pub use dynamic::DynamicPanels;
pub use log::LogPanel;
pub use manager::{Activity, DockArea, DockLayout, DockStack, PanelKind, PanelManager};
pub use plugins::PluginsPanel;
pub use replay::ReplayPanel;
pub use sender::SenderPanel;
pub use terminal::TerminalPanel;

/// 将毫秒时间戳格式化为本地时间 HH:MM:SS.mmm
pub fn fmt_ts(ms: u64) -> String {
    let Some(dt_utc) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64) else {
        return "--:--:--.---".to_owned();
    };
    dt_utc
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S%.3f")
        .to_string()
}

/// 将 LogLevel 映射为 egui 颜色
pub fn level_color(level: tool_core::LogLevel) -> egui::Color32 {
    use tool_core::LogLevel;
    match level {
        LogLevel::Trace => egui::Color32::from_rgb(128, 128, 128),
        LogLevel::Debug => egui::Color32::from_rgb(100, 149, 237),
        LogLevel::Info => egui::Color32::from_rgb(144, 238, 144),
        LogLevel::Warn => egui::Color32::from_rgb(255, 215, 0),
        LogLevel::Error => egui::Color32::from_rgb(255, 99, 71),
    }
}

/// 计算面板行高
pub fn row_height(ui: &egui::Ui) -> f32 {
    let text_style_height = ui.text_style_height(&egui::TextStyle::Monospace);
    (text_style_height.ceil() + 6.0).max(20.0)
}

/// 截断字符串中间，用 "..." 连接
pub fn compact_middle(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let head = max_chars / 2;
    let tail = max_chars - head - 1; // -1 for the "…"
    let mut out = String::with_capacity(max_chars);
    for &c in &chars[..head] {
        out.push(c);
    }
    out.push('…');
    for &c in &chars[chars.len() - tail..] {
        out.push(c);
    }
    out
}

/// 截断字符串尾部，添加 "..."
pub fn ellipsize_tail(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}
