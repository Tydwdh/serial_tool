use std::collections::BTreeSet;
use tool_application::plugin::{PluginDiagnosticView, PluginStateView, PluginSummaryView};

/// 已安装插件的只读行 — Panel 不再持有 PluginManager。
#[derive(Debug, Clone)]
pub struct InstalledPluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: PluginStateView,
}

/// 插件面板的只读快照：已安装列表与清单加载诊断。
#[derive(Debug, Clone, Default)]
pub struct PluginViewState {
    pub installed: Vec<InstalledPluginRow>,
    pub diagnostics: Vec<PluginDiagnosticView>,
    pub installed_ids: BTreeSet<String>,
}

/// 插件面板向上回传的意图 — 由 Workbench::dispatch 承接。
#[derive(Debug, Clone)]
pub enum PluginUiCommand {
    Enable(String),
    Disable(String),
    Restart(String),
    DiscoverRoots { root: String },
}

impl From<&[PluginSummaryView]> for PluginViewState {
    fn from(summaries: &[PluginSummaryView]) -> Self {
        let installed = summaries
            .iter()
            .map(|summary| InstalledPluginRow {
                id: summary.id.clone(),
                name: summary.name.clone(),
                version: summary.version.clone(),
                state: summary.state,
            })
            .collect::<Vec<_>>();
        let installed_ids = installed.iter().map(|row| row.id.clone()).collect();
        Self {
            installed,
            diagnostics: Vec::new(),
            installed_ids,
        }
    }
}
