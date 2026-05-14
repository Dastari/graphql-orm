# Provider Roadmap

## Phase 1: Local Filesystem

Implemented first as the deterministic baseline.

Acceptance criteria:

- put/get/list/delete blob
- nested key support
- path traversal rejection
- delete missing blob succeeds

## Phase 2: S3

Do not implement direct AWS SDK integration in this crate yet.

`graphql-orm-storage` should first expose a shared lower-level streaming
`BlobStore` abstraction. `graphql-orm-backup` should then adapt that abstraction
to `BackupRepository` so primary object storage and backup repositories can
share S3-compatible provider code without sharing higher-level semantics.

Expected configuration:

- endpoint URL
- region
- bucket
- prefix
- credentials
- path-style toggle

## Phase 3: Azure Blob

Do not implement direct Azure SDK integration in this crate yet. Azure Blob
should follow the same future `graphql-orm-storage::BlobStore` adapter path as
S3 once the shared abstraction exists.

Expected configuration:

- account/container or connection string
- container
- prefix
- credentials

## Phase 4: SMB

Initial SMB support should be mounted filesystem support using `LocalBackupRepository`.

Native SMB protocol support is future work. Mounts, credentials, reconnect
behavior, and OS-level permissions are managed outside this crate. Use
`LocalBackupRepository::open_existing` to validate that the mounted path exists
and is a directory before using it as a repository root.

## Phase 5: Dropbox

Dropbox should be a backup repository provider only. It should not become a primary object storage backend unless a future product requirement justifies that.
