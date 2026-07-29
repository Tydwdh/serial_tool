//! 后端事件定义：所有从 Rust 发送到 Flutter 的事件。

use serde::Serialize;
use serde_json::Value;
use tool_core::{Direction, LogLevel};
use tool_transport::SerialPortDescriptor;

/// 从 Rust 后端发送到 Flutter 前端的事件。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BackendEvent {
    // ── 系统 ──
    /// 后端初始化完成
    Ready,
    /// 错误消息
    Error { message: String },

    // ── 串口 ──
    /// 串口列表更新
    PortList { ports: Vec<SerialPortDescriptor> },
    /// 串口数据
    SerialData {
        port: String,
        direction: Direction,
        data: Vec<u8>,
        timestamp: u64,
    },
    /// 串口打开结果
    SerialOpen {
        port: String,
        success: bool,
        error: Option<String>,
    },
    /// 串口已关闭
    SerialClose { port: String },
    /// 串口事件（挂起重连、自动重连等）
    SerialEvent {
        port: String,
        kind: String,
        message: String,
    },

    /// Protocol JSON emitted by plugins or decoders, used by Flutter dynamic
    /// charts, gauges, and attitude panels.
    ProtocolData {
        topic: String,
        data: Value,
        timestamp: u64,
    },

    // ── 日志 ──
    /// 系统日志
    Log {
        level: LogLevel,
        source: String,
        message: String,
    },

    // ── 通知 ──
    /// 状态栏通知
    Notification { level: String, message: String },

    // ── 录制 ──
    /// 录制状态更新
    RecorderStatus {
        recording: bool,
        stats: Option<Value>,
    },

    // ── 回放 ──
    /// 回放状态更新
    ReplayStatus { status: Value },

    // ── 插件 ──
    /// 插件列表更新
    PluginList { plugins: Vec<Value> },
    /// 插件诊断信息
    PluginDiagnostics { diagnostics: Vec<Value> },
    /// 插件事件（动态面板、贡献值等）
    PluginEvent {
        plugin_id: String,
        kind: String,
        data: Value,
    },

    // ── 配置 ──
    /// 配置已变更
    ConfigChanged,

    // ── 更新 ──
    /// 更新状态
    UpdateStatus {
        checking: bool,
        available: bool,
        version: Option<String>,
        error: Option<String>,
        progress: Option<f64>,
    },

    // ── 插件市场 ──
    /// 市场事件
    MarketplaceEvent { kind: String, data: Value },
}
