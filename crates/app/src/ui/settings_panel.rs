use crate::app::WorkbenchApp;
use crate::config::{config_path, default_activity_order, default_recorder_path};
use crate::state::StatusLevel;
use eframe::egui;
use std::path::{Path, PathBuf};
use tool_panels::theme;
use tool_panels::{DynamicField, dynamic_form_ui, parse_fields};

impl WorkbenchApp {
    pub(super) fn settings_panel(&mut self, ui: &mut egui::Ui) {
        // ── 工作区 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_SETTINGS);
                    ui.label(egui::RichText::new("📂 工作区").heading());
                });
                ui.separator();
                self.render_config_locations(ui);

                // 最近工作区
                if !self.recent_workspaces.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("最近工作区").color(theme::TEXT_SECONDARY));
                    let paths: Vec<(usize, std::path::PathBuf)> = self
                        .recent_workspaces
                        .iter()
                        .enumerate()
                        .map(|(i, p)| (i, std::path::PathBuf::from(p)))
                        .collect();
                    let mut to_remove: Option<usize> = None;
                    for (i, path) in &paths {
                        let path_str = path.display().to_string();
                        ui.horizontal(|ui| {
                            if ui.small_button("打开").clicked() {
                                match self.load_config_from_path(path) {
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
                            let display = truncate_path(&path_str, 60);
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
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_SETTINGS);
                    ui.label(egui::RichText::new("🎨 外观").heading());
                });
                ui.separator();
                let mut bottom_visible = self.panels.dock.bottom_visible;
                if ui.checkbox(&mut bottom_visible, "显示底部面板").changed() {
                    self.set_bottom_visible(bottom_visible);
                    if let Err(e) = self.save_config() {
                        log::warn!("save_config failed: {e}")
                    };
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
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

        ui.add_space(8.0);

        // ── 数据 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_SETTINGS);
                    ui.label(egui::RichText::new("📊 数据").heading());
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("终端合并阈值");
                    let mut ms = self.terminal_panel.merge_window_ms;
                    let resp = ui.add(
                        egui::Slider::new(&mut ms, 0..=100)
                            .step_by(5.0)
                            .suffix("ms"),
                    );
                    if resp.changed() {
                        self.terminal_panel.merge_window_ms = ms;
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                })
                .response
                .on_hover_text(
                    "同一端口、同一方向、间隔 ≤ 此毫秒且不含换行符的连续数据包合并显示。\
                     慢设备调小避免误合并，高速流调大减少视觉碎片。",
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("终端保留条数");
                    let mut n = self.terminal_panel.max_entries;
                    let resp = ui.add(
                        egui::Slider::new(&mut n, 500..=50000)
                            .step_by(500.0),
                    );
                    if resp.changed() {
                        self.terminal_panel.max_entries = n;
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                })
                .response
                .on_hover_text("接收区保留的最近条数上限，超出后丢弃最旧条目。");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("日志保留条数");
                    let mut n = self.bottom_log_panel.max_entries;
                    let resp = ui.add(
                        egui::Slider::new(&mut n, 500..=50000)
                            .step_by(500.0),
                    );
                    if resp.changed() {
                        self.bottom_log_panel.max_entries = n;
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                })
                .response
                .on_hover_text("日志面板保留的最近条数上限。");
            });

        ui.add_space(8.0);

        // ── 快捷键 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_SETTINGS);
                    ui.label(egui::RichText::new("⌨ 快捷键").heading());
                });
                ui.separator();
                self.render_keymap_editor(ui);
            });

        ui.add_space(8.0);

        // ── 插件设置 ──
        self.render_plugin_settings(ui);

        ui.add_space(8.0);

        // ── 恢复默认 ──
        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("🔄 恢复所有默认设置").color(theme::ORANGE))
                .clicked()
            {
                self.serial.selected_port = None;
                self.serial.baud_rate = "115200".to_owned();
                self.serial.data_bits = "8".to_owned();
                self.serial.stop_bits = "1".to_owned();
                self.serial.parity = "none".to_owned();
                self.recorder_path = default_recorder_path();
                self.activity_order = default_activity_order();
                self.serial.port_aliases.clear();
                self.serial.port_groups.clear();
                self.panels.dock = tool_panels::DockLayout::default();
                self.panels.dock.bottom_visible = true;
                self.terminal_panel.merge_window_ms = 5;
                self.terminal_panel.max_entries = 2000;
                self.bottom_log_panel.max_entries = 2000;
                self.monospace_font_size = 13.0;
                self.terminal_panel.font_size = 13.0;
                self.bottom_log_panel.font_size = 13.0;
                self.set_status_force(StatusLevel::Warn, "已恢复默认设置，重启后生效");
            }
        });

        ui.add_space(8.0);

        // ── 关于 ──
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    theme::card_accent_bar(ui, theme::CARD_ACCENT_SETTINGS);
                    ui.label(egui::RichText::new("ℹ 关于").heading());
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!("硬件调试工作台 v{}", env!("CARGO_PKG_VERSION")));
                    if ui.small_button("复制版本号").clicked() {
                        ui.ctx()
                            .copy_text(format!("v{}", env!("CARGO_PKG_VERSION")));
                        self.set_status_force(
                            StatusLevel::Info,
                            format!("已复制 v{}", env!("CARGO_PKG_VERSION")),
                        );
                    }
                });
            });
    }

    fn render_config_locations(&mut self, ui: &mut egui::Ui) {
        let workspace_config = config_path();
        let plugin_config_dir = self.plugin_manager.config_root().to_path_buf();

        self.render_config_location_row(ui, "工作区配置", &workspace_config, false);
        self.render_config_location_row(ui, "插件配置", &plugin_config_dir, true);
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
            ui.label(label);
            ui.add_space(4.0);
            // 等宽路径
            ui.label(
                egui::RichText::new(&path_text)
                    .monospace()
                    .color(theme::TEXT_PRIMARY),
            )
            .on_hover_text(&path_text);
            if ui.small_button("📋").on_hover_text("复制路径").clicked() {
                ui.ctx().copy_text(path_text.clone());
                self.set_status_force(StatusLevel::Info, format!("已复制: {path_text}"));
            }
            if ui
                .small_button("📂")
                .on_hover_text("打开所在目录")
                .clicked()
            {
                match open_config_location(path, open_self) {
                    Ok(target) => self.set_status_force(
                        StatusLevel::Info,
                        format!("已打开: {}", target.display()),
                    ),
                    Err(error) => self.set_status_force(StatusLevel::Error, error),
                }
            }
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
                // 表头
                ui.label(
                    egui::RichText::new("操作")
                        .strong()
                        .color(theme::TEXT_SECONDARY),
                );
                ui.label(
                    egui::RichText::new("快捷键")
                        .strong()
                        .color(theme::TEXT_SECONDARY),
                );
                ui.label("");
                ui.label("");
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
                    } else if ui.small_button("录制").clicked() {
                        self.key_recording = Some(action.clone());
                    }

                    // 清除按钮
                    if !bindings.is_empty() && ui.small_button("清除").clicked() {
                        self.keymap.set_bindings(action, vec![]);
                        if let Err(e) = self.save_config() {
                            log::warn!("save_config failed: {e}")
                        };
                    }
                    ui.end_row();
                }
            });

        ui.add_space(4.0);
        if ui.button("恢复默认快捷键").clicked() {
            self.keymap = Keymap::default();
            self.key_recording = None;
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
            self.set_status_force(StatusLevel::Warn, "快捷键已恢复默认");
        }
    }

    /// 渲染所有已启用插件的设置表单
    fn render_plugin_settings(&mut self, ui: &mut egui::Ui) {
        let plugin_settings = self.plugin_manager.plugin_settings();
        if plugin_settings.is_empty() {
            return;
        }
        for (plugin_id, plugin_name, settings) in &plugin_settings {
            // 从 ConfigStore 读取当前值，构建 DynamicField 列表
            let config_store = self.plugin_manager.config_store();
            let mut fields: Vec<DynamicField>;
            let mut fields_json = Vec::with_capacity(settings.len());

            for setting in settings {
                let current_value =
                    config_store.get(plugin_id, &setting.id, setting.default.clone());
                // 首次写入默认值
                if current_value == setting.default {
                    let keys = config_store.keys(plugin_id);
                    if !keys.contains(&setting.id) {
                        let _ = config_store.set(plugin_id, &setting.id, setting.default.clone());
                    }
                }

                let mut field_json = serde_json::json!({
                    "id": setting.id,
                    "label": setting.title,
                    "kind": setting.kind,
                    "value": current_value,
                });
                let obj = field_json.as_object_mut().unwrap();
                if !setting.options.is_empty() {
                    obj.insert(
                        "options".to_owned(),
                        serde_json::Value::Array(setting.options.clone()),
                    );
                }
                if let Some(min) = setting.min {
                    obj.insert("min".to_owned(), serde_json::json!(min));
                }
                if let Some(max) = setting.max {
                    obj.insert("max".to_owned(), serde_json::json!(max));
                }
                if let Some(step) = setting.step {
                    obj.insert("step".to_owned(), serde_json::json!(step));
                }
                if let Some(rows) = setting.rows {
                    obj.insert("rows".to_owned(), serde_json::json!(rows));
                }
                fields_json.push(field_json);
            }

            if let Ok(parsed) = parse_fields(Some(&serde_json::Value::Array(fields_json))) {
                fields = parsed;
            } else {
                continue;
            }

            egui::Frame::group(ui.style())
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        theme::card_accent_bar(ui, theme::CARD_ACCENT_PLUGIN);
                        ui.label(egui::RichText::new(format!("🧩 {plugin_name} 设置")).heading());
                    });
                    ui.separator();

                    let panel_id = format!("{plugin_id}.settings");

                    // 手动渲染表单
                    dynamic_form_ui(
                        ui,
                        &self.bus,
                        &panel_id,
                        &mut fields,
                        true, // auto_apply
                        &self.serial.ports,
                    );
                });
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

/// 截断过长路径，保留首尾、中间用 ... 替代
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    let head = &path[..max_len / 2 - 2];
    let tail = &path[path.len() - max_len / 2 + 1..];
    format!("{head}...{tail}")
}
