---
title: "Backup configuration and limits"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-12
review_by: 2027-02-01
supersedes: []
---

# Backup configuration and limits

This page is the complete public configuration/request-type index. The linked
Rust source remains canonical for exact types and errors; values are stated
only when implemented by the crate.

## Features

| Feature | Default | Effect |
| --- | --- | --- |
| `local` | Yes | `LocalBackupRepository` over local `BlobStore`. |
| `smb` | No | Enables native SMB construction through `graphql-orm-storage`; credentials remain runtime-only. |
| `orm` | No | Enables adapter traits for a host that has already selected exactly one ORM backend. |
| `orm-sqlite` | No | Enables the SQLite ORM adapter lane. |
| `orm-postgres` | No | Enables the PostgreSQL ORM adapter lane. |

Do not enable both ORM backend features. The package has no cloud credential
configuration of its own; adapt a configured `graphql-orm-storage::BlobStore`.

## Public configuration catalogue

| Type | Use | Defaults and limits |
| --- | --- | --- |
| [`BackupExecutionOptions`](../src/backup.rs) | `object_concurrency` and `lock` for full, incremental, and compaction writes. | Default concurrency 8 and default lock options. |
| [`RepositoryLockOptions`](../src/lock.rs) | `stale_after_seconds` for advisory writer locks. | Defaults to 3,600 seconds. Atomic conditional create remains mandatory. |
| [`FullBackupRequest`](../src/backup.rs) | `snapshot_id`, UTC `created_at`, stable `app_id`, and `app_version`. | No default. |
| [`IncrementalBackupRequest`](../src/backup.rs) | Full-request fields plus `parent_snapshot_id`. | No default; parent must be a valid chain member. |
| [`CompactChainRequest`](../src/backup.rs) | New `snapshot_id`, `source_snapshot_id`, time, app ID, and version. | No default; source chain is validated before publication. |
| [`VerificationOptions`](../src/verify.rs) | `blob_concurrency` for object/table/change checksum reads. | Defaults to 8; an explicit value below one is normalized to one. |
| [`RestoreMode`](../src/restore.rs) and [`RestoreContext`](../src/restore.rs) | `EmptyDatabase`/`DryRun`; `disable_policies` and `disable_change_journal`. | `RestoreContext::empty_database()` is the safe applying-mode constructor; dry run never calls adapter import. |
| [`KeepPolicy`](../src/prune.rs) | `keep_last` newest chains and `lock`. | Defaults to keeping one chain and default lock settings. It does not bypass verification or locking. |

## Operational boundary

The crate owns repository layout, checksums, manifest-chain validation, and
restore order. The host owns database export/import, object discovery,
scheduling, credentials, authorization, and a tested recovery procedure.
Never retry an uncertain lock acquisition with an exists-then-write sequence;
never treat a dry run as permission to skip an applying-restore preflight.

See [usage](usage.md), [restore semantics](restore-semantics.md), and the
[snapshot format](snapshot-format.md).
