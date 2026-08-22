use std::collections::BTreeSet;
use tool_extension::{PluginDiagnostic, PluginState};

/// 已安装插件的只读行 — Panel 不再持有 PluginManager。
#[derive(Debug, Clone)]
pub struct InstalledPluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: PluginState,
}

#[derive(Debug, Clone, Default)]
pub struct PluginViewState {
    pub installed: Vec<InstalledPluginRow>,
    pub diagnostics: Vec<PluginDiagnostic>,
    pub installed_ids: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub enum PluginUiCommand {
    Enable(String),
    Disable(String),
    Restart(String),
    DiscoverRoots { root: String },
}

impl From<&[tool_extension::PluginSummary]> for PluginViewState {
    fn from(summaries: &[tool_extension::PluginSummary]) -> Self {
        let installed = summaries
            .iter()
            .map(|s| InstalledPluginRow {
                id: s.id.clone(),
                name: s.name.clone(),
                version: s.version.clone(),
                state: s.state,
            })
            .collect::<Vec<_>>();
        let installed_ids = installed.iter().map(|r| r.id.clone()).collect();
        Self {
            installed,
            diagnostics: Vec::new(),
            installed_ids,
        }
    }
}
