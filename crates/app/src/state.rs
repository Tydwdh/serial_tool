use serde::{Deserialize, Serialize};
use tool_transport::{SerialConfig, SerialPortDescriptor};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum StatusLevel {
    Info,
    Warn,
    Error,
}

impl StatusLevel {
    pub(crate) fn ttl_ms(self) -> u64 {
        match self {
            Self::Info => 5_000,
            Self::Warn => 8_000,
            Self::Error => 15_000,
        }
    }
}

/// 通知：状态栏消息的最小单元。每条消息独立存在、独立过期。
#[derive(Clone)]
pub(crate) struct Notification {
    pub(crate) level: StatusLevel,
    pub(crate) text: String,
    /// 过期时间戳（ms），过期后自动从队列移除。
    pub(crate) deadline_ms: u64,
}

/// 通知队列：多条消息按时间排列，互不覆盖。
/// 同 source 的新消息会替换该 source 的旧消息（避免刷屏）。
#[derive(Clone)]
pub(crate) struct NotificationQueue {
    /// (source, Notification) — 按插入顺序排列。
    entries: std::collections::VecDeque<(String, Notification)>,
}

impl NotificationQueue {
    pub(crate) fn new() -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
        }
    }

    /// 推送一条通知。同 source 的旧消息被替换（去重但不丢失历史位置）。
    pub(crate) fn push(&mut self, source: &str, level: StatusLevel, text: impl Into<String>) {
        let now = tool_core::now_timestamp_ms();
        let notification = Notification {
            level,
            text: text.into(),
            deadline_ms: now + level.ttl_ms(),
        };

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
    pub(crate) fn current(&mut self) -> Vec<Notification> {
        let now = tool_core::now_timestamp_ms();
        while self
            .entries
            .front()
            .is_some_and(|(_, n)| now > n.deadline_ms)
        {
            self.entries.pop_front();
        }
        // 也清理中间过期的（保留顺序）
        self.entries.retain(|(_, n)| now <= n.deadline_ms);
        self.entries.iter().map(|(_, n)| n.clone()).collect()
    }
}

impl Default for NotificationQueue {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) const MAX_SEND_HISTORY: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedPanelAction {
    None,
    Attach,
    Close,
}

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
    pub(crate) popup_open: bool,
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
            popup_open: false,
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
#[derive(Clone)]
pub(crate) struct PendingReconnect {
    pub(crate) port_name: String,
    pub(crate) config: SerialConfig,
    pub(crate) attempts: u32,
    pub(crate) next_try_at: f64,
}

/// 串口相关的 UI 状态聚合：端口列表、选中端口、串口参数、自动重连、别名与配置档案。
///
/// 将原先散落在 `WorkbenchApp` 上的 13 个字段收拢于此，便于统一管理与持久化转换。
pub(crate) struct SerialUiState {
    pub(crate) ports: Vec<SerialPortDescriptor>,
    pub(crate) selected_port: Option<String>,
    pub(crate) baud_rate: String,
    pub(crate) data_bits: String,
    pub(crate) stop_bits: String,
    pub(crate) parity: String,
    pub(crate) last_port_refresh: f64,
    pub(crate) auto_reconnect: bool,
    pub(crate) pending_reconnect: Option<PendingReconnect>,
    pub(crate) port_aliases: std::collections::HashMap<String, String>,
    pub(crate) port_groups: std::collections::HashMap<String, String>,
    pub(crate) port_profiles: std::collections::HashMap<String, crate::config::PortProfile>,
    pub(crate) top_bar_serial_collapsed: bool,
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
            port_aliases: std::collections::HashMap::new(),
            port_groups: std::collections::HashMap::new(),
            port_profiles: std::collections::HashMap::new(),
            top_bar_serial_collapsed: false,
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
    /// 下载完成后的 SHA256
    pub(crate) downloaded_sha256: Option<String>,
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
    pub(crate) changelog: Vec<String>,
    /// 是否已缓存跳过（无需更新 UI）
    pub(crate) cached: bool,
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
            downloaded_sha256: None,
            want_restart: false,
            force_check: false,
            download_progress_arc: None,
        }
    }
}
