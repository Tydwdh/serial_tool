//! 插件清单数据模型：`PluginManifest` 及其关联类型。
//!
//! 从 `lib.rs` 抽出的纯数据结构，供 `PluginManager` 与外部消费。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub const CURRENT_PLUGIN_API_VERSION: &str = "0.1";
pub const SUPPORTED_PLUGIN_API_VERSIONS: &[&str] = &[CURRENT_PLUGIN_API_VERSION];

fn default_api_version() -> String {
    CURRENT_PLUGIN_API_VERSION.to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_api_version")]
    pub api_version: String,
    pub runtime: String,
    /// 实时入口的回退值：`live.main` 缺失时使用（见 `live_main`）
    pub main: String,

    #[serde(default)]
    pub permissions: Vec<String>,

    // ── 市场元数据（全部可选，用于插件市场展示与索引） ──
    /// 一句话描述插件功能
    #[serde(default)]
    pub description: Option<String>,

    /// 作者。建议 "名字 <邮箱>" 或 "GitHub 用户名" 格式
    #[serde(default)]
    pub author: Option<String>,

    /// 主页 URL（通常是仓库地址）
    #[serde(default)]
    pub homepage: Option<String>,

    /// 源码仓库 URL（与 homepage 可相同）
    #[serde(default)]
    pub repository: Option<String>,

    /// SPDX 许可证标识，如 "MIT"、"Apache-2.0"
    #[serde(default)]
    pub license: Option<String>,

    /// 分类标签，用于市场筛选。如 "gcode"、"chart"、"template"
    #[serde(default)]
    pub category: Option<String>,

    /// 图标文件相对路径（相对插件根目录），如 "icon.png"。市场 UI 展示用
    #[serde(default)]
    pub icon: Option<String>,

    #[serde(default)]
    pub contributes: PluginContributes,

    /// 实时插件配置（可选，不填时回退到顶层 main/permissions）
    #[serde(default)]
    pub live: Option<LiveConfig>,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.id.trim().is_empty() {
            errors.push("plugin id 不能为空".to_owned());
        } else if !is_safe_plugin_id(&self.id) {
            errors.push(format!(
                "plugin id '{}' 必须是安全的路径片段（仅字母数字、.、-、_，且不能以 . 开头/结尾或包含 ..）",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            errors.push("plugin name 不能为空".to_owned());
        }
        if self.version.trim().is_empty() {
            errors.push("plugin version 不能为空".to_owned());
        }
        if self.runtime.trim().is_empty() {
            errors.push("plugin runtime 不能为空".to_owned());
        } else if !crate::spec::RUNTIMES.contains(&self.runtime.as_str()) {
            errors.push(format!("不支持的 runtime '{}'", self.runtime));
        }
        if self.live_main().trim().is_empty() {
            errors.push("plugin live main 不能为空".to_owned());
        } else if !is_safe_plugin_relative_path(self.live_main()) {
            errors.push(format!(
                "plugin live main '{}' 必须是插件根目录内的相对路径（不能是绝对路径、..、Windows 盘符或反斜杠路径）",
                self.live_main()
            ));
        }

        // command id 集合要留到后面的 ui 循环：ui[].command 必须指向已声明的 command。
        let mut command_ids = BTreeSet::new();
        for command in &self.contributes.commands {
            if command.id.trim().is_empty() {
                errors.push("contributes.commands 包含空 id".to_owned());
                continue;
            }
            if !command_ids.insert(command.id.as_str()) {
                errors.push(format!("重复 command id '{}'", command.id));
            }
        }

        let mut ui_ids = BTreeSet::new();
        for item in &self.contributes.ui {
            if item.id.trim().is_empty() {
                errors.push("contributes.ui 包含空 id".to_owned());
                continue;
            }
            if !ui_ids.insert(item.id.as_str()) {
                errors.push(format!("重复 ui id '{}'", item.id));
            }
            if !crate::spec::UI_SLOTS.contains(&item.slot.as_str()) {
                errors.push(format!(
                    "ui '{}' 使用了不支持的 slot '{}'",
                    item.id, item.slot
                ));
            }
            if !crate::spec::UI_KINDS.contains(&item.kind.as_str()) {
                errors.push(format!(
                    "ui '{}' 使用了不支持的 kind '{}'",
                    item.id, item.kind
                ));
            }

            if let Some(command) = item
                .command
                .as_deref()
                .filter(|command| !command.trim().is_empty())
                && !command_ids.contains(command)
            {
                errors.push(format!(
                    "ui '{}' 引用了未声明 command '{}'",
                    item.id, command
                ));
            }
        }

        let mut panel_ids = BTreeSet::new();
        for panel in &self.contributes.panels {
            if panel.id.trim().is_empty() {
                errors.push("contributes.panels 包含空 id".to_owned());
                continue;
            }
            if !panel_ids.insert(panel.id.as_str()) {
                errors.push(format!("重复 panel id '{}'", panel.id));
            }
            if !crate::spec::PANEL_KINDS.contains(&panel.kind.as_str()) {
                errors.push(format!(
                    "panel '{}' 使用了不支持的 kind '{}'",
                    panel.id, panel.kind
                ));
            }
        }

        for setting in &self.contributes.settings {
            if !crate::spec::SETTING_KINDS.contains(&setting.kind.as_str()) {
                errors.push(format!(
                    "setting '{}' 使用了不支持的 kind '{}'",
                    setting.id, setting.kind
                ));
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(())
    }

    pub fn api_version_supported(&self) -> bool {
        SUPPORTED_PLUGIN_API_VERSIONS.contains(&self.api_version.as_str())
    }

    pub fn live_main(&self) -> &str {
        self.live
            .as_ref()
            .and_then(|live| live.main.as_deref())
            .unwrap_or(&self.main)
    }

    pub fn live_permissions(&self) -> &[String] {
        self.live
            .as_ref()
            .and_then(|live| live.permissions.as_ref())
            .unwrap_or(&self.permissions)
    }

    pub fn live_subscriptions(&self) -> &[String] {
        self.live
            .as_ref()
            .map(|live| live.subscriptions.as_slice())
            .unwrap_or(&[])
    }
}

/// 插件 id 同时是安装目录名、配置文件名与记录键，必须只能是单个安全路径片段。
fn is_safe_plugin_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// 插件入口必须是根目录内的普通相对路径；拒绝绝对路径、`..`、Windows 盘符与反斜杠。
fn is_safe_plugin_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && std::path::Path::new(path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveConfig {
    #[serde(default)]
    pub main: Option<String>,

    #[serde(default)]
    pub permissions: Option<Vec<String>>,

    #[serde(default)]
    pub subscriptions: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginDiagnostic {
    pub severity: PluginDiagnosticSeverity,
    pub code: String,
    pub plugin_id: Option<String>,
    pub path: PathBuf,
    pub message: String,
}

impl PluginDiagnostic {
    pub fn new(
        severity: PluginDiagnosticSeverity,
        code: impl Into<String>,
        plugin_id: Option<String>,
        path: PathBuf,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code: code.into(),
            plugin_id,
            path,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PluginContributes {
    #[serde(default)]
    pub commands: Vec<PluginCommand>,

    #[serde(default)]
    pub ui: Vec<PluginUiContribution>,

    #[serde(default)]
    pub panels: Vec<PluginPanelContribution>,

    #[serde(default)]
    pub settings: Vec<PluginSetting>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginUiContribution {
    pub id: String,
    pub slot: String,

    #[serde(default = "default_ui_contribution_kind")]
    pub kind: String,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub command: Option<String>,

    #[serde(default)]
    pub tooltip: Option<String>,

    #[serde(default)]
    pub order: i32,

    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_true")]
    pub visible: bool,

    #[serde(default)]
    pub record_send_input: bool,

    /// toggle 初始状态 / progress 初始值
    #[serde(default)]
    pub default: serde_json::Value,
}

fn default_ui_contribution_kind() -> String {
    "button".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginPanelContribution {
    pub id: String,
    pub title: String,

    #[serde(default = "default_panel_kind")]
    pub kind: String,

    /// 面板类型专属配置。宿主会把这些字段原样合并进
    /// `ui.panel.create`，因此声明式面板与动态面板使用同一协议。
    #[serde(flatten)]
    pub config: BTreeMap<String, serde_json::Value>,
}

fn default_panel_kind() -> String {
    "chart".to_owned()
}

fn default_setting_kind() -> String {
    "text".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginSetting {
    pub id: String,
    pub title: String,

    #[serde(default = "default_setting_kind")]
    pub kind: String,

    #[serde(default)]
    pub default: serde_json::Value,

    #[serde(default)]
    pub options: Vec<serde_json::Value>,

    #[serde(default)]
    pub min: Option<f64>,

    #[serde(default)]
    pub max: Option<f64>,

    #[serde(default)]
    pub step: Option<f64>,

    #[serde(default)]
    pub rows: Option<usize>,

    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PluginState {
    Discovered,
    Enabled,
    Running,
    Finished,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub runtime: String,
    pub state: PluginState,
    pub permissions: Vec<String>,
    pub contributes: PluginContributes,
    pub path: PathBuf,
    pub last_error: Option<String>,

    // ── 市场元数据（镜像 manifest，供 UI 展示） ──
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,

    // ── 命令状态（插件运行时诊断） ──
    #[serde(default)]
    pub registered_commands: Vec<String>,
    #[serde(default)]
    pub missing_commands: Vec<String>,
    #[serde(default)]
    pub undeclared_commands: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, main: &str) -> PluginManifest {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "test",
            "version": "1.0.0",
            "runtime": "lua",
            "main": main,
        }))
        .expect("minimal manifest JSON")
    }

    #[test]
    fn rejects_ids_that_can_escape_the_plugin_directory() {
        for id in [
            "..",
            "/home/user",
            "a/b",
            r"..\..",
            ".hidden",
            "trailing.",
            "a..b",
        ] {
            assert!(
                manifest(id, "main.lua").validate().is_err(),
                "id {id:?} must be rejected"
            );
        }
    }

    #[test]
    fn accepts_marketplace_style_ids() {
        for id in ["demo.gcode-sender", "template.hello", "vendor_plugin-1"] {
            assert!(
                manifest(id, "main.lua").validate().is_ok(),
                "id {id:?} must be accepted"
            );
        }
    }

    #[test]
    fn rejects_main_paths_that_escape_the_plugin_root() {
        for main in [
            "/dev/zero",
            "../outside.lua",
            r"..\outside.lua",
            r"C:\x.lua",
            r"lua\x.lua",
        ] {
            assert!(
                manifest("demo.test", main).validate().is_err(),
                "main {main:?} must be rejected"
            );
        }
        for main in ["main.lua", "lua/main.lua"] {
            assert!(
                manifest("demo.test", main).validate().is_ok(),
                "main {main:?} must be accepted"
            );
        }
    }
}
