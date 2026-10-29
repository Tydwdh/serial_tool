use eframe::egui;
use std::path::PathBuf;
use tool_panels::theme;

/// 默认窗口宽度。
pub const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
/// 默认窗口高度。
pub const DEFAULT_WINDOW_HEIGHT: f32 = 820.0;

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

/// 用户可写的应用数据目录。
///
/// Ubuntu 的 `.deb` 会把程序安装到 `/usr/lib/hardware-workbench`，该目录
/// 只应存放只读资源。插件、主题和用户数据必须放在当前用户的数据目录，
/// 这样从应用菜单启动时不需要 root 权限。
pub fn user_data_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        dirs_next::data_dir()
            .map(|dir| dir.join("HardwareWorkbench"))
            .unwrap_or_else(app_dir)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // 保持便携包和 Windows 安装器的既有目录行为。
        app_dir()
    }
}

pub fn user_plugins_dir() -> PathBuf {
    user_data_dir().join("plugins")
}

pub fn user_themes_dir() -> PathBuf {
    user_data_dir().join("themes")
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
            PathBuf::from("C:\\Windows\\Fonts\\seguiemj.ttf"),
            PathBuf::from("/System/Library/Fonts/Apple Color Emoji.ttc"),
            PathBuf::from("/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf"),
            PathBuf::from("/usr/share/fonts/noto-color-emoji/NotoColorEmoji.ttf"),
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
    egui_material_icons::initialize(&cc.egui_ctx);
}

fn load_font(fonts: &mut egui::FontDefinitions, name: &str, paths: &[PathBuf]) {
    // 按候选顺序取第一个能读到的字体文件；全都读不到就不注册该字体。
    if let Some(bytes) = paths.iter().find_map(|path| std::fs::read(path).ok()) {
        fonts
            .font_data
            .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
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

/// 应用主题：先把激活主题切到 `selected_theme`，再逐段重建全局 `Style` 并一次性写回。
///
/// 顺序约束：`set_active_theme` 必须早于下面所有 `theme::xxx()` 取色，否则取到的
/// 还是上一个主题的颜色；整份 `Style` 基于开头那次 `global_style()` 克隆构造，
/// 中途不要再读 ctx，最后统一由 `set_global_style` 写回。
pub fn apply_theme(ctx: &egui::Context, selected_theme: theme::AppTheme) {
    theme::set_active_theme(selected_theme);
    let is_dark = theme::active_theme_is_dark();
    let (egui_theme, system_theme) = if is_dark {
        (egui::Theme::Dark, egui::SystemTheme::Dark)
    } else {
        (egui::Theme::Light, egui::SystemTheme::Light)
    };
    ctx.set_theme(egui_theme);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system_theme));
    let is_catppuccin = theme::is_catppuccin();
    let mut s = (*ctx.global_style()).clone();

    apply_spacing_style(&mut s);
    apply_interaction_style(&mut s);
    #[cfg(debug_assertions)]
    disable_debug_overlays(&mut s);
    apply_text_styles(&mut s);
    s.visuals = build_visuals(is_dark, is_catppuccin);

    ctx.set_global_style(s);
}

fn apply_spacing_style(s: &mut egui::Style) {
    // ── 间距 ──
    // 行距略大于列距：深色界面上文字块之间需要更明显的纵向呼吸感，否则整页会
    // 挤成一团。基础控件 30px 高，design 的主/次按钮再高 2px 以突出层级。
    s.spacing.item_spacing = egui::vec2(10.0, 10.0);
    s.spacing.button_padding = egui::vec2(14.0, 7.0);
    s.spacing.interact_size = egui::vec2(44.0, 30.0);
    s.spacing.slider_width = 180.0;
    s.spacing.combo_width = 140.0;
    s.spacing.text_edit_width = 220.0;
    s.spacing.indent = 16.0;
    s.spacing.window_margin = egui::Margin::same(12);
    s.spacing.menu_margin = egui::Margin::same(8);
    s.spacing.scroll = egui::style::ScrollStyle {
        bar_width: 10.0,
        handle_min_length: 30.0,
        ..egui::style::ScrollStyle::solid()
    };
}

fn apply_interaction_style(s: &mut egui::Style) {
    // ── 交互 ──
    s.interaction.resize_grab_radius_side = 6.0;
    s.interaction.resize_grab_radius_corner = 10.0;
    s.interaction.show_tooltips_only_when_still = false;
    s.interaction.tooltip_delay = 0.3;

    // ── 动画 ──
    s.animation_time = 0.16;
}

#[cfg(debug_assertions)]
fn disable_debug_overlays(s: &mut egui::Style) {
    s.debug.show_interactive_widgets = false;
    s.debug.show_focused_widget = false;
    s.debug.show_unaligned = false;
    s.debug.warn_if_rect_changes_id = false;
    s.debug.show_resize = false;
    s.debug.show_widget_hits = false;
}

fn apply_text_styles(s: &mut egui::Style) {
    // ── 字体大小 ──
    // 中文在 13px 上下会明显发虚；正文抬到 15px，按钮 14px，等宽 14px 让日志
    // 与终端在 100% 缩放下仍可读。
    s.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::new(15.0, egui::FontFamily::Proportional),
    );
    s.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
    );
    s.text_styles.insert(
        egui::TextStyle::Monospace,
        egui::FontId::new(14.0, egui::FontFamily::Monospace),
    );
    s.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
    );
    s.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::new(19.0, egui::FontFamily::Proportional),
    );
}

/// 顺序不能调：基础配色 → catppuccin 覆盖 → widget 样式 → catppuccin widget 覆盖。
/// catppuccin 的两个分支必须晚于对应的基础设置，否则会被基础配色重新盖掉。
fn build_visuals(is_dark: bool, is_catppuccin: bool) -> egui::Visuals {
    let mut v = base_visuals(is_dark);
    if is_catppuccin {
        apply_catppuccin_visuals(&mut v, is_dark);
    }
    apply_widget_visuals(&mut v);
    if is_catppuccin {
        apply_catppuccin_widget_visuals(&mut v);
    }
    v
}

fn base_visuals(is_dark: bool) -> egui::Visuals {
    // ── Visuals ──
    let mut v = if is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    // 圆角：面板圆角略大，控件圆角跟随 design::CARD_RADIUS 的 10px 语言，
    // 避免 6/8/10 三种半径混在同一屏里。
    v.window_corner_radius = 10.into();
    v.menu_corner_radius = 8.into();
    v.window_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: egui::Color32::BLACK.gamma_multiply(0.28),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: egui::Color32::BLACK.gamma_multiply(0.30),
    };

    // 背景 — 使用新的层级色
    v.panel_fill = theme::bg_primary();
    v.window_fill = theme::bg_deep();
    v.extreme_bg_color = theme::bg_deep();
    v.faint_bg_color = theme::bg_card();
    v.code_bg_color = theme::bg_input();
    v.text_edit_bg_color = Some(theme::bg_input());

    // 文本
    v.override_text_color = Some(theme::text_primary());
    v.weak_text_color = Some(theme::text_secondary());
    v.warn_fg_color = theme::yellow();
    v.error_fg_color = theme::red();
    v.hyperlink_color = theme::cyan();

    // 选中
    v.selection.bg_fill = theme::bg_selection();
    v.selection.stroke = egui::Stroke::new(1.0, theme::blue());

    // 窗口
    v.window_stroke = egui::Stroke::new(1.0, theme::separator());
    v.resize_corner_size = 8.0;
    v.striped = true;
    v.collapsing_header_frame = false;
    v.window_highlight_topmost = false;
    v.button_frame = true;
    v.indent_has_left_vline = false;

    v
}

/// catppuccin-egui 的官方映射。该 crate 目前只支持到 egui 0.33，
/// 项目使用 0.35，因此在保持当前 egui 类型一致的前提下移植其 Visuals 规则。
fn apply_catppuccin_visuals(v: &mut egui::Visuals, is_dark: bool) {
    v.panel_fill = theme::bg_primary();
    v.window_fill = theme::bg_primary();
    v.extreme_bg_color = theme::bg_deep();
    v.faint_bg_color = theme::bg_tertiary();
    v.code_bg_color = theme::bg_secondary();
    v.text_edit_bg_color = Some(theme::bg_secondary());
    v.window_stroke = egui::Stroke::new(1.0, theme::overlay1());
    v.selection.bg_fill = theme::blue().linear_multiply(if is_dark { 0.2 } else { 0.4 });
    v.selection.stroke = egui::Stroke::new(1.0, theme::text_primary());
}

fn apply_widget_visuals(v: &mut egui::Visuals) {
    // ── Widget 样式 ──
    let w = &mut v.widgets;

    // noninteractive（标签、分隔线等）
    w.noninteractive.corner_radius = 6.into();
    w.noninteractive.bg_fill = theme::bg_card();
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, theme::border());
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::text_primary());
    w.noninteractive.weak_bg_fill = theme::bg_secondary();

    // inactive（按钮、输入框等未交互状态）
    w.inactive.corner_radius = 8.into();
    // 滚动条 handle（solid 模式，scroll_area.rs:1457-1466 走 &visuals.widgets.inactive.bg_fill）、
    // 也作为按钮/输入框等未交互状态的默认填充色。#373C47 中灰对两者都合适。
    w.inactive.bg_fill = theme::scrollbar();
    // 默认按钮原先用 bg_input（比卡片还暗），在深色面板上只剩一圈几乎看不见的
    // 描边；用比面板亮一层的 bg_tertiary，普通按钮才有可点击的实体感。
    w.inactive.weak_bg_fill = theme::bg_tertiary();
    w.inactive.bg_stroke = egui::Stroke::new(1.0, theme::border());
    w.inactive.fg_stroke = egui::Stroke::new(1.0, theme::text_primary());

    // hovered
    w.hovered.corner_radius = 8.into();
    w.hovered.bg_fill = theme::bg_hover();
    w.hovered.weak_bg_fill = theme::bg_hover();
    w.hovered.bg_stroke = egui::Stroke::new(1.0, theme::border_light());
    w.hovered.fg_stroke = egui::Stroke::new(1.0, theme::text_white());

    // active（按下/选中）
    w.active.corner_radius = 8.into();
    w.active.bg_fill = theme::widget_active_weak();
    w.active.weak_bg_fill = theme::widget_active_weak();
    w.active.bg_stroke = egui::Stroke::new(1.0, theme::blue());
    w.active.fg_stroke = egui::Stroke::new(1.0, theme::text_white());

    // open（展开状态）
    w.open.corner_radius = 8.into();
    w.open.bg_fill = theme::widget_open();
    w.open.weak_bg_fill = theme::bg_tertiary();
    w.open.bg_stroke = egui::Stroke::new(1.0, theme::border_light());
    w.open.fg_stroke = egui::Stroke::new(1.0, theme::text_primary());
}

fn apply_catppuccin_widget_visuals(v: &mut egui::Visuals) {
    let w = &mut v.widgets;

    let widget_stroke = egui::Stroke::new(1.0, theme::overlay1());
    let text_stroke = egui::Stroke::new(1.0, theme::text_primary());
    for widget in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        widget.bg_stroke = widget_stroke;
        widget.fg_stroke = text_stroke;
    }
    w.noninteractive.bg_fill = theme::bg_primary();
    w.noninteractive.weak_bg_fill = theme::bg_primary();
    w.inactive.bg_fill = theme::bg_tertiary();
    w.inactive.weak_bg_fill = theme::bg_tertiary();
    w.hovered.bg_fill = theme::surface2();
    w.hovered.weak_bg_fill = theme::surface2();
    w.active.bg_fill = theme::surface1();
    w.active.weak_bg_fill = theme::surface1();
    w.open.bg_fill = theme::bg_tertiary();
    w.open.weak_bg_fill = theme::bg_tertiary();
}
