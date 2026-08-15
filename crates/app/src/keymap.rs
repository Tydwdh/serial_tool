//! 可配置快捷键系统。
//!
//! 每个命令（以 [`crate::command_registry`] 中的命令 ID 标识）可以有多个
//! 快捷键绑定，存储在 [`Keymap`] 中。默认绑定参考 VSCode 风格，用户可在
//! 设置面板中自定义。
//!
//! 命令 ID 即持久化键：内置命令为 `$` 前缀（如 `$RefreshPorts`），插件命令
//! 为 `plugin_id:command_id`。该格式与既有 `workspace.json` 兼容。

use crate::command_registry::{
    CMD_CLEAR_TERMINAL, CMD_COMMAND_PALETTE, CMD_OPEN_PORT, CMD_REFRESH_PORTS, CMD_SEND,
    CMD_TOGGLE_BOTTOM_PANEL, CMD_TOGGLE_RIGHT_DOCK,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 单个快捷键绑定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct KeyBinding {
    /// egui::Key 名称，如 "R", "W", "Num1", "Backtick", "Enter"
    pub key: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl KeyBinding {
    pub(crate) fn new(key: impl Into<String>, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            key: key.into(),
            ctrl,
            shift,
            alt,
        }
    }

    /// 用户可读的显示字符串，如 "Ctrl+Shift+O"。
    pub(crate) fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// 快捷键映射表。key 为命令 ID（内置 `$` 前缀或 `plugin_id:command_id`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Keymap {
    /// 每个命令可以有多个快捷键绑定。
    pub bindings: HashMap<String, Vec<KeyBinding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
        }
    }
}

impl Keymap {
    /// 设置某个命令的绑定列表。
    pub(crate) fn set_bindings(&mut self, command_id: &str, bindings: Vec<KeyBinding>) {
        if bindings.is_empty() {
            self.bindings.remove(command_id);
        } else {
            self.bindings.insert(command_id.to_owned(), bindings);
        }
    }

    pub(crate) fn remove_binding_everywhere(&mut self, binding: &KeyBinding) {
        self.bindings.retain(|_, bindings| {
            bindings.retain(|candidate| candidate != binding);
            !bindings.is_empty()
        });
    }

    /// 获取某个命令的绑定列表。
    pub(crate) fn get_bindings(&self, command_id: &str) -> Vec<KeyBinding> {
        self.bindings.get(command_id).cloned().unwrap_or_default()
    }
}

/// 默认快捷键绑定（VSCode 风格）。
fn default_bindings() -> HashMap<String, Vec<KeyBinding>> {
    let mut m = HashMap::new();

    m.insert(
        CMD_REFRESH_PORTS.to_owned(),
        vec![KeyBinding::new("R", true, false, false)],
    );
    m.insert(
        CMD_OPEN_PORT.to_owned(),
        vec![KeyBinding::new("O", true, true, false)],
    );
    m.insert(
        CMD_TOGGLE_BOTTOM_PANEL.to_owned(),
        vec![KeyBinding::new("Backtick", true, false, false)],
    );
    m.insert(
        CMD_TOGGLE_RIGHT_DOCK.to_owned(),
        vec![KeyBinding::new("B", true, false, true)],
    );
    m.insert(
        CMD_SEND.to_owned(),
        vec![KeyBinding::new("Enter", true, false, false)],
    );
    m.insert(
        CMD_COMMAND_PALETTE.to_owned(),
        vec![KeyBinding::new("K", true, false, false)],
    );
    m.insert(
        CMD_CLEAR_TERMINAL.to_owned(),
        vec![KeyBinding::new("L", true, false, false)],
    );
    // StartRecording、ReconnectPort、AddBookmark 默认无快捷键

    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_registry::CommandRegistry;
    use crate::command_registry::{CMD_ADD_BOOKMARK, CMD_RECONNECT_PORT, CMD_START_RECORDING};

    /// 内置命令中默认应绑定快捷键的 ID（其余内置命令显式未绑定）。
    const DEFAULT_BOUND: &[&str] = &[
        CMD_REFRESH_PORTS,
        CMD_OPEN_PORT,
        CMD_TOGGLE_BOTTOM_PANEL,
        CMD_TOGGLE_RIGHT_DOCK,
        CMD_SEND,
        CMD_COMMAND_PALETTE,
        CMD_CLEAR_TERMINAL,
    ];
    const DEFAULT_UNBOUND: &[&str] = &[CMD_START_RECORDING, CMD_RECONNECT_PORT, CMD_ADD_BOOKMARK];

    #[test]
    fn default_keymap_covers_all_builtin_commands() {
        let km = Keymap::default();
        for command in CommandRegistry::builtin().all() {
            let bound = km.bindings.contains_key(&command.id);
            assert!(
                bound || DEFAULT_UNBOUND.contains(&command.id.as_str()),
                "command {} should have a default binding or be explicitly unbound",
                command.id
            );
        }
    }

    #[test]
    fn default_bindings_match_expected_set() {
        let km = Keymap::default();
        for id in DEFAULT_BOUND {
            assert!(
                km.bindings.contains_key(*id),
                "expected default binding for {id}"
            );
        }
        for id in DEFAULT_UNBOUND {
            assert!(
                !km.bindings.contains_key(*id),
                "expected {id} to be unbound by default"
            );
        }
    }

    #[test]
    fn keybinding_display_formats_correctly() {
        let kb = KeyBinding::new("O", true, true, false);
        assert_eq!(kb.display(), "Ctrl+Shift+O");

        let kb2 = KeyBinding::new("Backtick", true, false, false);
        assert_eq!(kb2.display(), "Ctrl+Backtick");

        let kb3 = KeyBinding::new("B", true, false, true);
        assert_eq!(kb3.display(), "Ctrl+Alt+B");
    }

    #[test]
    fn set_and_get_bindings() {
        let mut km = Keymap::default();
        let new_bindings = vec![KeyBinding::new("F5", false, false, false)];
        km.set_bindings(CMD_REFRESH_PORTS, new_bindings.clone());
        assert_eq!(km.get_bindings(CMD_REFRESH_PORTS), new_bindings);
    }

    #[test]
    fn clear_bindings() {
        let mut km = Keymap::default();
        km.set_bindings(CMD_REFRESH_PORTS, vec![]);
        assert!(km.get_bindings(CMD_REFRESH_PORTS).is_empty());
    }

    #[test]
    fn remove_binding_everywhere_clears_conflicts() {
        let mut km = Keymap::default();
        let binding = KeyBinding::new("F5", false, false, false);
        km.set_bindings(CMD_REFRESH_PORTS, vec![binding.clone()]);
        km.set_bindings(CMD_START_RECORDING, vec![binding.clone()]);

        km.remove_binding_everywhere(&binding);

        assert!(km.get_bindings(CMD_REFRESH_PORTS).is_empty());
        assert!(km.get_bindings(CMD_START_RECORDING).is_empty());
    }
}
