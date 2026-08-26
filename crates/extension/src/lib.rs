use thiserror::Error;

pub mod host_services;
pub mod manager;
pub mod manifest;
pub mod permission;
pub mod scenario;
pub mod spec;

// Re-export 核心类型，供外部 crate 直接通过 tool_extension:: 访问
pub use manager::{PluginManager, PluginScan, PluginScanCandidate};
pub use manifest::{PluginDiagnostic, PluginDiagnosticSeverity, PluginState, PluginSummary};
pub use permission::PermissionManager;
pub use scenario::SerialPluginScenarioRunner;

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

    #[error("plugin '{0}' is still shutting down")]
    Stopping(String),

    #[error("unsupported runtime '{0}'")]
    UnsupportedRuntime(String),

    #[error(
        "unsupported plugin api_version '{api_version}' for plugin '{plugin_id}' (supported: {supported})"
    )]
    UnsupportedApiVersion {
        plugin_id: String,
        api_version: String,
        supported: String,
    },

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
    use std::fs;
    use std::path::{Path, PathBuf};

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
        assert_eq!(
            manifest.api_version,
            crate::manifest::CURRENT_PLUGIN_API_VERSION
        );
        assert!(manifest.api_version_supported());
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
    fn manifest_validate_rejects_missing_ui_command() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "runtime": "lua",
          "main": "main.lua",
          "permissions": ["bus"],
          "contributes": {
            "ui": [
              {
                "id": "demo.test.run.button",
                "slot": "send.toolbar",
                "command": "demo.test.missing"
              }
            ]
          }
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        let errors = manifest.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("demo.test.missing"))
        );
    }

    #[test]
    fn plugin_schema_is_valid_json() {
        let schema_path = repo_root().join("plugins").join("plugin.schema.json");
        let text = fs::read_to_string(&schema_path)
            .unwrap_or_else(|error| panic!("{}: {error}", schema_path.display()));
        let value: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: {error}", schema_path.display()));

        assert_eq!(
            value.get("title").and_then(serde_json::Value::as_str),
            Some("Hardware Workbench plugin.json")
        );
    }

    #[test]
    fn lua_authoring_support_files_exist() {
        let root = repo_root();
        let luarc_path = root.join(".luarc.json");
        let luarc_text = fs::read_to_string(&luarc_path)
            .unwrap_or_else(|error| panic!("{}: {error}", luarc_path.display()));
        let luarc: serde_json::Value = serde_json::from_str(&luarc_text)
            .unwrap_or_else(|error| panic!("{}: {error}", luarc_path.display()));
        assert_eq!(
            luarc
                .get("runtime.version")
                .and_then(serde_json::Value::as_str),
            Some("Lua 5.4")
        );

        for relative_path in [
            "plugins/.lua/hardware-workbench.lua",
            "plugins/.lua/hw/codec.lua",
            "plugins/.lua/hw/utils.lua",
        ] {
            let path = root.join(relative_path);
            assert!(path.exists(), "{} should exist", path.display());
        }
    }

    #[test]
    fn bundled_plugin_manifests_are_valid() {
        let plugins_root = repo_root().join("plugins");
        let mut checked = 0;

        for entry in fs::read_dir(&plugins_root)
            .unwrap_or_else(|error| panic!("{}: {error}", plugins_root.display()))
        {
            let path = entry.unwrap().path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("plugin.json");
            if !manifest_path.exists() {
                continue;
            }

            assert_manifest_is_valid(&manifest_path);
            let manifest: PluginManifest = serde_json::from_str(
                &fs::read_to_string(&manifest_path)
                    .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display())),
            )
            .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));
            let live_path = path.join(manifest.live_main());
            assert!(
                live_path.is_file(),
                "{} declares live entry '{}' but it is missing",
                manifest_path.display(),
                manifest.live_main()
            );
            if let Some(replay) = manifest.replay_main() {
                let replay_path = path.join(replay);
                assert!(
                    replay_path.is_file(),
                    "{} declares replay entry '{}' but it is missing",
                    manifest_path.display(),
                    replay
                );
            }
            checked += 1;
        }

        assert!(
            checked >= 3,
            "expected bundled plugin manifests under {}",
            plugins_root.display()
        );
    }

    #[test]
    fn new_manifest_with_live_and_replay() {
        let json = r#"{
          "id": "demo.test",
          "name": "Test",
          "version": "1.0.0",
          "api_version": "0.1",
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
        assert_eq!(manifest.api_version, "0.1");
        assert!(manifest.api_version_supported());
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
            api_version: crate::manifest::CURRENT_PLUGIN_API_VERSION.to_owned(),
            runtime: "lua".to_owned(),
            main: "main.lua".to_owned(),
            permissions: vec!["filesystem".to_owned()],
            contributes: PluginContributes::default(),
            live: None,
            replay: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            category: None,
            icon: None,
        };

        assert!(PermissionManager::default().check(&manifest).is_err());
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    fn assert_manifest_is_valid(path: &Path) {
        let text =
            fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let value: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert_eq!(
            value.get("$schema").and_then(serde_json::Value::as_str),
            Some("../plugin.schema.json"),
            "{} should point at the local plugin schema",
            path.display()
        );

        let manifest: PluginManifest = serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        assert!(
            manifest.api_version_supported(),
            "{} uses unsupported api_version '{}'",
            path.display(),
            manifest.api_version
        );
        if let Err(errors) = manifest.validate() {
            panic!("{}: {}", path.display(), errors.join("; "));
        }
        PermissionManager::default()
            .check(&manifest)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}
