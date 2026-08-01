---
title: "Storage Provider Backlog"
kind: plan
status: draft
owner: graphql-orm-storage-maintainers
last_reviewed: 2026-08-01
review_by: 2026-11-01
supersedes: []
---

# Storage Provider Backlog

`graphql-orm-storage` owns reusable object and blob storage primitives.
Providers implement `BlobStore` first; higher-level object APIs remain layered
on that shared boundary.

## Current State

- Local filesystem, S3-compatible, and native SMB2/SMB3 providers are
  implemented behind their documented features.
- Azure Blob is feature-gated but intentionally returns an explicit unsupported
  error until its shared `BlobStore` provider is implemented.

## Backlog

- Implement Azure Blob as a streaming `BlobStore` provider with key safety,
  ranges, conditional writes, copy, paged listing, and provider round-trip
  coverage before exposing higher-level object behavior.
- Run and record compatibility validation for native SMB against supported
  Windows and NAS targets without collecting credentials or endpoints.
- Evaluate provider-managed encryption only with a provider-neutral contract
  and restore-compatible operational guidance.

Backup consumers reuse these providers through
`graphql-orm-backup::BlobStoreBackupRepository`; they must not duplicate
provider transport, locking, manifests, or retention behavior.
