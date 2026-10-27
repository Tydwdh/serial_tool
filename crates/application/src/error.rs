use thiserror::Error;

/// 每个变体的 `Display` 是「中文分类前缀 + 正文」：正文多半本就中文（`Plugin` 臂来自
/// `tool_extension::translate_error`，`Transport` 臂来自 `SendPlanError`），前缀只补一句
/// 「这是哪一路的失败」。它此前是英文（`transport: ` 一类），夹在 UI 那句中文框架和中文
/// 正文之间，要先读一句英文才看到正文 —— 第 24 轮换掉，逐字见证在
/// `tests/headless.rs::send_routing_dispatches_by_port_kind_and_rejects_invalid_hex`。
///
/// 这一层只有 native 看得到：`AppError` 被 `lib.rs` 的 `cfg(not(target_arch = "wasm32"))`
/// 挡在 wasm 之外，web 侧 `to_string()` 出的是**不带前缀**的正文 —— 换语言并不关掉这个
/// 两端差异，它仍按设计存在（见 `docs/known-gaps.md`「发送路径的残留差异 → 错误文案」）。
#[derive(Debug, Error)]
pub enum AppError {
    #[error("传输错误：{0}")]
    Transport(String),

    #[error("录制错误：{0}")]
    Recording(String),

    #[error("回放错误：{0}")]
    Replay(String),

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
            CommandError::Recording(message) => Self::Recording(message),
            CommandError::Replay(message) => Self::Replay(message),
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
            AppError::Recording(message) => Self::Recording(message),
            AppError::Replay(message) => Self::Replay(message),
            AppError::Storage(message) => Self::Storage(message),
            AppError::Plugin(message) => Self::Plugin(message),
            AppError::InvalidState(message) => Self::InvalidState(message),
            AppError::InvalidCommand(message) => Self::InvalidCommand(message),
        }
    }
}
