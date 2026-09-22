use thiserror::Error;
use tool_lua_host::LuaHostError;

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

    #[error("permission '{permission}' is not allowed for plugin '{plugin_id}'")]
    PermissionDenied {
        plugin_id: String,
        permission: String,
    },

    #[error("lua error: {0}")]
    Lua(#[from] tool_lua_host::LuaHostError),
}

pub type ExtensionResult<T> = Result<T, ExtensionError>;

/// 把插件错误翻译成用户可见的中文提示。
///
/// 与 `tool_platform::translate_error` / `tool_transport::translate_error` 同一形状：
/// 一个错误枚举配一个纯函数映射器，按**变体**分发而不是匹配英文文案 —— 字符串匹配在
/// 第 21 轮刚刚害过一次（`tiles.rs` 用 `contains("shutting down")` 判重启交接，判定
/// 一被翻译就静默变永假）。调用方应在错误产生时立即翻译，别先把 `Display` 存成字符串。
///
/// 匹配**刻意不写 `_ =>` 分支**：新增变体而忘了配中文，这里就编译不过，而不是悄悄把
/// 英文句子漏给用户。`tests/error_wording.rs` 的 `variant_name` 同样不写 `_`，两处
/// 一起构成「全部变体都有专属文案」的机器形式。
///
/// `Io` / `Manifest` / `Lua(LuaHostError::Lua)` 走的是「中文框架 + 外来细节」：OS 与
/// mlua 的原文留着才有得排查（`tool_transport` 的 `Io` 臂同一取舍，如
/// `操作超时：read timed out`），这里只保证用户不必先读一整句英文才看到线索。
///
/// 各变体到用户眼前的实际路径（决定哪句文案真的会被读到）：
/// - `NotFound` / `AlreadyEnabled` / `Stopping` 由 `Workbench` 的 Enable/Disable 派发带出。
/// - `Io` 有三条活路：插件目录缺 `main.lua` 时的启用失败，`scan_roots` 的 `read_dir` 失败
///   经 `discover_plugins` / `reload_plugins` 的任务消息到达状态栏，以及第 24 轮起
///   `plugin.json` 读不出来时的那条 `manifest_parse_error` 诊断（此前它被内插成
///   「解析失败」，是一句谎报）。
/// - `UnsupportedRuntime` 的分支是活的，但输入进不来：`scan_roots` 先跑
///   `manifest.validate()`，而 `spec::RUNTIMES` 只有 `lua`，非法 runtime 在登记成
///   record 之前就被折成中文诊断了。留文案是因为这条判定仍然写在那里。
/// - `Manifest` 到用户眼前只有一条路：`scan_roots` 把它折成 `manifest_parse_error` 诊断，
///   第 24 轮起那条诊断的正文正是本函数的返回值（此前它内插 `Display`，用户读到的是一句
///   套在中文框架里的英文）。派发路径上它逃不出来，因为 `load_manifest` 只被那里调。
/// - `PermissionDenied` 同理：它只出现在 `permission_denied` 诊断里，那条也从第 24 轮起
///   调本函数（此前是裸 `to_string()`，整句英文直接渲染进面板）。
/// - `Lua` 目前只能由 `enable_lua` 里那句 `run_plugin(..)?` 产生，而 `run_plugin` 现在
///   一律返回 `Ok`（脚本错误走运行时状态与诊断），所以这一臂还没有活的用户路径。
pub fn translate_error(err: &ExtensionError) -> String {
    match err {
        ExtensionError::Io(error) => format!("插件文件访问失败：{error}"),
        ExtensionError::Manifest(error) => format!("manifest 解析失败：{error}"),
        ExtensionError::NotFound(plugin_id) => {
            format!("未找到插件 {plugin_id}（请先重新扫描插件目录）")
        }
        ExtensionError::AlreadyEnabled(plugin_id) => {
            format!("插件 {plugin_id} 已在运行（无需重复启用）")
        }
        ExtensionError::Stopping(plugin_id) => format!("插件 {plugin_id} 正在停止，请稍候再启用"),
        ExtensionError::UnsupportedRuntime(runtime) => {
            format!("不支持的插件运行时 {runtime}（请检查 plugin.json 的 runtime 字段）")
        }
        // 与 `tool_transport` 那句「浏览器拒绝了串口授权请求」是两件事：这里是插件
        // 在 plugin.json 里声明了宿主白名单外的权限，跟用户授权无关，不能共用一句话。
        ExtensionError::PermissionDenied {
            plugin_id,
            permission,
        } => format!(
            "插件 {plugin_id} 声明了宿主不允许的权限 {permission}（请检查 plugin.json 的 permissions 字段）"
        ),
        ExtensionError::Lua(error) => match error {
            LuaHostError::AlreadyRunning => "插件脚本已在运行（请等待上一次启用结束）".into(),
            // 内层单独取 mlua 的原文：整只 `LuaHostError` 的 `Display` 自带
            // 「lua error: 」，套进中文框架就成了「插件脚本错误：lua error: …」那种双重框架。
            LuaHostError::Lua(detail) => format!("插件脚本错误：{detail}"),
        },
    }
}

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
        let value = read_json(&schema_path);

        assert_eq!(
            value.get("title").and_then(serde_json::Value::as_str),
            Some("Hardware Workbench plugin.json")
        );
    }

    #[test]
    fn lua_authoring_support_files_exist() {
        let root = repo_root();
        let luarc_path = root.join(".luarc.json");
        let luarc = read_json(&luarc_path);
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
            let text = read_text(&manifest_path);
            let manifest: PluginManifest = serde_json::from_str(&text)
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
        // 完全没有 `live` 配置时，subscriptions 视为空
        let without_live: PluginManifest = serde_json::from_str(
            r#"{
              "id": "t",
              "name": "T",
              "version": "1",
              "runtime": "lua",
              "main": "m.lua",
              "permissions": []
            }"#,
        )
        .unwrap();
        assert!(without_live.live_subscriptions().is_empty());
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

    /// 读文件失败时把路径带进 panic 信息，否则看不出是哪个插件/文件坏掉。
    fn read_text(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn read_json(path: &Path) -> serde_json::Value {
        let text = read_text(path);
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn assert_manifest_is_valid(path: &Path) {
        let value = read_json(path);
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
