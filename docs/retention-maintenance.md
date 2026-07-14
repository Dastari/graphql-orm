# Bounded append-only retention maintenance

Append-only entities continue to omit update, upsert, replace, and delete APIs.
Their managed triggers also reject ordinary database updates and deletes. A
host that owns a regulated retention policy can opt one entity into a separate,
bounded physical-purge capability:

```rust
#[derive(GraphQLEntity, GraphQLOperations, Clone, Debug, Serialize, Deserialize)]
#[graphql_entity(
    table = "audit_events",
    plural = "AuditEvents",
    append_only = true,
    retention_purge = "audit.retention.purge"
)]
struct AuditEvent {
    #[primary_key]
    id: Uuid,
    #[filterable(type = "integer")]
    created_at: i64,
    payload: Vec<u8>,
}
```

`retention_purge` is the dedicated entity-policy key. It is rejected unless the
entity is append-only and generated for a managed writable SQLite or PostgreSQL
backend. The key is recorded in entity metadata; the capability itself is
recorded in table models, stable schema fingerprints, module fingerprints, and
backup descriptors. Existing append-only entities do not become purgeable.
Existing row-write policy checks continue to receive the entity's normal
`write_policy` key on the distinct retention-maintenance surface.

## Host-only API

Register an `EntityPolicy` that explicitly allows the policy key on
`EntityAccessSurface::RetentionMaintenance`, then use the dedicated runner:

```rust
let result = database
    .retention_transaction_with_auth(Some(&db_auth), |maintenance| {
        Box::pin(async move {
            let result = maintenance
                .purge::<AuditEvent>(
                    AuditEventWhereInput {
                        created_at: Some(IntFilter {
                            lt: Some(cutoff),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    MutationLimit::new(500)?,
                )
                .await?;

            maintenance
                .insert::<RetentionAuditFact>(CreateRetentionAuditFactInput {
                    policy: "audit.retention.purge".into(),
                    cutoff,
                })
                .await?;
            Ok(result)
        })
    })
    .await?;

match result {
    RetentionPurgeOutcome::Purged { affected } => { /* exact count */ }
    RetentionPurgeOutcome::LimitExceeded { maximum } => { /* no rows changed */ }
}
```

The runner always uses state-machine isolation: SQLite acquires `BEGIN
IMMEDIATE` before the callback can read, and PostgreSQL selects `SERIALIZABLE`
before application statements. A purge requires the generated entity type, its
generated nonempty typed filter, and a nonzero `MutationLimit`. Residual
in-memory filters are rejected. Matching rows are selected in complete
primary-key order with one look-ahead row; overflow returns `LimitExceeded`
before deletion. The database delete then has to affect exactly the selected
cardinality or the transaction fails closed. A retention-enabled entity cannot
declare a self-referential `ON DELETE CASCADE`: such a cascade could delete
additional rows of the same entity beyond the explicit maximum.
Any purge error poisons the retention context, so catching that error inside
the callback cannot accidentally commit partial maintenance work. A
`LimitExceeded` outcome is not an error and deliberately leaves the transaction
usable for a separate audit append.

`RetentionContext` exposes typed reads/projections, normal generated inserts,
and generated purge. It exposes no pool, connection, executor, raw row, table
name, column name, SQL fragment, truncate, update, or unbounded delete. It
cannot be constructed by a host or escape the callback lifetime. A normal
`Database::transaction` receives `MutationContext`, which has no purge method.
No GraphQL field or input is generated for retention maintenance.

Successful purges use the entity's existing generated delete-event path, so
search cleanup, relation-change propagation, and ordinary entity changed-event
subscribers retain their normal semantics. Those existing changed events carry
the entity shape defined by the application. `RetentionPurgeEvent` is an
additional redacted summary containing only entity/table identity and the exact
count; it is not a replacement for existing event contracts.

This release deliberately implements physical deletion only. It does not
provide append-only updates or protected-field tombstoning. A future update
form would require a separately reviewed field allowlist and immutable identity
checks.

## Enforcement and cleanup

SQLite uses a reserved `__graphql_orm_retention_context` table. A retention
runner holds the SQLite write lock, adds the exact generated table marker in
the same uncommitted transaction immediately around the generated DELETE, and
removes it before after-hooks or host code can run. The DELETE trigger allows
only that marker; UPDATE remains unconditionally denied.
Rollback, callback panic, cancellation, or connection drop rolls the marker
back with the row changes; panics are not caught by the runner, and transaction
drop performs the rollback while unwinding. Introspection validates the
complete triggers, the exact internal table shape, and that no committed marker
remains.

PostgreSQL uses the transaction-local
`graphql_orm.retention_entity` setting. The generated security-definer trigger
function permits DELETE only when the setting equals its exact table and always
rejects UPDATE. The ORM clears it immediately after the generated DELETE and
again before commit; PostgreSQL also clears it on transaction completion.
Function language, owner, trigger shape, fixed
`pg_catalog` search path, privilege revocation, and complete function body are
structurally introspected. If the entity enables managed RLS, graphql-orm adds
an exact internal DELETE policy for the same transaction-local setting.

The database account used by application code must not be offered as a generic
SQL execution service. A principal that can execute arbitrary SQL with those
credentials is already outside the ORM boundary and can attempt to manipulate
backend transaction state. This boundary includes callers that deliberately use
the public compatibility escape hatch `MutationContext::executor()`: that API
can issue arbitrary SQL and therefore does not inherit the generated retention
policy, predicate, or cardinality guarantees. Security-sensitive maintenance
code should expose only the narrow `RetentionContext` callback to untrusted
components. Schema ownership/migration credentials should remain separate from
runtime credentials on PostgreSQL.

Capability changes produce an explicit `SetAppendOnly` migration step that
recreates the enforcement contract. A recorded module or host migration version
with remaining trigger/context/RLS work fails closed. Review existing data and
foreign-key behavior before enabling physical purge, and apply under a fresh
migration/module version. `MutationLimit` bounds rows selected and directly
deleted from the retained entity; database cascades into ordinary child tables
are not included in that count and can affect more child rows than the limit.
A cascade into another append-only table remains blocked by that table's exact
retention trigger and rolls the transaction back. Avoid cascades or account for
their independent cardinality when selecting a retention batch size.

## Verification

SQLite coverage runs with the normal test matrix. PostgreSQL parity owns its
container, generated credentials, database, and cleanup; it never reads
`DATABASE_URL` or `TEST_DATABASE_URL`:

```bash
cargo test -p graphql-orm --no-default-features --features postgres \
  --test retention_purge_postgres -- --ignored --nocapture
```

MSSQL remains read-only and intentionally generates no retention runtime
capability. Schema metadata rejects the writable retention opt-in for MSSQL.
