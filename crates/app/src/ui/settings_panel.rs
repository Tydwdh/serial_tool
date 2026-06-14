use crate::app::WorkbenchApp;
use crate::config::default_activity_order;
use crate::config::{
    default_recorder_path, pick_workspace_open_path, pick_workspace_save_path,
};
use crate::state::StatusLevel;
use eframe::egui;

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

            if ui.button("另存为...").clicked() {
                if let Some(path) = pick_workspace_save_path() {
                    match self.save_config_to_path(&path) {
                        Ok(()) => {
                            self.set_status_force(
                                StatusLevel::Info,
                                format!("工作区已保存: {}", path.display()),
                            );
                        }
                        Err(e) => {
                            self.set_status_force(
                                StatusLevel::Error,
                                format!("保存失败：{e}"),
                            );
                        }
                    }
                }
            }

            if ui.button("打开...").clicked() {
                if let Some(path) = pick_workspace_open_path() {
                    match self.load_config_from_path(&path) {
                        Ok(()) => {
                            self.set_status_force(
                                StatusLevel::Info,
                                format!("工作区已加载: {}", path.display()),
                            );
                        }
                        Err(e) => {
                            self.set_status_force(
                                StatusLevel::Error,
                                format!("加载失败：{e}"),
                            );
                        }
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            if ui.button("恢复默认").clicked() {
                self.selected_port = None;
                self.baud_rate = "115200".to_owned();
                self.data_bits = "8".to_owned();
                self.stop_bits = "1".to_owned();
                self.parity = "none".to_owned();
                self.timeout_ms = "50".to_owned();
                self.recorder_path = default_recorder_path();
                self.activity_order = default_activity_order();
                self.port_aliases.clear();
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

        ui.separator();
        ui.heading("外观");
        let mut bottom_visible = self.panels.dock.bottom_visible;
        if ui.checkbox(&mut bottom_visible, "底部面板").changed() {
            self.set_bottom_visible(bottom_visible);
            let _ = self.save_config();
        }
        ui.separator();
        ui.heading("快捷键");
        ui.label("Ctrl+R 刷新  Ctrl+Shift+O 打开  Ctrl+B 底部  Ctrl+1~4 切换");
        ui.separator();
        ui.label("硬件调试工作台 v0.1.0");
    }
}
