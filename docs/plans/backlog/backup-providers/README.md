---
title: "Backup Provider Backlog"
kind: plan
status: draft
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-01
review_by: 2026-11-01
supersedes: []
---

# Backup Provider Backlog

`graphql-orm-backup` owns provider-independent repository, snapshot, restore,
verification, and locking semantics. It reuses shared provider transport through
`graphql-orm-storage::BlobStore` and `BlobStoreBackupRepository`.

## Current State

- Local repositories, S3-compatible storage reuse, and native SMB reuse are
  implemented.
- Azure Blob remains an unsupported storage placeholder; backup will reuse it
  only after `graphql-orm-storage` provides a production `BlobStore` provider.
- Mounted SMB remains an explicitly named legacy deployment of the local
  repository; the host owns its mount and credentials.

## Backlog

- Evaluate a backup-only Dropbox repository only when a concrete product need
  defines its restore, verification, locking, and retention behavior.
- Improve repository pagination only with provider-scale evidence and preserve
  streaming, key-layout, and lock compatibility.
- Consider client-side encryption or content-defined chunking only with a
  compatible restore and migration design.

Provider implementations must not duplicate storage transport or weaken the
atomic `put_blob_if_not_exists` locking primitive.
