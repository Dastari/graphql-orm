//! Provider-neutral object storage primitives for applications using graphql-orm.
//!
//! This crate stores file bytes in an object backend and returns metadata that an
//! application can persist in its own graphql-orm entity.

#[cfg(feature = "azure")]
mod azure;
mod backend;
mod blob;
mod checksum;
mod error;
mod key;
#[cfg(feature = "local")]
mod local;
mod object;
#[cfg(feature = "s3")]
mod s3;
mod service;

#[cfg(feature = "azure")]
pub use azure::{AzureBlobStorageBackend, AzureBlobStorageConfig};
pub use backend::{StorageBackend, StorageNamespace, unsupported_backend};
pub use blob::{
    BlobBody, BlobMetadata, BlobStore, BlobWriteOutcome, BoxedStorageStream, StorageByteStream,
    collect_storage_stream, validate_blob_key,
};
pub use checksum::sha256_hex;
pub use error::StorageError;
pub use key::{build_storage_key, file_extension};
#[cfg(feature = "local")]
pub use local::LocalStorageBackend;
pub use object::{
    StorageObjectBody, StorageObjectStream, StoragePutRequest, StoragePutStreamRequest,
    StoredObject,
};
#[cfg(feature = "s3")]
pub use s3::{S3StorageBackend, S3StorageConfig};
pub use service::{ObjectStorage, StorageService};
