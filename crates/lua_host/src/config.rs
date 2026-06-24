use parking_lot::Mutex;
use serde_json::Map;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 跨组件共享的配置持久化存储。
///
/// 每个插件的配置存储在 `{workspace}/plugin-config/{sanitized_plugin_id}.json`。
/// 写入使用临时文件 + rename 保证原子性。
#[derive(Debug)]
pub struct ConfigStore {
    root: PathBuf,
    /// 内存缓存，避免重复读盘
    cache: Mutex<HashMap<String, serde_json::Value>>,
}

impl ConfigStore {
    pub fn new(root: PathBuf) -> Self {
        // 确保目录存在
        let _ = fs::create_dir_all(&root);
        Self {
            root,
            cache: Mutex::new(HashMap::new()),
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
        let mut cache = self.cache.lock();
        if cache.contains_key(plugin_id) {
            return;
        }
        let path = self.config_path(plugin_id);
        let data = if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default()
        } else {
            Map::new()
        };
        cache.insert(plugin_id.to_owned(), serde_json::Value::Object(data));
    }

    fn write(&self, plugin_id: &str) -> std::io::Result<()> {
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

        let tmp = path.with_extension("tmp");
        let content = serde_json::to_string_pretty(&data).unwrap_or_else(|_| "{}".to_owned());
        fs::write(&tmp, &content)?;
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
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
}
