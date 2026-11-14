---
title: "Migration Guide"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Migration Guide

## 0.11.0 to 0.12.0: S3 conditional creates, key errors, SMB configuration

Update the storage requirement to 0.12.0 and refresh the consumer lockfile.

1. **Unknown S3 conditional-create outcomes.** `put_blob_if_not_exists` on
   `S3StorageBackend` sends its committing request once, without the SDK's
   automatic retries. When that request's outcome is unknown, because its
   response was lost or unreadable, it timed out or failed after it may have
   been sent, or the server answered with HTTP 5xx, the call fails with
   `StorageError::SingleAttemptFailed`, which is not retryable: the key may
   hold this call's complete object. Such failures used to be retried inside
   the SDK, where a retry after a lost success returned `Ok(None)` for the
   writer's own object, or surfaced as a retryable `StorageError::Provider`.
   Handle it as for local and SMB storage: do not report it as a collision,
   and do not retry the same call blindly, because a retry would find the
   key taken. `Ok(Some(_))` and `Ok(None)` keep their meanings.
2. **S3 conditional creates probe first.** The first conditional create on
   each `S3StorageBackend` instance, which includes the first repository
   lock acquisition, now writes, re-creates and deletes one
   `.graphql-orm-probe-<uuid>` object below the key prefix, so the
   credentials need write, read and delete permission there. A provider
   that fails the probe, or ignores `If-None-Match`, now gets a
   non-retryable `RemoteProvider` error of kind
   `ConditionalCreateUnverified` instead of an unsafe lock. Only where the
   probe's writes are not permitted and enforcement was verified another
   way, construct the backend with
   `with_conditional_writes_verified_by_host()`; if that assertion is wrong,
   two holders can acquire one lock.
3. **`InvalidStorageKey` message.** Its `Display` is now the fixed
   `invalid storage key`. Code that parsed the key from the message must
   read the `key` field instead, and must not show it to untrusted clients.
4. **`SmbStorageConfig::portable_keys`.** The configuration has a new public
   field, `false` by default. Code that builds `SmbStorageConfig` with a
   struct literal must set it; code that uses `SmbStorageConfig::new` and
   assigns fields is unaffected. Set it to `true` for a share on a Windows
   server, or one Windows clients use; it refuses new writes of keys that
   are not portable, and existing objects stay addressable.

## 0.10.0 to 0.11.0: SMB signing, conditional-create outcomes, S3 messages

Update the storage requirement to 0.11.0 and refresh the consumer lockfile.

1. **`require_signing = false`.** SMB configurations that set it now fail
   validation, so `connect` and `probe` return a non-retryable configuration
   error. Remove the assignment: the client always signs authenticated
   sessions, and the setting never turned signing off. Use
   `require_encryption` for a stronger guarantee.
2. **Unknown SMB conditional-create outcomes.** `put_blob_if_not_exists` now
   publishes a complete temporary file as a hard link at the key that never
   replaces an existing file. The server must support `FileLinkInformation`
   (hard links) on the share; Samba and NTFS shares do. If the link's response
   is lost, times out or cannot be verified, it fails with
   `StorageError::SingleAttemptFailed`, which is not retryable: the key may
   hold this call's complete object. Its temporary file is left in place and
   hidden from listings. Treat it as an unknown outcome, as for
   local storage. Do not report it as a collision and do not retry the same
   call blindly; a retry would find the key taken. `Ok(Some(_))` and
   `Ok(None)` keep their meanings, and the key is never visible while
   partly written.
3. **SMB transfer concurrency.** `max_transfer_concurrency` bounds concurrent
   SMB READ and WRITE requests. An open download stream or a write waiting
   for its body no longer holds a permit, so more transfers than the limit
   can be open at once. Bound the number of open streams in the caller if
   that matters.
4. **S3 error messages.** `StorageError::Provider` messages from the S3
   backend are now a short summary, such as
   `S3 service error AccessDenied (HTTP 403, request id …): Access Denied`,
   instead of the SDK's debug output. Code that searched the message for
   status codes or SDK type names must use `StorageError::is_retryable`,
   `MissingBlob`, or `Ok(None)` from the conditional writes instead. Some
   errors change retryability: a client error is no longer retryable because
   the response had a `Connection` header or its message mentioned a timeout.
5. **S3 missing bucket.** `get_blob`, `get_blob_range` and `copy_blob`
   against a missing bucket (`NoSuchBucket`, HTTP 404) always fail with a
   non-retryable `StorageError::Provider` error, never with
   `StorageError::MissingBlob`. Only `NoSuchKey`, `NotFound` and a 404
   without an error code mean a missing key. Before, the result depended on
   the SDK's debug text: a missing bucket was `MissingBlob` when that text
   contained `NotFound`, `Not Found` or `status: 404`, and otherwise a
   provider error, retryable whenever the response had a `Connection`
   header. Treat this error as a configuration fault, not as an absent
   object. `head_blob` and `blob_exists` still report a missing bucket as a
   missing key, because a HEAD response carries no error code to tell them
   apart.

## 0.9.3 to 0.10.0: SMB 3.1.1 minimum, reserved namespace, string prefixes

Update the storage requirement to 0.10.0 and refresh the consumer lockfile.

1. **SMB servers that cannot negotiate SMB 3.1.1.** `SmbStorageConfig::new`
   now defaults `min_dialect` to `SmbDialect::Smb3_1_1`. Connecting to a
   server limited to SMB 3.0 or 3.0.2 fails with a non-retryable error before
   any credential is sent. Prefer upgrading the server. Otherwise opt in
   explicitly, preferably with encryption required:

   ```rust,ignore
   config.min_dialect = SmbDialect::Smb3_0; // or SmbDialect::Smb3_0_2
   config.require_encryption = true;
   ```

   Below SMB 3.1.1 the negotiation is not integrity-protected, so an on-path
   party could select any dialect from the configured minimum up. See
   [dialect](docs/native-smb.md#dialect). Configurations that already set
   `min_dialect` keep their value.
2. **Keys in the internal namespace.** Keys and buckets whose first component
   is `.graphql-orm-storage`, in any ASCII case and with or without trailing
   dots or spaces, are refused with `InvalidStorageKey` and no longer listed.
   On local storage that namespace holds internal state, and callers should
   never have used it. On S3 or SMB, move any such objects to another key
   with the provider's own tools before upgrading; this crate can no longer
   address them.

   The same applies to configured prefixes. An S3 `key_prefix` or SMB
   `root_prefix` whose first component is in the reserved namespace now makes
   the configuration invalid: `SmbStorageConfig::validate`, and so `connect`
   and `probe`, refuse it, and every S3 operation, including an empty-prefix
   listing, fails with `InvalidStorageKey` before any request. Move such a
   repository to another prefix.
3. **Local aliases.** On local roots the reservation is by file identity: a
   key, prefix or bucket whose first component resolves to
   `<root>/.graphql-orm-storage` under another name, through case folding, a
   Windows short name or a link, is refused. Local writes now create that
   directory up front.
4. **`:` where it separates streams.** `:` names an alternate data stream on
   Windows filesystems and on SMB servers (Windows, and Samba with
   `streams_xattr`), where `a:b` opens stream `b` of the file `a`:
   - Local roots on Windows refuse `:` in key, prefix and bucket components
     and no longer list stored names containing it.
   - Local roots elsewhere are unchanged: `:` is an ordinary character, and
     keys such as ISO timestamps keep working.
   - SMB refuses `:` in keys, prefixes and `root_prefix` on every platform,
     and listings skip names containing it. Rename such keys, for example to
     use `-`, with the server's own tools before upgrading.
   - S3 is unaffected.
5. **Local listing prefixes.** A local `list_blobs`, `list_blobs_page` or
   `list_objects` prefix is now a string prefix, as on S3 and SMB: `a` also
   lists `ab` and `a-b/x`. To list one directory only, end the prefix with `/`
   (`a/`), which every provider now accepts, or filter the result. A custom
   `BlobStore` must accept a trailing `/` and treat prefixes the same way.
6. **Listings skip unaddressable names.** Names that are not valid keys,
   including SMB names that are not valid UTF-16, local names that are not
   valid UTF-8 or contain `\`, and S3 folder markers, are no longer listed.
   They could not be read back through their listed key.

No object format or repository layout changes.

## 0.9.2 to 0.9.3: digest-pinned managed Samba fixture

Update the storage requirement to 0.9.3 and refresh the consumer lockfile. No
API, configuration or object format change is required.

The managed Samba runner now uses the image
`dperson/samba@sha256:66088b78a19810dd1457a8f39340e95e663c728083efa5fe7dc0d40b2478e869`
by default. Local runs keep the never-pull default, so pull that exact
reference once before running `tests/samba/run.sh` if only the `latest` tag
is cached. An `SMB_TEST_IMAGE` override must be a digest reference
(`name@sha256:<64 hex digits>`); a tag-only override exits with status 64.

## 0.9.1 to 0.9.2: ordered and durable local storage

Update the storage requirement to 0.9.2 and refresh the consumer lockfile. No
API, configuration or object format change is required.

- Local `list_blobs` and `list_blobs_page` now return keys in exact
  lexicographic key order. Code that relied on the earlier directory-walk order
  must sort keys itself or accept the new order. Paged listings no longer skip
  keys next to sibling names such as `a-b` or `a.b`. A paged listing that
  spans the upgrade can still have skipped such keys, so restart it after
  upgrading.
- Local writes and deletions now fsync file data and the affected directories
  before they return. Expect lower write throughput, most of all on file
  systems with slow syncs. Data written by earlier versions has not been
  synced. Run any verification you depend on, such as a backup verify, after
  an unclean shutdown that happened before the upgrade.
- A file system that cannot sync directories keeps working: on Linux,
  Android, Apple platforms, the BSDs, illumos, Solaris and AIX, `EINVAL` and
  `EBADF` from a directory fsync are accepted. A file system that rejects
  file fsync now fails local writes. Use a root on a file system that supports
  it.
- `put_blob_if_not_exists` can now return non-retryable
  `StorageError::SingleAttemptFailed` when the key was created but its
  directory sync failed. Treat it as outcome unknown: the key may exist. For a
  backup repository lock, do not retry the acquisition. Recover offline, after
  quiescing all writers and handles, as for any lock of unknown state.

## 0.9.0 to 0.9.1: backend-owned temporary names

Update the storage requirement to 0.9.1 and refresh the consumer lockfile. No
API, configuration or object format change is required.

- The local backend now names temporary upload files `.<uuid>.uploading`, and
  only that exact form is hidden from listings and removed by
  `sweep_temp_files`. Stored objects whose names merely end in `.uploading`
  are now listed and are never swept.
- Earlier local versions named temporary files `<name>.<uuid>.uploading`.
  Such a name cannot be told apart from a stored object, so a file left by an
  interrupted upload of an earlier version is now listed and is not swept.
  This keeps the backend from ever deleting a stored object. To remove such
  leftovers, stop every writer that runs an earlier version, then review and
  delete files under the root matching `*.<uuid>.uploading` that do not begin
  with a dot and that you did not store. Do not automate this for roots that
  may hold stored objects with such names.
- Upgrade every writer that shares a local root together. During a
  mixed-version rollout, a 0.9.1 listing shows an earlier writer's in-flight
  `<name>.<uuid>.uploading` temporary file as an ordinary key.
- Stop running a sweep from an earlier version against a root a 0.9.1 writer
  uses; the earlier sweep still removes any file ending in `.uploading`.
  `StorageService` no longer generates such keys, but direct `BlobStore`
  callers may.
- On SMB, only `.<uuid>.uploading` and `.<name>.<uuid>.uploading` are
  filtered. A key such as `report.<uuid>.uploading` is now listed.
- Writing a key whose name has one of these temporary forms now fails with
  `InvalidStorageKey` on local and SMB.
- `file_extension` returns `None` for the extension `uploading`, in any case.
  `StorageService` stores such files without an extension.

## 0.8.1 to 0.9.0: SMB resource bounds and server identity

Update the storage requirement to 0.9.0 and refresh the consumer lockfile. No
key, object format or persisted-data migration is required.

- Code that constructs `SmbStorageConfig` with a struct literal must set
  `expected_server_guid`, `max_listing_depth`, `max_listing_entries`,
  `max_listing_bytes` and `listing_timeout`. Prefer `SmbStorageConfig::new` and
  assign fields.
- A backend now pins the SMB server GUID from its first connection and refuses
  a reconnect to a server reporting another GUID. If a server is replaced or
  rebuilt, reconstruct the backend after confirming the change. To pin from the
  first connection, record `SmbProbeResult::server_guid` and set
  `expected_server_guid`. Clusters whose nodes report different GUIDs under one
  name must pin a single node's name.
- A listing deeper than 64 directories below the root prefix, larger than
  1,000,000 entries, or longer than 15 minutes now fails with a non-retryable
  error. Raise `max_listing_depth`, `max_listing_entries` or `listing_timeout`
  for repositories that legitimately exceed them. A listing that accumulates
  more than 256 MiB of names and paths fails the same way; raise
  `max_listing_bytes`. `listing_timeout` now covers the retry after a
  reconnect.
- A listed name longer than 255 UTF-16 code units, or a key longer than 4,096
  bytes, fails the listing. Keys longer than 4,096 bytes, or with a component
  longer than 255 UTF-16 code units, are rejected on write with
  `InvalidStorageKey`.
- `put_blob` now names its temporary upload file `.<uuid>.uploading` instead
  of `.<name>.<uuid>.uploading`. Tools that look for interrupted uploads should
  match both forms; the backend filters both.
- `list_blobs_page` continuation tokens changed format. Do not persist them
  across an upgrade; restart a paged listing instead. A chain fails after
  1,024 pages or `max_listing_entries` keys, so prefer `list_blobs`, which now
  lists the tree once.
- DFS referrals are no longer followed. Configure the server that holds the
  share directly.
- Authentication is unchanged and NTLM-only. If the service account's
  password is short, shared or reused, replace it with a long random one used
  only by this backend.

## 0.8.0 to 0.8.1: SMB response verification (GHSA-439h-2cx8-xwgx)

Update the storage requirement to 0.8.1 and refresh the consumer lockfile. No
API, configuration or data migration is required. Two SMB behaviours change:

- A response that is not signed or encrypted as the session requires now fails
  with `StorageProviderErrorKind::SecurityNegotiation` before its status is
  read. Earlier it could surface as an authoritative status such as
  `RemotePathNotFound`. Treat the operation's outcome as unknown.
- A listing fails with a non-retryable `Protocol` error when a directory it
  enumerated is not found or is not a directory. Earlier the subtree was
  silently omitted. Investigate outside changes to the share before retrying.

## 0.7.5 to 0.8.0: resumable multipart uploads

- Exhaustive matches on `StorageError` must handle the new `Multipart` variant.
  No other existing API changes.
- Use multipart through `LocalStorageBackend` or `S3StorageBackend` directly,
  through `BlobStore::as_multipart()`, or through `StorageService::multipart()`.
- Local staging lives under the existing internal `.graphql-orm-storage`
  directory. Don't write blobs whose keys start with `.graphql-orm-storage/`.
- S3 `list_blobs_page` with an empty prefix now lists only the backend's
  `key_prefix/` namespace. Hosts that relied on it also returning keys of a
  longer prefix sharing the bucket, such as `project2/` for `proj`, must list
  those through a backend configured for that prefix.
- No object format, key, schema or persisted-data migration is required.

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
