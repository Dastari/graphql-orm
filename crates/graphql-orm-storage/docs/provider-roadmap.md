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

Add behind the `s3` feature.

Expected configuration:

- endpoint URL
- region
- bucket
- key prefix
- access key
- secret key
- path-style toggle

The implementation must use the same `storage_key` values as local storage.

## Phase 3: Azure Blob Storage

Add behind the `azure` feature.

Expected configuration:

- account/container or connection string
- container name
- key prefix
- credentials

The implementation must use the same `storage_key` values as local storage.

## Out Of Scope

Dropbox and SMB are backup repository targets for `graphql-orm-backup`, not primary object storage backends for this crate.
