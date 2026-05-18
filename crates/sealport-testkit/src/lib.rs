//! Shared test helpers, fake stores, corruption fixtures, and platform fixtures.

use std::collections::BTreeMap;

use sealport_storage::{
    DeleteCapability, ListingCapability, ObjectKey, ObjectKeyPrefix, ObjectStore, PutStatus,
    StorageCapabilities, StorageError, StorageFuture,
};
use tokio::sync::Mutex;

pub fn temp_repository() -> tempfile::TempDir {
    tempfile::tempdir().expect("create temporary repository directory")
}

#[derive(Debug, Default)]
pub struct FakeObjectStore {
    objects: Mutex<BTreeMap<ObjectKey, Vec<u8>>>,
}

impl FakeObjectStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn object_count(&self) -> usize {
        self.objects.lock().await.len()
    }
}

impl ObjectStore for FakeObjectStore {
    fn capabilities(&self) -> StorageCapabilities {
        StorageCapabilities::in_memory_fake()
    }

    fn put_if_absent<'a>(
        &'a self,
        key: &'a ObjectKey,
        bytes: &'a [u8],
    ) -> StorageFuture<'a, PutStatus> {
        Box::pin(async move {
            let mut objects = self.objects.lock().await;
            if let Some(existing) = objects.get(key) {
                if existing == bytes {
                    Ok(PutStatus::AlreadyPresent)
                } else {
                    Err(StorageError::ObjectAlreadyExists { key: key.clone() })
                }
            } else {
                objects.insert(key.clone(), bytes.to_vec());
                Ok(PutStatus::Created)
            }
        })
    }

    fn get<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.objects
                .lock()
                .await
                .get(key)
                .cloned()
                .ok_or_else(|| StorageError::ObjectNotFound { key: key.clone() })
        })
    }

    fn exists<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, bool> {
        Box::pin(async move { Ok(self.objects.lock().await.contains_key(key)) })
    }

    fn delete<'a>(&'a self, key: &'a ObjectKey) -> StorageFuture<'a, ()> {
        Box::pin(async move {
            self.objects.lock().await.remove(key);
            Ok(())
        })
    }

    fn list_prefix<'a>(&'a self, prefix: &'a ObjectKeyPrefix) -> StorageFuture<'a, Vec<ObjectKey>> {
        Box::pin(async move {
            let objects = self.objects.lock().await;
            Ok(objects
                .keys()
                .filter(|key| {
                    prefix.as_str().is_empty()
                        || key.as_str() == prefix.as_str()
                        || key
                            .as_str()
                            .strip_prefix(prefix.as_str())
                            .is_some_and(|remainder| remainder.starts_with('/'))
                })
                .cloned()
                .collect())
        })
    }
}

pub fn assert_basic_fake_capabilities(capabilities: &StorageCapabilities) {
    assert!(capabilities.conditional_create);
    assert!(capabilities.atomic_visibility);
    assert!(capabilities.strong_read_after_write);
    assert_eq!(capabilities.delete, DeleteCapability::Idempotent);
    assert_eq!(capabilities.listing, ListingCapability::Prefix);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(value: &str) -> ObjectKey {
        ObjectKey::new(value).expect("valid object key")
    }

    #[tokio::test]
    async fn fake_store_round_trips_and_lists_objects() {
        let store = FakeObjectStore::new();
        let first = key("chunks/aa/blob");
        let second = key("indexes/current");

        assert_basic_fake_capabilities(&store.capabilities());
        assert_eq!(
            store.put_if_absent(&first, b"one").await.expect("put"),
            PutStatus::Created
        );
        assert_eq!(
            store.put_if_absent(&second, b"two").await.expect("put"),
            PutStatus::Created
        );
        assert_eq!(store.get(&first).await.expect("get"), b"one");
        assert_eq!(
            store
                .list_prefix(&ObjectKeyPrefix::new("chunks").expect("prefix"))
                .await
                .expect("list"),
            vec![first.clone()]
        );
        assert_eq!(store.object_count().await, 2);
    }

    #[tokio::test]
    async fn fake_store_enforces_immutable_idempotent_puts() {
        let store = FakeObjectStore::new();
        let object = key("manifests/snapshot");

        assert_eq!(
            store.put_if_absent(&object, b"same").await.expect("put"),
            PutStatus::Created
        );
        assert_eq!(
            store
                .put_if_absent(&object, b"same")
                .await
                .expect("same put"),
            PutStatus::AlreadyPresent
        );
        let error = store
            .put_if_absent(&object, b"different")
            .await
            .expect_err("conflict");
        assert!(matches!(error, StorageError::ObjectAlreadyExists { .. }));
    }
}
