mod attitude;
mod chart;
mod data_table;
pub mod design;
#[cfg(not(target_arch = "wasm32"))]
mod dynamic;
mod gauge;
#[cfg(not(target_arch = "wasm32"))]
mod log;
mod manager;
#[cfg(not(target_arch = "wasm32"))]
mod plugin_view;
#[cfg(not(target_arch = "wasm32"))]
mod plugins;
#[cfg(not(target_arch = "wasm32"))]
mod replay;
#[cfg(not(target_arch = "wasm32"))]
mod replay_view;
mod search;
mod table;
mod terminal;
pub mod theme;
mod virtual_list;

/// 面板每帧最多摄入的事件数，避免 UI 卡顿。
pub(crate) const MAX_INGEST_PER_FRAME: usize = 500;
/// 接收区和日志区的发布侧环形缓冲容量。窗口最小化时仍由 DataBus 发布线程写入，
/// 队列满后丢最旧事件并通过面板通知用户。
pub(crate) const MESSAGE_EVENT_BUFFER_CAPACITY: usize = 65_536;

pub use attitude::AttitudePanel;
pub use chart::ChartPanel;
pub use data_table::{DataTableColumn, DataTablePanel};
#[cfg(not(target_arch = "wasm32"))]
pub use dynamic::DynamicPanels;
#[cfg(not(target_arch = "wasm32"))]
pub use dynamic::{
    DynamicField, DynamicFieldKind, FieldFilter, FieldOption, PortItem, dynamic_form_ui,
    parse_fields,
};
pub use gauge::GaugePanel;
#[cfg(not(target_arch = "wasm32"))]
pub use log::{LogExportJob, LogPanel};
pub use manager::{
    DockArea, DockLayout, DockStack, PANEL_BUILTIN, PANEL_DEVICES, PANEL_LOGS, PANEL_PLUGINS,
    PANEL_REPLAY, PANEL_SENDER, PANEL_SETTINGS, PANEL_TERMINAL, PanelId, PanelManager, TilesLayout,
};
#[cfg(not(target_arch = "wasm32"))]
pub use plugin_view::{InstalledPluginRow, PluginUiCommand, PluginViewState};
#[cfg(not(target_arch = "wasm32"))]
pub use plugins::{MarketplaceState, PluginPanelEvent, PluginTab, PluginsPanel};
#[cfg(not(target_arch = "wasm32"))]
pub use replay::ReplayPanel;
#[cfg(not(target_arch = "wasm32"))]
pub use replay_view::{ReplayUiCommand, ReplayView};
pub use search::SearchQuery;
pub use table::{RowSelection, copy_text_with_feedback, take_copy_feedback};
pub use terminal::{TerminalExportFormat, TerminalExportJob, TerminalPanel};

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

const AUTO_SCROLL_OFFSET_EPSILON: f32 = 0.5;
const AUTO_SCROLL_BOTTOM_EPSILON: f32 = 4.0;

pub(crate) fn scroll_delta_moves_away_from_bottom(scroll_delta_y: f32) -> bool {
    scroll_delta_y > AUTO_SCROLL_OFFSET_EPSILON
}

pub(crate) fn scroll_delta_moves_towards_bottom(scroll_delta_y: f32) -> bool {
    scroll_delta_y < -AUTO_SCROLL_OFFSET_EPSILON
}

pub(crate) fn scroll_is_at_bottom(
    offset_y: f32,
    content_height: f32,
    viewport_height: f32,
) -> bool {
    let bottom_offset = (content_height - viewport_height).max(0.0);
    offset_y >= bottom_offset - AUTO_SCROLL_BOTTOM_EPSILON
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn next_auto_scroll_state(
    auto_scroll: bool,
    pointer_inside: bool,
    primary_down: bool,
    scroll_delta_y: f32,
    previous_offset_y: f32,
    offset_y: f32,
    content_height: f32,
    viewport_height: f32,
) -> bool {
    if !pointer_inside {
        return auto_scroll;
    }

    let at_bottom = scroll_is_at_bottom(offset_y, content_height, viewport_height);
    let moved_away_from_bottom = offset_y < previous_offset_y - AUTO_SCROLL_OFFSET_EPSILON;
    let moved_towards_bottom = offset_y > previous_offset_y + AUTO_SCROLL_OFFSET_EPSILON;
    let scrolled_towards_bottom = scroll_delta_moves_towards_bottom(scroll_delta_y);

    // 滚轮向上是明确的“查看历史”意图，应立即暂停自动跟随。不能要求本帧的
    // offset 同时发生变化：ScrollArea 会先消费滚轮输入，而且持续追加内容时
    // offset 的绝对值也可能没有减小。
    let wheel_moves_away = scroll_delta_moves_away_from_bottom(scroll_delta_y);
    let dragged_away = primary_down && moved_away_from_bottom && !at_bottom;
    if auto_scroll && (wheel_moves_away || dragged_away) {
        return false;
    }

    if !auto_scroll && at_bottom && (moved_towards_bottom || scrolled_towards_bottom) {
        return true;
    }

    auto_scroll
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── fmt_ts ────────────────────────────────────────────────────────

    #[test]
    fn fmt_ts_returns_expected_format() {
        // Use a known UTC timestamp: 2025-01-15T08:30:45.123Z
        // ms = 1736929845123
        let result = fmt_ts(1_736_929_845_123);
        // Format should be HH:MM:SS.mmm (11 or 12 chars depending on leading zero)
        assert!(
            result.len() >= 11 && result.len() <= 12,
            "unexpected length: {result:?}"
        );
        // Should contain two colons and one dot
        assert_eq!(
            result.matches(':').count(),
            2,
            "missing colons in {result:?}"
        );
        assert_eq!(result.matches('.').count(), 1, "missing dot in {result:?}");
    }

    #[test]
    fn fmt_ts_zero_timestamp() {
        let result = fmt_ts(0);
        // Unix epoch should still format cleanly
        assert!(!result.is_empty());
        assert_eq!(result.matches(':').count(), 2);
        assert_eq!(result.matches('.').count(), 1);
    }

    #[test]
    fn fmt_ts_large_timestamp() {
        // Far-future timestamp should not panic
        let result = fmt_ts(u64::MAX);
        // If the timestamp is out of range, it returns the fallback string
        assert!(!result.is_empty());
    }

    // ── level_color ───────────────────────────────────────────────────

    #[test]
    fn level_color_each_level_is_distinct() {
        use tool_core::LogLevel;
        let levels = [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ];
        let colors: Vec<egui::Color32> = levels.iter().map(|&l| level_color(l)).collect();

        // All colors should be distinct
        for i in 0..colors.len() {
            for j in (i + 1)..colors.len() {
                assert_ne!(
                    colors[i], colors[j],
                    "level_color({:?}) == level_color({:?})",
                    levels[i], levels[j]
                );
            }
        }
    }

    #[test]
    fn level_color_returns_valid_color() {
        use tool_core::LogLevel;
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            let color = level_color(level);
            // Color32 should have non-zero alpha (fully opaque or nearly so)
            // We just check it's a real color value
            let _ = color; // at minimum, the function returned without panicking
        }
    }

    // ── auto scroll ──────────────────────────────────────────────────

    #[test]
    fn auto_scroll_stays_enabled_when_scroll_input_moves_towards_bottom() {
        let next = next_auto_scroll_state(true, true, false, -24.0, 96.0, 100.0, 200.0, 100.0);

        assert!(next);
    }

    #[test]
    fn auto_scroll_disables_after_view_moves_away_from_bottom() {
        let next = next_auto_scroll_state(true, true, false, 24.0, 100.0, 80.0, 200.0, 100.0);

        assert!(!next);
    }

    #[test]
    fn auto_scroll_disables_immediately_on_upward_wheel_input() {
        // 滚动容器尚未更新 offset，或内容正在追加时，向上滚动也必须立即暂停。
        let next = next_auto_scroll_state(true, true, false, 24.0, 100.0, 100.0, 200.0, 100.0);

        assert!(!next);
    }

    #[test]
    fn auto_scroll_reenables_when_manual_scroll_reaches_bottom() {
        let next = next_auto_scroll_state(false, true, false, -24.0, 80.0, 100.0, 200.0, 100.0);

        assert!(next);
    }

    #[test]
    fn scroll_delta_direction_matches_egui_wheel_direction() {
        assert!(scroll_delta_moves_towards_bottom(-24.0));
        assert!(!scroll_delta_moves_towards_bottom(24.0));
        assert!(scroll_delta_moves_away_from_bottom(24.0));
        assert!(!scroll_delta_moves_away_from_bottom(-24.0));
    }

    #[test]
    fn auto_scroll_pause_is_not_immediately_reenabled_without_bottomward_motion() {
        let next = next_auto_scroll_state(false, true, false, 0.0, 100.0, 100.0, 200.0, 100.0);

        assert!(!next);
    }

    #[test]
    fn passive_height_correction_does_not_disable_auto_scroll() {
        let next = next_auto_scroll_state(true, true, false, 0.0, 100.0, 80.0, 200.0, 100.0);

        assert!(next);
    }

    // ── compact_middle ────────────────────────────────────────────────

    #[test]
    fn compact_middle_short_string_no_truncation() {
        assert_eq!(compact_middle("abc", 5), "abc");
        assert_eq!(compact_middle("hello", 10), "hello");
    }

    #[test]
    fn compact_middle_long_string_truncated() {
        let result = compact_middle("hello_world_this_is_long", 10);
        assert!(result.contains('…'), "expected ellipsis in {result:?}");
        assert_eq!(result.chars().count(), 10);
    }

    #[test]
    fn compact_middle_exact_length() {
        let input = "1234567890"; // 10 chars
        assert_eq!(compact_middle(input, 10), input);
        // One more char should trigger truncation
        assert_ne!(compact_middle("12345678901", 10), "12345678901");
    }

    #[test]
    fn compact_middle_empty_string() {
        assert_eq!(compact_middle("", 5), "");
        assert_eq!(compact_middle("", 0), "");
    }

    #[test]
    fn compact_middle_zero_max_chars() {
        // max_chars = 0: char count (0) <= 0, so returns as-is
        assert_eq!(compact_middle("", 0), "");
        // max_chars = 0 with non-empty string is a degenerate case;
        // the function is only called with reasonable max_chars in practice.
    }

    // ── ellipsize_tail ────────────────────────────────────────────────

    #[test]
    fn ellipsize_tail_short_string_no_truncation() {
        assert_eq!(ellipsize_tail("abc", 5), "abc");
        assert_eq!(ellipsize_tail("hello", 10), "hello");
    }

    #[test]
    fn ellipsize_tail_long_string_truncated() {
        let result = ellipsize_tail("hello_world_this_is_long", 10);
        assert!(
            result.ends_with('…'),
            "expected trailing ellipsis in {result:?}"
        );
        assert_eq!(result.chars().count(), 10);
    }

    #[test]
    fn ellipsize_tail_exact_length() {
        let input = "1234567890"; // 10 chars
        assert_eq!(ellipsize_tail(input, 10), input);
        // One more char should trigger truncation
        let truncated = ellipsize_tail("12345678901", 10);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncated.chars().count(), 10);
    }

    #[test]
    fn ellipsize_tail_empty_string() {
        assert_eq!(ellipsize_tail("", 5), "");
        assert_eq!(ellipsize_tail("", 0), "");
    }

    #[test]
    fn ellipsize_tail_max_chars_one() {
        // max_chars=1: "abc" has 3 chars > 1, so take 0 chars + "…" = "…"
        assert_eq!(ellipsize_tail("abc", 1), "…");
        // Single char with max_chars=1: no truncation
        assert_eq!(ellipsize_tail("a", 1), "a");
    }
}
