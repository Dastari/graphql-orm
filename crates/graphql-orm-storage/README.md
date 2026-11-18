---
title: "graphql-orm-storage"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-storage

Version 0.17.0 declares Rust 1.97.1 as the minimum supported compiler. CI verifies
the workspace and explicit backend/provider graphs with Rust 1.97.1.

Windows local storage requires explicit `LocalStorageBackend::new_trusted_windows_root`
construction for a host-protected tree; `new` does not exist on Windows. This mode does
not enforce links as a security boundary. Unix keeps its no-follow boundary.
See the storage [trust boundary documentation](docs/blob-store.md#local-filesystem-trust-boundary).

Provider-neutral object storage primitives for `graphql-orm` applications.
Use it to keep object bytes in a storage backend while your application owns
database metadata, authorization, upload/download routes, and workflow.

It is not an application file service: it defines no tables, GraphQL roots,
tenant model, authorization policy, or HTTP endpoint. It never stores file
bytes in database rows.

The local backend requires a trusted, exclusively managed filesystem root and
restrictive permissions. See the [local filesystem trust boundary](docs/blob-store.md#local-filesystem-trust-boundary)
before using local directories or OS-mounted shares.

Version 0.16.0 resolves every Unix local storage operation relative to held
directory handles without following links, so local storage refuses every
read, write, copy and delete through a filesystem link below its root, even
one swapped in while the operation runs, skips links in listings, and refuses
to read files with more than one hard link. Local
listing pages resume with bounded work and memory and can hold fewer keys
than their limit. An opt-in canonical key mode for SMB and local targets
that ignore case or normalisation (`validate_canonical_blob_key`,
`with_canonical_keys`) refuses non-canonical keys on new writes. It documents
that local and SMB storage ignore `BlobPutOptions::content_type`. See
[MIGRATION.md](MIGRATION.md).

Version 0.15.0 bounds the SMB cleanup that dropped trees, sessions and
connections start: each step finishes or is abandoned within the connection's
timeout and never keeps the session, connection or socket alive. The public
API is unchanged. See [MIGRATION.md](MIGRATION.md).

Version 0.14.0 makes SMB handle close cancel-safe: a close that times out or
is cancelled before its request is sent no longer leaks the server handle for
the rest of the session. The public API is unchanged. See
[MIGRATION.md](MIGRATION.md).

Version 0.13.0 adds `SmbStorageBackend::sweep_temp_files`, which removes
stale SMB temporary upload files during a quiesced maintenance window on its
own connection and reports what it removed, skipped and failed to clean up,
also when it fails (`SmbTempSweepError`). It reads SMB ranges from their
offset, reports last-modified times on S3 and SMB, makes filesystem errors
retryable only for transient kinds, and keeps keys and paths out of every
error message. See [MIGRATION.md](MIGRATION.md).

Version 0.12.0 sends S3 conditional creates as one attempt, so a lost
response is reported as an unknown outcome instead of the writer's own object
appearing taken, and makes S3 conditional creates, and so repository locks,
fail closed until a per-instance probe shows that the provider enforces
`If-None-Match`. It adds an opt-in portable key mode for new writes to SMB
shares on Windows servers (`validate_portable_blob_key`,
`SmbStorageConfig::portable_keys`), and a fixed `InvalidStorageKey` message
that never reveals the key. See [MIGRATION.md](MIGRATION.md).

Version 0.11.0 makes SMB conditional creates publish only complete objects,
holds SMB transfer permits only while a request is in flight, lists only the
SMB directories a prefix can reach, refuses the ineffective
`require_signing = false`, classifies S3 errors from structured metadata with
short messages, and removes local temporary files of abandoned writes. See
[MIGRATION.md](MIGRATION.md).

Version 0.9.3 pins the managed Samba test fixture to a reviewed image
digest and refuses tag-only image overrides; the library is unchanged.

Version 0.9.2 makes the local backend list keys in exact lexicographic key
order, so paged listings no longer skip keys next to sibling names such as
`a-b`, and makes local writes and deletions durable before they return. See
[ordering and durability](docs/blob-store.md#listing-prefixes-order-and-durability).

Version 0.9.1 treats only the backend's own upload temp files as temporary.
The local backend names them `.<uuid>.uploading`, and listings and
`sweep_temp_files` act on that exact form only, so a stored object whose name
ends in `.uploading`, including one stored from a client filename such as
`report.uploading`, is listed and never swept. SMB filters only its own
dot-prefixed temporary names. `file_extension` no longer returns `uploading`.
See the [migration guide](MIGRATION.md) for temporary files left by earlier
local versions.

Version 0.9.0 bounds what an SMB server can make the native client hold or
repeat. Listings have depth, entry, byte, name-length and overall-time limits,
`list_blobs` lists once, and paged listings end; listings fail instead of
returning a partial result; the client drops responses to requests it did not
send, holds a bounded number of early responses, and closes a failed
connection instead of retrying it in a loop. Reconnects refuse a server whose
GUID changed before any credential is sent. DFS referrals and server
notifications are disabled. `SmbStorageConfig` gains `expected_server_guid`
and listing-limit fields. Authentication remains
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

Find the package manifest in the
[workspace package inventory](../../docs/reference/workspace-packages.md)
and the released version and source revision in the
[workspace release manifest](../../docs/operations/release/process.md#release-identities).

Packages are Git-only. Pin the full reviewed monorepo revision, and use that
same revision for every GraphQL ORM companion crate in one application:

Upgrade deliberately: review the target revision's package changelog and
migration guide, update every companion package together, then test the
resolved dependency graph.

```toml
[dependencies]
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>" }
```

For S3-compatible storage without the default local backend:

```toml
graphql-orm-storage = { git = "https://github.com/Dastari/graphql-orm.git", rev = "<reviewed-full-40-character-commit-sha>", default-features = false, features = ["s3"] }
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
NULs, platform prefixes and the reserved `.graphql-orm-storage` namespace are
rejected. Listing prefixes are plain string prefixes on every provider; see
[listing prefixes](docs/blob-store.md#listing-prefixes-order-and-durability). `StorageError::is_retryable()` is a
provider signal, not permission to replay a non-idempotent stream.

For backups, adapt `Arc<dyn BlobStore>` with the companion backup repository;
use atomic `put_blob_if_not_exists` for locks and deduplication.

## Further reading

- [Documentation index](docs/README.md)
- [Usage](docs/usage.md), [streaming](docs/streaming.md), [resumable multipart uploads](docs/multipart.md), and [large-object recording](docs/recording-streams.md)
- [Native SMB setup and safety](docs/native-smb.md)
- [Backup integration](docs/backup-integration.md)
- [Migration guide](MIGRATION.md) and [changelog](CHANGELOG.md)
