//! 插件权限管理：声明式白名单与运行时校验。

use crate::manifest::PluginManifest;
use crate::{ExtensionError, ExtensionResult};
use std::collections::BTreeSet;
use tool_plugin_api::{PluginCapability, PluginPermissions};

/// 权限管理器：维护一组允许权限，校验插件清单。未列入白名单的权限一律拒绝。
#[derive(Debug, Clone)]
pub struct PermissionManager {
    allowed: BTreeSet<String>,
}

impl PermissionManager {
    pub fn new(allowed: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: allowed.into_iter().map(Into::into).collect(),
        }
    }

    pub fn check(&self, manifest: &PluginManifest) -> ExtensionResult<()> {
        for permission in manifest.live_permissions() {
            if !self.allowed.contains(permission) {
                return Err(ExtensionError::PermissionDenied {
                    plugin_id: manifest.id.clone(),
                    permission: permission.clone(),
                });
            }
        }

        // replay 侧固定走协议白名单（log / storage），不受宿主 allowed 集合放宽影响。
        for permission in manifest.replay_permissions() {
            if !crate::spec::REPLAY_PERMISSIONS.contains(&permission.as_str()) {
                return Err(ExtensionError::PermissionDenied {
                    plugin_id: manifest.id.clone(),
                    permission: permission.clone(),
                });
            }
        }

        Ok(())
    }

    /// Resolve manifest permissions against a platform capability set. This
    /// is intentionally separate from `check`: a permission can be valid in
    /// the plugin specification while the current platform still cannot
    /// provide it (for example `process` in a browser).
    pub fn missing_capabilities(
        &self,
        manifest: &PluginManifest,
        available: impl IntoIterator<Item = PluginCapability>,
    ) -> Vec<PluginCapability> {
        let requested: PluginPermissions = manifest.required_capabilities();
        requested.missing_from(available)
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new(crate::spec::LIVE_PERMISSIONS.iter().copied())
    }
}
