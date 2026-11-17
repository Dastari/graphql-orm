---
title: "Pagination Migration Guide"
kind: reference
status: active
owner: graphql-orm-maintainers
last_reviewed: 2026-11-16
review_by: 2027-02-01
supersedes: []
---

# Pagination Migration Guide

Generated search fallback always requires the host scan configuration, including
when no `RowPolicy` is registered, because database RLS is not discoverable from
that registry. Native Complete/Unrestricted search keeps SQL count/page execution.

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

Since 0.92.0, static offset connections use the selected fields to omit unused
SQL while preserving selected values and authorization. No data migration is needed.

Generated top-level and relation connections inspect schema field names through
fragments and aliases. `edges`, `startCursor` or `endCursor` require page rows;
`totalCount` requires a count. `hasNextPage` without a count uses one extra row
(and truncates the probe before returning edges). Selecting only
`hasPreviousPage` requires no SQL for the connection. Relation batch/cache
identity includes selection requirements. Residual row policies retain the
authorized scan, even for count-only requests; totals count readable rows only.

Direct `fetch_connection` and `fetch_connection_with_auth` retain the full
contract. `EntityQuery::fetch_connection_selected_with_auth` accepts
`graphql::pagination::ConnectionSelection { rows, count, has_next_page }`;
unrequested components are placeholders and must not be exposed by callers.
`ConnectionSelection::ALL` requests the full contract; `from_context(ctx)`
provides generated resolver selection requirements.

When rows and count are both required, the existing paired fetch retains its
pinned database auth transaction. Keyset connections and authorized scans keep
their explicit count opt-in. `EntityQuery::fetch_all` without a page reads
every row, for trusted internal use. With a `paginate(page)` page, `fetch_all`,
`fetch_all_with_auth`, `fetch_all_on` and `fetch_one*` honour `page.limit`
exactly or fail with `PAGE_LIMIT_EXCEEDED`, like the connection reads (see
[page size](#page-size-reduced-by-default-refused-on-request)).

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

`ReadPredicate::column::<Backend, Entity>(column, ReadPredicateOp::Gt,
SqlValue::Int(bound))` validates the physical name against `Entity::columns()`,
quotes it, and binds the value. It accepts private (`read = false, filter = false`)
persisted columns without adding them to `WhereInput`. Operators are `Eq`, `Ne`,
`Lt`, `Lte`, `Gt`, and `Gte`; NULL comparisons use ordinary SQL semantics.

`ReadPredicate::and` and `or` compose predicates for the same entity/backend.
`predicate.not()?` uses SQL `NOT`: TRUE becomes FALSE, FALSE becomes TRUE,
and NULL/UNKNOWN remains UNKNOWN (denied in WHERE). It never treats UNKNOWN as
FALSE before negating. Empty SQL fragments and empty conjunctions fail closed; an empty disjunction
is the supported constant FALSE. Relation
EXISTS is two-valued: `exists_any.not()?.or(exists_matching)?` means absent or
matching. For a present related row whose comparison is UNKNOWN, matching does
not grant access. Avoid `not(exists(not(matches)))` when NULL must be denied:
NULL comparisons inside EXISTS do not match rows.
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
these. Single-record, mutation, aggregate and legacy transaction APIs retain their
separate authorization contracts. Generated protected search now consumes the
same SQL visibility contract; residual callbacks still require implementation.

### Exact offsets remain compatible

Generated ordinary list resolvers use database pagination and selected authorized
SQL counts for `Complete` and `Unrestricted`. Their `totalCount`, visible offsets,
and existing offset cursor contract remain unchanged. With `CallbackOnly` or
`Prefilter`, exact visible offsets and totals require examining all candidates,
but 0.46 performs that work in finite SQL batches using an explicitly installed
`AuthorizedScanConfig`. Only the requested visible window is retained. Missing
bounds, a filter requiring whole-result in-memory matching, or failure to prove
exhaustion within the row budget returns `AUTHORIZATION_MISCONFIGURED` without
partial nodes/counts. An empty denied batch is not end-of-data. Partial SQL
predicates reduce the candidate set; they do not remove the need for a budget.

Generated protected search preserves native SQLite/PostgreSQL ranking with
bounded candidate batches, or scores bounded raw fallback batches while
retaining only the requested highest-score window. Fallback budgets count
nonmatching raw candidates too. Complete native SQL count/page reads remain
available; protected fallback still requires a budget. See the
[0.46 migration](../../../MIGRATION.md#0450-to-0460-bounded-protected-offset-and-search-reads)
for host configuration and AI/tool consumer failure handling.

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
Such a request is refused with a distinct reason. Schema registration refuses a
Keyset root whose row policy explicitly declares residual-only visibility; see
[Keyset roots and residual-only read policies](macros-and-attributes.md#keyset-roots-and-residual-only-read-policies).
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

### Page size: reduced by default, refused on request

A generated GraphQL page that asks for more rows than the database's maximum
page limit (`PaginationConfig::max_limit`, `100` by default) is served the
maximum, and nothing in the response says the size was reduced. A client that
computes page counts from `totalCount` and the size it asked for then gets
them wrong, and cannot tell a reduced page from a last page.

Set `DatabaseBuilder::refuse_oversized_page_limit(true)` (or
`Database::with_oversized_page_limit_refusal(true)`) to refuse instead.
An explicit `page.limit` on a list root, to-many relation or search
connection, or an explicit `limit`, `first` or `last` on a keyset connection,
above the maximum then fails with `PAGE_LIMIT_EXCEEDED` before any row is
read. The message names the limit asked for and the maximum. A limit at the
maximum and an omitted limit are served as before. The setting is off by
default, so existing responses do not change. A custom resolver that takes a
client `PageInput` must resolve it with
`Database::client_pagination_config().resolve_client_page` before passing it
to `paginate`, as generated resolvers do, so the client's limit is reduced, or
refused when the setting is on. The setting does not apply to host reads: a
`PageInput` that reaches `paginate` directly is a trusted request and is
exact or refused, as below.

A schema's maximum page limit is fixed at compile time:
`schema_roots!(max_page_limit: N, ...)`, or 100 when the declaration is
omitted. The generated module exposes it as `GRAPHQL_ORM_MAX_PAGE_LIMIT`, the
semantic catalogue publishes it as the ceiling of every pageable relationship
list, the relation complexity estimate caps a requested page at it, and
building the schema panics when the `Database` applies a different maximum or
none. The published bound and the estimate therefore always agree with what
the database applies. With several schemas in one process each complexity
estimate uses its own schema data; bounds are never shared between schemas.

Rust repository reads are separate: trusted code gets the rows it asked for
or an error. `limit(n)` on a repository or transaction builder is exact or
refused, and `with_max_limit(n)` gives one trusted read its own maximum (see
the 0.128.0 migration note). The keyset helpers (`Record::keyset_page`,
`keyset_connection_page`, `tx.keyset_page`, and `fetch_authorized_keyset*` or
`fetch_authorized_scan` called without a GraphQL context) follow the same
rule: an explicit limit above the maximum fails with `PAGE_LIMIT_EXCEEDED`,
and an omitted limit uses the default limit, reduced to the maximum. Called
with a GraphQL context, `fetch_authorized_keyset*` and `fetch_authorized_scan`
treat the limit as a client's: it is reduced, or refused when
`refuse_oversized_page_limit` is set. `paginate(PageInput)` follows the same
host rule: `EntityQuery::paginate(page)` read through `fetch_all`,
`fetch_all_with_auth`, `fetch_all_on`, `fetch_one*`, `fetch_connection`,
`fetch_connection_with_auth` or `fetch_connection_selected_with_auth`, and
`EntitySearchQuery::paginate(page)` read through any fetch, honour
`page.limit` exactly or fail with `PAGE_LIMIT_EXCEEDED` (a negative limit with
`INVALID_INPUT`), whatever `refuse_oversized_page_limit` says. Before the
breaking release these two reduced the limit to the maximum without saying
so. To read more rows a page from trusted code, give the read its own
maximum: `EntityQuery::<Record>::new().with_max_limit(n)` before
`fetch_authorized_keyset(&db, None, None, page)` or `paginate(page)` and a
`fetch_connection*` call, or `.pagination_config(config.with_max_limit(Some(n)))`
on a search. A handle with a raised maximum works too:
`db.clone().with_pagination_config(db.pagination_config().with_max_limit(Some(n)))`.
Either way the page is bounded by `n`, and `pageInfo.hasNextPage` and the end
cursor page through a larger set.

### Page offset: bounded at 10,000 by default

Every backend reads and discards the rows an offset skips, so a client that
sends `page: { offset: 1000000000 }` costs a scan of that many rows. Since the
breaking release a `Database` refuses a client offset above
`PaginationConfig::DEFAULT_MAX_OFFSET` (`Some(10_000)`, the value of
`PaginationConfig::RECOMMENDED_MAX_OFFSET`) with `PAGE_LIMIT_EXCEEDED`, the
code of a refused page limit, before any row is read. Release 0.169.0 added
the bound as an opt-in; it was unbounded by default until the breaking
release. The message is `requested offset N exceeds the maximum page offset
M`. Set another maximum with `DatabaseBuilder::max_page_offset(Some(n))` (or
`Database::with_max_page_offset(Some(n))`), or opt out with
`max_page_offset(None)` to leave client offsets unbounded; page deeper with
keyset cursors. A `ClientPaginationConfig` built directly from a
`PaginationConfig`, as a pool or transaction without a `Database` uses, has no
offset maximum unless `with_max_offset` sets one. The bound applies on SQLite, PostgreSQL and SQL Server to the same
pages as the limit refusal: list roots, to-many relations and search
connections, custom resolvers that use
`ClientPaginationConfig::resolve_client_page` (or `admit_client_offset`), and
the Rust reads that take a `PageInput` (`EntityQuery::paginate(page)` through
`fetch_connection*`, and `EntitySearchQuery::paginate(page)`). An
offset that trusted code sets with a builder's own `offset(n)`, including
`search_db(...).offset(n)` and `EntitySearchQuery::offset`, is not bounded;
`paginate` after `offset` makes the page's offset a client offset again.
Keyset connections take no offset.

### What a keyset cursor is bound to

A generated keyset cursor (`gomk1.`) holds the order values of one row. It is
hex-encoded JSON with a checksum that uses no secret. The checksum catches
corruption and the bindings below; it does not authenticate the cursor. A
client can read the order values in a cursor it was given, and can write a
cursor with other values and a recomputed checksum. Such a cursor is accepted
and only moves the resume position within the same order: every page is read
under the caller's entity, row and field policies, exactly as a first page is,
so a rewritten cursor returns no row the caller could not already list.
Ordering by a field therefore discloses its values to whoever receives the
cursor; `keyset_cursor_field_authority.rs` covers the order-field checks.
Treat a cursor as a position, never as an authorization token.
`a_rewritten_cursor_only_moves_the_position_and_row_policy_still_applies` in
`crates/graphql-orm/tests/keyset_cursor_binding.rs` pins this. The cursor
length is bounded only by the request size the host accepts.

A cursor is refused with `CURSOR_INVALID` unless it matches all of:

| Bound to | Mismatch |
| --- | --- |
| The entity (for the configured order) | A cursor from another entity with the same key columns |
| The complete effective order, including `orderBy` and the primary-key suffix | A different order, direction or null placement |
| The caller-visible filter, when one was supplied | A different filter, a filter where there was none, or none where there was one |

Only a SHA-256 of the filter is bound: neither the cursor nor the error contains
filter contents. An empty filter is the same query as no filter and shares its
cursors. The binding covers generated `recordsKeyset`, `keyset_page`,
`keyset_connection_page` and their `_ordered` forms, `recordsScan` continuations
(which are also encrypted), and the `MutationContext` keyset methods, which all
use the same function, `bind_keyset_fingerprint`. Private authorization
predicates are not part of the filter, so changing a caller's visibility never
invalidates a cursor by itself; current policy applies to every resumed page.

Three paths intentionally stay unbound:

- `RepositoryEntity` types and handwritten `DatabaseFilter` implementations
  return no `keyset_filter_identity`, because the ORM does not own their filter
  shape. Their cursors are bound to entity and order only. A caller that needs
  filter binding on them must keep the filter fixed while paging.
- Runtime GraphQL cursors with `RuntimeCursorProfile::Unprotected` are readable
  and unauthenticated; only the default protected profile binds the filter.
  The protected profile authenticates a digest of the caller's `where` input
  (before the host's grant is applied) as associated data, and so refuses a
  changed filter with the usual `invalid_cursor`. Key order in the `where`
  object is part of that digest.
- `RuntimeReadRequest` built directly by a host receives one combined predicate
  (filter and grant), so it cannot tell the caller's filter from the host's.
  Hosts that page through it should bind their own filter, for example in the
  cursor protection they already supply. Offset cursors (ordinary list,
  search and relation connections) are positions, not keys, and are not
  bound to anything.

#### Value drift is inherent to keyset paging

A cursor records the order values of its row, not the row. Keyset pagination
resumes "after these values", so a cursor taken before a row's order values
change is still accepted, and the next page reflects the new values: a row
moved before the cursor is not returned again, a row moved after it is returned
(possibly a second time), and a deleted cursor row changes nothing. Only a
cursor that is malformed, corrupted, or bound to something else (above) is
refused. The ORM does not try to detect this; doing so would need row versions
or snapshots, which keyset pagination exists to avoid. Clients that need a
stable view should page inside one transaction or order by an immutable column
such as a creation time. `value_drift_after_the_cursor_is_accepted_and_resumes_by_value`
in `crates/graphql-orm/tests/keyset_cursor_binding.rs` pins this behaviour.

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

### Context-bound authorized pages for handwritten resolvers (0.162.0)

`EntityQuery::fetch_authorized_page` is the bounded scan above for a
handwritten GraphQL resolver or query builder whose pages belong to a
server-side context: for example a private cache or another server-owned read
model that must not be continued across a different committed snapshot, source
registration, workspace or authority. The generated roots and
`fetch_authorized_scan` are unchanged, and neither API enables the other.

```rust,ignore
// Start-up: load the service-owned 32-byte secret from the service's
// encrypted settings.
let database = database.with_authorized_page_config(
    AuthorizedPageConfig::new(AuthorizedPageKeys::new("2026-11", page_key)?, 256, 4096)?
        .with_max_page_size(100)?,
);

// Every request, in the resolver: state the context from server state only.
let binding = AuthorizedPageBinding::builder()
    .component("snapshot-generation", snapshot.generation_bytes())
    .component("source-registration", source.registration_id())
    .component("source-incarnation", source.incarnation())
    .component("workspace", workspace.id())
    .component("original-principal", session.original_principal())
    .component("current-principal", session.current_principal())
    .component("grants", session.grants_digest())
    .component("credential-revision", source.credential_revision())
    .component("configuration-revision", source.configuration_revision())
    .component("placement-revision", source.placement_revision())
    .build()
    .map_err(AuthorizedPageBindingError::into_graphql_error)?;
let page = EntityQuery::<Record, SqliteBackend>::new()
    .filter_with_entity_matching(&filter)
    .fetch_authorized_page(
        db,
        Some(ctx),
        auth,
        AuthorizedPageRequest::new(binding).after(after).limit(limit),
    )
    .await?;
```

The result is the `AuthorizedScanPage` of the scan API. `pageInfo.status` keeps
the `EXHAUSTED`, `PAGE_FULL` and `BUDGET_EXHAUSTED` meanings in the table above,
`continuation` must be passed back as `after` even after an empty page, and
`totalCount` is always absent.

**The binding.** `AuthorizedPageBinding` is the server's statement of the
context. Build it on every request from trusted server state, never from a
GraphQL argument, a client-controlled header or any other browser value, and
include everything that must end a page sequence when it changes; the ORM
cannot tell that a binding is incomplete. It is opaque to the ORM, at most
4096 bytes (`AUTHORIZED_PAGE_MAX_BINDING_BYTES`), and its `Debug` output is
redacted. The builder length-delimits up to 32 named components and binds
their names and order, so add the same components in the same order every
time; it refuses empty names, empty values and repeated names, so state an
absent value explicitly. `AuthorizedPageBinding::from_bytes` takes bytes the
server already keeps in canonical form.

**The cursor.** A continuation is `gomp1.<key id>.<sealed bytes>`: codec
version 1, the public identifier of the sealing key, and the
XChaCha20-Poly1305 ciphertext of the scan position and an expiry time under a
random nonce. The binding, the entity name, the key identifier and the
fingerprint of the keyset order and caller-visible filter are authenticated as
associated data; the binding is not in the cursor. The codec is
internal: there is no public encoder, decoder or signer hook, and a later
codec version gets a new prefix rather than reinterpreting `gomp1` cursors.

A continuation is refused with `CURSOR_INVALID`, before any count or row is
read, when it was issued under a different binding, entity, order, filter or
key, was altered or truncated, has another prefix (including `goms1` scan
cursors), or is longer than the configured bound. These cases are
indistinguishable to the caller. Two more cases carry a `reason` extension
because they concern a cursor that is otherwise acceptable. Expiry is
reported only for a cursor that authenticates under the same binding, entity,
order, filter and key:

| `reason` | Meaning | Client action |
| --- | --- | --- |
| `AUTHORIZED_PAGE_CURSOR_EXPIRED` | Authenticated under the current binding, past its lifetime | Restart from the first page |
| `AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE` | Names a key identifier this service does not accept | Restart from the first page |

Errors carry the default message for their code and never the cursor, the
binding, a position value or key material.

**Authorization.** A continuation is a position, never an authorization.
Every request resolves entity access and read visibility again before the
cursor is opened, then row callbacks and field policies for every row it
returns, exactly as for a first page. A
row or field denied after a cursor was issued is not released by it, and a
cursor presented by another caller under an unchanged binding gets that
caller's own decisions.

**Bounds.**

| Bound | Setting | Default |
| --- | --- | --- |
| Rows per statement | `AuthorizedPageConfig::new(.., batch_size, ..)` | required |
| Candidates examined per request | `AuthorizedPageConfig::new(.., max_scanned_rows)` | required |
| Page size | `with_max_page_size` | 100 |
| Cursor bytes | `with_max_cursor_bytes` (256 to 65536) | 8192 |
| Cursor lifetime | `with_cursor_ttl` (1 millisecond to 30 days) | 1 hour |
| Binding bytes | fixed | 4096 |

A `limit` above the page-size bound is refused with `PAGE_LIMIT_EXCEEDED`; it
is not truncated. A request without a limit gets the smaller of the bound and
100. `PaginationConfig` limits do not apply to this API. A position too large
to seal within the cursor bound fails the request with
`AUTHORIZATION_MISCONFIGURED` rather than issuing an oversized cursor. Each
page issues a fresh continuation, so the lifetime bounds the pause between two
requests rather than the whole traversal; replicas compare it with their own
clocks.

**Key custody and rotation.** The key is a secret, randomly generated 32 bytes
owned by the service. Keep it in the service's encrypted settings (or its
root-secret store) and load it at start-up. Never derive it from a provider
credential, a user or tenant secret or any value a client can see or choose,
and never send it to a client. Replicas that serve the same pages need the
same keys under the same identifiers; a replica with different key bytes
refuses the others' cursors. `AuthorizedPageKeys` holds one active key, which
seals, and up to seven accept-only keys:

1. Add the new key with `accepting` on every replica.
2. Make it the active key and keep the previous key with `accepting` for at
   least the cursor lifetime.
3. Remove the previous key. Its cursors are then refused with
   `AUTHORIZED_PAGE_CURSOR_KEY_UNAVAILABLE`.

Without an installed `AuthorizedPageConfig` the call fails closed with
`AUTHORIZATION_MISCONFIGURED`. Residual caller filters are refused, as for
scans. Coverage on SQLite, PostgreSQL and SQL Server is in
[`authorized_page_context_binding.rs`](../../../crates/graphql-orm/tests/authorized_page_context_binding.rs).

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
