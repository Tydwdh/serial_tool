//! 可配置快捷键系统。
//!
//! 每个 [`Action`] 可以有多个快捷键绑定，存储在 [`Keymap`] 中。
//! 默认绑定参考 VSCode 风格，用户可在设置面板中自定义。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 所有可通过快捷键触发的操作。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    /// 刷新串口列表
    RefreshPorts,
    /// 打开/关闭选中串口
    OpenPort,
    /// 切换左侧活动栏
    ToggleActivityBar,
    /// 切换底部面板
    ToggleBottomPanel,
    /// 切换右侧边栏
    ToggleRightDock,
    /// 发送当前输入
    Send,
    /// 开始/停止录制
    StartRecording,
    /// 重连当前串口
    ReconnectPort,
    /// 录制时添加标记点
    AddBookmark,
    /// 打开命令面板
    CommandPalette,
    /// 插件命令: (plugin_id, command_id)
    PluginCommand(String, String),
}

impl Action {
    /// 所有内置动作列表（不含插件命令）。
    pub(crate) const ALL: &[Action] = &[
        Action::RefreshPorts,
        Action::OpenPort,
        Action::ToggleActivityBar,
        Action::ToggleBottomPanel,
        Action::ToggleRightDock,
        Action::Send,
        Action::StartRecording,
        Action::ReconnectPort,
        Action::AddBookmark,
        Action::CommandPalette,
    ];

    /// 合并内置 Action 与插件命令。
    pub(crate) fn all_with_plugins(
        plugin_summaries: &[tool_extension::PluginSummary],
    ) -> Vec<Action> {
        let mut actions: Vec<Action> = Self::ALL.to_vec();
        for summary in plugin_summaries {
            for cmd in &summary.contributes.commands {
                actions.push(Action::PluginCommand(summary.id.clone(), cmd.id.clone()));
            }
        }
        actions
    }

    /// 编码为 Keymap 中的字符串 key。
    pub(crate) fn key(&self) -> String {
        match self {
            Action::RefreshPorts => "$RefreshPorts".into(),
            Action::OpenPort => "$OpenPort".into(),
            Action::ToggleActivityBar => "$ToggleActivityBar".into(),
            Action::ToggleBottomPanel => "$ToggleBottomPanel".into(),
            Action::ToggleRightDock => "$ToggleRightDock".into(),
            Action::Send => "$Send".into(),
            Action::StartRecording => "$StartRecording".into(),
            Action::ReconnectPort => "$ReconnectPort".into(),
            Action::AddBookmark => "$AddBookmark".into(),
            Action::CommandPalette => "$CommandPalette".into(),
            Action::PluginCommand(plugin_id, command_id) => {
                format!("{plugin_id}:{command_id}")
            }
        }
    }

    /// 从字符串 key 解码。内置 key 以 `$` 开头，插件 key 为 `plugin_id:command_id`。
    pub(crate) fn from_key(key: &str) -> Option<Action> {
        match key {
            "$RefreshPorts" => Some(Action::RefreshPorts),
            "$OpenPort" => Some(Action::OpenPort),
            "$ToggleActivityBar" => Some(Action::ToggleActivityBar),
            "$ToggleBottomPanel" => Some(Action::ToggleBottomPanel),
            "$ToggleRightDock" => Some(Action::ToggleRightDock),
            "$Send" => Some(Action::Send),
            "$StartRecording" => Some(Action::StartRecording),
            "$ReconnectPort" => Some(Action::ReconnectPort),
            "$AddBookmark" => Some(Action::AddBookmark),
            "$CommandPalette" => Some(Action::CommandPalette),
            other => {
                // 插件命令: "plugin_id:command_id"
                let (plugin_id, command_id) = other.split_once(':')?;
                Some(Action::PluginCommand(
                    plugin_id.to_owned(),
                    command_id.to_owned(),
                ))
            }
        }
    }

    /// 用户可见的中文标签。
    pub(crate) fn label(&self) -> String {
        match self {
            Action::RefreshPorts => "刷新串口".into(),
            Action::OpenPort => "打开/关闭串口".into(),
            Action::ToggleActivityBar => "切换左侧活动栏".into(),
            Action::ToggleBottomPanel => "切换底部面板".into(),
            Action::ToggleRightDock => "切换右侧边栏".into(),
            Action::Send => "发送".into(),
            Action::StartRecording => "开始/停止录制".into(),
            Action::ReconnectPort => "重连串口".into(),
            Action::AddBookmark => "添加录制标记".into(),
            Action::CommandPalette => "命令面板".into(),
            Action::PluginCommand(plugin_id, command_id) => {
                format!("{plugin_id}:{command_id}")
            }
        }
    }

    /// 带插件信息的完整标签（用于设置面板显示）。
    pub(crate) fn label_with_plugins(
        &self,
        plugin_summaries: &[tool_extension::PluginSummary],
    ) -> String {
        match self {
            Action::PluginCommand(plugin_id, command_id) => {
                let plugin_name = plugin_summaries
                    .iter()
                    .find(|s| s.id == *plugin_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or(plugin_id.as_str());
                let command_title = plugin_summaries
                    .iter()
                    .find(|s| s.id == *plugin_id)
                    .and_then(|s| s.contributes.commands.iter().find(|c| c.id == *command_id))
                    .map(|c| c.title.as_str())
                    .unwrap_or(command_id.as_str());
                format!("{plugin_name}: {command_title}")
            }
            _ => self.label(),
        }
    }
}

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

/// 快捷键映射表。key 为 `$` 前缀的内置 Action 名或 `plugin_id:command_id`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Keymap {
    /// 每个动作可以有多个快捷键绑定。
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
    /// 获取某个动作的快捷键显示字符串（取第一个绑定）。
    #[allow(dead_code)]
    pub(crate) fn shortcut_display(&self, action: &Action) -> String {
        self.bindings
            .get(&action.key())
            .and_then(|v| v.first())
            .map(|b| b.display())
            .unwrap_or_default()
    }

    /// 设置某个动作的绑定列表。
    pub(crate) fn set_bindings(&mut self, action: &Action, bindings: Vec<KeyBinding>) {
        let key = action.key();
        if bindings.is_empty() {
            self.bindings.remove(&key);
        } else {
            self.bindings.insert(key, bindings);
        }
    }

    pub(crate) fn remove_binding_everywhere(&mut self, binding: &KeyBinding) {
        self.bindings.retain(|_, bindings| {
            bindings.retain(|candidate| candidate != binding);
            !bindings.is_empty()
        });
    }

    /// 获取某个动作的绑定列表。
    pub(crate) fn get_bindings(&self, action: &Action) -> Vec<KeyBinding> {
        self.bindings
            .get(&action.key())
            .cloned()
            .unwrap_or_default()
    }
}

/// 默认快捷键绑定（VSCode 风格）。
fn default_bindings() -> HashMap<String, Vec<KeyBinding>> {
    let mut m = HashMap::new();

    m.insert(
        Action::RefreshPorts.key(),
        vec![KeyBinding::new("R", true, false, false)],
    );
    m.insert(
        Action::OpenPort.key(),
        vec![KeyBinding::new("O", true, true, false)],
    );
    m.insert(
        Action::ToggleActivityBar.key(),
        vec![KeyBinding::new("B", true, false, false)],
    );
    m.insert(
        Action::ToggleBottomPanel.key(),
        vec![KeyBinding::new("Backtick", true, false, false)],
    );
    m.insert(
        Action::ToggleRightDock.key(),
        vec![KeyBinding::new("B", true, false, true)],
    );
    m.insert(
        Action::Send.key(),
        vec![KeyBinding::new("Enter", true, false, false)],
    );
    m.insert(
        Action::CommandPalette.key(),
        vec![KeyBinding::new("K", true, false, false)],
    );
    // StartRecording、ReconnectPort 默认无快捷键

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keymap_has_all_actions() {
        let km = Keymap::default();
        for action in Action::ALL {
            let key = action.key();
            assert!(
                km.bindings.contains_key(&key)
                    || matches!(
                        action,
                        Action::StartRecording | Action::ReconnectPort | Action::AddBookmark
                    ),
                "action {action:?} (key={key}) should have a default binding or be explicitly unbound"
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
    fn action_labels_are_unique() {
        let mut labels: Vec<String> = Action::ALL.iter().map(|a| a.label()).collect();
        labels.sort();
        let orig_len = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), orig_len, "all action labels must be unique");
    }

    #[test]
    fn action_key_roundtrip() {
        for action in Action::ALL {
            let key = action.key();
            let decoded = Action::from_key(&key);
            assert!(decoded.is_some(), "failed to decode key: {key}");
            assert_eq!(decoded.unwrap().key(), key);
        }
    }

    #[test]
    fn plugin_command_key_roundtrip() {
        let action = Action::PluginCommand("demo.test".into(), "demo.test.run".into());
        let key = action.key();
        assert_eq!(key, "demo.test:demo.test.run");
        let decoded = Action::from_key(&key);
        assert!(decoded.is_some());
        match decoded.unwrap() {
            Action::PluginCommand(pid, cid) => {
                assert_eq!(pid, "demo.test");
                assert_eq!(cid, "demo.test.run");
            }
            _ => panic!("expected PluginCommand"),
        }
    }

    #[test]
    fn set_and_get_bindings() {
        let mut km = Keymap::default();
        let action = Action::RefreshPorts;
        let new_bindings = vec![KeyBinding::new("F5", false, false, false)];
        km.set_bindings(&action, new_bindings.clone());
        assert_eq!(km.get_bindings(&action), new_bindings);
    }

    #[test]
    fn clear_bindings() {
        let mut km = Keymap::default();
        let action = Action::RefreshPorts;
        km.set_bindings(&action, vec![]);
        assert!(km.get_bindings(&action).is_empty());
    }

    #[test]
    fn remove_binding_everywhere_clears_conflicts() {
        let mut km = Keymap::default();
        let binding = KeyBinding::new("F5", false, false, false);
        km.set_bindings(&Action::RefreshPorts, vec![binding.clone()]);
        km.set_bindings(&Action::StartRecording, vec![binding.clone()]);

        km.remove_binding_everywhere(&binding);

        assert!(km.get_bindings(&Action::RefreshPorts).is_empty());
        assert!(km.get_bindings(&Action::StartRecording).is_empty());
    }
}
