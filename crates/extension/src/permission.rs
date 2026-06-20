//! 插件权限管理：声明式白名单与运行时校验。

use crate::manifest::PluginManifest;
use crate::{ExtensionError, ExtensionResult};
use std::collections::BTreeSet;

/// 权限管理器：维护一组允许权限，校验插件清单。
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
        // 检查 live 权限
        for permission in manifest.live_permissions() {
            if !self.allowed.contains(permission) {
                return Err(ExtensionError::PermissionDenied {
                    plugin_id: manifest.id.clone(),
                    permission: permission.clone(),
                });
            }
        }

        // 检查 replay 权限（只允许 log / storage）
        const REPLAY_ALLOWED: &[&str] = &["log", "storage"];
        for permission in manifest.replay_permissions() {
            if !REPLAY_ALLOWED.contains(&permission.as_str()) {
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
        Self::new([
            "bus",
            "log",
            "serial",
            "ui",
            "storage",
            "timer",
            "testing",
            "dialog",
            "fs.read.user_selected",
            "task",
            "config",
        ])
    }
}
