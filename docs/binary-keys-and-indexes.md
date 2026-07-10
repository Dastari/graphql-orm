# Binary Keys and Conditional Indexes

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

Adding a unique conditional index can fail when existing rows inside the selected set have duplicate
keys. Validate and repair data before applying the migration. SQLite and PostgreSQL support this
form; SQL Server does not.

## Strict same-row comparisons

`gt_field`, `gte_field`, `lte_field`, and `lt_field` require persisted fields with the same scalar
Rust type and generate named managed checks on both write backends.

SQL comparisons involving `NULL` evaluate to UNKNOWN, which satisfies a check constraint. Use
non-null fields or separate nullability constraints when the comparison must always be evaluated.
