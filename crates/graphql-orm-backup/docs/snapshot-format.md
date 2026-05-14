# Snapshot Format

## Layout

```text
snapshots/{snapshot_id}/manifest.json
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
snapshots/{snapshot_id}/database/changes/{table_name}.jsonl.zst
objects/sha256/{first_two}/{next_two}/{sha256}
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

## Database Blobs

The current table export payload is uncompressed JSON Lines. Each line is one
serialized backup row and ends with `\n`.

The repository key keeps the planned compressed filename:

```text
snapshots/{snapshot_id}/database/tables/{table_name}.jsonl.zst
```

Compression is not implemented yet. The filename reserves the intended format so future implementation has a stable layout.
