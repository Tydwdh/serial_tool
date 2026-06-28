use egui::Color32;

// 背景
pub const BG_PRIMARY: Color32 = Color32::from_rgb(35, 39, 47);
pub const BG_SECONDARY: Color32 = Color32::from_rgb(28, 32, 38);
pub const BG_TERTIARY: Color32 = Color32::from_rgb(43, 49, 60);
pub const BG_INPUT: Color32 = Color32::from_rgb(31, 36, 45);
pub const BG_SELECTION: Color32 = Color32::from_rgb(36, 72, 108);

// 文本
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(205, 211, 222);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(145, 154, 168);
pub const TEXT_DIMMED: Color32 = Color32::from_rgb(120, 128, 140);
pub const TEXT_WHITE: Color32 = Color32::from_rgb(232, 236, 243);

// 语义色
pub const RED: Color32 = Color32::from_rgb(218, 96, 105);
pub const GREEN: Color32 = Color32::from_rgb(137, 180, 108);
pub const YELLOW: Color32 = Color32::from_rgb(210, 166, 90);
pub const BLUE: Color32 = Color32::from_rgb(70, 130, 190);
pub const PURPLE: Color32 = Color32::from_rgb(175, 108, 200);
pub const CYAN: Color32 = Color32::from_rgb(72, 158, 170);
pub const ORANGE: Color32 = Color32::from_rgb(190, 130, 78);

// 小部件
pub const WIDGET_BG: Color32 = BG_TERTIARY;
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(49, 56, 68);
pub const WIDGET_ACTIVE: Color32 = BLUE;
pub const WIDGET_ACTIVE_WEAK: Color32 = Color32::from_rgb(34, 68, 102);
pub const WIDGET_ACTIVE_STRONG: Color32 = Color32::from_rgb(44, 88, 132);
pub const WIDGET_OPEN: Color32 = Color32::from_rgb(40, 47, 58);

// 滚动条
pub const SCROLLBAR: Color32 = Color32::from_rgb(55, 62, 75);
pub const SCROLLBAR_HOVER: Color32 = Color32::from_rgb(70, 78, 92);

// 边框
pub const BORDER: Color32 = Color32::from_rgb(39, 45, 55);
pub const BORDER_LIGHT: Color32 = Color32::from_rgb(48, 56, 68);
pub const BORDER_DARK: Color32 = Color32::from_rgb(22, 25, 30);

// 专门给 ui.separator() / 面板分隔线用
pub const SEPARATOR: Color32 = Color32::from_rgb(58, 67, 82);
pub const SEPARATOR_STRONG: Color32 = Color32::from_rgb(70, 82, 100);

// ── Chart 调色板 ──
pub const CHART_COLORS: [Color32; 6] = [GREEN, BLUE, YELLOW, RED, PURPLE, CYAN];

// ── 图表画布 ──
pub const CHART_BG: Color32 = BG_SECONDARY;
pub const CHART_GRID: Color32 = Color32::from_rgb(48, 55, 66);

// ── 3D 姿态视图 ──
pub const ATTITUDE_BG: Color32 = BG_SECONDARY;
pub const ATTITUDE_BODY: Color32 = YELLOW;
pub const ATTITUDE_AXIS_X: Color32 = RED;
pub const ATTITUDE_AXIS_Y: Color32 = GREEN;
pub const ATTITUDE_AXIS_Z: Color32 = BLUE;

// ── 公共 UI 工具 ──

/// 自动滚动按钮：暂停/恢复自动滚动。
/// 返回 `true` 表示需要强制滚动到底部。
pub fn auto_scroll_button(ui: &mut egui::Ui, auto_scroll: &mut bool) -> bool {
    if *auto_scroll {
        if ui.button("⏸").on_hover_text("暂停自动滚动").clicked() {
            *auto_scroll = false;
        }
        false
    } else if ui.button("↓").on_hover_text("滚动到底部").clicked() {
        *auto_scroll = true;
        true
    } else {
        false
    }
}
