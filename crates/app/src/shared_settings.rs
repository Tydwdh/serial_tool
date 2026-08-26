//! Settings navigation shared by the Native and Web composition roots.

use eframe::egui;
use egui_material_icons::icons::{
    ICON_APPS, ICON_DATA_USAGE, ICON_INFO, ICON_KEYBOARD, ICON_SETTINGS,
};
use tool_panels::design;

pub(crate) const SETTINGS_NAV_BUTTON_SIZE: egui::Vec2 = egui::vec2(136.0, 32.0);

pub(crate) const SETTINGS_NAV_ITEMS: [(usize, egui_material_icons::MaterialIcon, &str); 5] = [
    (0, ICON_SETTINGS, "常规"),
    (1, ICON_DATA_USAGE, "连接与数据"),
    (2, ICON_KEYBOARD, "快捷键"),
    (3, ICON_APPS, "插件设置"),
    (4, ICON_INFO, "关于与重置"),
];

pub(crate) fn settings_nav_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon: egui_material_icons::MaterialIcon,
    label: &str,
) -> egui::Response {
    ui.add_sized(
        SETTINGS_NAV_BUTTON_SIZE,
        egui::Button::selectable(selected, design::icon_text(icon, label)).corner_radius(7.0),
    )
}
