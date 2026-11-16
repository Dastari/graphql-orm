---
title: "BlobStore"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-11-15
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
The committing request must not be replayed either: a replay after a lost
success fails the condition against the writer's own object. A create whose
outcome is unknown therefore fails with non-retryable
`StorageError::SingleAttemptFailed` on every native provider. Native S3 sends
its conditional `PutObject` or `CompleteMultipartUpload` once, with SDK
retries disabled; only a request that was never sent, or that the server
refused with HTTP 4xx, keeps an ordinary error. Native S3 also fails closed
when the provider's enforcement is unverified: each backend instance probes
the provider before its first conditional create (see
[backup integration](backup-integration.md#provider-ownership)).

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

## Content type

`BlobPutOptions::content_type`, including the `content_type` of a multipart
upload's `MultipartStartOptions::put`, is passed to providers that store
object metadata. S3 stores it as the object's `Content-Type`. Local and SMB
storage ignore it: a file system or share has no per-file content type, and
the blob providers do not keep metadata beside the bytes. This is accepted
behaviour, not a defect, and `BlobMetadata` never reports a content type.
Hosts that need it on local or SMB storage keep it in their own records, as
backup manifests do for objects (`mime_type`). `StreamingObjectStore` keeps
its `content_type` on every provider, because it writes an object metadata
record.

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

A page can hold fewer keys than its limit, even none, while
`next_continuation` is set, as S3 pages can. Keep paging until
`next_continuation` is `None`; `list_blobs` does. The continuation token is
opaque and belongs to the provider that issued it.

Local listing pages do bounded work and hold bounded memory:

- A page resumes at its continuation without revisiting what earlier pages
  covered. A subtree that sorts entirely before the position is skipped from
  its parent's entry without being read; only the directories on the path to
  the position are read again.
- Each directory read keeps only the smallest entries the page still needs,
  at least the rest of the page plus one and at least 32, and reads the
  directory again if the page needs later ones. A page therefore holds about
  that many entries per directory on its path, however large a directory is:
  memory per page is bounded by the page size and the depth of the tree.
- Work per page is not bounded by the page size alone. A read still reads
  every entry of the directory, because file systems return entries in no
  particular order, so a page's work grows with the size of the largest
  directory it reads: a directory with `n` entries costs all `n` entries on
  every page that passes through it, and listing all of a flat directory of
  `n` keys in pages of `limit` reads about `n² / limit` entries. This is an
  accepted, documented limit. Backup content keys are sharded two levels
  deep (`objects/sha256/ab/cd/…`) so that no single directory grows large.
- Entry types come from the directory read: no entry is inspected
  separately, except on a file system that does not report entry types.
- After `2 × limit + 64` directory reads, as when many directories hold no
  keys, a page ends with the keys it found and a continuation. Such a token
  names the last directory finished, followed by `/`; other tokens are the
  last key returned. Every page makes progress.

Listings return only keys the provider accepts: a stored name that is not a
valid key is skipped, with everything below it. That covers names that are not
valid UTF-8 (local) or UTF-16 (SMB), names `validate_blob_key` refuses, such
as a local file whose name contains `\`, S3 folder markers ending in `/`, and
the reserved internal namespace. A listed key therefore always opens the
object it names.

Listings return keys in ascending lexicographic order of their UTF-8 bytes,
and a continuation resumes after the position the previous page reached in
that order. Every
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

`validate_portable_blob_key` applies `validate_blob_key` and also refuses
what SMB servers and Windows file systems cannot store under the name as
written: `:` and `< > " | ? *`, control characters, a component ending in
`.` or a space (Windows strips them, so `report.` aliases `report`), and
reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`,
`COM0`-`COM9`, `LPT0`-`LPT9` and their superscript-digit forms) in any ASCII
case and with any extension. Hosts can call it for any provider whose keys
may later move to such a target; the SMB backend applies it to new writes
when `SmbStorageConfig::portable_keys` is set, never to reads, deletes or
listings of existing keys. It does not resolve case-folding or Unicode
normalisation, so keys that differ only in case or normalisation still alias
on such a target.

`validate_canonical_blob_key` is the strict form for such targets. It applies
`validate_portable_blob_key` and also refuses every character outside ASCII
and every uppercase letter `A`-`Z`, so keys are lowercase ASCII. No two
distinct canonical keys can alias on a target that ignores case or Unicode
normalisation: an ASCII string is its own normal form under NFC, NFD, NFKC
and NFKD, and a lowercase ASCII string has no other ASCII case spelling.
NFC with a Unicode case fold was considered and not chosen. It admits more
keys, but targets compare case with their own tables rather than Unicode case
folding (a table built from simple uppercase mappings, as Windows uses, maps
both `i` and the dotless `ı` to `I`, which the case fold keeps apart), so it
would not rule out aliasing, and it would need Unicode tables that change
between Unicode versions.

The mode is opt-in and the default is unchanged: by default keys are
case-sensitive and not normalised, and on a target that ignores case or
normalisation two such keys name one file, so a write to one silently
replaces the other. Enable `SmbStorageBackend::with_canonical_keys(true)` or
`LocalStorageBackend::with_canonical_keys(true)` (for a local root on NTFS,
APFS or an OS-mounted share) to refuse non-canonical keys on new writes with
`InvalidStorageKey` before anything is written. Reads, deletes and listings
keep the ordinary validation, so objects stored earlier stay readable, listed
and deletable. The mode does not detect collisions among keys written before
it was enabled; a per-write check that lists the parent directory and
compares folded names was rejected, because it races concurrent writers and
costs a listing per write. Canonical mode refuses keys that contain uppercase
or non-ASCII characters, for example a backup table export of a table whose
name has uppercase letters, so check a repository's key shapes before
enabling it.

`StorageError::InvalidStorageKey` displays the fixed message `invalid storage
key` and never the key, which can reveal a repository path or root prefix.
The `key` field and `Debug` output hold it for host diagnostics only.

`list_blobs("")` is allowed and lists all blobs.

## Local filesystem trust boundary

Use a root managed by the trusted storage deployment, and restrict
permissions on the root and its descendants so untrusted local users and
applications cannot create or replace storage entries. This applies equally
to local backups and OS-mounted shares using this backend. On Unix, the backend
enforces the boundary below the root itself; it does not rely on the tree
being free of links.

**Root-relative, no-follow resolution on Unix.** Every local operation (reads,
writes and publication, renames, deletes, listings, metadata, streaming
objects, multipart staging and completion, and the temporary-file sweep)
opens the configured root and reaches its target one component at a time,
relative to the directory handle it already holds, without following links.
Leaf operations act on the held parent directory, never on a full path:

| Platform | Resolution |
| --- | --- |
| Linux and Android | A directory chain is opened in one `openat2` call with `RESOLVE_BENEATH \| RESOLVE_NO_SYMLINKS \| RESOLVE_NO_MAGICLINKS`. Where the kernel lacks `openat2` or a sandbox refuses it, each component is opened with `openat(O_DIRECTORY \| O_NOFOLLOW)` relative to its parent's descriptor. |
| Other Unix | Each component is opened with `openat(O_DIRECTORY \| O_NOFOLLOW)` relative to its parent's descriptor. |
| All Unix | Files are opened with `openat(O_NOFOLLOW)`, created with `O_CREAT \| O_EXCL \| O_NOFOLLOW`, published with `renameat` (conditional creates with `renameat2(RENAME_NOREPLACE)` on Linux and `RENAME_EXCL` on Apple platforms), removed with `unlinkat`, created with `mkdirat` and inspected with `fstatat(AT_SYMLINK_NOFOLLOW)`, all on the held parent descriptor. Directory syncs use the held descriptors. |
| Windows | `LocalStorageBackend::new` does not exist on Windows, so a call to it fails to compile. A host must explicitly use `LocalStorageBackend::new_trusted_windows_root` for a trusted root tree. This mode uses path-based operations and does **not** enforce a race-safe link boundary. Protect the root, its ancestors and descendants from concurrent mutation. Existing reparse-point checks are best effort only. Conditional creation uses atomic `MoveFileExW` without replacement, never hard links. |

On Unix, a directory replaced by a link after an operation resolved it never
redirects that operation: it keeps using the handle it holds, and the next
operation is refused.

**What is refused on Unix.** A key whose path below the root names or passes through
a symbolic link, wherever it points, fails with
`StorageError::InvalidStorageKey` before the link's target is touched. Writes
are refused before they create any directory. A FIFO, socket or device file at
a key is refused too, as opening one can block. Listings never list or enter a
link, and a listing whose prefix directory is reached through a link is
refused. The temporary-file sweep skips links and non-file entries. The
configured root itself may be a link: the host chose it, and only links below
it are refused. The sweep skips a root that is itself a symbolic link.

**Hard links.** Every read (`get_blob`, `get_blob_range`, `head_blob`,
`blob_exists`, the source of `copy_blob`, streaming object data and metadata,
and multipart manifests, receipts and part data) refuses a regular file with
more than one hard link with `InvalidStorageKey`, because such a file may share
its data with a file outside the root. Neither storage nor backup leaves a
second hard link on a file it wrote: writes publish by renaming a unique
temporary file, and conditional creates rename without replacing. On Unix filesystems without a non-replacing rename, conditional creation
links the temporary file at the key and then unlinks the temporary name. A
concurrent read may refuse the two-link file. Temporary unlink is checked and
retried once. If both attempts fail, publication is rolled back only after
verifying the published inode is the one we linked, and the call returns an
error. A temporary that is already gone when the retry runs counts as
published. A rollback that itself fails may leave the key published, so that
error is single-attempt (`StorageError::is_retryable()` is false): the outcome
is unknown and a retry must not replay it. Success is never reported with both
names left behind. Hosts must resolve persistent filesystem errors before
retrying. Windows uses an atomic no-replace rename and never this fallback.
A delete of a hard-linked key removes only the link inside the root.

**What is not covered.** The root itself, and anything that can change the
files the root's own directories hold, such as a process with the storage
account's permissions writing file contents directly, are the deployment's to
protect. On Windows, trusted-root local storage promises no race-safe boundary: it
holds directory handles that deny only delete sharing (read and write sharing
stay allowed) while an operation runs, a best-effort guard that does not make
concurrent mutation of the tree safe.

## Error Taxonomy

`StorageError::Provider` includes a `retryable` flag for future network
providers. Callers can use `StorageError::is_retryable()` to decide whether a
failed operation is worth retrying. A local filesystem IO error is retryable
only when its kind may be transient (`io_error_is_retryable`): denied
permission, a full or read-only file system, an exceeded quota and invalid
names are permanent, as are invalid keys, missing blobs, unsupported
backends, and failed preconditions.

Error messages never contain keys or paths, which can reveal a repository
path or configured root prefix. The variants' fields, and so `Debug`, keep
them for host diagnostics; never return either to an untrusted client.

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
