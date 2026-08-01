use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Map;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tool_core::config::{CURRENT_SCHEMA_VERSION, atomic_write_json, quarantine_corrupt_file};

#[derive(Debug, Serialize, Deserialize)]
struct PluginConfigDocument {
    #[serde(default, alias = "version")]
    schema_version: u32,
    #[serde(default)]
    values: Map<String, serde_json::Value>,
}

/// 跨组件共享的配置持久化存储。
///
/// 每个插件的配置存储在 `{workspace}/plugin-config/{sanitized_plugin_id}.json`。
/// 写入使用临时文件 + rename 保证原子性。
#[derive(Debug)]
pub struct ConfigStore {
    root: PathBuf,
    /// 内存缓存，避免重复读盘
    cache: Mutex<HashMap<String, serde_json::Value>>,
    /// 高于当前版本的配置只读，避免旧程序覆盖新程序写入的数据。
    unsupported_versions: Mutex<HashSet<String>>,
}

impl ConfigStore {
    pub fn new(root: PathBuf) -> Self {
        // 确保目录存在
        let _ = fs::create_dir_all(&root);
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
            unsupported_versions: Mutex::new(HashSet::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn config_path(&self, plugin_id: &str) -> PathBuf {
        let safe_id = sanitize_plugin_id(plugin_id);
        self.root.join(format!("{safe_id}.json"))
    }

    fn ensure_loaded(&self, plugin_id: &str) {
        if self.cache.lock().contains_key(plugin_id) {
            return;
        }
        let path = self.config_path(plugin_id);
        let (data, migrated, unsupported_version) = self.read_document(&path);
        self.cache
            .lock()
            .insert(plugin_id.to_owned(), serde_json::Value::Object(data));
        if unsupported_version {
            self.unsupported_versions
                .lock()
                .insert(plugin_id.to_owned());
            return;
        }
        if migrated && let Err(error) = self.write(plugin_id) {
            log::warn!("plugin config migration for {plugin_id} could not be persisted: {error}");
        }
    }

    fn read_document(&self, path: &Path) -> (Map<String, serde_json::Value>, bool, bool) {
        let Ok(source) = fs::read_to_string(path) else {
            return (Map::new(), false, false);
        };
        let value: serde_json::Value = match serde_json::from_str(&source) {
            Ok(value) => value,
            Err(error) => {
                let backup = quarantine_corrupt_file(path).ok().flatten();
                log::warn!(
                    "plugin config {} is invalid: {error}; backup: {}",
                    path.display(),
                    backup.as_ref().map_or_else(
                        || "unavailable".to_owned(),
                        |path| path.display().to_string()
                    )
                );
                return (Map::new(), false, false);
            }
        };
        let Some(object) = value.as_object() else {
            let backup = quarantine_corrupt_file(path).ok().flatten();
            log::warn!(
                "plugin config {} root must be an object; backup: {}",
                path.display(),
                backup.as_ref().map_or_else(
                    || "unavailable".to_owned(),
                    |path| path.display().to_string()
                )
            );
            return (Map::new(), false, false);
        };

        if object.contains_key("schema_version") || object.contains_key("version") {
            let version = object
                .get("schema_version")
                .or_else(|| object.get("version"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u32;
            if version > CURRENT_SCHEMA_VERSION {
                log::error!(
                    "plugin config {} uses unsupported future schema v{version}",
                    path.display()
                );
                return (Map::new(), false, true);
            }
            let values = object
                .get("values")
                .and_then(serde_json::Value::as_object)
                .cloned()
                .unwrap_or_default();
            return (values, version != CURRENT_SCHEMA_VERSION, false);
        }

        // v0: 插件键直接位于根对象，读取后立即保存为带 schema_version 的文档。
        (object.clone(), true, false)
    }

    fn write(&self, plugin_id: &str) -> std::io::Result<()> {
        if self.unsupported_versions.lock().contains(plugin_id) {
            return Err(std::io::Error::other(
                "插件配置版本高于当前程序支持范围，已按只读方式打开",
            ));
        }
        let data = {
            let cache = self.cache.lock();
            cache.get(plugin_id).cloned()
        };
        let Some(data) = data else {
            return Ok(());
        };
        let path = self.config_path(plugin_id);
        let parent = path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;

        let values = data.as_object().cloned().unwrap_or_default();
        let document = PluginConfigDocument {
            schema_version: CURRENT_SCHEMA_VERSION,
            values,
        };
        atomic_write_json(&path, &document).map_err(std::io::Error::other)
    }

    fn ensure_writable(&self, plugin_id: &str) -> std::io::Result<()> {
        if self.unsupported_versions.lock().contains(plugin_id) {
            return Err(std::io::Error::other(
                "插件配置版本高于当前程序支持范围，已按只读方式打开",
            ));
        }
        Ok(())
    }

    // ── public API ──

    pub fn get(&self, plugin_id: &str, key: &str, default: serde_json::Value) -> serde_json::Value {
        self.ensure_loaded(plugin_id);
        let cache = self.cache.lock();
        cache
            .get(plugin_id)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(key))
            .cloned()
            .unwrap_or(default)
    }

    pub fn set(&self, plugin_id: &str, key: &str, value: serde_json::Value) -> std::io::Result<()> {
        self.ensure_loaded(plugin_id);
        self.ensure_writable(plugin_id)?;
        {
            let mut cache = self.cache.lock();
            if let Some(obj) = cache.get_mut(plugin_id).and_then(|v| v.as_object_mut()) {
                obj.insert(key.to_owned(), value);
            }
        }
        self.write(plugin_id)
    }

    pub fn remove(&self, plugin_id: &str, key: &str) -> std::io::Result<()> {
        self.ensure_loaded(plugin_id);
        self.ensure_writable(plugin_id)?;
        {
            let mut cache = self.cache.lock();
            if let Some(obj) = cache.get_mut(plugin_id).and_then(|v| v.as_object_mut()) {
                obj.remove(key);
            }
        }
        self.write(plugin_id)
    }

    pub fn keys(&self, plugin_id: &str) -> Vec<String> {
        self.ensure_loaded(plugin_id);
        let cache = self.cache.lock();
        cache
            .get(plugin_id)
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    }

    // ── profile API ──

    /// 返回所有 profile 名称列表（不含系统预留键如 \$profiles）。
    pub fn profile_list(&self, plugin_id: &str) -> Vec<String> {
        self.ensure_loaded(plugin_id);
        let cache = self.cache.lock();
        cache
            .get(plugin_id)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("$profiles"))
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 加载指定 profile，返回 table 或 nil。
    pub fn profile_load(&self, plugin_id: &str, name: &str) -> Option<serde_json::Value> {
        self.ensure_loaded(plugin_id);
        let cache = self.cache.lock();
        cache
            .get(plugin_id)
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("$profiles"))
            .and_then(|v| v.as_object())
            .and_then(|o| o.get(name))
            .cloned()
    }

    /// 保存 profile。
    pub fn profile_save(
        &self,
        plugin_id: &str,
        name: &str,
        data: serde_json::Value,
    ) -> std::io::Result<()> {
        self.ensure_loaded(plugin_id);
        self.ensure_writable(plugin_id)?;
        {
            let mut cache = self.cache.lock();
            if let Some(obj) = cache.get_mut(plugin_id).and_then(|v| v.as_object_mut()) {
                let profiles = obj
                    .entry("$profiles".to_owned())
                    .or_insert_with(|| serde_json::Value::Object(Map::new()));
                if let Some(profiles_obj) = profiles.as_object_mut() {
                    profiles_obj.insert(name.to_owned(), data);
                }
            }
        }
        self.write(plugin_id)
    }

    /// 删除 profile。
    pub fn profile_delete(&self, plugin_id: &str, name: &str) -> std::io::Result<()> {
        self.ensure_loaded(plugin_id);
        self.ensure_writable(plugin_id)?;
        {
            let mut cache = self.cache.lock();
            if let Some(obj) = cache.get_mut(plugin_id).and_then(|v| v.as_object_mut())
                && let Some(profiles) = obj.get_mut("$profiles").and_then(|v| v.as_object_mut())
            {
                profiles.remove(name);
            }
        }
        self.write(plugin_id)
    }
}

/// 将 plugin_id 中的非法文件名字符替换为 `_`。
fn sanitize_plugin_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_get_set_keys_roundtrip() {
        let tmp = std::env::temp_dir().join(format!(
            "hw-config-test-{}-{}",
            tool_core::now_timestamp_ms(),
            line!()
        ));
        let store = ConfigStore::new(tmp.clone());
        let pid = "demo.test";

        // 初始 get 返回 default
        assert_eq!(
            store.get(pid, "baud", serde_json::json!(9600)),
            serde_json::json!(9600)
        );

        // set 后 get 返回新值
        store.set(pid, "baud", serde_json::json!(115200)).unwrap();
        assert_eq!(
            store.get(pid, "baud", serde_json::json!(9600)),
            serde_json::json!(115200)
        );

        // keys
        let keys = store.keys(pid);
        assert!(keys.contains(&"baud".to_owned()));

        // remove
        store.remove(pid, "baud").unwrap();
        assert_eq!(
            store.get(pid, "baud", serde_json::json!(9600)),
            serde_json::json!(9600)
        );

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn profile_save_load_delete() {
        let tmp = std::env::temp_dir().join(format!(
            "hw-config-test-{}-{}",
            tool_core::now_timestamp_ms(),
            line!()
        ));
        let store = ConfigStore::new(tmp.clone());
        let pid = "demo.test";

        store
            .profile_save(
                pid,
                "default",
                serde_json::json!({"speed": 1000, "accel": 500}),
            )
            .unwrap();
        let list = store.profile_list(pid);
        assert!(list.contains(&"default".to_owned()));

        let loaded = store.profile_load(pid, "default");
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap()["speed"], 1000);

        store.profile_delete(pid, "default").unwrap();
        assert!(!store.profile_list(pid).contains(&"default".to_owned()));

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn sanitize_plugin_id_replaces_special_chars() {
        assert_eq!(sanitize_plugin_id("demo.test"), "demo.test");
        assert_eq!(sanitize_plugin_id("demo/test:1"), "demo_test_1");
        assert_eq!(sanitize_plugin_id("my plugin@v2"), "my_plugin_v2");
    }

    #[test]
    fn persistence_survives_reload() {
        let tmp = std::env::temp_dir().join(format!(
            "hw-config-test-{}-{}",
            tool_core::now_timestamp_ms(),
            line!()
        ));
        let pid = "persist.test";

        {
            let store = ConfigStore::new(tmp.clone());
            store.set(pid, "x", serde_json::json!(42)).unwrap();
        }

        {
            let store = ConfigStore::new(tmp.clone());
            assert_eq!(
                store.get(pid, "x", serde_json::json!(0)),
                serde_json::json!(42)
            );
        }

        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn legacy_plugin_config_is_migrated_to_versioned_document() {
        let tmp = std::env::temp_dir().join(format!(
            "hw-config-migration-test-{}-{}",
            tool_core::now_timestamp_ms(),
            line!()
        ));
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("demo.test.json"), r#"{"baud":115200}"#).unwrap();

        let store = ConfigStore::new(tmp.clone());
        assert_eq!(store.get("demo.test", "baud", serde_json::json!(0)), 115200);

        let document: PluginConfigDocument =
            serde_json::from_str(&fs::read_to_string(tmp.join("demo.test.json")).unwrap()).unwrap();
        assert_eq!(document.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(document.values["baud"], 115200);
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn corrupted_plugin_config_is_quarantined_before_rewrite() {
        let tmp = std::env::temp_dir().join(format!(
            "hw-config-corrupt-test-{}-{}",
            tool_core::now_timestamp_ms(),
            line!()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("demo.test.json");
        fs::write(&path, "not json").unwrap();

        let store = ConfigStore::new(tmp.clone());
        assert_eq!(store.get("demo.test", "x", serde_json::json!(7)), 7);
        assert!(!path.exists());
        assert!(fs::read_dir(&tmp).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("demo.test.json.corrupt-")
        }));

        store.set("demo.test", "x", serde_json::json!(9)).unwrap();
        assert!(path.exists());
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn future_plugin_config_is_never_overwritten() {
        let tmp = std::env::temp_dir().join(format!(
            "hw-config-future-test-{}-{}",
            tool_core::now_timestamp_ms(),
            line!()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("demo.test.json");
        let source = r#"{"schema_version":99,"values":{"x":1}}"#;
        fs::write(&path, source).unwrap();

        let store = ConfigStore::new(tmp.clone());
        assert_eq!(store.get("demo.test", "x", serde_json::json!(0)), 0);
        assert!(store.set("demo.test", "x", serde_json::json!(2)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
        let _ = fs::remove_dir_all(tmp);
    }
}
