# graphql-orm-storage Architecture

## Boundary

`graphql-orm-storage` owns object bytes and object locators. It does not own
database rows. This keeps the crate usable by any application that wants to
persist storage metadata differently.

## BlobStore Boundary

`BlobStore` is the low-level key-addressed storage abstraction. It stores and
loads safe relative blob keys, supports streaming bodies, and exposes existence,
metadata, listing, and delete operations.

`StorageService` is the high-level primary object workflow. It generates object
IDs, namespaces, storage keys, byte counts, checksums, and timestamps before
applications persist metadata in their own database rows.

`StreamingObjectStore` is the bucket/key workflow for large objects such as
recordings. It supports multipart writes, atomic visibility after completion,
caller metadata, range reads, listing, and retention deletion without requiring
the caller to buffer a full object in memory.

`graphql-orm-backup` should reuse future cloud provider implementations through
a `BlobStore` adapter, not through `StorageService`.

## GraphQL Resolver Boundary

The core crate should not provide default GraphQL upload, download, delete, or metadata mutation resolvers.

Reason: storage authorization is application-specific. Host applications often
combine:

- `graphql-orm` read/write policy names on metadata entities
- application row-policy checks
- tenant, project, or ownership checks
- administrator bypass rules
- route-level bearer-token validation for file download
- route-level upload checks before bytes are accepted

A generic crate cannot safely know these rules. Shipping generic resolvers that only check "is authenticated" would be too weak for multi-tenant or collection-scoped applications.

Future GraphQL support should be optional and should expose resolver building blocks, not ready-to-use unaudited endpoints. Any resolver helper must require the host application to provide an authorization adapter.

Suggested future shape:

```rust
#[async_trait::async_trait]
pub trait StorageAccessPolicy<Context, Metadata>: Send + Sync {
    async fn can_upload(
        &self,
        context: &Context,
        scope: &StorageUploadScope,
    ) -> Result<bool, StorageError>;

    async fn can_read(
        &self,
        context: &Context,
        metadata: &Metadata,
    ) -> Result<bool, StorageError>;

    async fn can_delete(
        &self,
        context: &Context,
        metadata: &Metadata,
    ) -> Result<bool, StorageError>;
}
```

The host app should still own:

- the `graphql-orm` storage metadata entity
- application-specific policy names
- row ownership checks
- upload/download HTTP routes or GraphQL mutation wrappers
- audit logging

## Data Flow

1. Caller provides `StoragePutRequest`.
2. `StorageService` generates a UUID object ID.
3. `StorageService` creates a sharded storage key.
4. `StorageService` delegates byte persistence to `BlobStore`.
5. Backend writes bytes and returns size plus SHA-256.
6. `StorageService` returns the `StoredObject`.
7. Caller persists returned metadata in its own database transaction.

## Object Key Safety

The original filename is metadata only. The key generator copies only a sanitized extension. The local backend validates every `storage_key` before joining it with the root path:

- no absolute paths
- no `..`
- no `.`
- no platform prefix components
- only normal path components

## Error Model

The crate uses `StorageError` through `thiserror`. Application code can convert this into its own API or GraphQL error types.

## Provider Features

Provider-specific code should live behind cargo features:

- `local`: default, implemented now
- `s3`: implemented with `aws-sdk-s3`
- `azure`: reserved placeholder

Provider implementations must satisfy the same `BlobStore` trait. High-level
`ObjectStorage` behavior should delegate to the provider's `BlobStore`
implementation.
