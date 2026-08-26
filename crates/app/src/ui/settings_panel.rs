use crate::app::WorkbenchApp;
use crate::bootstrap::apply_theme;
use crate::config::{config_path, default_recorder_path};
use crate::shared_settings::{SETTINGS_NAV_ITEMS, settings_nav_button};
use crate::state::StatusLevel;
use eframe::egui;
use egui_material_icons::icons::{
    ICON_APPS, ICON_CONTENT_COPY, ICON_FOLDER, ICON_FOLDER_OPEN, ICON_INFO, ICON_KEYBOARD,
    ICON_NETWORK_CHECK, ICON_PALETTE, ICON_RESTART_ALT, ICON_TUNE,
};
use std::path::{Path, PathBuf};
use tool_databus::EventPublisher;
use tool_panels::{
    DataSettingsView, KeymapAction, KeymapEntry, PluginSettingsView, copy_text_with_feedback,
    data_settings_ui,
    design::{self, ButtonKind},
    keymap_ui, plugin_settings_ui, theme,
};

const CONFIG_LOCATION_LABEL_WIDTH: f32 = 96.0;
const REPOSITORY_URL: &str = env!("CARGO_PKG_REPOSITORY");

impl WorkbenchApp {
    pub(crate) fn settings_panel(&mut self, ui: &mut egui::Ui) {
        let nav_id = ui.id().with("settings_category");
        let mut category = ui
            .ctx()
            .data_mut(|data| data.get_persisted::<usize>(nav_id))
            .unwrap_or(0)
            .min(4);
        design::elevated_card().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, icon, label) in SETTINGS_NAV_ITEMS {
                    if settings_nav_button(ui, category == index, icon, label).clicked() {
                        category = index;
                    }
                }
            });
        });
        ui.ctx()
            .data_mut(|data| data.insert_persisted(nav_id, category));
        ui.add_space(design::SECTION_GAP);

        if category == 0 {
            // ── 工作区 ──
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_FOLDER, "工作区");
                ui.separator();
                self.render_config_locations(ui);

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("工作区布局");
                    let confirm_id = ui.id().with("reset_workspace_layout_confirm");
                    let now = ui.input(|input| input.time);
                    let armed_at: Option<f64> =
                        ui.ctx().memory(|memory| memory.data.get_temp(confirm_id));
                    let armed = armed_at.is_some_and(|time| now - time < 3.0);
                    let label = if armed {
                        "确认恢复默认布局?"
                    } else {
                        "恢复默认布局"
                    };
                    if ui
                        .button(egui::RichText::new(label).color(if armed {
                            theme::orange()
                        } else {
                            theme::text_primary()
                        }))
                        .on_hover_text("仅重置面板位置，不修改主题、串口、快捷键和插件状态")
                        .clicked()
                    {
                        if armed {
                            self.panels.reset_tiles_layout();
                            ui.ctx()
                                .memory_mut(|memory| memory.data.remove_temp::<f64>(confirm_id));
                            match self.save_config() {
                                Ok(()) => self.set_status_force(
                                    StatusLevel::Info,
                                    "已恢复默认布局，运行中的插件面板已保留",
                                ),
                                Err(error) => self.set_status_force(StatusLevel::Error, error),
                            }
                        } else {
                            ui.ctx().memory_mut(|memory| {
                                memory.data.insert_temp(confirm_id, now);
                            });
                        }
                    }
                    if armed && ui.small_button("取消").clicked() {
                        ui.ctx()
                            .memory_mut(|memory| memory.data.remove_temp::<f64>(confirm_id));
                    }
                });

                // 最近工作区
                if !self.recent_workspaces.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("最近工作区").color(theme::text_secondary()));
                    let paths: Vec<(usize, std::path::PathBuf)> = self
                        .recent_workspaces
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (i, std::path::PathBuf::from(p)))
                        .collect();
                    let mut to_remove: Option<usize> = None;
                    for (i, path) in &paths {
                        let path_str = path.display().to_string();
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button("打开").clicked() {
                                match self.load_config_from_path(path) {
                                    Ok(()) => {
                                        self.apply_loaded_workspace_postprocess();
                                        apply_theme(ui.ctx(), self.ui_theme);
                                        self.set_status_force(
                                            StatusLevel::Info,
                                            format!("已加载: {path_str}"),
                                        );
                                    }
                                    Err(e) => self.set_status_force(StatusLevel::Error, e),
                                }
                            }
                            let display = tool_panels::compact_middle(&path_str, 60);
                            ui.label(&display).on_hover_text(&path_str);
                            if ui.small_button("× 移除").clicked() {
                                to_remove = Some(*i);
                            }
                        });
                    }
                    if let Some(i) = to_remove {
                        self.recent_workspaces.remove(i);
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                }
            });

            ui.add_space(8.0);

            // ── 外观 ──
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_PALETTE, "外观");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("界面主题");
                    let selected_label =
                        theme::current_theme_name().unwrap_or_else(|| "选择主题…".to_owned());
                    egui::ComboBox::from_id_salt("app-theme")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for (path, name) in theme::discover_theme_files(&self.theme_dir) {
                                if ui
                                    .selectable_label(self.theme_path.as_ref() == Some(&path), name)
                                    .clicked()
                                {
                                    match theme::load_theme_file(&path) {
                                        Ok(_) => {
                                            self.ui_theme = theme::builtin_theme_for_path(&path)
                                                .unwrap_or(theme::AppTheme::Custom);
                                            self.theme_path = Some(path);
                                            apply_theme(ui.ctx(), self.ui_theme);
                                            if let Err(error) = self.save_config() {
                                                log::warn!("save_config failed: {error}");
                                            }
                                        }
                                        Err(error) => self.set_status_force(
                                            StatusLevel::Error,
                                            format!("加载主题失败：{error}"),
                                        ),
                                    }
                                }
                            }
                        });
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("打开主题目录")
                        .on_hover_text("将 JSON 主题文件放入此目录")
                        .clicked()
                        && let Err(error) = open::that(&self.theme_dir)
                    {
                        self.set_status_force(
                            StatusLevel::Error,
                            format!("打开主题目录失败：{error}"),
                        );
                    }
                });
                ui.add_space(4.0);
                let mut bottom_visible = self.panels.bottom_visible();
                if ui.checkbox(&mut bottom_visible, "显示底部面板").changed() {
                    self.set_bottom_visible(bottom_visible);
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("等宽字体大小");
                    let mut size = self.monospace_font_size;
                    let resp = ui.add(
                        egui::Slider::new(&mut size, 10.0..=24.0)
                            .step_by(1.0)
                            .suffix("px"),
                    );
                    if resp.changed() {
                        self.monospace_font_size = size;
                        self.terminal_panel.font_size = size;
                        self.bottom_log_panel.font_size = size;
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                });
            });
        }

        ui.add_space(8.0);

        if category == 1 {
            // ── 网络 ──
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_NETWORK_CHECK, "网络");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label("代理地址");
                    let proxy_width = (ui.available_width() - 8.0).clamp(140.0, 260.0);
                    let response = ui
                        .add(
                            egui::TextEdit::singleline(&mut self.network_proxy_url)
                                .desired_width(proxy_width)
                                .hint_text("留空：系统/环境代理或直连"),
                        )
                        .on_hover_text("支持 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080");
                    if response.changed()
                        && let Err(error) = self.save_config()
                    {
                        log::warn!("save_config failed: {error}");
                    }
                });
            });

            ui.add_space(8.0);

            // ── 数据 ──
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_TUNE, "数据");
                ui.separator();
                let (changed, terminal_max_entries, log_max_entries) = {
                    let mut view = DataSettingsView {
                        merge_window_ms: &mut self.terminal_panel.merge_window_ms,
                        terminal_max_entries: &mut self.terminal_panel.max_entries,
                        log_max_entries: &mut self.bottom_log_panel.max_entries,
                    };
                    let changed = data_settings_ui(ui, &mut view);
                    (changed, *view.terminal_max_entries, *view.log_max_entries)
                };
                if changed {
                    self.terminal_panel.set_max_entries(terminal_max_entries);
                    self.bottom_log_panel.set_max_entries(log_max_entries);
                    if let Err(error) = self.save_config() {
                        log::warn!("save_config failed: {error}");
                    }
                }
            });
        }

        ui.add_space(8.0);

        if category == 2 {
            // ── 快捷键 ──
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_KEYBOARD, "快捷键");
                ui.separator();
                self.render_keymap_editor(ui);
            });
        }

        ui.add_space(8.0);

        if category == 3 {
            // ── 插件设置 ──
            self.render_plugin_settings(ui);
        }

        ui.add_space(8.0);

        if category == 4 {
            // ── 恢复默认 ──
            ui.horizontal_wrapped(|ui| {
                if design::button(ui, ICON_RESTART_ALT, "恢复所有默认设置", ButtonKind::Danger)
                    .clicked()
                {
                    self.serial.selected_port = None;
                    self.serial.baud_rate = "115200".to_owned();
                    self.serial.data_bits = "8".to_owned();
                    self.serial.stop_bits = "1".to_owned();
                    self.serial.parity = "none".to_owned();
                    self.recorder_path = default_recorder_path();
                    self.serial.port_aliases.clear();
                    self.serial.port_groups.clear();
                    self.panels.reset_tiles_layout();
                    self.terminal_panel.merge_window_ms = 5;
                    self.terminal_panel.set_max_entries(50000);
                    self.bottom_log_panel.set_max_entries(50000);
                    self.monospace_font_size = 13.0;
                    self.ui_theme = theme::AppTheme::default();
                    self.theme_path = theme::builtin_theme_path(self.ui_theme, &self.theme_dir);
                    if let Err(error) = theme::load_builtin_theme(self.ui_theme, &self.theme_dir) {
                        log::warn!("load default theme failed: {error}");
                    }
                    self.terminal_panel.font_size = 13.0;
                    self.bottom_log_panel.font_size = 13.0;
                    apply_theme(ui.ctx(), self.ui_theme);
                    self.set_status_force(StatusLevel::Warn, "已恢复默认设置");
                }
            });

            ui.add_space(8.0);

            // ── 关于 ──
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::section_header(ui, ICON_INFO, "关于");
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("硬件调试工作台 v{}", env!("CARGO_PKG_VERSION")));
                    ui.hyperlink_to(REPOSITORY_URL, REPOSITORY_URL)
                        .on_hover_text("打开项目仓库");
                    if ui.small_button("复制版本号").clicked() {
                        copy_text_with_feedback(
                            ui,
                            format!("v{}", env!("CARGO_PKG_VERSION")),
                            format!("已复制 v{}", env!("CARGO_PKG_VERSION")),
                        );
                    }
                });
            });
        }
    }

    fn render_config_locations(&mut self, ui: &mut egui::Ui) {
        let workspace_config = config_path();
        let plugin_config_dir = self.workbench.plugin_config_root();

        self.render_config_location_row(
            ui,
            "工作区配置",
            &workspace_config.display().to_string(),
            false,
        );
        self.render_config_location_row(ui, "插件配置", &plugin_config_dir, true);
    }

    fn render_config_location_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        path: &str,
        open_self: bool,
    ) {
        let path_text = path.to_owned();
        ui.horizontal(|ui| {
            // 固定标签列并关闭自动换行，保证两条路径的起点和右侧操作列对齐。
            ui.add_sized(
                egui::vec2(CONFIG_LOCATION_LABEL_WIDTH, ui.spacing().interact_size.y),
                egui::Label::new(label).halign(egui::Align::LEFT),
            );

            // 等宽路径
            let item_spacing = ui.spacing().item_spacing.x;
            let action_width = design::ICON_BUTTON_SIZE * 2.0 + item_spacing * 2.0;
            let available_width = (ui.available_width() - action_width).max(64.0);
            let display_path = tool_panels::compact_middle(
                &path_text,
                ((available_width / 8.0) as usize).clamp(20, 80),
            );
            ui.add_sized(
                egui::vec2(available_width, ui.spacing().interact_size.y),
                egui::Label::new(
                    egui::RichText::new(display_path)
                        .monospace()
                        .color(theme::text_primary()),
                )
                .truncate()
                .halign(egui::Align::LEFT),
            )
            .on_hover_text(&path_text);
            if design::icon_button(ui, ICON_CONTENT_COPY, "复制路径").clicked() {
                copy_text_with_feedback(ui, path_text.clone(), format!("已复制{label}路径"));
            }
            if design::icon_button(ui, ICON_FOLDER_OPEN, "打开所在目录").clicked() {
                match open_config_location(Path::new(&path_text), open_self) {
                    Ok(target) => self.set_status_force(
                        StatusLevel::Info,
                        format!("已打开: {}", target.display()),
                    ),
                    Err(error) => self.set_status_force(StatusLevel::Error, error),
                }
            }
        });
    }

    /// 快捷键编辑器：表格展示所有命令及其绑定，支持录制新快捷键。
    fn render_keymap_editor(&mut self, ui: &mut egui::Ui) {
        use crate::keymap::Keymap;

        // 所有可配置的命令：内置 + 插件命令（统一 CommandRegistry）。
        // 命令元数据由 tick_plugin_lifecycle 随插件启停同步，此处直接读取。
        let all_commands: Vec<crate::command_registry::Command> = self.commands.all().to_vec();
        let entries = all_commands
            .iter()
            .map(|command| KeymapEntry {
                id: command.id.clone(),
                title: command.title.clone(),
                bindings: self
                    .keymap
                    .get_bindings(&command.id)
                    .iter()
                    .map(|binding| binding.display())
                    .collect::<Vec<_>>()
                    .join(", "),
                recording: self.key_recording.as_ref() == Some(&command.id),
            })
            .collect::<Vec<_>>();

        for action in keymap_ui(ui, &entries) {
            match action {
                KeymapAction::Record(command_id) => self.key_recording = Some(command_id),
                KeymapAction::Clear(command_id) => {
                    self.keymap.set_bindings(&command_id, vec![]);
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    }
                }
                KeymapAction::RestoreDefaults => {
                    self.keymap = Keymap::default();
                    self.key_recording = None;
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
                    self.set_status_force(StatusLevel::Warn, "快捷键已恢复默认");
                }
            }
        }
    }

    /// 渲染所有已发现插件的设置表单
    fn render_plugin_settings(&mut self, ui: &mut egui::Ui) {
        let plugin_settings = self.workbench.plugin_settings();
        if plugin_settings.is_empty() {
            design::card().show(ui, |ui| {
                design::empty_state(ui, ICON_APPS, "暂无插件设置");
            });
            return;
        }
        let ports = self
            .serial
            .ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect::<Vec<_>>();
        for (plugin_id, plugin_name, settings) in &plugin_settings {
            // Application 提供当前值；共享面板只负责 manifest 表单展示。
            let keys = self.workbench.plugin_setting_keys(plugin_id);
            let mut values = std::collections::BTreeMap::new();
            for setting in settings {
                let current_value = self.workbench.plugin_setting_value(
                    plugin_id,
                    &setting.id,
                    setting.default.clone(),
                );
                // 首次写入默认值
                if !keys.contains(&setting.id) {
                    let _ = self.workbench.set_plugin_setting(
                        plugin_id,
                        &setting.id,
                        current_value.clone(),
                    );
                }
                values.insert(setting.id.clone(), current_value);
            }
            let before = values.clone();
            let mut view = PluginSettingsView {
                plugin_id,
                plugin_name,
                settings,
                ports: &ports,
                values: &mut values,
            };
            plugin_settings_ui(ui, &mut view);

            if values != before {
                let event_sink = self.workbench.event_sink();
                let payload = serde_json::json!({
                    "panel_id": format!("{plugin_id}.settings"),
                    "values": values,
                });
                event_sink.publish_event(tool_core::Event::new(
                    tool_core::topics::UI_FORM_CHANGED,
                    format!("ui.panel:{plugin_id}.settings"),
                    tool_core::Direction::Internal,
                    tool_core::Payload::Json(payload),
                ));
            }
        }
    }
}

fn open_config_location(path: &Path, open_self: bool) -> Result<PathBuf, String> {
    let target = if open_self {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    std::fs::create_dir_all(&target).map_err(|e| format!("创建目录失败：{e}"))?;
    open::that(&target)
        .map_err(|e| format!("打开目录失败：{e}"))
        .map(|()| target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_navigation_does_not_move_when_longest_item_is_selected() {
        let mut first_row = Vec::new();
        let mut second_row = Vec::new();

        egui::__run_test_ui(|ui| {
            ui.set_max_width(900.0);
            ui.horizontal_wrapped(|ui| {
                for (index, icon, label) in SETTINGS_NAV_ITEMS {
                    first_row.push(settings_nav_button(ui, index == 0, icon, label).rect);
                }
            });
            ui.horizontal_wrapped(|ui| {
                for (index, icon, label) in SETTINGS_NAV_ITEMS {
                    second_row.push(settings_nav_button(ui, index == 1, icon, label).rect);
                }
            });
        });

        assert_eq!(first_row.len(), second_row.len());
        for (before, after) in first_row.iter().zip(&second_row) {
            assert!((before.left() - after.left()).abs() < 0.1);
            assert!(
                (before.width() - crate::shared_settings::SETTINGS_NAV_BUTTON_SIZE.x).abs() < 0.1
            );
            assert!(
                (after.width() - crate::shared_settings::SETTINGS_NAV_BUTTON_SIZE.x).abs() < 0.1
            );
            assert!(
                (before.height() - crate::shared_settings::SETTINGS_NAV_BUTTON_SIZE.y).abs() < 0.1
            );
            assert!(
                (after.height() - crate::shared_settings::SETTINGS_NAV_BUTTON_SIZE.y).abs() < 0.1
            );
        }
    }
}
