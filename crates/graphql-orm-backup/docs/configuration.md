---
title: "Backup configuration and limits"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
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
| [`ManifestKey`](../src/authentication.rs) | Host-held key material for manifest seals, from `ManifestKey::from_bytes`. | At least 32 bytes (`MIN_MANIFEST_KEY_BYTES`) of random secret material; never stored in the repository. `Debug` shows only the public key id. |
| [`ManifestAuthenticator`](../src/authentication.rs) | Seals new manifests with its current key and verifies seals from it or from keys added with `with_previous_key`. For format 3, configure `with_repository_identity` with the UUID retained independently by the host. Attach with `LocalBackupRepository::with_manifest_authenticator` or `BlobStoreBackupRepository::with_manifest_authenticator`, or return it from `BackupRepository::manifest_authenticator`. | A key is required for all manifests; format 3 also requires a host UUID pin. Without a key every manifest read or write fails with `ManifestAuthenticationRequired`. |
| [`LegacyManifestPolicy`](../src/authentication.rs) | `AcceptSealedLegacy` accepts authenticated but unbound format 2 only when its key was never shared across repositories. `TrustPinned(snapshot id -> checksum)` authenticates format 1 or 2 against independent host records; format 2 also needs a valid seal. Format-2 compaction requires individual pins. Only `TrustUnauthenticated` accepts format 1 without host-held authentication. | `Refuse`. See [MIGRATION.md](../MIGRATION.md) for the policy required by each operation. |
| [`RestoreKeyScope`](../src/restore.rs) | Live-store key prefixes that `BlobStoreRestoreObjectSink` may write, or `entire_store()`. | No default; prefixes must be valid blob keys and match whole segments. |
| [`BackupExecutionOptions`](../src/backup.rs) | `object_concurrency`, `lock` and `cancellation` for full, incremental, and compaction writes. | Default concurrency 8, default lock options and a `BackupCancellation` that is never cancelled. Cancelling stops at the next safe point, removes the run's payloads and releases the lock. |
| [`RepositoryLockOptions`](../src/lock.rs) | `owner_label` is recorded in the lock's `RepositoryLockOwner` record; `stale_after_seconds` is a legacy ignored setting. | No label; a label is at most 1024 bytes (`MAX_LOCK_OWNER_LABEL_BYTES`) without control characters. Existing locks never expire automatically; recover a crashed run's lock with `break_repository_lock` (owner run id and minimum age) after stopping every writer, or offline for a lock without an owner record. Atomic conditional create remains mandatory. |
| [`BackupCapacityLimits`](../src/capacity.rs) | `buffered_bytes`, `decompressed_bytes`, `stage_bytes` and `object_bytes` on the repository handle. | 64 MiB stored payload or manifest, 256 MiB decoded payload, 512 MiB decoded stage, unbounded object. Backups, compaction, restore and verification enforce the same values, so use one configuration for backup and restore. |
| [`FullBackupRequest`](../src/backup.rs) | `snapshot_id`, UTC `created_at`, stable `app_id`, and `app_version`. | No default. |
| [`IncrementalBackupRequest`](../src/backup.rs) | Full-request fields plus `parent_snapshot_id`. | No default; parent must be a valid chain member. |
| [`CompactChainRequest`](../src/backup.rs) | New `snapshot_id`, `source_snapshot_id`, time, app ID, and version. | No default; source chain is validated before publication. |
| [`VerificationOptions`](../src/verify.rs) | `blob_concurrency` for object/table/change checksum reads, `lock`, `lock_mode` and `cancellation`. | Defaults to 8; an explicit value below one is normalized to one. Verification holds the exclusive repository lock unless `lock_mode` is `AssumeNoConcurrentWriters`. |
| [`RestoreOptions`](../src/restore.rs) | `lock_mode`, `lock` and `cancellation` for `restore_snapshot_with_options` and `restore_objects_with_options`. | `RepositoryLockMode::Exclusive`, which needs write access. `AssumeNoConcurrentWriters` reads without the lock for read-only repositories; the caller guarantees no concurrent prune, deletion or compaction. |
| [`RestoreMode`](../src/restore.rs) and [`RestoreContext`](../src/restore.rs) | `EmptyDatabase`/`DryRun`; `disable_policies` and `disable_change_journal`. | `RestoreContext::empty_database()` is the safe applying-mode constructor; dry run never calls adapter import. |
| [`KeepPolicy`](../src/prune.rs) | `keep_last` newest chains and `lock`. | Defaults to keeping one chain and default lock settings. `keep_last: 0` is refused with `InvalidRetentionPolicy`. It does not bypass verification or locking. |

## Operational boundary

The crate owns repository layout, checksums, manifest sealing and
verification, manifest-chain validation, and restore order. The host owns the
manifest key, database export/import, object discovery, scheduling,
credentials, authorization, and a tested recovery procedure. See the
[trust model](architecture.md#trust-model).
Never retry an uncertain lock acquisition with an exists-then-write sequence;
never treat a dry run as permission to skip an applying-restore preflight.

See [usage](usage.md), [restore semantics](restore-semantics.md), and the
[snapshot format](snapshot-format.md).
