use crate::app::WorkbenchApp;
use crate::config::default_activity_order;
use crate::config::{
    config_path, default_recorder_path, pick_workspace_open_path, pick_workspace_save_path,
};
use crate::state::StatusLevel;
use eframe::egui;
use std::path::{Path, PathBuf};
use tool_panels::theme;

impl WorkbenchApp {
    pub(crate) fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置");
        ui.separator();

        ui.heading("工作区");
        ui.horizontal(|ui| {
            if ui.button("保存").clicked() {
                match self.save_config() {
                    Ok(()) => {
                        self.set_status_force(StatusLevel::Info, "工作区已保存");
                    }
                    Err(e) => {
                        self.set_status_force(StatusLevel::Error, format!("保存失败：{e}"));
                    }
                }
            }

            if ui.button("另存为...").clicked()
                && let Some(path) = pick_workspace_save_path()
            {
                match self.save_config_to_path(&path) {
                    Ok(()) => {
                        self.add_recent_workspace(&path);
                        self.set_status_force(
                            StatusLevel::Info,
                            format!("工作区已保存: {}", path.display()),
                        );
                    }
                    Err(e) => {
                        self.set_status_force(StatusLevel::Error, format!("保存失败：{e}"));
                    }
                }
            }

            if ui.button("打开...").clicked()
                && let Some(path) = pick_workspace_open_path()
            {
                match self.load_config_from_path(&path) {
                    Ok(()) => {
                        self.set_status_force(
                            StatusLevel::Info,
                            format!("工作区已加载: {}", path.display()),
                        );
                    }
                    Err(e) => {
                        self.set_status_force(StatusLevel::Error, format!("加载失败：{e}"));
                    }
                }
            }
        });

        self.render_config_locations(ui);

        ui.horizontal(|ui| {
            if ui.button("恢复默认").clicked() {
                self.serial.selected_port = None;
                self.serial.baud_rate = "115200".to_owned();
                self.serial.data_bits = "8".to_owned();
                self.serial.stop_bits = "1".to_owned();
                self.serial.parity = "none".to_owned();
                self.recorder_path = default_recorder_path();
                self.activity_order = default_activity_order();
                self.serial.port_aliases.clear();
                self.panels.dock = tool_panels::DockLayout::default();
                self.bottom_panel_visible = true;
                self.set_status_force(StatusLevel::Warn, "已恢复默认设置，请保存后生效");
            }

            if ui.button("保存并应用").clicked() {
                match self.save_config() {
                    Ok(()) => {
                        self.set_status_force(StatusLevel::Info, "工作区已保存");
                    }
                    Err(e) => {
                        self.set_status_force(StatusLevel::Error, format!("保存失败：{e}"));
                    }
                }
            }
        });

        if !self.recent_workspaces.is_empty() {
            ui.separator();
            ui.label("最近工作区：");
            let paths: Vec<(usize, std::path::PathBuf)> = self
                .recent_workspaces
                .iter()
                .enumerate()
                .map(|(i, p)| (i, std::path::PathBuf::from(p)))
                .collect();
            let mut to_remove: Option<usize> = None;
            for (i, path) in paths {
                let path_str = path.display().to_string();
                ui.horizontal(|ui| {
                    if ui.button("打开").clicked() {
                        match self.load_config_from_path(&path) {
                            Ok(()) => {
                                self.apply_loaded_workspace_postprocess();
                                self.set_status_force(
                                    StatusLevel::Info,
                                    format!("已加载: {path_str}"),
                                );
                            }
                            Err(e) => self.set_status_force(StatusLevel::Error, e),
                        }
                    }
                    ui.label(&path_str);
                    if ui.small_button("×").clicked() {
                        to_remove = Some(i);
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

        ui.separator();
        ui.heading("外观");
        let mut bottom_visible = self.panels.dock.bottom_visible;
        if ui.checkbox(&mut bottom_visible, "底部面板").changed() {
            self.set_bottom_visible(bottom_visible);
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
        }
        ui.separator();
        ui.heading("快捷键");
        self.render_keymap_editor(ui);
        ui.separator();
        ui.label("硬件调试工作台 v0.1.0");
    }

    fn render_config_locations(&mut self, ui: &mut egui::Ui) {
        let workspace_config = config_path();
        let plugin_config_dir = self.plugin_manager.config_root().to_path_buf();

        ui.add_space(6.0);
        ui.label(egui::RichText::new("配置文件位置").strong());
        self.render_config_location_row(ui, "工作区配置", &workspace_config, false);
        self.render_config_location_row(ui, "插件配置目录", &plugin_config_dir, true);
    }

    fn render_config_location_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        path: &Path,
        open_self: bool,
    ) {
        let path_text = path.display().to_string();
        ui.horizontal(|ui| {
            ui.add_sized([92.0, 0.0], egui::Label::new(label));
            if ui.small_button("复制").clicked() {
                ui.ctx().copy_text(path_text.clone());
                self.set_status_force(StatusLevel::Info, format!("已复制: {path_text}"));
            }
            if ui.small_button("打开目录").clicked() {
                match open_config_location(path, open_self) {
                    Ok(target) => self.set_status_force(
                        StatusLevel::Info,
                        format!("已打开: {}", target.display()),
                    ),
                    Err(error) => self.set_status_force(StatusLevel::Error, error),
                }
            }
            ui.monospace(&path_text).on_hover_text(path_text);
        });
    }

    /// 快捷键编辑器：表格展示所有动作及其绑定，支持录制新快捷键。
    fn render_keymap_editor(&mut self, ui: &mut egui::Ui) {
        use crate::keymap::{Action, Keymap};

        // 收集所有可配置的动作：内置 + 插件命令
        let plugin_summaries: Vec<tool_extension::PluginSummary> = self
            .plugin_manager
            .summaries()
            .into_iter()
            .filter(|s| {
                matches!(
                    s.state,
                    tool_extension::PluginState::Enabled | tool_extension::PluginState::Running
                )
            })
            .collect();
        let all_actions = Action::all_with_plugins(&plugin_summaries);

        egui::Grid::new("keymap_grid")
            .num_columns(4)
            .striped(true)
            .min_col_width(80.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("操作").strong());
                ui.label(egui::RichText::new("快捷键").strong());
                ui.label(egui::RichText::new("").strong());
                ui.label(egui::RichText::new("").strong());
                ui.end_row();

                for action in &all_actions {
                    let bindings = self.keymap.get_bindings(action);
                    let action_label = action.label_with_plugins(&plugin_summaries);
                    let is_recording = self.key_recording.as_ref() == Some(action);

                    ui.label(&action_label);

                    // 快捷键显示
                    if bindings.is_empty() {
                        ui.colored_label(theme::TEXT_DIMMED, "未绑定");
                    } else {
                        let shortcuts: Vec<String> = bindings.iter().map(|b| b.display()).collect();
                        ui.label(shortcuts.join(", "));
                    }

                    // 录制按钮 / 录制中状态
                    if is_recording {
                        ui.colored_label(theme::YELLOW, "按下按键...");
                    } else if ui.button("录制").clicked() {
                        self.key_recording = Some(action.clone());
                    }

                    // 清除按钮
                    if !bindings.is_empty() && ui.button("清除").clicked() {
                        self.keymap.set_bindings(action, vec![]);
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                    ui.end_row();
                }
            });

        ui.separator();
        if ui.button("恢复默认快捷键").clicked() {
            self.keymap = Keymap::default();
            self.key_recording = None;
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
            self.set_status_force(StatusLevel::Warn, "快捷键已恢复默认");
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
