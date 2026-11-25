---
title: "Public ORM Error-Code Contract"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-17
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
| `CURSOR_INVALID` | Pagination cursor malformed, corrupted, bound to another entity, order or filter, or of another version. A static keyset cursor is not authenticated; see [what a keyset cursor is bound to](pagination-migration.md#what-a-keyset-cursor-is-bound-to) |
| `PAGE_LIMIT_EXCEEDED` | A Rust repository `limit`, a host keyset or scan page size, or a host `EntityQuery::paginate`/`EntitySearchQuery::paginate` limit exceeds the configured (or per-read) maximum, a GraphQL page limit exceeds it while `DatabaseBuilder::refuse_oversized_page_limit(true)` is set (by default it is reduced instead), a client page offset exceeds `DatabaseBuilder::max_page_offset` (10,000 by default; `max_page_offset(None)` leaves client offsets unbounded), a visible-row scan ran out of budget, or a generated GraphQL bulk write selects more rows than `PaginationConfig::max_bulk_write_rows` |
| `SERVICE_UNAVAILABLE` | Temporary dependency failure |
| `INTERNAL_ERROR` | Unexpected failure |
| `AUTHORIZATION_MISCONFIGURED` | A declared policy key has no provider. Under `DeclaredPoliciesRequired` this covers every key; in every mode it covers an entity `write_policy` without a row provider on generated writes |

## One vocabulary with the router

Generated entity roots and `graphql-orm-router` share one `extensions.code`
vocabulary. A client behind the router can therefore match one set of codes,
whichever component refused. The rules:

- Codes are `SCREAMING_SNAKE_CASE` ASCII. A generic category (`FORBIDDEN`,
  `INVALID_INPUT`) has no prefix. A code that only one feature raises is
  prefixed with that feature (`SUBSCRIPTION_`, `PERSISTED_DOCUMENT_`,
  `BYTE_ROUTE_`).
- A code means the same thing wherever it is raised, and a new case reuses an
  existing code when the client's next step is the same. The single-row result
  codes below therefore reuse `NOT_FOUND`, `INVALID_INPUT` and
  `CONSTRAINT_VIOLATION`, and are not new codes.
- New codes and new extensions that would change what an existing client sees
  are opt-in.
- Runtime GraphQL keeps its lower-case codes; see
  [code case by surface](#code-case-by-surface).

The ORM owns its constants: [`OrmErrorCode`](#codes),
`graphql_orm::graphql::subscriptions::{SUBSCRIPTION_CONTINUITY_LOST,
SUBSCRIPTION_SOURCE_CLOSED, SUBSCRIPTION_SELECTION_UNSUPPORTED}` and
`graphql_orm::graphql::errors::MutationOutcome`. The router keeps its own
literals and depends on no ORM package. The constants are not in
`graphql-orm-router-protocol` because that crate holds descriptor data, and
the ORM depends on it only under an optional feature. This table is the
mapping between the two:

| Code | Raised by | Meaning for the client |
| --- | --- | --- |
| `UNAUTHENTICATED` | ORM, router | No principal, or the router's token check failed. Authenticate and retry |
| `FORBIDDEN` | ORM, router | The principal may not do this. Do not retry unchanged |
| `INVALID_INPUT` | ORM, router | The request's input is invalid. Fix it before retrying |
| `SERVICE_UNAVAILABLE` | ORM, router | A dependency is temporarily unavailable. A retry may succeed |
| `SUBSCRIPTION_CONTINUITY_LOST` | ORM, router | The stream ended and missed events. Subscribe again and refetch |
| `NOT_FOUND`, `CONFLICT`, `CONSTRAINT_VIOLATION`, `CURSOR_INVALID`, `PAGE_LIMIT_EXCEEDED`, `INTERNAL_ERROR`, `AUTHORIZATION_MISCONFIGURED`, `STEP_UP_REQUIRED` | ORM | See [codes](#codes) |
| `SUBSCRIPTION_SOURCE_CLOSED`, `SUBSCRIPTION_ADMISSION_REQUIRED`, `INPUT_VALIDATION_REQUIRED` | ORM | See [code case by surface](#code-case-by-surface) |
| `SUBSCRIPTION_SELECTION_UNSUPPORTED` | ORM, opt-in | The `Changed` selection can never be streamed (a relation, a computed field or an unavailable key). Subscribe with a smaller selection, for example keys only |
| `SUBSCRIPTIONS_NOT_SUPPORTED` | router | A subscription was sent over HTTP, or the router has no subscription gateway. Subscribe over the router's `graphql-transport-ws` endpoint |
| `MUTATION_NOT_ALLOWED_OVER_HTTP_GET` | router | A mutation was sent with HTTP `GET`. Send it with `POST` |
| `GRAPHQL_VALIDATION_FAILED`, `REQUEST_TOO_LARGE`, `OPERATION_LIMIT_EXCEEDED`, `INTROSPECTION_DISABLED`, `SUBGRAPH_FAILURE`, `SUBGRAPH_UNAVAILABLE`, `SUBSCRIPTION_LIMIT_EXCEEDED`, `SUBSCRIPTION_AUTHORIZATION_CHANGED`, `SUBSCRIPTION_SCHEMA_RELOAD`, `PERSISTED_DOCUMENT_*`, `BYTE_ROUTE_*` | router | See the [router configuration reference](../../../crates/graphql-orm-router/docs/configuration.md) |

The router's wire tests show a subgraph error's `extensions.code` and
`extensions.outcome` reaching the client unchanged, over HTTP and the
WebSocket gateway.

## Unsupported subscription selection

A generated `Changed` subscription refuses, before it registers, a selection it
can never stream: a relation, a computed field, or a key the subscription
cannot release. By default the refusal carries `INVALID_INPUT`, the code of
other invalid stream input such as a `Keys` list over its bound. With
`DatabaseBuilder::subscription_selection_unsupported_code(true)` (or
`Database::with_subscription_selection_unsupported_code(true)`) it carries
`SUBSCRIPTION_SELECTION_UNSUPPORTED` instead, with the same message, so a
client can fall back to a smaller selection without reading the message.

## Mutation outcome

With `DatabaseBuilder::mutation_error_outcome(true)` (or
`Database::with_mutation_error_outcome(true)`), every error raised by a
generated create, upsert, update, delete, `updateMany` or `deleteMany` root
carries `extensions.outcome` beside its `code`:

| `outcome` | Meaning | Examples |
| --- | --- | --- |
| `refused` | Refused before the root attempted its write. Nothing was written | Authentication, scopes, assurance, an entity or row policy, a write-input transform, a before hook, a stale `expectedVersion` |
| `failed` | The write statement ran, or was sent, and the transaction rolled back. Nothing remains | A constraint violation, an after hook's refusal, a post-image check, a refusal at commit that rolls back |
| `committed` | The write committed. Repeating the mutation applies it again | A `notify` handler or output authorization failing after the commit |
| `unknown` | The server cannot tell | The commit itself failed, for example on a lost connection or a deferred constraint |

A client must read an absent `outcome` as `unknown`. The outcome says whether
the write took effect, not whether a retry would succeed; the `code` says that.
The ORM sets `outcome` itself, replacing any value a host callback put on its
error, so a `notify` handler cannot report a committed write as refused.
Before-hook writes made through the hook's transaction roll back with it and
still report `refused`.

With the setting, a generated bulk write whose commit fails raises a
top-level error with `outcome: "unknown"` instead of returning
`success: false`. A `success: false` payload from any generated root then
always means nothing was written.

Runtime GraphQL mutations already send `committed` and `unknown` from the same
vocabulary on their own errors, without the setting.

An entity with `dependent_deletes` runs its delete through the dependent
delete plan; an error raised once that plan runs reports `unknown`.

## Router refusal outcome

With `RouterConfig::with_refusal_outcome(true)` (`refusalOutcome: true` in the
file configuration), `graphql-orm-router` adds `outcome: "refused"` to
each error of an operation it refused before it sent any subgraph request, so
a client handles a router refusal and a service refusal the same way. It
covers the router's own refusals (bearer and authority checks, root and
nested authorization, the field limit, subscriptions over HTTP, persisted
documents, and the WebSocket gateway's admission refusals) and the engine's
refusals before execution (request, parse, validation, depth, alias and
directive limits, variable coercion, `INTROSPECTION_DISABLED` and
`MUTATION_NOT_ALLOWED_OVER_HTTP_GET`).

The router adds only `refused`, and only before any subgraph request. It never
marks an operation that reached a subgraph, even partly, nor the request
timeout or an execution failure, and it keeps an `outcome` a subgraph sent.
An absent `outcome` still means unknown. The setting is off by default and
changes no code, message, path or HTTP status. See the
[router configuration reference](../../../crates/graphql-orm-router/docs/configuration.md#refusal-outcome).

## Mutation result error codes

A generated single-row `…Result` (create, update and delete) reports some
failures in its payload, with `success: false` and a free-text `error`. An
entity declared with `#[graphql_entity(mutation_error_codes)]` adds a nullable
`errorCode` field (named by the schema's field case) holding the code a
top-level error would carry:

| `error` | `errorCode` |
| --- | --- |
| `Entity not found` (missing or not visible) | `NOT_FOUND` |
| `Entity not found after update` | `NOT_FOUND` |
| `No fields to update` | `INVALID_INPUT` |
| A failed write statement, such as a unique violation | that error's code, such as `CONSTRAINT_VIOLATION` |

The same attribute gives the bulk `Where filter is required…` refusals of
`updateMany` and `deleteMany` the code `INVALID_INPUT`; their other refusals
already carry a code. `errorCode` is null when `success` is true. A
`success: false` from a single-row root means nothing was written. Upsert
reports every failure as a top-level error, so its result has no `errorCode`.
An upsert result with `success: true` and `action: null` is an
[acknowledged retry](runtime-and-writes.md#acknowledging-a-retry-without-writing):
nothing was written and the row is the existing one. Only an entity that
declares `upsert_acknowledge` returns it.

Without the attribute the single-row result types, the SDL and the bulk codes
are unchanged. Rust code that builds a single-row result with a struct literal
must add `error_code: None` once the attribute is set; the constructors `ok`
and `err` are unaffected. On an entity with `dependent_deletes`, the root
delete reports the same `error` and `errorCode` as a direct delete, including
for a root row the plan refuses once its transaction holds the row.

`errorCode` holds the code of the error that failed the write, and that error
can come from the host. When a host callback, such as a mutation hook,
refuses a write with its own `extensions.code`, a bulk
`updateMany` or `deleteMany` payload reports that code as `errorCode`, with
or without the attribute, so `errorCode` is not always an
[`OrmErrorCode`](#codes) value. A single-row root reports such a refusal as a
top-level error carrying the same code. Clients should treat a code they do
not recognise as opaque.

## GraphQL Extensions

Default extensions:

```json
{
  "code": "FORBIDDEN",
  "correlationId": "optional-host-supplied-id"
}
```

A `CURSOR_INVALID` error from `EntityQuery::fetch_authorized_page` adds a
`reason` extension in two cases: `AUTHORIZED_PAGE_CURSOR_EXPIRED` for a cursor
that authenticated under the current context binding and is past its lifetime,
and `AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE` for a cursor that names a key
identifier the service does not accept. Every other refused cursor has no
reason. See
[context-bound authorized pages](pagination-migration.md#context-bound-authorized-pages-for-handwritten-resolvers-01620).

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

Internal diagnostic strings are never placed in extensions.

## Server-side diagnostics

The ORM reports diagnostics as `tracing` events and writes nothing to standard
output or standard error. Install a `tracing` subscriber to receive them.

| Target | Level | Event |
| --- | --- | --- |
| `graphql_orm::internal_error` | `ERROR` | an `INTERNAL_ERROR`, `SERVICE_UNAVAILABLE` or `AUTHORIZATION_MISCONFIGURED` response; field `code` |
| `graphql_orm::internal_error` | `DEBUG` | any other public error that carries a diagnostic; field `code` |
| `graphql_orm::post_commit` | `ERROR` | a post-commit action failed and no `PostCommitErrorHandler` is installed; field `error` |
| `graphql_orm::subscriptions` | `WARN` | `Changed` events are dropped for a misconfigured entity (once per process); field `entity` |
| `graphql_orm::migrations` | `WARN` | an unowned SQLite rebuild table was preserved; field `table` |

An internal error event carries the diagnostic text, in the `detail` field,
only while `GRAPHQL_ORM_LOG_INTERNAL_ERRORS` is set. That text can quote a
driver message or a host callback's error, so it stays an explicit opt-in. The
ORM never formats a row or a bound value into an event.

## Authorization refusals keep their code

An authorization refusal reaches the caller as `UNAUTHENTICATED`, `FORBIDDEN`
or `AUTHORIZATION_MISCONFIGURED` on every path: generated resolvers,
repository helpers, transaction helpers and relation boundaries. It is never
reported as `INTERNAL_ERROR`, and a host failure is never reported as a
refusal:

| Callback outcome | Repository field policy callbacks | Relation boundary |
| --- | --- | --- |
| `Ok(false)` | `FORBIDDEN` (a write denial names the field) | `FORBIDDEN` |
| authored error with an ORM public `code`, or a typed `OrmPublicError` returned with `?` | that code, with its message | `UNAUTHENTICATED`, `AUTHORIZATION_MISCONFIGURED`, `INTERNAL_ERROR` and `SERVICE_UNAVAILABLE` keep their code; any other refusal is `FORBIDDEN`; no message |
| authored error without a code, or with a host-specific code | `INTERNAL_ERROR`, default message | `FORBIDDEN`; no message |
| propagated error (it has a source) | `INTERNAL_ERROR` or `SERVICE_UNAVAILABLE`, default message | the same failure class; no message |

The text of an uncoded callback error is a server-side diagnostic only, on
these paths and for every other callback family (entity policy, row policy,
write-input transform and mutation hook), also when a custom resolver maps a
repository error with `graphql_error_from_sqlx`. Generated GraphQL operations
report it as `INTERNAL_ERROR` with the default message unless the schema's
`Database` opted in with `DatabaseBuilder::expose_uncoded_callback_messages(true)`. The rule is stated once in
[policy callback errors](strict-authorization.md#policy-callback-errors). A missing field policy provider for a declared key
is `AUTHORIZATION_MISCONFIGURED` in both strict modes.

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
  `SUBSCRIPTION_ADMISSION_REQUIRED`, and a hand-built schema without
  `GeneratedInputValidation` refuses every generated resolver that takes an
  input object with `INPUT_VALIDATION_REQUIRED`. An event-independent authority timer uses
  `FORBIDDEN` for definite subscriber denial and `SUBSCRIPTION_CONTINUITY_LOST`
  for host/DB failures, once, then ends. A stream that lagged ends on that
  timer's next tick, with `FORBIDDEN` when its authority is gone and
  `SUBSCRIPTION_CONTINUITY_LOST` otherwise. All per-event denials stay silent.
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
Lag ends a stream on its next jittered tick: with `authorization_denied` when
the subscriber is then denied, otherwise with `SUBSCRIPTION_CONTINUITY_LOST`.

A runtime page whose boundary row has order values too long for a cursor
(usually one long stored string sort key) is refused with
`cursor_key_too_large`, on root and relation connections. Order by a bounded
field, or raise `max_cursor_bytes` and the cursor protection limits.

Runtime live streams deliberately share the generated exceptions:
`SUBSCRIPTION_CONTINUITY_LOST` and `SUBSCRIPTION_SOURCE_CLOSED` stay uppercase.
Each is a `next` frame carrying `errors[].extensions.code`, followed by `complete`,
with no sequence, identifier or dropped count. The host can explicitly signal
missed batches, restart, restore or schema revisions. See the
[runtime subscriptions guide](runtime-subscriptions.md#continuity-and-generated-clients).
