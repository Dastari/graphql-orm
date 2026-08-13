---
title: "graphql-orm-storage"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-storage

Provider-neutral object storage primitives for `graphql-orm` applications.
Use it to keep object bytes in a storage backend while your application owns
database metadata, authorization, upload/download routes, and workflow.

It is not an application file service: it defines no tables, GraphQL roots,
tenant model, authorization policy, or HTTP endpoint. It never stores file
bytes in database rows.

## Install

Packages are Git-only. Pin the full reviewed monorepo revision, and use that
same revision for every GraphQL ORM companion crate in one application:

Upgrade deliberately: review the target revision's package changelog and
migration guide, update every companion package together, then test the
resolved dependency graph.

```toml
[dependencies]
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.6.1" }
```

For S3-compatible storage without the default local backend:

```toml
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.6.1", default-features = false, features = ["s3"] }
```

## Five-minute local start

`local` is enabled by default. The service generates an object ID, safe
sharded key, size, SHA-256 checksum, and timestamp; persist that returned
metadata in a host-owned entity. The canonical runnable source is
[`examples/local_storage.rs`](examples/local_storage.rs):

```sh
cargo run -p graphql-orm-storage --example local_storage
```

```rust,no_run
use std::sync::Arc;
use graphql_orm_storage::{LocalStorageBackend, StorageNamespace, StoragePutRequest, StorageService};

# async fn example() -> Result<(), graphql_orm_storage::StorageError> {
let service = StorageService::new(Arc::new(LocalStorageBackend::new("./data/storage")));
let object = service.put_object(StoragePutRequest {
    namespace: StorageNamespace::Originals,
    file_name: Some("note.txt".into()),
    mime_type: Some("text/plain".into()),
    bytes: b"hello".to_vec(),
}).await?;
println!("{} {}", object.storage_key, object.sha256_hex);
# Ok(())
# }
```

Use [`BlobStore`](docs/blob-store.md) instead when the caller owns safe keys
and needs streaming, ranges, conditional writes, copy, or paging.

## Providers and features

| Feature | Default | Provider | Notes |
| --- | --- | --- | --- |
| `local` | Yes | `LocalStorageBackend` | Baseline filesystem provider. |
| `s3` | No | `S3StorageBackend` | S3-compatible endpoints, including path-style MinIO setups. |
| `smb` | No | `SmbStorageBackend` | Native SMB2/SMB3; credentials are runtime-only. |
| `azure` | No | `AzureBlobStorageConfig` | Explicit placeholder; returns `UnsupportedBackend`. |

There is no implicit credential lookup. Keep S3 and SMB secrets out of config
files, logs, metadata, and error reports. Azure is not a supported provider.

## Configuration and operations

The [configuration and limits reference](docs/configuration.md) lists every
public provider and request option, including source-backed defaults. Blob keys
are relative `/`-separated identifiers: traversal, absolute paths, backslashes,
NULs, and platform prefixes are rejected. `StorageError::is_retryable()` is a
provider signal, not permission to replay a non-idempotent stream.

For backups, adapt `Arc<dyn BlobStore>` with the companion backup repository;
use atomic `put_blob_if_not_exists` for locks and deduplication.

## Further reading

- [Documentation index](docs/README.md)
- [Usage](docs/usage.md), [streaming](docs/streaming.md), and [large-object recording](docs/recording-streams.md)
- [Native SMB setup and safety](docs/native-smb.md)
- [Backup integration](docs/backup-integration.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
