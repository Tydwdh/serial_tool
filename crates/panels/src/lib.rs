mod attitude;
mod chart;
mod data_settings;
mod data_table;
pub mod design;
mod dynamic;
mod gauge;
mod keymap;
mod log;
mod manager;
mod network;
mod plugin_settings;
mod plugins;
mod recording;
mod replay;
mod replay_policy;
mod replay_view;
mod search;
pub mod sender;
pub mod serial;
mod status;
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
pub use data_settings::{DataSettingsView, data_settings_ui};
pub use data_table::{DataTableColumn, DataTablePanel};
pub use dynamic::{
    DynamicField, DynamicFieldKind, DynamicPanels, FieldFilter, FieldOption, PortItem,
    dynamic_form_ui, parse_fields,
};
pub use gauge::GaugePanel;
pub use keymap::{KeymapAction, KeymapEntry, keymap_ui};
pub use log::{LogExportCursor, LogExportJob, LogPanel};
pub use manager::{
    DockArea, DockLayout, DockStack, PANEL_BUILTIN, PANEL_CHART, PANEL_DEVICES, PANEL_LOGS,
    PANEL_PLUGINS, PANEL_REPLAY, PANEL_SENDER, PANEL_SETTINGS, PANEL_TERMINAL, PanelId,
    PanelManager, TilesLayout,
};
pub use network::{NetworkSerialAction, NetworkSerialFormView, network_serial_form_ui};
pub use plugin_settings::{PluginSettingsView, plugin_settings_ui};
pub use plugins::{
    MarketplaceState, PluginPanelEvent, PluginPanelOptions, PluginTab, PluginsPanel,
};
pub use recording::{RecordingAction, RecordingMode, RecordingView, recording_ui};
pub use replay::ReplayPanel;
pub use replay_policy::{ReplayPolicyOption, replay_policy_ui};
pub use replay_view::ReplayUiCommand;
pub use search::SearchQuery;
pub use sender::{
    SEND_LAYOUT_BREAKPOINT, SendAction, SendLayout, SendLineEnding, SendPortItem,
    SendToolbarButton, SendView, record_history, send_layout_for_width, sender_ui,
};
pub use serial::{
    SerialAction, SerialPanel, SerialPortItem, SerialPortMetadata, SerialTopBarAction,
    SerialTopBarView, SerialView,
};
pub use status::{StatusBarAction, StatusBarView, StatusSignalView, status_bar_contents_ui};
pub use table::{RowSelection, copy_text_with_feedback, take_copy_feedback};
pub use terminal::{TerminalExportCursor, TerminalExportFormat, TerminalExportJob, TerminalPanel};

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

/// 判定自动跟随是否继续所需的全部输入：一次开关状态 + 指针位置 + 本帧滚动几何。
///
/// 收拢之前这里是 3 个相邻 bool 紧跟 4 个相邻 f32：七对相邻参数里有六对同型，
/// 那六对任意对调都编译得过且零告警（第 30 轮逐对普查）；当时唯一的调用点
/// `AutoScrollState::update` 还没有用例覆盖 —— 实参顺序排错，`tool-panels`
/// 整套测试照样全绿。
///
/// 第 31 轮补上了那个调用点：`table.rs` 的用例按真实帧驱动 `update`，六对里每一对
/// 现在都至少被一条用例抓到（含曾经全盲的 `content_height` ↔ `viewport_height`）。
///
/// 与 `transport` 的 `NetworkWorkerContext` 同一式子（无 trait、无泛型、无 builder），
/// 也受同一条限度约束：命名字段拦的是**写法**不是**取值**。位置排错在结构体字面量上
/// 没有语法，漏字段 E0063、类型不对 E0308；但 `previous_offset_y: offset_y` 这种同型
/// 对调照样编译得过，它只是要求把矛盾写在字段名旁边，不是类型层的保证。
pub(crate) struct AutoScrollInputs {
    /// 进入本帧时的自动跟随开关：true = 跟随底部。
    pub(crate) auto_scroll: bool,
    /// 指针是否悬停在该视图的可视区内；不在就原样保留开关，不做任何判定。
    pub(crate) pointer_inside: bool,
    /// 鼠标左键是否按下：用来区分“用户拖着滚动条离开底部”与“内容高度自己变了”。
    pub(crate) primary_down: bool,
    /// 本帧滚轮增量，符号照 egui：正 = 向上（看历史），负 = 向下（追最新）。
    pub(crate) scroll_delta_y: f32,
    /// 上一帧记下的偏移，只用来和本帧比较方向，不参与“是否到底”的判断。
    pub(crate) previous_offset_y: f32,
    /// 本帧的偏移，与 `previous_offset_y` 是同一物理量的两个时刻。两者对调会同时翻转
    /// `moved_away_from_bottom` 与 `moved_towards_bottom`，自动跟随的进出因此整个反过来。
    pub(crate) offset_y: f32,
    /// 内容总高。第 30 轮普查里唯一没人抓到的一对就是它与 `viewport_height` 的对调：
    /// 换过来后 `bottom_offset` 被 `.max(0.0)` 钳成 0，“是否到底”恒为真，而那七条
    /// 用例的期望值全都照样成立。第 31 轮起由 `table.rs` 的
    /// `following_resumes_only_once_the_view_reaches_the_bottom` 在调用点抓到。
    pub(crate) content_height: f32,
    /// 可视区高度：调用方传的是 `inner_rect.height()`，不是内容能显示多少行。
    pub(crate) viewport_height: f32,
}

pub(crate) fn next_auto_scroll_state(inputs: AutoScrollInputs) -> bool {
    // 解构回原来的名字：下面的判定只是读这八个值各一次，
    // 收拢参数不该把每一次读取都改成结构体字段访问。
    let AutoScrollInputs {
        auto_scroll,
        pointer_inside,
        primary_down,
        scroll_delta_y,
        previous_offset_y,
        offset_y,
        content_height,
        viewport_height,
    } = inputs;

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

/// 截断字符串中间，用 "…" 连接首尾
pub fn compact_middle(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let head = max_chars / 2;
    let tail = max_chars - head - 1; // 给 "…" 留一个字符位
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

/// 截断字符串尾部，添加 "…"
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
        // 固定 UTC 时间戳：2025-01-15T08:30:45.123Z，即 1736929845123ms
        let result = fmt_ts(1_736_929_845_123);
        // 输出格式为 HH:MM:SS.mmm，前导零会让长度在 11~12 之间浮动
        assert!(
            result.len() >= 11 && result.len() <= 12,
            "unexpected length: {result:?}"
        );
        // 应包含两个冒号、一个小数点
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
        // Unix 纪元起点也应能正常格式化
        assert!(!result.is_empty());
        assert_eq!(result.matches(':').count(), 2);
        assert_eq!(result.matches('.').count(), 1);
    }

    #[test]
    fn fmt_ts_large_timestamp() {
        // 远期时间戳不应 panic：超出 chrono 范围时返回占位串
        let result = fmt_ts(u64::MAX);
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

        // 各等级的颜色必须互不相同，否则日志会失去可辨识度
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
            // 至少保证每个等级都能取到一个颜色且不 panic
            let _ = level_color(level);
        }
    }

    // ── auto scroll ──────────────────────────────────────────────────

    #[test]
    fn auto_scroll_stays_enabled_when_scroll_input_moves_towards_bottom() {
        let next = next_auto_scroll_state(AutoScrollInputs {
            auto_scroll: true,
            pointer_inside: true,
            primary_down: false,
            scroll_delta_y: -24.0,
            previous_offset_y: 96.0,
            offset_y: 100.0,
            content_height: 200.0,
            viewport_height: 100.0,
        });

        assert!(next);
    }

    #[test]
    fn auto_scroll_disables_after_view_moves_away_from_bottom() {
        let next = next_auto_scroll_state(AutoScrollInputs {
            auto_scroll: true,
            pointer_inside: true,
            primary_down: false,
            scroll_delta_y: 24.0,
            previous_offset_y: 100.0,
            offset_y: 80.0,
            content_height: 200.0,
            viewport_height: 100.0,
        });

        assert!(!next);
    }

    #[test]
    fn auto_scroll_disables_immediately_on_upward_wheel_input() {
        // 滚动容器尚未更新 offset，或内容正在追加时，向上滚动也必须立即暂停。
        // previous_offset_y == offset_y：本帧偏移一点没变，只有滚轮增量能说明意图。
        let next = next_auto_scroll_state(AutoScrollInputs {
            auto_scroll: true,
            pointer_inside: true,
            primary_down: false,
            scroll_delta_y: 24.0,
            previous_offset_y: 100.0,
            offset_y: 100.0,
            content_height: 200.0,
            viewport_height: 100.0,
        });

        assert!(!next);
    }

    #[test]
    fn auto_scroll_reenables_when_manual_scroll_reaches_bottom() {
        let next = next_auto_scroll_state(AutoScrollInputs {
            auto_scroll: false,
            pointer_inside: true,
            primary_down: false,
            scroll_delta_y: -24.0,
            previous_offset_y: 80.0,
            offset_y: 100.0,
            content_height: 200.0,
            viewport_height: 100.0,
        });

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
        let next = next_auto_scroll_state(AutoScrollInputs {
            auto_scroll: false,
            pointer_inside: true,
            primary_down: false,
            scroll_delta_y: 0.0,
            previous_offset_y: 100.0,
            offset_y: 100.0,
            content_height: 200.0,
            viewport_height: 100.0,
        });

        assert!(!next);
    }

    #[test]
    fn passive_height_correction_does_not_disable_auto_scroll() {
        // 偏移自己离开了底部（previous_offset_y > offset_y），但既没有滚轮增量也没按着键：
        // 这就是测试名说的“被动修正”，没有用户动作就不该退出跟随。
        let next = next_auto_scroll_state(AutoScrollInputs {
            auto_scroll: true,
            pointer_inside: true,
            primary_down: false,
            scroll_delta_y: 0.0,
            previous_offset_y: 100.0,
            offset_y: 80.0,
            content_height: 200.0,
            viewport_height: 100.0,
        });

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
        let input = "1234567890"; // 10 个字符
        assert_eq!(compact_middle(input, 10), input);
        // 再多一个字符就应触发截断
        assert_ne!(compact_middle("12345678901", 10), "12345678901");
    }

    #[test]
    fn compact_middle_empty_string() {
        assert_eq!(compact_middle("", 5), "");
        assert_eq!(compact_middle("", 0), "");
    }

    #[test]
    fn compact_middle_zero_max_chars() {
        // max_chars = 0：空串字符数 0 <= 0，原样返回
        assert_eq!(compact_middle("", 0), "");
        // max_chars = 0 配非空串是退化输入，实际调用只会传合理上限，不覆盖
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
        let input = "1234567890"; // 10 个字符
        assert_eq!(ellipsize_tail(input, 10), input);
        // 再多一个字符就应触发截断
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
        // max_chars=1："abc" 有 3 个字符 > 1，取 0 个字符 + "…" = "…"
        assert_eq!(ellipsize_tail("abc", 1), "…");
        // 单字符且 max_chars=1：无需截断
        assert_eq!(ellipsize_tail("a", 1), "a");
    }
}
