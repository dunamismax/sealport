//! Local and object storage abstractions and backend implementations.

use std::{
    fmt,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
};

use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
};

pub type StorageResult<T> = Result<T, StorageError>;
pub type StorageFuture<'a, T> = Pin<Box<dyn Future<Output = StorageResult<T>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("object key {value:?} is invalid: {reason}")]
    InvalidObjectKey { value: String, reason: &'static str },

    #[error("object {key} already exists with different contents")]
    ObjectAlreadyExists { key: ObjectKey },

    #[error("object {key} was not found")]
    ObjectNotFound { key: ObjectKey },

    #[error("{operation} failed")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },

    #[error("{operation} failed for object {key}")]
    ObjectIo {
        operation: &'static str,
        key: ObjectKey,
        #[source]
        source: io::Error,
    },
}

impl StorageError {
    fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }

    fn object_io(operation: &'static str, key: &ObjectKey, source: io::Error) -> Self {
        if source.kind() == io::ErrorKind::NotFound {
            Self::ObjectNotFound { key: key.clone() }
        } else {
            Self::ObjectIo {
                operation,
                key: key.clone(),
                source,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageCapabilities {
    pub backend: BackendKind,
    pub conditional_create: bool,
    pub atomic_visibility: bool,
    pub strong_read_after_write: bool,
    pub delete: DeleteCapability,
    pub listing: ListingCapability,
}

impl StorageCapabilities {
    pub fn local_filesystem() -> Self {
        Self {
            backend: BackendKind::LocalFilesystem,
            conditional_create: true,
            atomic_visibility: true,
            strong_read_after_write: true,
            delete: DeleteCapability::Idempotent,
            listing: ListingCapability::Prefix,
        }
    }

    pub fn in_memory_fake() -> Self {
        Self {
            backend: BackendKind::InMemoryFake,
            conditional_create: true,
            atomic_visibility: true,
            strong_read_after_write: true,
            delete: DeleteCapability::Idempotent,
            listing: ListingCapability::Prefix,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    LocalFilesystem,
    S3Compatible,
    InMemoryFake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteCapability {
    Unsupported,
    BestEffort,
    Idempotent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListingCapability {
    Unsupported,
    Prefix,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PutStatus {
    Created,
    AlreadyPresent,
}

pub trait ObjectStore: Send + Sync {
    fn capabilities(&self) -> StorageCapabilities;

    fn put_if_absent<'a>(
        &'a self,
        key: &'a ObjectKey,
        bytes: &'a [u8],
    ) -> StorageFuture<'a, PutStatus>;

    fn get<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, Vec<u8>>;

    fn exists<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, bool>;

    fn delete<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, ()>;

    fn list_prefix<'a>(&'a self, prefix: &'a ObjectKeyPrefix) -> StorageFuture<'a, Vec<ObjectKey>>;
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn new(value: impl Into<String>) -> StorageResult<Self> {
        let value = value.into();
        validate_key(&value, false)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn relative_path(&self) -> PathBuf {
        self.0.split('/').collect()
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<&str> for ObjectKey {
    type Error = StorageError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectKeyPrefix(String);

impl ObjectKeyPrefix {
    pub fn root() -> Self {
        Self(String::new())
    }

    pub fn new(value: impl Into<String>) -> StorageResult<Self> {
        let value = value.into();
        validate_key(&value, true)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn relative_path(&self) -> PathBuf {
        self.0.split('/').filter(|part| !part.is_empty()).collect()
    }

    fn contains(&self, key: &ObjectKey) -> bool {
        if self.0.is_empty() {
            return true;
        }

        key.0 == self.0
            || key
                .0
                .strip_prefix(&self.0)
                .is_some_and(|remainder| remainder.starts_with('/'))
    }
}

impl TryFrom<&str> for ObjectKeyPrefix {
    type Error = StorageError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

fn validate_key(value: &str, allow_empty: bool) -> StorageResult<()> {
    if value.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(StorageError::InvalidObjectKey {
                value: value.to_owned(),
                reason: "key must not be empty",
            })
        };
    }

    if value.starts_with('/') || value.ends_with('/') {
        return Err(StorageError::InvalidObjectKey {
            value: value.to_owned(),
            reason: "key must be relative and must not end with a separator",
        });
    }

    if value.contains('\\') {
        return Err(StorageError::InvalidObjectKey {
            value: value.to_owned(),
            reason: "key must use forward slashes",
        });
    }

    for segment in value.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(StorageError::InvalidObjectKey {
                value: value.to_owned(),
                reason: "key segments must not be empty, '.', or '..'",
            });
        }

        if !segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'='))
        {
            return Err(StorageError::InvalidObjectKey {
                value: value.to_owned(),
                reason: "key segments may contain only ASCII letters, digits, '.', '_', '-', or '='",
            });
        }
    }

    Ok(())
}

#[derive(Clone, Debug)]
pub struct LocalStore {
    root: PathBuf,
}

impl LocalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, key: &ObjectKey) -> PathBuf {
        self.root.join(key.relative_path())
    }

    fn prefix_path(&self, prefix: &ObjectKeyPrefix) -> PathBuf {
        self.root.join(prefix.relative_path())
    }

    fn temp_dir(&self) -> PathBuf {
        self.root.join(".sealport-tmp")
    }

    fn temp_path(&self, key: &ObjectKey) -> PathBuf {
        static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        self.temp_dir().join(format!(
            "{}-{}-{id}.part",
            std::process::id(),
            key.as_str().replace('/', "_")
        ))
    }
}

impl ObjectStore for LocalStore {
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities::local_filesystem()
    }

    fn put_if_absent<'a>(
        &'a self,
        key: &'a ObjectKey,
        bytes: &'a [u8],
    ) -> StorageFuture<'a, PutStatus> {
        Box::pin(async move {
            let path = self.object_path(key);
            let temp_path = self.temp_path(key);

            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await.map_err(|source| {
                    StorageError::object_io("create object parent", key, source)
                })?;
            }

            fs::create_dir_all(self.temp_dir())
                .await
                .map_err(|source| StorageError::io("create temporary object directory", source))?;

            let mut temp_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .await
                .map_err(|source| StorageError::io("create temporary object", source))?;
            temp_file
                .write_all(bytes)
                .await
                .map_err(|source| StorageError::io("write temporary object", source))?;
            temp_file
                .sync_all()
                .await
                .map_err(|source| StorageError::io("sync temporary object", source))?;
            drop(temp_file);

            match fs::hard_link(&temp_path, &path).await {
                Ok(()) => {
                    remove_temp(&temp_path).await?;
                    Ok(PutStatus::Created)
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    remove_temp(&temp_path).await?;
                    let existing = read_existing_for_compare(key, &path).await?;
                    if existing == bytes {
                        Ok(PutStatus::AlreadyPresent)
                    } else {
                        Err(StorageError::ObjectAlreadyExists { key: key.clone() })
                    }
                }
                Err(source) => {
                    remove_temp(&temp_path).await?;
                    Err(StorageError::object_io("publish object", key, source))
                }
            }
        })
    }

    fn get<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, Vec<u8>> {
        Box::pin(async move {
            fs::read(self.object_path(key))
                .await
                .map_err(|source| StorageError::object_io("read object", key, source))
        })
    }

    fn exists<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, bool> {
        Box::pin(async move {
            match fs::metadata(self.object_path(key)).await {
                Ok(metadata) => Ok(metadata.is_file()),
                Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(source) => Err(StorageError::object_io("stat object", key, source)),
            }
        })
    }

    fn delete<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            match fs::remove_file(self.object_path(key)).await {
                Ok(()) => Ok(()),
                Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(StorageError::object_io("delete object", key, source)),
            }
        })
    }

    fn list_prefix<'a>(&'a self, prefix: &'a ObjectKeyPrefix) -> StorageFuture<'a, Vec<ObjectKey>> {
        Box::pin(async move {
            let root = self.root.clone();
            let start = self.prefix_path(prefix);
            let mut output = Vec::new();

            match fs::metadata(&start).await {
                Ok(metadata) if metadata.is_file() => {
                    push_object_path(&root, &start, &mut output)?;
                }
                Ok(metadata) if metadata.is_dir() => {
                    collect_files(&root, &start, &mut output).await?;
                }
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => return Err(StorageError::io("stat prefix", source)),
            }

            output
                .retain(|key| prefix.contains(key) && !key.as_str().starts_with(".sealport-tmp/"));
            output.sort();
            Ok(output)
        })
    }
}

async fn remove_temp(path: &Path) -> StorageResult<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StorageError::io("remove temporary object", source)),
    }
}

async fn read_existing_for_compare(key: &ObjectKey, path: &Path) -> StorageResult<Vec<u8>> {
    fs::read(path)
        .await
        .map_err(|source| StorageError::object_io("read existing object", key, source))
}

async fn collect_files(
    root: &Path,
    directory: &Path,
    output: &mut Vec<ObjectKey>,
) -> StorageResult<()> {
    let mut stack = vec![directory.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = fs::read_dir(&current)
            .await
            .map_err(|source| StorageError::io("read object directory", source))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|source| StorageError::io("read object directory entry", source))?
        {
            let path = entry.path();
            let file_type = entry
                .file_type()
                .await
                .map_err(|source| StorageError::io("read object file type", source))?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                push_object_path(root, &path, output)?;
            }
        }
    }

    Ok(())
}

fn push_object_path(root: &Path, path: &Path, output: &mut Vec<ObjectKey>) -> StorageResult<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| StorageError::InvalidObjectKey {
            value: path.display().to_string(),
            reason: "object path is outside the storage root",
        })?;
    let key = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    output.push(ObjectKey::new(key)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> ObjectKey {
        ObjectKey::new(value).expect("valid object key")
    }

    #[test]
    fn object_keys_reject_path_escape_and_platform_separators() {
        for invalid in [
            "",
            "/chunks/a",
            "chunks/",
            "chunks//a",
            "chunks/../a",
            "chunks\\a",
        ] {
            assert!(ObjectKey::new(invalid).is_err(), "{invalid:?}");
        }

        assert_eq!(
            key("chunks/ab/cd.ef_01-02=03").as_str(),
            "chunks/ab/cd.ef_01-02=03"
        );
    }

    #[tokio::test]
    async fn local_store_put_get_list_and_delete_round_trip() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = LocalStore::new(temp.path());
        let object = key("chunks/aa/blob");

        assert_eq!(
            store.capabilities(),
            StorageCapabilities::local_filesystem()
        );
        assert!(!store.exists(&object).await.expect("exists"));
        assert_eq!(
            store.put_if_absent(&object, b"sealed").await.expect("put"),
            PutStatus::Created
        );
        assert!(store.exists(&object).await.expect("exists"));
        assert_eq!(store.get(&object).await.expect("get"), b"sealed");
        assert_eq!(
            store
                .list_prefix(&ObjectKeyPrefix::new("chunks").expect("prefix"))
                .await
                .expect("list"),
            vec![object.clone()]
        );

        store.delete(&object).await.expect("delete");
        store
            .delete(&object)
            .await
            .expect("delete remains idempotent");
        assert!(!store.exists(&object).await.expect("exists"));
    }

    #[tokio::test]
    async fn local_store_put_if_absent_is_idempotent_for_same_bytes() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = LocalStore::new(temp.path());
        let object = key("indexes/current");

        assert_eq!(
            store.put_if_absent(&object, b"index").await.expect("put"),
            PutStatus::Created
        );
        assert_eq!(
            store
                .put_if_absent(&object, b"index")
                .await
                .expect("put again"),
            PutStatus::AlreadyPresent
        );
        assert_eq!(store.get(&object).await.expect("get"), b"index");
    }

    #[tokio::test]
    async fn local_store_rejects_conflicting_immutable_write() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = LocalStore::new(temp.path());
        let object = key("manifests/snap");

        store.put_if_absent(&object, b"first").await.expect("put");
        let error = store
            .put_if_absent(&object, b"second")
            .await
            .expect_err("conflict");
        assert!(matches!(error, StorageError::ObjectAlreadyExists { .. }));
        assert_eq!(store.get(&object).await.expect("get"), b"first");
    }

    #[tokio::test]
    async fn local_store_ignores_leftover_temporary_objects() {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = LocalStore::new(temp.path());
        let object = key("chunks/bb/blob");
        let temp_dir = store.temp_dir();

        fs::create_dir_all(&temp_dir).await.expect("temp dir");
        fs::write(temp_dir.join("interrupted.part"), b"partial")
            .await
            .expect("write temp");
        store
            .put_if_absent(&object, b"complete")
            .await
            .expect("put");

        assert_eq!(
            store
                .list_prefix(&ObjectKeyPrefix::root())
                .await
                .expect("list"),
            vec![object]
        );
    }
}
