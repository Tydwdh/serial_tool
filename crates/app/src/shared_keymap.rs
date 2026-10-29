//! Keyboard binding model for the native app.
//!
//! The persisted command IDs and binding format are stable.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(crate) const CMD_REFRESH_PORTS: &str = "$RefreshPorts";
pub(crate) const CMD_OPEN_PORT: &str = "$OpenPort";
pub(crate) const CMD_TOGGLE_BOTTOM_PANEL: &str = "$ToggleBottomPanel";
pub(crate) const CMD_TOGGLE_RIGHT_DOCK: &str = "$ToggleRightDock";
pub(crate) const CMD_SEND: &str = "$Send";
pub(crate) const CMD_START_RECORDING: &str = "$StartRecording";
pub(crate) const CMD_RECONNECT_PORT: &str = "$ReconnectPort";
pub(crate) const CMD_ADD_BOOKMARK: &str = "$AddBookmark";
pub(crate) const CMD_COMMAND_PALETTE: &str = "$CommandPalette";
pub(crate) const CMD_CLEAR_TERMINAL: &str = "$ClearTerminal";

/// 单个快捷键绑定。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct KeyBinding {
    /// egui::Key 的 Debug 名称，如 `R`、`Backtick`、`Enter`。
    pub(crate) key: String,
    pub(crate) ctrl: bool,
    pub(crate) shift: bool,
    pub(crate) alt: bool,
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
    #[serde(default = "default_bindings")]
    pub(crate) bindings: HashMap<String, Vec<KeyBinding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
        }
    }
}

impl Keymap {
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

    pub(crate) fn get_bindings(&self, command_id: &str) -> Vec<KeyBinding> {
        self.bindings.get(command_id).cloned().unwrap_or_default()
    }
}

/// 默认快捷键映射表：命令 ID → (主键, Ctrl, Shift, Alt)。
/// 这是新建配置的默认值来源，并会随 `PersistedConfig.keymap` 一起持久化。
const DEFAULT_KEYBINDINGS: &[(&str, &str, bool, bool, bool)] = &[
    (CMD_REFRESH_PORTS, "R", true, false, false),
    (CMD_OPEN_PORT, "O", true, true, false),
    (CMD_TOGGLE_BOTTOM_PANEL, "Backtick", true, false, false),
    (CMD_TOGGLE_RIGHT_DOCK, "B", true, false, true),
    (CMD_SEND, "Enter", true, false, false),
    (CMD_COMMAND_PALETTE, "K", true, false, false),
    (CMD_CLEAR_TERMINAL, "L", true, false, false),
];

fn default_bindings() -> HashMap<String, Vec<KeyBinding>> {
    DEFAULT_KEYBINDINGS
        .iter()
        .map(|(id, key, ctrl, shift, alt)| {
            let binding = KeyBinding::new(*key, *ctrl, *shift, *alt);
            ((*id).to_owned(), vec![binding])
        })
        .collect()
}
