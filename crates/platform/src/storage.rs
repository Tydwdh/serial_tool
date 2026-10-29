//! Platform-neutral settings and file capabilities.
//!
//! `Application` talks in terms of keys, file IDs and byte blobs. The native
//! implementation uses filesystem paths internally but does not expose a
//! `PathBuf` to callers.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId(String);

impl FileId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A user-selected file without exposing a native path in application
/// commands.  Native creates this from a dialog-selected path; Web creates a
/// name-only handle and supplies the bytes obtained from the browser picker.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileHandle {
    id: FileId,
    name: String,
    native_path: Option<PathBuf>,
}

impl FileHandle {
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: FileId::new(name.clone()),
            name,
            native_path: None,
        }
    }

    pub fn from_native_path(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let name = path.display().to_string();
        Self {
            id: FileId::new(name.clone()),
            name,
            native_path: Some(path),
        }
    }

    pub fn id(&self) -> &FileId {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn native_path(&self) -> Option<&Path> {
        self.native_path.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBlob {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StorageError {
    #[error("storage capability is not supported on this platform")]
    Unsupported,
    #[error("storage key not found: {0}")]
    NotFound(String),
    #[error("storage operation failed: {0}")]
    Operation(String),
}

pub type StorageResult<T> = Result<T, StorageError>;
pub type StorageFuture<T> = Pin<Box<dyn Future<Output = StorageResult<T>> + 'static>>;

/// 把平台存储错误翻译成用户可见的中文提示。
///
/// **与 crate 根那个同名的 `tool_platform::translate_error` 是两份函数、各管一个枚举**
/// （那一份入参是 `TransportError`）：调用点都写全路径，传错枚举编译就不过，所以不合并。
///
/// `Unsupported` 刻意**不共用**传输侧那句「此平台不支持该传输能力」：存储与传输是两种
/// 能力，用户在导出/保存设置时看到「传输能力不支持」只会去怀疑串口（第 22 轮给插件权限
/// 那句立的同一条避让规则）。
///
/// `Operation(detail)` 套**一层**中文框架、原样带出载荷 —— 这里和 transport 侧相反：那一侧
/// 的载荷进门时就已经是 `tool_transport::translate_error` 的结果，再套就成了「操作失败：
/// 串口错误：…」；而本枚举的载荷**没有一个是中文框架**（native：`std::io::Error` 原文、
/// worker 线程退出、file id 越界；wasm：window 取不到、JS 值原文、`decode settings:`、
/// serde 原文），逐条清单与语言记在 `tests/storage_error_wording.rs`。外来细节照旧留着
/// （否则没法排查），中文框架是给用户的契约，与本 crate 其它映射器一致。
pub fn translate_error(err: &StorageError) -> String {
    match err {
        StorageError::Unsupported => "此平台不支持该存储能力".into(),
        StorageError::NotFound(key) => format!("未找到存储项 {key}（它还没有被保存过）"),
        StorageError::Operation(detail) => format!("存储操作失败：{detail}"),
    }
}

pub trait SettingsStore: Clone + 'static {
    fn load(&self, key: String) -> StorageFuture<Option<Vec<u8>>>;
    fn save(&self, key: String, bytes: Vec<u8>) -> StorageFuture<()>;
    fn remove(&self, key: String) -> StorageFuture<()>;
}

pub trait FileService: Clone + 'static {
    fn read(&self, id: FileId) -> StorageFuture<FileBlob>;
    fn write(&self, id: FileId, blob: FileBlob) -> StorageFuture<()>;
}

pub mod native {
    use std::path::{Component, Path, PathBuf};
    use std::thread;

    use futures_channel::oneshot;

    use super::{FileBlob, FileId, FileService, SettingsStore, StorageError, StorageFuture};

    #[derive(Clone)]
    pub struct NativeSettingsStore {
        root: PathBuf,
    }

    impl NativeSettingsStore {
        pub fn new(root: impl Into<PathBuf>) -> Self {
            Self { root: root.into() }
        }

        /// Synchronous bridge for the native bootstrap path. Normal runtime
        /// callers should keep the returned future on an application task.
        pub fn load_blocking(
            &self,
            key: impl Into<String>,
        ) -> Result<Option<Vec<u8>>, StorageError> {
            futures_executor::block_on(self.load(key.into()))
        }

        /// Synchronous bridge for the native bootstrap path. Normal runtime
        /// callers should keep the returned future on an application task.
        pub fn save_blocking(
            &self,
            key: impl Into<String>,
            bytes: Vec<u8>,
        ) -> Result<(), StorageError> {
            futures_executor::block_on(self.save(key.into(), bytes))
        }

        fn path(&self, key: &str) -> PathBuf {
            self.root.join(key)
        }
    }

    fn spawn_io<T, F>(work: F) -> StorageFuture<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, StorageError> + Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        thread::spawn(move || {
            let _ = sender.send(work());
        });
        Box::pin(async move {
            receiver
                .await
                .map_err(|_| StorageError::Operation("native storage worker stopped".into()))?
        })
    }

    fn io_error(error: std::io::Error) -> StorageError {
        StorageError::Operation(error.to_string())
    }

    fn ensure_relative(root: &Path, id: &FileId) -> Result<PathBuf, StorageError> {
        let relative = Path::new(id.as_str());
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(StorageError::Operation(
                "file id escapes storage root".into(),
            ));
        }
        Ok(root.join(relative))
    }

    impl SettingsStore for NativeSettingsStore {
        fn load(&self, key: String) -> StorageFuture<Option<Vec<u8>>> {
            let path = self.path(&key);
            spawn_io(move || match std::fs::read(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(io_error(error)),
            })
        }

        fn save(&self, key: String, bytes: Vec<u8>) -> StorageFuture<()> {
            let path = self.path(&key);
            let root = self.root.clone();
            spawn_io(move || {
                std::fs::create_dir_all(root).map_err(io_error)?;
                let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
                std::fs::write(&temporary, bytes).map_err(io_error)?;
                match std::fs::rename(&temporary, &path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        // Windows does not replace an existing destination in
                        // rename(). Keep the replace fallback inside the
                        // capability so callers do not need platform checks.
                        std::fs::remove_file(&path)
                            .and_then(|()| std::fs::rename(&temporary, &path))
                            .map_err(io_error)
                    }
                    Err(error) => Err(io_error(error)),
                }
            })
        }

        fn remove(&self, key: String) -> StorageFuture<()> {
            let path = self.path(&key);
            spawn_io(move || match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(io_error(error)),
            })
        }
    }

    #[derive(Clone)]
    pub struct NativeFileService {
        root: PathBuf,
    }

    impl NativeFileService {
        pub fn new(root: impl Into<PathBuf>) -> Self {
            Self { root: root.into() }
        }

        /// Write a user-selected native path. The capability keeps this
        /// escape hatch native-only; Application still performs it on a task
        /// worker and Web uses a browser FileService instead.
        pub fn write_path(&self, path: PathBuf, blob: FileBlob) -> StorageFuture<()> {
            spawn_io(move || std::fs::write(path, blob.bytes).map_err(io_error))
        }

        pub fn block_on_write_path(
            &self,
            path: PathBuf,
            blob: FileBlob,
        ) -> Result<(), StorageError> {
            futures_executor::block_on(self.write_path(path, blob))
        }
    }

    impl FileService for NativeFileService {
        fn read(&self, id: FileId) -> StorageFuture<FileBlob> {
            let root = self.root.clone();
            spawn_io(move || {
                let path = ensure_relative(&root, &id)?;
                let bytes = std::fs::read(&path).map_err(io_error)?;
                Ok(FileBlob {
                    name: id.as_str().to_owned(),
                    mime: "application/octet-stream".to_owned(),
                    bytes,
                })
            })
        }

        fn write(&self, id: FileId, blob: FileBlob) -> StorageFuture<()> {
            let root = self.root.clone();
            spawn_io(move || {
                let path = ensure_relative(&root, &id)?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(io_error)?;
                }
                std::fs::write(path, blob.bytes).map_err(io_error)
            })
        }
    }
}
