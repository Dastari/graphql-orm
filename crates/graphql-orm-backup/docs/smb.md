---
title: "Native and Mounted SMB Repositories"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Native and Mounted SMB Repositories

Native SMB uses `graphql-orm-storage::SmbStorageBackend` through
`BlobStoreBackupRepository`. Backup orchestration contains no SMB transport
code, and the snapshot layout is unchanged.

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

## Mounted SMB legacy mode

An OS-mounted share can remain a separately named legacy provider using
`LocalBackupRepository::open_existing`. The deployment then owns mount creation,
credentials, reconnect behavior, permissions, and capacity monitoring.

Local repository key validation still rejects empty/absolute keys, empty
segments, `.`, `..`, backslashes, NUL, and platform prefixes. Mounted storage
must reliably support same-directory temporary write and rename semantics.
