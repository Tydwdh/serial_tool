use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin capability is not available: {0}")]
    UnsupportedCapability(String),
    #[error("plugin permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid plugin value: {0}")]
    InvalidValue(String),
    #[error("plugin runtime error: {0}")]
    Runtime(String),
    #[error("plugin host error: {0}")]
    Host(String),
}

pub type PluginResult<T> = Result<T, PluginError>;
