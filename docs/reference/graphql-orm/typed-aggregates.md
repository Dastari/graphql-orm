---
title: Typed grouped aggregates
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-10-02
review_by: 2027-02-01
supersedes: []
---

# Typed grouped aggregates

`GraphQLEntity` emits a closed aggregate-field enum from public, persisted,
readable fields. The policy-aware builder accepts only that enum: application
code supplies no table name, column name, SQL expression, alias, or backend
fragment.

```rust,ignore
let rows = WorkEntry::aggregate(&database)
    .filter(WorkEntryWhereInput {
        recorded_at: Some(DateTimeFilter { /* typed bounds */ }),
        ..Default::default()
    })
    .group_by(WorkEntryAggregateField::Technician)?
    .group_by(WorkEntryAggregateField::WorkKind)?
    .count_rows()?
    .sum(WorkEntryAggregateField::Hours)?
    .sum(WorkEntryAggregateField::Cost)?
    .group_limit(25)?
    .fetch()
    .await?;
```

The database filters source rows, performs every aggregate, groups the result,
orders groups, and only then applies `group_limit`. An entity page-size ceiling
therefore never truncates aggregate input rows. A group limit must be positive
and cannot exceed the database's `PaginationConfig.max_limit`. At most 16 group
keys and 32 distinct metrics are accepted.

## Operators and result values

The portable operator set is `COUNT`, `MIN`, `MAX`, and `SUM`. `AVG` is not in
the contract because its exact cross-backend numeric result rules are not yet
defined.

- `COUNT(*)` and `COUNT(field)` return `AggregateValue::Count(i64)`; the latter
  excludes nulls under normal SQL semantics.
- Integral `SUM` returns `AggregateValue::Integral(i128)`. PostgreSQL and SQL
  Server widen in the query; SQLite retains its exact integer accumulator and
  reports overflow instead of silently switching to a floating value.
- A field declared with
  `#[graphql_orm(decimal(precision = P, scale = S))]` returns an exact
  `rust_decimal::Decimal`. Portable precision is 1 through 18 and scale cannot
  exceed precision. Values requiring rounding or exceeding the declared range
  fail validation.
- Floating sums return `f64`. No integral or decimal result is silently
  converted to floating point.
- `MIN`, `MAX`, and `SUM` return `AggregateValue::Null` for an empty/all-null
  input. An ungrouped empty query still returns one metric row; a grouped empty
  query returns no rows.
- Nullable group keys are represented by `AggregateValue::Null` and sort first,
  followed by ascending key values. Multiple group keys use declaration order.

Internal projection aliases are ordinal and reserved, so field names cannot
collide with them.

## Authorization

Aggregate execution checks entity read policy and the field policy of every
group key, metric field, and active generated-filter field before issuing SQL.
Generated filters must be completely SQL-renderable. PostgreSQL authenticated
execution keeps the supplied `DbAuthContext`, so transaction-local RLS is
applied before aggregation.

From ORM/macros 0.38.0, both ordinary `fetch` (including generated GraphQL
aggregate resolvers) and `fetch_group_page` accept an installed `RowPolicy` only
when its current decision is `ReadVisibility::Unrestricted` or `Complete`.
Complete SQL predicates are intersected with caller filters before aggregation,
including ungrouped COUNT/SUM and empty-source aggregates. `CallbackOnly` and
`Prefilter` are rejected before the aggregate SELECT: no post-aggregate callback
can recover correct counts or groups. A caller filter never replaces policy.
The host must accurately declare completeness and reevaluate current identity,
ownership and permissions on every request.

Generated aggregate enums carry their entity TypeId to validate predicates.
Handwritten `TypedAggregateField` implementations remain source compatible;
complete visibility requires implementing the optional hidden `entity_type_id`
hook with the associated entity's TypeId. Unrestricted reads need no token.
No group, metric or filter field policy is bypassed by complete visibility.

## Opt-in GraphQL aggregate root

Generated schemas do not gain aggregate operations by upgrading. Opt in on an
entity:

```rust,ignore
#[derive(GraphQLEntity, GraphQLOperations, Clone)]
#[graphql_entity(
    table = "work_entries",
    plural = "WorkEntries",
    aggregate = true,
    auth = "required"
)]
struct WorkEntry {
    #[primary_key]
    id: String,
    #[filterable(type = "string")]
    technician: Option<String>,
    hours: i64,
    #[graphql_orm(decimal(precision = 12, scale = 2))]
    cost: rust_decimal::Decimal,
}
```

This adds `WorkEntriesAggregate` using the configured resolver/argument/field
naming cases. Its inputs are the generated `WhereInput`, aggregate-field enum,
closed metric input, and positive group limit. Its output contains ordered
group and metric entries with an explicit value kind and exact string
representation. Unsupported field/operator pairs fail closed.

The operation catalogue records the root as
`GeneratedGraphqlOperationCategory::Aggregate`. It is discovery and drift
evidence only; normal resolver, entity, field, tenant, RLS, and assurance checks
remain authoritative.

## Decimal storage

Portable Decimal fields require explicit precision/scale metadata. SQLite uses
a checked scaled `i64`; PostgreSQL uses `NUMERIC(P,S)`; SQL Server uses
`DECIMAL(P,S)`. Decimal defaults are exact literals normalized at macro time,
and generated decimal filters bind validated values rather than interpolating
them into SQL. Logical backup rows retain the exact Decimal value together with
its definition, allowing SQLite and PostgreSQL exports to restore without
rounding or treating a native numeric as text.

## Complete SQLite and PostgreSQL text-group pages

ORM/macros 0.35.0 introduced `fetch_group_page`; 0.38.0 adds PostgreSQL
execution. The API remains supported for complete event-name enumeration, with
exactly one text grouping field, optional existing metrics, and ascending `Binary`
or `SqliteNoCase` comparison. Other scalar families, multi-key groups and MSSQL
reject before pool I/O. Existing GraphQL SDL and cursor wire shapes are unchanged. Repository-only entities remain private and need no direct
async-graphql dependency.

```rust,ignore
let page = PrivateEvent::aggregate(&database)
    .filter(PrivateEventWhereInput {
        tenant: Some(StringFilter { eq: Some(verified_tenant.clone()), ..Default::default() }),
        ..Default::default()
    })
    .group_by(PrivateEventAggregateField::Event)?
    .group_limit(100)?
    .fetch_group_page(AggregateGroupPageOptions {
        order: AggregateGroupOrder::SqliteNoCase,
        exclude_blank: true,
        context: trusted_authorization_partition_and_public_revision,
    }, previous_cursor.as_ref())
    .await?;
```

Metrics may be omitted to enumerate distinct original values. Repeat with the
returned `end_cursor` until `has_next_page` is false. Each query returns at most
`group_limit + 1` groups and the page exposes at most `group_limit`; the limit is
1–1,000 and respects a stricter database maximum. The secure default is unchanged.
Use existing `PaginationConfig::legacy()` or an explicit 1,000-row maximum only
when that larger bound is intended. A complete set larger than a single-page cap
does not require increasing that cap or loading the ledger into Rust.

On SQLite, group equality remains the original column's native SQL equality. Ordering does
not case-fold or trim group identities. For deterministic representative values,
paging selects the BINARY minimum original text in each native group. On ordinary
BINARY text storage that is the exact original distinct string. NOCASE storage
can merge text variants under its native equality and returns the original BINARY
minimum variant. `SqliteNoCase` uses SQLite's ASCII folding followed by a BINARY
tie-breaker; SQL NULL sorts first. `exclude_blank` applies native SQL
`TRIM(field) <> ''` before grouping: ordinary spaces/empty strings and SQL NULL
are excluded, padded nonblank strings are retained unchanged, and tabs/newlines
are not removed by SQLite's default TRIM.

PostgreSQL pages group the original TEXT/VARCHAR value under explicit `C`
comparison (`pg_catalog."C"`), even if the source column has a locale-dependent or nondeterministic
collation. Case-distinct original strings therefore remain separate groups;
there is no locale case conversion or identity trimming. `Binary` uses byte
ordering. The historical `SqliteNoCase` spelling also works on PostgreSQL: it
uses `pg_catalog.translate` for only ASCII A–Z to a–z, orders that expression under
`pg_catalog."C"`, then uses
the original value under `C` as tie-breaker. Unicode is retained without folding
or normalization. On UTF-8 databases this matches SQLite's ASCII NOCASE plus
BINARY ordering. PostgreSQL NULL sorts first; default TRIM removes ordinary
spaces only, matching blank exclusion on SQLite. PostgreSQL fixed-width CHAR,
citext/custom storage, numeric and other nontext declarations are not supported
page profiles; TEXT and VARCHAR are. Built-in collation/function names are qualified, so a host search_path cannot
substitute a user-defined C collation or translate function. Blank exclusion also
uses byte comparison: non-empty Unicode values are retained even when a source
ICU collation regards them as equal to empty. Other database encodings are not a Unicode
parity claim. [PostgreSQL collation documentation](https://www.postgresql.org/docs/17/collation.html)
and [ASCII translation](https://www.postgresql.org/docs/17/functions-string.html)
describe the primitives; no application SQL fragment is required.

| Provider | Group pages | Ordinary aggregates with complete SQL visibility |
| --- | --- | --- |
| SQLite | BINARY/native equality; Binary/SqliteNoCase ordering | Supported |
| PostgreSQL | Exact text identity; C/ASCII-folded C ordering | Supported; DbAuthContext/RLS retained |
| MSSQL | Explicitly rejected before pool I/O | Existing COUNT/MIN/MAX/SUM renderers supported |

MSSQL page support remains separate: native text equality, trailing-space rules,
collation, conversion width and provider version require their own faithful
boundary contract. SQLite/PostgreSQL semantics are not silently substituted.
No new provider is implied by portable ordinary aggregates.

Continuation uses HAVING over the same grouping representative and comparisons
as ordering. It never filters source rows with a group cursor, which could
truncate metrics or split a group. Metrics include every authorized source row
in each returned group before the group limit. The database computes groups on
each request; this API does not promise a cross-request snapshot under concurrent
ledger writes. Separate page calls use separate reads/transactions (PostgreSQL's
ordinary statement snapshots), not one pinned repeatable-read snapshot. Insertions
before a consumed boundary may be missed, insertions after it may appear, deletion
or renaming may remove/move groups, and metrics may change between requests.
Stable source rows yield complete enumeration without duplicate/skipped groups;
refresh from a null cursor for a refreshed dropdown. A cursor is not a data revision
or durable replay token. Concurrent authorization changes are always rechecked.

Every request rechecks entity and group/metric/filter field authority. The page
path permits current `ReadVisibility::Complete` SQL predicates or explicit
`Unrestricted`; callback-only or prefilter/residual policies fail closed because
post-aggregate row callbacks cannot produce correct groups/counts. Ordinary
aggregate `fetch` uses the same complete-visibility contract from 0.38.0. SQL
visibility is applied before grouping and pagination, and contributes to cursor identity.

`AggregateGroupCursor::to_json`/`from_json` provide a strict framework-neutral
boundary for host-owned opaque envelopes. The wire object has exactly
`format_version: 1`, a lowercase 64-digit SHA-256 `fingerprint`, and required
`key` (original string or JSON null for SQL NULL). Missing/unknown fields, other
key types and versions are rejected. JSON is at most 256 KiB, text keys at most
64 KiB UTF-8, and the trusted context is 1–4,096 bytes. A last returned grouping
key outside these bounds fails the page rather than emitting an unusable cursor.
These are cursor bounds, not a new record field storage limit.

The fingerprint binds backend, entity/table/group/metric definitions, active
filters, complete row predicate, ordering, blank exclusion, supplied
`DbAuthContext` identity and trusted host context. Changing page size is allowed;
changing these bindings rejects a cursor before the aggregate SELECT. The host
must include its verified authorization partition and complete public/policy
revision in context and validate its outer envelope. Fingerprints do not grant
authority or provide encryption. Protect the complete envelope when the host
requires confidentiality; existing static/runtime cursor formats are unchanged.

The [standalone private consumer example](../../../crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/examples/complete_events.rs)
runs with a disposable SQLite database or the owned PostgreSQL test through ORM
migrations/typed inserts
and collects 1,225 original event types in 100-group pages without application query
SQL or public GraphQL roots:

```sh
cargo run --manifest-path crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml   --locked --no-default-features --features sqlite --example complete_events
```

[Execution regressions](../../../crates/graphql-orm/tests/complete_group_pages.rs)
cover native identities, SQL NULL/blank/padding, literal characters, more groups
than the secure maximum, 1,000 groups plus lookahead, tenant/current policy
isolation, denied fields/entities, strict cursors and complete metrics after
continuation. [Portable execution parity](../../../crates/graphql-orm/tests/portable_group_pages.rs)
covers over 1,000 distinct groups in 100-group and 1,000-group pages, ASCII/case/Unicode
ordering, nullable/blank values, metrics, current tenant policy, ordinary aggregates,
GraphQL field authority, concurrent-write behavior and invalid cursor bindings.
PostgreSQL tests explicitly own disposable loopback containers; MSSQL pages reject
before pool I/O. Run the external PostgreSQL example with:

```sh
cargo test --manifest-path crates/graphql-orm/tests/fixtures/repository-aggregate-consumer/Cargo.toml --locked --no-default-features --features postgres --test portable_groups -- --ignored --test-threads=1
```

 Computed SQL Server grouping/whole-set totals and
joined/computed record queries remain separate capabilities.
