# Migration Guide

## 0.5.0 to 0.6.0

The `BlobStore`, `ObjectStorage`, and `StreamingObjectStore` trait contracts are
unchanged. Native SMB consumers should update the crate version:

```toml
graphql-orm-storage = {
    version = "0.6.0",
    default-features = false,
    features = ["smb"]
}
```

Downstream stream chunking workarounds can be removed. In particular, callers
do not need to split `StorageByteStream` items to 1 MiB: the provider now bounds
each SMB request using negotiated connection facts while preserving streaming
backpressure.

`SmbProbeResult` adds `max_read_size`, `max_write_size`,
`max_transact_size`, and `write_request_payload_limit`, and is now
`#[non_exhaustive]`. Code that destructures the result must use `..`. The
existing dialect, signing/encryption, and reachability fields retain their
meanings.

`SmbStorageBackend::diagnostics` is an optional new monitoring surface. It does
not contain endpoints, usernames, passwords, or keys. Structured tracing events
also avoid credentials; applications only need to install a tracing subscriber
if they want those events.

Conditional-create behavior is unchanged: `Ok(None)` still means another writer
won `FILE_CREATE`. A timeout with an unknown conditional-create outcome remains
an error and must not be treated as a collision. If an application retries a
failed streaming upload, it must continue to supply a fresh body.

## 0.4.x to 0.5.0

The default feature set and local filesystem behavior are unchanged. Native
SMB is opt-in:

```toml
graphql-orm-storage = {
    version = "0.5.0",
    default-features = false,
    features = ["smb"]
}
```

### Exhaustive enum matches

`StorageBackend` adds `Smb`. It was already marked `#[non_exhaustive]`, so
external matches should retain a wildcard arm.

`StorageError` adds `RemoteProvider`, carrying a redaction-safe
`StorageProviderErrorKind`, operation name, message, and retry flag. Code that
matches `StorageError` exhaustively inside this crate or through future enum
changes must handle this variant. Prefer `StorageError::is_retryable()` when
the decision does not require the detailed classification.

### Native SMB configuration

Native SMB configuration accepts a server name or IP address and share name;
it intentionally does not accept a UNC path, mapped drive, or local mount path.
Construct `SmbStorageConfig` with a runtime `secrecy::SecretString`. The crate
does not serialize or persist the password.

Port 445, SMB 3.0 minimum, and required signing are secure defaults. Hosts that
previously used `LocalStorageBackend` over a mounted SMB directory can retain
that behavior as an explicitly named legacy provider or migrate to direct SMB
fields. Repository data and blob keys do not need conversion.

### Backup consumers

`graphql-orm-backup` 0.4.0 consumes this release through
`BlobStoreBackupRepository`. Publish or pin storage 0.5.0 before resolving the
backup crate so both the host and backup crate use one `graphql-orm-storage`
crate instance and therefore share identical trait types.
