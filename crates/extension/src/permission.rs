//! 插件权限管理：声明式白名单与运行时校验。

use crate::manifest::PluginManifest;
use crate::{ExtensionError, ExtensionResult};
use std::collections::BTreeSet;

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

        Ok(())
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new(crate::spec::LIVE_PERMISSIONS.iter().copied())
    }
}
