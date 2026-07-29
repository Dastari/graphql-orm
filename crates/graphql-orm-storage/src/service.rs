use std::{ops::Range, sync::Arc};

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    BlobMetadata, BlobPutOptions, BlobStore, StorageBackend, StorageByteStream, StorageError,
    StorageObjectBody, StorageObjectStream, StoragePutRequest, StoragePutStreamRequest,
    StoredObject, build_storage_key, collect_storage_stream, file_extension,
};

/// Provider implementation contract for object storage backends.
#[async_trait]
pub trait ObjectStorage: BlobStore {
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
        self.put_object_stream(StoragePutStreamRequest {
            namespace: request.namespace,
            file_name: request.file_name,
            mime_type: request.mime_type,
            body: StorageByteStream::from_bytes(request.bytes),
        })
        .await
    }

    /// Stores streaming bytes and returns provider-neutral object metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot persist the object.
    pub async fn put_object_stream(
        &self,
        request: StoragePutStreamRequest,
    ) -> Result<StoredObject, StorageError> {
        let mut object = build_stream_stored_object(self.backend.backend(), &request);
        let options = BlobPutOptions {
            content_type: request.mime_type.clone(),
        };
        let outcome = self
            .backend
            .put_blob(&object.storage_key, request.body, options)
            .await?;
        object.size_bytes = outcome.size_bytes;
        object.sha256_hex = outcome.sha256_hex;
        Ok(object)
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
        let object_stream = self.get_object_stream(object).await?;
        let bytes = collect_storage_stream(object_stream.body).await?;
        Ok(StorageObjectBody {
            object: object_stream.object,
            bytes: bytes.to_vec(),
        })
    }

    /// Loads a streaming object body for existing metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot load the object.
    pub async fn get_object_stream(
        &self,
        object: &StoredObject,
    ) -> Result<StorageObjectStream, StorageError> {
        let body = self.backend.get_blob(&object.storage_key).await?;
        Ok(StorageObjectStream {
            object: object.clone(),
            body: body.body,
        })
    }

    /// Loads a byte range as a streaming object body for existing metadata.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot load the object range.
    pub async fn get_object_range_stream(
        &self,
        object: &StoredObject,
        range: Range<u64>,
    ) -> Result<StorageObjectStream, StorageError> {
        let body = self
            .backend
            .get_blob_range(&object.storage_key, range)
            .await?;
        Ok(StorageObjectStream {
            object: object.clone(),
            body: body.body,
        })
    }

    /// Deletes an object from the configured backend.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot delete the object.
    pub async fn delete_object(&self, object: &StoredObject) -> Result<(), StorageError> {
        self.backend.delete_object(object).await
    }

    /// Checks whether an object's backend blob exists.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot check the object.
    pub async fn object_exists(&self, object: &StoredObject) -> Result<bool, StorageError> {
        self.backend.blob_exists(&object.storage_key).await
    }

    /// Loads backend metadata for an object's blob.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the backend cannot load metadata.
    pub async fn object_backend_metadata(
        &self,
        object: &StoredObject,
    ) -> Result<Option<BlobMetadata>, StorageError> {
        self.backend.head_blob(&object.storage_key).await
    }
}

fn build_stream_stored_object(
    backend: StorageBackend,
    request: &StoragePutStreamRequest,
) -> StoredObject {
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
        size_bytes: 0,
        sha256_hex: String::new(),
        created_at: OffsetDateTime::now_utc(),
    }
}
