# Restore Semantics

## Initial Supported Mode

Only restore into an empty target is supported initially.

```rust
RestoreMode::EmptyDatabase
```

In-place restore and replacement are future work.

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
- Preserve primary keys.
- Preserve created and updated timestamps where entities define them.
- Restore rows in dependency order.
- Do not emit change journal entries during restore.
- Do not run normal GraphQL row policies during restore.
- Refuse non-empty target databases in `EmptyDatabase` mode.

## Future Replacement Mode

Future in-place restore must require:

- explicit operator confirmation
- pre-restore backup
- application quiescing or maintenance mode
- rollback strategy
- audit event
