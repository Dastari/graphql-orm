---
title: "Resumable multipart uploads"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-10-31
review_by: 2027-04-30
supersedes: []
---

# Resumable multipart uploads

`MultipartBlobStore` uploads one object as numbered parts that can arrive from
different requests or processes, and survive a dropped connection or a restart.
The object becomes visible only when completion succeeds.

The crate defines no sessions, quotas, authorization, routes or expiry policy.
The caller stores the handle and receipts, decides who may continue an upload,
and computes any whole-file checksum policy from the parts it relays.

## Providers

| Provider | Multipart | Part size | Parts |
| --- | --- | --- | --- |
| `LocalStorageBackend` | Yes | 1 byte to 5 GiB (`with_multipart_max_part_size`) | 10,000 |
| `S3StorageBackend` | Yes | 5 MiB (except the last) to 64 MiB by default, up to 5 GiB (`with_multipart_max_part_size`) | 10,000 |
| `SmbStorageBackend` | No: `UnsupportedOperation` | | |
| Azure placeholder | No: `UnsupportedOperation` | | |

Read the limits from `multipart_limits()`. `BlobStore::as_multipart()` returns
the provider's implementation, or none. `StorageService::multipart()` returns
`StorageError::UnsupportedOperation` for providers without it.

`put_blob`, `put_object_stream` and `MultipartWriter` are unchanged and
independent of these uploads.

## Limits and memory

The S3 default maximum part size is 64 MiB, and every part except the last must
be at least 5 MiB. Local parts may be from 1 byte to 5 GiB.

## Lifecycle

```rust,ignore
let handle = store.start_multipart("videos/talk.mp4", MultipartStartOptions {
    put: BlobPutOptions { content_type: Some("video/mp4".into()) },
    if_not_exists: true,
}).await?;
persist(&handle);                                         // serde, no credentials

let receipt = store.upload_part(&handle, 1, body).await?; // 1..=10_000, bounded
persist(&receipt);

let parts = store.list_parts(&handle).await?;            // resume after losing state
let outcome = store.complete_multipart(&handle, &parts).await?;
// or: store.abort_multipart(&handle).await?;
```

- `MultipartUploadHandle { backend, key, upload_id, if_not_exists }` and
  `PartReceipt { part_number, size_bytes, sha256, provider_tag }` are `Clone`,
  `Send`, `Sync`, `Serialize` and `Deserialize`. Their serialized form is stable
  across patch releases. `if_not_exists` is omitted from the wire when false.
  The handle carries no credentials or endpoint, so a later
  `presign_upload_part` can be added without changing it.
- A part is streamed and refused with `PartTooLarge` as soon as it exceeds the
  maximum. Local storage writes it straight to disk. S3 needs the part length
  before the body, so it holds the part in memory. Memory use is about the
  maximum part size (64 MiB by default) times the number of parts in flight at
  once.
- Re-uploading a part number replaces that part. Concurrent uploads of one part
  number leave one complete part, never an interleaving.
- Completion requires the receipts to equal the stored parts, in ascending
  part-number order, with equal sizes, provider tags and checksums. Every part
  but the last must reach the minimum part size. On any failure, including a
  mismatch, an `if_not_exists` conflict or a provider error, the parts stay in
  place, so the caller can retry or abort.
- `complete_multipart` returns a `MultipartCompleteOutcome { size_bytes,
  sha256_hex: Option<String> }`. Local storage hashes the object while it
  assembles it and always fills `sha256_hex`. S3 returns none and does not read
  the object back. Its integrity comes from the per-part SHA-256 checksums the
  provider verified when each part arrived, plus completion against the exact
  receipt list. To hash an object later, for example in a background job, call
  `BlobStore::compute_blob_sha256(key)`, which streams the object.
- Abort removes every part, and succeeds for an unknown, already aborted,
  completed or expired upload.
- A completion racing an abort ends in one of two states: the complete object
  is published and nothing is left to abort, or completion fails with
  `UnknownUpload` and no object was published.
- `list_incomplete_multipart(prefix, continuation, limit)` pages through
  started uploads with their start times, for cleanup. Expiry is the caller's
  policy: abort what is older than it allows.

## Retrying an ambiguous completion

A completion whose outcome is unknown, such as a lost response or a crash, can
be retried with the same handle and receipts. The retry reports success when it
can verify that the object it would publish is already there.

- **S3** sends completion once. A retry after a lost response finds the upload
  gone (`NoSuchUpload`). It then `HEAD`s the key, and reports success only when
  two things match the receipts:
  - the object's size equals the receipts' total;
  - its ETag equals the multipart ETag computed from the receipts: the MD5 of
    the concatenated part MD5s, then `-` and the part count.

  Otherwise the retry fails with `UnknownUpload`, including on providers whose
  part ETags are not MD5s, where the check isn't possible. The returned
  `sha256_hex` is none.
- **Local** storage can crash after publishing the object but before removing
  the staged parts. A retry then finds the staged parts and an existing target.
  Under `if_not_exists`, the retry reports success and removes the staged parts
  when the existing target's size and SHA-256 equal the assembled parts.
  Otherwise it fails with `PreconditionFailed`. A retry after the staged parts
  were removed fails with `UnknownUpload`.

## Errors

`StorageError::Multipart { key, kind }` carries a `MultipartErrorKind`:

| Kind | Meaning |
| --- | --- |
| `UnknownUpload` | Unknown, completed, aborted or expired upload |
| `BackendMismatch` | The handle belongs to another backend |
| `PartNumberOutOfRange` | Part number outside 1 to the backend's part limit |
| `PartTooSmall` | A part other than the last is below the minimum |
| `PartTooLarge` | A part exceeds the maximum |
| `ReceiptMismatch` | Missing, extra, reordered or changed receipts |

An `if_not_exists` conflict at completion is `StorageError::PreconditionFailed`.
Messages contain the key and the kind, never credentials or provider responses.

## Local storage

Parts are staged under `<root>/.graphql-orm-storage/multipart/<upload_id>/`,
outside the visible key space: listing never returns them. The directory holds a
manifest, one data file per part and one receipt per part number. Every file is
written to a temporary name, fsynced and renamed into place, and the directory
is fsynced, so staged parts survive a crash or restart.

Receipt commits, completion and abort hold an exclusive advisory lock
(`<upload_id>.lock`, through `std::fs::File::lock`). The operating system
releases it when the process exits, so a crash never leaves an upload locked.
Run uploads against one host's local filesystem; advisory locks are not reliable
on every network filesystem.

Completion concatenates the parts into a temporary file beside the target,
re-checking each part's size and SHA-256, fsyncs it, then renames it into place,
or hard-links it under `if_not_exists`. This is a full synchronous copy that
holds the upload's lock, so it takes about the object size divided by the disk's
sequential throughput: roughly 20 seconds for 10 GB at 500 MB/s. Run completion
outside a request deadline for multi-gigabyte objects. Part uploads to the same
upload wait for the lock meanwhile. It then fsyncs the directory and removes
the staging directory. A crash before publishing leaves the parts in place; a
crash after it leaves the complete object, plus a stale upload that
`list_incomplete_multipart` reports and abort removes.

## S3

The backend uses `CreateMultipartUpload`, `UploadPart`, `ListParts`,
`CompleteMultipartUpload`, `AbortMultipartUpload` and `ListMultipartUploads`,
with the existing endpoint, TLS, credentials and `key_prefix`.

- Uploads are created with the SHA-256 checksum algorithm. Each part carries
  `x-amz-checksum-sha256`, so the provider verifies it.
- Completion runs with SDK retries disabled, because a lost response must not
  be replayed against an upload that may already have become the object.
  `if_not_exists` sends `If-None-Match: *`.
- S3 keeps no per-upload condition, so `if_not_exists` travels in the handle.
  Handles returned by `list_incomplete_multipart` on S3 report
  `if_not_exists: false`. On S3 the guarantee therefore rests on the persisted
  handle: store handles server-side and never accept them from untrusted
  clients, since a client could clear the flag. Local storage records the
  condition in its manifest as well and honours it when either the manifest or
  the handle sets it.
- Only `NoSuchUpload` maps to `UnknownUpload`. Any other 404, such as a missing
  bucket, is a provider error, so aborting against misconfigured storage fails
  rather than appearing to succeed.
- Listings stay inside the backend's `key_prefix/` namespace. With
  `key_prefix = "proj"`, keys under `project2/` sharing the bucket are never
  listed.
- Some S3-compatible servers don't return part checksums from `ListParts`.
  Receipts rebuilt from such a listing have an empty `sha256`; sizes and ETags
  still bind them.

The owned conformance test runs against a digest-pinned RustFS container; see
[development](development.md).
