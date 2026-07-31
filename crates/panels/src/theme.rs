use egui::Color32;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        OnceLock, RwLock,
        atomic::{AtomicU8, Ordering},
    },
};

/// 可选界面配色。One Dark Pro 系列取自官方 VS Code 主题；Catppuccin 配色取自官方调色板库。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    #[default]
    OneDarkPro,
    OneDarkProFlat,
    OneDarkProDarker,
    OneDarkProMix,
    OneDarkProNightFlat,
    Custom,
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl AppTheme {
    pub const ALL: [Self; 9] = [
        Self::OneDarkPro,
        Self::OneDarkProFlat,
        Self::OneDarkProDarker,
        Self::OneDarkProMix,
        Self::OneDarkProNightFlat,
        Self::Latte,
        Self::Frappe,
        Self::Macchiato,
        Self::Mocha,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::OneDarkPro => "One Dark Pro（默认）",
            Self::OneDarkProFlat => "One Dark Pro Flat",
            Self::OneDarkProDarker => "One Dark Pro Darker",
            Self::OneDarkProMix => "One Dark Pro Mix",
            Self::OneDarkProNightFlat => "One Dark Pro Night Flat",
            Self::Custom => "自定义 JSON 主题",
            Self::Latte => "Catppuccin Latte（浅色）",
            Self::Frappe => "Catppuccin Frappé（柔和深色）",
            Self::Macchiato => "Catppuccin Macchiato（深色）",
            Self::Mocha => "Catppuccin Mocha（深色）",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::OneDarkPro => "经典编辑器深色配色，高对比且适合长时间工作",
            Self::OneDarkProFlat => "扁平层级、减少区域明暗差异",
            Self::OneDarkProDarker => "更深的 #23272e 编辑区背景",
            Self::OneDarkProMix => "默认编辑区搭配更深的工具栏与标签栏",
            Self::OneDarkProNightFlat => "最深的 #16191d 夜间编辑区背景",
            Self::Custom => "从 themes 目录加载的自定义 JSON 配色",
            Self::Latte => "明亮、低疲劳的浅色界面",
            Self::Frappe => "柔和、偏灰蓝的深色界面",
            Self::Macchiato => "对比适中的暖调深色界面",
            Self::Mocha => "对比鲜明的深紫蓝界面",
        }
    }

    pub const fn is_dark(self) -> bool {
        !matches!(self, Self::Latte)
    }

    const fn from_index(index: u8) -> Self {
        match index {
            0 => Self::OneDarkPro,
            1 => Self::OneDarkProFlat,
            2 => Self::OneDarkProDarker,
            3 => Self::OneDarkProMix,
            4 => Self::OneDarkProNightFlat,
            5 => Self::Latte,
            6 => Self::Frappe,
            7 => Self::Macchiato,
            8 => Self::Mocha,
            _ => Self::Custom,
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::OneDarkPro => 0,
            Self::OneDarkProFlat => 1,
            Self::OneDarkProDarker => 2,
            Self::OneDarkProMix => 3,
            Self::OneDarkProNightFlat => 4,
            Self::Latte => 5,
            Self::Frappe => 6,
            Self::Macchiato => 7,
            Self::Mocha => 8,
            Self::Custom => 9,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct ThemeColors {
    base: Color32,
    mantle: Color32,
    crust: Color32,
    card: Color32,
    input: Color32,
    surface0: Color32,
    surface1: Color32,
    surface2: Color32,
    overlay0: Color32,
    overlay1: Color32,
    text: Color32,
    text_white: Color32,
    subtext0: Color32,
    subtext1: Color32,
    red: Color32,
    green: Color32,
    yellow: Color32,
    blue: Color32,
    mauve: Color32,
    teal: Color32,
    peach: Color32,
}

/// 用户主题文件。`base` 可继承任一内置主题，`colors` 只需填写要覆盖的颜色。
#[derive(Debug, Clone, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    #[serde(default)]
    pub dark_mode: Option<bool>,
    #[serde(default)]
    pub base: Option<AppTheme>,
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
    /// 可选的组件状态颜色，例如当前标签页的背景、边框和前景色。
    #[serde(default)]
    pub ui: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Default)]
struct ThemeUi {
    selection: Option<Color32>,
    active: Option<Color32>,
    tab_bar_background: Option<Color32>,
    tab_bar_border: Option<Color32>,
    tab_active_background: Option<Color32>,
    tab_active_border: Option<Color32>,
    tab_active_foreground: Option<Color32>,
    tab_inactive_background: Option<Color32>,
    tab_inactive_foreground: Option<Color32>,
    toggle_selected_background: Option<Color32>,
    toggle_selected_foreground: Option<Color32>,
    toggle_selected_border: Option<Color32>,
}

#[derive(Clone)]
struct CustomTheme {
    name: String,
    dark_mode: bool,
    colors: ThemeColors,
    ui: ThemeUi,
}

const BUNDLED_THEME_FILES: [(&str, &str); 9] = [
    (
        "one-dark-pro.json",
        include_str!("../../../assets/themes/one-dark-pro.json"),
    ),
    (
        "one-dark-pro-flat.json",
        include_str!("../../../assets/themes/one-dark-pro-flat.json"),
    ),
    (
        "one-dark-pro-darker.json",
        include_str!("../../../assets/themes/one-dark-pro-darker.json"),
    ),
    (
        "one-dark-pro-mix.json",
        include_str!("../../../assets/themes/one-dark-pro-mix.json"),
    ),
    (
        "one-dark-pro-night-flat.json",
        include_str!("../../../assets/themes/one-dark-pro-night-flat.json"),
    ),
    (
        "catppuccin-latte.json",
        include_str!("../../../assets/themes/catppuccin-latte.json"),
    ),
    (
        "catppuccin-frappe.json",
        include_str!("../../../assets/themes/catppuccin-frappe.json"),
    ),
    (
        "catppuccin-macchiato.json",
        include_str!("../../../assets/themes/catppuccin-macchiato.json"),
    ),
    (
        "catppuccin-mocha.json",
        include_str!("../../../assets/themes/catppuccin-mocha.json"),
    ),
];

static CUSTOM_THEME: OnceLock<RwLock<Option<CustomTheme>>> = OnceLock::new();

fn custom_theme_store() -> &'static RwLock<Option<CustomTheme>> {
    CUSTOM_THEME.get_or_init(|| RwLock::new(None))
}

pub fn current_theme_name() -> Option<String> {
    custom_theme_store()
        .read()
        .ok()
        .and_then(|theme| theme.as_ref().map(|theme| theme.name.clone()))
}

pub fn custom_theme_is_dark() -> bool {
    custom_theme_store()
        .read()
        .ok()
        .and_then(|theme| theme.as_ref().map(|theme| theme.dark_mode))
        .unwrap_or(true)
}

pub fn load_theme_file(path: &Path) -> Result<String, String> {
    let custom = parse_theme_file(path, 0)?;
    let name = custom.name.clone();
    *custom_theme_store()
        .write()
        .map_err(|_| "主题配置锁不可用".to_owned())? = Some(custom);
    Ok(name)
}

fn parse_theme_file(path: &Path, depth: u8) -> Result<CustomTheme, String> {
    if depth > 8 {
        return Err("主题 base 继承层级过深或存在循环".to_owned());
    }
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("读取主题 {} 失败：{error}", path.display()))?;
    parse_theme_source(&source, &path.display().to_string(), depth)
}

fn parse_theme_source(source: &str, source_name: &str, depth: u8) -> Result<CustomTheme, String> {
    if depth > 8 {
        return Err("主题 base 继承层级过深或存在循环".to_owned());
    }
    let file: ThemeFile = serde_json::from_str(source)
        .map_err(|error| format!("解析主题 {source_name} 失败：{error}"))?;
    let (mut colors, mut ui, inherited_dark) = match file.base {
        Some(AppTheme::Custom) => return Err("自定义主题不能以 custom 作为 base".to_owned()),
        Some(base) => {
            let inherited = parse_builtin_theme(base, depth + 1)?;
            (inherited.colors, inherited.ui, inherited.dark_mode)
        }
        None => (ThemeColors::default(), ThemeUi::default(), true),
    };
    for (key, value) in &file.colors {
        apply_color_override(&mut colors, key, value)?;
    }
    if colors.card == Color32::TRANSPARENT {
        colors.card = colors.mantle;
    }
    if colors.input == Color32::TRANSPARENT {
        colors.input = colors.surface0;
    }
    if colors.text_white == Color32::TRANSPARENT {
        colors.text_white = colors.text;
    }
    for (key, value) in &file.ui {
        apply_ui_override(&mut ui, key, value)?;
    }
    Ok(CustomTheme {
        name: file.name,
        dark_mode: file.dark_mode.unwrap_or(inherited_dark),
        colors,
        ui,
    })
}

fn parse_builtin_theme(theme: AppTheme, depth: u8) -> Result<CustomTheme, String> {
    let file = builtin_theme_file(theme).ok_or_else(|| "custom 不是内置主题".to_owned())?;
    let source = bundled_theme_source(file).ok_or_else(|| format!("内置主题资源不存在：{file}"))?;
    parse_theme_source(source, file, depth)
}

pub fn load_builtin_theme(theme: AppTheme, _dir: &Path) -> Result<String, String> {
    let custom = parse_builtin_theme(theme, 0)?;
    let name = custom.name.clone();
    *custom_theme_store()
        .write()
        .map_err(|_| "主题配置锁不可用".to_owned())? = Some(custom);
    Ok(name)
}

/// 内置主题对应的 JSON 文件路径。UI 和持久化统一按此路径标识主题。
pub fn builtin_theme_path(theme: AppTheme, dir: &Path) -> Option<PathBuf> {
    builtin_theme_file(theme).map(|file| dir.join(file))
}

/// 若路径是内置主题文件，返回其内部主题标识；其他 JSON 使用 `Custom` 作为运行时回退。
pub fn builtin_theme_for_path(path: &Path) -> Option<AppTheme> {
    let file = path.file_name()?.to_str()?;
    AppTheme::ALL
        .into_iter()
        .find(|theme| builtin_theme_file(*theme) == Some(file))
}

fn builtin_theme_file(theme: AppTheme) -> Option<&'static str> {
    Some(match theme {
        AppTheme::OneDarkPro => "one-dark-pro.json",
        AppTheme::OneDarkProFlat => "one-dark-pro-flat.json",
        AppTheme::OneDarkProDarker => "one-dark-pro-darker.json",
        AppTheme::OneDarkProMix => "one-dark-pro-mix.json",
        AppTheme::OneDarkProNightFlat => "one-dark-pro-night-flat.json",
        AppTheme::Latte => "catppuccin-latte.json",
        AppTheme::Frappe => "catppuccin-frappe.json",
        AppTheme::Macchiato => "catppuccin-macchiato.json",
        AppTheme::Mocha => "catppuccin-mocha.json",
        AppTheme::Custom => return None,
    })
}

fn bundled_theme_source(file: &str) -> Option<&'static str> {
    BUNDLED_THEME_FILES
        .iter()
        .find_map(|(name, source)| (*name == file).then_some(*source))
}

/// 枚举主题目录中的所有 JSON 文件；内置和用户新增文件使用同一套选择逻辑。
pub fn discover_theme_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut themes = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".example.json"))
        })
        .filter_map(|path| {
            let source = std::fs::read_to_string(&path).ok()?;
            let file = serde_json::from_str::<ThemeFile>(&source).ok()?;
            Some((path, file.name))
        })
        .collect::<Vec<_>>();
    themes.sort_by(|left, right| left.1.cmp(&right.1));
    themes
}

pub fn ensure_theme_directory(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("创建主题目录 {} 失败：{error}", dir.display()))?;
    for (file, source) in BUNDLED_THEME_FILES {
        let path = dir.join(file);
        std::fs::write(&path, source)
            .map_err(|error| format!("同步内置主题 {} 失败：{error}", path.display()))?;
    }
    // 模板供用户复制后修改，不作为可选主题出现在下拉列表中。
    let example = dir.join("custom-theme.example.json");
    if !example.exists() {
        std::fs::write(
            &example,
            r##"{
  "name": "My Theme",
  "base": "one_dark_pro",
  "dark_mode": true,
  "colors": {
    "bg_primary": "#282c34",
    "blue": "#61afef"
  },
  "ui": {
    "tab_active_background": "#282c34",
    "tab_active_border": "#00000000",
    "tab_active_foreground": "#dcdcdc",
    "tab_inactive_background": "#21252b",
    "tab_inactive_foreground": "#9da5b4",
    "tab_bar_background": "#21252b",
    "tab_bar_border": "#00000000",
    "toggle_selected_background": "#2c5884",
    "toggle_selected_foreground": "#e8ecf3",
    "toggle_selected_border": "#508cd2"
  }
}"##,
        )
        .map_err(|error| format!("写入主题示例 {} 失败：{error}", example.display()))?;
    }
    Ok(())
}

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(AppTheme::OneDarkPro.index());

pub fn active_theme() -> AppTheme {
    AppTheme::from_index(ACTIVE_THEME.load(Ordering::Relaxed))
}

pub fn set_active_theme(theme: AppTheme) {
    ACTIVE_THEME.store(theme.index(), Ordering::Relaxed);
}

fn colors() -> ThemeColors {
    if let Ok(custom) = custom_theme_store().read()
        && let Some(custom) = custom.as_ref()
    {
        return custom.colors;
    }
    ThemeColors::default()
}

fn ui() -> ThemeUi {
    if let Ok(custom) = custom_theme_store().read()
        && let Some(custom) = custom.as_ref()
    {
        return custom.ui;
    }
    ThemeUi::default()
}

pub fn active_theme_is_dark() -> bool {
    if active_theme() == AppTheme::Custom {
        custom_theme_is_dark()
    } else {
        active_theme().is_dark()
    }
}

/// Catppuccin 的四个内置主题使用其官方 egui `Visuals` 映射。
pub fn is_catppuccin() -> bool {
    matches!(
        active_theme(),
        AppTheme::Latte | AppTheme::Frappe | AppTheme::Macchiato | AppTheme::Mocha
    )
}

fn parse_hex_color(value: &str) -> Result<Color32, String> {
    let hex = value.trim().trim_start_matches('#');
    let parse = |range: std::ops::Range<usize>| {
        u8::from_str_radix(&hex[range], 16).map_err(|_| format!("无效颜色值：{value}"))
    };
    match hex.len() {
        6 => Ok(Color32::from_rgb(parse(0..2)?, parse(2..4)?, parse(4..6)?)),
        8 => Ok(Color32::from_rgba_unmultiplied(
            parse(0..2)?,
            parse(2..4)?,
            parse(4..6)?,
            parse(6..8)?,
        )),
        _ => Err(format!("颜色必须是 #RRGGBB 或 #RRGGBBAA：{value}")),
    }
}

fn apply_color_override(colors: &mut ThemeColors, key: &str, value: &str) -> Result<(), String> {
    let color = parse_hex_color(value)?;
    match key {
        "bg_primary" => colors.base = color,
        "bg_secondary" => colors.mantle = color,
        "bg_deep" => colors.crust = color,
        "bg_tertiary" => colors.surface0 = color,
        "bg_card" => colors.card = color,
        "bg_input" => colors.input = color,
        "bg_hover" => colors.surface1 = color,
        "border" => colors.surface2 = color,
        "border_light" | "separator" => colors.overlay0 = color,
        "separator_strong" => colors.overlay1 = color,
        "text_primary" => colors.text = color,
        "text_white" => colors.text_white = color,
        "text_dimmed" => colors.subtext0 = color,
        "text_secondary" => colors.subtext1 = color,
        "red" => colors.red = color,
        "green" => colors.green = color,
        "yellow" => colors.yellow = color,
        "blue" => colors.blue = color,
        "purple" => colors.mauve = color,
        "cyan" => colors.teal = color,
        "orange" => colors.peach = color,
        unknown => return Err(format!("不支持的主题颜色字段：{unknown}")),
    }
    Ok(())
}

fn apply_ui_override(ui: &mut ThemeUi, key: &str, value: &str) -> Result<(), String> {
    let color = parse_hex_color(value)?;
    match key {
        "selection" => ui.selection = Some(color),
        "active" => ui.active = Some(color),
        "tab_bar_background" => ui.tab_bar_background = Some(color),
        "tab_bar_border" => ui.tab_bar_border = Some(color),
        "tab_active_background" => ui.tab_active_background = Some(color),
        "tab_active_border" => ui.tab_active_border = Some(color),
        "tab_active_foreground" => ui.tab_active_foreground = Some(color),
        "tab_inactive_background" => ui.tab_inactive_background = Some(color),
        "tab_inactive_foreground" => ui.tab_inactive_foreground = Some(color),
        "toggle_selected_background" => ui.toggle_selected_background = Some(color),
        "toggle_selected_foreground" => ui.toggle_selected_foreground = Some(color),
        "toggle_selected_border" => ui.toggle_selected_border = Some(color),
        unknown => return Err(format!("不支持的主题 UI 字段：{unknown}")),
    }
    Ok(())
}

fn translucent(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

// 背景层级
pub fn bg_deep() -> Color32 {
    colors().crust
}
pub fn bg_primary() -> Color32 {
    colors().base
}
pub fn bg_secondary() -> Color32 {
    colors().mantle
}
pub fn bg_tertiary() -> Color32 {
    colors().surface0
}
pub fn surface1() -> Color32 {
    colors().surface1
}
pub fn surface2() -> Color32 {
    colors().surface2
}
pub fn overlay1() -> Color32 {
    colors().overlay1
}
pub fn bg_card() -> Color32 {
    colors().card
}
pub fn bg_input() -> Color32 {
    colors().input
}
pub fn bg_selection() -> Color32 {
    ui().selection
        .unwrap_or_else(|| translucent(colors().blue, 100))
}
pub fn bg_hover() -> Color32 {
    let colors = colors();
    if active_theme() == AppTheme::Latte {
        colors.surface2
    } else {
        colors.surface1
    }
}

// 文本层级
pub fn text_primary() -> Color32 {
    colors().text
}
pub fn text_secondary() -> Color32 {
    colors().subtext1
}
pub fn text_dimmed() -> Color32 {
    colors().subtext0
}
pub fn text_white() -> Color32 {
    colors().text_white
}

// 语义色
pub fn red() -> Color32 {
    colors().red
}
pub fn green() -> Color32 {
    colors().green
}
pub fn yellow() -> Color32 {
    colors().yellow
}
pub fn blue() -> Color32 {
    colors().blue
}
pub fn purple() -> Color32 {
    colors().mauve
}
pub fn cyan() -> Color32 {
    colors().teal
}
pub fn orange() -> Color32 {
    colors().peach
}

pub fn red_bg() -> Color32 {
    translucent(red(), 70)
}
pub fn green_bg() -> Color32 {
    translucent(green(), 70)
}
pub fn yellow_bg() -> Color32 {
    translucent(yellow(), 70)
}
pub fn blue_bg() -> Color32 {
    translucent(blue(), 70)
}
pub fn purple_bg() -> Color32 {
    translucent(purple(), 70)
}
pub fn cyan_bg() -> Color32 {
    translucent(cyan(), 70)
}
pub fn orange_bg() -> Color32 {
    translucent(orange(), 70)
}

// 卡片、控件、边框
pub fn card_accent_device() -> Color32 {
    blue()
}
pub fn card_accent_record() -> Color32 {
    red()
}
pub fn card_accent_port() -> Color32 {
    green()
}
pub fn card_accent_replay() -> Color32 {
    cyan()
}
pub fn card_accent_plugin() -> Color32 {
    purple()
}
pub fn card_accent_settings() -> Color32 {
    orange()
}
pub fn widget_bg() -> Color32 {
    bg_tertiary()
}
pub fn widget_hover() -> Color32 {
    bg_hover()
}
pub fn widget_active() -> Color32 {
    blue()
}
pub fn widget_active_weak() -> Color32 {
    ui().active.unwrap_or_else(|| translucent(blue(), 95))
}
pub fn widget_active_strong() -> Color32 {
    ui().active.unwrap_or_else(|| translucent(blue(), 135))
}
pub fn widget_open() -> Color32 {
    bg_hover()
}
pub fn scrollbar() -> Color32 {
    let colors = colors();
    if active_theme() == AppTheme::Latte {
        colors.surface0
    } else {
        colors.surface1
    }
}
pub fn scrollbar_hover() -> Color32 {
    let colors = colors();
    if active_theme() == AppTheme::Latte {
        colors.overlay0
    } else {
        colors.surface2
    }
}
pub fn border() -> Color32 {
    colors().surface2
}
pub fn border_light() -> Color32 {
    colors().overlay0
}
pub fn border_dark() -> Color32 {
    colors().crust
}
pub fn separator() -> Color32 {
    colors().overlay0
}
pub fn separator_strong() -> Color32 {
    colors().overlay1
}
pub fn nav_highlight() -> Color32 {
    translucent(blue(), 70)
}

/// 编辑器标签遵循 VS Code 的层级：激活标签融入编辑区，未激活标签融入标签栏。
pub fn tab_bar_bg() -> Color32 {
    ui().tab_bar_background.unwrap_or_else(bg_secondary)
}

pub fn tab_bar_outline() -> Color32 {
    ui().tab_bar_border.unwrap_or(Color32::TRANSPARENT)
}

pub fn tab_active_bg() -> Color32 {
    ui().tab_active_background.unwrap_or_else(bg_primary)
}

pub fn tab_active_outline() -> Color32 {
    ui().tab_active_border.unwrap_or(Color32::TRANSPARENT)
}

pub fn tab_active_text() -> Color32 {
    ui().tab_active_foreground.unwrap_or_else(text_white)
}

pub fn tab_inactive_bg() -> Color32 {
    ui().tab_inactive_background.unwrap_or_else(tab_bar_bg)
}

pub fn tab_inactive_text() -> Color32 {
    ui().tab_inactive_foreground.unwrap_or_else(text_secondary)
}

/// 持久开关（例如搜索栏 `Aa`）的启用态，独立于半透明的文本选区色。
pub fn toggle_selected_bg() -> Color32 {
    ui().toggle_selected_background
        .unwrap_or_else(|| translucent(blue(), 180))
}

pub fn toggle_selected_text() -> Color32 {
    ui().toggle_selected_foreground.unwrap_or_else(text_white)
}

pub fn toggle_selected_border() -> Color32 {
    ui().toggle_selected_border.unwrap_or_else(blue)
}

pub fn chart_colors() -> [Color32; 6] {
    [green(), blue(), yellow(), red(), purple(), cyan()]
}
pub fn chart_bg() -> Color32 {
    bg_secondary()
}
pub fn chart_grid() -> Color32 {
    colors().surface1
}
pub fn chart_crosshair() -> Color32 {
    colors().overlay1
}
pub fn chart_tooltip_bg() -> Color32 {
    colors().mantle
}
pub fn attitude_bg() -> Color32 {
    bg_secondary()
}
pub fn attitude_body() -> Color32 {
    yellow()
}
pub fn attitude_body_edge() -> Color32 {
    orange()
}
pub fn attitude_axis_x() -> Color32 {
    red()
}
pub fn attitude_axis_y() -> Color32 {
    green()
}
pub fn attitude_axis_z() -> Color32 {
    blue()
}
pub fn gauge_bg() -> Color32 {
    bg_secondary()
}
pub fn gauge_arc() -> Color32 {
    colors().surface2
}
pub fn gauge_value() -> Color32 {
    cyan()
}

/// 绘制带左边框色彩指示器的卡片标题行。
pub fn card_accent_bar(ui: &mut egui::Ui, color: Color32) {
    let rect = ui.available_rect_before_wrap();
    let accent_rect = egui::Rect::from_min_size(
        rect.left_top(),
        egui::vec2(3.0, rect.height().max(ui.spacing().interact_size.y)),
    );
    ui.painter().rect_filled(accent_rect, 2.0, color);
    ui.add_space(6.0);
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_theme_selects_the_expected_palette_colors() {
        let saved = active_theme();

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes");
        load_builtin_theme(AppTheme::OneDarkPro, &root).expect("bundled One Dark Pro loads");
        set_active_theme(AppTheme::OneDarkPro);
        assert_eq!(bg_primary(), Color32::from_rgb(35, 39, 47));
        assert_eq!(bg_card(), Color32::from_rgb(38, 43, 53));
        assert_eq!(bg_input(), Color32::from_rgb(31, 36, 45));
        assert_eq!(tab_active_bg(), Color32::from_rgb(40, 44, 52));
        assert_eq!(tab_inactive_bg(), Color32::from_rgb(33, 37, 43));
        assert_eq!(tab_active_outline(), Color32::TRANSPARENT);
        assert_eq!(tab_active_text(), Color32::from_rgb(220, 220, 220));

        load_builtin_theme(AppTheme::Latte, &root).expect("bundled Latte loads");
        set_active_theme(AppTheme::Latte);
        assert_eq!(bg_primary(), Color32::from_rgb(239, 241, 245));
        assert_eq!(blue(), Color32::from_rgb(30, 102, 245));

        set_active_theme(saved);
    }

    #[test]
    fn themes_expose_one_dark_pro_and_all_catppuccin_choices() {
        assert_eq!(AppTheme::ALL.len(), 9);
        assert_eq!(AppTheme::default(), AppTheme::OneDarkPro);
        assert!(!AppTheme::Latte.is_dark());
        assert!(AppTheme::OneDarkPro.is_dark());
    }

    #[test]
    fn every_bundled_theme_file_loads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes");
        for theme in AppTheme::ALL {
            load_builtin_theme(theme, &root).expect("bundled theme should parse");
            assert_ne!(bg_primary(), Color32::TRANSPARENT);
        }
    }

    #[test]
    fn catppuccin_variants_use_the_official_egui_surface_hierarchy() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/themes");
        for theme in [
            AppTheme::Latte,
            AppTheme::Frappe,
            AppTheme::Macchiato,
            AppTheme::Mocha,
        ] {
            load_builtin_theme(theme, &root).expect("bundled Catppuccin theme loads");
            set_active_theme(theme);
            assert!(is_catppuccin());
            assert_eq!(tab_active_bg(), bg_primary());
            assert_eq!(tab_inactive_bg(), bg_secondary());
            assert_eq!(tab_active_outline(), Color32::TRANSPARENT);
        }
    }

    #[test]
    fn custom_theme_colors_override_the_selected_base() {
        let file: ThemeFile = serde_json::from_str(
            r##"{
                "name": "Custom",
                "base": "one_dark_pro_darker",
                "colors": { "bg_primary": "#010203", "bg_secondary": "#1E2227", "blue": "#040506" }
            }"##,
        )
        .expect("valid custom theme JSON");
        let mut colors = ThemeColors::default();
        for (key, value) in &file.colors {
            apply_color_override(&mut colors, key, value).expect("valid color override");
        }
        assert_eq!(colors.base, Color32::from_rgb(1, 2, 3));
        assert_eq!(colors.blue, Color32::from_rgb(4, 5, 6));
        assert_eq!(colors.mantle, Color32::from_rgb(30, 34, 39));
    }
}
