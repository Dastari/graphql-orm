---
title: "Changelog"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Changelog

## 0.12.0 (unreleased)

Breaking: S3 conditional creates whose outcome is unknown fail as
single-attempt errors, an S3 backend probes its provider before its first
conditional create and fails closed, `InvalidStorageKey` has a fixed
message, and `SmbStorageConfig` gains a field. See [MIGRATION.md](MIGRATION.md).

- S3: `put_blob_if_not_exists` sends its committing request, the
  conditional `PutObject` or, for a body larger than one part, the
  conditional `CompleteMultipartUpload`, as one attempt with the SDK's
  automatic retries disabled. Before, when the server created the object
  but its response was lost, the SDK retried, the retry failed the
  `If-None-Match` condition against this writer's own object, and the call
  returned `Ok(None)`, so a lock holder could report its own lock as taken.
  A failure whose outcome is unknown, because the response was lost or
  unreadable, the request timed out or failed after it may have been sent,
  or the server answered with HTTP 5xx, now fails with non-retryable
  `StorageError::SingleAttemptFailed`, as on the local and SMB backends. A
  request that was never sent or that the server refused with HTTP 4xx keeps
  its classification, and a failed condition is still `Ok(None)`.
  ([#657](https://github.com/Dastari/graphql-orm/issues/657))
- S3: conditional creates fail closed unless the provider is known to
  enforce `If-None-Match: *`, which repository locks and content
  deduplication rely on. Before its first conditional create
  (`put_blob_if_not_exists`, or a resumable multipart upload started or
  completed with `if_not_exists`), each backend instance runs
  `S3StorageBackend::probe_conditional_create` and caches only a pass, in
  that instance and its clones. A probe that fails, or a provider that
  ignores the condition, makes the create fail with non-retryable
  `StorageError::RemoteProvider` of kind
  `StorageProviderErrorKind::ConditionalCreateUnverified`, so no lock is
  acquired. The probe creates a random `.graphql-orm-probe-<uuid>` object
  below the key prefix, tries to create the key again with a conditional
  `PutObject` and with a conditional one-part multipart completion, checks
  that both were refused and the first content kept, and deletes the
  object. `S3StorageBackend::with_conditional_writes_verified_by_host` is
  the one opt-out, for providers where the probe's writes are not
  permitted: the host asserts enforcement and takes responsibility for it.
  The probe cannot certify enforcement that depends on object size or a
  proxy, and a cancelled probe or failed cleanup can leave its object and
  an incomplete upload for that key. The owned RustFS lane passes it.
  ([#478](https://github.com/Dastari/graphql-orm/issues/478))
- `validate_portable_blob_key` validates a key for SMB servers and Windows
  file systems: on top of `validate_blob_key` it refuses `:`, `< > " | ? *`,
  control characters, a component ending in `.` or a space, and reserved
  device names such as `CON`, `NUL.txt`, `COM1` and `LPT¹`, in any ASCII
  case. `SmbStorageConfig` gains `portable_keys`, `false` by default. When
  set, it applies to new writes only: a written path, root prefix included,
  must be portable, and a refused write fails with `InvalidStorageKey`
  before any request. Reads, deletes and listings keep the ordinary
  validation, so objects stored earlier stay listed, readable and
  deletable. Case-folding and Unicode-normalisation aliasing are not
  addressed. ([#482](https://github.com/Dastari/graphql-orm/issues/482))
- `StorageError::InvalidStorageKey` displays the fixed message `invalid
  storage key`. It used to print the key, which can reveal a repository path
  or configured root prefix. The `key` field, and so `Debug`, still holds
  it for host diagnostics; never return either to an untrusted client.
- SMB: `SmbStorageBackend::sweep_temp_files(quiesced, older_than)` removes
  temporary upload files that a lost process or connection left on the
  share, as `LocalStorageBackend::sweep_temp_files` does for local roots,
  and returns an `SmbTempSweepReport`. It requires an
  `SmbUploadsQuiesced`, the host's statement that no upload to the
  repository is in progress. It walks the root prefix within the configured
  listing limits and deadline and considers only exact backend temporary
  names (`.<uuid>.uploading`, and the pre-0.9.0 `.<name>.<uuid>.uploading`)
  whose last-write time is at least `older_than` before the server's current
  time, read from a file the sweep creates and removes; callers' keys ending
  in `.uploading` are kept, and `older_than` below one hour is refused. It
  never follows a reparse point the server reports: such entries are
  skipped, and directories are opened as themselves and checked again
  before they are entered. Each candidate is opened exclusively without
  following a reparse point and removed through that handle, once, only if
  it still has the size and last-write time enumeration saw; a file another
  handle has open, or one that changed, is skipped. Servers that resolve
  links themselves, as Samba does for symbolic links by default, present
  them as ordinary directories, which no client can detect. A fifth
  owner-local vendor patch, `share-access-override`, adds the share-access
  option the exclusive open needs.
  ([#593](https://github.com/Dastari/graphql-orm/issues/593))

## 0.11.0 (unreleased)

Breaking: SMB `require_signing = false` is refused, SMB conditional creates
whose outcome is unknown fail as single-attempt errors, and S3 provider error
messages have a new form. See [MIGRATION.md](MIGRATION.md).

- SMB: `put_blob_if_not_exists` writes the body to a unique temporary file
  and publishes it as a hard link at the key, with `FileLinkInformation`
  and `ReplaceIfExists` false, then removes the temporary name. It used to
  create the final key with `FILE_CREATE` and stream the body into it, so
  the key existed, partly written, until the write finished, and a
  concurrent writer of the same key got `Ok(None)` for an object that was
  incomplete and was deleted again if that write failed. Now the key appears
  only with its complete content, and `Ok(None)` means another writer's
  complete object holds it. Creating a link fails atomically when the name
  exists (`link(2)` fails with `EEXIST` on Samba), so concurrent writers on
  separate connections get exactly one `Ok(Some(_))`; the managed Samba
  suite races four such writers over 25 keys. A rename without replacement
  was not used, because older Samba releases implement it as a check
  followed by a replacing rename. A link whose outcome is unknown, because
  its response was lost, timed out or could not be verified, including an
  unsigned `STATUS_PENDING` interim response, fails with
  non-retryable `StorageError::SingleAttemptFailed`, and the temporary file
  is left in place, hidden from listings, because the key may hold this
  writer's complete object.
  ([#475](https://github.com/Dastari/graphql-orm/issues/475))
- SMB: a transfer permit is held only while one READ or WRITE request is in
  flight. A download held a permit from `get_blob` until its stream ended or
  was dropped, and an upload held one for the whole write, so a consumer that
  stopped reading, or a body that stopped yielding, blocked every other
  transfer once `max_transfer_concurrency` such streams stalled.
  `max_transfer_concurrency` now bounds concurrent READ and WRITE requests.
  ([#483](https://github.com/Dastari/graphql-orm/issues/483))
- SMB: listings enter only directories that can hold keys with the listing
  prefix, instead of walking the whole tree below `root_prefix` for every
  listing and page. Listing limits count only the entries enumerated.
  ([#483](https://github.com/Dastari/graphql-orm/issues/483))
- SMB: `SmbStorageConfig::validate`, and so `connect` and `probe`, refuse
  `require_signing = false` with a non-retryable configuration error. The
  client signs every authenticated session and refuses guest sessions, so the
  setting never had an effect.
  ([#483](https://github.com/Dastari/graphql-orm/issues/483))
- S3: errors are classified from the SDK's error kind, the service error code
  and the HTTP status, not by searching the SDK's debug text. Before, text
  anywhere in that text, such as a `Connection: close` response header or the
  server's message, could make a client error retryable. Provider error
  messages are a short summary: the error code, HTTP status, request id and
  the server's message, at most 256 characters of it, or the transport
  failure. They no longer contain the SDK's debug output with the raw
  response headers and body. Retryable errors are timeouts, transport and
  response failures, HTTP 5xx and 429, and throttling and timeout error codes.
  A failed `If-None-Match` condition is recognised by its code or status 412.
  ([#479](https://github.com/Dastari/graphql-orm/issues/479))
- Local: a write that is cancelled or dropped before it publishes removes its
  temporary file, as S3 aborts its multipart upload. This covers `put_blob`,
  `put_blob_if_not_exists`, `copy_blob`, `put_object_stream` and a multipart
  object writer dropped without `complete` or `abort`. Before, the abandoned
  `.<uuid>.uploading` file stayed until `sweep_temp_files` removed it.

## 0.10.0 (unreleased)

Breaking: the SMB default minimum dialect, the reserved internal key
namespace and local listing prefixes. See [MIGRATION.md](MIGRATION.md).

- SMB: the default `min_dialect` is SMB 3.1.1, the only dialect whose
  negotiation is protected by pre-authentication integrity; it was SMB 3.0.
  SMB 3.0 and 3.0.2 remain available as an explicit opt-in. A connection whose
  server negotiates a dialect below the configured minimum fails with a
  non-retryable `SecurityNegotiation` error.
- Keys and streaming-object buckets whose first component is the internal
  `.graphql-orm-storage` namespace, in any ASCII case and with or without
  trailing dots or spaces, are refused with `InvalidStorageKey` by
  `validate_blob_key` and every provider. The local backend keeps multipart
  staging and object metadata there, which callers could previously read,
  overwrite or delete. On local roots the reservation is also by file
  identity: a key, listing prefix or bucket whose first component resolves to
  the internal directory under another name (a case-insensitive or Unicode
  case fold, a Windows short name or a link) is refused, writes create the
  internal directory first, and listings never enter such an entry. An S3
  `key_prefix` or SMB `root_prefix` in the reserved namespace makes the
  configuration invalid; S3 now validates it for empty-prefix listings too.
- `:` is refused where it separates alternate data streams: SMB keys,
  prefixes and `root_prefix` refuse it on every platform and listings skip
  such names; local roots refuse it only on Windows. Elsewhere local keys may
  contain `:` as before. S3 is unaffected.
- Listing prefixes are plain string prefixes on every provider, as S3 and SMB
  already treated them: on local storage, `a` now also lists `ab` and
  `a-b/x`, not only `a` and keys under `a/`. A prefix may now end in one `/`
  to list a single directory. `BlobStore::list_blobs_page` documents the
  contract, and a shared provider test runs it against local, S3 and SMB.
  The local `list_objects` follows the same contract.
  ([#614](https://github.com/Dastari/graphql-orm/issues/614))
- Listings return only keys the provider accepts. SMB decodes listed names
  strictly, so two names that are not valid UTF-16 no longer merge into one
  lossy key, and the local backend skips names that are not valid UTF-8.
  Names `validate_blob_key` refuses, such as the reserved namespace or a local
  file name containing `\`, and S3 keys such as folder markers ending in `/`,
  are skipped. ([#588](https://github.com/Dastari/graphql-orm/issues/588))
- S3: `put_blob` and `put_blob_if_not_exists` abort the multipart upload they
  started when the body or a part upload fails, and, best effort, when the
  write is cancelled, instead of leaving its parts stored as an incomplete
  upload. ([#611](https://github.com/Dastari/graphql-orm/issues/611))
- SMB: opening an existing key whose file has a pending delete
  (`STATUS_DELETE_PENDING`) reports the key as missing for `get_blob`,
  `blob_exists`, `head_blob` and deletion, instead of a non-retryable
  `Protocol` error. ([#594](https://github.com/Dastari/graphql-orm/issues/594))
- Vendored SMB client: a new owner-local patch,
  `PATCHES/server-response-errors.patch`, turns four panics a server could
  trigger into typed errors: a transform nested in a decrypted or
  decompressed payload, an async header without an async ID, a file on a
  printer share, and a short session key.
  ([#515](https://github.com/Dastari/graphql-orm/issues/515))
- The `local_storage` example requires the `local` feature, so test builds
  without it compile. CI checks the S3-only and SMB-only feature selections
  with `--all-targets`. ([#586](https://github.com/Dastari/graphql-orm/issues/586))
- The managed Samba suite caps the constrained-write server at SMB 3.0 and
  adds the listing contract and dialect-minimum cases.

## 0.9.3 (unreleased)

- The managed Samba test runner (`tests/samba/run.sh`) now defaults to the
  reviewed multi-platform index digest of `dperson/samba:latest`
  (`sha256:66088b78a19810dd1457a8f39340e95e663c728083efa5fe7dc0d40b2478e869`)
  instead of the floating `latest` tag, and refuses an `SMB_TEST_IMAGE`
  override that is not pinned by digest before it inspects, pulls or starts
  anything.
- Test harness only: no library, public API, configuration or object format
  change.

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
  place, then fsync the parent directory. Before publishing, each write,
  multipart staging and multipart completion fsyncs the target's directory
  chain up to the root or to a directory already made durable, including
  directories that another writer created. Each backend instance remembers the
  directories it made durable, assuming nothing outside it removes directories
  under its root while it runs. The removals made by `delete_blob`,
  `delete_blob_once` and `StreamingObjectStore::delete_object` are synced as
  well. Before this, a crash could leave a truncated or missing object behind
  a successful write, such as a truncated backup manifest or content blob.
  ([#473](https://github.com/Dastari/graphql-orm/issues/473))
- If `put_blob_if_not_exists` creates the key but the directory sync after it
  fails, it returns non-retryable `StorageError::SingleAttemptFailed`, because
  a retry would find the key taken. When it finds the key already present, it
  syncs the directory before returning `Ok(None)`.
- Directory syncs run on Unix only. A directory fsync that fails with `EINVAL`
  or `EBADF` from a file system without directory sync is accepted, as
  PostgreSQL does, on Linux, Android, Apple platforms, the BSDs, illumos,
  Solaris and AIX; other Unix targets accept no directory sync error. File fsync errors always fail the operation. The multipart
  staging directory syncs follow the same rule.
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
