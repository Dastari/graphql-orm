# Migration Guide

## 0.3.x to 0.4.0

The snapshot format and repository key layout are unchanged. Existing local,
S3-backed, and custom repositories can read and write the same snapshots.

### Streaming trait methods

`BackupRepository`, `BackupObjectIndex`, and `RestoreObjectSink` add streaming
methods. They have buffered default implementations, so existing trait
implementations remain source-compatible and do not need immediate changes.

Providers that handle large stored objects should override:

- `BackupRepository::put_blob_stream`
- `BackupRepository::put_blob_stream_if_absent`
- `BackupRepository::get_blob_stream`
- `BackupObjectIndex::load_object_stream`
- `RestoreObjectSink::restore_object_stream`

`BlobStoreBackupRepository` and `BlobStoreRestoreObjectSink` already provide
native streaming overrides. Small manifests and compressed database table
payloads retain their buffered convenience APIs.

### Native SMB repositories

Enable native SMB without the local provider:

```toml
graphql-orm-backup = {
    version = "0.4.0",
    default-features = false,
    features = ["smb"]
}
```

Construct `graphql-orm-storage::SmbStorageBackend`, erase it to
`Arc<dyn BlobStore>`, and pass it to `BlobStoreBackupRepository`. Do not pass a
mount path or UNC string as native SMB configuration. Existing mounted-share
deployments can continue through `LocalBackupRepository`, preferably under an
explicit legacy provider name.

Credentials remain host-owned runtime inputs. No manifest or repository data
migration is required.

### Dependency identity and release order

Storage 0.5.0 must be released or pinned before backup 0.4.0. Applications that
also use `graphql-orm-storage` directly must resolve the same canonical source
and reviewed revision as this crate; otherwise Rust treats the duplicated
`BlobStore` traits as different types.

### Host authorization

No `agql-auth` migration is required. Hosts continue to authorize
configuration, validation, backup, restore, delete, and prune operations and to
provide an internal trusted path for scheduled backups.
