use thiserror::Error;

/// 每个变体的 `Display` 是「中文分类前缀 + 正文」：正文多半本就中文（`Plugin` 臂来自
/// `tool_extension::translate_error`，`Transport` 臂来自 `SendPlanError`），前缀只补一句
/// 「这是哪一路的失败」。它此前是英文（`transport: ` 一类），夹在 UI 那句中文框架和中文
/// 正文之间，要先读一句英文才看到正文 —— 第 24 轮换掉，逐字见证在
/// `tests/headless.rs::send_routing_dispatches_by_port_kind_and_rejects_invalid_hex`。
///
/// `AppError` 是 native 组合根的错误分类层：命令层的正文由它加上
/// 「传输错误」「存储错误」等前缀后再展示。
#[derive(Debug, Error)]
pub enum AppError {
    #[error("传输错误：{0}")]
    Transport(String),

    #[error("存储错误：{0}")]
    Storage(String),

    #[error("插件错误：{0}")]
    Plugin(String),

    #[error("当前状态不允许：{0}")]
    InvalidState(String),

    #[error("命令不支持：{0}")]
    InvalidCommand(String),
}

impl From<crate::command::CommandError> for AppError {
    fn from(error: crate::command::CommandError) -> Self {
        use crate::command::CommandError;
        match error {
            CommandError::Transport(message) => Self::Transport(message),
            CommandError::Storage(message) => Self::Storage(message),
            CommandError::Plugin(message) => Self::Plugin(message),
            CommandError::InvalidState(message) => Self::InvalidState(message),
            CommandError::InvalidCommand(message) => Self::InvalidCommand(message),
        }
    }
}

impl From<AppError> for crate::command::CommandError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::Transport(message) => Self::Transport(message),
            AppError::Storage(message) => Self::Storage(message),
            AppError::Plugin(message) => Self::Plugin(message),
            AppError::InvalidState(message) => Self::InvalidState(message),
            AppError::InvalidCommand(message) => Self::InvalidCommand(message),
        }
    }
}
