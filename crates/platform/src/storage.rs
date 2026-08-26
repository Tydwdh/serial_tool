//! Platform-neutral settings and file capabilities.
//!
//! `Application` talks in terms of keys, file IDs and byte blobs. Native
//! implementations may use filesystem paths internally; Web implementations
//! do not expose a `PathBuf` to callers.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    native_path: Option<PathBuf>,
}

impl FileHandle {
    pub fn named(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: FileId::new(name.clone()),
            name,
            #[cfg(not(target_arch = "wasm32"))]
            native_path: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
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

    #[cfg(not(target_arch = "wasm32"))]
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

pub trait SettingsStore: Clone + 'static {
    fn load(&self, key: String) -> StorageFuture<Option<Vec<u8>>>;
    fn save(&self, key: String, bytes: Vec<u8>) -> StorageFuture<()>;
    fn remove(&self, key: String) -> StorageFuture<()>;
}

pub trait FileService: Clone + 'static {
    fn read(&self, id: FileId) -> StorageFuture<FileBlob>;
    fn write(&self, id: FileId, blob: FileBlob) -> StorageFuture<()>;
}

#[cfg(not(target_arch = "wasm32"))]
pub mod native {
    use std::path::{Path, PathBuf};
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

    fn ensure_relative(root: &Path, id: &FileId) -> Result<PathBuf, StorageError> {
        let relative = Path::new(id.as_str());
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
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
                Err(error) => Err(StorageError::Operation(error.to_string())),
            })
        }

        fn save(&self, key: String, bytes: Vec<u8>) -> StorageFuture<()> {
            let path = self.path(&key);
            let root = self.root.clone();
            spawn_io(move || {
                std::fs::create_dir_all(root)
                    .map_err(|error| StorageError::Operation(error.to_string()))?;
                let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
                std::fs::write(&temporary, bytes)
                    .map_err(|error| StorageError::Operation(error.to_string()))?;
                match std::fs::rename(&temporary, &path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        // Windows does not replace an existing destination in
                        // rename(). Keep the replace fallback inside the
                        // capability so callers do not need platform checks.
                        std::fs::remove_file(&path)
                            .and_then(|()| std::fs::rename(&temporary, &path))
                            .map_err(|error| StorageError::Operation(error.to_string()))
                    }
                    Err(error) => Err(StorageError::Operation(error.to_string())),
                }
            })
        }

        fn remove(&self, key: String) -> StorageFuture<()> {
            let path = self.path(&key);
            spawn_io(move || match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(StorageError::Operation(error.to_string())),
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
            spawn_io(move || {
                std::fs::write(path, blob.bytes)
                    .map_err(|error| StorageError::Operation(error.to_string()))
            })
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
                let bytes = std::fs::read(&path)
                    .map_err(|error| StorageError::Operation(error.to_string()))?;
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
                    std::fs::create_dir_all(parent)
                        .map_err(|error| StorageError::Operation(error.to_string()))?;
                }
                std::fs::write(path, blob.bytes)
                    .map_err(|error| StorageError::Operation(error.to_string()))
            })
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub mod web {
    use wasm_bindgen::JsValue;

    use super::{FileBlob, FileId, FileService, SettingsStore, StorageError, StorageFuture};

    #[derive(Clone)]
    pub struct WebSettingsStore {
        storage: web_sys::Storage,
        namespace: String,
    }

    impl WebSettingsStore {
        pub fn from_window(namespace: impl Into<String>) -> Result<Self, StorageError> {
            let window = web_sys::window()
                .ok_or_else(|| StorageError::Operation("window unavailable".into()))?;
            let storage = window
                .local_storage()
                .map_err(js_error)?
                .ok_or(StorageError::Unsupported)?;
            Ok(Self {
                storage,
                namespace: namespace.into(),
            })
        }

        fn key(&self, key: &str) -> String {
            format!("{}:{key}", self.namespace)
        }
    }

    fn js_error(error: JsValue) -> StorageError {
        StorageError::Operation(error.as_string().unwrap_or_else(|| format!("{error:?}")))
    }

    impl SettingsStore for WebSettingsStore {
        fn load(&self, key: String) -> StorageFuture<Option<Vec<u8>>> {
            let result = self
                .storage
                .get_item(&self.key(&key))
                .map_err(js_error)
                .and_then(|value| {
                    value
                        .map(|text| {
                            serde_json::from_str::<Vec<u8>>(&text).map_err(|error| {
                                StorageError::Operation(format!("decode settings: {error}"))
                            })
                        })
                        .transpose()
                });
            Box::pin(async move { result })
        }

        fn save(&self, key: String, bytes: Vec<u8>) -> StorageFuture<()> {
            let result = serde_json::to_string(&bytes)
                .map_err(|error| StorageError::Operation(error.to_string()))
                .and_then(|value| {
                    self.storage
                        .set_item(&self.key(&key), &value)
                        .map_err(js_error)
                });
            Box::pin(async move { result })
        }

        fn remove(&self, key: String) -> StorageFuture<()> {
            let result = self.storage.remove_item(&self.key(&key)).map_err(js_error);
            Box::pin(async move { result })
        }
    }

    /// Browser fallback for small user-selected files. The key/value store is
    /// deliberately behind FileService so it can be replaced with OPFS or the
    /// File System Access API without changing Application semantics.
    #[derive(Clone)]
    pub struct WebFileService {
        storage: WebSettingsStore,
    }

    impl WebFileService {
        pub fn new(storage: WebSettingsStore) -> Self {
            Self { storage }
        }
    }

    impl FileService for WebFileService {
        fn read(&self, id: FileId) -> StorageFuture<FileBlob> {
            let future = self.storage.load(id.as_str().to_owned());
            let name = id.as_str().to_owned();
            Box::pin(async move {
                let bytes = future
                    .await?
                    .ok_or_else(|| StorageError::NotFound(name.clone()))?;
                Ok(FileBlob {
                    name,
                    mime: "application/octet-stream".to_owned(),
                    bytes,
                })
            })
        }

        fn write(&self, id: FileId, blob: FileBlob) -> StorageFuture<()> {
            self.storage.save(id.as_str().to_owned(), blob.bytes)
        }
    }
}
