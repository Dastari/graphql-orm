---
title: "Storage configuration and limits"
kind: reference
status: active
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# Storage configuration and limits

This is the complete public configuration and request-type index for this
crate. The linked Rust source is canonical for a field's exact type and error
contract; no default is implied where a type has no `Default` implementation.

## Provider features

| Feature | Default | Public entry point | Status |
| --- | --- | --- | --- |
| `local` | Yes | `LocalStorageBackend` | Supported baseline provider. |
| `s3` | No | `S3StorageBackend`, `S3StorageConfig` | Supported S3-compatible provider. |
| `smb` | No | `SmbStorageBackend`, `SmbStorageConfig` | Supported native SMB2/SMB3 provider. |
| `azure` | No | `AzureBlobStorageConfig` | Explicit unsupported placeholder; operations return `UnsupportedBackend`. |

`local` and `s3` use no implicit remote credentials. Pass credentials only at
process construction; do not serialize, log, or place secrets in source.

## Public configuration catalogue

| Type | Use | Defaults and limits |
| --- | --- | --- |
| [`S3StorageConfig`](../src/s3.rs) | `endpoint_url`, `region`, `bucket`, optional `key_prefix`, `access_key_id`, `secret_access_key`, and explicit `path_style`. | No defaults; secrets are redacted from `Debug`. Multipart part size is internal (currently 8 MiB), not a host contract. |
| [`SmbStorageConfig`](../src/smb.rs) | `server`, `port`, `share`, optional `root_prefix`/`domain`, `username`, secret `password`, dialect, signing/encryption, timeouts, transfer concurrency. | `new` defaults: port 445, SMB 3.0 minimum, signing on, encryption off, 10 s connect, 60 s operation, 8 transfers. Port/timeouts/concurrency must be positive; encryption requires SMB 3.0+. |
| [`SmbProbeOptions`](../src/smb.rs) | `create_prefix` controls whether a probe creates a missing root prefix. | Defaults false. Use only a test-owned or operator-approved share. |
| [`AzureBlobStorageConfig`](../src/azure.rs) | Future Azure declaration fields. | Not operational; all backend operations return `UnsupportedBackend`. |
| [`BlobPutOptions`](../src/blob.rs) | Optional `content_type`. | Defaults to `None`; conditional create is `put_blob_if_not_exists`, not an option flag. |
| [`StoragePutRequest`](../src/object.rs) | Buffered `namespace`, optional `file_name`/`mime_type`, and `bytes`. | No defaults; ID, safe key, checksum, size, and timestamp are generated. |
| [`StoragePutStreamRequest`](../src/object.rs) | Streaming `namespace`, optional `file_name`/`mime_type`, and `body`. | No defaults; same generated metadata as buffered writes. |

## Safety rules

Keys are relative `/`-separated identifiers; validation rejects absolute paths,
traversal, backslashes, NULs, and platform prefixes. `BlobStore` calls expose
provider errors through `StorageError`; use `is_retryable()` only as a retry
signal, never as permission to replay a non-idempotent upload stream. For
conditional repository locks, use the atomic `put_blob_if_not_exists` primitive.

See [BlobStore](blob-store.md), [streaming](streaming.md), and [native SMB](native-smb.md).
