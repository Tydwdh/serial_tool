use egui::Color32;

// ══════════════════════════════════════════
//  背景层级：深底 → 卡片面 → 输入框
// ══════════════════════════════════════════
pub const BG_DEEP: Color32 = Color32::from_rgb(33, 37, 43); // #21252B
pub const BG_PRIMARY: Color32 = Color32::from_rgb(35, 39, 47);
pub const BG_SECONDARY: Color32 = Color32::from_rgb(28, 32, 38);
pub const BG_TERTIARY: Color32 = Color32::from_rgb(43, 49, 60);
pub const BG_CARD: Color32 = Color32::from_rgb(38, 43, 53);
pub const BG_INPUT: Color32 = Color32::from_rgb(31, 36, 45);
pub const BG_SELECTION: Color32 = Color32::from_rgb(36, 72, 108);
pub const BG_HOVER: Color32 = Color32::from_rgb(49, 56, 68);

// ══════════════════════════════════════════
//  文本层级
// ══════════════════════════════════════════
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(205, 211, 222);
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(145, 154, 168);
pub const TEXT_DIMMED: Color32 = Color32::from_rgb(120, 128, 140);
pub const TEXT_WHITE: Color32 = Color32::from_rgb(232, 236, 243);

// ══════════════════════════════════════════
//  语义色 — 前景（文字/图标用）
// ══════════════════════════════════════════
pub const RED: Color32 = Color32::from_rgb(237, 108, 115);
pub const GREEN: Color32 = Color32::from_rgb(137, 180, 108);
pub const YELLOW: Color32 = Color32::from_rgb(210, 166, 90);
pub const BLUE: Color32 = Color32::from_rgb(80, 140, 210);
pub const PURPLE: Color32 = Color32::from_rgb(175, 108, 200);
pub const CYAN: Color32 = Color32::from_rgb(72, 158, 170);
pub const ORANGE: Color32 = Color32::from_rgb(210, 140, 80);

// ══════════════════════════════════════════
//  语义色 — 背景（标签/徽章/卡片指示器用）
// ══════════════════════════════════════════
pub const RED_BG: Color32 = Color32::from_rgb(70, 30, 35);
pub const GREEN_BG: Color32 = Color32::from_rgb(40, 65, 40);
pub const YELLOW_BG: Color32 = Color32::from_rgb(65, 50, 25);
pub const BLUE_BG: Color32 = Color32::from_rgb(30, 50, 75);
pub const PURPLE_BG: Color32 = Color32::from_rgb(55, 35, 65);
pub const CYAN_BG: Color32 = Color32::from_rgb(25, 55, 60);
pub const ORANGE_BG: Color32 = Color32::from_rgb(65, 45, 25);

// ══════════════════════════════════════════
//  卡片指示器色条（左边框装饰）
// ══════════════════════════════════════════
pub const CARD_ACCENT_DEVICE: Color32 = BLUE;
pub const CARD_ACCENT_RECORD: Color32 = RED;
pub const CARD_ACCENT_PORT: Color32 = GREEN;
pub const CARD_ACCENT_REPLAY: Color32 = CYAN;
pub const CARD_ACCENT_PLUGIN: Color32 = PURPLE;
pub const CARD_ACCENT_SETTINGS: Color32 = ORANGE;

// ══════════════════════════════════════════
//  小部件
// ══════════════════════════════════════════
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

// 分隔线
pub const SEPARATOR: Color32 = Color32::from_rgb(58, 67, 82);
pub const SEPARATOR_STRONG: Color32 = Color32::from_rgb(70, 82, 100);

// ── Chart 调色板 ──
pub const CHART_COLORS: [Color32; 6] = [GREEN, BLUE, YELLOW, RED, PURPLE, CYAN];

// ── 图表画布 ──
pub const CHART_BG: Color32 = BG_SECONDARY;
pub const CHART_GRID: Color32 = Color32::from_rgb(48, 55, 66);
pub const CHART_CROSSHAIR: Color32 = Color32::from_rgb(100, 110, 125);
pub const CHART_TOOLTIP_BG: Color32 = Color32::from_rgb(30, 34, 42);

// ── 3D 姿态视图 ──
pub const ATTITUDE_BG: Color32 = BG_SECONDARY;
pub const ATTITUDE_BODY: Color32 = YELLOW;
pub const ATTITUDE_BODY_EDGE: Color32 = Color32::from_rgb(180, 155, 60);
pub const ATTITUDE_AXIS_X: Color32 = RED;
pub const ATTITUDE_AXIS_Y: Color32 = GREEN;
pub const ATTITUDE_AXIS_Z: Color32 = BLUE;

// ── 仪表盘 ──
pub const GAUGE_BG: Color32 = BG_SECONDARY;
pub const GAUGE_ARC: Color32 = Color32::from_rgb(60, 68, 82);
pub const GAUGE_VALUE: Color32 = CYAN;

// ── 公共 UI 工具 ──

/// 绘制带左边框色彩指示器的卡片标题行。
/// 在卡片内第一行调用，画一条 3px 宽的色条在标题左侧。
pub fn card_accent_bar(ui: &mut egui::Ui, color: Color32) {
    let rect = ui.available_rect_before_wrap();
    let accent_rect = egui::Rect::from_min_size(
        rect.left_top(),
        egui::vec2(3.0, rect.height().max(ui.spacing().interact_size.y)),
    );
    ui.painter().rect_filled(accent_rect, 2.0, color);
    // 给色条右边留间距
    ui.add_space(6.0);
}

/// 自动滚动按钮：暂停/恢复自动滚动。
/// 返回 `true` 表示需要强制滚动到底部。
///
/// 两种状态使用相同文字「跟随」，靠 selected 高亮区分：
/// 开 = 高亮（滚动跟随最新数据）；关 = 普通（已暂停跟随）。固定文字避免切换时按钮宽度抖动。
pub fn auto_scroll_button(ui: &mut egui::Ui, auto_scroll: &mut bool) -> bool {
    let btn = egui::Button::new("跟随").selected(*auto_scroll);
    let resp = ui.add(btn).on_hover_text(if *auto_scroll {
        "滚动跟随最新数据 · 点击暂停跟随"
    } else {
        "已暂停跟随 · 点击滚动到底并恢复"
    });
    if resp.clicked() {
        // 关→开：恢复跟随并强制滚到底；开→关：仅停止跟随。
        if !*auto_scroll {
            *auto_scroll = true;
            true
        } else {
            *auto_scroll = false;
            false
        }
    } else {
        false
    }
}
