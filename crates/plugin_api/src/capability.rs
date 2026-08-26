use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Capabilities declared by `plugin.json` and resolved against a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    Bus,
    Config,
    Dialog,
    Filesystem,
    Log,
    Process,
    Serial,
    Storage,
    Task,
    Timer,
    Testing,
    Ui,
}

impl PluginCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bus => "bus",
            Self::Config => "config",
            Self::Dialog => "dialog",
            Self::Filesystem => "filesystem",
            Self::Log => "log",
            Self::Process => "process",
            Self::Serial => "serial",
            Self::Storage => "storage",
            Self::Task => "task",
            Self::Timer => "timer",
            Self::Testing => "testing",
            Self::Ui => "ui",
        }
    }

    /// Map the permission spelling used by `plugin.json` to the stable
    /// capability vocabulary. Compatibility aliases stay here instead of
    /// leaking into each engine or platform implementation.
    pub fn from_permission(value: &str) -> Option<Self> {
        Some(match value {
            "bus" => Self::Bus,
            "config" => Self::Config,
            "dialog" => Self::Dialog,
            "fs.read.user_selected" => Self::Filesystem,
            "log" => Self::Log,
            "process" => Self::Process,
            "serial" => Self::Serial,
            "storage" => Self::Storage,
            "task" => Self::Task,
            "timer" => Self::Timer,
            "testing" => Self::Testing,
            "ui" => Self::Ui,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginPermissions {
    requested: BTreeSet<PluginCapability>,
}

impl PluginPermissions {
    pub fn new(capabilities: impl IntoIterator<Item = PluginCapability>) -> Self {
        Self {
            requested: capabilities.into_iter().collect(),
        }
    }

    pub fn contains(&self, capability: PluginCapability) -> bool {
        self.requested.contains(&capability)
    }

    pub fn from_permission_names(permissions: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        Self::new(
            permissions
                .into_iter()
                .filter_map(|permission| PluginCapability::from_permission(permission.as_ref())),
        )
    }

    pub fn iter(&self) -> impl Iterator<Item = PluginCapability> + '_ {
        self.requested.iter().copied()
    }

    pub fn missing_from(
        &self,
        available: impl IntoIterator<Item = PluginCapability>,
    ) -> Vec<PluginCapability> {
        let available = available.into_iter().collect::<BTreeSet<_>>();
        self.requested
            .iter()
            .copied()
            .filter(|capability| !available.contains(capability))
            .collect()
    }
}
