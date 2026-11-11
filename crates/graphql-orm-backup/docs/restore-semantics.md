---
title: "Restore Semantics"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-11-11
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
`ChecksumMismatch`. A bundled `BlobStore` discards such a write; buffering
sinks never receive the bytes. A custom streaming sink must likewise discard a
body whose stream ends in an error. A sink that returns success without
reading the whole stream also fails with `ChecksumMismatch`.

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
