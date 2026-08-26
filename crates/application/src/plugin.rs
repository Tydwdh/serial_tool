//! Platform-neutral plugin presentation DTOs.
//!
//! Native and browser Lua plugins use the same manifest, source and DTOs.
//! Runtime adapters convert into these DTOs before they reach the UI.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStateView {
    Discovered,
    Enabled,
    Running,
    Finished,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDiagnosticSeverityView {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDiagnosticView {
    pub severity: PluginDiagnosticSeverityView,
    pub code: String,
    pub plugin_id: Option<String>,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommandView {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginUiContributionView {
    pub id: String,
    pub slot: String,
    pub kind: String,
    pub title: Option<String>,
    pub command: Option<String>,
    pub tooltip: Option<String>,
    pub order: i32,
    pub enabled: bool,
    pub visible: bool,
    pub record_send_input: bool,
    pub default: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginPanelContributionView {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub config: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginSettingView {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub default: serde_json::Value,
    pub options: Vec<serde_json::Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub rows: Option<usize>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginContributesView {
    pub commands: Vec<PluginCommandView>,
    pub ui: Vec<PluginUiContributionView>,
    pub panels: Vec<PluginPanelContributionView>,
    pub settings: Vec<PluginSettingView>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginSummaryView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub runtime: String,
    pub state: PluginStateView,
    pub permissions: Vec<String>,
    pub contributes: PluginContributesView,
    pub path: String,
    pub last_error: Option<String>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    pub has_replay_analyzer: bool,
    pub replay_subscriptions: Vec<String>,
    pub replay_outputs: Vec<String>,
    pub registered_commands: Vec<String>,
    pub missing_commands: Vec<String>,
    pub undeclared_commands: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginView {
    pub summaries: Vec<PluginSummaryView>,
    pub diagnostics: Vec<PluginDiagnosticView>,
}

/// Plugin intent crossing the Application boundary.
///
/// The native implementation executes these through `PluginManager`; the
/// browser implementation queues them for its Lua capability. Neither UI is
/// allowed to manipulate a Lua VM or host handle directly.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginCommand {
    Enable {
        plugin_id: String,
    },
    Disable {
        plugin_id: String,
    },
    Reload,
    Execute {
        plugin_id: String,
        command_id: String,
        context: serde_json::Value,
    },
}
