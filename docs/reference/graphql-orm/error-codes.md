---
title: "Public ORM Error-Code Contract"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-11
review_by: 2027-02-01
supersedes: []
---

# Public ORM Error-Code Contract

GraphQL clients receive only stable codes and safe messages. SQL text, table names,
constraint names, filesystem paths, and configuration strings stay server-side.

## Codes

| Code | Meaning |
| --- | --- |
| `INVALID_INPUT` | Client input failed validation |
| `UNAUTHENTICATED` | No principal present |
| `STEP_UP_REQUIRED` | Current user session does not satisfy declared operation assurance |
| `FORBIDDEN` | Principal present but not authorized, including a PostgreSQL row-level security `WITH CHECK` refusal or missing privilege (SQLSTATE `42501`) |
| `NOT_FOUND` | Resource missing or not visible (no cross-tenant existence leak) |
| `CONFLICT` | State conflict |
| `CONSTRAINT_VIOLATION` | Database constraint failed without schema leak |
| `CURSOR_INVALID` | Pagination cursor invalid/tampered/version mismatch |
| `PAGE_LIMIT_EXCEEDED` | Requested page size exceeds configured max |
| `SERVICE_UNAVAILABLE` | Temporary dependency failure |
| `INTERNAL_ERROR` | Unexpected failure |
| `AUTHORIZATION_MISCONFIGURED` | A declared policy key has no provider. Under `DeclaredPoliciesRequired` this covers every key; in every mode it covers an entity `write_policy` without a row provider on generated writes |

## GraphQL Extensions

Default extensions:

```json
{
  "code": "FORBIDDEN",
  "correlationId": "optional-host-supplied-id"
}
```

## Database constraint classification

`OrmPublicError::from_sqlx` maps unique, foreign-key and check violations reported
by the driver to `CONSTRAINT_VIOLATION`. SQLite reports a refusal by an
`ON DELETE/UPDATE RESTRICT` foreign key through its internal trigger program,
with extended code `SQLITE_CONSTRAINT_TRIGGER` (1811) rather than
`SQLITE_CONSTRAINT_FOREIGNKEY` (787). The ORM also classifies that case as
`CONSTRAINT_VIOLATION` when the error comes from the SQLite driver, carries
extended code 1811 and has SQLite's exact `FOREIGN KEY constraint failed`
message. Other trigger failures, such as an application `RAISE(ABORT, ...)`,
remain `INTERNAL_ERROR`. Runtime mutations map the same cases to
`constraint_violation`, so a referenced-row refusal has one category on SQLite
and PostgreSQL (SQLSTATE 23503).

Internal diagnostic strings are never placed in extensions. Set
`GRAPHQL_ORM_LOG_INTERNAL_ERRORS=1` to emit internal detail to stderr for local
debugging.

Operation assurance uses the same lowercase extension key `code`. The optional
`auth-agql` evaluator maps upstream evaluation states to `UNAUTHENTICATED`,
`STEP_UP_REQUIRED`, or `FORBIDDEN` without parsing messages. Detailed upstream
denial reasons remain separate from the stable transport category.

Runtime-schema record/handle errors use the separate lowercase
`RuntimeRecordErrorCode` contract because they are host-side runtime IR errors,
not GraphQL responses. The categories include unknown/cross-collection/stale
handles, unloaded/null/wrong-kind values, missing columns, backend type
mismatches, malformed portable values, unsupported backends, and invalid
serialized records. Safe formatting includes stable IDs only; the standard
error source retains driver detail for trusted logging. See
[Runtime records](runtime-records.md#stable-errors).

Runtime query validation and execution use `RuntimeQueryErrorCode`: `invalid_handle`,
`invalid_request`, `invalid_filter`, `unsupported_operator`, `unsupported_order`,
`unsupported_backend`, `resource_limit`, `cursor_invalid`, `cursor_schema_mismatch`, `decode`,
`invalid_search`, and `backend_execution`. Safe formatting omits SQL, physical identifiers, bound values, cursor contents,
and backend detail; the standard error source remains available to trusted logs. See
[runtime queries](runtime-queries.md).

## Code case by surface

The case of `extensions.code` depends on which surface produced the error, and
the split is deliberate. Each surface keeps one case, so a client matches codes
per surface.

- **Generated (static) entity operations and subscriptions** use upper case:
  the codes above, plus `SUBSCRIPTION_CONTINUITY_LOST` and
  `SUBSCRIPTION_SOURCE_CLOSED` on generated `Changed` subscriptions.
  Their structural multi-root admission refusal uses `subscription_root_limit_exceeded`.
  Missing/misconfigured admission on a hand-built schema refuses with
  `SUBSCRIPTION_ADMISSION_REQUIRED`. An event-independent authority timer uses
  `FORBIDDEN` for definite subscriber denial and `SUBSCRIPTION_CONTINUITY_LOST`
  for host/DB failures, once, then ends. All per-event denials stay silent.
- **Runtime GraphQL** (fields installed by a `RuntimeGraphqlModule`) and the
  runtime host APIs use lower case, for example `authorization_denied`,
  `resource_limit`, `cursor_invalid` and `search_index_not_ready`. See
  [runtime GraphQL](runtime-graphql.md) and [runtime search](runtime-search.md).

A schema that composes both surfaces returns both cases. Clients should
compare codes exactly, not case-insensitively.

## Before / After

```rust
// Before (leaks infrastructure)
Err(async_graphql::Error::new(sqlx_error.to_string()))

// After
Err(OrmPublicError::from_sqlx(&sqlx_error).into_graphql_error())
// or
Err(graphql_error_from_sqlx(sqlx_error))
```

## Runtime identity and live streams

IR declarations use `invalid_node_identity` and `invalid_live_subscriptions`.
Runtime composition refuses missing sources with `subscription_source_missing`,
unsupported deleted-image/storage profiles with `unsupported_deleted_image_profile`,
and opaque host/generated subscription root mixing with
`unsupported_subscription_composition`. Key admission uses
`subscription_key_limit_exceeded`: the smaller of the collection and database
`subscription_key_limit` bounds entries in the sole live root's `keys` list,
before deduplication. If an operation selects a generated runtime live root,
multiple root-field occurrences refuse with `subscription_root_limit_exceeded`
before any source, authority or read. Aliases, repeated response names,
fragments and skipped fields count; unused operations do not. Unkeyed/null key
lists are invalid input. Registration read/field refusals remain lowercase and
never release a partial event. During delivery a denied registration or delivery
grant quietly withholds the event and leaves the stream open; other authority
errors, reload errors and restrictions changing at a prepared row/removal's
final release end with `SUBSCRIPTION_CONTINUITY_LOST` then `complete`, independent
of prior visibility. Empty reloads without approved image evidence stay quiet.

Runtime live streams deliberately share the generated exceptions:
`SUBSCRIPTION_CONTINUITY_LOST` and `SUBSCRIPTION_SOURCE_CLOSED` stay uppercase.
Each is a `next` frame carrying `errors[].extensions.code`, followed by `complete`,
with no sequence, identifier or dropped count. The host can explicitly signal
missed batches, restart, restore or schema revisions. See the
[runtime subscriptions guide](runtime-subscriptions.md#continuity-and-generated-clients).
