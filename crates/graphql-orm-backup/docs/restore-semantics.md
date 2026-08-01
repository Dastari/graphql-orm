---
title: "Restore Semantics"
kind: reference
status: active
owner: graphql-orm-backup-maintainers
last_reviewed: 2026-08-01
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

## Safety Rules

- Load and validate the selected manifest chain before writing.
- Verify manifests before writing.
- Verify object checksums before final success.
- Verify table payload checksums against the stored compressed bytes.
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
