use crate::app::WorkbenchApp;
use crate::keymap::{Action, KeyBinding};
use crate::state::StatusLevel;
use eframe::egui;
use tool_panels::PanelKind;

impl WorkbenchApp {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        // 快捷键录制期间跳过全局快捷键，避免误触发其他动作导致离开设置页
        if self.key_recording.is_some() {
            return;
        }
        let keymap = self.keymap.clone();
        let mut triggered: Option<Action> = None;

        ctx.input(|i| {
            for (key, bindings) in &keymap.bindings {
                for binding in bindings {
                    if let Some(egui_key) = parse_egui_key(&binding.key) {
                        let mods_match = i.modifiers.ctrl == binding.ctrl
                            && i.modifiers.shift == binding.shift
                            && i.modifiers.alt == binding.alt;
                        if mods_match && i.key_pressed(egui_key) {
                            triggered = Action::from_key(key);
                        }
                    }
                }
            }
        });

        if let Some(action) = triggered {
            self.pending_action = Some(action);
        }
    }

    /// 执行当前帧触发的快捷键动作（在 tick_pre_ui 中调用）。
    pub(super) fn flush_pending_action(&mut self) {
        if let Some(action) = self.pending_action.take() {
            self.execute_action(action);
        }
    }

    /// 执行快捷键对应的操作。
    fn execute_action(&mut self, action: Action) {
        match action {
            Action::RefreshPorts => self.refresh_ports(),
            Action::OpenPort => self.toggle_selected_port(),
            Action::ToggleBottomPanel => self.toggle_bottom_panel(),
            Action::ToggleRightDock => {
                let visible = self.panels.right_visible();
                self.panels.set_right_visible(!visible);
                if let Err(e) = self.save_config() {
                    log::warn!("save_config failed: {e}")
                };
            }
            Action::Send => {
                if self.send_target_port_open() && !self.send.input.trim().is_empty() {
                    self.do_send();
                }
            }
            Action::StartRecording => self.start_or_stop_recording(),
            Action::ReconnectPort => self.reconnect_selected_port(),
            Action::AddBookmark => {
                if self.recorder.is_running() {
                    self.recorder.add_bookmark("");
                }
            }
            Action::CommandPalette => {
                self.command_palette.open = !self.command_palette.open;
                self.command_palette.query.clear();
                self.command_palette.selected = None;
            }
            Action::ClearTerminal => {
                self.terminal_panel.clear();
            }
            Action::ToggleTerminalPause => {
                self.terminal_panel.paused = !self.terminal_panel.paused;
            }
            Action::PluginCommand(plugin_id, command_id) => {
                self.publish_plugin_command_action(&plugin_id, &command_id);
            }
        }
    }

    /// 快捷键录制：按 Escape 取消，离开设置页时取消。检测到按键则保存绑定。
    pub(super) fn tick_key_recording(&mut self, ctx: &egui::Context) {
        if self.key_recording.is_none() {
            return;
        }
        // 离开设置页时取消录制。这里看 dock 的实际可见面板，而不是可能滞后的 active_tab。
        if !self.panels.is_panel_visible(&PanelKind::Settings) {
            self.key_recording = None;
            return;
        }
        // Escape 只取消录制，不能被保存成快捷键。
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.key_recording = None;
            return;
        }
        // 检查是否有按键事件
        if let Some((key_name, modifiers)) = Self::capture_key_for_recording(ctx) {
            // SAFETY: tick_key_recording starts by checking key_recording.is_none() and returns
            // early, and there's no async yield point between that guard and here.
            let action = self
                .key_recording
                .take()
                .expect("key_recording was checked non-None above");
            let new_binding =
                KeyBinding::new(&key_name, modifiers.ctrl, modifiers.shift, modifiers.alt);

            self.keymap.remove_binding_everywhere(&new_binding);
            let mut bindings = self.keymap.get_bindings(&action);
            bindings.retain(|b| {
                !(b.ctrl == new_binding.ctrl
                    && b.shift == new_binding.shift
                    && b.alt == new_binding.alt)
            });
            bindings.push(new_binding);
            self.keymap.set_bindings(&action, bindings);
            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };
            self.set_status_force(
                StatusLevel::Info,
                format!(
                    "{} 快捷键已更新",
                    action.label_with_plugins(self.plugin_summaries())
                ),
            );
        }
    }

    /// 捕获按键事件用于快捷键录制。返回按下的键名。
    fn capture_key_for_recording(ctx: &egui::Context) -> Option<(String, egui::Modifiers)> {
        ctx.input(|i| {
            for event in &i.events {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                {
                    // 过滤修饰键本身：Ctrl/Shift/Alt/Meta 键不应被录制为主体按键
                    if Self::is_modifier_key(*key) {
                        continue;
                    }
                    return Some((format!("{key:?}"), *modifiers));
                }
            }
            None
        })
    }

    /// 判断按键是否为修饰键（Ctrl/Shift/Alt 本身），这些键不应被录制为快捷键的主体。
    fn is_modifier_key(key: egui::Key) -> bool {
        matches!(
            key,
            egui::Key::ControlLeft
                | egui::Key::ControlRight
                | egui::Key::ShiftLeft
                | egui::Key::ShiftRight
                | egui::Key::AltLeft
                | egui::Key::AltRight
        )
    }
}

/// 将 keymap 中的键名字符串转换为 egui::Key。
fn parse_egui_key(name: &str) -> Option<egui::Key> {
    match name {
        "A" => Some(egui::Key::A),
        "B" => Some(egui::Key::B),
        "C" => Some(egui::Key::C),
        "D" => Some(egui::Key::D),
        "E" => Some(egui::Key::E),
        "F" => Some(egui::Key::F),
        "G" => Some(egui::Key::G),
        "H" => Some(egui::Key::H),
        "I" => Some(egui::Key::I),
        "J" => Some(egui::Key::J),
        "K" => Some(egui::Key::K),
        "L" => Some(egui::Key::L),
        "M" => Some(egui::Key::M),
        "N" => Some(egui::Key::N),
        "O" => Some(egui::Key::O),
        "P" => Some(egui::Key::P),
        "Q" => Some(egui::Key::Q),
        "R" => Some(egui::Key::R),
        "S" => Some(egui::Key::S),
        "T" => Some(egui::Key::T),
        "U" => Some(egui::Key::U),
        "V" => Some(egui::Key::V),
        "W" => Some(egui::Key::W),
        "X" => Some(egui::Key::X),
        "Y" => Some(egui::Key::Y),
        "Z" => Some(egui::Key::Z),
        "Num0" => Some(egui::Key::Num0),
        "Num1" => Some(egui::Key::Num1),
        "Num2" => Some(egui::Key::Num2),
        "Num3" => Some(egui::Key::Num3),
        "Num4" => Some(egui::Key::Num4),
        "Num5" => Some(egui::Key::Num5),
        "Num6" => Some(egui::Key::Num6),
        "Num7" => Some(egui::Key::Num7),
        "Num8" => Some(egui::Key::Num8),
        "Num9" => Some(egui::Key::Num9),
        "Escape" => Some(egui::Key::Escape),
        "Enter" => Some(egui::Key::Enter),
        "Tab" => Some(egui::Key::Tab),
        "Space" => Some(egui::Key::Space),
        "Backspace" => Some(egui::Key::Backspace),
        "Delete" => Some(egui::Key::Delete),
        "Insert" => Some(egui::Key::Insert),
        "Home" => Some(egui::Key::Home),
        "End" => Some(egui::Key::End),
        "PageUp" => Some(egui::Key::PageUp),
        "PageDown" => Some(egui::Key::PageDown),
        "ArrowUp" => Some(egui::Key::ArrowUp),
        "ArrowDown" => Some(egui::Key::ArrowDown),
        "ArrowLeft" => Some(egui::Key::ArrowLeft),
        "ArrowRight" => Some(egui::Key::ArrowRight),
        "F1" => Some(egui::Key::F1),
        "F2" => Some(egui::Key::F2),
        "F3" => Some(egui::Key::F3),
        "F4" => Some(egui::Key::F4),
        "F5" => Some(egui::Key::F5),
        "F6" => Some(egui::Key::F6),
        "F7" => Some(egui::Key::F7),
        "F8" => Some(egui::Key::F8),
        "F9" => Some(egui::Key::F9),
        "F10" => Some(egui::Key::F10),
        "F11" => Some(egui::Key::F11),
        "F12" => Some(egui::Key::F12),
        "Backtick" => Some(egui::Key::Backtick),
        "Minus" => Some(egui::Key::Minus),
        "Equals" => Some(egui::Key::Equals),
        "Comma" => Some(egui::Key::Comma),
        "Period" => Some(egui::Key::Period),
        "Slash" => Some(egui::Key::Slash),
        "Backslash" => Some(egui::Key::Backslash),
        "Semicolon" => Some(egui::Key::Semicolon),
        "Quote" => Some(egui::Key::Quote),
        "OpenBracket" => Some(egui::Key::OpenBracket),
        "CloseBracket" => Some(egui::Key::CloseBracket),
        _ => None,
    }
}
