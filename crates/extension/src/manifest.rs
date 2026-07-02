//! 插件清单数据模型：`PluginManifest` 及其关联类型。
//!
//! 从 `lib.rs` 抽出的纯数据结构，供 `PluginManager` 与外部消费。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
    /// 默认入口（live.replay 不存在时使用）
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

    /// 回放解析器配置（可选）
    #[serde(default)]
    pub replay: Option<ReplayConfig>,
}

impl PluginManifest {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.id.trim().is_empty() {
            errors.push("plugin id 不能为空".to_owned());
        }
        if self.name.trim().is_empty() {
            errors.push("plugin name 不能为空".to_owned());
        }
        if self.version.trim().is_empty() {
            errors.push("plugin version 不能为空".to_owned());
        }
        if self.runtime.trim().is_empty() {
            errors.push("plugin runtime 不能为空".to_owned());
        }
        if self.live_main().trim().is_empty() {
            errors.push("plugin live main 不能为空".to_owned());
        }

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
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn api_version_supported(&self) -> bool {
        SUPPORTED_PLUGIN_API_VERSIONS
            .iter()
            .any(|supported| *supported == self.api_version)
    }

    pub fn live_main(&self) -> &str {
        self.live
            .as_ref()
            .and_then(|l| l.main.as_deref())
            .unwrap_or(&self.main)
    }

    pub fn live_permissions(&self) -> &[String] {
        self.live
            .as_ref()
            .and_then(|l| l.permissions.as_ref())
            .unwrap_or(&self.permissions)
    }

    pub fn live_subscriptions(&self) -> &[String] {
        self.live
            .as_ref()
            .map(|l| l.subscriptions.as_slice())
            .unwrap_or(&[])
    }

    pub fn has_replay_analyzer(&self) -> bool {
        self.replay.is_some()
    }

    pub fn replay_main(&self) -> Option<&str> {
        self.replay.as_ref().map(|r| r.main.as_str())
    }

    pub fn replay_permissions(&self) -> &[String] {
        self.replay
            .as_ref()
            .map(|r| r.permissions.as_slice())
            .unwrap_or(&[])
    }

    pub fn replay_subscriptions(&self) -> &[String] {
        self.replay
            .as_ref()
            .map(|r| r.subscriptions.as_slice())
            .unwrap_or(&[])
    }

    pub fn replay_outputs(&self) -> &[String] {
        self.replay
            .as_ref()
            .map(|r| r.outputs.as_slice())
            .unwrap_or(&[])
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayConfig {
    pub main: String,

    #[serde(default)]
    pub subscriptions: Vec<String>,

    #[serde(default)]
    pub outputs: Vec<String>,

    #[serde(default)]
    pub permissions: Vec<String>,
}

/// 已发现插件中 replay analyzer 的元信息。
/// 不需要插件处于 enabled 状态。
#[derive(Debug, Clone)]
pub struct ReplayAnalyzerEntry {
    pub plugin_id: String,
    pub manifest: PluginManifest,
    pub root: PathBuf,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginPanelContribution {
    pub id: String,
    pub title: String,

    #[serde(default)]
    pub kind: String,
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

    // ── replay analyzer ──
    pub has_replay_analyzer: bool,
    pub replay_subscriptions: Vec<String>,
    pub replay_outputs: Vec<String>,

    // ── 命令状态 ──
    /// 运行时已注册的命令 ID 列表（仅 Running 状态有意义）
    #[serde(default)]
    pub registered_commands: Vec<String>,

    /// 声明但未注册的命令 ID 列表
    #[serde(default)]
    pub missing_commands: Vec<String>,

    /// 动态注册但未在 manifest 声明的命令 ID 列表
    #[serde(default)]
    pub undeclared_commands: Vec<String>,
}
