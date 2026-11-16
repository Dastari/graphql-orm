---
title: "Restore Semantics"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-15
review_by: 2027-02-01
supersedes: []
---

# Restore Semantics

## Supported Modes

The default applying mode restores into an empty target:

```rust
RestoreMode::EmptyDatabase
```

Dry-run restore is also supported:

```rust
RestoreMode::DryRun
```

Dry run loads and validates the manifest chain, verifies payload checksums, decompresses table and
change payloads, compares the manifest backend/schema hash with the target, and
parses JSON Lines without calling database restore methods.

In-place restore and replacement remain future work.

## Restore Context

Restore runs under an explicit context:

```rust
pub struct RestoreContext {
    pub mode: RestoreMode,
    pub disable_policies: bool,
    pub disable_change_journal: bool,
}
```

The default empty restore context requests application-policy and
change-journal suppression because restore is an administrative data
operation, not a GraphQL user mutation. `OrmBackupAdapter` rejects applying
contexts that do not request both.

The ORM row importer does not use generated GraphQL/repository mutation paths.
The flags do not authorize restore, change the connection's database role, or
bypass database-native row-level security. Those remain host responsibilities.

## Trust

Restore trusts a manifest only after verifying its host-keyed seal, its
snapshot id against its path, its format version and its entry namespaces.
See the [trust model](architecture.md#trust-model). Without a configured
`ManifestAuthenticator` restore fails with `ManifestAuthenticationRequired`
before reading any manifest. Unauthenticated legacy manifests are refused
with `LegacyManifestRefused` unless the host opts in with a
`LegacyManifestPolicy`.

## Safety Rules

- Load and validate the selected manifest chain before writing.
- Authenticate every manifest in the chain and bind it to its path before
  writing.
- Verify object checksums before final success.
- Verify table payload checksums against the stored compressed bytes, both
  during verification and again on the exact bytes buffered for decoding.
- Decompress table payloads only after checksum verification.
- Refuse a manifest backend/schema hash that differs from the target before
  checking emptiness or writing.
- Refuse `EmptyDatabase` restore when the database adapter reports a non-empty target.
- Preserve primary keys.
- Preserve created and updated timestamps where entities define them.
- Restore rows in dependency order.
- Do not emit change journal entries during restore.
- Do not run normal GraphQL row policies during restore.
- Refuse non-empty target databases in `EmptyDatabase` mode.

## Object Restore

Database restore and object restore are separate. `restore_snapshot` restores table/change payloads
through `GraphqlOrmBackupAdapter`. `restore_objects` loads verified object blobs and passes them to
a caller-supplied `RestoreObjectSink` so applications can choose how to rehydrate their primary
object store.

Before restoring any object, `restore_objects` authenticates the manifest it
is given, requires every object's live-store key to be a valid
`graphql-orm-storage` key (no empty, `.` or `..` segments, absolute paths,
backslashes or NUL), and calls `RestoreObjectSink::check_restore_target` for
every object. A rejected key fails with `RestoreTargetRejected` before the
sink receives bytes.

`BlobStoreRestoreObjectSink` takes a `RestoreKeyScope`. It writes only keys
below the scope's prefixes (whole segments: `media` permits `media/a` but not
`media-old/a`), or anywhere in the store for `RestoreKeyScope::entire_store()`,
which suits a store that holds only restored objects. It creates each object
with an atomic conditional write and never overwrites: a key that already
holds the same bytes is left in place, and a key that holds different bytes
fails with `RestoreObjectConflict`. Restore into an empty staging namespace.

The object stream handed to a sink fails at its end when its bytes do not
match the manifest digest, and `restore_objects` then returns
`ChecksumMismatch`. A bundled `BlobStore` discards such a write, and none of
them makes the refused object readable at its key: local storage and SMB
write a hidden temporary file and publish it only after the whole body, and
S3 sends its committing request only after the body ends. Before
`graphql-orm-storage` 0.11.0, SMB created the object at its final key with
delete-on-close set, so a concurrent reader of the live store could read it
until the delete took effect
([#475](https://github.com/Dastari/graphql-orm/issues/475)). Buffering sinks
never receive the bytes. A custom streaming sink must likewise discard a
body whose stream ends in an error. A sink that returns success without
reading the whole stream also fails with `ChecksumMismatch`.

`BlobStoreRestoreObjectSink` passes each object's manifest `mime_type` as the
`content_type` of its write. S3 stores it. Local and SMB storage ignore it,
since a file has no content type there, so an object restored to them keeps
its bytes and key but no content type; serve the type from the host's own
object records. This is accepted behaviour.

### Repository reads

A full restore reads each object blob from the repository three times and
each table or change payload twice. `restore_snapshot` first verifies the
whole snapshot, reading every payload and object to check its size and
digest. Then it reads each payload again to decode it. `restore_objects`
reads each object once to check its size and digest before calling the sink,
and once more as the digest-checked stream the sink consumes.

This is accepted and intended. A repository can serve different bytes on a
later read than it served to verification, so every read whose bytes are
used is checked again, and an object's size is known before the sink sees
any of it. Plan restore time and repository egress for about three times the
object bytes and twice the payload bytes. A single verified streaming pass
would need a sink that can stage and roll back unverified bytes, which the
`RestoreObjectSink` contract does not require.

A successful database restore does not imply that external objects,
credentials, provider continuations, application reconciliation, or runtime
readiness exist. Hosts must verify and reconcile those separately.

## Future Replacement Mode

Future in-place restore must require:

- explicit operator confirmation
- pre-restore backup
- application quiescing or maintenance mode
- rollback strategy
- audit event
