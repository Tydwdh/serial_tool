use std::path::PathBuf;

use tool_databus::DataBus;
use tool_extension::{PluginManager, PluginSummary};
use tool_marketplace::{Registry, RegistryPlugin};
use tool_panels::{DynamicPanels, PluginsPanel};
use tool_transport::TransportManager;

pub struct PluginUiState {
    pub manager: PluginManager,
    pub plugins_panel: PluginsPanel,
    pub dynamic: DynamicPanels,
    pub registry: Option<Registry>,
    pub registry_error: Option<String>,
    pub registry_refreshing: bool,
    pub installing: std::collections::HashMap<String, f32>,
}

impl PluginUiState {
    pub fn new(bus: &DataBus, transport: &TransportManager) -> Self {
        let manager = PluginManager::new(bus.clone(), transport.clone());
        let dynamic = DynamicPanels::new(bus);
        Self {
            manager,
            plugins_panel: PluginsPanel::new(),
            dynamic,
            registry: None,
            registry_error: None,
            registry_refreshing: false,
            installing: Default::default(),
        }
    }

    pub fn discover_defaults(&mut self) {
        let plugin_roots: Vec<PathBuf> = vec![
            PathBuf::from("plugins"),
            dirs_next::config_dir()
                .map(|d| d.join("HardwareWorkbench").join("plugins"))
                .unwrap_or_else(|| PathBuf::from("plugins")),
        ]
        .into_iter()
        .filter(|p| p.exists())
        .collect();
        if plugin_roots.is_empty() {
            return;
        }
        let _ = self.manager.discover_roots(plugin_roots);
    }

    pub fn summaries(&self) -> Vec<PluginSummary> {
        self.manager.summaries()
    }

    pub fn installed_tab_summary(&self) -> String {
        let s = self.summaries();
        let running = s.iter().filter(|x| x.state == tool_extension::PluginState::Running).count();
        format!("{} 个插件，{} 运行中", s.len(), running)
    }

    pub fn dynamic_ids(&self) -> Vec<String> {
        self.dynamic.ids().map(|s| s.to_owned()).collect()
    }

    pub fn find_market_plugin(&self, id: &str) -> Option<RegistryPlugin> {
        self.registry.as_ref().and_then(|r| r.plugins.iter().find(|p| p.id == id).cloned())
    }
}
