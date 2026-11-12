---
title: "Changelog"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-11
review_by: 2027-02-01
supersedes: []
---

# Changelog

## 0.9.2

- Local listings return keys in exact lexicographic key order. The local
  backend walked each directory in path order, so a sibling whose name sorts
  below `/`, such as `a-b` or `a.b` beside the directory `a`, was listed after
  the keys under `a/`. Unpaged `list_blobs` returned keys out of order, and a
  paged listing could skip keys, because each continuation token resumes after
  the last key in key order. Directories now sort as `<name>/`, matching S3 and
  SMB. Paged listings also skip directories whose keys all precede the
  continuation. ([#472](https://github.com/Dastari/graphql-orm/issues/472))
- Local writes are durable once they return. `put_blob`,
  `put_blob_if_not_exists`, `copy_blob`, `put_object_stream` and multipart
  object writers fsync the temporary file before renaming or linking it into
  place, then fsync the parent directory. New directories, and the removals made
  by `delete_blob`, `delete_blob_once` and `StreamingObjectStore::delete_object`,
  are synced as well. Before this, a crash could leave a truncated or missing
  object behind a successful write, such as a truncated backup manifest or
  content blob. Directory syncs run on Unix only.
  ([#473](https://github.com/Dastari/graphql-orm/issues/473))
- Writes are slower on the local backend, because each one now waits for the
  file system to reach stable storage.
- No public API, configuration or object format change.

## 0.9.1

- Only treat the backend's own upload temp files as temporary. The local
  backend treated every file whose name ended in `.uploading` as an
  interrupted upload: listings skipped it and `sweep_temp_files` deleted it
  once it was older than the threshold. A stored object could have such a
  name, for example when `StorageService` kept the extension of a client
  filename such as `report.UPLOADING`.
- The local backend now writes its temporary files as `.<uuid>.uploading`
  beside the target, as SMB does, and listings and `sweep_temp_files` act on
  that exact form only, with the UUID in lowercase hyphenated form. Any other
  name ending in `.uploading` is a stored object.
- Local temporary files left by earlier versions, named
  `<name>.<uuid>.uploading`, cannot be told apart from stored objects. They
  are now listed and are not swept; see the migration guide.
- SMB no longer hides keys such as `report.<uuid>.uploading`: only
  `.<uuid>.uploading` and the `.<name>.<uuid>.uploading` form written before
  0.9.0 are temporary.
- Local and SMB writes refuse a key whose name is a temporary name with
  `InvalidStorageKey`, so a written object is never filtered or swept. The
  local backend refuses it before creating any directory.
- `file_extension` returns `None` for the extension `uploading` in any case,
  so `StorageService` never generates a key ending in `.uploading`, and a
  sweep by an older version sharing the root cannot remove objects stored
  through `StorageService`. Direct `BlobStore` callers may still write names
  that an older version's sweep removes; see the migration guide.
- No public API, configuration or object format change.

## 0.9.0 (unreleased)

- Breaking: `SmbStorageConfig` gains `expected_server_guid`,
  `max_listing_depth`, `max_listing_entries`, `max_listing_bytes` and
  `listing_timeout`.
  `SmbStorageConfig::new` sets the defaults; code that builds the struct
  literally must add the fields. `SmbProbeResult` gains `server_guid`.
- Server identity: every SMB connection negotiates and checks the server GUID
  before session setup sends the NTLM response. The first connection pins the
  GUID unless `expected_server_guid` pins it in advance. A reconnect to a
  server reporting another GUID is refused with a non-retryable
  `SecurityNegotiation` error after sending only `NEGOTIATE`, instead of
  repeating the credential exchange against it on each of three attempts. The
  check runs inside the vendored client's negotiation, so a fresh DNS
  resolution cannot bypass it. The GUID is not authenticated; see the
  documented limits.
- Listings are bounded: depth 64 below the root prefix, 1,000,000 entries,
  256 MiB of names and paths, and 15 minutes overall by default, all
  configurable. The deadline covers the retry after a reconnect. A listed name
  longer than 255 UTF-16 code units, or a key longer than 4,096 bytes, fails
  the listing, and the backend refuses to write a key longer than 4,096 bytes.
  The 4,096-byte limit and a 255-unit limit on each component apply to the
  caller's key, also through `put_blob`, whose temporary upload name is now
  `.<uuid>.uploading` so that it fits any valid name; older temporary names
  are still recognised. A `QUERY_DIRECTORY` reply larger than the requested
  output buffer is refused before it is decoded, and a listing that stops at a
  limit closes its directory handle.
  A self-nesting directory now fails the listing with a non-retryable
  `Protocol` error instead of being listed forever. A listing that reaches any
  limit fails rather than returning a partial result.
- `SmbStorageBackend` implements `list_blobs` with a single listing instead of
  re-listing the tree for every 1,000-key page. `list_blobs_page` tokens now
  begin with `smb1:` and carry their chain's page and key counts. A chain fails
  after 1,024 pages or `max_listing_entries` keys, so a server that keeps adding
  names cannot make a paged listing run forever. Tokens from earlier versions
  are rejected with `PreconditionFailed`. A malformed or exhausted token is
  refused before the tree is listed.
- Connections no longer follow DFS referrals, which this backend does not
  support, and no longer enable server-to-client notifications. The vendored
  async notification handler never drained its channel, so the receive loop
  could stall after ten notifications.
- The vendored client accepts a response only for a message ID it sent and
  that is still unanswered, holds at most 256 responses that no task is yet
  waiting for, and tracks at most 8,192 outstanding request IDs. Earlier, any
  response, signed or not and even before authentication, was stored without
  limit, at up to 16 MiB each.
- The vendored receive loop closes the connection on any transport failure,
  and after 64 consecutive messages that fail to process, instead of logging
  and retrying immediately. Waiting requests fail at once and trigger the
  bounded reconnect.
- `probe` checks the probe file's declared size and reads at most the 16 bytes
  it wrote. `collect_storage_stream` preallocates at most 8 MiB from a size
  hint.
- The vendored client wraps the password in its zeroizing secret before any
  fallible step, so no error path drops it as a plain string.
- Authentication remains NTLM-only. The documentation now describes the
  exposure and recommends a long, random, single-purpose service-account
  password. Optional Kerberos support is tracked in
  [#585](https://github.com/Dastari/graphql-orm/issues/585).
- The vendor changes are one new owner-local patch,
  `PATCHES/resource-bounds-and-server-identity.patch`, recorded in the vendor
  provenance with its regression tests.
- No key, object format or persisted-data change.

## 0.8.1 (unreleased)

- Security ([GHSA-439h-2cx8-xwgx](https://github.com/Dastari/graphql-orm/security/advisories/GHSA-439h-2cx8-xwgx)):
  the vendored SMB client evaluated a response status before verifying the
  message's signature or encryption. An on-path party without credentials could
  answer a pending request with an unsigned error such as
  `STATUS_OBJECT_NAME_NOT_FOUND`, even with `require_encryption = true`, making
  `blob_exists` return `false`, deletion report success, or a listing skip a
  directory. A new owner-local patch, recorded in the vendor provenance,
  verifies signing or encryption first. An unverified response now fails with a
  non-retryable `SecurityNegotiation` error. Only the MS-SMB2 3.2.5.1.3
  exemptions remain (interim `STATUS_PENDING`, and `SESSION_SETUP` before setup
  completes), and only for statuses that fail the request.
- Interim `STATUS_PENDING` responses are untrusted and are checked against the
  session the request was sent on, not the interim's own header. An interim is
  discarded if it names another session, if that session is unknown, or if it
  is unencrypted on an encryption-required session. An interim can no longer
  cause the genuine final response to be rejected because of an async ID
  mismatch. Forged interims can delay a request up to `operation_timeout`, not
  change its outcome.
- A response naming another session, or no session, is rejected with the same
  `SecurityNegotiation` error as an unsigned one.
- SMB listings fail closed: a directory reported by enumeration that is not
  found, or does not open as a directory, fails the listing with a
  non-retryable `Protocol` error instead of being skipped. A missing configured
  root still lists as empty.
- No API, key, object format or persisted-data change.

## 0.8.0 (unreleased)

- Add resumable multipart uploads: `MultipartBlobStore` with
  `start_multipart`, `upload_part`, `list_parts`, `complete_multipart`,
  `abort_multipart`, `list_incomplete_multipart` and `multipart_limits`.
  `MultipartUploadHandle` and `PartReceipt` are serializable and carry no
  credentials. See [multipart uploads](docs/multipart.md).
- `complete_multipart` returns `MultipartCompleteOutcome { size_bytes,
  sha256_hex: Option<String> }`. Local storage fills the digest; S3 returns none
  and never reads the object back, because integrity rests on the per-part
  SHA-256 checksums and the exact receipt list. Add the default
  `BlobStore::compute_blob_sha256` to hash an object later.
- A retried completion reports success when it can verify that its own object
  is published. On S3 that means the size plus the multipart ETag; on local
  storage, after a crash that left the staged parts, the size plus the SHA-256.
- `LocalStorageBackend` stages fsynced parts under
  `.graphql-orm-storage/multipart/`, and publishes the object atomically under an
  advisory per-upload lock. `S3StorageBackend` uses the native multipart calls
  with per-part SHA-256 checksums and a single-attempt conditional completion.
  SMB and Azure don't implement it.
- Add `BlobStore::as_multipart` (default none), `StorageService::multipart`,
  `LocalStorageBackend::with_multipart_max_part_size` and
  `S3StorageBackend::with_multipart_max_part_size`.
- Breaking: add `StorageError::Multipart { key, kind: MultipartErrorKind }`.
  `StorageBackend` now derives `Hash`. The `s3` feature adds the `base64` and
  `md-5` dependencies, both already in the lockfile. Default features are unchanged.
- Fix: S3 `list_blobs_page` with an empty prefix now lists only
  `key_prefix/`, and drops keys outside it. Before, `key_prefix = "proj"` also
  listed `project2/...`.
- `put_blob`, `put_object_stream` and `MultipartWriter` are unchanged.

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
