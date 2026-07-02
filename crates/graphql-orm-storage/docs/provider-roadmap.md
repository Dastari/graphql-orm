# Provider Roadmap

## Phase 1: Local Filesystem

Implemented first because it is required for standalone and self-hosted deployments and is easiest to test deterministically.

Acceptance criteria:

- atomic-ish temp write then rename
- parent directory creation
- delete missing object succeeds
- path traversal rejected
- round-trip tests pass

## Phase 2: S3-Compatible Storage

Implemented behind the `s3` feature.

S3 is implemented as a `BlobStore` first. The high-level `ObjectStorage`
behavior delegates to the same S3 blob operations so backup integrations can
reuse the provider through a future adapter.

Expected configuration:

- endpoint URL
- region
- bucket
- key prefix
- access key
- secret key
- path-style toggle

The implementation must use the same `storage_key` values as local storage.

Integration tests are opt-in through `S3_TEST_ENDPOINT` and `S3_TEST_BUCKET`.
Use the optional `S3_TEST_PATH_STYLE` setting for MinIO compatibility.

## Phase 3: Azure Blob Storage

Add behind the `azure` feature.

Implement Azure Blob as a `BlobStore` first. Azure should follow the same
provider layering as S3 after the S3 implementation proves the shared blob
interface.

Expected configuration:

- account/container or connection string
- container name
- key prefix
- credentials

The implementation must use the same `storage_key` values as local storage.

## Out Of Scope

Dropbox and SMB are backup repository targets for `graphql-orm-backup`, not primary object storage backends for this crate.
