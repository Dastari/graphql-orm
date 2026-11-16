---
title: "Native and Mounted SMB Repositories"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Native and Mounted SMB Repositories

Native SMB uses `graphql-orm-storage::SmbStorageBackend` through
`BlobStoreBackupRepository`. Backup orchestration contains no SMB transport
code. Snapshot and repository-identity layouts are provider-independent.
Format 3 requires a host-held repository UUID pin on the authenticator; retain
it outside the share and preserve it when moving the whole repository. See
[identity enrollment and legacy restore](../MIGRATION.md).

```rust,no_run
use std::sync::Arc;
use graphql_orm_backup::{BackupRepository, BlobStoreBackupRepository, ManifestAuthenticator};
use graphql_orm_storage::{BlobStore, SmbStorageBackend, SmbStorageConfig};
use secrecy::SecretString;

# async fn example(
#     authenticator: ManifestAuthenticator,
# ) -> Result<Arc<dyn BackupRepository>, Box<dyn std::error::Error>> {
let config = SmbStorageConfig::new(
    "files.example.org",
    "backups",
    "backup-service",
    SecretString::from("runtime secret"),
);
let store: Arc<dyn BlobStore> = Arc::new(SmbStorageBackend::connect(config).await?);
Ok(Arc::new(
    BlobStoreBackupRepository::new(store).with_manifest_authenticator(authenticator),
))
# }
```

Enable the `smb` feature. Backup 0.7.0 uses the workspace storage 0.6.0
revision so downstream builds use the implementation exercised by this
release.

See the repository [migration guide](../MIGRATION.md) for streaming trait
compatibility, mounted-provider migration choices, and dependency-source
identity requirements.

Native SMB supports create, list, verify, restore, delete, prune, and locking
through provider-independent APIs. Normal writes use remote temporary files,
flush, close, and rename. Locks use `BlobStore::put_blob_if_not_exists`,
which publishes a complete temporary file as a hard link at the lock key.
Creating the link fails atomically when the lock exists, so the share must
support hard links.

Large referenced objects use streaming extensions on `BackupRepository`,
`BackupObjectIndex`, and `RestoreObjectSink`. Existing implementations remain
source-compatible through buffered defaults. Small manifests and compressed
table payloads remain buffered convenience values.

Credentials are runtime inputs. Neither reusable crate persists them or writes
them to manifests, locks, diagnostics, logs, or configuration exports.
Application authorization remains a host responsibility.

## Key case

Repository keys are lowercase ASCII (`snapshots/<uuid>/…`, `objects/sha256/…`,
`locks/repository.lock`, `repository-id`) except the table name in each
snapshot's table and change export keys, which is used as declared. Two
tables whose names differ only in case would share one export file on a
server that ignores case, as Samba and Windows do by default. To refuse such
writes, connect with `SmbStorageBackend::with_canonical_keys(true)`; a backup
of a table whose name is not lowercase ASCII then fails with
`InvalidStorageKey` before writing it. See the storage
[native SMB guide](../../graphql-orm-storage/docs/native-smb.md). The mode is
off by default. Objects restored to a native SMB store keep no content type,
since SMB files have none; see [object restore](restore-semantics.md#object-restore).

## Mounted SMB legacy mode

An OS-mounted share can remain a separately named legacy provider using
`LocalBackupRepository::open_existing` (on Windows,
`open_existing_trusted_windows_root`, which does not enforce links). The deployment then owns mount creation,
credentials, reconnect behavior, permissions, and capacity monitoring.

Local repository key validation still rejects empty/absolute keys, empty
segments, `.`, `..`, backslashes, NUL, and platform prefixes. Mounted storage
must reliably support same-directory temporary write and rename semantics.
