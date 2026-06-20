//! 可配置快捷键系统。
//!
//! 每个 [`Action`] 可以有多个快捷键绑定，存储在 [`Keymap`] 中。
//! 默认绑定参考 VSCode 风格，用户可在设置面板中自定义。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 所有可通过快捷键触发的操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    ToggleRightSidebar,
    /// 切换到活动栏第 1 项（设备）
    SelectActivity1,
    /// 切换到活动栏第 2 项（回放）
    SelectActivity2,
    /// 切换到活动栏第 3 项（插件）
    SelectActivity3,
    /// 切换到活动栏第 4 项（设置）
    SelectActivity4,
    /// 发送当前输入
    Send,
    /// 开始/停止录制
    StartRecording,
    /// 重连当前串口
    ReconnectPort,
}

impl Action {
    /// 所有可配置的动作列表。
    pub(crate) const ALL: &[Action] = &[
        Action::RefreshPorts,
        Action::OpenPort,
        Action::ToggleActivityBar,
        Action::ToggleBottomPanel,
        Action::ToggleRightSidebar,
        Action::SelectActivity1,
        Action::SelectActivity2,
        Action::SelectActivity3,
        Action::SelectActivity4,
        Action::Send,
        Action::StartRecording,
        Action::ReconnectPort,
    ];

    /// 用户可见的中文标签。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Action::RefreshPorts => "刷新串口",
            Action::OpenPort => "打开/关闭串口",
            Action::ToggleActivityBar => "切换左侧活动栏",
            Action::ToggleBottomPanel => "切换底部面板",
            Action::ToggleRightSidebar => "切换右侧边栏",
            Action::SelectActivity1 => "切换到设备",
            Action::SelectActivity2 => "切换到回放",
            Action::SelectActivity3 => "切换到插件",
            Action::SelectActivity4 => "切换到设置",
            Action::Send => "发送",
            Action::StartRecording => "开始/停止录制",
            Action::ReconnectPort => "重连串口",
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
        Self { key: key.into(), ctrl, shift, alt }
    }

    /// 用户可读的显示字符串，如 "Ctrl+Shift+O"。
    pub(crate) fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl { parts.push("Ctrl"); }
        if self.alt { parts.push("Alt"); }
        if self.shift { parts.push("Shift"); }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// 快捷键映射表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Keymap {
    /// 每个动作可以有多个快捷键绑定。
    pub bindings: HashMap<Action, Vec<KeyBinding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self { bindings: default_bindings() }
    }
}

impl Keymap {
    /// 获取某个动作的快捷键显示字符串（取第一个绑定）。
    #[allow(dead_code)]
    pub(crate) fn shortcut_display(&self, action: Action) -> String {
        self.bindings
            .get(&action)
            .and_then(|v| v.first())
            .map(|b| b.display())
            .unwrap_or_default()
    }

    /// 设置某个动作的绑定列表。
    pub(crate) fn set_bindings(&mut self, action: Action, bindings: Vec<KeyBinding>) {
        if bindings.is_empty() {
            self.bindings.remove(&action);
        } else {
            self.bindings.insert(action, bindings);
        }
    }
}

/// 默认快捷键绑定（VSCode 风格）。
fn default_bindings() -> HashMap<Action, Vec<KeyBinding>> {
    use Action::*;
    let mut m = HashMap::new();

    m.insert(RefreshPorts,       vec![KeyBinding::new("R", true, false, false)]);
    m.insert(OpenPort,           vec![KeyBinding::new("O", true, true, false)]);
    m.insert(ToggleActivityBar,  vec![KeyBinding::new("B", true, false, false)]);
    m.insert(ToggleBottomPanel,  vec![KeyBinding::new("Backtick", true, false, false)]);
    m.insert(ToggleRightSidebar, vec![KeyBinding::new("B", true, false, true)]);
    m.insert(SelectActivity1,    vec![KeyBinding::new("Num1", true, false, false)]);
    m.insert(SelectActivity2,    vec![KeyBinding::new("Num2", true, false, false)]);
    m.insert(SelectActivity3,    vec![KeyBinding::new("Num3", true, false, false)]);
    m.insert(SelectActivity4,    vec![KeyBinding::new("Num4", true, false, false)]);
    m.insert(Send,               vec![KeyBinding::new("Enter", true, false, false)]);
    // StartRecording 和 ReconnectPort 默认无快捷键

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keymap_has_all_actions() {
        let km = Keymap::default();
        for action in Action::ALL {
            assert!(
                km.bindings.contains_key(action) || *action == Action::StartRecording || *action == Action::ReconnectPort,
                "action {action:?} should have a default binding or be explicitly unbound"
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
        let mut labels: Vec<&str> = Action::ALL.iter().map(|a| a.label()).collect();
        labels.sort();
        let orig_len = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), orig_len, "all action labels must be unique");
    }
}
