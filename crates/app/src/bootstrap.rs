use eframe::egui;
use std::path::PathBuf;
use tool_panels::theme;

pub const ACTIVITY_BAR_WIDTH: f32 = 104.0; //左侧活动栏宽度
pub const BOTTOM_PANEL_MIN: f32 = 280.0; //底部面板最小高度
pub const DEFAULT_WINDOW_WIDTH: f32 = 1280.0; //默认窗口宽度
pub const DEFAULT_WINDOW_HEIGHT: f32 = 820.0; //默认窗口高度

/// 应用所在目录（基于 exe 路径，不依赖 CWD）。
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| {
            let fallback = std::env::current_dir().unwrap_or_else(|e| {
                eprintln!("[app] WARNING: current_dir() failed: {e}, falling back to '.'");
                PathBuf::from(".")
            });
            eprintln!(
                "[app] WARNING: current_exe() unavailable, using CWD: {}",
                fallback.display()
            );
            fallback
        })
}

pub fn setup_fonts(cc: &eframe::CreationContext<'_>) {
    let dir = app_dir();

    let mut fonts = egui::FontDefinitions::default();

    load_font(
        &mut fonts,
        "jetbrains",
        &[
            dir.join("assets/JetBrainsMonoNerdFontMono-Regular.ttf"),
            PathBuf::from("assets/JetBrainsMonoNerdFontMono-Regular.ttf"),
        ],
    );

    load_font(
        &mut fonts,
        "zh",
        &[
            dir.join("assets/NotoSansSC-VF.ttf"),
            PathBuf::from("assets/NotoSansSC-VF.ttf"),
            PathBuf::from("C:\\Windows\\Fonts\\msyh.ttc"),
        ],
    );

    load_font(
        &mut fonts,
        "emoji",
        &[
            dir.join("assets/seguiemj.ttf"),
            PathBuf::from("C:\\Windows\\Fonts\\seguiemj.ttf"),
        ],
    );

    set_family(
        &mut fonts,
        egui::FontFamily::Proportional,
        &["zh", "jetbrains", "emoji"],
    );

    set_family(
        &mut fonts,
        egui::FontFamily::Monospace,
        &["jetbrains", "zh", "emoji"],
    );

    cc.egui_ctx.set_fonts(fonts);
}

fn load_font(fonts: &mut egui::FontDefinitions, name: &str, paths: &[PathBuf]) {
    for path in paths {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
            return;
        }
    }
}

fn set_family(fonts: &mut egui::FontDefinitions, family: egui::FontFamily, names: &[&str]) {
    let list = fonts.families.entry(family).or_default();

    for &name in names.iter().rev() {
        if fonts.font_data.contains_key(name) {
            list.insert(0, name.to_owned());
        }
    }
}

// ── 主题 ──
pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
    let mut s = (*ctx.global_style()).clone();

    // ── 间距 ──
    s.spacing.item_spacing = egui::vec2(10.0, 8.0);
    s.spacing.button_padding = egui::vec2(12.0, 6.0);
    s.spacing.interact_size = egui::vec2(40.0, 28.0);
    s.spacing.slider_width = 180.0;
    s.spacing.combo_width = 140.0;
    s.spacing.text_edit_width = 220.0;
    s.spacing.indent = 14.0;
    s.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 6.0,
        handle_min_length: 30.0,
        ..egui::style::ScrollStyle::solid()
    };

    // ── 交互 ──
    s.interaction.resize_grab_radius_side = 6.0;
    s.interaction.resize_grab_radius_corner = 10.0;
    s.interaction.show_tooltips_only_when_still = false;
    s.interaction.tooltip_delay = 0.3;

    // ── 动画 ──
    s.animation_time = 0.05;

    #[cfg(debug_assertions)]
    {
        s.debug.show_interactive_widgets = false;
        s.debug.show_focused_widget = false;
        s.debug.show_unaligned = false;
        s.debug.warn_if_rect_changes_id = false;
        s.debug.show_resize = false;
        s.debug.show_widget_hits = false;
    }

    // ── 字体大小 ──
    let mut text_styles = s.text_styles.clone();
    text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
    );
    text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
    );
    text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(13.0, egui::FontFamily::Monospace),
    );
    text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
    );
    text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(18.0, egui::FontFamily::Proportional),
    );
    s.text_styles = text_styles;

    // ── Visuals ──
    let mut v = egui::Visuals::dark();

    // 圆角
    v.window_corner_radius = 8.into();
    v.menu_corner_radius = 6.into();

    // 背景 — 使用新的层级色
    v.panel_fill = theme::BG_PRIMARY;
    v.window_fill = theme::BG_DEEP;
    v.extreme_bg_color = theme::BG_DEEP;
    v.faint_bg_color = theme::BG_CARD;
    v.code_bg_color = theme::BG_INPUT;
    v.text_edit_bg_color = Some(theme::BG_INPUT);

    // 文本
    v.override_text_color = Some(theme::TEXT_PRIMARY);
    v.weak_text_color = Some(theme::TEXT_SECONDARY);
    v.warn_fg_color = theme::YELLOW;
    v.error_fg_color = theme::RED;
    v.hyperlink_color = theme::CYAN;

    // 选中
    v.selection.bg_fill = theme::BG_SELECTION;
    v.selection.stroke = egui::Stroke::new(1.0, theme::BLUE);

    // 窗口
    v.window_stroke = egui::Stroke::new(1.0, theme::SEPARATOR);
    v.resize_corner_size = 8.0;
    v.striped = true;
    v.collapsing_header_frame = false;
    v.window_highlight_topmost = false;
    v.button_frame = true;
    v.indent_has_left_vline = false;

    // ── Widget 样式 ──
    let w = &mut v.widgets;

    // noninteractive（标签、分隔线等）
    w.noninteractive.corner_radius = 4.into();
    w.noninteractive.bg_fill = theme::BG_CARD;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    w.noninteractive.weak_bg_fill = theme::BG_SECONDARY;

    // inactive（按钮、输入框等未交互状态）
    w.inactive.corner_radius = 6.into();
    w.inactive.bg_fill = theme::BG_TERTIARY;
    w.inactive.weak_bg_fill = theme::BG_INPUT;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER);
    w.inactive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);

    // hovered
    w.hovered.corner_radius = 6.into();
    w.hovered.bg_fill = theme::BG_HOVER;
    w.hovered.weak_bg_fill = theme::BG_TERTIARY;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, theme::BORDER_LIGHT);
    w.hovered.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);

    // active（按下/选中）
    w.active.corner_radius = 6.into();
    w.active.bg_fill = theme::WIDGET_ACTIVE_WEAK;
    w.active.weak_bg_fill = theme::WIDGET_ACTIVE_WEAK;
    w.active.bg_stroke = egui::Stroke::new(1.0, theme::BLUE);
    w.active.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_WHITE);

    // open（展开状态）
    w.open.corner_radius = 6.into();
    w.open.bg_fill = theme::WIDGET_OPEN;
    w.open.weak_bg_fill = theme::BG_INPUT;
    w.open.bg_stroke = egui::Stroke::new(1.0, theme::BORDER_LIGHT);
    w.open.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);

    s.visuals = v;
    ctx.set_global_style(s);
}
