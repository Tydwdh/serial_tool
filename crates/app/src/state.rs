use serde::{Deserialize, Serialize};
use tool_application::query::PortView;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum StatusLevel {
    Info,
    Warn,
    Error,
}

impl StatusLevel {
    /// 通知自动过期时间（毫秒）。错误保留更久，但也会自动关闭。
    pub(crate) fn ttl_ms(self) -> Option<u64> {
        match self {
            Self::Info => Some(1_500),
            Self::Warn => Some(2_000),
            Self::Error => Some(4_000),
        }
    }
}

/// 通知：状态栏消息的最小单元。每条消息独立存在、独立过期。
#[derive(Clone)]
pub(crate) struct Notification {
    /// 单调递增的通知版本。即使同一 source 更新也会产生新编号，供 Toast 识别。
    pub(crate) id: u64,
    pub(crate) level: StatusLevel,
    pub(crate) text: String,
    /// 过期时间戳（ms）。None 表示永不过期（Error 级别）。
    pub(crate) deadline_ms: Option<u64>,
}

impl Notification {
    fn is_expired(&self, now: u64) -> bool {
        self.deadline_ms.is_some_and(|dl| now > dl)
    }
}

/// 通知队列：多条消息按时间排列，互不覆盖。
/// 同 source 的新消息会替换该 source 的旧消息（避免刷屏）。
#[derive(Clone)]
pub(crate) struct NotificationQueue {
    /// (source, Notification) — 按插入顺序排列。
    entries: std::collections::VecDeque<(String, Notification)>,
    next_id: u64,
}

impl NotificationQueue {
    pub(crate) fn new() -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            next_id: 1,
        }
    }

    /// 推送一条通知。同 source 的旧消息被替换（去重但不丢失历史位置）。
    /// Error 级别展示时间更长，也可手动 dismiss。
    pub(crate) fn push(&mut self, source: &str, level: StatusLevel, text: impl Into<String>) {
        let now = tool_core::now_timestamp_ms();
        let deadline_ms = level.ttl_ms().map(|ttl| now + ttl);
        let notification = Notification {
            id: self.next_id,
            level,
            text: text.into(),
            deadline_ms,
        };
        self.next_id = self.next_id.wrapping_add(1).max(1);

        // 同 source 替换旧消息，保持队列位置不变
        for (s, n) in self.entries.iter_mut().rev() {
            if s == source {
                *n = notification;
                return;
            }
        }
        // 新 source：推入末尾
        self.entries.push_back((source.to_owned(), notification));
    }

    /// 获取当前未过期的所有通知（按插入顺序）。
    /// Error 级别展示时间更长。
    pub(crate) fn current(&mut self) -> Vec<Notification> {
        let now = tool_core::now_timestamp_ms();
        // 清理头部过期的（非 Error）
        while self.entries.front().is_some_and(|(_, n)| n.is_expired(now)) {
            self.entries.pop_front();
        }
        // 也清理中间过期的（保留顺序，但保留 Error）
        self.entries.retain(|(_, n)| !n.is_expired(now));
        self.entries.iter().map(|(_, n)| n.clone()).collect()
    }

    /// 手动移除一个通知（按 source）。用于用户交互关闭。
    #[allow(dead_code)]
    pub(crate) fn dismiss(&mut self, source: &str) {
        self.entries.retain(|(s, _)| s != source);
    }
}

impl Default for NotificationQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_source_creates_a_new_notification_version() {
        let mut queue = NotificationQueue::new();
        queue.push("serial", StatusLevel::Info, "first");
        let first_id = queue.current()[0].id;

        queue.push("serial", StatusLevel::Warn, "second");
        let notifications = queue.current();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].id > first_id);
        assert_eq!(notifications[0].text, "second");
    }

    /// 与 `tool_updater::is_cache_valid` 同一时间基准（Unix 毫秒）。
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    // ── 自动更新的两道 gate ──
    //
    // 反向自检（必须能失败，改完请还原）：
    //   1. 删掉 `cached_check_result` 里的 `is_pinned_sha256(&cache.sha256)` 条件
    //      → `cached_check_hit_requires_pinned_sha256` 变红。
    //   2. 删掉 `pinned_update_sha256` 的 `.filter(...)`
    //      → `download_pin_gate_requires_pinned_sha256` 变红。

    #[test]
    fn cached_check_hit_requires_pinned_sha256() {
        let pin = "a".repeat(64);
        let cache = |sha256: &str| tool_updater::CheckCache {
            last_check_time: now_ms(),
            latest_version: "1.3.0".to_owned(),
            had_update: true,
            sha256: sha256.to_owned(),
        };

        // 24h 内 + pinned 摘要合法：命中，且带出去的就是那个固定值。
        let hit = cached_check_result(|| Some(cache(&pin)), false).expect("合法缓存应命中");
        assert_eq!(
            hit.sha256, pin,
            "缓存命中必须把 pinned 摘要带进 CheckResult"
        );
        assert_eq!(hit.version, "1.3.0");
        assert!(hit.cached);
        assert!(
            hit.download_url.is_empty(),
            "缓存命中不该凭空造出一个下载 URL"
        );

        // 空 / 短 / 非十六进制：一律未命中。缓存是本机可改的文件，
        // 放行这些值就等于让"有版本号但没有固定摘要"的结果去驱动下载。
        for bad in ["", "deadbeef", &"a".repeat(63), &"g".repeat(64)] {
            assert!(
                cached_check_result(|| Some(cache(bad)), false).is_none(),
                "缓存里的 {bad:?} 不是合法 pinned 摘要，必须重新拉取 update.json"
            );
        }

        // 过期缓存与"没有缓存记录"同样不命中。
        let expired = tool_updater::CheckCache {
            last_check_time: 0,
            ..cache(&pin)
        };
        assert!(
            cached_check_result(|| Some(expired), false).is_none(),
            "超过 24h 的缓存必须重取"
        );
        assert!(
            cached_check_result(|| None, false).is_none(),
            "没有缓存记录时不得凭空命中"
        );

        // 用户手动检查（force）不命中，而且连缓存文件都不读（抽取前的行为）。
        let read = std::cell::Cell::new(false);
        assert!(
            cached_check_result(
                || {
                    read.set(true);
                    Some(cache(&pin))
                },
                true,
            )
            .is_none(),
            "手动检查更新不得复用 24h 缓存"
        );
        assert!(!read.get(), "手动检查更新连缓存都不该读");
    }

    #[test]
    fn download_pin_gate_requires_pinned_sha256() {
        let pin = "0123456789abcdef".repeat(4);
        let state = |expected_sha256: Option<String>| UpdateState {
            expected_sha256,
            ..Default::default()
        };

        assert_eq!(
            state(Some(pin.clone())).pinned_update_sha256(),
            Some(pin.as_str()),
            "合法的 pinned 摘要应原样交给下载侧比对"
        );

        // 合法但大写/来源不同的值：gate 只判形状，不改值、也不额外收紧成"必须小写"，
        // 否则一份大写十六进制的 update.json 会被静默拒掉。
        let upper = pin.to_uppercase();
        assert_eq!(
            state(Some(upper.clone())).pinned_update_sha256(),
            Some(upper.as_str()),
            "gate 不得改动 pinned 值本身"
        );

        // 缺失、被清空、被截断、非十六进制都不能当通行证。
        for bad in [
            None,
            Some(String::new()),
            Some("deadbeef".to_owned()),
            Some("a".repeat(63)),
            Some("g".repeat(64)),
        ] {
            assert_eq!(
                state(bad.clone()).pinned_update_sha256(),
                None,
                "{bad:?} 不能作为下载校验的期望值"
            );
        }
    }
}

pub(crate) const MAX_SEND_HISTORY: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LineEnding {
    None,
    Lf,
    Cr,
    Crlf,
}

impl LineEnding {
    pub(crate) const ALL: [Self; 4] = [Self::None, Self::Lf, Self::Cr, Self::Crlf];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Lf => "LF",
            Self::Cr => "CR",
            Self::Crlf => "CRLF",
        }
    }

    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Lf => "\n",
            Self::Cr => "\r",
            Self::Crlf => "\r\n",
        }
    }
}

pub(crate) struct SendUiState {
    pub(crate) input: String,
    pub(crate) hex_mode: bool,
    pub(crate) line_ending: LineEnding,
    pub(crate) error: Option<String>,
    pub(crate) target_port: Option<String>,
    pub(crate) send_history: std::collections::VecDeque<String>,
    /// 发送历史 popup 的搜索框文本。
    pub(crate) history_search: String,
    /// ↑↓ 方向键导航历史时的当前索引（None = 未导航，Some(0)=最新一条）。
    pub(crate) history_index: Option<usize>,
    /// 导航开始前保存的原始输入，按 ↓ 到尽头时恢复。
    pub(crate) saved_input: String,
    pub(crate) hex_strict: bool,
    pub(crate) dtr_high: bool,
    pub(crate) rts_high: bool,
    pub(crate) periodic_enabled: bool,
    pub(crate) periodic_interval_ms: String,
    pub(crate) periodic_send_count: u64,
    pub(crate) periodic_max_count: Option<u64>,
}

impl Default for SendUiState {
    fn default() -> Self {
        Self {
            input: String::new(),
            hex_mode: false,
            line_ending: LineEnding::None,
            error: None,
            target_port: None,
            send_history: std::collections::VecDeque::new(),
            history_search: String::new(),
            history_index: None,
            saved_input: String::new(),
            hex_strict: true,
            dtr_high: true,
            rts_high: true,
            periodic_enabled: false,
            periodic_interval_ms: "1000".to_owned(),
            periodic_send_count: 0,
            periodic_max_count: None,
        }
    }
}

/// 待重连的串口信息（拔出后自动重连用）。
/// 重连时使用当前 UI 串口配置，故仅记录端口名。
#[derive(Clone)]
pub(crate) struct PendingReconnect {
    pub(crate) port_name: String,
    pub(crate) attempts: u32,
    pub(crate) next_try_at: f64,
}

/// 等待 transport 确认端口已真正打开后再显示成功提示。
#[derive(Clone)]
pub(crate) struct PendingPortOpenNotice {
    pub(crate) port_name: String,
    pub(crate) success_message: String,
    pub(crate) requested_at: f64,
}

/// 串口相关的 UI 状态聚合：端口列表、选中端口、串口参数、自动重连、别名与配置档案。
///
/// 将原先散落在 `WorkbenchApp` 上的 13 个字段收拢于此，便于统一管理与持久化转换。
pub(crate) struct SerialUiState {
    pub(crate) ports: Vec<PortView>,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) last_port_refresh: f64,
    pub(crate) auto_reconnect: bool,
    pub(crate) pending_reconnect: Option<PendingReconnect>,
    /// Ports manually closed by the user must not be treated as unexpected
    /// disconnects by the periodic port-list refresh.
    pub(crate) manual_disconnects: std::collections::HashSet<String>,
    pub(crate) pending_open_notice: Option<PendingPortOpenNotice>,
    pub(crate) port_aliases: std::collections::HashMap<String, String>,
    pub(crate) port_groups: std::collections::HashMap<String, String>,
    pub(crate) port_profiles: std::collections::HashMap<String, crate::config::PortProfile>,
    pub(crate) top_bar_serial_collapsed: bool,
    /// 网络模拟串口列表（WebSocket + JSON-RPC gcode 桥），持久化到配置。
    pub(crate) network_ports: Vec<tool_application::query::NetworkPortConfig>,
    /// “网络端口”连接表单的主机输入。
    pub(crate) network_host: String,
    /// “网络端口”连接表单的端口输入。
    pub(crate) network_port: String,
    /// “网络端口”连接表单的 API Key 输入（仅作为编辑缓冲，不单独持久化）。
    pub(crate) network_api_key: String,
}

impl SerialUiState {
    /// 获取端口的用户友好显示名。有别名则显示 `别名 (COMx)`，否则显示原始端口名。
    pub(crate) fn port_label(&self, port: &str) -> String {
        match self.port_aliases.get(port).filter(|s| !s.trim().is_empty()) {
            Some(alias) => format!("{alias} ({port})"),
            None => port.to_owned(),
        }
    }
}

impl Default for SerialUiState {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            selected_port: None,
            baud_rate: "115200".to_owned(),
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            parity: "none".to_owned(),
            last_port_refresh: 0.0,
            auto_reconnect: true,
            pending_reconnect: None,
            manual_disconnects: std::collections::HashSet::new(),
            pending_open_notice: None,
            port_aliases: std::collections::HashMap::new(),
            port_groups: std::collections::HashMap::new(),
            port_profiles: std::collections::HashMap::new(),
            top_bar_serial_collapsed: false,
            network_ports: Vec::new(),
            network_host: String::new(),
            network_port: "7125".to_owned(),
            network_api_key: String::new(),
        }
    }
}

// ── 自动更新状态 ──

/// 自动更新状态。
pub(crate) struct UpdateState {
    /// 远端最新版本号（如 "0.3.0"）
    pub(crate) latest_version: Option<String>,
    /// 更新日志
    pub(crate) changelog: Vec<String>,
    /// 下载进度 0.0–1.0
    pub(crate) download_progress: f32,
    /// 是否有新版本可用
    pub(crate) update_available: bool,
    /// 更新包是否已下载完成
    pub(crate) downloaded: bool,
    /// 错误信息
    pub(crate) error: Option<String>,
    /// 是否正在检查更新
    pub(crate) checking: bool,
    /// 是否正在下载
    pub(crate) downloading: bool,
    /// 后台检查线程的 JoinHandle
    pub(crate) check_handle: Option<std::thread::JoinHandle<Result<CheckResult, String>>>,
    /// 后台下载线程的 JoinHandle
    pub(crate) download_handle: Option<std::thread::JoinHandle<Result<String, String>>>,
    /// 下载 URL（从 update.json 获取）
    pub(crate) download_url: Option<String>,
    /// 本次待装包的 pinned 摘要（来自 update.json），**不是**下载自算值。
    ///
    /// 刻意不设"下载流自算 SHA256"那种字段：它与 pin 形状相同，迟早会被写成
    /// `expected_sha256.or(自算值)` 的回退，而那等于"拿下载内容跟下载内容比"。
    pub(crate) expected_sha256: Option<String>,
    /// 用户点击"更新并重启"后，标记需要退出
    pub(crate) want_restart: bool,
    /// 用户手动触发检查（跳过 24h 缓存）
    pub(crate) force_check: bool,
    /// 下载进度共享变量（0-1000 表示 0.0%-100.0%），由后台下载线程写入、tick 读取
    pub(crate) download_progress_arc: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
}

/// 后台检查线程的返回结果。
pub(crate) struct CheckResult {
    pub(crate) version: String,
    pub(crate) download_url: String,
    /// 来自 update.json 的外置固定摘要：下载与写 manifest 都以它为准。
    pub(crate) sha256: String,
    pub(crate) changelog: Vec<String>,
    /// 是否已缓存跳过（无需更新 UI）
    pub(crate) cached: bool,
}

impl UpdateState {
    /// gate 2/2（下载与写 manifest 侧）：本次更新唯一可用的校验期望值。
    ///
    /// 只有真正来自 `update.json` 且形状合法的 pinned 摘要才算数：缺失（`None`）、
    /// 被清空（`""`）、被截断或含非十六进制字符一律返回 `None`，调用方据此拒绝下载
    /// 并报错。绝不回退到"下完再自算"——那等于没有校验。
    ///
    /// 刻意留在 `UpdateState` 上而不是写在 `WorkbenchApp` 里：这条判定只读字段，
    /// 抽出来才能在本文件的 `mod tests` 里被直接测到（`WorkbenchApp` 的构造需要
    /// `&eframe::CreationContext`，headless 下跑不了）。
    pub(crate) fn pinned_update_sha256(&self) -> Option<&str> {
        self.expected_sha256
            .as_deref()
            .filter(|pin| tool_updater::update_info::is_pinned_sha256(pin))
    }
}

/// gate 1/2（检查侧）：24h 检查缓存能否直接复用。
///
/// 命中会跳过 `update.json` 的拉取，所以缓存里的 `sha256` 必须是上次真实从清单
/// 拿到的固定值。老缓存（该字段 `#[serde(default)]` ⇒ `""`）与被改空/改短/改成非
/// 十六进制的记录都当作未命中（返回 `None`，调用方重新联网拉取），不给它们放行
/// 一条"有版本号但没有 pinned 摘要"的结果。
///
/// `force`（用户手动检查更新）无视缓存，且连缓存文件都不去读 —— 故缓存用
/// 惰性读取传入，而不是先读好再传进来。
pub(crate) fn cached_check_result(
    read_cache: impl FnOnce() -> Option<tool_updater::CheckCache>,
    force: bool,
) -> Option<CheckResult> {
    if force {
        return None;
    }
    let cache = read_cache().filter(|cache| {
        tool_updater::is_cache_valid(cache)
            && tool_updater::update_info::is_pinned_sha256(&cache.sha256)
    })?;
    Some(CheckResult {
        version: cache.latest_version,
        download_url: String::new(),
        sha256: cache.sha256,
        changelog: Vec::new(),
        cached: true,
    })
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            latest_version: None,
            changelog: Vec::new(),
            download_progress: 0.0,
            update_available: false,
            downloaded: false,
            error: None,
            checking: false,
            downloading: false,
            check_handle: None,
            download_handle: None,
            download_url: None,
            expected_sha256: None,
            want_restart: false,
            force_check: false,
            download_progress_arc: None,
        }
    }
}
