# Runtime select, filter, order, and keyset reads

Version 0.12 adds a bounded read executor for collections that exist only in a
validated `RuntimeSchema`. It builds on the runtime record boundary and does
not require a compiled entity, SQL, column-name strings, driver rows, or a
backend pool in host query code.

## Trust boundary

Only `ValidatedRuntimeSchema` constructs executable predicates, orders, and
requests. Every component carries the schema fingerprint and stable collection
and field IDs. Construction rejects stale or cross-collection handles,
non-filterable/non-sortable fields, wrong value kinds, duplicate order terms,
empty or oversized projections, and resource-limit breaches. Physical
identifiers come only from validated handle state and are quoted by the backend
dialect. Runtime values are always bound parameters.

Hosts must authorize projected, filterable, and sortable fields. graphql-orm
provides structural predicate composition but does not invent product policy.
Compile application and authorization filters separately, combine them with
`runtime_and`, and then execute. `DbAuthContext` may additionally be passed to
`Database::execute_runtime_read` for PostgreSQL transaction-local RLS context;
it does not replace a required structural policy.

## Example

```rust,no_run
# use graphql_orm::graphql::orm::*;
# async fn example<B>(database: &graphql_orm::db::Database<B>, schema: &ValidatedRuntimeSchema) -> Result<(), RuntimeQueryError>
# where B: OrmBackend + RuntimeRowDecoder {
let limits = RuntimeQueryLimits::default();
let customers = schema.resolve_collection(&CollectionId::new("customers").unwrap())?;
let id = schema.resolve_field(&customers, &FieldId::new("customer_id").unwrap())?;
let status = schema.resolve_field(&customers, &FieldId::new("status").unwrap())?;
let projection = schema.resolve_projection(&customers, &[id.clone(), status.clone()])?;

let application_filter = schema.runtime_compare(
    &customers,
    &status,
    RuntimeScalarOperator::Eq,
    RuntimeValue::String("active".to_string()),
    limits,
)?;
let predicate = schema.runtime_and(&customers, vec![application_filter], limits)?;
let order = schema.runtime_order(
    &customers,
    Some(vec![RuntimeOrderInput {
        field: status,
        direction: RuntimeOrderDirection::Asc,
        nulls: RuntimeNullPlacement::Last,
    }]),
    limits,
)?;
let request = schema.runtime_read_request(
    &customers,
    &projection,
    Some(predicate),
    order,
    RuntimePageRequest::first(25, None),
    true,
    limits,
)?;
let connection = database.execute_runtime_read(&request, None).await?;
let next = connection.page_info.end_cursor.clone();
let total = connection.total_count;
# let _ = (next, total); Ok(()) }
```

The same calls work with SQLite and PostgreSQL. MSSQL returns
`unsupported_backend` before I/O because its exact runtime row decoder remains
unsupported; static MSSQL reads are unchanged.

## Operator and logical semantics

| Operator | Kinds |
| --- | --- |
| `runtime_is_null` | every kind |
| `eq`, `ne` | boolean, integer, finite float, string, UUID, bytes, datetime |
| `in`, `not_in` | the same seven scalar kinds |
| `lt`, `lte`, `gt`, `gte`, `between` | integer, finite float, datetime, string |
| `contains`, `starts_with`, `ends_with` | string, case-sensitive |
| JSON | `is_null` only |

Operator names are enums, never host strings. Null literals are rejected by
value operators; use `runtime_is_null`. Lists cannot contain null and are
bounded. JSON equality is deferred because SQLite JSON text and PostgreSQL
JSONB do not share one proven numeric and object-key equality contract.

Empty `and` is true, empty `or` is false, `not` negates one child, empty `in`
is false, and empty `not_in` is true. A null SQL row does not satisfy scalar or
membership comparison; combine an explicit null predicate when it belongs in
the result.

String equality, range comparison, matching, and ordering are case-sensitive.
Order and range comparisons use SQLite `BINARY` and PostgreSQL `C` collation.
Pattern operators use backend functions and bind the whole literal, so `%`,
`_`, quotes, comments, placeholder-shaped text, control characters, and
Unicode remain data. Locale case folding and Unicode normalization are not
claimed.

## Ordering and cursors

Without explicit terms, the validated collection default order is used.
Missing primary-key fields are appended in declared order. Duplicate terms,
JSON order, unsortable keys, or an order that cannot become unique fail before
I/O. Null placement is explicit and reverses with direction for backward reads.

`RuntimePageRequest::first(size, after)` and `last(size, before)` share one
logical order. Sizes are positive and capped. Row queries fetch at most
`size + 1`; offset cursors are not accepted. Versioned `gormrq1` cursors bind
the schema fingerprint, collection stable ID, complete stable-field order
signature, directions, null placement, and typed values. Strict size, arity,
kind, and checksum checks reject malformed or tampered cursors. Cursors are
opaque integrity envelopes, not authorization tokens or cryptographic secrets.

Order fields absent from the output projection are selected only for cursor
construction. Returned `RuntimeRecord` nodes are narrowed back to the caller
projection, so hidden order fields remain `RuntimeFieldState::Unloaded`.

## Limits, count, and consistency

`RuntimeQueryLimits` bounds predicate depth/nodes, list values, total bind
parameters, order terms, projection fields, page size, cursor bytes, and cursor
value count. Hosts may choose smaller limits. `default_page_size` is a host
recommendation; request construction still requires an explicit positive size.

`total_count` is absent unless requested. Its query uses exactly the structural
filter and no page/order. PostgreSQL's authenticated row/count pair uses the
same backend transaction. Calls without an encompassing snapshot must not be
described as a durable count/page snapshot under concurrent writes.

## Backend mapping and compatibility

SQLite binds boolean, i64, finite f64, text, UUID canonical text, bytes, and UTC
microsecond datetime text. PostgreSQL binds native boolean, i64, f64, UUID, and
BYTEA; canonical datetime strings are explicitly cast to `TIMESTAMPTZ` at the
placeholder. Rows use the existing exact `RuntimeRowDecoder`.

No `OrmBackend`, `SqlValue`, `SelectQuery`, legacy/static cursor, generated
entity, or serialized runtime-schema API changed. Third-party backends remain
source-compatible and explicitly unsupported for runtime execution until they
provide exact runtime decoding on a supported dialect. Relations, aggregates
beyond exact count, dynamic GraphQL, runtime writes, and runtime migrations are
intentionally deferred.
