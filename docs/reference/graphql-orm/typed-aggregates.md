---
title: Typed grouped aggregates
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-13
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

An application-side `RowPolicy` cannot safely inspect rows after aggregation;
the builder therefore rejects that configuration. Move the restriction into a
typed SQL filter or database RLS instead of aggregating unauthorized rows and
filtering the result.

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
them into SQL.
