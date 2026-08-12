---
title: "graphql-orm-backup documentation"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# graphql-orm-backup documentation

Start with the package [README](../README.md).

## Learn

- [Usage guide](usage.md) — host adapters and a complete backup/restore flow.
- [Architecture](architecture.md) — responsibilities and authority boundary.
- [Configuration and limits](configuration.md) — public execution and retention options.

## How-to

- [Restore safely](restore-semantics.md).
- [Use native or mounted SMB](smb.md).
- [Adapt a storage `BlobStore`](../../graphql-orm-storage/docs/backup-integration.md).

## Reference

- [Snapshot format](snapshot-format.md).
- [Migration guide](../MIGRATION.md) and [changelog](../CHANGELOG.md).

## Concepts

- [Cloud-provider direction](cloud-provider-direction.md) records scope, not a supported-provider promise.
