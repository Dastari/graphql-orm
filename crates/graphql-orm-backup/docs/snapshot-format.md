---
title: "Snapshot Format"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-12
review_by: 2027-02-01
supersedes: []
---

# Snapshot Format

## Layout

```text
snapshots/{snapshot_id}/manifest.json
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst
objects/sha256/{first_two}/{next_two}/{sha256}
locks/repository.lock
```

## Manifest

The manifest records:

- format version
- snapshot id
- parent snapshot id for incremental snapshots
- application id and version
- graphql-orm schema version and hash
- database backend
- backup kind
- database table export entries
- database change export entries
- database payload compression
- object entries
- tombstones
- manifest checksum
- authentication seal

The manifest checksum is the SHA-256 of the serialized manifest with the
`checksum` and `authentication` fields cleared. It detects corruption only.

## Manifest Authentication

`format_version` 2 manifests carry an `authentication` seal:

```json
{
  "authentication": {
    "algorithm": "HmacSha256",
    "key_id": "0123456789abcdef",
    "mac": "<64 lowercase hex digits>"
  }
}
```

`mac` is HMAC-SHA256 over the label
`graphql-orm-backup/manifest/hmac-sha256/v2` and a newline, followed by the
same canonical bytes as the checksum. The MAC key and the public `key_id` are
derived from the host's `ManifestKey` with HKDF-SHA256. Readers reject unknown
fields, so every accepted field is covered by the seal. Format version 1
manifests have no `authentication` field and are read only under an explicit
`LegacyManifestPolicy`. Other versions are refused. See the
[trust model](architecture.md#trust-model).

## Key Constraints

Readers refuse a manifest unless:

- it names the snapshot id of the `snapshots/{snapshot_id}/manifest.json`
  path it was read from;
- each object's `content_key` is exactly
  `objects/sha256/{first_two}/{next_two}/{sha256_hex}` for its own digest;
- each table or change `content_key` is exactly this snapshot's
  `database/tables/{table_name}.jsonl.zst` or
  `database/changes/{table_name}.jsonl.zst` key;
- every digest is 64 lowercase hex digits and every table name is a single
  safe key segment; and
- full and synthetic-full snapshots have no parent and no changes, and
  incremental snapshots have a different parent and no full tables.

Object `storage_key` values are live-store keys. They must be valid
`graphql-orm-storage` blob keys; backup refuses other keys before writing.

## Object Blobs

Object blobs are content-addressed by SHA-256:

```text
objects/sha256/ab/cd/abcdef...
```

This allows dedupe across snapshots and providers.

## Database Table Blobs

The table export payload is JSON Lines compressed with zstd. Each decompressed
line is one serialized backup row and ends with `\n`.

Row values are adapter-defined. With the `graphql-orm` adapter, a `DateTime` column's value is a
string holding canonical UTC RFC 3339 text with six fractional digits, UTC years 0001–9999 and
seconds 00–59 on every backend; see the
[backup runtime reference](../../../docs/reference/graphql-orm/backup-runtime.md#logical-rows).

The repository key uses the compressed filename:

```text
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
```

The manifest records:

```json
{
  "export_format": "jsonl",
  "compression": "Zstd"
}
```

Table entry checksums are computed over the stored compressed bytes, not the
decompressed JSON Lines payload. This lets repository verification validate the
exact bytes stored in the backup repository.

## Database Change Blobs

Incremental snapshots store change payloads under:

```text
snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst
```

Each decompressed line is a serialized `BackupChangeExport`. Delete changes also
produce manifest tombstones.

## Synthetic Full Snapshots

`compact_chain` writes a `SyntheticFull` manifest by replaying a full snapshot
and its incremental change payloads into a new full table payload set. The
synthetic full has no parent snapshot id.

## Repository Lock

Backups, compaction, pruning, snapshot deletion, restore and verification use
one exclusive advisory lock blob:

```text
locks/repository.lock
```

The lock is created with repository conditional-write semantics and is removed
with single-attempt deletion when the operation completes, after checking that
it still names the run. Its body is a JSON `RepositoryLockOwner` record:

```json
{"run_id":"…","host":"…","process_id":4242,"operation":"full_backup","label":null,"acquired_at":1775174400}
```

Locks never expire. Earlier releases wrote the acquisition time as a bare
integer; such locks have no owner record. See
[locking and recovery](usage.md#locking-and-recovery).
