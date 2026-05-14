# graphql-orm Agent Brief: Backup And Restore Support

## Goal

`graphql-orm-backup` needs `graphql-orm` to provide stable database metadata, export, import, restore context, and change-journal APIs.

Provider SDKs, object stores, Dropbox, SMB, and backup repository implementations are out of scope for `graphql-orm`.

## Existing Starting Point

The `/home/toby/graphql-orm` repo already contains early backup metadata primitives in `crates/graphql-orm/src/graphql/orm/core.rs`, including:

```rust
pub struct EntityBackupDescriptor {
    pub entity_name: String,
    pub table_name: String,
    pub primary_key_column: String,
    pub export_order: i32,
    pub restore_order: i32,
    pub columns: Vec<ColumnBackupDescriptor>,
    pub dependencies: Vec<EntityDependencyDescriptor>,
}

pub struct GraphqlOrmSchemaSnapshot {
    pub backend: String,
    pub migration_version: String,
    pub entities: Vec<EntityBackupDescriptor>,
    pub schema_hash: String,
}
```

Please extend and stabilize this surface instead of replacing it.

## Required graphql-orm Capabilities

1. Entity backup descriptors for all registered backup-enabled entities.
2. Stable schema hash and migration version.
3. Backend-agnostic full row export.
4. Backend-agnostic row import into an empty database.
5. Restore context that bypasses policies and change journaling.
6. Optional change journal for true incremental backups.
7. Delete tombstones for incremental restore.
8. SQLite and PostgreSQL tests.

## Proposed Runtime API Shape

```rust
#[async_trait::async_trait]
pub trait GraphqlOrmBackupRuntime {
    async fn schema_snapshot(&self) -> Result<GraphqlOrmSchemaSnapshot, BackupError>;
    async fn export_full(&self) -> Result<Vec<BackupTableExport>, BackupError>;
    async fn export_incremental(
        &self,
        parent_snapshot_id: uuid::Uuid,
    ) -> Result<Vec<BackupChangeExport>, BackupError>;
    async fn restore_full(
        &self,
        export: Vec<BackupTableExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError>;
    async fn restore_incremental(
        &self,
        changes: Vec<BackupChangeExport>,
        context: RestoreContext,
    ) -> Result<(), BackupError>;
}
```

The backup crate currently defines this as an interim adapter contract. The final API should live in `graphql-orm` or be satisfied by a thin adapter.

## Change Journal Direction

Add an optional built-in change journal, probably feature-gated:

```rust
pub struct OrmChangeLog {
    pub id: uuid::Uuid,
    pub entity_name: String,
    pub table_name: String,
    pub primary_key: String,
    pub action: String,
    pub changed_at: i64,
    pub transaction_id: Option<String>,
    pub row_hash: Option<String>,
    pub actor_id: Option<String>,
    pub correlation_id: Option<String>,
}
```

Generated CRUD and runtime insert/update/delete paths should write journal entries unless an explicit restore context disables journaling.

## Please Return

- Final runtime API names and modules.
- Macro changes required.
- Change journal schema.
- Row value representation.
- Import/export implementation plan.
- Tests added.
- Blockers for SQLite/PostgreSQL parity.
