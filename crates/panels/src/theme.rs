//! One Dark Pro 暗色主题配色常量
//! https://github.com/Binaryify/OneDark-Pro
//!
//! 以 egui::Visuals::dark() 为基底，在此之上覆写全部小部件颜色，
//! 确保主题一致性。所有颜色均来自 One Dark Pro 配色方案。

use egui::Color32;

// ── 背景色 ──
pub const BG_PRIMARY: Color32 = Color32::from_rgb(40, 44, 52); // #282c34 编辑器背景
pub const BG_SECONDARY: Color32 = Color32::from_rgb(33, 37, 43); // #21252b 侧栏/面板背景
pub const BG_TERTIARY: Color32 = Color32::from_rgb(51, 56, 66); // #333842 控件非活跃态
pub const BG_INPUT: Color32 = Color32::from_rgb(44, 49, 60); // #2c313c 输入框背景
pub const BG_SELECTION: Color32 = Color32::from_rgb(46, 80, 120); // #2e5078 选中高亮

// ── 文本色 ──
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(171, 178, 191); // #abb2bf 正文
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(92, 99, 112); // #5c6370 次要/弱化
pub const TEXT_DIMMED: Color32 = Color32::from_rgb(130, 137, 151); // #828997 注释/提示
pub const TEXT_WHITE: Color32 = Color32::from_rgb(220, 223, 228); // #dcdfe4 高亮文本（如选中按钮文字）

// ── 语义色 ──
pub const RED: Color32 = Color32::from_rgb(224, 108, 117); // #e06c75
pub const GREEN: Color32 = Color32::from_rgb(152, 195, 121); // #98c379
pub const YELLOW: Color32 = Color32::from_rgb(229, 192, 123); // #e5c07b
pub const BLUE: Color32 = Color32::from_rgb(97, 175, 239); // #61afef
pub const PURPLE: Color32 = Color32::from_rgb(198, 120, 221); // #c678dd
pub const CYAN: Color32 = Color32::from_rgb(86, 182, 194); // #56b6c2
pub const ORANGE: Color32 = Color32::from_rgb(209, 154, 102); // #d19a66

// ── 小部件色 ──
pub const WIDGET_BG: Color32 = Color32::from_rgb(44, 49, 60); // #2c313c 控件默认背景
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(62, 68, 81); // #3e4451 悬浮
pub const WIDGET_ACTIVE: Color32 = BLUE;
pub const WIDGET_ACTIVE_WEAK: Color32 = Color32::from_rgb(56, 120, 180); // 弱激活态
pub const WIDGET_OPEN: Color32 = Color32::from_rgb(51, 56, 66); // #333842 展开态

// ── 边框/分隔线 ──
pub const BORDER: Color32 = Color32::from_rgb(24, 26, 31); // #181a1f 深色边框
pub const BORDER_LIGHT: Color32 = Color32::from_rgb(56, 62, 74); // #383e4a 浅色边框

// ── Chart 调色板 ──
pub const CHART_COLORS: [Color32; 6] = [
    GREEN,  // (152, 195, 121)
    BLUE,   // (97, 175, 239)
    YELLOW, // (229, 192, 123)
    RED,    // (224, 108, 117)
    PURPLE, // (198, 120, 221)
    CYAN,   // (86, 182, 194)
];

// ── 图表画布 ──
pub const CHART_BG: Color32 = BG_SECONDARY;
pub const CHART_GRID: Color32 = Color32::from_rgb(52, 58, 68);

// ── 3D 姿态视图 ──
pub const ATTITUDE_BG: Color32 = Color32::from_rgb(33, 37, 43);
pub const ATTITUDE_BODY: Color32 = Color32::from_rgb(229, 192, 123); // 与 YELLOW 一致
pub const ATTITUDE_AXIS_X: Color32 = RED;
pub const ATTITUDE_AXIS_Y: Color32 = GREEN;
pub const ATTITUDE_AXIS_Z: Color32 = BLUE;
