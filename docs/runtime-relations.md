# Validated runtime relation batching

Version 0.13 adds one schema-bound relation layer on top of runtime records and
runtime keyset reads. It is intended for hosts whose collections exist in an
owned `RuntimeSchema`: no compiled entity, SQL, driver row, column-name string,
async-graphql type, or global cache is required.

## Parent anchors and least privilege

Use `runtime_read_request_with_relation_keys` when a later relation layer is
required. It builds the same bounded `RuntimeReadRequest`, but adds the source
primary key and requested relation source fields to the internal decode
projection. `execute_runtime_anchored_read` immediately narrows each public
`RuntimeRecord` back to the caller projection. The additional values exist only
inside a `RuntimeParentAnchor`; they remain `Unloaded` through normal record
access.

Anchors are owned and bound to the validated schema fingerprint, source and
target collections, relation ID, parent identity, and typed relation key. They
do not implement serialization, cannot be constructed by a host, and redact
all values from `Debug` and errors. An anchor with any SQL-null source-key
component resolves locally to no to-one row or an empty to-many connection.
An unloaded key is invalid rather than being treated as null.

Hosts must separately authorize the relation, target projection, target
predicate, target order, and count before constructing a request. Capturing a
key is an execution mechanism, not an authorization decision. Pass the same
canonical `DbAuthContext` used for the parent layer; PostgreSQL executes row and
optional count statements under the existing transaction-local RLS context.

## One bounded relation layer

```rust,ignore
let parent_request = schema.runtime_read_request_with_relation_keys(
    &customers,
    &customer_projection,
    Some(customer_policy_and_application_predicate),
    customer_order,
    RuntimePageRequest::first(50, None),
    false,
    std::slice::from_ref(&contact_details),
    RuntimeQueryLimits::default(),
)?;
let parents = database
    .execute_runtime_anchored_read(&parent_request, auth.as_ref())
    .await?;

let anchors = parents.relation_parents(&contact_details)?;
let relation_request = schema.runtime_relation_batch_request(
    &contact_details,
    anchors.clone(),
    &contact_projection,
    Some(authorized_contact_predicate),
    contact_order,
    RuntimeRelationSelection::ToMany {
        pages: vec![RuntimePageRequest::first(25, None); anchors.len()],
        include_count: true,
    },
    RuntimeRelationLimits::default(),
)?;
let by_parent = database
    .execute_runtime_relation_batch(&relation_request, auth.as_ref())
    .await?;
```

`RuntimeRelationBatch.results` is sorted by the input parent index. Each value
is either `ToOne(Option<RuntimeRecord>)` or `ToMany(RuntimeConnection)`. A
to-one query reads at most two candidates and returns
`cardinality_violation` instead of choosing silently. Every to-many branch
reads at most `page_size + 1`; optional exact counts use precisely the relation
key plus target predicate and no order/page.

Compatible parents are combined into one union statement, not one round trip
per parent. Duplicate relation keys with the same page shape share a branch.
Optional counts use one additional grouped union statement. Different
fingerprints, relations, projections, predicates, effective orders, page
shapes, backends, or auth contexts are separate requests and are never cached
or mixed by graphql-orm. A host can repeat the same flow for a child relation;
the ORM deliberately executes one explicit bounded layer at a time.

## Composite keys, pages, and cursors

Relation key pairs retain declaration order and typed equality. Boolean,
integer, string, UUID, bytes, and canonical datetime keys are portable.
Floating-point and JSON relation keys are rejected before I/O. Values are
always backend parameters; keys are never delimiter-joined, normalized, logged,
or placed in cache/debug strings.

To-many pages use the target's validated total order, including explicit null
placement and missing primary-key tie-breakers. Forward and backward branches
reverse database order as needed but return the same logical order. The
`gormrr1` cursor is distinct from top-level `gormrq1` and static cursors. It
binds the schema fingerprint, relation ID, parent typed identity, target
collection, complete stable-ID order signature, and typed order values. A
cursor for another parent, relation, schema, target, or order returns
`cursor_mismatch` before database I/O. Cursors are integrity envelopes, not
authorization tokens or secrets.

`RuntimeRelationLimits` independently bounds parents, key arity, page size,
bind parameters, cursor bytes, and compatible query groups. Arithmetic and
actual rendered bind counts are checked before execution. Concurrent writes
remain subject to the database transaction/isolation used by the caller; the
API does not claim a durable snapshot outside one.

## Backend and compatibility matrix

- SQLite and PostgreSQL support to-one and to-many batches, ordered composite
  keys, nullable anchors, nested forward/backward keysets, and per-parent
  counts with typed bindings.
- PostgreSQL uses the existing authenticated transaction helper, so row and
  count queries receive the same transaction-local RLS context and cleanup.
- MSSQL and third-party dialects return `unsupported_backend` before I/O until
  exact runtime row decoding and relation execution are implemented. Existing
  static MSSQL relations remain unchanged.

The release adds types and methods only. It does not change `RuntimeRecord`,
`RuntimeConnection`, `RuntimeReadRequest`, `RuntimeQueryLimits`, `OrmBackend`,
`RuntimeRowDecoder`, serialized runtime schema documents, top-level cursor
formats, static generated relations, GraphQL SDL, or database schemas.
Relation predicates on parent collections, dynamic GraphQL registration,
runtime writes, recursive traversal, and aggregates beyond exact per-parent
count remain intentionally deferred.
