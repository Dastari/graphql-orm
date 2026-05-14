use std::sync::Arc;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    StorageBackend, StorageError, StorageObjectBody, StoragePutRequest, StoredObject,
    build_storage_key, file_extension, sha256_hex,
};

/// Provider implementation contract for object storage backends.
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    /// Returns the provider identifier for this backend.
    fn backend(&self) -> StorageBackend;

    /// Persists object bytes and returns stored metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot persist the object.
    async fn put_object(
        &self,
        object: StoredObject,
        bytes: Vec<u8>,
    ) -> Result<StoredObject, StorageError>;

    /// Loads object bytes for existing metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot load the object.
    async fn get_object(&self, object: &StoredObject) -> Result<StorageObjectBody, StorageError>;

    /// Deletes an object from the backend.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot delete the object.
    async fn delete_object(&self, object: &StoredObject) -> Result<(), StorageError>;
}

/// Service that generates object metadata before delegating bytes to a backend.
#[derive(Clone)]
pub struct StorageService {
    backend: Arc<dyn ObjectStorage>,
}

impl StorageService {
    /// Creates a storage service backed by a provider implementation.
    #[must_use]
    pub fn new(backend: Arc<dyn ObjectStorage>) -> Self {
        Self { backend }
    }

    /// Returns the provider identifier for the configured backend.
    #[must_use]
    pub fn backend(&self) -> StorageBackend {
        self.backend.backend()
    }

    /// Stores bytes and returns provider-neutral object metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot persist the object.
    pub async fn put_object(
        &self,
        request: StoragePutRequest,
    ) -> Result<StoredObject, StorageError> {
        let object = build_stored_object(self.backend.backend(), &request);
        self.backend.put_object(object, request.bytes).await
    }

    /// Loads object bytes for existing metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot load the object.
    pub async fn get_object(
        &self,
        object: &StoredObject,
    ) -> Result<StorageObjectBody, StorageError> {
        self.backend.get_object(object).await
    }

    /// Deletes an object from the configured backend.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot delete the object.
    pub async fn delete_object(&self, object: &StoredObject) -> Result<(), StorageError> {
        self.backend.delete_object(object).await
    }
}

fn build_stored_object(backend: StorageBackend, request: &StoragePutRequest) -> StoredObject {
    let object_id = Uuid::new_v4();
    let extension = request.file_name.as_deref().and_then(file_extension);
    let storage_key = build_storage_key(request.namespace, &object_id, extension);

    StoredObject {
        object_id,
        namespace: request.namespace,
        backend,
        storage_key,
        original_file_name: request.file_name.clone(),
        mime_type: request.mime_type.clone(),
        size_bytes: u64::try_from(request.bytes.len()).unwrap_or(u64::MAX),
        sha256_hex: sha256_hex(&request.bytes),
        created_at: OffsetDateTime::now_utc(),
    }
}
