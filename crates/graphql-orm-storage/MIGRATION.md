---
title: "Migration Guide"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-10-15
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

## 0.7.4 to 0.7.5: maintained protocol-validation package identity

Update the reviewed workspace Git revision and regenerate the consuming
application's lockfile. This patch gives the maintained Samba fixtures and
protocol-validation scripts a new immutable package identity; Rust code, public
APIs, provider behavior, features and external dependencies are unchanged.
No provider configuration, object key, schema or persisted-data migration is
required. Keep all companion packages on the same reviewed revision.

## 0.7.3 to 0.7.4: narrow SMB directory-stream backport

Update the reviewed workspace Git revision and regenerate the consuming
application's lockfile. The `smb` dependency is now a direct owner-local path
inside this package's Git source tree, retaining version 0.10.2 and its existing
protocol/authentication companions, crypto dependencies and feature selection.
Do not replace it with the unpatched registry package or a consumer-only patch.

The backport retains a pending directory notification and lets an idle producer
exit when its stream receiver is dropped. It does not promise immediate
cancellation of a network query already in flight. No public API, provider
configuration, object key, schema or persisted-data migration is required.
Exercise the owned Samba suite against your resolved dependency graph before
deployment. See [client provenance and maintenance](docs/native-smb.md#client-selection).

## 0.7.2 to 0.7.3: corrected S3 credential cache dependency

Regenerate the consuming application's lockfile after updating its reviewed
workspace revision. The S3 SDK minimum is now 1.144.0, which replaces its affected
LRU cache dependency and requires corresponding compatible Smithy runtime
updates. This minimum requires Rust 1.94.1; the workspace release toolchain
already exceeds that minimum. The explicit modern HTTPS client, certificate
validation, signing and Tokio features are preserved.

No API, provider configuration, object key or persisted-data migration is
required. Exercise your S3-compatible endpoint and credential configuration
before deployment. This updates the storage provider's SDK graph; it does not
resolve independent LRU dependencies in GraphQL or router dependencies.

## 0.7.1 to 0.7.2: modern S3 transport selection

The `s3` feature now disables the AWS SDK's defaults and explicitly retains
`sigv4a`, `http-1x`, `default-https-client`, `rt-tokio` and
`behavior-version-latest`. It omits the SDK's legacy `rustls` alias, which also
activated an obsolete HTTP/TLS connector. HTTPS remains enabled through the
modern default HTTPS client; certificate validation remains enabled.

The storage S3 feature graph no longer selects Rustls 0.21/WebPKI 0.101 or
Hyper 0.14/H2 0.3. Other workspace backend selections can retain independent
legacy TLS dependencies; this change does not migrate those backends.

No API, provider configuration or persisted-data migration is required. Keep
selected workspace packages on one reviewed revision and regenerate `Cargo.lock`.
See the [development guide](docs/development.md#owned-https-transport-fixture)
for the opt-in disposable HTTPS validation command.

## 0.7.0 to 0.7.1: local temporary-file sweep

The local sweep skips symbolic links, linked roots, directory cycles and
non-file/non-directory entries. If you intentionally used links to arrange
cleanup, schedule cleanup against each real trusted target root instead.
Ordinary local reads/writes still follow links. This patch does not introduce
race-resistant filesystem confinement: all local and mounted-share roots need
trusted exclusive management and restrictive permissions, as described in the
[storage reference](docs/blob-store.md#local-filesystem-trust-boundary).

No public API, provider configuration, object key or persisted data changes.
No data migration is required. Update the reviewed workspace revision and
regenerate `Cargo.lock`.

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
SMB conditional creation also uses handle-only failure/cancellation cleanup.
After the original handle closes, no fallback deletion reopens the shared key:
a new owner may already have acquired it. Ambiguous cleanup/disarm can leave an
orphan object requiring offline recovery instead of risking another writer's
lock. Cleanup of unique temporary upload keys retains its previous behavior.
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


### Single-attempt error classification

`StorageError::SingleAttemptFailed` preserves the underlying failure in its
source chain and always reports `is_retryable() == false`. Update exhaustive
storage error matches for this added variant as well as `UnsupportedOperation`.
Native single-attempt deletion and the backup adapter never advertise retrying
by key, even when the underlying ordinary I/O/provider failure was retryable.
Non-retryable here does **not** mean the operation had no effect: a request can
have succeeded remotely while its response was lost. Establish quiescence
before offline recovery; never infer that the lock is safe to reclaim from this
flag. Ordinary deletion retains its prior error taxonomy.

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
