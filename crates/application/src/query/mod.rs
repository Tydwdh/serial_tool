//! Query — 只读视图，UI 通过 `Workbench::query_*()` 读取状态。

pub use crate::plugin::{
    PluginCommandView, PluginContributesView, PluginDiagnosticSeverityView, PluginDiagnosticView,
    PluginPanelContributionView, PluginSettingView, PluginStateView, PluginSummaryView,
    PluginUiContributionView, PluginView,
};
pub use crate::recording::{RecordModeView, RecorderStatsView, RecordingStatusView};
pub use crate::replay::{
    ReplayBlockReasonView, ReplayBookmarkView, ReplayLoadReportView, ReplayPolicyView,
    ReplayStateView, ReplayStatusView,
};
pub use crate::transport::{
    PortStatusView, PortTypeView, PortView, SerialConfigView, TransportView,
};

pub type NetworkPortConfig = tool_platform::NetworkSerialConfig;

impl From<tool_recorder::ReplayState> for ReplayStateView {
    fn from(state: tool_recorder::ReplayState) -> Self {
        match state {
            tool_recorder::ReplayState::Empty => Self::Empty,
            tool_recorder::ReplayState::Loaded => Self::Loaded,
            tool_recorder::ReplayState::Playing => Self::Playing,
            tool_recorder::ReplayState::Paused => Self::Paused,
            tool_recorder::ReplayState::Finished => Self::Finished,
        }
    }
}

impl From<tool_recorder::ReplayBlockReason> for ReplayBlockReasonView {
    fn from(reason: tool_recorder::ReplayBlockReason) -> Self {
        match reason {
            tool_recorder::ReplayBlockReason::NeedAnalyzer => Self::NeedAnalyzer,
            tool_recorder::ReplayBlockReason::AnalyzerFailed(error) => Self::AnalyzerFailed(error),
        }
    }
}

impl From<tool_recorder::ReplayPolicy> for ReplayPolicyView {
    fn from(policy: tool_recorder::ReplayPolicy) -> Self {
        match policy {
            tool_recorder::ReplayPolicy::AutoPreferRecorded => Self::AutoPreferRecorded,
            tool_recorder::ReplayPolicy::ExactRecorded => Self::ExactRecorded,
            tool_recorder::ReplayPolicy::ReparseRaw => Self::ReparseRaw,
        }
    }
}

impl From<ReplayPolicyView> for tool_recorder::ReplayPolicy {
    fn from(policy: ReplayPolicyView) -> Self {
        match policy {
            ReplayPolicyView::AutoPreferRecorded => Self::AutoPreferRecorded,
            ReplayPolicyView::ExactRecorded => Self::ExactRecorded,
            ReplayPolicyView::ReparseRaw => Self::ReparseRaw,
        }
    }
}

impl From<tool_recorder::ReplayLoadReport> for ReplayLoadReportView {
    fn from(report: tool_recorder::ReplayLoadReport) -> Self {
        Self {
            loaded: report.loaded,
            skipped: report.skipped,
            first_errors: report.first_errors,
        }
    }
}

// 插件清单的应用层只读视图。
//
// 这里刻意不暴露 `tool_extension` 的 manifest 类型，也不把本地
// `PathBuf` 穿过 Application 边界。插件管理器仍然可以使用自己的内部
// 模型，但 UI 只依赖这些稳定的展示 DTO。
impl From<tool_extension::PluginState> for PluginStateView {
    fn from(state: tool_extension::PluginState) -> Self {
        match state {
            tool_extension::PluginState::Discovered => Self::Discovered,
            tool_extension::PluginState::Enabled => Self::Enabled,
            tool_extension::PluginState::Running => Self::Running,
            tool_extension::PluginState::Finished => Self::Finished,
            tool_extension::PluginState::Failed => Self::Failed,
            tool_extension::PluginState::Disabled => Self::Disabled,
        }
    }
}

impl From<tool_extension::PluginDiagnosticSeverity> for PluginDiagnosticSeverityView {
    fn from(severity: tool_extension::PluginDiagnosticSeverity) -> Self {
        match severity {
            tool_extension::PluginDiagnosticSeverity::Warning => Self::Warning,
            tool_extension::PluginDiagnosticSeverity::Error => Self::Error,
        }
    }
}

impl From<tool_extension::PluginDiagnostic> for PluginDiagnosticView {
    fn from(diagnostic: tool_extension::PluginDiagnostic) -> Self {
        Self {
            severity: diagnostic.severity.into(),
            code: diagnostic.code,
            plugin_id: diagnostic.plugin_id,
            path: diagnostic.path.display().to_string(),
            message: diagnostic.message,
        }
    }
}

impl From<tool_extension::manifest::PluginCommand> for PluginCommandView {
    fn from(command: tool_extension::manifest::PluginCommand) -> Self {
        Self {
            id: command.id,
            title: command.title,
        }
    }
}

impl From<tool_extension::manifest::PluginUiContribution> for PluginUiContributionView {
    fn from(contribution: tool_extension::manifest::PluginUiContribution) -> Self {
        Self {
            id: contribution.id,
            slot: contribution.slot,
            kind: contribution.kind,
            title: contribution.title,
            command: contribution.command,
            tooltip: contribution.tooltip,
            order: contribution.order,
            enabled: contribution.enabled,
            visible: contribution.visible,
            record_send_input: contribution.record_send_input,
            default: contribution.default,
        }
    }
}

impl From<tool_extension::manifest::PluginPanelContribution> for PluginPanelContributionView {
    fn from(panel: tool_extension::manifest::PluginPanelContribution) -> Self {
        Self {
            id: panel.id,
            title: panel.title,
            kind: panel.kind,
            config: panel.config,
        }
    }
}

impl From<tool_extension::manifest::PluginSetting> for PluginSettingView {
    fn from(setting: tool_extension::manifest::PluginSetting) -> Self {
        Self {
            id: setting.id,
            title: setting.title,
            kind: setting.kind,
            default: setting.default,
            options: setting.options,
            min: setting.min,
            max: setting.max,
            step: setting.step,
            rows: setting.rows,
            description: setting.description,
        }
    }
}

impl From<tool_extension::manifest::PluginContributes> for PluginContributesView {
    fn from(contributes: tool_extension::manifest::PluginContributes) -> Self {
        Self {
            commands: contributes.commands.into_iter().map(Into::into).collect(),
            ui: contributes.ui.into_iter().map(Into::into).collect(),
            panels: contributes.panels.into_iter().map(Into::into).collect(),
            settings: contributes.settings.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<tool_extension::PluginSummary> for PluginSummaryView {
    fn from(summary: tool_extension::PluginSummary) -> Self {
        Self {
            id: summary.id,
            name: summary.name,
            version: summary.version,
            api_version: summary.api_version,
            runtime: summary.runtime,
            state: summary.state.into(),
            permissions: summary.permissions,
            contributes: summary.contributes.into(),
            path: summary.path.display().to_string(),
            last_error: summary.last_error,
            description: summary.description,
            author: summary.author,
            homepage: summary.homepage,
            repository: summary.repository,
            license: summary.license,
            category: summary.category,
            icon: summary.icon,
            has_replay_analyzer: summary.has_replay_analyzer,
            replay_subscriptions: summary.replay_subscriptions,
            replay_outputs: summary.replay_outputs,
            registered_commands: summary.registered_commands,
            missing_commands: summary.missing_commands,
            undeclared_commands: summary.undeclared_commands,
        }
    }
}
