#![warn(missing_docs)]

//! Provider-neutral object storage primitives for applications using
//! `graphql-orm`.
//!
//! This crate stores file bytes in an object backend and returns metadata that an
//! application can persist in its own `graphql-orm` entity.
//!
//! # Boundaries
//!
//! `graphql-orm-storage` does not provide GraphQL upload/download resolvers,
//! application authorization, database entities, MIME sniffing, derivative
//! generation, or file bytes stored in database rows.
//!
//! Use [`StorageService`] for primary object workflows that need generated
//! object metadata. Use [`StreamingObjectStore`] for bucket/key workloads such
//! as large recordings. Use [`BlobStore`] for lower-level key-addressed blob
//! operations, such as backup repository adapters.
//!
//! [`BlobStore::observe_capacity`] reports a bounded [`CapacityObservation`]
//! of the capacity the authenticated caller may still allocate at the exact
//! configured destination.
//!
//! # Example
//!
//! ```
//! use graphql_orm_storage::{StorageByteStream, sha256_hex, validate_blob_key};
//!
//! validate_blob_key("objects/sha256/aa/bb/hash")?;
//! assert_eq!(sha256_hex(b"bytes").len(), 64);
//!
//! let stream = StorageByteStream::from_bytes(b"bytes".to_vec());
//! assert_eq!(stream.size_hint(), Some(5));
//! # Ok::<(), graphql_orm_storage::StorageError>(())
//! ```

#[cfg(feature = "azure")]
mod azure;
mod backend;
mod blob;
mod capacity;
mod checksum;
mod error;
mod key;
#[cfg(feature = "local")]
mod local;
#[cfg(feature = "local")]
mod local_capacity;
mod object;
#[cfg(feature = "s3")]
mod s3;
mod service;
#[cfg(feature = "smb")]
mod smb;
mod streaming_object;

#[cfg(feature = "azure")]
pub use azure::{AzureBlobStorageBackend, AzureBlobStorageConfig};
pub use backend::{StorageBackend, StorageNamespace, unsupported_backend};
pub use blob::{
    BlobBody, BlobListPage, BlobMetadata, BlobPutOptions, BlobStore, BlobWriteOutcome,
    BoxedStorageStream, StorageByteStream, collect_storage_stream, validate_blob_key,
};
pub use capacity::{
    CapacityBinding, CapacityObservation, CapacityPlacement, CapacityQuota, CapacityRequest,
    CapacityState, CapacityUnknownReason, CapacityValidationError, DEFAULT_CAPACITY_VALIDITY,
    KnownCapacity, MAX_CAPACITY_VALIDITY, ProviderManagedBasis, ProviderManagedCapacity,
    QuotaLimit, QuotaScope, UnknownCapacity,
};
pub use checksum::sha256_hex;
pub use error::{StorageError, StorageProviderErrorKind};
pub use key::{build_storage_key, file_extension};
#[cfg(feature = "local")]
pub use local::LocalStorageBackend;
pub use object::{
    StorageObjectBody, StorageObjectStream, StoragePutRequest, StoragePutStreamRequest,
    StoredObject,
};
#[cfg(feature = "s3")]
pub use s3::{S3CapacityMode, S3StorageBackend, S3StorageConfig};
pub use service::{ObjectStorage, StorageService};
#[cfg(feature = "smb")]
pub use smb::{
    SmbBackendDiagnostics, SmbDialect, SmbProbeOptions, SmbProbeResult, SmbStorageBackend,
    SmbStorageConfig,
};
pub use streaming_object::{
    BoxedMultipartWriter, MultipartWriter, ObjectContentRange, ObjectInfo, ObjectMetadata,
    ObjectRangeBody, StreamingObjectStore, validate_object_bucket,
};
