# Runtime values, records, handles, and row decoding

`graphql-orm` provides an owned read-side value boundary for hosts whose
collections are known only through a validated [`RuntimeSchema`](runtime-schema-ir.md).
This release is the foundation for a later runtime select/filter/order executor;
it deliberately does not accept client-provided SQL or execute queries.

## Value and record model

`RuntimeValue` covers every `RuntimeValueKind`: boolean, signed `i64`, finite
`f64`, Unicode string, UUID, JSON, bytes, and datetime. `RuntimeValue::Null` is
an explicit selected SQL `NULL`; it is not an omitted/default write state.
`RuntimeFloat` rejects NaN and infinities and normalizes negative zero.

`RuntimeDateTime` accepts RFC 3339, normalizes offsets to UTC, rounds to the
microsecond precision PostgreSQL stores, and serializes with six fractional
digits (`YYYY-MM-DDTHH:MM:SS.ffffffZ`). This gives SQLite text and PostgreSQL
`TIMESTAMPTZ` the same logical value. It does not infer datetime, UUID, JSON,
or any other kind from an untrusted catalog value: decoding always follows the
validated field's declared `RuntimeValueKind`.

`RuntimeRecord` owns its values and records its `SchemaFingerprint` and stable
`CollectionId`. Values are addressed by stable `FieldId`. A known field absent
from the selected projection returns `RuntimeFieldState::Unloaded`; a selected
SQL `NULL` returns `RuntimeFieldState::Null`. `value` and typed accessors return
stable structured errors for unknown, unloaded, null, wrong-kind, and
schema/collection-mismatch cases. Records and values use deterministic,
versioned Serde representations; invalid record versions and value-kind
inconsistencies fail during deserialization.

## Validated handles and projections

Only `ValidatedRuntimeSchema` creates `RuntimeCollectionHandle`,
`RuntimeFieldHandle`, `RuntimeRelationHandle`, and `RuntimeProjection`. Handles
are owned, contain the stable ID and already-validated physical identifier,
and are bound to the schema fingerprint that created them. Their fields are
private, so an arbitrary client string cannot become a trusted table or column
identifier.

Use:

```rust
# use graphql_orm::graphql::orm::{CollectionId, FieldId, ValidatedRuntimeSchema};
# fn projection(schema: &ValidatedRuntimeSchema) -> Result<(), Box<dyn std::error::Error>> {
let customer = CollectionId::new("customer")?;
let projection = schema.resolve_projection_ids(
    &customer,
    &[
        FieldId::new("customer.id")?,
        FieldId::new("customer.name")?,
        FieldId::new("customer.email")?,
    ],
)?;

assert_eq!(projection.collection().id(), &customer);
# Ok(())
# }
```

Resolution rejects unknown IDs, fields from another collection, duplicate or
empty projection members, and stale handles whose fingerprint no longer
matches. `resolve_relation` also resolves its ordered source/target key pairs,
so later relation work does not need to repeat ownership or key validation.

The owned-handle model permits caching within one validated schema generation.
After catalog activation, resolve fresh handles from the new
`ValidatedRuntimeSchema`; attempting to combine old and new handles produces
`schema_mismatch` before SQL execution.

## Projection-aware decoding

`RuntimeProjection::decode_row::<B>` reads exactly the resolved projection's
fields, in projection order. It neither enumerates nor trusts unexpected row
columns. A missing selected column, incompatible backend type, malformed
portable value, or SQL `NULL` for a non-nullable field returns no partial
record.

The exact Phase 2 integration sequence for a catalog collection named
`Customer` is:

```rust,ignore
let customer = validated.resolve_collection(&CollectionId::new("Customer")?)?;
let id = validated.resolve_field(&customer, &FieldId::new("Customer.id")?)?;
let status = validated.resolve_field(&customer, &FieldId::new("Customer.status")?)?;
let projection = validated.resolve_projection(&customer, &[id.clone(), status.clone()])?;

// The later runtime select executor will render/execute from `projection` and
// invoke this decoder internally. A backend integration fixture can use the
// already-returned backend row directly without copying decoding rules:
let record = projection.decode_row::<SqliteBackend>(&row)?;
assert_eq!(record.string(&status)?, "active");
```

This API does not render a `SELECT`, filter, order, paginate, batch relations,
or expose dynamic GraphQL fields. Those remain later slices. Hosts must not
turn `physical_table()` or `physical_column()` into ad-hoc client-driven SQL;
they are trusted outputs intended for ORM query renderers.

## Backend mappings

| Logical kind | SQLite | PostgreSQL |
| --- | --- | --- |
| boolean | integer decoded as `bool` | `BOOLEAN` |
| integer | `INTEGER` / `i64` | `BIGINT` / `i64` |
| float | finite `REAL` / `f64` | finite `DOUBLE PRECISION` / `f64` |
| string | `TEXT` | `TEXT` |
| UUID | canonical UUID `TEXT` | native `UUID` |
| JSON | validated JSON `TEXT` | native `JSON` / `JSONB` |
| bytes | `BLOB` | `BYTEA` |
| datetime | RFC 3339 `TEXT` | `TIMESTAMPTZ` |

Both decoders use typed driver decoding. They do not fall back to text guesses
when a backend column has the wrong type. SQLite and PostgreSQL set
`RuntimeRowDecoder::RUNTIME_ROW_DECODING_SUPPORTED` to `true`.

MSSQL and the `NoDefaultBackend` compatibility sentinel remain explicit
unsupported capabilities in this slice: their constant is `false`, and the
default decoder returns `unsupported_backend`. Existing MSSQL static generated
reads are unchanged. The repository still requires at least one backend
feature, so a completely backend-free crate build remains intentionally
unsupported; all currently supported backend combinations continue to compile.

Third-party `OrmBackend` implementations remain source-compatible because
runtime decoding is an additive `RuntimeRowDecoder` trait. An implementation
must opt in, decode every kind exactly, and return owned values. It can attach
stable handle context and retain a driver source with
`RuntimeRecordError::new(...).for_field(...).with_source(...)`; safe error
formatting never prints SQL, physical identifiers, raw values, or the source.

## Stable errors

`RuntimeRecordErrorCode` is serialized in lowercase snake case. Its categories
cover handle resolution, stale schemas, field load/null/kind state, missing
columns, backend type mismatches, invalid portable values, unsupported
backends, and invalid serialized records. Stable collection, field, and
relation IDs are available to trusted host diagnostics. The standard error
source chain retains backend detail for server logging only.
