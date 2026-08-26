use serde::{Deserialize, Serialize};

/// Opaque file identity exposed to plugins.
///
/// Native maps this handle to a `PathBuf`; Web maps it to a browser `File` or
/// OPFS entry. Plugin code must never need to parse a real path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileHandle(String);

impl FileHandle {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
