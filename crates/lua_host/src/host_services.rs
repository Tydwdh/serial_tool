//! 宿主服务数据结构：文件过滤、对话框请求、文件授权、行缓冲区、运行时服务装配。
//!
//! 这些类型由 extension crate 组装后传给 lua_host 运行时，是"管理面"与"执行面"的桥梁。

use crate::ConfigStore;
use parking_lot::Mutex as ParkingMutex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

// ── FileFilter ──

#[derive(Debug, Clone)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

// ── DialogRequest ──

pub struct DialogRequest {
    pub plugin_id: String,
    pub title: String,
    pub filters: Vec<FileFilter>,
    pub response_sender: crossbeam_channel::Sender<Option<PathBuf>>,
}

// ── FileAccessBroker ──

/// 跨组件共享的文件访问授权管理器。
#[derive(Debug, Default)]
pub struct FileAccessBroker {
    authorized: parking_lot::Mutex<HashMap<String, HashSet<PathBuf>>>,
}

fn canonical_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

impl FileAccessBroker {
    pub fn authorize(&self, plugin_id: &str, path: PathBuf) {
        let canonical = canonical_path(&path);
        self.authorized
            .lock()
            .entry(plugin_id.to_owned())
            .or_default()
            .insert(canonical);
    }

    pub fn is_authorized(&self, plugin_id: &str, path: &Path) -> bool {
        let canonical = canonical_path(path);
        self.authorized
            .lock()
            .get(plugin_id)
            .map(|paths| paths.contains(&canonical))
            .unwrap_or(false)
    }

    pub fn clear(&self, plugin_id: &str) {
        self.authorized.lock().remove(plugin_id);
    }
}

// ── LineBuffer ──

/// 按 plugin_id + port_name 隔离的行缓冲区
#[derive(Debug, Clone)]
pub struct LineBuffer {
    pub lines: VecDeque<String>,
    raw: Vec<u8>,
    pub max_buffer_bytes: usize,
    pub max_line_bytes: usize,
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self {
            lines: VecDeque::new(),
            raw: Vec::new(),
            max_buffer_bytes: 256 * 1024,
            max_line_bytes: 16 * 1024,
        }
    }
}

/// `feed()` 返回的统计信息，用于检测缓冲区溢出。
#[derive(Debug, Default)]
pub struct FeedStats {
    /// 因缓冲区满而丢弃的完整行数。
    pub lines_dropped: usize,
    /// 因单行过长而被截断的行数。
    pub lines_truncated: usize,
    /// 因 raw buffer 满而被丢弃的原始字节数。
    pub bytes_dropped: usize,
}

impl LineBuffer {
    /// 喂入原始字节，拆分完整行。超出容量时丢弃最老的行。
    /// 返回 `FeedStats` 供调用方检测溢出并发布日志。
    pub(crate) fn feed(&mut self, data: &[u8]) -> FeedStats {
        let mut stats = FeedStats::default();

        // 防止无换行长流撑爆 raw buffer
        if self.raw.len() + data.len() > self.max_buffer_bytes {
            // 先尝试丢弃已解析的完整行
            while self.raw.len() + data.len() > self.max_buffer_bytes && !self.lines.is_empty() {
                self.lines.pop_front();
                stats.lines_dropped += 1;
            }
            // 如果仍然超限，截断 raw 头部
            if self.raw.len() + data.len() > self.max_buffer_bytes {
                let excess = self.raw.len() + data.len() - self.max_buffer_bytes;
                let drain_pos = excess.min(self.raw.len());
                stats.bytes_dropped = drain_pos;
                self.raw.drain(..drain_pos);
            }
        }
        self.raw.extend_from_slice(data);

        // 按 \n 拆行
        while let Some(pos) = self.raw.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = self.raw.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).to_string();
            let trimmed = line
                .trim_end_matches('\r')
                .trim_end_matches('\n')
                .to_owned();
            if trimmed.len() <= self.max_line_bytes {
                self.lines.push_back(trimmed);
            } else {
                // UTF-8 安全截断：找到不超过 max_line_bytes 字节的最大字符边界
                let mut byte_cutoff = 0;
                for ch in trimmed.chars() {
                    let next = byte_cutoff + ch.len_utf8();
                    if next > self.max_line_bytes {
                        break;
                    }
                    byte_cutoff = next;
                }
                let truncated = trimmed[..byte_cutoff].to_owned();
                self.lines.push_back(truncated);
                stats.lines_truncated += 1;
            }
        }

        stats
    }

    pub(crate) fn next_line(&mut self) -> Option<String> {
        self.lines.pop_front()
    }
}

/// 跨组件共享的行缓冲区映射。
pub type LineBufferMap = Arc<ParkingMutex<HashMap<String, LineBuffer>>>;

pub(crate) fn line_buffer_key(plugin_id: &str, port_name: &str) -> String {
    format!("{plugin_id}:{port_name}")
}

// ── LuaHostServices ──

/// 传递给 Lua runtime 的宿主服务。
pub struct LuaHostServices {
    pub plugin_root: Option<PathBuf>,
    pub plugin_id: String,
    pub dialog_sender: Option<crossbeam_channel::Sender<DialogRequest>>,
    pub file_broker: Option<Arc<FileAccessBroker>>,
    pub stop_flag: Option<Arc<AtomicBool>>,
    pub line_buffers: Option<LineBufferMap>,
    pub config_store: Option<Arc<ConfigStore>>,
}
