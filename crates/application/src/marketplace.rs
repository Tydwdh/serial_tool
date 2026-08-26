//! Platform-neutral marketplace presentation DTOs.
//!
//! Download and installation are Application capabilities. The panel only
//! receives a stable registry/status view and never owns their task handles.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplacePluginView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub category: Option<String>,
    pub icon: Option<String>,
    pub permissions: Vec<String>,
    pub size: u64,
    pub published: Option<String>,
    pub manifest_url: Option<String>,
    pub main_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketplaceView {
    pub version: u32,
    pub updated: String,
    pub plugins: Vec<MarketplacePluginView>,
}

/// Current marketplace capability state owned by Application.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketplaceStatusView {
    pub registry: Option<MarketplaceView>,
    pub refreshing: bool,
    pub error: Option<String>,
    pub installing: Vec<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tool_marketplace::RegistryPlugin> for MarketplacePluginView {
    fn from(plugin: tool_marketplace::RegistryPlugin) -> Self {
        Self {
            id: plugin.id,
            name: plugin.name,
            version: plugin.version,
            api_version: plugin.api_version,
            description: plugin.description,
            author: plugin.author,
            homepage: plugin.homepage,
            repository: plugin.repository,
            license: plugin.license,
            category: plugin.category,
            icon: plugin.icon,
            permissions: plugin.permissions,
            size: plugin.size,
            published: plugin.published,
            manifest_url: None,
            main_url: None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tool_marketplace::Registry> for MarketplaceView {
    fn from(registry: tool_marketplace::Registry) -> Self {
        Self {
            version: registry.version,
            updated: registry.updated,
            plugins: registry.plugins.into_iter().map(Into::into).collect(),
        }
    }
}
