use thiserror::Error;

pub mod host_services;
pub mod manager;
pub mod manifest;
pub mod permission;

// Re-export 核心类型，供外部 crate 直接通过 tool_extension:: 访问
pub use manager::PluginManager;
pub use manifest::{PluginState, PluginSummary};
pub use permission::PermissionManager;

// topic_matches 已移至 tool_core，此处保持向后兼容 re-export
pub use tool_core::topic_matches;

#[derive(Debug, Error)]
pub enum ExtensionError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("manifest parse error: {0}")]
    Manifest(#[from] serde_json::Error),

    #[error("plugin '{0}' was not found")]
    NotFound(String),

    #[error("plugin '{0}' is already enabled")]
    AlreadyEnabled(String),

    #[error("unsupported runtime '{0}'")]
    UnsupportedRuntime(String),

    #[error("permission '{permission}' is not allowed for plugin '{plugin_id}'")]
    PermissionDenied {
        plugin_id: String,
        permission: String,
    },

    #[error("lua error: {0}")]
    Lua(#[from] tool_lua_host::LuaHostError),
}

pub type ExtensionResult<T> = Result<T, ExtensionError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PluginContributes, PluginManifest};

    #[test]
    fn old_manifest_without_live_replay_is_compatible() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus", "log", "ui"]
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.live_main(), "main.lua");
        assert_eq!(manifest.live_permissions().len(), 3);
        assert!(manifest.contributes.ui.is_empty());
        assert!(!manifest.has_replay_analyzer());
        assert!(manifest.replay_main().is_none());
    }

    #[test]
    fn manifest_parses_ui_contributions() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus"],
          "contributes": {
            "commands": [
              { "id": "demo.test.run", "title": "Run" }
            ],
            "ui": [
              {
                "id": "demo.test.run.button",
                "slot": "send.toolbar",
                "command": "demo.test.run",
                "title": "Run",
                "tooltip": "Run from the send toolbar",
                "order": 20
              }
            ]
          }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.contributes.ui.len(), 1);
        let item = &manifest.contributes.ui[0];
        assert_eq!(item.slot, "send.toolbar");
        assert_eq!(item.kind, "button");
        assert_eq!(item.command.as_deref(), Some("demo.test.run"));
        assert!(item.enabled);
        assert!(item.visible);
        assert!(!item.record_send_input);
    }

    #[test]
    fn new_manifest_with_live_and_replay() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus"],
          "live": {
            "main": "live.lua",
            "permissions": ["bus", "log", "serial", "ui"]
          },
          "replay": {
            "main": "replay.lua",
            "subscriptions": ["transport.serial.default.rx"],
            "outputs": ["protocol.demo.sample"],
            "permissions": ["log", "storage"]
          }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.live_main(), "live.lua");
        assert_eq!(manifest.live_permissions().len(), 4);
        assert!(manifest.has_replay_analyzer());
        assert_eq!(manifest.replay_main(), Some("replay.lua"));
        assert_eq!(manifest.replay_subscriptions().len(), 1);
        assert_eq!(manifest.replay_outputs().len(), 1);
        assert_eq!(manifest.replay_permissions().len(), 2);
    }

    #[test]
    fn manifest_parses_live_subscriptions() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus"],
          "live": {
            "main": "live.lua",
            "permissions": ["bus"],
            "subscriptions": ["transport.serial.default.rx"]
          }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(
            manifest.live_subscriptions(),
            &["transport.serial.default.rx".to_owned()]
        );
        // 不填 subscriptions 时返回空
        let manifest2: PluginManifest =
            serde_json::from_str(r#"{"id":"t","name":"T","version":"1","runtime":"lua","main":"m.lua","permissions":[]}"#)
                .unwrap();
        assert!(manifest2.live_subscriptions().is_empty());
    }

    #[test]
    fn rejects_unknown_permission() {
        let manifest = PluginManifest {
            id: "bad".to_owned(),
            name: "Bad".to_owned(),
            version: "0.1.0".to_owned(),
            runtime: "lua".to_owned(),
            main: "main.lua".to_owned(),
            permissions: vec!["filesystem".to_owned()],
            contributes: PluginContributes::default(),
            live: None,
            replay: None,
        };

        assert!(PermissionManager::default().check(&manifest).is_err());
    }
}
