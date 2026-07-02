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
change payloads, and parses JSON Lines without calling database restore methods.

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

The default empty restore context disables application policies and change journaling because restore is an administrative data operation, not a GraphQL user mutation.

## Safety Rules

- Load and validate the selected manifest chain before writing.
- Verify manifests before writing.
- Verify object checksums before final success.
- Verify table payload checksums against the stored compressed bytes.
- Decompress table payloads only after checksum verification.
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

## Future Replacement Mode

Future in-place restore must require:

- explicit operator confirmation
- pre-restore backup
- application quiescing or maintenance mode
- rollback strategy
- audit event
