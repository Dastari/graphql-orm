---
title: "BlobStore"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# BlobStore

`BlobStore` is the low-level, key-addressed storage abstraction in
`graphql-orm-storage`.

Use it when code needs to put, get, list, inspect, or delete arbitrary safe blob
keys without creating primary object metadata.

## Boundary

`BlobStore` does not know about:

- GraphQL
- application authorization
- tenants
- collections
- database rows
- object namespaces as domain policy
- upload or download routes

It only stores bytes at provider-neutral keys.

## API Shape

```rust
#[async_trait::async_trait]
pub trait BlobStore: Send + Sync {
    fn backend(&self) -> StorageBackend;

    async fn put_blob(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<BlobWriteOutcome, StorageError>;

    async fn put_blob_if_not_exists(
        &self,
        key: &str,
        body: StorageByteStream,
        options: BlobPutOptions,
    ) -> Result<Option<BlobWriteOutcome>, StorageError>;

    async fn get_blob(&self, key: &str) -> Result<BlobBody, StorageError>;

    async fn get_blob_range(&self, key: &str, range: Range<u64>) -> Result<BlobBody, StorageError>;

    async fn blob_exists(&self, key: &str) -> Result<bool, StorageError>;

    async fn head_blob(&self, key: &str) -> Result<Option<BlobMetadata>, StorageError>;

    async fn list_blobs_page(
        &self,
        prefix: &str,
        continuation: Option<String>,
        limit: usize,
    ) -> Result<BlobListPage, StorageError>;

    async fn list_blobs(&self, prefix: &str) -> Result<Vec<String>, StorageError>;

    async fn copy_blob(&self, from: &str, to: &str) -> Result<(), StorageError>;

    async fn delete_blob_once(&self, key: &str) -> Result<(), StorageError>;

    async fn delete_blob(&self, key: &str) -> Result<(), StorageError>;
}
```

`list_blobs` is a convenience method that drains `list_blobs_page`.
Provider implementations should make `list_blobs_page` the native listing path.

`put_blob_if_not_exists` returns `Ok(None)` when the target key already exists.
Implementations must make this operation atomic across all writers. The default
returns non-retryable `StorageError::UnsupportedOperation` without provider I/O;
custom providers must override it using a native atomic primitive. A separate
existence check followed by a write is unsafe for deduplication and backup locks.

`delete_blob_once` is the release capability for lock keys that another writer
can immediately reuse. Its default returns `UnsupportedOperation` without I/O.
Implementations must never replay deletion by key after mutation may have
reached storage. Native S3 disables SDK operation retries and native SMB avoids
path reopening after disposition/close; local uses one filesystem removal.
Ordinary `delete_blob` can retain provider retry behavior. A lost response or
cancellation leaves outcome unknown: do not retry by key, and establish that all
writers, handles and in-flight requests are quiesced before offline recovery.
Operation errors from this capability must report non-retryable. Native providers
use `StorageError::SingleAttemptFailed` to preserve the cause in the error source
chain; the backup adapter also suppresses erroneous custom-provider retry hints.
A non-retryable flag does not mean that storage was unchanged.

`copy_blob` may use provider-side copy and does not return a SHA-256 checksum.
Callers can use `head_blob` after copying when they need backend metadata.

The local provider writes through a temporary `.<uuid>.uploading` file beside
the target and renames it into place. Only that exact form, with a lowercase
hyphenated UUID, is temporary: listings skip it, and
`LocalStorageBackend::sweep_temp_files` removes it once it is older than the
given age. Any other name ending in `.uploading` is a stored object, listed and
never swept, and a write to a key whose name has the temporary form fails with
`InvalidStorageKey`. Host applications can call `sweep_temp_files`
periodically to remove stale temp files. Temporary files left by local
versions before 0.9.1, named `<name>.<uuid>.uploading`, are indistinguishable
from stored objects and are neither hidden nor swept; see the
[migration guide](../MIGRATION.md).

## Listing prefixes, order and durability

A listing prefix is a plain string prefix of the key on every provider:

| Prefix | Lists | Does not list |
| --- | --- | --- |
| `a` | `a/x`, `a-b/x`, `a.b`, `ab` | `b/a` |
| `a/` | `a/x`, `a/y/z` | `a-b/x`, `ab` |
| `a/y` | `a/y/z` | `a/x` |

An empty prefix lists every key. A non-empty prefix must be a valid key,
optionally followed by one `/`; use the trailing `/` to list one directory and
nothing beside it. Local listings start at the deepest directory the prefix
names, so a prefix costs no more than walking that directory. The shared
provider test in `tests/support/listing_contract.rs` runs the same prefix
cases against the local, S3 and SMB backends.

Listings return only keys the provider accepts: a stored name that is not a
valid key is skipped, with everything below it. That covers names that are not
valid UTF-8 (local) or UTF-16 (SMB), names `validate_blob_key` refuses, such
as a local file whose name contains `\`, S3 folder markers ending in `/`, and
the reserved internal namespace. A listed key therefore always opens the
object it names.

Listings return keys in ascending lexicographic order of their UTF-8 bytes,
and a continuation resumes after the last key returned in that order. Every
provider meets this contract: S3 lists in key order, SMB sorts its full
listing, and the local backend walks each directory with subdirectories
ordered as `<name>/`, so `a-b` and `a.b` list before `a/x`. Custom providers
must use the same order, or paging through a listing skips or repeats keys.

A successful local write is durable once it returns:

- `put_blob`, `put_blob_if_not_exists`, `copy_blob`, `put_object_stream`,
  multipart object writers and multipart completion fsync the temporary file
  before renaming or linking it into place, then fsync the parent directory.
  Object metadata written by `put_object_stream` is synced the same way.
- Before publishing, a write makes the target's directory chain durable. It
  fsyncs each directory from the target's parent upward, making that
  directory's entry durable, until it reaches the root or a directory the
  backend has already made durable. A directory that already existed is
  included, because another writer may have created it and not synced it yet.
  The root's own entry is synced only when the write created the root.
  Multipart staging makes its upload directories durable the same way.
- Each backend instance remembers up to 4,096 directories it has made durable,
  shared by its clones. It records a directory only after the whole chain has
  synced, so a failed sync is repeated by the next write. When a write finds a
  remembered directory missing, it forgets that directory before recreating
  it, then syncs the entry itself, so no concurrent write trusts the
  recreated directory before its entry is durable.
- This memory assumes that nothing outside the backend removes directories
  under its root while the backend runs, as the
  [trust boundary](#local-filesystem-trust-boundary) requires. A remembered
  directory that another process removes and recreates, while this backend's
  writes still find it present, stays trusted until the backend instance is
  recreated, and a crash can then lose its entry.
- `delete_blob`, `delete_blob_once` and `StreamingObjectStore::delete_object`
  fsync the parent directory after removing the file. A deletion of a key that
  is already missing still syncs its directory when the directory exists.
- When `put_blob_if_not_exists` finds the key already present, it syncs the
  directory and returns `Ok(None)`. When it created the key but the following
  directory sync failed, it returns non-retryable
  `StorageError::SingleAttemptFailed`: the key may exist, so a retry would find
  it taken. Ordinary writes report a failed sync as a retryable
  `StorageError::Io`, and repeating them is safe.

File fsync errors always fail the operation. Directory syncs run on Unix only;
other platforms rely on the file system to persist directory entries. On
Linux, Android, Apple platforms, FreeBSD, NetBSD, OpenBSD, DragonFly BSD,
illumos, Solaris and AIX, a directory fsync that fails with `EINVAL` or
`EBADF`, which file systems without directory sync report (some CIFS mounts
and VM shared folders, for example), is accepted without syncing, as
PostgreSQL does. Other Unix targets accept no directory sync error. Any other
directory sync error, including failing to open the directory, fails the
operation.
Network file systems mounted under a local root may not honour these syncs.

## Key Safety

Blob keys are `/`-separated relative keys. `validate_blob_key` rejects:

- empty keys
- absolute paths
- empty path segments
- `.`
- `..`
- backslashes
- NUL bytes
- platform prefix components
- a first component naming the reserved internal namespace,
  `.graphql-orm-storage`, in any ASCII case and with or without trailing dots
  or spaces

The local backend keeps multipart staging and object metadata under
`<root>/.graphql-orm-storage/`. Reserving the name on every provider keeps
callers from reading, overwriting or listing that state, and keeps keys
portable between providers. The name is reserved only as a key's first
component and as a streaming-object bucket: `data/.graphql-orm-storage/x`
is an ordinary key.

On local roots the reservation is by file identity, not only by spelling. A
case-insensitive filesystem resolves other spellings to the same directory,
for example a Unicode case fold such as `ſ` for `s` on APFS, or a Windows 8.3
short name. `LocalStorageBackend` therefore also refuses any key, listing
prefix or bucket whose first component resolves to the same filesystem object
as `<root>/.graphql-orm-storage`, compared by device and file identity, and a
listing never enters a root entry that does. Writes create the internal
directory first, so an aliasing name can never claim it before the backend
does.

`:` is refused only where servers or filesystems treat it as a stream
separator:

| Provider | `:` in keys, prefixes and buckets or root prefixes |
| --- | --- |
| Local on Windows | Refused, and stored names with `:` are not listed: `name::$INDEX_ALLOCATION` would alias a directory. |
| Local elsewhere | Allowed: `:` is an ordinary filename character, so keys such as ISO timestamps work and are listed. |
| SMB | Refused everywhere, and listed names with `:` are skipped: Windows servers, and Samba with `streams_xattr`, open stream `b` of file `a` for `a:b`. |
| S3 | Allowed. |

`list_blobs("")` is allowed and lists all blobs.

## Local filesystem trust boundary

Use a root managed exclusively by the trusted storage deployment. Restrict
permissions on the root and its descendants so untrusted local users/applications
cannot create or replace storage entries. Protect ancestor path components against
replacement too. This applies equally to
local backups and OS-mounted shares using this backend.

Key validation is lexical. Ordinary local reads/writes follow existing links;
the configured root is not a confinement boundary against a hostile local
writer. Keep links that could resolve outside the intended tree out of the
storage hierarchy. The temporary-file sweep skips symbolic links, including
linked directories, broken links and cycles; it also skips non-file/non-directory
entries. A root that is itself a symbolic link is skipped by the sweep.
Deployments intentionally using links must arrange cleanup of their real target
roots explicitly.

The sweep inspects paths before operating on them. A concurrent replacement can
still race that inspection. Race-resistant confinement would require directory
handle relative resolution and no-follow operations at every path component,
with platform-specific handling of reparse points and all read/write/delete
paths. That is not implemented here. Canonicalizing a path before opening it
would still leave a check/use race. Do not use this backend as a sandbox for
untrusted local filesystem writers; isolate them through host permissions or a
separate storage provider.

## Error Taxonomy

`StorageError::Provider` includes a `retryable` flag for future network
providers. Callers can use `StorageError::is_retryable()` to decide whether a
failed operation is worth retrying. Local filesystem IO errors are treated as
retryable; invalid keys, missing blobs, unsupported backends, and failed
preconditions are permanent.

`StorageError::Multipart { key, kind }` reports a refused multipart operation
(unknown upload, backend mismatch, part number, part size or receipt mismatch).
It is permanent. See [multipart uploads](multipart.md).

## Multipart uploads

`BlobStore::as_multipart()` returns the provider's `MultipartBlobStore`, or none.
Local and S3 storage implement it; SMB and Azure do not. See
[resumable multipart uploads](multipart.md).

## Relationship To ObjectStorage

`ObjectStorage` builds on `BlobStore`. Provider implementations should implement
`BlobStore` first, then expose object-storage behavior on top of it.

`StorageService` remains the high-level API for primary object metadata. It
generates object IDs, storage keys, sizes, hashes, and timestamps.

## Backup Integration

`BlobStore` is the sharing point used by `graphql-orm-backup` through
`BlobStoreBackupRepository`. Backup repositories should adapt `BlobStore`; they should not use
`StorageService` or `StoredObject`, because backup keys and primary object
metadata have different semantics.
