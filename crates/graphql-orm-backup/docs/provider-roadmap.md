# Provider Roadmap

## Phase 1: Local Filesystem

Implemented first as the deterministic baseline.

Acceptance criteria:

- put/get/list/delete blob
- nested key support
- path traversal rejection
- delete missing blob succeeds

## Phase 2: S3

Add behind the `s3` feature.

Expected configuration:

- endpoint URL
- region
- bucket
- prefix
- credentials
- path-style toggle

## Phase 3: Azure Blob

Add behind the `azure` feature.

Expected configuration:

- account/container or connection string
- container
- prefix
- credentials

## Phase 4: SMB

Initial SMB support should be mounted filesystem support using `LocalBackupRepository`.

Native SMB protocol support is future work.

## Phase 5: Dropbox

Dropbox should be a backup repository provider only. It should not become a primary object storage backend unless a future product requirement justifies that.
