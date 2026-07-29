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

The manifest checksum is the SHA-256 of the serialized manifest with the `checksum` field cleared.

## Object Blobs

Object blobs are content-addressed by SHA-256:

```text
objects/sha256/ab/cd/abcdef...
```

This allows dedupe across snapshots and providers.

## Database Table Blobs

The table export payload is JSON Lines compressed with zstd. Each decompressed
line is one serialized backup row and ends with `\n`.

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

Writer operations use an advisory lock blob:

```text
locks/repository.lock
```

The lock is created with repository conditional-write semantics and is removed
when the writer completes. Stale lock handling is controlled by
`RepositoryLockOptions`.
