use crate::app::WorkbenchApp;
use crate::bootstrap::{user_data_dir, user_logs_dir};
use crate::state::LineEnding;
use crate::state::MAX_SEND_HISTORY;
use std::path::{Path, PathBuf};
use tool_application::query::NetworkPortConfig;
#[cfg(test)]
use tool_core::config::atomic_write_json;
use tool_core::{
    config::{CURRENT_SCHEMA_VERSION, quarantine_corrupt_file},
    now_timestamp_ms,
};
use tool_platform::storage::native::NativeSettingsStore;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tool_panels::PanelManager;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct PortProfile {
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedConfig {
    /// 配置格式版本。缺失时按 v0 读取并迁移到当前版本。
    #[serde(default, alias = "version")]
    pub(crate) schema_version: u32,
    pub(crate) panels: PanelManager,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) recorder_path: String,

    #[serde(default)]
    pub(crate) enabled_plugins: Vec<String>,
    #[serde(default)]
    pub(crate) port_aliases: HashMap<String, String>,
    #[serde(default)]
    pub(crate) port_groups: HashMap<String, String>,
    #[serde(default)]
    pub(crate) send_history: Vec<String>,
    #[serde(default = "default_line_ending")]
    pub(crate) line_ending: LineEnding,
    #[serde(default)]
    pub(crate) port_profiles: HashMap<String, PortProfile>,
    #[serde(default)]
    pub(crate) recent_workspaces: Vec<String>,
    #[serde(default = "default_true")]
    pub(crate) auto_reconnect: bool,
    /// 可配置快捷键映射（默认 VSCode 风格）。
    #[serde(default)]
    pub(crate) keymap: crate::keymap::Keymap,
    /// 等宽字体大小（终端/日志区），默认 13.0。
    #[serde(default = "default_monospace_font_size")]
    pub(crate) monospace_font_size: f32,
    /// 旧版主题标识，仅用于迁移没有 `theme_path` 的配置。
    #[serde(default, skip_serializing)]
    pub(crate) ui_theme: tool_panels::theme::AppTheme,
    /// 当前主题 JSON 路径。内置和用户新增主题都走同一字段。
    #[serde(default, alias = "custom_theme_path")]
    pub(crate) theme_path: Option<String>,
    /// 终端展示块空闲结束阈值（ms）。仅用于展示块封存，不代表协议帧。默认 5。
    #[serde(default = "default_terminal_merge_window_ms")]
    pub(crate) terminal_merge_window_ms: u64,
    /// 终端保留条数上限。默认 2000。
    #[serde(default = "default_terminal_max_entries")]
    pub(crate) terminal_max_entries: usize,
    /// 日志保留条数上限。默认 2000。
    #[serde(default = "default_log_max_entries")]
    pub(crate) log_max_entries: usize,
    /// 命令面板最近使用顺序（命令 id 列表，最近使用的在前）。
    /// 列表里可能有认不出的值（例如按标题记的历史）：那只是排不到前面，不是脏数据，
    /// 载入时原样保留、不清空。
    #[serde(default)]
    pub(crate) command_usage_order: Vec<String>,
    /// 市场与更新使用的可选 HTTP/SOCKS 代理地址；为空时自动使用系统/环境代理。
    #[serde(default)]
    pub(crate) network_proxy_url: Option<String>,
    /// 网络模拟串口列表（WebSocket + JSON-RPC gcode 桥，Nexus Prime 等 Klipper 服务器）。
    #[serde(default)]
    pub(crate) network_ports: Vec<NetworkPortConfig>,
}

fn default_terminal_merge_window_ms() -> u64 {
    5
}

fn default_terminal_max_entries() -> usize {
    50_000
}

fn default_log_max_entries() -> usize {
    50_000
}

fn default_true() -> bool {
    true
}

fn default_monospace_font_size() -> f32 {
    13.0
}

fn default_line_ending() -> LineEnding {
    LineEnding::None
}

/// 配置加载结果：区分"无配置文件"和"配置损坏"两种情况。
#[derive(Debug)]
// `Ok` 带的是整份 `PersistedConfig`：它只在启动时读一次，随后就被拆给各子系统持有。
// 装箱换来的只是每处 match 多一次解引用，对一次性加载路径没有收益，故保留本豁免。
#[allow(clippy::large_enum_variant)]
pub(crate) enum ConfigLoadResult {
    /// 成功加载
    Ok {
        config: PersistedConfig,
        migrated: bool,
    },
    /// 配置文件不存在（首次运行或配置被删除）
    NotFound,
    /// 配置文件存在但解析失败（配置损坏）
    ParseError {
        path: PathBuf,
        error: String,
        backup_path: Option<PathBuf>,
    },
    /// 配置来自更高版本的程序；为保护数据仅允许本次使用默认值，不写回原文件。
    FutureVersion { path: PathBuf, version: u32 },
}

/// 读出配置声明的 schema 版本，不解析其余字段；解析失败或非对象都按"没有声明"处理。
///
/// 键名规则（`schema_version` 优先、历史键名 `version` 兜底）见
/// `tool_core::config::declared_schema_version`：本函数与下面的
/// `parse_persisted_config` 因此共用同一条规则，不会再各读各的。
fn declared_schema_version(text: &str) -> Option<u32> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let object = value.as_object()?;
    tool_core::config::declared_schema_version(object)
}

fn parse_persisted_config(text: &str) -> Result<(PersistedConfig, bool), String> {
    let mut value: serde_json::Value =
        serde_json::from_str(text).map_err(|error| format!("JSON 解析失败：{error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "配置根节点必须是 JSON 对象".to_owned())?;
    // 未声明版本按 v0 读取并迁移到当前版本。
    let version = tool_core::config::declared_schema_version(object).unwrap_or(0);
    if version > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "配置版本 {version} 高于当前程序支持的 {CURRENT_SCHEMA_VERSION}"
        ));
    }

    let migrated = version != CURRENT_SCHEMA_VERSION;
    if version == 0 {
        if !object.contains_key("theme_path")
            && let Some(legacy_theme_path) = object.get("custom_theme_path").cloned()
        {
            object.insert("theme_path".to_owned(), legacy_theme_path);
        }
        // `theme_path` 已经由迁移写入（或新旧字段同时存在）后，移除旧字段，避免
        // serde 的 alias 将两个键识别为同一字段。
        object.remove("custom_theme_path");
    }
    object.insert(
        "schema_version".to_owned(),
        serde_json::Value::from(CURRENT_SCHEMA_VERSION),
    );
    // 历史键名 `version` 与 `schema_version` 在 `PersistedConfig` 上是同一个字段
    // （`#[serde(default, alias = "version")]`），所以归一化之后必须把旧键删掉：
    // 两个键同时留在 map 里会让 serde 报 `duplicate field`，而 `load_config` 把解析失败
    // 当作"配置损坏"处理 —— 结果是升级后第一次开机就把用户的 workspace.json 隔离掉、
    // 回落到默认设置。`migrate_theme_file` 对同一件事用的是同一手 `remove("version")`。
    object.remove("version");

    let mut config = serde_json::from_value::<PersistedConfig>(value)
        .map_err(|error| format!("配置字段无效：{error}"))?;
    config.schema_version = CURRENT_SCHEMA_VERSION;
    Ok((config, migrated))
}

pub(crate) fn load_config() -> ConfigLoadResult {
    let primary = config_path();

    // 尝试读主路径
    if let Ok(Some(bytes)) = read_native_file(&primary)
        && let Ok(text) = String::from_utf8(bytes)
    {
        if let Some(version) = declared_schema_version(&text)
            && version > CURRENT_SCHEMA_VERSION
        {
            return ConfigLoadResult::FutureVersion {
                path: primary,
                version,
            };
        }
        return match parse_persisted_config(&text) {
            Ok((config, migrated)) => ConfigLoadResult::Ok { config, migrated },
            Err(error) => {
                // 必须先隔离损坏文件：下一行会把 `primary` 移动进返回值。
                let backup_path = quarantine_corrupt_file(&primary).ok().flatten();
                ConfigLoadResult::ParseError {
                    path: primary,
                    error,
                    backup_path,
                }
            }
        };
    }

    // 从旧路径 (CWD/workspace.json) 迁移
    let legacy = std::env::current_dir()
        .ok()
        .map(|dir| dir.join("workspace.json"));
    if let Some(legacy) = legacy.as_ref()
        && let Ok(text) = std::fs::read_to_string(legacy)
        && let Ok((config, _)) = parse_persisted_config(&text)
    {
        let _ = write_native_json(&primary, &config);
        return ConfigLoadResult::Ok {
            config,
            migrated: true,
        };
    }
    ConfigLoadResult::NotFound
}

pub(crate) fn config_path() -> PathBuf {
    // 优先使用平台配置目录，避免 CWD 变化导致配置"丢失"
    if let Some(dir) = dirs_next::config_dir() {
        let app_dir = dir.join("HardwareWorkbench");
        let _ = std::fs::create_dir_all(&app_dir);
        return app_dir.join("workspace.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("workspace.json")
}

fn native_store_key(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
}

/// 配置文件在平台存储层里以"根目录 + 文件名"寻址，两者都取自路径本身。
fn native_store_for(path: &Path) -> Result<(NativeSettingsStore, String), String> {
    let root = path
        .parent()
        .ok_or_else(|| format!("配置路径没有父目录：{}", path.display()))?;
    let key =
        native_store_key(path).ok_or_else(|| format!("无效配置文件名：{}", path.display()))?;
    Ok((NativeSettingsStore::new(root), key))
}

fn read_native_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let (store, key) = native_store_for(path)?;
    // 本函数其余失败正文都是中文（见 `native_store_for`），这里也不能把 `StorageError`
    // 的英文 `Display` 原样摊给用户：设置面板「最近工作区 → 打开」失败时直接渲染这句。
    store
        .load_blocking(key)
        .map_err(|error| tool_platform::storage::translate_error(&error))
}

fn write_native_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| format!("JSON 序列化失败：{error}"))?;
    let (store, key) = native_store_for(path)?;
    // 与 `read_native_file` 同一个正文规则。今天这条到不了用户眼前（`save_config` 的
    // 十个调用点都只 `log::warn!`），统一它是为了同一个失败只有一种说法。
    store
        .save_blocking(key, bytes)
        .map_err(|error| tool_platform::storage::translate_error(&error))
}

/// 主题目录内的文件以相对路径保存，便携包或安装目录改变后仍能恢复主题。
pub(crate) fn resolve_theme_path(theme_dir: &Path, stored_path: &str) -> PathBuf {
    let path = PathBuf::from(stored_path);
    if path.is_absolute() {
        path
    } else {
        theme_dir.join(path)
    }
}

pub(crate) fn windows_open_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("JSONL", &["jsonl"])
        .set_directory("logs")
        .pick_file()
}

pub(crate) fn pick_recorder_path(current: &str) -> Option<PathBuf> {
    let current_path = PathBuf::from(current);

    let mut dialog = rfd::FileDialog::new().add_filter("JSONL", &["jsonl"]);

    if let Some(parent) = current_path.parent()
        && !parent.as_os_str().is_empty()
    {
        dialog = dialog.set_directory(parent);
    } else {
        dialog = dialog.set_directory("logs");
    }

    if let Some(file_name) = current_path.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(file_name);
    } else {
        dialog = dialog.set_file_name(format!("session-{}.jsonl", now_timestamp_ms()));
    }

    dialog.save_file().map(ensure_jsonl_extension)
}

pub(crate) fn ensure_jsonl_extension(mut path: PathBuf) -> PathBuf {
    let is_jsonl = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));

    if !is_jsonl {
        path.set_extension("jsonl");
    }

    path
}

pub(crate) fn default_recorder_path() -> String {
    user_logs_dir()
        .join(format!("session-{}.jsonl", now_timestamp_ms()))
        .display()
        .to_string()
}

/// 将旧配置中的相对录制路径解析到用户可写目录。
///
/// 旧版本保存的是 `logs/session-*.jsonl`。在 Linux `.deb` 安装中，进程
/// 可能从只读的系统目录启动，因此不能再把相对路径交给录制器。
pub(crate) fn resolve_recorder_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        user_data_dir().join(path)
    }
}

// ── WorkbenchApp 配置持久化方法 ──
// 从 commands.rs 迁入，集中配置快照/保存/加载职责。

impl WorkbenchApp {
    /// 构建当前配置的快照
    pub(crate) fn build_config_snapshot(&self) -> PersistedConfig {
        let mut panels = self.panels.clone();
        panels.discard_dynamic_tabs();
        let enabled_plugins: Vec<String> = self
            .workbench
            .query_plugins()
            .summaries
            .into_iter()
            .filter(|summary| {
                matches!(
                    summary.state,
                    tool_application::query::PluginStateView::Enabled
                        | tool_application::query::PluginStateView::Running
                )
            })
            .map(|summary| summary.id)
            .collect();
        let network_proxy_url = self.network_proxy_url.trim();
        PersistedConfig {
            schema_version: CURRENT_SCHEMA_VERSION,
            panels,
            selected_port: self.serial.selected_port.clone(),
            baud_rate: self.serial.baud_rate.clone(),
            data_bits: self.serial.data_bits.clone(),
            stop_bits: self.serial.stop_bits.clone(),
            parity: self.serial.parity.clone(),
            recorder_path: self.recorder_path.clone(),
            enabled_plugins,
            port_aliases: self.serial.port_aliases.clone(),
            port_groups: self.serial.port_groups.clone(),
            send_history: self.send.send_history.iter().cloned().collect(),
            line_ending: self.send.line_ending,
            port_profiles: self.serial.port_profiles.clone(),
            recent_workspaces: self.recent_workspaces.clone(),
            auto_reconnect: self.serial.auto_reconnect,
            keymap: self.keymap.clone(),
            monospace_font_size: self.monospace_font_size,
            ui_theme: self.ui_theme,
            theme_path: self.theme_path.as_ref().map(|path| {
                path.strip_prefix(&self.theme_dir)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            }),
            terminal_merge_window_ms: self.terminal_panel.merge_window_ms,
            terminal_max_entries: self.terminal_panel.max_entries,
            log_max_entries: self.bottom_log_panel.max_entries,
            command_usage_order: self.command_palette.usage_order.clone(),
            network_proxy_url: (!network_proxy_url.is_empty())
                .then(|| network_proxy_url.to_owned()),
            network_ports: self.serial.network_ports.clone(),
        }
    }

    pub(crate) fn save_config(&self) -> Result<(), String> {
        let cfg = self.build_config_snapshot();
        let path = config_path();
        write_native_json(&path, &cfg)
    }

    pub(crate) fn load_config_from_path(&mut self, path: &Path) -> Result<(), String> {
        let bytes = read_native_file(path)?
            .ok_or_else(|| format!("读取失败：文件不存在：{}", path.display()))?;
        let text = String::from_utf8(bytes).map_err(|error| format!("读取失败：{error}"))?;
        let (cfg, _) = parse_persisted_config(&text)?;
        self.serial.selected_port = cfg.selected_port;
        self.serial.baud_rate = cfg.baud_rate;
        self.serial.data_bits = cfg.data_bits;
        self.serial.stop_bits = cfg.stop_bits;
        self.serial.parity = cfg.parity;
        self.recorder_path = cfg.recorder_path;
        self.serial.port_aliases = cfg.port_aliases;
        self.serial.port_groups = cfg.port_groups;
        self.serial.port_profiles = cfg.port_profiles;
        self.serial.auto_reconnect = cfg.auto_reconnect;
        self.keymap = cfg.keymap;
        self.monospace_font_size = cfg.monospace_font_size.clamp(10.0, 24.0);
        self.theme_path = cfg
            .theme_path
            .as_deref()
            .map(|stored| resolve_theme_path(&self.theme_dir, stored));
        if let Some(path) = self.theme_path.as_deref() {
            tool_panels::theme::load_theme_file(path)?;
            self.ui_theme = tool_panels::theme::builtin_theme_for_path(path)
                .unwrap_or(tool_panels::theme::AppTheme::Custom);
        } else {
            self.ui_theme = cfg.ui_theme;
        }
        self.terminal_panel.merge_window_ms = cfg.terminal_merge_window_ms;
        self.terminal_panel
            .set_max_entries(cfg.terminal_max_entries);
        self.bottom_log_panel.set_max_entries(cfg.log_max_entries);
        self.command_palette.usage_order = cfg.command_usage_order;
        self.network_proxy_url = cfg.network_proxy_url.unwrap_or_default();
        self.serial.network_ports = cfg.network_ports;
        self.send.send_history = cfg
            .send_history
            .into_iter()
            .filter(|item| !item.trim().is_empty())
            .take(MAX_SEND_HISTORY)
            .collect();
        self.send.line_ending = cfg.line_ending;
        self.panels = cfg.panels;
        self.apply_loaded_workspace_postprocess();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn legacy_workspace_json() -> String {
        serde_json::json!({
            "panels": PanelManager::default(),
            "selected_port": null,
            "baud_rate": "115200",
            "data_bits": "8",
            "stop_bits": "1",
            "parity": "none",
            "recorder_path": "logs/session.jsonl",
            "custom_theme_path": "one-dark-pro.json"
        })
        .to_string()
    }

    /// 「最近工作区 → 打开」把 `read_native_file` 的 Err 正文**原样**渲染进状态栏
    /// （`ui/settings_panel.rs` 的那条 `set_status_force(StatusLevel::Error, error)`），
    /// 所以这句必须是中文框架，不能是 `StorageError` 的英文 `Display`。
    /// 形状选「该是文件的地方放一个目录」：文件缺失走的是 `Ok(None)`，只有读失败才出错，
    /// 而这个做法不依赖平台（Windows 给拒绝访问、类 Unix 给 Is a directory）。
    #[test]
    fn unreadable_workspace_file_fails_with_the_chinese_storage_frame() {
        let root = std::env::temp_dir().join(format!(
            "hw-config-read-words-{}-{}",
            std::process::id(),
            tool_core::now_timestamp_ms()
        ));
        std::fs::create_dir_all(&root).expect("建临时根目录");
        let path = root.join("workspace.json");
        std::fs::create_dir(&path).expect("把 workspace.json 做成目录，读取必失败");

        let error = read_native_file(&path).expect_err("读不出来的配置文件必须报错");

        assert!(
            error.starts_with("存储操作失败："),
            "配置文件读取失败必须走 `StorageError` 的中文框架，实际 {error:?}"
        );
        assert!(
            !error.contains("storage operation failed:"),
            "`StorageError` 的英文 `Display` 又漏进正文了，它会被直接渲染进状态栏：{error:?}"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_workspace_migrates_to_versioned_theme_path() {
        let (config, migrated) = parse_persisted_config(&legacy_workspace_json())
            .expect("legacy workspace should migrate");

        assert!(migrated);
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(config.theme_path.as_deref(), Some("one-dark-pro.json"));
    }

    #[test]
    fn future_workspace_schema_is_rejected() {
        let mut value: serde_json::Value =
            serde_json::from_str(&legacy_workspace_json()).expect("valid test JSON");
        value["schema_version"] = serde_json::json!(CURRENT_SCHEMA_VERSION + 1);

        assert_eq!(
            declared_schema_version(&value.to_string()),
            Some(CURRENT_SCHEMA_VERSION + 1)
        );
        assert!(parse_persisted_config(&value.to_string()).is_err());
    }

    /// 只用历史键名 `version` 声明版本的文档，两条读取路径必须给出同一个版本号。
    /// 这里断言完整错误文本而不是 `is_err()`：消息里的版本号是"parse 侧到底认出了
    /// 哪个键"的唯一可观察量——若它漏掉 `version` 而读成 v0，就不会拒绝而是迁移放行。
    #[test]
    fn legacy_version_key_is_a_version_for_both_config_readers() {
        let mut value: serde_json::Value =
            serde_json::from_str(&legacy_workspace_json()).expect("valid test JSON");
        value["version"] = serde_json::json!(CURRENT_SCHEMA_VERSION + 1);
        let text = value.to_string();

        let version = CURRENT_SCHEMA_VERSION + 1;
        assert_eq!(declared_schema_version(&text), Some(version));
        assert_eq!(
            parse_persisted_config(&text).err(),
            Some(format!(
                "配置版本 {version} 高于当前程序支持的 {CURRENT_SCHEMA_VERSION}"
            ))
        );
    }

    /// 从 `schema_version` 改名之前的版本升级上来时，磁盘上的配置只有历史键 `version`。
    /// 这样的文件必须**照常读出来**：`load_config` 在解析失败时会直接
    /// `quarantine_corrupt_file(primary)` 把用户的工作台配置挪走并回落到默认值，
    /// 所以这里不是"报错文案不好"，而是升级后第一次开机就丢设置。
    #[test]
    fn legacy_version_only_config_still_loads() {
        let mut value: serde_json::Value =
            serde_json::from_str(&legacy_workspace_json()).expect("valid test JSON");
        value["version"] = serde_json::json!(CURRENT_SCHEMA_VERSION);
        let text = value.to_string();

        let (config, migrated) = parse_persisted_config(&text)
            .unwrap_or_else(|error| panic!("只用历史键声明版本的配置必须能读出来，实际: {error}"));
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(!migrated, "版本已经是当前值，不该被当成需要迁移");
        // 设置本身没被吃掉：挑一个有代表性的字段核对。
        assert_eq!(config.baud_rate, "115200");
    }

    /// 配置里的使用顺序可能存着认不出的值（按标题记的历史、已下架的插件命令）。
    /// 解析必须把它们原样读出来：这些值只是排不到前面，而一旦解析侧"顺手"裁掉
    /// 不认识的项，用户下次开机丢的就不是排序而是整份历史。
    #[test]
    fn stale_title_usage_order_is_loaded_verbatim() {
        let mut value: serde_json::Value =
            serde_json::from_str(&legacy_workspace_json()).expect("valid test JSON");
        value["command_usage_order"] = serde_json::json!(["刷新端口", "连接串口"]);

        let (config, _) = parse_persisted_config(&value.to_string()).unwrap_or_else(|error| {
            panic!("带 command_usage_order 的配置必须读得出来，实际: {error}")
        });

        assert_eq!(
            config.command_usage_order,
            ["刷新端口", "连接串口"],
            "历史值必须按原顺序原样保留，解析侧不裁剪也不改写"
        );
    }

    // ── ensure_jsonl_extension ──

    #[test]
    fn ensure_jsonl_extension_no_extension_adds_jsonl() {
        let path = PathBuf::from("/tmp/session-12345");
        let result = ensure_jsonl_extension(path);
        assert_eq!(result.extension().unwrap(), "jsonl");
        assert!(result.to_string_lossy().ends_with(".jsonl"));
    }

    #[test]
    fn ensure_jsonl_extension_already_jsonl_unchanged() {
        let path = PathBuf::from("/tmp/session-12345.jsonl");
        let result = ensure_jsonl_extension(path.clone());
        assert_eq!(result, path);
    }

    #[test]
    fn ensure_jsonl_extension_case_insensitive_jsonl_unchanged() {
        let path = PathBuf::from("/tmp/session-12345.JSONL");
        let result = ensure_jsonl_extension(path.clone());
        assert_eq!(result, path);
    }

    #[test]
    fn ensure_jsonl_extension_other_extension_replaced() {
        let path = PathBuf::from("/tmp/session-12345.txt");
        let result = ensure_jsonl_extension(path);
        assert_eq!(result.extension().unwrap(), "jsonl");
    }

    // ── default_recorder_path ──

    #[test]
    fn default_recorder_path_contains_session_and_jsonl() {
        let path = default_recorder_path();
        assert!(
            path.contains("session-"),
            "path should contain 'session-': {path}"
        );
        assert!(
            path.ends_with(".jsonl"),
            "path should end with .jsonl: {path}"
        );
    }

    // ── default_true ──

    #[test]
    fn default_true_returns_true() {
        assert!(default_true());
    }

    // ── atomic_write_json ──

    #[test]
    fn atomic_write_json_basic_write_and_read_back() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Data {
            key: String,
            value: i32,
        }

        let original = Data {
            key: "hello".into(),
            value: 42,
        };

        atomic_write_json(&path, &original).expect("write should succeed");
        assert!(path.exists(), "file should exist after write");

        let read_back: Data =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back, original);
    }

    #[test]
    fn atomic_write_json_overwrite() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        let first = Data { value: 1 };
        let second = Data { value: 2 };

        atomic_write_json(&path, &first).expect("first write");
        atomic_write_json(&path, &second).expect("second write");

        let read_back: Data =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back, second);
    }

    #[test]
    fn atomic_write_json_creates_backup() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");
        let backup_path = dir.path().join("test.json.backup");

        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        let first = Data { value: 1 };
        let second = Data { value: 2 };

        // 第一次写入不会创建备份（因为目标文件还不存在）
        atomic_write_json(&path, &first).expect("first write");
        assert!(!backup_path.exists(), "no backup on first write");

        // 第二次写入应该创建备份
        atomic_write_json(&path, &second).expect("second write");
        assert!(backup_path.exists(), "backup should exist after overwrite");

        // 备份文件应该包含第一次写入的内容
        let backup_data: Data =
            serde_json::from_str(&std::fs::read_to_string(&backup_path).unwrap()).unwrap();
        assert_eq!(backup_data, first);
    }

    #[test]
    fn atomic_write_json_no_temp_file_left_behind() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.json");
        let temp_path = dir.path().join("test.tmp");

        #[derive(Serialize)]
        struct Data {
            value: i32,
        }

        atomic_write_json(&path, &Data { value: 1 }).expect("write should succeed");
        assert!(
            !temp_path.exists(),
            "temp file should not remain after write"
        );
    }
}
