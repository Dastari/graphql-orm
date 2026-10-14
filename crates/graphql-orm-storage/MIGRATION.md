---
title: "Migration Guide"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-08-01
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

## 0.6.2 to 0.7.0: atomic conditional creation

This is a breaking pre-1.0 release. Custom `BlobStore` providers must override
`put_blob_if_not_exists` with an atomic create-if-absent operation across all
writers. The default now returns the new non-retryable
`StorageError::UnsupportedOperation` without provider I/O. Update exhaustive
matches on `StorageError` for the added variant. An existence check followed by
a write does not meet the conditional-create contract.

The new `BlobStore::delete_blob_once` capability also defaults to
`UnsupportedOperation` without I/O. Custom providers used for backup locks must
explicitly implement it without replaying deletion by key after the server may
have applied the mutation, including after cancellation or a lost response.
Ordinary idempotent deletion with retries is insufficient when the key can be
reused immediately. Native local deletion uses one filesystem removal, native
S3 disables SDK operation retries, and native SMB never reopens the key after
mutation may have applied. Ordinary `delete_blob` behavior is unchanged.
A failure/cancellation leaves an unknown outcome; do not retry or reclaim until
all writers and in-flight requests are quiesced and no handles remain.

Local, SMB and S3 retain their native atomic creation overrides. The Azure placeholder
continues to report unsupported backend. `graphql-orm-backup` 0.8.0 uses this
contract for fail-closed writer locks; custom providers inheriting the default
can no longer create an unsafe lock through the adapter. Follow backup's
migration guide for coordinated upgrades and offline lock recovery.

Provider configuration, storage keys and stored data are unchanged. No data
migration is required. Update all selected workspace packages to one reviewed
revision and regenerate `Cargo.lock`.

## 0.6.1 to 0.6.2

Version 0.6.2 refreshes maintained workspace guidance and establishes a new
immutable package tag for the changed package source tree. The public API,
provider configuration, storage-key layout, persisted metadata, and feature
defaults are unchanged. No data migration is required. Git consumers should
move every selected monorepo package to the same reviewed full revision and
regenerate `Cargo.lock`.

## 0.6.0 to 0.6.1

Version 0.6.1 publishes the maintained documentation and examples and aligns
the workspace guidance with `graphql-orm` 0.22.0 and
`graphql-orm-backup` 0.7.1. The public API, provider configuration, storage-key
layout, and persisted metadata are unchanged. No data migration is required;
Git consumers should update every selected monorepo package to the same
reviewed full revision and regenerate `Cargo.lock`.

## Repository consolidation

The source repository is now
`https://github.com/Dastari/graphql-orm.git`. Git consumers should change the
URL, retain `version = "0.6.0"`, and pin the same reviewed full monorepo
revision as any direct `graphql-orm`, `graphql-orm-backup`, or
`graphql-orm-ai` dependencies.

This is a source-layout change only. It does not change storage keys, provider
configuration, persisted metadata, or the Rust API, and requires no data
migration. Regenerate `Cargo.lock`, confirm the old storage repository URL is
absent, and rerun the selected provider checks.

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
