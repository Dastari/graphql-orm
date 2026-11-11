---
title: "graphql-orm-storage"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-11
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

The local backend requires a trusted, exclusively managed filesystem root and
restrictive permissions. See the [local filesystem trust boundary](docs/blob-store.md#local-filesystem-trust-boundary)
before using local directories or OS-mounted shares.

Version 0.9.0 bounds what an SMB server can make the native client hold or
repeat. Listings have depth, entry and overall-time limits and fail instead of
returning a partial result; the client drops responses to requests it did not
send, holds a bounded number of early responses, and closes a failed
connection instead of retrying it in a loop. Reconnects refuse a server whose
GUID changed before any credential is sent. `SmbStorageConfig` gains
`expected_server_guid` and listing-limit fields. Authentication remains
NTLM-only; use a long, random, single-purpose service-account password. See
[server identity](docs/native-smb.md#server-identity),
[resource bounds](docs/native-smb.md#resource-bounds) and the
[migration guide](MIGRATION.md).

Version 0.8.1 fixes [GHSA-439h-2cx8-xwgx](https://github.com/Dastari/graphql-orm/security/advisories/GHSA-439h-2cx8-xwgx):
the native SMB client now verifies signing or encryption before it acts on a
response status, and listings fail closed instead of skipping a directory they
cannot open. See [response verification](docs/native-smb.md#response-verification).

Version 0.8.0 adds resumable multipart uploads: `MultipartBlobStore` on local
and S3 storage, with persistable handles and receipts, list, complete and abort.
SMB and Azure return `UnsupportedOperation`. `StorageError` gains the
`Multipart` variant. See [multipart uploads](docs/multipart.md) and the
[migration guide](MIGRATION.md).

Version 0.7.5 retains the SMB 0.10.2 dependency family with a narrow,
owner-local directory-stream backport. See the [native SMB reference](docs/native-smb.md#client-selection)
and [migration guide](MIGRATION.md) before updating a consuming lockfile.

## Install

Packages are Git-only. Pin the full reviewed monorepo revision, and use that
same revision for every GraphQL ORM companion crate in one application:

Upgrade deliberately: review the target revision's package changelog and
migration guide, update every companion package together, then test the
resolved dependency graph.

```toml
[dependencies]
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.8.1" }
```

For S3-compatible storage without the default local backend:

```toml
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", version = "0.8.1", default-features = false, features = ["s3"] }
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
| `local` | Yes | `LocalStorageBackend` | Baseline filesystem provider. Resumable multipart uploads. |
| `s3` | No | `S3StorageBackend` | S3-compatible endpoints, including path-style MinIO setups. Resumable multipart uploads. |
| `smb` | No | `SmbStorageBackend` | Native SMB2/SMB3; credentials are runtime-only. No multipart uploads. |
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
- [Usage](docs/usage.md), [streaming](docs/streaming.md), [resumable multipart uploads](docs/multipart.md), and [large-object recording](docs/recording-streams.md)
- [Native SMB setup and safety](docs/native-smb.md)
- [Backup integration](docs/backup-integration.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
