---
title: "Pagination Migration Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-09-20
review_by: 2027-02-01
supersedes: []
---

# Pagination Migration Guide

## What Changed In 0.3.0

| Setting | Before (≤0.2.x) | After (0.3.0 default) | Legacy restore |
| --- | --- | --- | --- |
| Default limit | 1000 | 50 | `PaginationConfig::legacy()` |
| Max limit | 1000 | 100 | `PaginationConfig::legacy()` |

```rust
// Secure defaults (crate default)
let database = Database::new(pool);

// Restore previous limits during migration
let database = Database::new(pool)
    .with_pagination_config(PaginationConfig::legacy());

// Trusted internal jobs only
let database = Database::builder(pool)
    .unbounded_pagination()
    .build();
```

## Cursor Models

Existing generated connections continue to use offset-style cursors
(`encode_cursor(offset)`). Base64 alone is not a security boundary. Entities
with an explicit keyset order also have versioned, order-fingerprint-bound
opaque keyset cursors.

Version 0.7 adds the repository-only `keyset_connection_page` helper for
bounded bidirectional windows:

```rust
let tail = Event::keyset_connection_page(
    &database,
    EventWhereInput::default(),
    KeysetConnectionInput {
        last: Some(50),
        ..Default::default()
    },
).await?;

let older = Event::keyset_connection_page(
    &database,
    EventWhereInput::default(),
    KeysetConnectionInput {
        before: tail.page_info.start_cursor,
        last: Some(50),
        ..Default::default()
    },
).await?;
```

Backward database reads are returned in the entity's canonical order. Page
sizes are clamped by `PaginationConfig`, and total counts remain explicit
opt-in work.

- treat all cursors as opaque positions, not secrets
- restart traversal when an order fingerprint changes
- prefer bounded keyset windows for deep or append-heavy timelines
- do not expose unbounded GraphQL lists

## Field Selection And Counts

Legacy offset connections compute exact `totalCount` even when it is not
selected. Keyset connections and authorized scans provide explicit count opt-in.
Repository `fetch_all` remains intentionally unbounded for trusted internal use.

## Authorization-aware pagination (0.33.0)

`RowPolicy::read_visibility` is a defaulted, request-local method. Existing
implementations compile unchanged and return `CallbackOnly`. The provider
receives the same context, entity, policy key, and access surface as the row
callback. It must return one of these explicit decisions:

| Decision | SQL restriction | Residual callback |
| --- | --- | --- |
| `CallbackOnly` | Caller filters | Every candidate |
| `Prefilter(predicate)` | Caller filters AND predicate | Every candidate |
| `Complete(predicate)` | Caller filters AND predicate | None on supported pagination paths |
| `Unrestricted` | Caller filters | None on supported pagination paths |

A complete predicate is a security contract: it must fully describe read
visibility. If a callback can still reject a row, return `Prefilter` instead.
`Unrestricted` is an explicit grant for this request/entity/surface; absence of
a predicate, role names, and samples of allowed rows never imply this grant.
An installation with no RowPolicy retains its existing entity-policy-only
behavior. Provider errors abort the request before querying. An entity/backend mismatch,
empty typed predicate, or filter requiring Rust matching also fails closed.

Build predicates from generated filters, including equality, `in_list`, and
nested `and`/`or` inputs. Values use the existing parameterized expression
renderer. For example, inside a provider's `read_visibility` implementation:

```rust,ignore
// `subject_id` and `permitted_ids` are obtained from your verified request
// identity and permission source. These are illustrative entity/field names.
let filter = RecordWhereInput {
    owner_id: Some(StringFilter {
        eq: Some(subject_id),
        ..Default::default()
    }),
    id: Some(UuidFilter {
        in_list: Some(permitted_ids),
        ..Default::default()
    }),
    ..Default::default()
};
let predicate = ReadPredicate::from_filter::<SqliteBackend, _>(&filter)?;
Ok(ReadVisibility::Complete(predicate))
```

`ReadPredicate::and` and `or` compose predicates for the same entity/backend.
`ReadPredicate::related::<Backend, Source, Target>(&[("source_column",
"target_column")], target_predicate)` provides a validated, parameterized
EXISTS equality relation, including composite key mappings. Column names are
checked against persisted entity metadata; clients never submit SQL. This is
an explicit key relation, not a named relationship resolver. Include any
source/target discriminator conditions in the respective predicates. Target
row/entity policies are not automatically imported into the source policy.
Self-relations are supported; arbitrary join expressions are not.

Entity access, resolver authentication, exact operation scopes, assurance,
GraphQL field policies, database session authorization, and repository field
policies remain independent checks. A complete row decision grants none of
these. Single-record, mutation, relation-loader, search, aggregate, and legacy
transaction APIs retain their existing authorization contracts; their row
callbacks must still be implemented.

### Exact offsets remain compatible

Generated ordinary list resolvers use database pagination and authorized SQL
counts for `Complete` and `Unrestricted`. Their `totalCount`, visible offsets,
and existing offset cursor contract remain unchanged. With `CallbackOnly` or
`Prefilter`, the exact visible offset and exact total still require loading and
checking all candidates. Partial predicates reduce that candidate set but do
not eliminate this cost. SQLite spatial caller filters that require decoded
entity matching also retain their scan-dependent behavior.

A bounded SQL result limits transferred/decoded rows; it does not guarantee a
bounded execution time. Exact COUNT, sorting, predicate selectivity, and query
plans can still require substantial database work.

### Keyset connections

Entities with `keyset = "priority asc nulls last, id asc"` already declare a
stable order ending in their single-column primary key. Their generated
GraphQL `recordsKeyset` and standalone Rust `Record::keyset_page` and
`Record::keyset_connection_page` now accept complete database visibility.
Caller filters, counts, and both pagination probes use the same decision.
Counts remain opt-in with `includeTotalCount`; omission executes no COUNT.
The standalone Rust helpers now use the same policy-aware executor as GraphQL.
GraphQL callback-only or partial policies must use scans or exact list offsets.
Standalone repository keyset helpers preserve bounded fail-closed callback
reads: every page candidate, lookahead, and opposite probe must pass the
callback, or the request fails. These legacy reads do not advance through
denials and still reject callback totals. Use scans to advance safely.

The existing `MutationContext::keyset_page` / `keyset_connection_page`
transaction methods retain their previous callback checks and rejection of
row-policy totals. They do not yet consume database visibility decisions.
Use the standalone authorized executor for the new capability. Handwritten
pool-bound queries are still low-level queries and do not automatically apply
application authorization.

### Opt-in bounded scans

Install server-owned limits and a secret, randomly generated 32-byte key:

```rust,ignore
let database = database.with_authorized_scan_config(
    AuthorizedScanConfig::new(cursor_encryption_key, 256, 4096),
);
```

Keyset-enabled generated entities expose `recordsScan`, with the same
operation scopes and assurance classification as `recordsKeyset`. The endpoint
fails closed until scan configuration is installed. Rust callers can use
`Record::authorized_scan` or `EntityQuery::fetch_authorized_scan` (the latter
also accepts request context and database authorization). All persisted fields
are decoded for residual callbacks.

```graphql
query {
  recordsScan(page: { limit: 50 }) {
    nodes { id }
    pageInfo { status continuation totalCount }
  }
}
```

Pass `pageInfo.continuation` back as `page.after` even when `nodes` is empty.
Each request re-resolves entity and row authorization. Queries advance through
bounded database batches, including wholly denied batches, using the declared
keyset order. The cursor records the last examined candidate, not a visible
row count. Equal values and null placement use the existing keyset renderer;
the primary key supplies a unique tie-breaker.

| Status | Meaning | Client action |
| --- | --- | --- |
| `EXHAUSTED` | No remaining candidate in the observed query | Stop; continuation is absent |
| `PAGE_FULL` | The requested number of visible rows was returned | Resume if desired; another visible row is not promised |
| `BUDGET_EXHAUSTED` | The row budget was consumed before filling the page | Resume, including after an empty page |

There is deliberately no `hasNextPage` on scan results. At an exact budget or
page boundary, exhaustion may require one more request. `includeTotalCount`
defaults to false. It is supported only with complete database authorization;
requesting it with residual authorization fails before row fetching. Use the
legacy exact list if an exact callback-based total is essential.

Scan cursors encrypt and authenticate their underlying values with
XChaCha20-Poly1305 so denied sort values are not disclosed. They are bound to
the entity and order; changing the encryption key invalidates outstanding
cursors. Share the key securely across instances that accept the same cursors.
Do not put it in client configuration. Cursors are positions, never credentials:
cross-user reuse still applies the receiving user's permissions. They do not
bind a filter or freeze permissions; restart traversal when changing filters.

The budget bounds fetched/decoded candidates and callback invocations per
request, not callback latency or total database execution work. Sparse
visibility can require many requests and substantial scanning. No arbitrary
Rust callback can provide constant-time authorization pagination.

### Consistency and backend boundaries

Pages and permission callbacks do not form a snapshot across requests.
Revocations affect subsequent requests; grants behind the cursor are not
revisited. Inserts before the cursor are omitted, deletions disappear, and
updates to sort keys can move rows across the position. Stable data and stable
permissions give duplicate-free traversal without skipped candidates.
Counts and probes are separate statements and can differ under concurrent
writes according to backend isolation; they always include the same SQL
visibility restriction. A permission change during one request has the same
request-local decision boundary as other authorization checks.

SQLite, PostgreSQL, and SQL Server use the shared parameterized renderer and
bounded backend execution. The generated keyset capability currently requires
a single-column primary key, supported persisted scalar sort keys, and an
explicit final unique tie-breaker. Runtime-discovered schema queries,
composite-primary-key entities, computed ordering expressions, residual caller
filters, and named conditional relationship inference are outside this new
scan capability. Residual caller filters fail explicitly rather than silently
running an unbounded keyset scan.

`ReadQueryObserver`, installed with `Database::with_read_query_observer`, reports
actual fetched rows and parameterized SQL for `EntityQuery` reads. It excludes
bind values and should avoid blocking query execution. It does not intercept
all low-level driver/transaction APIs. Integration coverage and the reproducible
release benchmark live in
[`authorized_pagination.rs`](../../../crates/graphql-orm/tests/authorized_pagination.rs).

The [release benchmark and query evidence](../../archive/2026/authorization-pagination-benchmark.md)
records measured row transfers, callback counts, latency, process memory, and
backend verification commands.
