---
title: "Changelog"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-10-15
review_by: 2027-02-01
supersedes: []
---

# Changelog

## Unreleased (namespace identity; version assigned when rebased after the 0.7.6 capacity release)

- Added `BlobStore::observe_namespace`, `initialise_namespace`,
  `observe_capacity_domain`, `put_blob_fenced` and `observation_epoch`, with
  non-positive defaults (`Unsupported` or `UnsupportedOperation`, no I/O).
- Local, SMB and S3 implement durable object-namespace markers:
  - strict canonical bodies and bounded reads;
  - explicit create-if-absent publication with same-identity reconciliation;
  - bounded topology scans with completeness states;
  - scoped capacity-domain facts and fenced writes.
  See [namespace identity](docs/namespace-identity.md).
- Ordinary key paths reject the reserved marker names under each provider's key
  equivalence (case folding, trailing dots and spaces, SMB streams and short
  names), and listings hide them. SMB keys containing `:` are now rejected,
  because they address alternate data streams.
- No storage key, object format or persisted-data migration is required for
  existing data.

## 0.7.5 - 2026-10-18

- Refresh package identity for the maintained Samba fixture and protocol
  validation scripts included after 0.7.4. Preserve the existing Rust code,
  public API, provider behavior, feature selection and dependency graph.
- No storage key, object format, schema or persisted-data migration is required.

## 0.7.4 - 2026-10-15

- Backport the upstream SMB directory-stream notification and idle-drop fixes
  to an owner-local copy of SMB 0.10.2. Preserve the existing protocol,
  authentication and cryptographic dependency family and feature selection.
- Retain archive, patch and MIT-license provenance with the dependency. No
  public API, provider configuration, storage key or persisted-data changes.

## 0.7.3 - 2026-10-15

- Require the S3 SDK release that adopts the corrected S3 Express credential
  cache dependency. Preserve the modern HTTPS transport and explicit signing
  features introduced in 0.7.2.
- No public API, object format or provider configuration changes. Other packages'
  independent transitive advisories still require separate reachability review.

## 0.7.2 - 2026-10-15

- Select the S3 SDK's modern HTTPS client explicitly without its legacy
  `rustls` feature, removing Hyper 0.14/H2 0.3 and the obsolete TLS branch
  from this package's S3 feature graph. Signing, Tokio and HTTPS remain enabled.
- Added a disposable HTTPS transport fixture with trusted-CA success and
  untrusted-CA rejection; retained the owned HTTP release-replay regression.
- No public API, provider configuration, storage key or data format changes.

## 0.7.1 - 2026-10-14

- Hardened local temporary-file sweeping to skip symbolic links, directory
  cycles and non-file/non-directory entries (R-004).
- Documented the required trusted, exclusively managed filesystem tree and
  restrictive permissions, including mounted shares. Ordinary path operations
  do not provide confinement against hostile local filesystem writers.
- Added an owned-directory regression covering linked files, outside-root
  directories, broken links, cycles and a linked sweep root.

## 0.7.0 - 2026-10-14

- Fixed the S-005 custom-provider path: inherited conditional creation now
  fails closed instead of performing an existence check followed by a write.
- Breaking: added non-retryable `StorageError::UnsupportedOperation`; custom
  providers must implement atomic `BlobStore::put_blob_if_not_exists`.
- SMB conditional-create failure/cancellation cleanup closes only its original
  handle and never removes a replacement object by reopening the shared key.
- Added non-retryable `SingleAttemptFailed` errors preserving the original cause
  without advertising unsafe retries after an ambiguous deletion.
- Added explicit `delete_blob_once` capability for lock release, defaulting to
  unsupported. Native local uses one removal; S3 disables SDK operation retries
  and SMB does not reopen paths after a possibly applied deletion.
- Native local, SMB and S3 overrides and Azure unsupported behavior are retained;
  provider configuration and persisted data are unchanged.

## 0.6.2 - 2026-08-31

- Refreshed the package's maintained workspace handoff to the current ORM,
  backup, and authorization revisions. Storage APIs, provider behavior,
  persisted metadata, and feature defaults are unchanged.
- This patch establishes a new immutable package tag because maintained files
  under the package source tree changed after 0.6.1. No data migration is
  required.

## 0.6.1 - 2026-08-13

- Published the maintained configuration catalogue, backup-integration guide,
  and checked-in local-storage example from the coordinated monorepo docs
  overhaul. The storage API and persistence contracts are unchanged.
- Aligned current workspace guidance with `graphql-orm` 0.22.0 and
  `graphql-orm-backup` 0.7.1. No data migration is required.

## 0.6.0

- Consolidated the crate into the `graphql-orm` workspace with one root
  lockfile and a shared reviewed Git revision for companion crates.

- Native SMB writes now split every incoming stream chunk into requests no
  larger than the negotiated `MaxWriteSize`, with a conservative 1 MiB cap and
  64 KiB SMB2 credit alignment. Partial acknowledgements advance exact offsets;
  zero-byte acknowledgements fail explicitly; the stream remains backpressured
  and is never collected.
- Parent-directory creation is now single-flight per path and backed by a
  bounded per-backend cache. Reconnects and path-not-found responses invalidate
  the cache, eliminating repeated ancestor `OpenIf` calls for shared object
  prefixes.
- Transient failures now replace the shared client by generation and retry only
  idempotent connection, directory, open, listing, deletion, and unique-temp
  creation operations. Conditional `FILE_CREATE` is never replayed after an
  ambiguous response.
- Direct and temporary writes use delete-on-close ownership plus cancellation
  guards. A conditional-create collision never arms cleanup and therefore never
  deletes the pre-existing winner.
- `SmbProbeResult` now reports negotiated dialect, signing/encryption state,
  maximum read/write/transaction sizes, and the effective write-request limit.
- Added redaction-safe `SmbStorageBackend::diagnostics` counters and structured
  `tracing` events for connection generations, upload progress, failures,
  directory coordination, and reconnects.
- Added unit and managed-Samba coverage for oversized chunks, constrained
  negotiated limits, partial and zero-byte writes, exact checksums,
  cancellation cleanup, conditional/overwrite behavior, reconnect cache
  invalidation, concurrency 1/2/4/8, and the shared-parent performance
  regression.

## 0.5.0

- Added the feature-gated native `SmbStorageBackend` using a pure-Rust
  SMB2/SMB3 client. The provider supports streamed reads and writes, safe
  temporary-file publication, atomic conditional creation, signing and
  encryption requirements, reconnect handling, and redaction-safe probing.
- Added `SmbStorageConfig`, `SmbDialect`, `SmbProbeOptions`, and
  `SmbProbeResult` behind the `smb` feature.
- Added `StorageProviderErrorKind` and `StorageError::RemoteProvider` for
  structured network-provider diagnostics and retry classification.
- Added unit coverage for SMB configuration, secret redaction, key mapping,
  temporary-file filtering, and retry/error classification.
- Added opt-in Samba integration coverage for authentication, workgroup
  credentials, signing, encryption, streamed round trips, interruption
  cleanup, atomic conditional creation, and reconnect after server restart.
- `StorageBackend` and `StorageError` gain new variants. Exhaustive downstream
  matches must add SMB and remote-provider cases; see [MIGRATION.md](MIGRATION.md).

Earlier release details remain in the archived
[release notes](../../docs/archive/2026/graphql-orm-storage-release-notes.md).
