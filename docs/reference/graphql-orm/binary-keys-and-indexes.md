---
title: "Binary Keys and Indexes"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-08-10
review_by: 2027-02-01
supersedes: []
---

# Binary Keys and Indexes

## Binary primary keys

Managed SQLite `BLOB` and PostgreSQL `BYTEA` primary keys use raw `Vec<u8>` values throughout
repository and transaction APIs. CRUD, CAS, byte equality filters, row policies, hooks, and keyset
cursors bind bytes directly; host applications do not encode digest keys as text.

```rust
#[primary_key]
#[filterable(type = "bytes")]
#[sortable]
#[graphql_orm(private, auto_generated = false, min_length = 32, max_length = 32)]
digest: Vec<u8>,
```

`private`, `skip_input`, and GraphQL skip metadata can hide a host-assigned key from public create
inputs without removing it from the trusted Rust `Create...Input`.

## Private repository upserts

An `upsert = "digest"` target may be private when the trusted Rust create input supplies it. If all
target fields are public, the GraphQL upsert field is generated as before. If any conflict-target
field is absent from the public create input, graphql-orm omits that GraphQL field and retains
repository and `MutationContext::upsert` capability.

## Named directional ordinary indexes

The original shorthand remains available and produces an all-ascending index
with a deterministic generated name:

```rust
#[graphql_entity(index = "provider,tenant_key,generation")]
```

Use the nested form when an existing physical contract requires a stable name
or per-column order:

```rust
#[graphql_entity(index(
    name = "idx_snapshot_latest",
    columns = ["provider", "tenant_key", "generation"],
    directions = ["asc", "asc", "desc"]
))]
```

`columns` names Rust fields and is translated through each field's
`db_column`. `directions` is optional; when present it must contain exactly one
`asc` or `desc` value per column. SQLite and PostgreSQL introspection retain
the order, and a direction mismatch plans an explicit drop/recreate rather
than being silently adopted.

## Portable conditional indexes

```rust
#[graphql_orm(conditional_index(
    name = "uidx_jobs_digest_active",
    columns = ["digest"],
    unique = true,
    predicate_field = "status",
    predicate_values = ["APPROVED", "PENDING"]
))]
```

The predicate is typed metadata, not raw SQL. The current portable form accepts a persisted
`String` or `Option<String>` predicate field and an exact closed set of string values. Values are
sorted and deduplicated for stable hashing. SQLite and PostgreSQL definitions are introspected and
canonicalized; missing, narrowed, broadened, non-unique, wrong-column, and wrong-predicate indexes
plan drop/recreate work.

Canonicalization recognizes only the complete generated closed-set grammar. Identifier quoting,
whitespace, redundant balanced outer parentheses, value ordering/deduplication, and PostgreSQL's
catalog-generated `= ANY (ARRAY[...])` text-literal form are harmless. Leading or trailing boolean
expressions, functions, comments retained by SQLite, additional predicates, and unsupported casts
are not interpreted as equivalent. PostgreSQL removes comments from stored index expressions, so a
comment-only source spelling cannot be observed during live introspection there.

Adding a unique conditional index can fail when existing rows inside the selected set have duplicate
keys. Validate and repair data before applying the migration. SQLite and PostgreSQL support this
form; SQL Server does not.

## Strict same-row comparisons

`gt_field`, `gte_field`, `lte_field`, and `lt_field` require persisted fields with the same scalar
Rust type and generate named managed checks on both write backends.

SQL comparisons involving `NULL` evaluate to UNKNOWN, which satisfies a check constraint. Use
non-null fields or separate nullability constraints when the comparison must always be evaluated.

Strict numeric literal bounds are available through `min_exclusive` and
`max_exclusive`, alongside the inclusive `min`, `max`, and `non_negative`
forms:

```rust
#[graphql_orm(min_exclusive = 0)]
generation: i64,
```

Inclusive and exclusive bounds for the same side are mutually exclusive, and
the macro rejects an empty numeric range.
