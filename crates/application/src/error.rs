use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("transport: {0}")]
    Transport(String),

    #[error("recording: {0}")]
    Recording(String),

    #[error("replay: {0}")]
    Replay(String),

    #[error("storage: {0}")]
    Storage(String),

    #[error("plugin: {0}")]
    Plugin(String),

    #[error("invalid state: {0}")]
    InvalidState(String),

    #[error("invalid command: {0}")]
    InvalidCommand(String),
}
