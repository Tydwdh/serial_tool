use crate::app::WorkbenchApp;
use crate::config::default_activity_order;
use crate::config::{default_recorder_path, pick_workspace_open_path, pick_workspace_save_path};
use crate::state::StatusLevel;
use eframe::egui;
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

        ui.horizontal(|ui| {
            if ui.button("恢复默认").clicked() {
                self.serial.selected_port = None;
                self.serial.baud_rate = "115200".to_owned();
                self.serial.data_bits = "8".to_owned();
                self.serial.stop_bits = "1".to_owned();
                self.serial.parity = "none".to_owned();
                self.serial.timeout_ms = "50".to_owned();
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

    /// 快捷键编辑器：表格展示所有动作及其绑定，支持录制新快捷键。
    fn render_keymap_editor(&mut self, ui: &mut egui::Ui) {
        use crate::keymap::{Action, KeyBinding, Keymap};

        // 录制状态
        let mut recording_action: Option<Action> = None;
        let mut recording_index: Option<usize> = None;

        // 检查是否有按键事件用于录制
        let pressed_key = self.capture_key_for_recording(ui.ctx());

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

                for action in Action::ALL {
                    let bindings = self
                        .keymap
                        .bindings
                        .get(action)
                        .cloned()
                        .unwrap_or_default();
                    let action_label = action.label();

                    ui.label(action_label);

                    // 快捷键显示
                    if bindings.is_empty() {
                        ui.colored_label(theme::TEXT_DIMMED, "未绑定");
                    } else {
                        let shortcuts: Vec<String> = bindings.iter().map(|b| b.display()).collect();
                        ui.label(shortcuts.join(", "));
                    }

                    // 录制按钮
                    if ui.button("录制").clicked() {
                        recording_action = Some(*action);
                        recording_index = Some(bindings.len());
                    }

                    // 清除按钮
                    if !bindings.is_empty() && ui.button("清除").clicked() {
                        self.keymap.set_bindings(*action, vec![]);
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                    ui.end_row();
                }
            });

        // 处理录制结果
        if let (Some(action), Some(key_name)) = (recording_action, pressed_key) {
            let mut bindings = self
                .keymap
                .bindings
                .get(&action)
                .cloned()
                .unwrap_or_default();
            // 录制新快捷键：替换同修饰键的旧绑定，或追加
            let ctrl = ui.ctx().input(|i| i.modifiers.ctrl);
            let shift = ui.ctx().input(|i| i.modifiers.shift);
            let alt = ui.ctx().input(|i| i.modifiers.alt);
            let new_binding = KeyBinding::new(&key_name, ctrl, shift, alt);
            // 移除同修饰键组合的旧绑定
            bindings.retain(|b| !(b.ctrl == ctrl && b.shift == shift && b.alt == alt));
            bindings.push(new_binding);
            self.keymap.set_bindings(action, bindings);
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
            self.set_status_force(
                StatusLevel::Info,
                format!("{} 快捷键已更新", action.label()),
            );
        }

        ui.separator();
        if ui.button("恢复默认快捷键").clicked() {
            self.keymap = Keymap::default();
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
            self.set_status_force(StatusLevel::Warn, "快捷键已恢复默认");
        }
    }

    /// 捕获按键事件用于快捷键录制。返回按下的键名。
    fn capture_key_for_recording(&self, ctx: &egui::Context) -> Option<String> {
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers: _,
                    ..
                } = event
                {
                    return Some(format!("{key:?}"));
                }
            }
            None
        })
    }
}
